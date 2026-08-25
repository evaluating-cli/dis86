//! SST 80286 runner: executes hardware-captured MOO tests against emu86.
//!
//! `run_test` executes a single test's first instruction (via exactly one
//! `Machine::step()`), then compares the resulting machine state against the
//! sparse final state captured by the hardware. `run_file` runs a strided
//! subset of a whole suite and buckets the outcomes.

use moo::prelude::*;
use moo::registers::{MooRegisters, MooRegisters16};
use moo::types::MooRamEntry;
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::asm::decode::decode_one;
use crate::asm::instr::{Opcode, Operand, OperandReg, Reg};
use crate::emu86::machine::*;
use crate::region::RegionIter;

/// Default set of FLAGS bits that the runner compares (emu86's FLAG_MASK).
pub const DEFAULT_FLAGS_UMASK: u16 = 0x0FD7;

/// Names of the 14 emu86 registers in order (AX..FLAGS), matching
/// `Machine::cpu.regs` indices 0..13.
pub const REG_NAMES: [&str; 14] = [
  "AX", "BX", "CX", "DX", "SI", "DI", "BP", "SP",
  "IP", "CS", "DS", "ES", "SS", "FLAGS",
];

const IP_IDX: usize = 8;
const FLAGS_IDX: usize = 13;
const CX_IDX: usize = 1;

/// Maximum number of `unexpected_writes` entries retained on an `Outcome::Fail`
/// to keep memory-bounded reporting sane.
const MAX_UNEXPECTED_SAMPLES: usize = 8;

/// Per-instruction run options.
#[derive(Clone, Copy, Debug)]
pub struct RunOpts {
  /// Baseline set of FLAGS bits to compare. When `count_sensitive_flags` is
  /// enabled, shift/rotate undefined bits are refined from the decoded test's
  /// effective count so count==1 defined OF cannot be hidden by a form-wide mask.
  pub flags_umask: u16,
  /// If true, track memory writes and report final changes not declared in the
  /// test's `final.ram` as `unexpected_writes`. Tracking records only touched
  /// addresses; it does not clone the whole memory image.
  pub check_extra_writes: bool,
  /// Refine shift/rotate FLAGS definedness using the effective count. Disable
  /// only for an explicit diagnostic `--umask`, where the caller's mask is exact.
  pub count_sensitive_flags: bool,
}

impl Default for RunOpts {
  fn default() -> Self {
    RunOpts {
      flags_umask: DEFAULT_FLAGS_UMASK,
      check_extra_writes: true,
      count_sensitive_flags: true,
    }
  }
}

/// Detail of a FLAGS mismatch, present only when unmasked bits differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagsDiff {
  pub expected: u16,
  pub actual: u16,
  pub umask: u16,
}

/// Classification of a single test run.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
  Pass,
  Fail {
    reg_diffs: Vec<(String, u16, u16)>,
    ram_diffs: Vec<(u32, u8, u8)>,
    unexpected_writes: Vec<(u32, u8, u8)>,
    unexpected_writes_total: usize,
    flags_diff: Option<FlagsDiff>,
  },
  DecodeErr(String),
  Panic(String),
  SkipException { num: u8 },
  Skip32BitState,
  InvalidVector(String),
}

/// A single classified sample for reporting (capped per bucket).
#[derive(Debug, Clone)]
pub struct Sample {
  pub idx: usize,
  pub name: String,
  pub hash: String,
  pub detail: String,
}

/// Aggregate results for one file.
#[derive(Debug, Clone, Default)]
pub struct FileSummary {
  pub filename: String,
  pub total: usize,
  /// Tests walked (strided subset size).
  pub visited: usize,
  /// Tests actually executed via [`run_test`] (visited minus filtered).
  pub executed: usize,
  pub pass: usize,
  pub fail: usize,
  pub decode_err: usize,
  pub panic: usize,
  pub skip_exception: usize,
  pub skip_32bit: usize,
  pub filtered: usize,
  /// Tests skipped because their SHA1 is on the pinned upstream revocation list.
  pub revoked: usize,
  pub samples: Vec<Sample>,
}

impl Outcome {
  /// A short human-readable bucket tag.
  pub fn bucket_name(&self) -> &'static str {
    match self {
      Outcome::Pass => "PASS",
      Outcome::Fail { .. } => "FAIL",
      Outcome::DecodeErr(_) => "DECODE_ERR",
      Outcome::Panic(_) => "PANIC",
      Outcome::SkipException { .. } => "SKIP_EXCEPTION",
      Outcome::Skip32BitState => "SKIP_32BIT",
      Outcome::InvalidVector(_) => "INVALID_VECTOR",
    }
  }
}

/// Return true if the test should be skipped by the conservative prefix filter.
///
/// The filter skips tests whose first byte is a segment/operand/address prefix
/// or REP/LOCK prefix byte — EXCEPT when the opcode (after stripping prefixes)
/// is a string op (A4-AF), where the prefix is semantically meaningful (REP,
/// segment-override, or LOCK on MOVS/CMPS/STOS/LODS/SCAS) and emu86 implements
/// it. This scoped exception lets the string family run under the conservative
/// filter without un-filtering prefix tests for non-string forms.
pub fn is_prefix_filtered(bytes: &[u8]) -> bool {
  // Strip leading prefix bytes to find the opcode.
  let mut i = 0;
  while i < bytes.len() && matches!(bytes[i], 0x26 | 0x2E | 0x36 | 0x3E | 0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3) {
    i += 1;
  }
  // String ops (A4-A7, AA-AF) with a prefix are valid and implemented — keep them.
  if i < bytes.len() && matches!(bytes[i], 0xA4..=0xA7 | 0xAA..=0xAF) {
    return false;
  }
  // Otherwise, filter iff the first byte is a prefix (original conservative rule).
  matches!(bytes.first(), Some(b) if matches!(b, 0x26 | 0x2E | 0x36 | 0x3E | 0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3))
}

