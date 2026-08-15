use super::super::emu::{Emu, Emulator, LoadConfig, StepOutcome};
use super::super::cpu::*;
use super::dosemu_process::DosemuProcess;
use crate::segoff::SegOff;
use super::mirroring::apply_overrides;
use super::fixture::{MemoryWindow, Fixture};

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

struct Validator {
  reference: Box<dyn Emu>,
  emu86: Box<dyn Emu>,
  /// Byte-for-byte memory window compared at every boundary. `None` disables
  /// memory comparison (register-only, matching the pre-corpus behavior).
  memory_window: Option<MemoryWindow>,
}

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

fn synchronize_initial_variance(reference: &dyn Emu, candidate: &mut dyn Emu) {
  for reg in [AX, BX, CX, DX, SI, DI, BP, FLAGS] {
    candidate.reg_write(reg, reference.reg_read(reg));
  }
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
    Self::new_with_window(exe_path, None)
  }

  /// Builds a validator comparing the fixture's deterministic memory window.
  /// The window is resolved against dosemu2's published runtime PSP so the
  /// image-region root matches the actual load segment on both sides.
  fn new_with_fixture(exe_path: &str, fixture: &Fixture) -> Result<Self, String> {
    let dosemu = DosemuProcess::spawn(exe_path)?;
    let runtime_psp = dosemu.runtime_psp();
    if runtime_psp == 0 {
      return Err("dosemu2 validator published an invalid runtime PSP".to_string());
    }
    let mut emu86 = Emulator::new_with_load_config(exe_path, LoadConfig { psp_segment: runtime_psp })?;
    compare_initial_states(&dosemu.cpu_state(), &emu86.cpu_state())?;
    synchronize_initial_variance(&dosemu, &mut emu86);

    let code_seg = runtime_psp + 0x10;
    let window = fixture.window(code_seg);

    Ok(Self {
      reference: Box::new(dosemu),
      emu86: Box::new(emu86),
      memory_window: Some(window),
    })
  }

  fn new_with_window(exe_path: &str, memory_window: Option<MemoryWindow>) -> Result<Self, String> {
    let dosemu = DosemuProcess::spawn(exe_path)?;
    let runtime_psp = dosemu.runtime_psp();
    if runtime_psp == 0 {
      return Err("dosemu2 validator published an invalid runtime PSP".to_string());
    }
    let mut emu86 = Emulator::new_with_load_config(exe_path, LoadConfig { psp_segment: runtime_psp })?;
    compare_initial_states(&dosemu.cpu_state(), &emu86.cpu_state())?;
    synchronize_initial_variance(&dosemu, &mut emu86);

    Ok(Self {
      reference: Box::new(dosemu),
      emu86: Box::new(emu86),
      memory_window,
    })
  }

  fn run(&mut self) -> Result<(), String> {
    let validation = self.run_inner();
    let shutdown = self.reference.shutdown();
    match (validation, shutdown) {
      (Ok(()), result) => result,
      (Err(error), Ok(())) => Err(error),
      (Err(error), Err(shutdown_error)) =>
        Err(format!("{}\nAdditionally, backend shutdown failed: {}", error, shutdown_error)),
    }
  }

  fn run_inner(&mut self) -> Result<(), String> {
    let mut count = 0;
    loop {
      let reference_addr = self.reference.instr_addr();
      let emu86_addr = self.emu86.instr_addr();
      if count > 90_000 {
        self.emu86.report();
      }
      count += 1;

      let outcome = advance_candidate(self.reference.as_mut(), self.emu86.as_mut())?;
      if outcome.has(StepOutcome::TARGET_EXIT) || self.reference.finished() {
        return Ok(());
      }

      match detect_interrupts(self.reference.as_ref()) {
        Interrupt::None => (),
        Interrupt::Pic(handler) => {
          let m = self.emu86.machine().unwrap();
          m.interrupt_save();
          m.reg_write_addr(CS, IP, handler);
        }
      }

      apply_overrides(reference_addr, self.reference.as_mut(), self.emu86.as_mut());
      if !self.match_states() {
        return Err(self.failure(reference_addr, emu86_addr, None));
      }
      if let Some(window) = &self.memory_window {
        if let Some(diff) = compare_memory(self.reference.as_ref(), self.emu86.as_ref(), window) {
          return Err(self.failure(reference_addr, emu86_addr, Some(diff)));
        }
      }
    }
  }

  fn match_states(&mut self) -> bool {
    let reference_state = self.reference.cpu_state();
    let emu86_state = self.emu86.cpu_state();
    for reg in [AX, BX, CX, DX, SI, DI, BP, SP, IP, CS, DS, ES, SS, FLAGS] {
      if reference_state.reg_read_u16(reg) != emu86_state.reg_read_u16(reg) {
        return false;
      }
    }
    true
  }

  fn failure(&mut self, reference_addr: SegOff, emu86_addr: SegOff, mem_diff: Option<MemoryDiff>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
      "\nState divergence:\n  dosemu2 @  {}\n  emu86   @  {}\n\n",
      reference_addr, emu86_addr
    ));
    out.push_str(&format!(
      "dosemu2 changes:\n{}",
      format_changes(&self.reference.last_cpu_state(), &self.reference.cpu_state())
    ));
    out.push_str(&format!(
      "\nemu86 changes:\n{}",
      format_changes(&self.emu86.last_cpu_state(), &self.emu86.cpu_state())
    ));
    out.push_str(&format!(
      "\ndosemu2 state:\n{}emu86 state:\n{}",
      self.reference.cpu_state(), self.emu86.cpu_state()
    ));
    if let Some(diff) = mem_diff {
      out.push_str(&format!(
        "\nmemory divergence at abs 0x{:x}: dosemu2={:02x} emu86={:02x}",
        diff.abs, diff.reference_byte, diff.emu86_byte
      ));
    }
    out
  }
}

