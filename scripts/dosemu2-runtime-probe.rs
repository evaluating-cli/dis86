use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const CONTROL_PATH: &str = "/dev/shm/hydra_remote";
const LOWMEM_PATH: &str = "/dev/shm/dosemu_mem";
const ABI_VERSION: u32 = 1;
const ABI_SIZE: usize = 88;

const OFF_ABI_VERSION: usize = 64;
const OFF_STRUCT_SIZE: usize = 68;
const OFF_INIT: usize = 0;
const OFF_END: usize = 4;
const OFF_RUNTIME_PSP: usize = 72;
const OFF_REQ: usize = 16;
const OFF_ACK: usize = 24;
const OFF_DECODED: usize = 76;
const OFF_STEP_FLAGS: usize = 80;
const OFF_AX: usize = 32;
const OFF_BX: usize = 34;
const OFF_DX: usize = 38;
const OFF_IP: usize = 48;
const OFF_CS: usize = 50;

const DIIS_STEP_END_ACK: u32 = 1 << 3;
const DIIS_STEP_FAULT: u32 = 1 << 2;
const DIIS_STEP_TARGET_EXIT: u32 = 1 << 4;

const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_SHARED: i32 = 0x01;

extern "C" {
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: isize,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: usize) -> i32;
}

struct Mapping {
    _file: File,
    ptr: *mut u8,
    len: usize,
}

