use super::super::emu::{Emu, Emulator, LoadConfig, StepOutcome};
use super::super::cpu::*;
use super::hydra_process::HydraProcess;
use super::dosemu_process::DosemuProcess;
use crate::segoff::SegOff;
use super::mirroring::apply_overrides;

enum Interrupt {
  None,
  Pic(SegOff),
}

fn detect_interrupts(emu: &dyn Emu) -> Interrupt {
  let pic_handler = SegOff::new(0x0c77, 0x0096);
  if emu.instr_addr() == pic_handler {
    Interrupt::Pic(pic_handler)
  } else {
    Interrupt::None
  }
}

pub enum EmulatorBackend {
  DosboxX,
  Dosemu2,
}

impl EmulatorBackend {
  pub fn from_env() -> Self {
    match std::env::var("EMULATOR_BACKEND").as_deref() {
      Ok("dosemu2") | Ok("dosemu") => EmulatorBackend::Dosemu2,
      _ => EmulatorBackend::DosboxX,
    }
  }
}

struct Validator {
  hydra: Box<dyn Emu>,
  emu86: Box<dyn Emu>,
}

// DOS EXEC leaves the scratch general registers unspecified, and dosemu2's
// real-mode FLAGS snapshot includes reserved bits which emu86 intentionally
// does not model. The entry address, stack, segments, and all architectural
// status/control flags remain strict.
const DOSEMU_INITIAL_POLICY: InitialStatePolicy = InitialStatePolicy {
  normalize_general_registers: true,
  flags_mask: !super::super::cpu_flags::FLAG_MASK | 0x0002,
  flags_value: 0,
};

fn differing_registers(left: &Cpu, right: &Cpu) -> Vec<&'static str> {
  const REGS: [(&str, Register); 14] = [
    ("AX", AX), ("BX", BX), ("CX", CX), ("DX", DX), ("SI", SI), ("DI", DI),
    ("BP", BP), ("SP", SP), ("IP", IP), ("CS", CS), ("DS", DS), ("ES", ES),
    ("SS", SS), ("FLAGS", FLAGS),
  ];
  REGS.iter().filter_map(|(name, reg)|
    (left.reg_read_u16(*reg) != right.reg_read_u16(*reg)).then_some(*name)).collect()
}

fn compare_initial_states(reference: &Cpu, emu86: &Cpu) -> Result<(), String> {
  let mut reference = reference.clone();
  let mut emu86 = emu86.clone();
  normalize_initial_state(&mut reference, DOSEMU_INITIAL_POLICY);
  normalize_initial_state(&mut emu86, DOSEMU_INITIAL_POLICY);
  let differing = differing_registers(&reference, &emu86);
  if differing.is_empty() { return Ok(()); }
  Err(format!("initial state divergence in {}\n  dosemu2: {}\n  emu86: {}",
    differing.join(", "), reference, emu86))
}

fn advance_candidate(reference: &mut dyn Emu, candidate: &mut dyn Emu) -> Result<StepOutcome, String> {
  let outcome = reference.step()?;
  let known = StepOutcome::MULTI_INSN | StepOutcome::SAME_PC | StepOutcome::FAULT |
    StepOutcome::END_ACK | StepOutcome::TARGET_EXIT;
  if outcome.step_flags & !known != 0 {
    return Err(format!("backend published unknown step flags {:#x}", outcome.step_flags & !known));
  }
  if outcome.has(StepOutcome::END_ACK) {
    return Err("backend published an end acknowledgement during an active step".to_string());
  }
  if outcome.has(StepOutcome::TARGET_EXIT) {
    if outcome.decoded_instructions != 0 {
      return Err("target-exit boundary unexpectedly consumed decoded instructions".to_string());
    }
    return Ok(outcome);
  }
  if outcome.decoded_instructions == 0 {
    return Err("non-terminal boundary consumed zero decoded instructions".to_string());
  }
  if outcome.has(StepOutcome::MULTI_INSN) != (outcome.decoded_instructions > 1) {
    return Err("inconsistent multi-instruction node metadata".to_string());
  }
  for index in 0..outcome.decoded_instructions {
    if let Err(error) = candidate.step() {
      if outcome.has(StepOutcome::FAULT) {
        return Err(format!("both backends faulted while consuming instruction {}/{}: {}",
          index + 1, outcome.decoded_instructions, error));
      }
      return Err(error);
    }
  }
  if outcome.has(StepOutcome::FAULT) {
    return Err(format!("dosemu2 faulted after {} decoded instruction(s), but emu86 did not",
      outcome.decoded_instructions));
  }
  Ok(outcome)
}