fn reg16(fin: &MooRegisters16, i: usize) -> Option<u16> {
  match i {
    0 => fin.ax(),
    1 => fin.bx(),
    2 => fin.cx(),
    3 => fin.dx(),
    4 => fin.si(),
    5 => fin.di(),
    6 => fin.bp(),
    7 => fin.sp(),
    8 => fin.ip(),
    9 => fin.cs(),
    10 => fin.ds(),
    11 => fin.es(),
    12 => fin.ss(),
    13 => fin.flags(),
    _ => unreachable!(),
  }
}

fn init_reg16(init: &MooRegisters16, i: usize) -> u16 {
  reg16(init, i).unwrap_or(0)
}

/// Extract the initial 14-register vector (indices match `REG_NAMES`).
fn initial_regs(init: &MooRegisters16) -> [u16; 14] {
  [
    init_reg16(init, 0),
    init_reg16(init, 1),
    init_reg16(init, 2),
    init_reg16(init, 3),
    init_reg16(init, 4),
    init_reg16(init, 5),
    init_reg16(init, 6),
    init_reg16(init, 7),
    init_reg16(init, 8),
    init_reg16(init, 9),
    init_reg16(init, 10),
    init_reg16(init, 11),
    init_reg16(init, 12),
    init_reg16(init, 13),
  ]
}

/// Refine FLAGS definedness for group-2 shift/rotate operations using the
/// *effective* 80286 count. The static per-form policy remains the baseline,
/// but it cannot express count==0 / count==1 / count>1 differences.
fn effective_flags_umask(test: &MooTest, init: &[u16; 14], opts: &RunOpts) -> u16 {
  if !opts.count_sensitive_flags {
    return opts.flags_umask;
  }

  let mut bin = RegionIter::new(test.bytes(), SegOff::new(0, 0));
  let instr = match decode_one(&mut bin) {
    Ok(Some((instr, _))) => instr,
    _ => return opts.flags_umask,
  };

  let count = match instr.operands.get(1) {
    Some(Operand::Imm(imm)) => (imm.val as u8) & 0x1f,
    Some(Operand::Reg(OperandReg(Reg::CL))) => (init[CX_IDX] as u8) & 0x1f,
    _ => return opts.flags_umask,
  };

  match instr.opcode {
    Opcode::OP_SHL | Opcode::OP_SHR | Opcode::OP_SAR => {
      // Shift forms historically carry form-wide masks that drop AF and, for
      // variable/immediate counts, OF. Restore them first, then remove only the
      // bits that are actually undefined for this test's effective count.
      let mut mask = opts.flags_umask | FLAG_AF | FLAG_OF;
      if count != 0 {
        mask &= !FLAG_AF; // AF undefined for a non-zero shift.
      }
      if count > 1 {
        mask &= !FLAG_OF; // OF defined only for count==1; count==0 preserves it.
      }
      mask
    }
    Opcode::OP_ROL => {
      // ROL preserves SF/ZF/PF/AF. CF is defined for non-zero counts and OF is
      // defined only for count==1; count==0 is a complete FLAGS-preserving no-op.
      let mut mask = opts.flags_umask | FLAG_AF | FLAG_OF;
      if count > 1 {
        mask &= !FLAG_OF;
      }
      mask
    }
    _ => opts.flags_umask,
  }
}

/// Write the test's initial RAM (sparse) into the machine's flat memory.
/// Returns an `InvalidVector` outcome (without running) if any address is out
/// of bounds, or `Ok(())` on success.
fn write_initial_ram(machine: &mut Machine, ram: &[MooRamEntry]) -> Result<(), Outcome> {
  for e in ram {
    let addr = e.address as usize;
    if addr >= machine.mem.0.len() {
      return Err(Outcome::InvalidVector(format!(
        "initial ram address 0x{:05X} out of bounds (len 0x{:05X})",
        addr,
        machine.mem.0.len()
      )));
    }
    machine.mem.0[addr] = e.value;
  }
  Ok(())
}

/// Apply the 14 initial registers to the machine.
fn set_registers(machine: &mut Machine, init: &[u16; 14]) {
  for i in 0..14 {
    if i == FLAGS_IDX {
      machine.flag_write_all(Flags(init[i]));
    } else {
      machine.cpu.regs[i] = init[i];
    }
  }
}

fn read_actual_regs(machine: &Machine) -> [u16; 14] {
  let mut actual = [0u16; 14];
  for i in 0..14 {
    actual[i] = machine.cpu.regs[i];
  }
  actual
}

