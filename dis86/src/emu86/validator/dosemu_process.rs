use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Child, Stdio};
use super::super::emu::{Emu, StepOutcome};
use super::super::cpu::*;
use super::super::cpu_flags::Flag;
use super::super::machine::Machine;
use super::super::value::Value;
use super::shmdata::ShmData;
use super::shmmem::ShmMem;
use crate::segoff::SegOff;
use crate::{shmdata_read, shmdata_write};
use crate::binfmt::mz;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const HYDRA_SHM_PATH: &str = "/dev/shm/hydra_remote";
const DOSEMU_MEM_PATH: &str = "/dev/shm/dosemu_mem";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STEP_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const DIAGNOSTIC_LIMIT: u64 = 64 * 1024;
const VALIDATOR_ABI_VERSION: u32 = 1;

pub struct DosemuProcess {
  dosemu: Child,
  diagnostic_path: PathBuf,
  data: ShmData,
  #[allow(dead_code)]
  pub mem: ShmMem,

  cpu_state: Cpu,
  last_cpu_state: Cpu,
  finished: bool,
  shut_down: bool,
}

impl DosemuProcess {
  fn remove_stale_mapping(path: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
      Ok(()) => Ok(()),
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
      Err(e) => Err(format!("Failed to remove stale shared memory {}: {}", path, e)),
    }
  }

  fn wait_for_mapping<T, F>(dosemu: &mut Child, name: &str, mut attach: F) -> Result<T, String>
  where
    F: FnMut() -> Result<T, String>,
  {
    let deadline = Instant::now() + STARTUP_TIMEOUT;

    loop {
      match attach() {
        Ok(mapping) => return Ok(mapping),
        Err(e) => {
          if let Some(status) = dosemu.try_wait().map_err(|err| format!("Failed to query dosemu2 status: {}", err))? {
            return Err(format!("dosemu2 exited before {} was ready: {}", name, status));
          }
          if Instant::now() >= deadline {
            return Err(format!("Timed out waiting for {}: {}", name, e));
          }
        }
      }

      std::thread::sleep(Duration::from_millis(10));
    }
  }

  fn kill_on_startup_error<T>(dosemu: &mut Child, result: Result<T, String>) -> Result<T, String> {
    match result {
      Ok(value) => Ok(value),
      Err(e) => {
        let _ = dosemu.kill();
        let _ = dosemu.wait();
        Err(e)
      }
    }
  }

  fn create_diagnostic_log() -> Result<(PathBuf, File), String> {
    for attempt in 0..100u32 {
      let path = std::env::temp_dir().join(format!(
        "dis86-dosemu2-{}-{}.stderr", std::process::id(), attempt));
      match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => return Ok((path, file)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
        Err(e) => return Err(format!("Failed to create dosemu2 diagnostic log: {}", e)),
      }
    }
    Err("Failed to allocate a unique dosemu2 diagnostic log".to_string())
  }

  fn diagnostic_tail(path: &Path) -> String {
    let mut file = match File::open(path) {
      Ok(file) => file,
      Err(e) => return format!("\n[dosemu2 stderr unavailable: {}]", e),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(DIAGNOSTIC_LIMIT);
    if file.seek(SeekFrom::Start(start)).is_err() {
      return "\n[dosemu2 stderr could not be read]".to_string();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
      return "\n[dosemu2 stderr could not be read]".to_string();
    }
    if bytes.is_empty() { return "\n[dosemu2 produced no stderr]".to_string(); }
    let prefix = if start == 0 { "" } else { "[earlier output omitted]\n" };
    format!("\n--- dosemu2 stderr (last {} bytes) ---\n{}{}",
      bytes.len(), prefix, String::from_utf8_lossy(&bytes))
  }

  fn with_diagnostics(&self, error: String) -> String {
    format!("{}{}", error, Self::diagnostic_tail(&self.diagnostic_path))
  }

  fn terminate(dosemu: &mut Child) {
    let _ = dosemu.kill();
    let _ = dosemu.wait();
  }

  pub fn spawn(exe_path: &str) -> Result<DosemuProcess, String> {
    let exe = Path::new(exe_path);
    let image = std::fs::read(exe)
      .map_err(|e| format!("Failed to read target executable {}: {}", exe.display(), e))?;
    let mz = mz::Exe::decode(&image)
      .map_err(|e| format!("Failed to decode target executable {}: {}", exe.display(), e))?;
    let mz_cs = mz.hdr.cs as u16;
    let mz_ip = mz.hdr.ip;
    // -K mounts the containing directory as C:, so this is the canonical DOS
    // identity placed in the PSP environment by the matching -E invocation.
    let target_dos_path = format!("C:\\{}", exe.file_name().unwrap().to_string_lossy());

    Self::remove_stale_mapping(HYDRA_SHM_PATH)?;
    Self::remove_stale_mapping(DOSEMU_MEM_PATH)?;

    // Spawn dosemu2 in headless batch mode (-dumb -quiet)
    let dosemu_bin = std::env::var("DOSEMU_BIN").unwrap_or_else(|_| "dosemu".to_string());
    let (diagnostic_path, diagnostic_file) = Self::create_diagnostic_log()?;
    let spawn_result = Command::new(&dosemu_bin)
      .args(&[
        "-dumb",
        "-quiet",
        "-K", &format!("{}", exe.parent().unwrap().to_string_lossy()),
        "-E", &format!("{}", exe.file_name().unwrap().to_string_lossy()),
        "-I", "cpu_vm emulated",
        "-I", "cpuemu 1",
        "-I", "cpu_vm_dpmi emulated",
        "-I", "mappingdriver mapshm",
      ])
      .env("DIIS_DOSEMU_VALIDATOR", "1")
      .env("DIIS_DOSEMU_MZ_CS", mz_cs.to_string())
      .env("DIIS_DOSEMU_MZ_IP", mz_ip.to_string())
      .env("DIIS_DOSEMU_TARGET_DOS_PATH", target_dos_path)
      .stdout(Stdio::null())
      .stderr(Stdio::from(diagnostic_file))
      .spawn();
    let mut dosemu = match spawn_result {
      Ok(child) => child,
      Err(e) => {
        let diagnostics = Self::diagnostic_tail(&diagnostic_path);
        let _ = std::fs::remove_file(&diagnostic_path);
        return Err(format!("Failed to execute dosemu2 ({}): {}{}", dosemu_bin, e, diagnostics));
      }
    };

    let data_result = Self::wait_for_mapping(&mut dosemu, "hydra_remote shared memory", || {
      ShmData::attach(HYDRA_SHM_PATH)
    });
    let data = match Self::kill_on_startup_error(&mut dosemu, data_result) {
      Ok(data) => data,
      Err(e) => {
        let error = format!("{}{}", e, Self::diagnostic_tail(&diagnostic_path));
        let _ = std::fs::remove_file(&diagnostic_path);
        return Err(error);
      }
    };

    let mem_result = Self::wait_for_mapping(&mut dosemu, "dosemu_mem shared memory", || {
      ShmMem::attach(DOSEMU_MEM_PATH)
    });
    let mem = match Self::kill_on_startup_error(&mut dosemu, mem_result) {
      Ok(mem) => mem,
      Err(e) => {
        let error = format!("{}{}", e, Self::diagnostic_tail(&diagnostic_path));
        let _ = std::fs::remove_file(&diagnostic_path);
        return Err(error);
      }
    };

    let mut this = DosemuProcess {
      dosemu,
      diagnostic_path,
      data,
      mem,
      cpu_state: Cpu::default(),
      last_cpu_state: Cpu::default(),
      finished: false,
      shut_down: false,
    };

    if let Err(e) = this.wait_for_init() {
      Self::terminate(&mut this.dosemu);
      return Err(this.with_diagnostics(e));
    }
    // init is release-published after the first target snapshot. Capture it
    // before the validator constructs or steps the comparison emulator.
    this.read_cpu_state();

    Ok(this)
  }

  fn wait_for_init(&mut self) -> Result<(), String> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;

    while self.data.load_init(Ordering::Acquire) == 0 {
      if let Some(status) = self.dosemu.try_wait().map_err(|e| format!("Failed to query dosemu2 status: {}", e))? {
        return Err(format!("dosemu2 exited before Hydra initialized: {}", status));
      }
      if Instant::now() >= deadline {
        return Err("Timed out waiting for Hydra initialization in dosemu2".to_string());
      }
      std::thread::sleep(Duration::from_millis(1));
    }

    let shared_pid = shmdata_read!(self.data, pid);
    if shared_pid != self.dosemu.id() {
      return Err(format!("hydra_remote belongs to PID {}, expected {}", shared_pid, self.dosemu.id()));
    }

    let abi_version = shmdata_read!(self.data, abi_version);
    let struct_size = shmdata_read!(self.data, struct_size) as usize;
    if abi_version != VALIDATOR_ABI_VERSION || struct_size < std::mem::size_of::<super::shmdata::ShmDataRaw>() {
      return Err(format!("unsupported dosemu2 validator ABI version {} size {}", abi_version, struct_size));
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

  pub fn step(&mut self) -> Result<StepOutcome, String> {
    self.wait_for_init().map_err(|e| self.with_diagnostics(e))?;

    let ack = self.data.load_ack(Ordering::Acquire);
    let next_ack = ack.wrapping_add(1);

    self.data.store_req(next_ack, Ordering::Release);

    let deadline = Instant::now() + STEP_TIMEOUT;
    let mut spins = 0u32;
    while next_ack != self.data.load_ack(Ordering::Acquire) {
      std::hint::spin_loop();
      spins = spins.wrapping_add(1);

      if spins & 0xffff == 0 {
        if let Some(status) = self.dosemu.try_wait()
          .map_err(|e| self.with_diagnostics(format!("Failed to query dosemu2 status: {}", e)))? {
          return Err(self.with_diagnostics(format!("dosemu2 exited while waiting for step acknowledgement: {}", status)));
        }
        if Instant::now() >= deadline {
          return Err(self.with_diagnostics(format!("Timed out waiting for dosemu2 step acknowledgement {}", next_ack)));
        }
      }
    }

    // The acquire-load of flags is the publication barrier for both the CPU
    // snapshot and decoded-instruction count (including terminal outcomes).
    let step_flags = self.data.load_step_flags(Ordering::Acquire);
    let outcome = StepOutcome {
      decoded_instructions: self.data.load_decoded_instructions(),
      step_flags,
    };
    self.read_cpu_state();
    if outcome.has(StepOutcome::TARGET_EXIT) {
      self.finished = true;
    }
    Ok(outcome)
  }

  pub fn runtime_psp(&self) -> u16 {
    shmdata_read!(self.data, runtime_psp)
  }

  pub fn shutdown(&mut self) -> Result<(), String> {
    if self.shut_down { return Ok(()); }
    if let Some(status) = self.dosemu.try_wait()
      .map_err(|e| self.with_diagnostics(format!("Failed to query dosemu2 during shutdown: {}", e)))? {
      self.shut_down = true;
      return Err(self.with_diagnostics(format!(
        "dosemu2 exited before shutdown acknowledgement: {}", status)));
    }

    self.data.store_end(1, Ordering::Release);
    let ack_deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    loop {
      if self.data.load_step_flags(Ordering::Acquire) & StepOutcome::END_ACK != 0 { break; }
      if let Some(status) = self.dosemu.try_wait()
        .map_err(|e| self.with_diagnostics(format!("Failed to query dosemu2 during shutdown: {}", e)))? {
        self.shut_down = true;
        return Err(self.with_diagnostics(format!("dosemu2 exited before shutdown acknowledgement: {}", status)));
      }
      if Instant::now() >= ack_deadline {
        Self::terminate(&mut self.dosemu);
        self.shut_down = true;
        return Err(self.with_diagnostics("Timed out waiting for dosemu2 shutdown acknowledgement".to_string()));
      }
      std::thread::sleep(Duration::from_millis(1));
    }

    let exit_deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    loop {
      if let Some(status) = self.dosemu.try_wait()
        .map_err(|e| self.with_diagnostics(format!("Failed to query dosemu2 during shutdown: {}", e)))? {
        self.shut_down = true;
        return if status.success() { Ok(()) } else {
          Err(self.with_diagnostics(format!("dosemu2 failed after shutdown acknowledgement: {}", status)))
        };
      }
      if Instant::now() >= exit_deadline {
        Self::terminate(&mut self.dosemu);
        self.shut_down = true;
        return Err(self.with_diagnostics("Timed out waiting for dosemu2 to exit after shutdown acknowledgement".to_string()));
      }
      std::thread::sleep(Duration::from_millis(1));
    }
  }
}

impl Drop for DosemuProcess {
  fn drop(&mut self) {
    if !self.shut_down {
      self.data.store_end(1, Ordering::Release);
      Self::terminate(&mut self.dosemu);
    }
    let _ = std::fs::remove_file(&self.diagnostic_path);
  }
}

impl Emu for DosemuProcess {
  fn step(&mut self) -> Result<StepOutcome, String> {
    self.last_cpu_state = self.cpu_state();
    Self::step(self)
  }
  fn shutdown(&mut self) -> Result<(), String> { Self::shutdown(self) }
  fn finished(&self) -> bool { self.finished }
  fn cpu_state(&self) -> Cpu {
    self.cpu_state.clone()
  }
  fn last_cpu_state(&self) -> Cpu {
    self.last_cpu_state.clone()
  }
  fn instr_addr(&self) -> SegOff {
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
    panic!("interrupt_handler unimpl for dosemu process");
  }
  fn machine(&mut self) -> Option<&mut Machine> {
    None
  }
  fn report(&self) {
    panic!("Unimpl");
  }
}