/// A single differing byte between the two backends within a compared window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemoryDiff {
  abs: usize,
  reference_byte: u8,
  emu86_byte: u8,
}

/// Byte-by-byte comparison of `window` between two `Emu` backends. Returns the
/// first differing byte, or `None` when the window is identical. Ranges are
/// clamped to the smaller of the two backends' exposed memory lengths.
fn compare_memory(reference: &dyn Emu, candidate: &dyn Emu, window: &MemoryWindow) -> Option<MemoryDiff> {
  let limit = reference.mem_len().min(candidate.mem_len());
  for range in &window.ranges {
    let start = range.start;
    let end = range.end.min(limit);
    if start >= end {
      continue;
    }
    let len = end - start;
    let seg_off = super::fixture::seg_off_from_abs(start);
    let reference_bytes = reference.mem_slice(seg_off, len as u32);
    let candidate_bytes = candidate.mem_slice(seg_off, len as u32);
    for (i, (&ref_b, &cand_b)) in reference_bytes.iter().zip(candidate_bytes.iter()).enumerate() {
      if ref_b != cand_b {
        return Some(MemoryDiff { abs: start + i, reference_byte: ref_b, emu86_byte: cand_b });
      }
    }
  }
  None
}

pub fn run(exe_path: &str) -> Result<(), String> {
  Validator::new(exe_path)?.run()
}

/// Runs the declarative corpus against the pinned dosemu2 runtime. Each
/// fixture is validated independently and divergences are aggregated rather
/// than aborting on the first failure. `dir` is the workdir the fixtures' MZ
/// images are materialized into; the fixture definitions themselves come from
/// the Rust-side declarative list (`corpus_fixtures`), not from disk.
pub fn run_corpus(dir: &std::path::Path) -> Result<(), String> {
  let fixtures = corpus_fixtures()?;
  if fixtures.is_empty() {
    return Err("corpus declared no fixtures".to_string());
  }
  let mut failures = Vec::new();
  for fixture in &fixtures {
    let name = fixture.name.clone();
    let result = (|| -> Result<(), String> {
      let workdir = dir.join(&name);
      std::fs::create_dir_all(&workdir)
        .map_err(|e| format!("failed to create corpus workdir {}: {}", workdir.display(), e))?;
      let exe_path = fixture.write_to(&workdir)
        .map_err(|e| format!("failed to materialize fixture '{}': {}", name, e))?;
      let exe_str = exe_path.to_str()
        .ok_or_else(|| format!("corpus fixture path is not valid UTF-8: {}", exe_path.display()))?;
      let mut validator = Validator::new_with_fixture(exe_str, fixture)?;
      validator.run()
    })();
    match result {
      Ok(()) => println!("corpus[{}]: PASS", name),
      Err(error) => {
        eprintln!("corpus[{}]: FAIL\n{}", name, error);
        failures.push(name);
      }
    }
  }
  if failures.is_empty() {
    Ok(())
  } else {
    Err(format!("corpus diverged in {} of {} fixture(s): {}", failures.len(), fixtures.len(), failures.join(", ")))
  }
}

