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
const ABI_SIZE: usize = 88;
const ABI_VERSION: u32 = 1;

const OFF_INIT: usize = 0;
const OFF_END: usize = 4;
const OFF_REQ: usize = 16;
const OFF_ACK: usize = 24;
const OFF_AX: usize = 32;
const OFF_DX: usize = 38;
const OFF_SI: usize = 40;
const OFF_IP: usize = 48;
const OFF_CS: usize = 50;
const OFF_DS: usize = 52;
const OFF_ABI_VERSION: usize = 64;
const OFF_STRUCT_SIZE: usize = 68;
const OFF_RUNTIME_PSP: usize = 72;
const OFF_DECODED: usize = 76;
const OFF_STEP_FLAGS: usize = 80;

const DIIS_STEP_END_ACK: u32 = 1 << 3;
const DIIS_STEP_TARGET_EXIT: u32 = 1 << 4;
const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_SHARED: i32 = 0x01;

extern "C" {
    fn mmap(addr: *mut c_void, length: usize, prot: i32, flags: i32, fd: i32, offset: isize)
        -> *mut c_void;
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
                            mmap(ptr::null_mut(), len, PROT_READ | PROT_WRITE,
                                MAP_SHARED, file.as_raw_fd(), 0)
                        };
                        if mapped as isize == -1 {
                            return Err(io::Error::last_os_error());
                        }
                        return Ok(Self { _file: file, ptr: mapped.cast(), len });
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut,
                    format!("timed out waiting for {path}")));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    unsafe fn read_u16(&self, off: usize) -> u16 {
        u16::from_le(ptr::read_volatile(self.ptr.add(off).cast::<u16>()))
    }
    unsafe fn read_u32(&self, off: usize) -> u32 {
        u32::from_le(ptr::read_volatile(self.ptr.add(off).cast::<u32>()))
    }
    unsafe fn write_u16(&self, off: usize, value: u16) {
        ptr::write_volatile(self.ptr.add(off).cast::<u16>(), value.to_le());
    }
    unsafe fn atomic_u32(&self, off: usize) -> &AtomicU32 {
        &*self.ptr.add(off).cast::<AtomicU32>()
    }
    unsafe fn atomic_u64(&self, off: usize) -> &AtomicU64 {
        &*self.ptr.add(off).cast::<AtomicU64>()
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { let _ = munmap(self.ptr.cast(), self.len); }
    }
}

