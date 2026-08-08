use std::process::{Command, Child, Stdio};
use super::super::emu::Emu;
use super::super::cpu::*;
use super::super::cpu_flags::Flag;
use super::super::machine::Machine;
use super::super::value::Value;
use super::shmdata::ShmData;
use super::shmmem::ShmMem;
use crate::segoff::SegOff;
use crate::{shmdata_read, shmdata_write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const HYDRA_SHM_PATH: &str = "/dev/shm/hydra_remote";
const DOSBOX_MEM_PATH: &str = "/dev/shm/dosbox_mem";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

pub struct HydraProcess {
  hydra: Child,
  data: ShmData,
  #[allow(dead_code)]
  pub mem: ShmMem,

  cpu_state: Cpu,
  last_cpu_state: Cpu,
}

impl HydraProcess {
  fn remove_stale_mapping(path: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
      Ok(()) => Ok(()),
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
      Err(e) => Err(format!("Failed to remove stale shared memory {}: {}", path, e)),
    }
  }

  fn wait_for_mapping<T, F>(hydra: &mut Child, name: &str, mut attach: F) -> Result<T, String>
  where
    F: FnMut() -> Result<T, String>,
  {
    let deadline = Instant::now() + STARTUP_TIMEOUT;

    loop {
      match attach() {
        Ok(mapping) => return Ok(mapping),
        Err(e) => {
          if let Some(status) = hydra.try_wait().map_err(|err| format!("Failed to query DOSBox-X status: {}", err))? {
            return Err(format!("DOSBox-X exited before {} was ready: {}", name, status));
          }
          if Instant::now() >= deadline {
            return Err(format!("Timed out waiting for {}: {}", name, e));
          }
        }
      }

      std::thread::sleep(Duration::from_millis(10));
    }
  }

  fn kill_on_startup_error<T>(hydra: &mut Child, result: Result<T, String>) -> Result<T, String> {
    match result {
      Ok(value) => Ok(value),
      Err(e) => {
        let _ = hydra.kill();
        let _ = hydra.wait();
        Err(e)
      }
    }
  }

  pub fn spawn(exe_path: &str) -> Result<HydraProcess, String> {
    let current_exe = std::env::current_exe().unwrap();
    let dir = current_exe.parent().unwrap().parent().unwrap().parent().unwrap().parent().unwrap();
    let exe = Path::new(exe_path);

    // These paths are process-global in the current Hydra/DOSBox-X ABI. Clear
    // leftovers before launching so a crashed prior run cannot be mistaken for
    // the new emulator's mappings.
    Self::remove_stale_mapping(HYDRA_SHM_PATH)?;
    Self::remove_stale_mapping(DOSBOX_MEM_PATH)?;

    let mut hydra = Command::new(&format!("{}/hydra/src/dosbox-x/src/dosbox-x", dir.display()))
      .args(&[
        "-conf", &format!("{}/hydra/conf/dosbox.conf", dir.display()),
        "-hydra", &format!("{}/hydra/build/src/remote/libhydraremote.so", dir.display()),
        "-hydra-conf", "normal",
        "-c", &format!("mount d {}", exe.parent().unwrap().display()),
        "-c", "D:",
        "-c", &format!("{}", exe.file_name().unwrap().to_string_lossy()),
        "-c", "exit"
      ])
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .map_err(|e| format!("Failed to execute DOSBox-X: {}", e))?;

    let data_result = Self::wait_for_mapping(&mut hydra, "hydra_remote shared memory", || {
      ShmData::attach(HYDRA_SHM_PATH)
    });
    let data = Self::kill_on_startup_error(&mut hydra, data_result)?;

    let mem_result = Self::wait_for_mapping(&mut hydra, "dosbox_mem shared memory", || {
      ShmMem::attach(DOSBOX_MEM_PATH)
    });
    let mem = Self::kill_on_startup_error(&mut hydra, mem_result)?;

    let mut this = HydraProcess {
      hydra,
      data,
      mem,
      cpu_state: Cpu::default(),
      last_cpu_state: Cpu::default(),
    };

    this.wait_for_init()?;

    Ok(this)
  }

  fn wait_for_init(&mut self) -> Result<(), String> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;

    while self.data.load_init(Ordering::Acquire) == 0 {
      if let Some(status) = self.hydra.try_wait().map_err(|e| format!("Failed to query DOSBox-X status: {}", e))? {
        return Err(format!("DOSBox-X exited before Hydra initialized: {}", status));
      }
      if Instant::now() >= deadline {
        return Err("Timed out waiting for Hydra initialization".to_string());
      }
      std::thread::sleep(Duration::from_millis(1));
    }

    // pid is published before the release-store to init, so the acquire load
    // above makes this ordinary read safe and lets us reject stale mappings.
    let shared_pid = shmdata_read!(self.data, pid);
    if shared_pid != self.hydra.id() {
      return Err(format!("hydra_remote belongs to PID {}, expected {}", shared_pid, self.hydra.id()));
    }

    Ok(())
  }

  pub fn read_cpu_state(&mut self) {
    let mut cpu = Cpu::default();
    cpu.regs[AX.idx as usize]    = shmdata_read!(self.data, ax);
    cpu.regs[BX.idx as usize]    = shmdata_read!(self.data, bx);
    cpu.regs[CX.idx as usize]    = shmdata_read!(self.data, cx);
    cpu.regs[DX.idx as usize]    = shmdata_read!(self.data, dx);
    cpu.regs[SI.idx as usize]    = shmdata_read!(self.data, si);
    cpu.regs[DI.idx as usize]    = shmdata_read!(self.data, di);
    cpu.regs[BP.idx as usize]    = shmdata_read!(self.data, bp);
    cpu.regs[SP.idx as usize]    = shmdata_read!(self.data, sp);
    cpu.regs[IP.idx as usize]    = shmdata_read!(self.data, ip);
    cpu.regs[CS.idx as usize]    = shmdata_read!(self.data, cs);
    cpu.regs[DS.idx as usize]    = shmdata_read!(self.data, ds);
    cpu.regs[ES.idx as usize]    = shmdata_read!(self.data, es);
    cpu.regs[SS.idx as usize]    = shmdata_read!(self.data, ss);
    cpu.regs[FLAGS.idx as usize] = shmdata_read!(self.data, flags);

    self.cpu_state = cpu;
  }

  pub fn write_cpu_state(&mut self) {
    let regs = &self.cpu_state.regs;
    shmdata_write!(self.data, ax, regs[AX.idx as usize]);
    shmdata_write!(self.data, bx, regs[BX.idx as usize]);
    shmdata_write!(self.data, cx, regs[CX.idx as usize]);
    shmdata_write!(self.data, dx, regs[DX.idx as usize]);
    shmdata_write!(self.data, si, regs[SI.idx as usize]);
    shmdata_write!(self.data, di, regs[DI.idx as usize]);
    shmdata_write!(self.data, bp, regs[BP.idx as usize]);
    shmdata_write!(self.data, sp, regs[SP.idx as usize]);
    shmdata_write!(self.data, ip, regs[IP.idx as usize]);
    shmdata_write!(self.data, cs, regs[CS.idx as usize]);
    shmdata_write!(self.data, ds, regs[DS.idx as usize]);
    shmdata_write!(self.data, es, regs[ES.idx as usize]);
    shmdata_write!(self.data, ss, regs[SS.idx as usize]);
    shmdata_write!(self.data, flags, regs[FLAGS.idx as usize]);
  }

  pub fn step(&mut self) -> Result<(), String> {
    self.wait_for_init()?;

    let ack = self.data.load_ack(Ordering::Acquire);
    let next_ack = ack.wrapping_add(1);

    // Publish any register changes before making the request visible to Hydra.
    self.data.store_req(next_ack, Ordering::Release);

    let deadline = Instant::now() + STEP_TIMEOUT;
    let mut spins = 0u32;
    while next_ack != self.data.load_ack(Ordering::Acquire) {
      std::hint::spin_loop();
      spins = spins.wrapping_add(1);

      // Keep the hot path as a spin wait, but periodically detect a dead or
      // wedged emulator so validator failures surface as errors rather than hangs.
      if spins & 0xffff == 0 {
        if let Some(status) = self.hydra.try_wait().map_err(|e| format!("Failed to query DOSBox-X status: {}", e))? {
          return Err(format!("DOSBox-X exited while waiting for step acknowledgement: {}", status));
        }
        if Instant::now() >= deadline {
          return Err(format!("Timed out waiting for Hydra step acknowledgement {}", next_ack));
        }
      }
    }

    // The acquire load above synchronizes with Hydra's release store to ack,
    // making the newly published register snapshot visible here.
    self.read_cpu_state();
    Ok(())
  }
}