/// The declarative differential corpus. Each entry becomes one MZ fixture
/// whose image region and declared scratch ranges are compared at every
/// boundary.
///
/// Differential scope is deliberately conservative for now: short, terminating
/// programs using plain register ALU/MOV/XCHG plus the DOS terminate service.
/// The corpus intentionally excludes (with documented reasons to resolve):
///
/// - REP string ops: emu86 completes a REP string op inside one `step()`
///   (whole-REP-per-step) while dosemu2 publishes one SAME_PC node per REP
///   iteration; locking these two stepping models together is an open
///   classification item, so REP scenarios stay host-side (emu86-only) in
///   `fixture.rs` for now;
/// - segment-override memory writes and PUSH/POP stack effects: their
///   differential window (deterministic region vs. DOS-visible region)
///   has not been proven on the pinned runtime yet;
/// - interrupts other than INT 21h/AH=4Ch: only the terminate path is
///   pinned-runtime-proven end to end today.
fn corpus_fixtures() -> Result<Vec<Fixture>, String> {
  let terminating = Fixture::new(
    "terminating_smoke",
    vec![
      0xb8, 0x34, 0x12, // mov ax,1234h
      0xbb, 0xff, 0x00, // mov bx,00ffh
      0x01, 0xd8,       // add ax,bx
      0xb8, 0x00, 0x4c, // mov ax,4c00h
      0xcd, 0x21,       // int 21h
    ],
  );
  // Register mutation plus terminate: exercises node-boundary register
  // comparison over MOV/ADD/XOR/INC/XCHG without touching guest memory.
  let register_smoke = Fixture::new(
    "register_mutation_smoke",
    vec![
      0xb8, 0x34, 0x12, // mov ax,1234h
      0x89, 0xc3,       // mov bx,ax
      0x01, 0xd8,       // add ax,bx
      0x31, 0xc9,       // xor cx,cx
      0x41,             // inc cx
      0x93,             // xchg bx,ax
      0x89, 0xca,       // mov dx,cx
      0xb8, 0x00, 0x4c, // mov ax,4c00h
      0xcd, 0x21,       // int 21h
    ],
  );
  Ok(vec![terminating, register_smoke])
}

fn format_changes(prev: &Cpu, cur: &Cpu) -> String {
  let mut out = String::new();
  format_change_reg(&mut out, "AX", AX, prev, cur);
  format_change_reg(&mut out, "BX", BX, prev, cur);
  format_change_reg(&mut out, "CX", CX, prev, cur);
  format_change_reg(&mut out, "DX", DX, prev, cur);
  format_change_reg(&mut out, "SI", SI, prev, cur);
  format_change_reg(&mut out, "DI", DI, prev, cur);
  format_change_reg(&mut out, "BP", BP, prev, cur);
  format_change_reg(&mut out, "SP", SP, prev, cur);
  format_change_reg(&mut out, "IP", IP, prev, cur);
  format_change_reg(&mut out, "CS", CS, prev, cur);
  format_change_reg(&mut out, "DS", DS, prev, cur);
  format_change_reg(&mut out, "ES", ES, prev, cur);
  format_change_reg(&mut out, "SS", SS, prev, cur);
  format_change_reg(&mut out, "FLAGS", FLAGS, prev, cur);
  out
}