impl Mapping {
    fn open(path: &str, min_len: usize, deadline: Instant) -> io::Result<Self> {
        loop {
            match OpenOptions::new().read(true).write(true).open(path) {
                Ok(file) => {
                    let len = file.metadata()?.len() as usize;
                    if len >= min_len {
                        let mapped = unsafe {
                            mmap(
                                ptr::null_mut(),
                                len,
                                PROT_READ | PROT_WRITE,
                                MAP_SHARED,
                                file.as_raw_fd(),
                                0,
                            )
                        };
                        if mapped as isize == -1 {
                            return Err(io::Error::last_os_error());
                        }
                        return Ok(Self {
                            _file: file,
                            ptr: mapped.cast(),
                            len,
                        });
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for {path}"),
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    unsafe fn read_u16(&self, off: usize) -> u16 {
        assert!(off + 2 <= self.len);
        u16::from_le(ptr::read_volatile(self.ptr.add(off).cast::<u16>()))
    }

    unsafe fn read_u32(&self, off: usize) -> u32 {
        assert!(off + 4 <= self.len);
        u32::from_le(ptr::read_volatile(self.ptr.add(off).cast::<u32>()))
    }

    unsafe fn write_u16(&self, off: usize, value: u16) {
        assert!(off + 2 <= self.len);
        ptr::write_volatile(self.ptr.add(off).cast::<u16>(), value.to_le());
    }

    unsafe fn atomic_u32(&self, off: usize) -> &AtomicU32 {
        assert!(off % std::mem::align_of::<AtomicU32>() == 0);
        assert!(off + std::mem::size_of::<AtomicU32>() <= self.len);
        &*self.ptr.add(off).cast::<AtomicU32>()
    }

    unsafe fn atomic_u64(&self, off: usize) -> &AtomicU64 {
        assert!(off % std::mem::align_of::<AtomicU64>() == 0);
        assert!(off + std::mem::size_of::<AtomicU64>() <= self.len);
        &*self.ptr.add(off).cast::<AtomicU64>()
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = munmap(self.ptr.cast(), self.len);
        }
    }
}

fn stage(name: &str) {
    eprintln!("PROBE_STAGE={name}");
}

fn wait_until(deadline: Instant, mut predicate: impl FnMut() -> bool, what: &str) -> Result<(), String> {
    while !predicate() {
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {what}"));
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn control_snapshot(control: &Mapping) -> String {
    unsafe {
        format!(
            "init={} end={} runtime_psp={:04x} req={} ack={} decoded={} step_flags={:#010x} ax={:04x} cs={:04x} ip={:04x}",
            control.atomic_u32(OFF_INIT).load(Ordering::Acquire),
            control.atomic_u32(OFF_END).load(Ordering::Acquire),
            control.read_u16(OFF_RUNTIME_PSP),
            control.atomic_u64(OFF_REQ).load(Ordering::Acquire),
            control.atomic_u64(OFF_ACK).load(Ordering::Acquire),
            control.read_u32(OFF_DECODED),
            control.atomic_u32(OFF_STEP_FLAGS).load(Ordering::Acquire),
            control.read_u16(OFF_AX),
            control.read_u16(OFF_CS),
            control.read_u16(OFF_IP),
        )
    }
}

fn step(
    control: &Mapping,
    req_value: u64,
    expected_ip: u16,
    expected_ax: u16,
    deadline: Instant,
) -> Result<(), String> {
    let req = unsafe { control.atomic_u64(OFF_REQ) };
    let ack = unsafe { control.atomic_u64(OFF_ACK) };
    let step_flags = unsafe { control.atomic_u32(OFF_STEP_FLAGS) };

    eprintln!("PROBE_STAGE=request_{req_value}_publish");
    req.store(req_value, Ordering::Release);
    eprintln!("PROBE_STAGE=request_{req_value}_wait_ack");
    wait_until(
        deadline,
        || {
            let flags = step_flags.load(Ordering::Acquire);
            if flags & DIIS_STEP_TARGET_EXIT != 0 {
                return true;
            }
            ack.load(Ordering::Acquire) == req_value
        },
        &format!("ack {req_value}"),
    )
    .map_err(|e| format!("{e}; {}", control_snapshot(control)))?;

    let flags = step_flags.load(Ordering::Acquire);
    if flags & DIIS_STEP_TARGET_EXIT != 0 {
        return Err(format!(
            "target exited while waiting for request {req_value}; {}",
            control_snapshot(control)
        ));
    }
    if flags & DIIS_STEP_END_ACK != 0 {
        return Err(format!(
            "unexpected END_ACK while waiting for request {req_value}; {}",
            control_snapshot(control)
        ));
    }

    eprintln!("PROBE_STAGE=request_{req_value}_validate");
    let decoded = unsafe { control.read_u32(OFF_DECODED) };
    let ax = unsafe { control.read_u16(OFF_AX) };
    let ip = unsafe { control.read_u16(OFF_IP) };
    if decoded != 1 {
        return Err(format!(
            "request {req_value}: expected one decoded instruction, got {decoded}; {}",
            control_snapshot(control)
        ));
    }
    if ax != expected_ax || ip != expected_ip {
        return Err(format!(
            "request {req_value}: expected AX={expected_ax:04x} IP={expected_ip:04x}, got AX={ax:04x} IP={ip:04x}; {}",
            control_snapshot(control)
        ));
    }
    Ok(())
}

fn run_end_barrier() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);

    stage("open_control");
    let control = Mapping::open(CONTROL_PATH, ABI_SIZE, deadline)
        .map_err(|e| format!("map control: {e}"))?;

    let init = unsafe { control.atomic_u32(OFF_INIT) };
    let end = unsafe { control.atomic_u32(OFF_END) };
    let step_flags = unsafe { control.atomic_u32(OFF_STEP_FLAGS) };

    stage("wait_init");
    wait_until(deadline, || init.load(Ordering::Acquire) == 1, "validator init")
        .map_err(|e| format!("{e}; {}", control_snapshot(&control)))?;

    stage("validate_control");
    let abi = unsafe { control.read_u32(OFF_ABI_VERSION) };
    let struct_size = unsafe { control.read_u32(OFF_STRUCT_SIZE) } as usize;
    if abi != ABI_VERSION || struct_size != ABI_SIZE {
        return Err(format!(
            "unexpected control ABI: version={abi}, size={struct_size}; {}",
            control_snapshot(&control)
        ));
    }

    let runtime_psp = unsafe { control.read_u16(OFF_RUNTIME_PSP) };
    if runtime_psp == 0 {
        return Err(format!(
            "runtime PSP was not published; {}",
            control_snapshot(&control)
        ));
    }
    let image_seg = runtime_psp.wrapping_add(0x10);
    let expected_cs = unsafe { control.read_u16(OFF_CS) };
    if expected_cs != image_seg {
        return Err(format!(
            "expected entry CS={image_seg:04x} from PSP={runtime_psp:04x}, got {expected_cs:04x}; {}",
            control_snapshot(&control)
        ));
    }

    stage("open_lowmem");
    let lowmem = Mapping::open(LOWMEM_PATH, 2, deadline)
        .map_err(|e| format!("map low memory: {e}; {}", control_snapshot(&control)))?;
    let sentinel = ((image_seg as usize) << 4) + 0x20;
    if sentinel + 2 > lowmem.len {
        return Err(format!(
            "sentinel linear address {sentinel:#x} exceeds /dosemu_mem size {:#x}",
            lowmem.len
        ));
    }

    stage("validate_initial_sentinel");
    let loaded = unsafe { lowmem.read_u16(sentinel) };
    if loaded != 0x1111 {
        return Err(format!(
            "probe image sentinel not visible through /dosemu_mem: expected 1111, got {loaded:04x}"
        ));
    }

    // External -> guest: the next guest instruction must read this value.
    stage("alias_external_to_guest");
    unsafe { lowmem.write_u16(sentinel, 0x1234) };
    step(&control, 1, 0x0004, 0x1234, deadline)?;

    // Pure register step.
    stage("register_step");
    step(&control, 2, 0x0005, 0x1235, deadline)?;

    // Guest -> external: store AX into the same shared low-memory word.
    stage("alias_guest_to_external");
    step(&control, 3, 0x0009, 0x1235, deadline)?;
    let guest_written = unsafe { lowmem.read_u16(sentinel) };
    if guest_written != 0x1235 {
        return Err(format!(
            "guest write not visible through /dosemu_mem: expected 1235, got {guest_written:04x}"
        ));
    }

    // Prepare AH=30h/AL=00h (get DOS version) at a controlled boundary.
    stage("prepare_dos_service");
    step(&control, 4, 0x000c, 0x3000, deadline)?;

    // The interrupt leaves target-owned code while DOS services it.  The ACK
    // must remain outstanding until control has returned to the instruction at
    // IP=000eh in the target image.  The previous implementation ACKed at the
    // handler entry and therefore published a non-target CS:IP here.
    stage("dos_service_return");
    request(&control, 5, deadline)?;
    let service_flags = step_flags.load(Ordering::Acquire);
    let service_decoded = unsafe { control.read_u32(OFF_DECODED) };
    let service_ax = unsafe { control.read_u16(OFF_AX) };
    let service_cs = unsafe { control.read_u16(OFF_CS) };
    let service_ip = unsafe { control.read_u16(OFF_IP) };
    if service_flags != 0
        || service_decoded != 1
        || service_cs != image_seg
        || service_ip != 0x000e
        || service_ax == 0x3000
    {
        return Err(format!(
            "DOS service was not acknowledged at the normalized post-service boundary; {}",
            control_snapshot(&control)
        ));
    }

    // Prove the target can consume the register result returned by DOS.
    stage("consume_dos_service_result");
    step(&control, 6, 0x0010, service_ax, deadline)?;
    let service_bx = unsafe { control.read_u16(OFF_BX) };
    if service_bx != service_ax {
        return Err(format!(
            "post-service AX was not consumable by target code: AX={service_ax:04x}, BX={service_bx:04x}; {}",
            control_snapshot(&control)
        ));
    }

    // The instruction at IP=0010h would increment the sentinel to 1236. End is
    // release-stored while dosemu is stopped at the pre-node barrier; END_ACK
    // must be acquire-visible without that instruction ever beginning.
    stage("end_request");
    end.store(1, Ordering::Release);
    stage("wait_end_ack");
    wait_until(
        deadline,
        || step_flags.load(Ordering::Acquire) & DIIS_STEP_END_ACK != 0,
        "END_ACK",
    )
    .map_err(|e| format!("{e}; {}", control_snapshot(&control)))?;

    stage("validate_end_barrier");
    let after_end = unsafe { lowmem.read_u16(sentinel) };
    if after_end != 0x1235 {
        return Err(format!(
            "instruction after end barrier executed: sentinel changed to {after_end:04x}"
        ));
    }
    thread::sleep(Duration::from_millis(50));
    let stable_after_end = unsafe { lowmem.read_u16(sentinel) };
    if stable_after_end != 0x1235 {
        return Err(format!(
            "sentinel changed after END_ACK: {stable_after_end:04x}"
        ));
    }

    stage("complete");
    println!(
        "runtime probe passed: PSP={runtime_psp:04x}, image={image_seg:04x}, sentinel={sentinel:#x}, service_AX={service_ax:04x}"
    );
    Ok(())
}

fn initialized_control(deadline: Instant) -> Result<Mapping, String> {
    stage("open_control");
    let control = Mapping::open(CONTROL_PATH, ABI_SIZE, deadline)
        .map_err(|e| format!("map control: {e}"))?;
    let init = unsafe { control.atomic_u32(OFF_INIT) };
    stage("wait_init");
    wait_until(deadline, || init.load(Ordering::Acquire) == 1, "validator init")
        .map_err(|e| format!("{e}; {}", control_snapshot(&control)))?;
    let abi = unsafe { control.read_u32(OFF_ABI_VERSION) };
    let size = unsafe { control.read_u32(OFF_STRUCT_SIZE) } as usize;
    if abi != ABI_VERSION || size != ABI_SIZE {
        return Err(format!("unexpected control ABI: version={abi}, size={size}"));
    }
    Ok(control)
}

fn request(control: &Mapping, value: u64, deadline: Instant) -> Result<(), String> {
    unsafe { control.atomic_u64(OFF_REQ) }.store(value, Ordering::Release);
    wait_until(
        deadline,
        || unsafe { control.atomic_u64(OFF_ACK) }.load(Ordering::Acquire) == value,
        &format!("ack {value}"),
    ).map_err(|e| format!("{e}; {}", control_snapshot(control)))
}

fn run_target_exit() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let control = initialized_control(deadline)?;
    step(&control, 1, 0x0003, 0x4c00, deadline)?;
    stage("request_terminal_publication");
    request(&control, 2, deadline)?;
    let flags = unsafe { control.atomic_u32(OFF_STEP_FLAGS) }.load(Ordering::Acquire);
    let decoded = unsafe { control.read_u32(OFF_DECODED) };
    let runtime_psp = unsafe { control.read_u16(OFF_RUNTIME_PSP) };
    if flags != DIIS_STEP_TARGET_EXIT || decoded != 0 || runtime_psp != 0 {
        return Err(format!("inconsistent TARGET_EXIT publication; {}", control_snapshot(&control)));
    }
    stage("validate_target_stays_inactive");
    thread::sleep(Duration::from_millis(100));
    if unsafe { control.read_u16(OFF_RUNTIME_PSP) } != 0
        || unsafe { control.atomic_u32(OFF_STEP_FLAGS) }.load(Ordering::Acquire) != DIIS_STEP_TARGET_EXIT
        || unsafe { control.atomic_u64(OFF_ACK) }.load(Ordering::Acquire) != 2
    {
        return Err(format!("target gate reactivated after exit; {}", control_snapshot(&control)));
    }
    println!("target-exit runtime probe passed");
    Ok(())
}

fn run_fault() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let control = initialized_control(deadline)?;
    step(&control, 1, 0x0003, 0x1234, deadline)?;
    step(&control, 2, 0x0005, 0x1234, deadline)?;
    step(&control, 3, 0x0007, 0x1234, deadline)?;
    stage("request_faulting_divide");
    request(&control, 4, deadline)?;
    let flags = unsafe { control.atomic_u32(OFF_STEP_FLAGS) }.load(Ordering::Acquire);
    let decoded = unsafe { control.read_u32(OFF_DECODED) };
    let ax = unsafe { control.read_u16(OFF_AX) };
    let bx = unsafe { control.read_u16(OFF_BX) };
    let dx = unsafe { control.read_u16(OFF_DX) };
    let ip = unsafe { control.read_u16(OFF_IP) };
    if flags & DIIS_STEP_FAULT == 0 || decoded != 1 || (ax, bx, dx, ip) != (0x1234, 0, 0, 0x0007) {
        return Err(format!("inconsistent post-fault publication; {}", control_snapshot(&control)));
    }
    stage("release_after_fault");
    unsafe { control.atomic_u32(OFF_END) }.store(1, Ordering::Release);
    wait_until(
        deadline,
        || unsafe { control.atomic_u32(OFF_STEP_FLAGS) }.load(Ordering::Acquire) & DIIS_STEP_END_ACK != 0,
        "END_ACK after fault",
    ).map_err(|e| format!("{e}; {}", control_snapshot(&control)))?;
    println!("fault runtime probe passed: decoded={decoded}, AX={ax:04x}, DX={dx:04x}, IP={ip:04x}");
    Ok(())
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "end-barrier".into());
    let result = match mode.as_str() {
        "end-barrier" => run_end_barrier(),
        "target-exit" => run_target_exit(),
        "fault" => run_fault(),
        _ => Err(format!("unknown probe mode {mode:?}; expected end-barrier, target-exit, or fault")),
    };
    if let Err(e) = result {
        eprintln!("dosemu2 runtime probe failed: {e}");
        std::process::exit(1);
    }
}