impl Drop for HydraProcess {
  fn drop(&mut self) {
    self.data.store_end(1, Ordering::Release);
    let _ = self.hydra.kill();
  }
}

impl Emu for HydraProcess {
  fn step(&mut self) -> Result<(), String> {
    self.last_cpu_state = self.cpu_state();
    Self::step(self)
  }
  fn cpu_state(&self) -> Cpu {
    self.cpu_state.clone()
  }
  fn last_cpu_state(&self) -> Cpu {
    self.last_cpu_state.clone()
  }
  fn instr_addr(&self) -> SegOff {
    // Synchronize with the most recent register snapshot published by Hydra.
    let _ = self.data.load_ack(Ordering::Acquire);
    let cs = shmdata_read!(self.data, cs);
    let ip = shmdata_read!(self.data, ip);
    SegOff::new(cs, ip)
  }
  fn reg_read(&self, reg: Register) -> Value {
    self.cpu_state.reg_read(reg)
  }
  fn reg_write(&mut self, reg: Register, val: Value) {
    self.cpu_state.reg_write(reg, val);
    self.write_cpu_state();
  }
  fn flag_write(&mut self, f: Flag, set: bool) {
    self.cpu_state.flag_write(f, set);
    self.write_cpu_state();
  }
  fn mem_slice(&self, addr: SegOff, len: u32) -> &[u8] {
    &self.mem.slice_starting_at(addr)[..len as usize]
  }
  fn interrupt_handler(&self, _vector: u8) -> Option<SegOff> {
    panic!("interrupt_handler unimpl for hydra process");
  }
  fn machine(&mut self) -> Option<&mut Machine> {
    None
  }
  fn report(&self) {
    panic!("Unimpl");
  }
}