fn format_change_reg(out: &mut String, name: &str, reg: Register, prev: &Cpu, cur: &Cpu) {
  let prev_val = prev.regs[reg.idx as usize];
  let cur_val  = cur.regs[reg.idx as usize];
  if prev_val != cur_val {
    out.push_str(&format!("  {} | 0x{:04x} => 0x{:04x}\n", name, prev_val, cur_val));
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
  use super::super::fixture::MemoryRange;
  use crate::segoff::Seg;
  use std::collections::VecDeque;

  struct FakeEmu {
    outcomes: VecDeque<StepOutcome>,
    steps: u32,
    cpu: Cpu,
    mem: Vec<u8>,
  }

  impl FakeEmu {
    fn new(outcomes: &[StepOutcome]) -> Self {
      Self { outcomes: outcomes.iter().copied().collect(), steps: 0, cpu: Cpu::default(), mem: vec![0u8; 0x10000] }
    }
    fn with_mem(mem: Vec<u8>) -> Self {
      Self { outcomes: VecDeque::new(), steps: 0, cpu: Cpu::default(), mem }
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
    fn mem_slice(&self, addr: SegOff, len: u32) -> &[u8] {
      &self.mem[addr.abs_normal()..][..len as usize]
    }
    fn mem_len(&self) -> usize { self.mem.len() }
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

  #[test]
  fn initial_variance_is_synchronized_for_strict_later_comparisons() {
    let mut reference = FakeEmu::new(&[]);
    let mut candidate = FakeEmu::new(&[]);
    reference.cpu.reg_write_u16(BX, 0x1234);
    reference.cpu.reg_write_u16(FLAGS, 0xf202);
    synchronize_initial_variance(&reference, &mut candidate);
    assert_eq!(candidate.cpu.reg_read_u16(BX), 0x1234);
    assert_eq!(candidate.cpu.reg_read_u16(FLAGS), 0xf202);
    assert_eq!(candidate.cpu.reg_read_u16(SP), 0);
  }

  #[test]
  fn compare_memory_reports_first_differing_byte() {
    let reference = FakeEmu::with_mem(vec![0x00; 0x100]);
    let mut candidate = FakeEmu::with_mem(vec![0x00; 0x100]);
    candidate.mem[0x30] = 0xab;
    candidate.mem[0x10] = 0xcd;
    let window = MemoryWindow { ranges: vec![MemoryRange { start: 0x10, end: 0x80 }] };
    let diff = compare_memory(&reference, &candidate, &window).unwrap();
    assert_eq!(diff.abs, 0x10);
    assert_eq!(diff.reference_byte, 0x00);
    assert_eq!(diff.emu86_byte, 0xcd);
  }

  #[test]
  fn compare_memory_is_identical_when_buffers_match() {
    let reference = FakeEmu::with_mem(vec![0x5a; 0x100]);
    let candidate = FakeEmu::with_mem(vec![0x5a; 0x100]);
    let window = MemoryWindow { ranges: vec![MemoryRange { start: 0x0, end: 0x100 }] };
    assert_eq!(compare_memory(&reference, &candidate, &window), None);
  }

  #[test]
  fn compare_memory_clamps_to_smaller_backend() {
    let reference = FakeEmu::with_mem(vec![0x11; 0x40]);
    let candidate = FakeEmu::with_mem(vec![0x11; 0x100]);
    // Window extends beyond the reference backend's 0x40-length buffer.
    let window = MemoryWindow { ranges: vec![MemoryRange { start: 0x0, end: 0x100 }] };
    assert_eq!(compare_memory(&reference, &candidate, &window), None);
  }

  #[test]
  fn compare_memory_skips_ranges_past_the_limit() {
    let reference = FakeEmu::with_mem(vec![0x11; 0x40]);
    let candidate = FakeEmu::with_mem(vec![0x11; 0x40]);
    let window = MemoryWindow { ranges: vec![MemoryRange { start: 0x1000, end: 0x2000 }] };
    assert_eq!(compare_memory(&reference, &candidate, &window), None);
  }
}
