use crate::binfmt::mz;
use super::machine::Machine;
use super::cpu::{Cpu, Register};
use super::cpu_flags::Flag;
use super::value::Value;
#[cfg(feature = "sdl")]
use super::sdl;
use std::path::Path;
use crate::segoff::{Seg, SegOff};

pub struct Emulator {
  #[allow(dead_code)]
  exe_path: String,
  #[allow(dead_code)]
  exe: mz::Exe,
  pub machine: Machine,
  app: App,
  step_count: u64,
  last_cpu_state: Cpu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadConfig { pub psp_segment: u16 }

/// Describes how much guest work an emulator consumed at one validation
/// boundary. A translated backend may execute several decoded instructions in
/// one node; instruction-at-a-time backends always return `single()`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StepOutcome {
  pub decoded_instructions: u32,
  pub step_flags: u32,
}

impl StepOutcome {
  pub const MULTI_INSN: u32 = 1 << 0;
  pub const SAME_PC: u32 = 1 << 1;
  pub const FAULT: u32 = 1 << 2;
  pub const END_ACK: u32 = 1 << 3;
  pub const TARGET_EXIT: u32 = 1 << 4;

  pub fn single() -> Self { Self { decoded_instructions: 1, step_flags: 0 } }
  pub fn has(self, flag: u32) -> bool { self.step_flags & flag != 0 }
}

impl Default for LoadConfig {
  fn default() -> Self { Self { psp_segment: 0x0813 } }
}

#[cfg(feature = "sdl")]
type App = sdl::App;

// Validator and unit-test runs are headless. Keeping this tiny frontend in the
// same emulator path ensures they exercise the same CPU implementation without
// pulling in a graphical host dependency.
#[cfg(not(feature = "sdl"))]
struct App;

#[cfg(not(feature = "sdl"))]
impl App {
  fn new() -> Self { Self }
  fn update(&mut self) -> Result<bool, String> { Ok(false) }
}

impl Emulator {
  pub fn new(exe_path: &str) -> Result<Emulator, String> {
    Self::new_with_load_config(exe_path, LoadConfig::default())
  }

  pub fn new_with_load_config(exe_path: &str, config: LoadConfig) -> Result<Emulator, String> {
    let Ok(data) = std::fs::read(exe_path) else {
      panic!("Failed to read file: {}", exe_path);
    };
    let exe = mz::Exe::decode(&data).unwrap();

    // As the filesystem rootdir, use the root dir of the exe
    let root_dir = Path::new(exe_path).parent().unwrap().to_str().unwrap();

    // Init the machine and load up the program
    let mut machine = Machine::new_with_psp_segment(Some(root_dir), config.psp_segment);
    machine.load_exe(&exe)?;

    let app = App::new();

    Ok(Emulator {
      exe_path: exe_path.to_string(),
      exe,
      machine,
      app,
      step_count: 0,
      last_cpu_state: Cpu::default(),
    })
  }

  pub fn step(&mut self) -> Result<(), String> {
    // Avoid updating SDL on every instruction ... too slow
    // FIXME: Add a proper time-based update (try to maintain some update Hz)
    if self.step_count % (1<<8) == 0 {
      let quit = self.app.update()?;
      if quit { return Err(format!("SDL Exited")) };
    }
    self.last_cpu_state = self.machine.cpu.clone();
    self.machine.step()?;
    self.step_count += 1;
    Ok(())
  }

  fn run(&mut self) -> Result<(), String> {
    while !self.machine.halted() {
      self.step()?;
    }
    println!("CPU State:");
    println!("{}", self.machine.cpu);
    Ok(())
  }
}

// A generic trait to make a unified interface for doing validation / comparisons of two very
// different implementations
pub trait Emu {
  fn step(&mut self) -> Result<StepOutcome, String>;
  /// Give process-backed emulators an opportunity to complete their shutdown
  /// protocol. Implementations without an external process have nothing to do.
  fn shutdown(&mut self) -> Result<(), String> { Ok(()) }
  fn finished(&self) -> bool { false }
  fn cpu_state(&self) -> Cpu;
  fn last_cpu_state(&self) -> Cpu;
  fn instr_addr(&self) -> SegOff;

  fn reg_read(&self, reg: Register) -> Value;
  fn reg_write(&mut self, reg: Register, val: Value);
  fn flag_write(&mut self, f: Flag, set: bool);

  fn mem_slice(&self, addr: SegOff, len: u32) -> &[u8];

  /// Length of the linear memory the backend exposes through `mem_slice`.
  /// Used by the validator to bound a differential memory comparison window.
  /// The default assumes a full 16-bit real-mode address space; backends backed
  /// by a smaller mapping (e.g. dosemu2's shared low-memory alias) override it.
  fn mem_len(&self) -> usize {
    crate::emu86::mem::MEM_SIZE
  }

  fn interrupt_handler(&self, vector: u8) -> Option<SegOff>;

  fn machine(&mut self) -> Option<&mut Machine>;
  fn report(&self);

  fn code_load_seg(&self) -> Seg {
    Seg::Normal(0x823)
  }
}

impl Emu for Emulator {
  fn step(&mut self) -> Result<StepOutcome, String> {
    Self::step(self)?;
    Ok(StepOutcome::single())
  }
  fn cpu_state(&self) -> Cpu {
    self.machine.cpu.clone()
  }
  fn last_cpu_state(&self) -> Cpu {
    self.last_cpu_state.clone()
  }
  fn instr_addr(&self) -> SegOff {
    self.machine.instr_addr()
  }
  fn reg_read(&self, reg: Register) -> Value {
    self.machine.reg_read(reg)
  }
  fn reg_write(&mut self, reg: Register, val: Value) {
    self.machine.reg_write(reg, val)
  }
  fn flag_write(&mut self, f: Flag, set: bool) {
    self.machine.flag_write(f, set)
  }
  fn mem_slice(&self, addr: SegOff, len: u32) -> &[u8] {
    &self.machine.mem.slice_starting_at(addr)[..len as usize]
  }
  fn interrupt_handler(&self, vector: u8) -> Option<SegOff> {
    self.machine.interrupt_vectors[vector as usize]
  }
  fn machine(&mut self) -> Option<&mut Machine> {
    Some(&mut self.machine)
  }
  fn report(&self) {
    self.machine.report().unwrap();
  }
  fn code_load_seg(&self) -> Seg { self.machine.code_load_seg() }
}

pub fn run(exe_path: &str) -> Result<(), String> {
  let mut emu = Emulator::new(exe_path)?;
  emu.run()?;
  Ok(())
}