impl Validator {
  fn new(exe_path: &str) -> Result<Self, String> {
    Self::new_with_backend(exe_path, EmulatorBackend::from_env())
  }

  fn new_with_backend(exe_path: &str, backend: EmulatorBackend) -> Result<Self, String> {
    let (hydra_impl, emu86_impl): (Box<dyn Emu>, Emulator) = match backend {
      EmulatorBackend::DosboxX => (
        Box::new(HydraProcess::spawn(exe_path)?),
        Emulator::new(exe_path)?,
      ),
      EmulatorBackend::Dosemu2 => {
        // dosemu chooses the PSP at runtime.  It must publish that choice before
        // emu86 loads or every segment and relocation would use a different base.
        let dosemu = DosemuProcess::spawn(exe_path)?;
        let runtime_psp = dosemu.runtime_psp();
        if runtime_psp == 0 {
          return Err("dosemu2 validator published an invalid runtime PSP".to_string());
        }
        let emu86 = Emulator::new_with_load_config(exe_path, LoadConfig { psp_segment: runtime_psp })?;
        compare_initial_states(&dosemu.cpu_state(), &emu86.cpu_state())?;
        (Box::new(dosemu), emu86)
      }
    };

    Ok(Self {
      hydra: hydra_impl,
      emu86: Box::new(emu86_impl),
    })
  }

  fn run(&mut self) -> Result<(), String> {
    let mut count = 0;
    loop {

      let hydra_addr = self.hydra.instr_addr();
      let emu86_addr = self.emu86.instr_addr();
      //println!("Stepping | hydra: {} | emu86: {}", hydra_addr, emu86_addr);
      if count > 90_000 {
        self.emu86.report();
      }
      count += 1;

      let outcome = advance_candidate(self.hydra.as_mut(), self.emu86.as_mut())?;
      if outcome.has(StepOutcome::TARGET_EXIT) || self.hydra.finished() {
        return Ok(());
      }

      // detect interrupt handler firing
      match detect_interrupts(self.hydra.as_ref()) {
        Interrupt::None => (),
        Interrupt::Pic(handler) => {
          // Force the same interrupt to trigger on emu86
          let m = self.emu86.machine().unwrap();
          m.interrupt_save();
          m.reg_write_addr(CS, IP, handler);
        }
      }

      apply_overrides(hydra_addr, self.hydra.as_mut(), self.emu86.as_mut());

      if !self.match_states() {
        self.failure(hydra_addr, emu86_addr);
      }
    }
  }

  fn match_states(&mut self) -> bool {
    let hydra_state = self.hydra.cpu_state();
    let emu86_state = self.emu86.cpu_state();

    if hydra_state.reg_read_u16(AX) != emu86_state.reg_read_u16(AX) { return false; }
    if hydra_state.reg_read_u16(BX) != emu86_state.reg_read_u16(BX) { return false; }
    if hydra_state.reg_read_u16(CX) != emu86_state.reg_read_u16(CX) { return false; }
    if hydra_state.reg_read_u16(DX) != emu86_state.reg_read_u16(DX) { return false; }
    if hydra_state.reg_read_u16(SI) != emu86_state.reg_read_u16(SI) { return false; }
    if hydra_state.reg_read_u16(DI) != emu86_state.reg_read_u16(DI) { return false; }
    if hydra_state.reg_read_u16(BP) != emu86_state.reg_read_u16(BP) { return false; }
    if hydra_state.reg_read_u16(SP) != emu86_state.reg_read_u16(SP) { return false; }
    if hydra_state.reg_read_u16(IP) != emu86_state.reg_read_u16(IP) { return false; }
    if hydra_state.reg_read_u16(CS) != emu86_state.reg_read_u16(CS) { return false; }
    if hydra_state.reg_read_u16(DS) != emu86_state.reg_read_u16(DS) { return false; }
    if hydra_state.reg_read_u16(ES) != emu86_state.reg_read_u16(ES) { return false; }
    if hydra_state.reg_read_u16(SS) != emu86_state.reg_read_u16(SS) { return false; }
    if hydra_state.reg_read_u16(FLAGS) != emu86_state.reg_read_u16(FLAGS) { return false; }

    true
  }

