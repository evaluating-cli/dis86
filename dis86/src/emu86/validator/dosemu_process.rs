use super::super::cpu::*;
use super::super::cpu_flags::Flag;
use super::super::emu::{Emu, StepOutcome};
use super::super::machine::Machine;
use super::super::value::Value;
use super::shmdata::ShmData;
use super::shmmem::ShmMem;
use crate::binfmt::mz;
use crate::segoff::SegOff;
use crate::{shmdata_read, shmdata_write};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const HYDRA_SHM_PATH: &str = "/dev/shm/hydra_remote";
const DOSEMU_MEM_PATH: &str = "/dev/shm/dosemu_mem";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STEP_TIMEOUT: Duration = Duration::from_secs(5);
// Keep the request/ack path hot while amortizing process and clock syscalls.
const STEP_STATUS_POLL_SPINS: u32 = 1024;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const DIAGNOSTIC_LIMIT: u64 = 64 * 1024;
const VALIDATOR_ABI_VERSION: u32 = 1;

#[derive(Debug, Eq, PartialEq)]
struct DosemuCommand {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

fn target_command(exe_path: &Path, program: String) -> Result<DosemuCommand, String> {
    let image = std::fs::read(exe_path).map_err(|e| {
        format!(
            "Failed to read target executable {}: {}",
            exe_path.display(),
            e
        )
    })?;
    let decoded = mz::Exe::decode(&image).map_err(|e| {
        format!(
            "Failed to decode target executable {}: {}",
            exe_path.display(),
            e
        )
    })?;
    let parent = exe_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            format!(
                "Target executable has no containing directory: {}",
                exe_path.display()
            )
        })?;
    let name = exe_path
        .file_name()
        .ok_or_else(|| format!("Target executable has no file name: {}", exe_path.display()))?
        .to_string_lossy()
        .into_owned();
    // Canonicalize the redirected host directory so aliases identify the same
    // dynamic drive, while the DOS identity remains drive-letter independent.
    let parent = std::fs::canonicalize(parent).map_err(|e| {
        format!(
            "Failed to canonicalize target directory {}: {}",
            parent.display(),
            e
        )
    })?;
    Ok(DosemuCommand {
        program,
        args: vec![
            "-dumb",
            "-quiet",
            "-K",
            &parent.to_string_lossy(),
            "-E",
            &name,
            "-I",
            "cpu_vm emulated",
            "-I",
            "cpuemu 1",
            "-I",
            "cpu_vm_dpmi emulated",
            "-I",
            "mappingdriver mapshm",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        env: vec![
            ("DIIS_DOSEMU_VALIDATOR".into(), "1".into()),
            (
                "DIIS_DOSEMU_MZ_CS".into(),
                (decoded.hdr.cs as u16).to_string(),
            ),
            ("DIIS_DOSEMU_MZ_IP".into(), {
                let ip = decoded.hdr.ip;
                ip.to_string()
            }),
            (
                "DIIS_DOSEMU_TARGET_DOS_PATH".into(),
                format!("?:\\{}", name),
            ),
        ],
    })
}

trait ProcessControl {
    fn id(&self) -> u32;
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
    fn wait(&mut self) -> std::io::Result<ExitStatus>;
}
impl ProcessControl for Child {
    fn id(&self) -> u32 {
        Child::id(self)
    }
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Child::wait(self)
    }
}

trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}
struct SystemClock {
    start: Instant,
}
impl SystemClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}
impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub struct DosemuProcess {
    dosemu: Box<dyn ProcessControl>,
    clock: Box<dyn Clock>,
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
  fn parent_pid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
    status.lines().find_map(|line| {
      let value = line.strip_prefix("PPid:")?;
      value.trim().parse().ok()
    })
  }

  fn process_belongs_to_launcher(pid: u32, launcher_pid: u32) -> bool {
    let mut current = pid;
    for _ in 0..64 {
            if current == launcher_pid {
                return true;
            }
            let Some(parent) = Self::parent_pid(current) else {
                return false;
            };
            if parent == 0 || parent == current {
                return false;
            }
      current = parent;
    }
    false
  }

  fn remove_stale_mapping(path: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
      Ok(()) => Ok(()),
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!(
                "Failed to remove stale shared memory {}: {}",
                path, e
            )),
    }
  }

    fn wait_for_mapping<T, F>(
        dosemu: &mut dyn ProcessControl,
        clock: &dyn Clock,
        name: &str,
        mut attach: F,
    ) -> Result<T, String>
  where
    F: FnMut() -> Result<T, String>,
  {
        let deadline = clock.now() + STARTUP_TIMEOUT;

    loop {
      match attach() {
        Ok(mapping) => return Ok(mapping),
        Err(e) => {
                    if let Some(status) = dosemu
                        .try_wait()
                        .map_err(|err| format!("Failed to query dosemu2 status: {}", err))?
                    {
                        return Err(format!(
                            "dosemu2 exited before {} was ready: {}",
                            name, status
                        ));
          }
                    if clock.now() >= deadline {
            return Err(format!("Timed out waiting for {}: {}", name, e));
          }
        }
      }

            clock.sleep(Duration::from_millis(10));
    }
  }

    fn kill_on_startup_error<T>(
        dosemu: &mut dyn ProcessControl,
        result: Result<T, String>,
    ) -> Result<T, String> {
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
                "dis86-dosemu2-{}-{}.stderr",
                std::process::id(),
                attempt
            ));
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
        if bytes.is_empty() {
            return "\n[dosemu2 produced no stderr]".to_string();
        }
        let prefix = if start == 0 {
            ""
        } else {
            "[earlier output omitted]\n"
        };
        format!(
            "\n--- dosemu2 stderr (last {} bytes) ---\n{}{}",
            bytes.len(),
            prefix,
            String::from_utf8_lossy(&bytes)
        )
  }

  fn with_diagnostics(&self, error: String) -> String {
    format!("{}{}", error, Self::diagnostic_tail(&self.diagnostic_path))
  }

    fn terminate(dosemu: &mut dyn ProcessControl) {
    let _ = dosemu.kill();
    let _ = dosemu.wait();
  }

  pub fn spawn(exe_path: &str) -> Result<DosemuProcess, String> {
    Self::spawn_with_paths(exe_path, HYDRA_SHM_PATH, DOSEMU_MEM_PATH)
  }

  fn spawn_with_paths(exe_path: &str, control_path: &str, memory_path: &str) -> Result<DosemuProcess, String> {
    let exe = Path::new(exe_path);
        let dosemu_bin = std::env::var("DOSEMU_BIN").unwrap_or_else(|_| "dosemu".to_string());
        let command = target_command(exe, dosemu_bin.clone())?;

    Self::remove_stale_mapping(control_path)?;
    Self::remove_stale_mapping(memory_path)?;

    // Spawn dosemu2 in headless batch mode (-dumb -quiet)
    let (diagnostic_path, diagnostic_file) = Self::create_diagnostic_log()?;
        let spawn_result = Command::new(&command.program)
            .args(&command.args)
            .envs(command.env.iter().cloned())
      .stdout(Stdio::null())
      .stderr(Stdio::from(diagnostic_file))
      .spawn();
        let mut dosemu: Box<dyn ProcessControl> = match spawn_result {
            Ok(child) => Box::new(child),
      Err(e) => {
        let diagnostics = Self::diagnostic_tail(&diagnostic_path);
        let _ = std::fs::remove_file(&diagnostic_path);
                return Err(format!(
                    "Failed to execute dosemu2 ({}): {}{}",
                    dosemu_bin, e, diagnostics
                ));
      }
    };

        let clock: Box<dyn Clock> = Box::new(SystemClock::new());
        let data_result =
            Self::wait_for_mapping(&mut *dosemu, &*clock, "hydra_remote shared memory", || {
      ShmData::attach(control_path)
    });
        let data = match Self::kill_on_startup_error(&mut *dosemu, data_result) {
      Ok(data) => data,
      Err(e) => {
        let error = format!("{}{}", e, Self::diagnostic_tail(&diagnostic_path));
        let _ = std::fs::remove_file(&diagnostic_path);
        return Err(error);
      }
    };

        let mem_result =
            Self::wait_for_mapping(&mut *dosemu, &*clock, "dosemu_mem shared memory", || {
      ShmMem::attach(memory_path)
    });
        let mem = match Self::kill_on_startup_error(&mut *dosemu, mem_result) {
      Ok(mem) => mem,
      Err(e) => {
        let error = format!("{}{}", e, Self::diagnostic_tail(&diagnostic_path));
        let _ = std::fs::remove_file(&diagnostic_path);
        return Err(error);
      }
    };

    let mut this = DosemuProcess {
      dosemu,
            clock,
      diagnostic_path,
      data,
      mem,
      cpu_state: Cpu::default(),
      last_cpu_state: Cpu::default(),
      finished: false,
      shut_down: false,
    };

    if let Err(e) = this.wait_for_init() {
            Self::terminate(&mut *this.dosemu);
      return Err(this.with_diagnostics(e));
    }
    // init is release-published after the first target snapshot. Capture it
    // before the validator constructs or steps the comparison emulator.
    this.read_cpu_state();

    Ok(this)
  }

  fn wait_for_init(&mut self) -> Result<(), String> {
        let deadline = self.clock.now() + STARTUP_TIMEOUT;

    while self.data.load_init(Ordering::Acquire) == 0 {
            if let Some(status) = self
                .dosemu
                .try_wait()
                .map_err(|e| format!("Failed to query dosemu2 status: {}", e))?
            {
                return Err(format!(
                    "dosemu2 exited before Hydra initialized: {}",
                    status
                ));
      }
            if self.clock.now() >= deadline {
        return Err("Timed out waiting for Hydra initialization in dosemu2".to_string());
      }
            self.clock.sleep(Duration::from_millis(1));
    }

    let shared_pid = shmdata_read!(self.data, pid);
    if !Self::process_belongs_to_launcher(shared_pid, self.dosemu.id()) {
            return Err(format!(
                "hydra_remote belongs to PID {}, which is not launcher PID {} or its descendant",
                shared_pid,
                self.dosemu.id()
            ));
    }

    let abi_version = shmdata_read!(self.data, abi_version);
    let struct_size = shmdata_read!(self.data, struct_size) as usize;
        if abi_version != VALIDATOR_ABI_VERSION
            || struct_size < std::mem::size_of::<super::shmdata::ShmDataRaw>()
        {
            return Err(format!(
                "unsupported dosemu2 validator ABI version {} size {}",
                abi_version, struct_size
            ));
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
        if self.shut_down {
            return Err("cannot step dosemu2 validator after shutdown".to_string());
        }
        if self.finished {
            return Err("cannot step dosemu2 validator after target exit".to_string());
        }
        if self.data.load_step_flags(Ordering::Acquire) & StepOutcome::END_ACK != 0 {
            return Err("cannot step dosemu2 validator after shutdown acknowledgement".to_string());
        }
    self.wait_for_init().map_err(|e| self.with_diagnostics(e))?;

    let ack = self.data.load_ack(Ordering::Acquire);
    let next_ack = ack.wrapping_add(1);

    self.data.store_req(next_ack, Ordering::Release);

        let deadline = self.clock.now() + STEP_TIMEOUT;
        let mut spins_until_status_poll = STEP_STATUS_POLL_SPINS;
    while next_ack != self.data.load_ack(Ordering::Acquire) {
      std::hint::spin_loop();
            spins_until_status_poll -= 1;
            if spins_until_status_poll != 0 {
                continue;
            }
            spins_until_status_poll = STEP_STATUS_POLL_SPINS;
            if let Some(status) = self.dosemu.try_wait().map_err(|e| {
                self.with_diagnostics(format!("Failed to query dosemu2 status: {}", e))
            })? {
                return Err(self.with_diagnostics(format!(
                    "dosemu2 exited while waiting for step acknowledgement: {}",
                    status
                )));
        }
            if self.clock.now() >= deadline {
                return Err(self.with_diagnostics(format!(
                    "Timed out waiting for dosemu2 step acknowledgement {}",
                    next_ack
                )));
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
        if self.shut_down {
            return Ok(());
        }
        if let Some(status) = self.dosemu.try_wait().map_err(|e| {
            self.with_diagnostics(format!("Failed to query dosemu2 during shutdown: {}", e))
        })? {
      self.shut_down = true;
            if self.finished && status.success() {
                return Ok(());
            }
      return Err(self.with_diagnostics(format!(
                "dosemu2 exited before shutdown acknowledgement: {}",
                status
            )));
    }

    self.data.store_end(1, Ordering::Release);
        let ack_deadline = self.clock.now() + SHUTDOWN_TIMEOUT;
    loop {
            if self.data.load_step_flags(Ordering::Acquire) & StepOutcome::END_ACK != 0 {
                break;
            }
            if let Some(status) = self.dosemu.try_wait().map_err(|e| {
                self.with_diagnostics(format!("Failed to query dosemu2 during shutdown: {}", e))
            })? {
        self.shut_down = true;
                if self.finished && status.success() {
                    return Ok(());
                }
                return Err(self.with_diagnostics(format!(
                    "dosemu2 exited before shutdown acknowledgement: {}",
                    status
                )));
      }
            if self.clock.now() >= ack_deadline {
                Self::terminate(&mut *self.dosemu);
        self.shut_down = true;
                return Err(self.with_diagnostics(
                    "Timed out waiting for dosemu2 shutdown acknowledgement".to_string(),
                ));
      }
            self.clock.sleep(Duration::from_millis(1));
    }

        let exit_deadline = self.clock.now() + SHUTDOWN_TIMEOUT;
    loop {
            if let Some(status) = self.dosemu.try_wait().map_err(|e| {
                self.with_diagnostics(format!("Failed to query dosemu2 during shutdown: {}", e))
            })? {
        self.shut_down = true;
                return if status.success() {
                    Ok(())
                } else {
                    Err(self.with_diagnostics(format!(
                        "dosemu2 failed after shutdown acknowledgement: {}",
                        status
                    )))
        };
      }
            if self.clock.now() >= exit_deadline {
                Self::terminate(&mut *self.dosemu);
        self.shut_down = true;
                return Err(self.with_diagnostics(
                    "Timed out waiting for dosemu2 to exit after shutdown acknowledgement"
                        .to_string(),
                ));
      }
            self.clock.sleep(Duration::from_millis(1));
    }
  }
}

impl Drop for DosemuProcess {
  fn drop(&mut self) {
    if !self.shut_down {
      self.data.store_end(1, Ordering::Release);
            Self::terminate(&mut *self.dosemu);
    }
    let _ = std::fs::remove_file(&self.diagnostic_path);
  }
}

impl Emu for DosemuProcess {
  fn step(&mut self) -> Result<StepOutcome, String> {
    self.last_cpu_state = self.cpu_state();
    Self::step(self)
  }
    fn shutdown(&mut self) -> Result<(), String> {
        Self::shutdown(self)
    }
    fn finished(&self) -> bool {
        self.finished
    }
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
  fn mem_len(&self) -> usize {
    self.mem.len
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

#[cfg(test)]
mod tests {
  use super::{target_command, DosemuProcess};
  use super::super::shmdata::{ShmData, ShmDataRaw};
  use std::fs::OpenOptions;
  use std::path::PathBuf;
  use std::process::Command;
  use std::sync::atomic::Ordering;

  fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("dis86-dosemu-test-{}-{}", std::process::id(), name))
  }

  fn mz_file(name: &str, cs: u16, ip: u16) -> PathBuf {
    let dir = temp(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("target.exe");
    let mut image = vec![0u8; 28];
    image[0..2].copy_from_slice(b"MZ");
    image[2..4].copy_from_slice(&(28u16).to_le_bytes()); // bytes in final page
    image[4..6].copy_from_slice(&(1u16).to_le_bytes());
    image[8..10].copy_from_slice(&(1u16).to_le_bytes());
    image[20..22].copy_from_slice(&ip.to_le_bytes());
    image[22..24].copy_from_slice(&cs.to_le_bytes());
    image[24..26].copy_from_slice(&(28u16).to_le_bytes());
    std::fs::write(&path, image).unwrap();
    path
  }

  #[test]
  fn exact_arguments_environment_and_mz_entry_point() {
    let path = mz_file("command", 0x1234, 0xabcd);
    let spec = target_command(&path, "dosemu-test".into()).unwrap();
    assert_eq!(spec.program, "dosemu-test");
    assert_eq!(spec.args, vec!["-dumb", "-quiet", "-K",
      path.parent().unwrap().to_str().unwrap(), "-E", "target.exe",
      "-I", "cpu_vm emulated", "-I", "cpuemu 1", "-I", "cpu_vm_dpmi emulated",
      "-I", "mappingdriver mapshm"]);
    assert_eq!(spec.env, vec![
      ("DIIS_DOSEMU_VALIDATOR".into(), "1".into()),
      ("DIIS_DOSEMU_MZ_CS".into(), "4660".into()),
      ("DIIS_DOSEMU_MZ_IP".into(), "43981".into()),
      ("DIIS_DOSEMU_TARGET_DOS_PATH".into(), "?:\\target.exe".into())]);
  }

  #[test]
  fn executable_and_path_errors_are_contextual() {
    let missing = temp("missing").join("none.exe");
    assert!(target_command(&missing, "dosemu".into()).unwrap_err().contains("Failed to read target executable"));
    let bad = temp("bad.exe");
    std::fs::write(&bad, b"not an mz").unwrap();
    assert!(target_command(&bad, "dosemu".into()).unwrap_err().contains("Failed to decode target executable"));
  }

  #[test]
  fn dynamic_drive_uses_canonical_directory_and_stable_dos_identity() {
    let path = mz_file("canonical/real", 0, 0);
    let alias = temp("canonical").join("alias");
    std::os::unix::fs::symlink(path.parent().unwrap(), &alias).unwrap();
    let through_alias = alias.join("target.exe");
    let direct = target_command(&path, "dosemu".into()).unwrap();
    let aliased = target_command(&through_alias, "dosemu".into()).unwrap();
    assert_eq!(direct.args, aliased.args);
    assert_eq!(direct.env.last(), aliased.env.last());
  }

  #[test]
  fn control_mapping_is_page_sized_with_88_byte_abi_prefix() {
    assert_eq!(std::mem::size_of::<ShmDataRaw>(), 88);
    assert_eq!(ShmData::size(), 4096);
    let path = temp("control-map");
    let _ = std::fs::remove_file(&path);
    let file = OpenOptions::new().create_new(true).read(true).write(true).open(&path).unwrap();
    file.set_len(4096).unwrap();
    let data = ShmData::attach(path.to_str().unwrap()).unwrap();
    assert_eq!(data.load_req(Ordering::Acquire), 0);
    drop(data);
    std::fs::remove_file(path).unwrap();
  }

  #[test]
  fn shared_memory_owner_may_be_launcher_or_child_and_rejects_unrelated_pid() {
    let launcher = std::process::id();
    assert!(DosemuProcess::process_belongs_to_launcher(launcher, launcher));
    assert!(!DosemuProcess::process_belongs_to_launcher(1, launcher));
    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    assert!(DosemuProcess::process_belongs_to_launcher(child.id(), launcher));
    let _ = child.kill();
    let _ = child.wait();
  }
}