fn wait_until(deadline: Instant, mut f: impl FnMut() -> bool, what: &str) -> Result<(), String> {
    while !f() {
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {what}"));
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn snapshot(c: &Mapping) -> String {
    unsafe {
        format!("req={} ack={} flags={:#x} decoded={} cs:ip={:04x}:{:04x} ax={:04x} dx={:04x} si={:04x}",
            c.atomic_u64(OFF_REQ).load(Ordering::Acquire),
            c.atomic_u64(OFF_ACK).load(Ordering::Acquire),
            c.atomic_u32(OFF_STEP_FLAGS).load(Ordering::Acquire),
            c.read_u32(OFF_DECODED), c.read_u16(OFF_CS), c.read_u16(OFF_IP),
            c.read_u16(OFF_AX), c.read_u16(OFF_DX), c.read_u16(OFF_SI))
    }
}

fn request(c: &Mapping, value: u64, deadline: Instant) -> Result<(), String> {
    unsafe { c.atomic_u64(OFF_REQ) }.store(value, Ordering::Release);
    wait_until(deadline,
        || unsafe { c.atomic_u64(OFF_ACK) }.load(Ordering::Acquire) == value,
        &format!("ack {value}"))
        .map_err(|e| format!("{e}; {}", snapshot(c)))
}

fn expect_single(c: &Mapping, ip: u16) -> Result<(), String> {
    let decoded = unsafe { c.read_u32(OFF_DECODED) };
    let actual_ip = unsafe { c.read_u16(OFF_IP) };
    let flags = unsafe { c.atomic_u32(OFF_STEP_FLAGS) }.load(Ordering::Acquire);
    if decoded != 1 || actual_ip != ip || flags & (DIIS_STEP_END_ACK | DIIS_STEP_TARGET_EXIT) != 0 {
        return Err(format!("expected single instruction ending at IP={ip:04x}; {}", snapshot(c)));
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let control = Mapping::open(CONTROL_PATH, ABI_SIZE, deadline)
        .map_err(|e| format!("map control: {e}"))?;
    wait_until(deadline,
        || unsafe { control.atomic_u32(OFF_INIT) }.load(Ordering::Acquire) == 1,
        "validator init")?;

    let abi = unsafe { control.read_u32(OFF_ABI_VERSION) };
    let size = unsafe { control.read_u32(OFF_STRUCT_SIZE) } as usize;
    let psp = unsafe { control.read_u16(OFF_RUNTIME_PSP) };
    let target_cs = unsafe { control.read_u16(OFF_CS) };
    if abi != ABI_VERSION || size != ABI_SIZE || psp == 0 {
        return Err(format!("invalid initialized control block; {}", snapshot(&control)));
    }

    let lowmem = Mapping::open(LOWMEM_PATH, 0x184, deadline)
        .map_err(|e| format!("map lowmem: {e}"))?;

    // AH=25h consumes DS:DX. Keep DS in target-owned memory and give SI a
    // sentinel proving the first post-service instruction has not run yet.
    unsafe {
        control.write_u16(OFF_DS, target_cs);
        control.write_u16(OFF_SI, 0x1234);
    }

    request(&control, 1, deadline)?;
    expect_single(&control, 0x0003)?;
    if unsafe { control.read_u16(OFF_DX) } != 0x000a {
        return Err(format!("MOV DX did not execute; {}", snapshot(&control)));
    }

    request(&control, 2, deadline)?;
    expect_single(&control, 0x0006)?;
    if unsafe { control.read_u16(OFF_AX) } != 0x2560 {
        return Err(format!("MOV AX did not execute; {}", snapshot(&control)));
    }

    // This is the semantic gate: the ACK must describe the returned target
    // boundary, not DOS handler entry, and MOV SI,AX must still be pending.
    request(&control, 3, deadline)?;
    expect_single(&control, 0x0008)?;
    if unsafe { control.read_u16(OFF_CS) } != target_cs || unsafe { control.read_u16(OFF_SI) } != 0x1234 {
        return Err(format!("interrupt acknowledged at the wrong boundary; {}", snapshot(&control)));
    }

    let vector = 0x60usize * 4;
    let ivt_off = unsafe { lowmem.read_u16(vector) };
    let ivt_seg = unsafe { lowmem.read_u16(vector + 2) };
    if (ivt_off, ivt_seg) != (0x000a, target_cs) {
        return Err(format!("DOS service had not completed at ACK: vector 60h={ivt_seg:04x}:{ivt_off:04x}"));
    }

    let returned_ax = unsafe { control.read_u16(OFF_AX) };
    request(&control, 4, deadline)?;
    expect_single(&control, 0x000a)?;
    if unsafe { control.read_u16(OFF_SI) } != returned_ax {
        return Err(format!("post-service MOV SI,AX was not isolated to the next request; {}", snapshot(&control)));
    }

    unsafe { control.atomic_u32(OFF_END) }.store(1, Ordering::Release);
    wait_until(deadline,
        || unsafe { control.atomic_u32(OFF_STEP_FLAGS) }.load(Ordering::Acquire) & DIIS_STEP_END_ACK != 0,
        "END_ACK")
        .map_err(|e| format!("{e}; {}", snapshot(&control)))?;

    println!("handler-return runtime probe passed: PSP={psp:04x}, target CS={target_cs:04x}");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("dosemu2 handler-return probe failed: {e}");
        std::process::exit(1);
    }
}