  fn failure(&mut self, hydra_addr: SegOff, emu86_addr: SegOff) {
    eprintln!("");
    eprintln!("State divergence:");
    eprintln!("  hydra  @  {}", hydra_addr);
    eprintln!("  emu86  @  {}", emu86_addr);
    eprintln!("");
    eprintln!("hydra changes:");
    print_changes(&self.hydra.last_cpu_state(), &self.hydra.cpu_state());
    eprintln!("");
    eprintln!("emu86 changes:");
    print_changes(&self.emu86.last_cpu_state(), &self.emu86.cpu_state());
    eprintln!("");
    eprintln!("hydra state:");
    eprintln!("{}", self.hydra.cpu_state());
    eprintln!("");
    eprintln!("emu86 state:");
    eprintln!("{}", self.emu86.cpu_state());
    eprintln!("");
    panic!("STOP");
  }
}

pub fn run(exe_path: &str) -> Result<(), String> {
  Validator::new(exe_path)?.run()
}

pub fn run_with_backend(exe_path: &str, backend: EmulatorBackend) -> Result<(), String> {
  Validator::new_with_backend(exe_path, backend)?.run()
}

fn print_changes(prev: &Cpu, cur: &Cpu) {
  print_change_reg("AX", AX, prev, cur);
  print_change_reg("BX", BX, prev, cur);
  print_change_reg("CX", CX, prev, cur);
  print_change_reg("DX", DX, prev, cur);
  print_change_reg("SI", SI, prev, cur);
  print_change_reg("DI", DI, prev, cur);
  print_change_reg("BP", BP, prev, cur);
  print_change_reg("SP", SP, prev, cur);
  print_change_reg("IP", IP, prev, cur);
  print_change_reg("CS", CS, prev, cur);
  print_change_reg("DS", DS, prev, cur);
  print_change_reg("ES", ES, prev, cur);
  print_change_reg("SS", SS, prev, cur);
  print_change_reg("FLAGS", FLAGS, prev, cur);
}

fn print_change_reg(name: &str, reg: Register, prev: &Cpu, cur: &Cpu) {
  let prev_val = prev.regs[reg.idx as usize];
  let cur_val  = cur.regs[reg.idx as usize];
  if prev_val != cur_val {
    eprintln!("  {} | 0x{:04x} => 0x{:04x}", name, prev_val, cur_val);
  }
}

#[allow(dead_code)]
fn dump_mem(msg: &str, emu: &dyn Emu, addr: SegOff, len: u32) {
  let mem = emu.mem_slice(addr, len);
  let hex = crate::util::hexdump::hexdump(mem);

  println!("Memdump for '{}'", msg);
  println!("----------------------------------------------");
  println!("{}", hex);
}

#[cfg(test)]
mod tests {
  use super::*;
  use super::super::super::cpu_flags::Flag;
  use super::super::super::machine::Machine;
  use super::super::super::value::Value;
  use crate::segoff::Seg;
  use std::collections::VecDeque;

  struct FakeEmu { outcomes: VecDeque<StepOutcome>, steps: u32, cpu: Cpu }

  impl FakeEmu {
    fn new(outcomes: &[StepOutcome]) -> Self {
      Self { outcomes: outcomes.iter().copied().collect(), steps: 0, cpu: Cpu::default() }
    }
  }

