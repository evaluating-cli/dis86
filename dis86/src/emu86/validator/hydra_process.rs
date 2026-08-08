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

pub struct HydraProcess {
  hydra: Child,
  data: ShmData,
  #[allow(dead_code)]
  pub mem: ShmMem,

  cpu_state: Cpu,
  last_cpu_state: Cpu,
}

impl HydraProcess {
  pub fn spawn(exe_path: &str) -> Result<HydraProcess, String> {
    let current_exe = std::env::current_exe().unwrap();
    let dir = current_exe.parent().unwrap().parent().unwrap().parent().unwrap().parent().unwrap();
    let exe = Path::new(exe_path);
    let hydra = Command::new(&format!("{}/hydra/src/dosbox-x/src/dosbox-x", dir.display()))
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

    std::thread::sleep(std::time::Duration::from_millis(1000));

    let data = ShmData::attach("/dev/shm/hydra_remote")
      .map_err(|e| format!("Failed to attach hydra_remote shared memory: {}", e))?;
    let mem = ShmMem::attach("/dev/shm/dosbox_mem")
      .map_err(|e| format!("Failed to attach dosbox_mem shared memory: {}", e))?;

    let mut this = HydraProcess {
      hydra,
      data,
      mem,
      cpu_state: Cpu::default(),
      last_cpu_state: Cpu::default(),
    };

    this.wait_for_init();

    Ok(this)
  }

  fn wait_for_init(&mut self) {
    while self.data.load_init(Ordering::Acquire) == 0 {
      std::hint::spin_loop();
    }
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

  pub fn step(&mut self) {
    self.wait_for_init();

    let ack = self.data.load_ack(Ordering::Acquire);
    let next_ack = ack.wrapping_add(1);

    // Publish any register changes before making the request visible to Hydra.
    self.data.store_req(next_ack, Ordering::Release);

    while next_ack != self.data.load_ack(Ordering::Acquire) {
      std::hint::spin_loop();
    }

    // The acquire load above synchronizes with Hydra's release store to ack,
    // making the newly published register snapshot visible here.
    self.read_cpu_state()
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
    Self::step(self);
    Ok(())
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