/// Compare the post-step machine against the expected (sparse) final state.
/// Returns `None` if there were no divergences, else the diff vectors.
fn compare_state(
  machine: &Machine,
  init: &[u16; 14],
  fin: &MooRegisters16,
  fin_ram: &[MooRamEntry],
  opts: &RunOpts,
) -> Option<(Vec<(String, u16, u16)>, Vec<(u32, u8, u8)>, Vec<(u32, u8, u8)>, usize, Option<FlagsDiff>)> {
  let actual = read_actual_regs(machine);

  let mut reg_diffs: Vec<(String, u16, u16)> = Vec::new();
  let mut flags_diff: Option<FlagsDiff> = None;

  for i in 0..14 {
    let fin_reg = reg16(fin, i);
    // An absent final register means "unchanged" (expected == initial), except
    // for IP where the suite's terminating-HALT convention still applies: the
    // reference records IP one past the HALT byte it executed, while emu86
    // stops one step before it, so a listed final IP is compared with -1. An
    // absent final IP means the reference's post-HALT IP wrapped back to the
    // initial IP (short branch onto a HALT byte at init-1), so emu86's one
    // step lands at init-1 and the same -1 applies.
    let expected = match (fin_reg, i == IP_IDX) {
      (Some(v), true) => v.wrapping_sub(1),
      (Some(v), false) => v,
      (None, true) => init[i].wrapping_sub(1),
      (None, _) => init[i],
    };
    let actual_val = actual[i];
    if i == FLAGS_IDX {
      let mismatch = (actual_val ^ expected) & opts.flags_umask;
      if mismatch != 0 {
        flags_diff = Some(FlagsDiff { expected, actual: actual_val, umask: opts.flags_umask });
      }
    } else if actual_val != expected {
      reg_diffs.push((REG_NAMES[i].to_string(), expected, actual_val));
    }
  }

  let mut ram_diffs: Vec<(u32, u8, u8)> = Vec::new();
  for e in fin_ram {
    let addr = e.address as usize;
    let actual_byte = machine.mem.0.get(addr).copied().unwrap_or(0);
    if actual_byte != e.value {
      ram_diffs.push((e.address, e.value, actual_byte));
    }
  }

  let mut unexpected: Vec<(u32, u8, u8)> = Vec::new();
  let mut unexpected_total: usize = 0;
  if let Some(writes) = machine.mem.tracked_write_originals() {
    let mut fin_map: Vec<u32> = fin_ram.iter().map(|e| e.address).collect();
    fin_map.sort_unstable();
    let mut tracked: Vec<(usize, u8)> = writes.iter().map(|(&addr, &before)| (addr, before)).collect();
    tracked.sort_unstable_by_key(|(addr, _)| *addr);
    for (addr, before) in tracked {
      let after = machine.mem.0[addr];
      if before != after && fin_map.binary_search(&(addr as u32)).is_err() {
        unexpected_total += 1;
        if unexpected.len() < MAX_UNEXPECTED_SAMPLES {
          unexpected.push((addr as u32, before, after));
        }
      }
    }
  }

  let any_divergence = !reg_diffs.is_empty()
    || !ram_diffs.is_empty()
    || unexpected_total > 0
    || flags_diff.is_some();

  if any_divergence {
    Some((reg_diffs, ram_diffs, unexpected, unexpected_total, flags_diff))
  } else {
    None
  }
}

/// Run a single MOO test against emu86 and classify the outcome.
pub fn run_test(test: &MooTest, opts: &RunOpts) -> Outcome {
  // 1. Exception-expected tests are skipped entirely.
  if let Some(ex) = test.exception() {
    return Outcome::SkipException { num: ex.exception_num };
  }

  // 2. Only the real-mode 16-bit register variant is supported.
  let (init, fin) = match (&test.initial_state().regs(), &test.final_state().regs()) {
    (MooRegisters::Sixteen(init), MooRegisters::Sixteen(fin)) => (init.clone(), *fin),
    _ => return Outcome::Skip32BitState,
  };

  let init_regs = initial_regs(&init);
  let mut compare_opts = *opts;
  compare_opts.flags_umask = effective_flags_umask(test, &init_regs, opts);

  let mut machine = Machine::new(None);

  // 3. Write initial RAM sparsely; bounds-checked.
  if let Err(outcome) = write_initial_ram(&mut machine, test.initial_state().ram()) {
    return outcome;
  }

  // 4. Set all 14 registers.
  set_registers(&mut machine, &init_regs);

  // 5. Enable address-level tracking after setup so initial-state writes do not
  // count. This is cheap enough to remain enabled for the full hardware sweep.
  if opts.check_extra_writes {
    machine.mem.begin_write_tracking();
  }

  // 6. Execute exactly one instruction, catching panics.
  let step_result = catch_unwind(AssertUnwindSafe(|| machine.step()));

  match step_result {
    Ok(Ok(())) => {}
    Ok(Err(e)) => return Outcome::DecodeErr(e),
    Err(payload) => {
      let msg = if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
      } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
      } else {
        "unknown panic payload".to_string()
      };
      return Outcome::Panic(msg);
    }
  }

  // 7-10. Compare registers + RAM + unexpected writes, then bucket.
  let fin_ram = test.final_state().ram();
  match compare_state(&machine, &init_regs, &fin, fin_ram, &compare_opts) {
    None => Outcome::Pass,
    Some((reg_diffs, ram_diffs, unexpected, unexpected_total, flags_diff)) => Outcome::Fail {
      reg_diffs,
      ram_diffs,
      unexpected_writes: unexpected,
      unexpected_writes_total: unexpected_total,
      flags_diff,
    },
  }
}

/// Render a human-readable detail string for a sample.
pub fn sample_detail(_test: &MooTest, outcome: &Outcome) -> String {
  match outcome {
    Outcome::Pass => "pass".to_string(),
    Outcome::DecodeErr(e) => format!("decode: {}", e),
    Outcome::Panic(e) => format!("panic: {}", e),
    Outcome::SkipException { num } => format!("exception #{}", num),
    Outcome::Skip32BitState => "32-bit state".to_string(),
    Outcome::InvalidVector(e) => format!("invalid: {}", e),
    Outcome::Fail { reg_diffs, ram_diffs, unexpected_writes, unexpected_writes_total, flags_diff } => {
      let mut parts: Vec<String> = Vec::new();
      if let Some(fd) = flags_diff {
        parts.push(format!(
          "flags exp=0x{:04X} act=0x{:04X} umask=0x{:04X}",
          fd.expected, fd.actual, fd.umask
        ));
      }
      for (name, exp, act) in reg_diffs {
        // FLAGS is reported via flags_diff above; don't render it twice.
        if name == "FLAGS" && flags_diff.is_some() {
          continue;
        }
        parts.push(format!("{} exp=0x{:04X} act=0x{:04X}", name, exp, act));
      }
      for (addr, exp, act) in ram_diffs {
        parts.push(format!("[0x{:05X}] exp=0x{:02X} act=0x{:02X}", addr, exp, act));
      }
      if *unexpected_writes_total > 0 {
        let shown = unexpected_writes
          .iter()
          .map(|(a, i, v)| format!("[0x{:05X}] {}->{}", a, i, v))
          .collect::<Vec<_>>()
          .join(" ");
        parts.push(format!(
          "unexpected_writes {} (shown: {})",
          unexpected_writes_total, shown
        ));
      }
      if parts.is_empty() {
        "fail".to_string()
      } else {
        parts.join("; ")
      }
    }
  }
}