  impl Emu for FakeEmu {
    fn step(&mut self) -> Result<StepOutcome, String> {
      self.steps += 1;
      Ok(self.outcomes.pop_front().unwrap_or_else(StepOutcome::single))
    }
    fn cpu_state(&self) -> Cpu { self.cpu.clone() }
    fn last_cpu_state(&self) -> Cpu { self.cpu.clone() }
    fn instr_addr(&self) -> SegOff { SegOff::new(0, 0) }
    fn reg_read(&self, reg: Register) -> Value { self.cpu.reg_read(reg) }
    fn reg_write(&mut self, reg: Register, value: Value) { self.cpu.reg_write(reg, value); }
    fn flag_write(&mut self, _flag: Flag, _set: bool) {}
    fn mem_slice(&self, _addr: SegOff, _len: u32) -> &[u8] { &[] }
    fn interrupt_handler(&self, _vector: u8) -> Option<SegOff> { None }
    fn machine(&mut self) -> Option<&mut Machine> { None }
    fn report(&self) {}
    fn code_load_seg(&self) -> Seg { Seg::Normal(0) }
  }

  #[test]
  fn interrupt_shadow_node_advances_every_decoded_instruction() {
    let node = StepOutcome { decoded_instructions: 2, step_flags: StepOutcome::MULTI_INSN };
    let mut reference = FakeEmu::new(&[node]);
    let mut candidate = FakeEmu::new(&[]);
    assert_eq!(advance_candidate(&mut reference, &mut candidate), Ok(node));
    assert_eq!(candidate.steps, 2);
  }

  #[test]
  fn rep_same_pc_boundary_advances_consumed_instruction() {
    let node = StepOutcome { decoded_instructions: 1, step_flags: StepOutcome::SAME_PC };
    let mut reference = FakeEmu::new(&[node]);
    let mut candidate = FakeEmu::new(&[]);
    assert_eq!(advance_candidate(&mut reference, &mut candidate), Ok(node));
    assert_eq!(candidate.steps, 1);
  }

  #[test]
  fn terminal_boundaries_do_not_advance_candidate() {
    let exit = StepOutcome { decoded_instructions: 0, step_flags: StepOutcome::TARGET_EXIT };
    let mut reference = FakeEmu::new(&[exit]);
    let mut candidate = FakeEmu::new(&[]);
    assert_eq!(advance_candidate(&mut reference, &mut candidate), Ok(exit));
    assert_eq!(candidate.steps, 0);

    let end = StepOutcome { decoded_instructions: 0, step_flags: StepOutcome::END_ACK };
    let mut reference = FakeEmu::new(&[end]);
    assert!(advance_candidate(&mut reference, &mut candidate).unwrap_err().contains("end acknowledgement"));
  }

  #[test]
  fn fault_and_invalid_metadata_are_explicit_errors() {
    let fault = StepOutcome { decoded_instructions: 1, step_flags: StepOutcome::FAULT };
    let mut reference = FakeEmu::new(&[fault]);
    let mut candidate = FakeEmu::new(&[]);
    assert!(advance_candidate(&mut reference, &mut candidate).unwrap_err().contains("but emu86 did not"));
    assert_eq!(candidate.steps, 1);

    let invalid = StepOutcome { decoded_instructions: 2, step_flags: 0 };
    let mut reference = FakeEmu::new(&[invalid]);
    assert!(advance_candidate(&mut reference, &mut candidate).unwrap_err().contains("inconsistent"));
  }

  #[test]
  fn initial_comparison_normalizes_only_observed_dos_variance() {
    let mut dosemu = Cpu::default();
    let mut emu86 = Cpu::default();
    dosemu.reg_write_u16(AX, 0xffff);
    dosemu.reg_write_u16(FLAGS, 0xf202);
    emu86.reg_write_u16(FLAGS, 0x0200);
    assert!(compare_initial_states(&dosemu, &emu86).is_ok());
    dosemu.reg_write_u16(CS, 1);
    let error = compare_initial_states(&dosemu, &emu86).unwrap_err();
    assert!(error.contains("initial state divergence in CS"));
  }
}
