use std::ffi::CString;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

//// IMPORTANT!! THIS MUST MATCH THE STRUCT DEFINED IN hydra/src/remote/shmdata.h
// Keep the control fields naturally aligned: req/ack are accessed atomically on
// both sides of the shared-memory ABI and AtomicU64 requires 8-byte alignment.
#[repr(C)]
#[derive(Debug)]
pub struct ShmDataRaw {
  // Legacy Hydra controls and register payload.  Never move these fields: old
  // controllers use this prefix as their complete 64-byte ABI.
  pub init: u32,
  pub end: u32,
  pub pid: u32,
  pub reserved0: u32,
  pub req: u64,  // request step by incrementing
  pub ack: u64,  // ack step by matching 'req' value

  // registers
  pub ax: u16,
  pub bx: u16,
  pub cx: u16,
  pub dx: u16,
  pub si: u16,
  pub di: u16,
  pub bp: u16,
  pub sp: u16,
  pub ip: u16,
  pub cs: u16,
  pub ds: u16,
  pub es: u16,
  pub ss: u16,
  pub flags: u16,
  pub legacy_reserved1: u32,

  // dosemu2 validator extension.  New metadata is appended to preserve the
  // established offsets above.
  pub abi_version: u32,
  pub struct_size: u32,
  pub runtime_psp: u16,
  pub reserved1: u16,
  pub decoded_instructions: u32,
  pub step_flags: u32,

  // memory
  // TODO...
}

// Keep the Rust side pinned to the C ABI in hydra/src/remote/shmdata.h.
static_assertions::const_assert_eq!(std::mem::size_of::<ShmDataRaw>(), 88);
static_assertions::const_assert_eq!(std::mem::align_of::<ShmDataRaw>(), 8);
static_assertions::const_assert_eq!(std::mem::offset_of!(ShmDataRaw, pid), 8);
static_assertions::const_assert_eq!(std::mem::offset_of!(ShmDataRaw, req), 16);
static_assertions::const_assert_eq!(std::mem::offset_of!(ShmDataRaw, ax), 32);
static_assertions::const_assert_eq!(std::mem::offset_of!(ShmDataRaw, abi_version), 64);

// These macros are for snapshot payload fields only. Synchronization/control
// fields (init/end/req/ack) must use the atomic accessors on ShmData.
#[macro_export]
macro_rules! shmdata_read {
  ($dat:expr, $field:ident) => {
    unsafe {
      let _typecheck: &ShmData = &$dat; // type check
      std::ptr::read_unaligned(std::ptr::addr_of!((*$dat.raw).$field))
    }
  };
}

#[macro_export]
macro_rules! shmdata_write {
  ($dat:expr, $field:ident, $val:expr) => {
    unsafe {
      let _typecheck: &ShmData = &$dat; // type check
      std::ptr::write_unaligned(std::ptr::addr_of_mut!((*$dat.raw).$field), $val)
    }
  };
}

pub struct ShmData {
  pub raw: *mut ShmDataRaw,
}

impl ShmData {
  fn size() -> usize {
    (std::mem::size_of::<ShmDataRaw>() + 4095) & !4095
  }

  fn load_u32(&self, ptr: *mut u32, ordering: Ordering) -> u32 {
    debug_assert_eq!((ptr as usize) % std::mem::align_of::<AtomicU32>(), 0);
    unsafe { (&*(ptr as *const AtomicU32)).load(ordering) }
  }

  fn store_u32(&self, ptr: *mut u32, value: u32, ordering: Ordering) {
    debug_assert_eq!((ptr as usize) % std::mem::align_of::<AtomicU32>(), 0);
    unsafe { (&*(ptr as *const AtomicU32)).store(value, ordering) }
  }

  fn load_u64(&self, ptr: *mut u64, ordering: Ordering) -> u64 {
    debug_assert_eq!((ptr as usize) % std::mem::align_of::<AtomicU64>(), 0);
    unsafe { (&*(ptr as *const AtomicU64)).load(ordering) }
  }

  fn store_u64(&self, ptr: *mut u64, value: u64, ordering: Ordering) {
    debug_assert_eq!((ptr as usize) % std::mem::align_of::<AtomicU64>(), 0);
    unsafe { (&*(ptr as *const AtomicU64)).store(value, ordering) }
  }

  pub fn load_init(&self, ordering: Ordering) -> u32 {
    let ptr = unsafe { std::ptr::addr_of_mut!((*self.raw).init) };
    self.load_u32(ptr, ordering)
  }

  pub fn store_end(&self, value: u32, ordering: Ordering) {
    let ptr = unsafe { std::ptr::addr_of_mut!((*self.raw).end) };
    self.store_u32(ptr, value, ordering);
  }

  pub fn load_req(&self, ordering: Ordering) -> u64 {
    let ptr = unsafe { std::ptr::addr_of_mut!((*self.raw).req) };
    self.load_u64(ptr, ordering)
  }

  pub fn store_req(&self, value: u64, ordering: Ordering) {
    let ptr = unsafe { std::ptr::addr_of_mut!((*self.raw).req) };
    self.store_u64(ptr, value, ordering);
  }

  pub fn load_ack(&self, ordering: Ordering) -> u64 {
    let ptr = unsafe { std::ptr::addr_of_mut!((*self.raw).ack) };
    self.load_u64(ptr, ordering)
  }

  pub fn load_step_flags(&self, ordering: Ordering) -> u32 {
    let ptr = unsafe { std::ptr::addr_of_mut!((*self.raw).step_flags) };
    self.load_u32(ptr, ordering)
  }

  pub fn attach(path: &str) -> Result<Self, String> {
    let size = Self::size();
    let cpath = CString::new(path).map_err(|e| e.to_string())?;

    let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDWR, 0o600u32) };
    if fd < 0 {
      return Err(last_os_error("open"));
    }

    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } < 0 {
      let err = last_os_error("fstat");
      unsafe { libc::close(fd); }
      return Err(err);
    }
    if stat.st_size < size as libc::off_t {
      unsafe { libc::close(fd); }
      return Err(format!("shared memory is not ready: {} bytes, need {}", stat.st_size, size));
    }

    let addr = unsafe {
      libc::mmap(
        ptr::null_mut(),
        size,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_SHARED,
        fd,
        0,
      )
    };
    if addr == libc::MAP_FAILED {
      unsafe { libc::close(fd); }
      return Err(last_os_error("mmap"));
    }

    unsafe { libc::close(fd); }

    let raw = addr as *mut ShmDataRaw;
    Ok(Self { raw })
  }

  // /// Get a typed pointer into the shared region at a byte offset.
  // /// Caller must ensure alignment, bounds, and that no conflicting references exist.
  // pub unsafe fn as_ptr<T>(&self, offset: usize) -> *mut T {
  //     (self.addr as *mut u8).add(offset) as *mut T
  // }

  // /// Get the region as a mutable byte slice.
  // pub unsafe fn as_bytes_mut(&mut self) -> &mut [u8] {
  //     std::slice::from_raw_parts_mut(self.addr as *mut u8, self.size)
  // }
}

impl Drop for ShmData {
  fn drop(&mut self) {
    unsafe {
      let raw: *mut libc::c_void = self.raw as *mut _;
      let size = Self::size();
      if !raw.is_null() && raw != libc::MAP_FAILED {
        libc::munmap(raw, size);
      }
    }
  }
}

fn last_os_error(context: &str) -> String {
  let err = std::io::Error::last_os_error();
  format!("{}: {}", context, err)
}