fn push_sample(summary: &mut FileSummary, idx: usize, test: &MooTest, outcome: &Outcome, cap: usize) {
  if summary.samples.len() >= cap {
    return;
  }
  summary.samples.push(Sample {
    idx,
    name: test.name().to_string(),
    hash: test.hash_string(),
    detail: sample_detail(test, outcome),
  });
}

/// Run a strided subset of a whole test file and bucket the results.
///
/// Deterministic and single-threaded. `sample_stride` selects `tests[i]` for
/// `i % sample_stride == 0`. `conservative_filter` skips (and counts as
/// `filtered`, never `pass`/`skip`) any test whose first byte is a prefix byte.
/// `skip_hashes` contains SHA1 hashes from the pinned upstream revocation list:
/// matching tests are counted in the `revoked` bucket and never executed.
/// `on_progress(completed, visited)` is called after each visited test.
pub fn run_file(
  tests: &[MooTest],
  opts: &RunOpts,
  sample_stride: usize,
  conservative_filter: bool,
  skip_hashes: Option<&HashSet<String>>,
  mut on_progress: impl FnMut(usize, usize),
) -> FileSummary {
  let stride = sample_stride.max(1);
  let mut summary = FileSummary::default();
  summary.total = tests.len();

  let mut completed = 0usize;
  for (i, test) in tests.iter().enumerate() {
    if i % stride != 0 {
      continue;
    }
    let visited = completed + 1;
    summary.visited += 1;

    if let Some(skip) = skip_hashes {
      if skip.contains(&test.hash_string()) {
        summary.revoked += 1;
        completed += 1;
        on_progress(completed, visited);
        continue;
      }
    }

    if conservative_filter && is_prefix_filtered(test.bytes()) {
      summary.filtered += 1;
      completed += 1;
      on_progress(completed, visited);
      continue;
    }

    summary.executed += 1;
    let outcome = run_test(test, opts);
    match &outcome {
      Outcome::Pass => summary.pass += 1,
      Outcome::Fail { .. } => {
        summary.fail += 1;
        push_sample(&mut summary, i, test, &outcome, 5);
      }
      Outcome::DecodeErr(_) => {
        summary.decode_err += 1;
        push_sample(&mut summary, i, test, &outcome, 5);
      }
      Outcome::Panic(_) => {
        summary.panic += 1;
        push_sample(&mut summary, i, test, &outcome, 5);
      }
      Outcome::SkipException { .. } => summary.skip_exception += 1,
      Outcome::Skip32BitState => summary.skip_32bit += 1,
      Outcome::InvalidVector(_) => {
        // Counted alongside decode errors for reporting purposes.
        summary.decode_err += 1;
        push_sample(&mut summary, i, test, &outcome, 5);
      }
    }

    completed += 1;
    on_progress(completed, visited);
  }

  summary
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::emu86::sst::policy::policy_for_file;
  use moo::prelude::MooRegistersInit;
  use moo::registers::{MooRegisters16Init, MooRegisters32Init};
  use moo::types::{MooException, MooStateType, MooTestState};

  fn ram(entries: &[(u32, u8)]) -> Vec<MooRamEntry> {
    entries.iter().map(|&(a, v)| MooRamEntry { address: a, value: v }).collect()
  }

  fn base_init() -> MooRegisters16Init {
    MooRegisters16Init {
      ax: 0, bx: 0, cx: 0, dx: 0, cs: 0, ss: 0, ds: 0, es: 0,
      sp: 0, bp: 0, si: 0, di: 0, ip: 0, flags: 0x0002,
    }
  }

  fn make_test(
    name: &str,
    bytes: &[u8],
    init: MooRegisters16Init,
    fin: MooRegisters16Init,
    init_ram: Vec<MooRamEntry>,
    fin_ram: Vec<MooRamEntry>,
  ) -> MooTest {
    let init_state =
      MooTestState::new(MooStateType::Initial, &MooRegistersInit::Sixteen(init.clone()), None, None, vec![], init_ram);
    let fin_state = MooTestState::new(
      MooStateType::Final,
      &MooRegistersInit::Sixteen(init),
      Some(&MooRegistersInit::Sixteen(fin)),
      None,
      vec![],
      fin_ram,
    );
    MooTest::new(name.to_string(), None, bytes, init_state, fin_state, &[], None, None)
  }

  /// `ADD AL,5` (04 05) + terminating HALT (F4), code at CS:IP = 0:0.
  fn add_test(init_ax: u16, exp_ax: u16, init_flags: u16, exp_flags: u16) -> MooTest {
    let mut init = base_init();
    init.ax = init_ax;
    init.flags = init_flags;
    let mut fin = init.clone();
    fin.ax = exp_ax;
    fin.ip = 3; // initial_ip(0) + 2 instr bytes + 1 HALT
    fin.flags = exp_flags;
    let init_ram = ram(&[(0x00000, 0x04), (0x00001, 0x05), (0x00002, 0xF4)]);
    make_test("add al,5", &[0x04, 0x05, 0xF4], init, fin, init_ram, vec![])
  }

  fn expect_fail_ax(outcome: &Outcome, expected_ax: u16, actual_ax: u16) {
    match outcome {
      Outcome::Fail { reg_diffs, flags_diff, .. } => {
        assert!(
          reg_diffs.contains(&("AX".to_string(), expected_ax, actual_ax)),
          "expected AX reg diff ({:04X},{:04X}) in {:?}",
          expected_ax,
          actual_ax,
          reg_diffs
        );
        assert!(flags_diff.is_none(), "flags should match here, got {:?}", flags_diff);
      }
      other => panic!("expected Fail, got {:?}", other),
    }
  }

  #[test]
  fn pass_add_al_5() {
    let test = add_test(0x7F, 0x84, 0x0002, 0x0896);
    let outcome = run_test(&test, &RunOpts::default());
    assert_eq!(outcome, Outcome::Pass, "outcome: {:?}", outcome);
  }

  #[test]
  fn ip_halt_normalization() {
    // final ip is listed as initial_ip + 3 (2 instr bytes + 1 HALT); runner
    // normalizes expected emu86 IP by -1.
    let test = add_test(0x7F, 0x84, 0x0002, 0x0896);
    let fin = test.final_state().regs();
    let MooRegisters::Sixteen(fin16) = fin else { panic!("expected 16-bit") };
    assert_eq!(fin16.ip(), Some(3));
    let outcome = run_test(&test, &RunOpts::default());
    assert_eq!(outcome, Outcome::Pass, "outcome: {:?}", outcome);
  }

  #[test]
  fn sparse_retention_unchanged_regs_do_not_fail() {
    // final regs absent for BX..SP, CS..SS -> expected == initial; ADD does not
    // touch them, so the test must Pass.
    let test = add_test(0x7F, 0x84, 0x0002, 0x0896);
    let fin = test.final_state().regs();
    let MooRegisters::Sixteen(fin16) = fin else { panic!("expected 16-bit") };
    assert_eq!(fin16.bx(), None);
    assert_eq!(fin16.cs(), None);
    let outcome = run_test(&test, &RunOpts::default());
    assert_eq!(outcome, Outcome::Pass, "outcome: {:?}", outcome);
  }

  #[test]
  fn fail_wrong_expected_ax() {
    let test = add_test(0x7F, 0x85, 0x0002, 0x0896);
    let outcome = run_test(&test, &RunOpts::default());
    expect_fail_ax(&outcome, 0x85, 0x84);
  }

  #[test]
  fn skip_exception_does_not_step() {
    let init = base_init();
    let mut fin = init.clone();
    fin.ax = 0x10; // would otherwise be a diff if executed
    let init_ram = ram(&[(0x00000, 0x90), (0x00001, 0xF4)]);
    let init_state =
      MooTestState::new(MooStateType::Initial, &MooRegistersInit::Sixteen(init.clone()), None, None, vec![], init_ram);
    let fin_state = MooTestState::new(
      MooStateType::Final,
      &MooRegistersInit::Sixteen(init),
      Some(&MooRegistersInit::Sixteen(fin)),
      None,
      vec![],
      vec![],
    );
    let test = MooTest::new(
      "nop-exc".to_string(),
      None,
      &[0x90, 0xF4],
      init_state,
      fin_state,
      &[],
      Some(MooException { exception_num: 13, flag_address: 0 }),
      None,
    );
    assert_eq!(run_test(&test, &RunOpts::default()), Outcome::SkipException { num: 13 });
  }

  #[test]
  fn decode_err_invalid_opcode() {
    // 0x64 is OP_INVAL in emu86's table -> decode fails.
    let init = base_init();
    let mut fin = init.clone();
    fin.ip = 3;
    let init_ram = ram(&[(0x00000, 0x64), (0x00001, 0xF4)]);
    let test = make_test("inval", &[0x64, 0xF4], init, fin, init_ram, vec![]);
    match run_test(&test, &RunOpts::default()) {
      Outcome::DecodeErr(_) => {}
      other => panic!("expected DecodeErr, got {:?}", other),
    }
  }

  #[test]
  fn panic_bucket_repeated_prefix_on_non_string() {
    // F3 27 = REP DAA: decodes, but REP on a non-string op panics at
    // step.rs (REP prefix is not yet implemented). Must be a Panic, not abort.
    let init = base_init();
    let mut fin = init.clone();
    fin.ip = 4;
    let init_ram = ram(&[(0x00000, 0xF3), (0x00001, 0x27), (0x00002, 0xF4)]);
    let test = make_test("rep daa", &[0xF3, 0x27, 0xF4], init, fin, init_ram, vec![]);
    match run_test(&test, &RunOpts::default()) {
      Outcome::Panic(_) => {}
      other => panic!("expected Panic, got {:?}", other),
    }
  }

  fn mov_test(fin_ram: Vec<MooRamEntry>) -> MooTest {
    // C6 06 00 00 5A = MOV BYTE PTR ds:0x0000, 0x5A ; F4 HALT
    // CS=0,IP=0 -> code at linear 0. DS=0x2000 -> target linear 0x20000.
    let mut init = base_init();
    init.ds = 0x2000;
    let mut fin = init.clone();
    fin.ip = 6; // 5 instr bytes + 1 HALT
    let init_ram = ram(&[
      (0x00000, 0xC6),
      (0x00001, 0x06),
      (0x00002, 0x00),
      (0x00003, 0x00),
      (0x00004, 0x5A),
      (0x00005, 0xF4),
      (0x20000, 0x00),
    ]);
    make_test("mov byte [abs],imm", &[0xC6, 0x06, 0x00, 0x00, 0x5A, 0xF4], init, fin, init_ram, fin_ram)
  }

  #[test]
  fn unexpected_write_reported_when_not_in_final_ram() {
    let test = mov_test(vec![]);
    match run_test(&test, &RunOpts::default()) {
      Outcome::Fail { unexpected_writes, unexpected_writes_total, reg_diffs, ram_diffs, .. } => {
        assert_eq!(unexpected_writes_total, 1, "reg: {:?} ram: {:?}", reg_diffs, ram_diffs);
        assert!(unexpected_writes.contains(&(0x20000, 0x00, 0x5A)), "got {:?}", unexpected_writes);
      }
      other => panic!("expected Fail, got {:?}", other),
    }
  }

  #[test]
  fn write_declared_in_final_ram_is_not_unexpected() {
    let test = mov_test(ram(&[(0x20000, 0x5A)]));
    let outcome = run_test(&test, &RunOpts::default());
    assert_eq!(outcome, Outcome::Pass, "outcome: {:?}", outcome);
  }

  #[test]
  fn flags_umask_honored() {
    // Expected flags differ from emu86's actual only in bit 3 (reserved,
    // outside 0x0FD7). With default umask it must Pass...
    let test = add_test(0x7F, 0x84, 0x0002, 0x089E); // 0x0896 | bit3
    assert_eq!(run_test(&test, &RunOpts::default()), Outcome::Pass);

    // ...and with a umask that also compares bit 3 it must Fail.
    let opts = RunOpts {
      flags_umask: DEFAULT_FLAGS_UMASK | 0x0008,
      count_sensitive_flags: false,
      ..RunOpts::default()
    };
    match run_test(&test, &opts) {
      Outcome::Fail { flags_diff, .. } => {
        let fd = flags_diff.expect("flags_diff must be present");
        assert_eq!(fd.expected, 0x089E);
        assert_eq!(fd.actual, 0x0896);
        assert_eq!(fd.umask, DEFAULT_FLAGS_UMASK | 0x0008);
      }
      other => panic!("expected Fail, got {:?}", other),
    }
  }

  #[test]
  fn shift_count_one_reenables_defined_of() {
    // D2 /4 = SHL AL,CL. The historical form-wide mask 0x07C7 drops OF,
    // but OF is architecturally defined when effective count==1. Deliberately
    // omit hardware OF from the expected state: the count-sensitive runner
    // must expose the mismatch instead of hiding it.
    let mut init = base_init();
    init.ax = 0x0040;
    init.cx = 0x0001;
    let mut fin = init.clone();
    fin.ax = 0x0080;
    fin.ip = 3;
    fin.flags = 0x0082; // actual is 0x0882: only defined OF differs.
    let init_ram = ram(&[(0x00000, 0xD2), (0x00001, 0xE0), (0x00002, 0xF4)]);
    let test = make_test("shl al,cl count=1", &[0xD2, 0xE0, 0xF4], init, fin, init_ram, vec![]);
    let policy_umask = policy_for_file("D2.4").unwrap().flags_umask.unwrap();
    assert_eq!(policy_umask, 0x07C7);
    let opts = RunOpts { flags_umask: policy_umask, ..RunOpts::default() };
    match run_test(&test, &opts) {
      Outcome::Fail { flags_diff: Some(fd), .. } => {
        assert_eq!(fd.umask, 0x0FC7);
        assert_eq!((fd.expected ^ fd.actual) & FLAG_OF, FLAG_OF);
      }
      other => panic!("defined count==1 OF mismatch must fail, got {:?}", other),
    }
  }

  #[test]
  fn shift_count_gt_one_keeps_of_masked() {
    // Same form, CL=2: OF is undefined, so a difference only in OF must pass.
    let mut init = base_init();
    init.ax = 0x0040;
    init.cx = 0x0002;
    let mut fin = init.clone();
    fin.ax = 0x0000;
    fin.ip = 3;
    fin.flags = 0x0847; // emu86 yields 0x0047; OF is undefined for count>1.
    let init_ram = ram(&[(0x00000, 0xD2), (0x00001, 0xE0), (0x00002, 0xF4)]);
    let test = make_test("shl al,cl count=2", &[0xD2, 0xE0, 0xF4], init, fin, init_ram, vec![]);
    let policy_umask = policy_for_file("D2.4").unwrap().flags_umask.unwrap();
    let opts = RunOpts { flags_umask: policy_umask, ..RunOpts::default() };
    assert_eq!(run_test(&test, &opts), Outcome::Pass);
  }

  #[test]
  fn rol_preserved_af_is_compared() {
    // D0 /0 = ROL AL,1. ROL preserves AF; a form-wide shift mask must not hide
    // an AF mismatch merely because AF is undefined for SHL/SHR/SAR.
    let mut init = base_init();
    init.ax = 0x0080;
    init.flags = 0x0012; // AF=1
    let mut fin = init.clone();
    fin.ax = 0x0001;
    fin.ip = 3;
    fin.flags = 0x0803; // deliberately clear preserved AF; actual is 0x0813.
    let init_ram = ram(&[(0x00000, 0xD0), (0x00001, 0xC0), (0x00002, 0xF4)]);
    let test = make_test("rol al,1 preserves af", &[0xD0, 0xC0, 0xF4], init, fin, init_ram, vec![]);
    let policy_umask = policy_for_file("D0.0").unwrap().flags_umask.unwrap();
    assert_eq!(policy_umask, 0x0FC7);
    let opts = RunOpts { flags_umask: policy_umask, ..RunOpts::default() };
    match run_test(&test, &opts) {
      Outcome::Fail { flags_diff: Some(fd), .. } => {
        assert_eq!(fd.umask, DEFAULT_FLAGS_UMASK);
        assert_eq!((fd.expected ^ fd.actual) & FLAG_AF, FLAG_AF);
      }
      other => panic!("ROL preserved-AF mismatch must fail, got {:?}", other),
    }
  }

  #[test]
  fn skip_32bit_state() {
    let init = MooRegisters32Init {
      cr0: 0, cr3: 0, eax: 0, ebx: 0, ecx: 0, edx: 0, esi: 0, edi: 0,
      ebp: 0, esp: 0, cs: 0, ds: 0, es: 0, fs: 0, gs: 0, ss: 0, eip: 0, dr6: 0, dr7: 0, eflags: 0x2,
    };
    let fin = init.clone();
    let init_state = MooTestState::new(MooStateType::Initial, &MooRegistersInit::ThirtyTwo(init.clone()), None, None, vec![], vec![]);
    let fin_state = MooTestState::new(
      MooStateType::Final,
      &MooRegistersInit::ThirtyTwo(init),
      Some(&MooRegistersInit::ThirtyTwo(fin)),
      None,
      vec![],
      vec![],
    );
    let test = MooTest::new("32bit".to_string(), None, &[0x90, 0xF4], init_state, fin_state, &[], None, None);
    assert_eq!(run_test(&test, &RunOpts::default()), Outcome::Skip32BitState);
  }

  #[test]
  fn invalid_vector_out_of_bounds_ram() {
    let init = base_init();
    let mut fin = init.clone();
    fin.ip = 2;
    // Entry at 0x10FFF0 == machine.mem.0.len() -> out of bounds.
    let init_ram = ram(&[(0x10FFF0, 0x00)]);
    let test = make_test("oob", &[0x90, 0xF4], init, fin, init_ram, vec![]);
    match run_test(&test, &RunOpts::default()) {
      Outcome::InvalidVector(_) => {}
      other => panic!("expected InvalidVector, got {:?}", other),
    }
  }

  #[test]
  fn prefix_filter_predicate() {
    assert!(is_prefix_filtered(&[0x2E, 0x04, 0x05, 0xF4]));
    assert!(is_prefix_filtered(&[0x26, 0x90, 0xF4]));
    assert!(is_prefix_filtered(&[0xF3, 0x27, 0xF4]));
    assert!(!is_prefix_filtered(&[0x04, 0x05, 0xF4]));
    assert!(!is_prefix_filtered(&[0x90, 0xF4]));
    // String ops with a prefix are NOT filtered (REP/seg/LOCK + A4-AF is valid).
    assert!(!is_prefix_filtered(&[0xF3, 0xA4, 0xF4])); // rep movsb
    assert!(!is_prefix_filtered(&[0xF2, 0xAE, 0xF4])); // repne scasb
    assert!(!is_prefix_filtered(&[0x3E, 0xA5, 0xF4])); // ds: movsw
    assert!(!is_prefix_filtered(&[0xF0, 0xAA, 0xF4])); // lock stosb
    assert!(!is_prefix_filtered(&[0xA4, 0xF4]));        // bare movsb (no prefix anyway)
    // A non-string op with a prefix is still filtered.
    assert!(is_prefix_filtered(&[0xF3, 0xA3, 0xF4])); // rep mov moffs (not a string op)
  }

  #[test]
  fn run_file_conservative_filter_counts_prefix_tests_as_filtered() {
    // CS: ADD AL,5 (2E 04 05) + HALT. Same arithmetic as add_test: with
    // AL=0 the result is AL=5 and PF (0b101 has two bits -> even parity);
    // 3 instr bytes + 1 HALT -> final ip 4.
    let prefix_init = base_init();
    let mut prefix_fin = prefix_init.clone();
    prefix_fin.ax = 0x0005;
    prefix_fin.flags = 0x0006;
    prefix_fin.ip = 4;
    let prefix_ram = ram(&[(0x00000, 0x2E), (0x00001, 0x04), (0x00002, 0x05), (0x00003, 0xF4)]);
    let prefix_test =
      make_test("cs add", &[0x2E, 0x04, 0x05, 0xF4], prefix_init, prefix_fin, prefix_ram, vec![]);

    let tests = vec![add_test(0x7F, 0x84, 0x0002, 0x0896), prefix_test];
    let summary = run_file(&tests, &RunOpts::default(), 1, true, None, |_, _| {});
    assert_eq!(summary.total, 2);
    assert_eq!(summary.visited, 2);
    assert_eq!(summary.executed, 1);
    assert_eq!(summary.filtered, 1);
    assert_eq!(summary.pass, 1);
    assert_eq!(summary.fail, 0);

    // With the filter disabled, the prefix test is executed (bucketed).
    let summary = run_file(&tests, &RunOpts::default(), 1, false, None, |_, _| {});
    assert_eq!(summary.filtered, 0);
    assert_eq!(summary.executed, 2);
    assert_eq!(summary.pass, 2);
  }

  /// MUL AX (F7 E0) with AX=0x0010: DX:AX = 0x0000:0x0100; CF/OF defined,
  /// PF/SF/ZF/AF undefined per Intel 80286.
  fn mul_test(init_flags: u16, fin_flags: u16) -> MooTest {
    let mut init = base_init();
    init.ax = 0x0010;
    init.flags = init_flags;
    let mut fin = init.clone();
    fin.ax = 0x0100;
    fin.dx = 0;
    fin.ip = 3;
    fin.flags = fin_flags;
    let init_ram = ram(&[(0x00000, 0xF7), (0x00001, 0xE0), (0x00002, 0xF4)]);
    make_test("mul ax", &[0xF7, 0xE0, 0xF4], init, fin, init_ram, vec![])
  }

  /// DIV CX (F7 F1) with DX:AX = 0x0000:0x0010, CX=4 -> AX=4, DX=0; all
  /// status flags undefined per Intel 80286.
  fn div_test(init_flags: u16, fin_flags: u16) -> MooTest {
    let mut init = base_init();
    init.ax = 0x0010;
    init.cx = 0x0004;
    init.flags = init_flags;
    let mut fin = init.clone();
    fin.ax = 0x0004;
    fin.ip = 3;
    fin.flags = fin_flags;
    let init_ram = ram(&[(0x00000, 0xF7), (0x00001, 0xF1), (0x00002, 0xF4)]);
    make_test("div cx", &[0xF7, 0xF1, 0xF4], init, fin, init_ram, vec![])
  }

  #[test]
  fn mul_umask_0801_ignores_undefined_flags() {
    // emu86's MUL leaves flags at 0x0002 (init). Expected differs only in the
    // undefined PF/SF/ZF/AF -> Pass with the policy umask 0x0801.
    let policy_umask = policy_for_file("F7.4").unwrap().flags_umask.unwrap();
    assert_eq!(policy_umask, 0x0801);
    let test = mul_test(0x0002, 0x0002 | 0x00C4);
    let opts = RunOpts { flags_umask: policy_umask, ..RunOpts::default() };
    assert_eq!(run_test(&test, &opts), Outcome::Pass, "undefined PF/SF/ZF/AF must not fail MUL");
  }

  #[test]
  fn mul_umask_0801_still_compares_cf() {
    // A defined CF difference must Fail even under the policy umask.
    let policy_umask = policy_for_file("F7.4").unwrap().flags_umask.unwrap();
    let test = mul_test(0x0002, 0x0002 | 0x0001);
    let opts = RunOpts { flags_umask: policy_umask, ..RunOpts::default() };
    match run_test(&test, &opts) {
      Outcome::Fail { flags_diff, .. } => {
        let fd = flags_diff.expect("flags_diff must be present for CF mismatch");
        assert_eq!(fd.umask, 0x0801);
      }
      other => panic!("expected Fail (CF mismatch), got {:?}", other),
    }
  }

  #[test]
  fn div_umask_0000_ignores_all_flags() {
    // Deliberately wrong expected flags (0xFFFF) still Pass because the policy
    // umask for DIV is 0x0000 (all status flags undefined on 80286).
    let policy_umask = policy_for_file("F7.6").unwrap().flags_umask.unwrap();
    assert_eq!(policy_umask, 0x0000);
    let test = div_test(0x0002, 0xFFFF);
    let opts = RunOpts { flags_umask: policy_umask, ..RunOpts::default() };
    assert_eq!(run_test(&test, &opts), Outcome::Pass, "undefined DIV flags must not fail");
  }

  #[test]
  fn absent_final_ip_short_branch_onto_halt_passes_d012() {
    // SST-D-012 reproduction: a short branch whose target = init IP - 1 (JMP
    // rel8 = -3 onto a 0xF4 HALT byte at 0x00001). The suite's terminating-HALT
    // convention: the reference executed the HALT, so its recorded post-HALT IP
    // wrapped back to the initial IP and is recorded "unchanged/absent"; emu86
    // stops one step before the HALT at init-1. The runner must apply the same
    // -1 to the absent final IP (was: expected = init, off by one -> false FAIL).
    let mut init = base_init();
    init.ip = 2; // JMP rel8 at 0x00002
    let fin = init.clone(); // IP absent: unchanged
    let init_ram = ram(&[
      (0x00001, 0xF4), // HALT at target = init-1
      (0x00002, 0xEB), // JMP rel8
      (0x00003, 0xFD), // rel8 = -3 -> target 0x00001
    ]);
    let test = make_test("jmp short onto halt", &[0xEB, 0xFD, 0xF4], init, fin, init_ram, vec![]);
    let outcome = run_test(&test, &RunOpts::default());
    assert_eq!(outcome, Outcome::Pass, "outcome: {:?}", outcome);
  }

  #[test]
  fn revocation_skips_matching_hashes() {
    use std::collections::HashSet;
    // t0 has no explicit hash ("##NOHASH##"); t1 carries an explicit SHA1.
    let t0 = add_test(0x7F, 0x84, 0x0002, 0x0896);
    let hash_hex: String = std::iter::repeat("11").take(20).collect();
    let mut skip = HashSet::new();
    skip.insert(hash_hex.clone());

    let init = base_init();
    let mut fin = init.clone();
    fin.ip = 2; // NOP (1 byte) + HALT
    let init_ram = ram(&[(0x00000, 0x90), (0x00001, 0xF4)]);
    let init_state =
      MooTestState::new(MooStateType::Initial, &MooRegistersInit::Sixteen(init.clone()), None, None, vec![], init_ram);
    let fin_state = MooTestState::new(
      MooStateType::Final,
      &MooRegistersInit::Sixteen(init),
      Some(&MooRegistersInit::Sixteen(fin)),
      None,
      vec![],
      vec![],
    );
    let t1 = MooTest::new("nop-hashed".to_string(), None, &[0x90, 0xF4], init_state, fin_state, &[], None, Some([0x11; 20]));

    let tests = vec![t0, t1];
    let summary = run_file(&tests, &RunOpts::default(), 1, false, Some(&skip), |_, _| {});
    assert_eq!(summary.revoked, 1, "one test must land in the revoked bucket");
    assert_eq!(summary.executed, 1);
    assert_eq!(summary.pass, 1);
    assert_eq!(summary.visited, 2);

    // Without the revocation set, both run and pass.
    let summary = run_file(&tests, &RunOpts::default(), 1, false, None, |_, _| {});
    assert_eq!(summary.revoked, 0);
    assert_eq!(summary.executed, 2);
    assert_eq!(summary.pass, 2);
  }
}
