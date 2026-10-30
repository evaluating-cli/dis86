use std::collections::{HashMap, HashSet};

use crate::asm::instr::{self, Instr as RawInstr, Operand, Reg};
use crate::asm::instr::Opcode::*;
use crate::binary::Binary;
use crate::config::{Config, FuncRegion};
use crate::decompile::ir::{Opcode as IrOpcode, Ref, IR};
use crate::decompile::{abi, ir_build, opt, sym};
use crate::segoff::{Off, Seg, SegOff};
use crate::spec::Spec;
use crate::types::Type;

struct BuiltFunc {
  name: String,
  start: SegOff,
  end: SegOff,
  annotated_ret: Option<Type>,
  ir: IR,
  raw: Vec<RawInstr>,
}

struct SpecSeed {
  name: String,
  func_name: Option<String>,
  start: SegOff,
  end: SegOff,
  regions: Vec<FuncRegion>,
}

/// A selected function that could not be analyzed. Structural range failures
/// and decode failures both become rows so one bad entry does not hide the
/// rest of the report.
pub struct FailedFunc {
  pub name: String,
  pub start: SegOff,
  pub end: SegOff,
  pub note: String,
}

impl FailedFunc {
  pub fn new(name: String, start: SegOff, end: SegOff, note: String) -> Self {
    FailedFunc { name, start, end, note }
  }
}

const MAX_FIXPOINT_ITERS: usize = 8;

pub fn run(cfg: &Config, binary: &Binary, specs: Vec<Spec<'_>>,
           pre_failed: Vec<FailedFunc>) -> i32 {
  let seeds: Vec<SpecSeed> = specs.into_iter().map(|spec| SpecSeed {
    name: spec.name,
    func_name: spec.func.map(|f| f.name.clone()),
    start: spec.start,
    end: spec.end,
    regions: spec.regions,
  }).collect();

  // Regargs propagate caller-ward: a caller pins a callee's declared register
  // inputs, so rebuilding with the inferred declarations reaches a fixpoint.
  let mut working = cfg.clone();
  let mut built = vec![];
  let mut failed = vec![];
  let mut converged = false;
  for _ in 0..MAX_FIXPOINT_ITERS {
    let specs: Vec<Spec<'_>> = seeds.iter().map(|seed| match &seed.func_name {
      Some(name) => Spec::from_config_name(&working, name).unwrap_or_else(|| Spec {
        func: None,
        name: seed.name.clone(),
        start: seed.start,
        end: seed.end,
        regions: seed.regions.clone(),
      }),
      None => Spec {
        func: None,
        name: seed.name.clone(),
        start: seed.start,
        end: seed.end,
        regions: seed.regions.clone(),
      },
    }).collect();
    (built, failed) = build_all(&working, binary, specs);

    let mut changed = false;
    for f in &working.funcs {
      let Some(bf) = built.iter().find(|bf| bf.name == f.name) else { continue; };
      let regs = abi::infer_register_inputs(&bf.ir);
      let new = if regs.is_empty() { None } else { Some(regs) };
      if f.regargs != new {
        changed = true;
        break;
      }
    }
    if !changed {
      converged = true;
      break;
    }
    for bf in &built {
      let regs = abi::infer_register_inputs(&bf.ir);
      let new = if regs.is_empty() { None } else { Some(regs) };
      if let Some(f) = working.funcs.iter_mut().find(|f| f.name == bf.name) {
        f.regargs = new;
      }
    }
  }
  if !converged {
    eprintln!("Warning: ABI inference did not converge after {} iterations; the report reflects the last pass.",
              MAX_FIXPOINT_ITERS);
  }

  let mut dx_reads = HashSet::new();
  for caller in &built {
    ir_dx_reads(caller, &mut dx_reads);
  }

  let args = infer_args(cfg, &built);
  println!("name\tstart\tend\targs\tdont_pop_args\tret\tregargs\tnote");
  for f in built {
    let reachable_ret = abi::has_reachable_ret(&f.ir);
    let dx = dx_reads.contains(&f.name);
    // An explicit annotation outranks reachability: the report must not
    // propose replacing a deliberate `ret u16` with `void` just because the
    // configured range ends before the ret. Reachability only qualifies the
    // annotation in that case.
    let ret = match &f.annotated_ret {
      Some(Type::Void) => "void".to_string(),
      Some(t) if !reachable_ret =>
        format!("{} (annotated; ret outside the reachable range)", t),
      Some(Type::U32) => {
        if dx { "u32".to_string() } else { "u32 (annotated; no caller DX read observed)".to_string() }
      }
      Some(t) => if dx { "u32".to_string() } else { format!("{}", t) },
      None if !reachable_ret => "void".to_string(),
      None => if dx { "u32".to_string() } else { "u16".to_string() },
    };
    let regs: Vec<&str> = abi::infer_register_inputs(&f.ir)
      .iter().map(|reg| reg.info().name).collect();
    let regs = if regs.is_empty() { "-".to_string() } else { regs.join(",") };
    let a = args.get(&f.name);
    let nargs = a.and_then(|a| a.args).map(|n| n.to_string()).unwrap_or_else(|| "?".to_string());
    let dont_pop = a.map(|a| a.dont_pop_args).unwrap_or(false);
    let note = a.map(|a| a.note.as_str()).unwrap_or("no call-site evidence");
    println!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
             f.name, f.start, f.end, nargs, dont_pop, ret, regs, note);
  }
  for f in failed.iter().chain(pre_failed.iter()) {
    println!("{}\t{}\t{}\t?\t?\t?\t-\t{}", f.name, f.start, f.end, f.note);
  }
  if failed.is_empty() && pre_failed.is_empty() { 0 } else { 1 }
}

struct ArgsInfo {
  args: Option<u16>,
  dont_pop_args: bool,
  note: String,
}

fn infer_args(cfg: &Config, built: &[BuiltFunc]) -> std::collections::HashMap<String, ArgsInfo> {
  let mut cleanups: std::collections::HashMap<String, Vec<u16>> = std::collections::HashMap::new();
  for caller in built {
    let reachable = reachable_raw(&caller.raw);
    for (i, ins) in caller.raw.iter().enumerate() {
      if !reachable[i] { continue; }
      if !matches!(ins.opcode, OP_CALL | OP_CALLF) { continue; }
      let Some(target) = direct_call_target(ins) else { continue; };
      let Some(callee) = cfg.func_lookup(target) else { continue; };
      let Some(bytes) = cleanup_bytes(&caller.raw, i) else { continue; };
      if bytes % 2 != 0 { continue; }
      // A post-call `add sp, imm` needs pushed argument slots to corroborate
      // it. Without them the adjustment is more likely frame teardown, which
      // would otherwise turn a no-arg callee into a false `args` suggestion.
      let pushed = pushed_bytes_before(&caller.raw, i);
      if pushed == 0 || bytes > pushed { continue; }
      cleanups.entry(callee.name.clone()).or_default().push(bytes / 2);
    }
  }

  let mut out = std::collections::HashMap::new();
  for f in built {
    if let Some(nargs) = ret_imm_args(&f.raw) {
      out.insert(f.name.clone(), ArgsInfo {
        args: Some(nargs),
        dont_pop_args: true,
        note: format!("ret {} (callee pops)", nargs * 2),
      });
      continue;
    }
    if let Some(values) = cleanups.get(&f.name) {
      let mut uniq = values.clone();
      uniq.sort(); uniq.dedup();
      if uniq.len() == 1 {
        out.insert(f.name.clone(), ArgsInfo {
          args: Some(uniq[0]),
          dont_pop_args: false,
          note: format!("caller cleans {} byte(s)", uniq[0] * 2),
        });
      } else {
        out.insert(f.name.clone(), ArgsInfo {
          args: None,
          dont_pop_args: false,
          note: format!("variadic/cleanup varies: {:?}", uniq),
        });
      }
    }
  }
  out
}

fn ret_imm_args(raw: &[RawInstr]) -> Option<u16> {
  let reachable = reachable_raw(raw);
  let mut values = std::collections::BTreeSet::new();
  let mut saw_ret = false;
  for (i, ins) in raw.iter().enumerate() {
    if !reachable[i] { continue; }
    if !matches!(ins.opcode, OP_RET | OP_RETF) { continue; }
    saw_ret = true;
    let mut imm = None;
    for op in &ins.operands {
      if let Operand::Imm(i) = op {
        if i.val % 2 == 0 { imm = Some(i.val / 2); }
      }
    }
    // A reachable plain `ret` means the callee does not uniformly pop
    // arguments, so a `ret imm` elsewhere is not evidence for the whole
    // function.
    match imm {
      Some(n) => { values.insert(n); }
      None => return None,
    }
  }
  if saw_ret && values.len() == 1 { values.into_iter().next() } else { None }
}

/// Raw-instruction reachability, used to ignore bytes decoded after a
/// terminator (padding, inline data). Follows fallthrough and direct
/// `Rel`/`Far` targets.
fn reachable_raw(raw: &[RawInstr]) -> Vec<bool> {
  let mut reachable = vec![false; raw.len()];
  if raw.is_empty() { return reachable; }
  let index: HashMap<SegOff, usize> =
    raw.iter().enumerate().map(|(i, ins)| (ins.addr, i)).collect();
  let mut stack = vec![0usize];
  while let Some(i) = stack.pop() {
    if i >= raw.len() || reachable[i] { continue; }
    reachable[i] = true;
    let ins = &raw[i];
    let fallthrough = !matches!(ins.opcode,
      OP_RET | OP_RETF | OP_HLT | OP_IRET | OP_JMP | OP_JMPF);
    if follows_target(ins.opcode) {
      for op in &ins.operands {
        let target = match op {
          Operand::Rel(rel) => Some(ins.rel_addr(rel)),
          Operand::Far(far) => Some(SegOff { seg: Seg::Normal(far.seg), off: Off(far.off) }),
          _ => None,
        };
        if let Some(target) = target {
          if let Some(&j) = index.get(&target) { stack.push(j); }
        }
      }
    }
    if fallthrough {
      if let Some(&j) = index.get(&ins.end_addr()) { stack.push(j); }
    }
  }
  reachable
}

/// Instructions whose `Rel`/`Far` operand is an intra-function control-flow
/// edge. Calls also carry a target, but it is not part of this function.
fn follows_target(opcode: instr::Opcode) -> bool {
  matches!(opcode,
    OP_JMP | OP_JMPF
      | OP_JA | OP_JAE | OP_JB | OP_JBE | OP_JCXZ | OP_JE | OP_JG | OP_JGE
      | OP_JL | OP_JLE | OP_JNE | OP_JNO | OP_JNP | OP_JNS | OP_JO | OP_JP
      | OP_JS | OP_LOOP | OP_LOOPE | OP_LOOPNE)
}

/// Caller stack cleanup immediately after a call. Only the `add sp, imm`
/// idiom is measured, and only within a short window that contains no other
/// stack write or control transfer. Cleanup done by `pop reg`, behind a
/// branch, or through a frame teardown yields no evidence rather than a guess.
fn cleanup_bytes(raw: &[RawInstr], call: usize) -> Option<u16> {
  let mut expected = raw.get(call)?.end_addr();
  for ins in raw.iter().skip(call + 1).take(CLEANUP_LOOKAHEAD) {
    // Regions are decoded separately, so the next vector element may belong
    // to another region or the shared-epilogue chunk. Only a contiguous
    // post-call instruction belongs to this call's cleanup window.
    if ins.addr != expected { return None; }
    if ins.opcode == OP_ADD && first_is_sp(ins) {
      return match ins.operands.get(1) {
        Some(Operand::Imm(imm)) => Some(imm.val),
        _ => None,
      };
    }
    if writes_sp(ins) || is_control_transfer(ins.opcode) { return None; }
    expected = ins.end_addr();
  }
  None
}

const CLEANUP_LOOKAHEAD: usize = 4;

fn first_is_sp(ins: &RawInstr) -> bool {
  matches!(ins.operands.get(0), Some(Operand::Reg(instr::OperandReg(Reg::SP))))
}

/// Contiguous `push` slots immediately before a call. Used to corroborate a
/// post-call `add sp, imm`; mirrors the IR builder's call-argument heuristic.
fn pushed_bytes_before(raw: &[RawInstr], call: usize) -> u16 {
  let mut bytes = 0u16;
  let Some(mut expected) = raw.get(call).map(|ins| ins.addr) else { return 0; };
  let mut j = call;
  while j > 0 {
    j -= 1;
    // Do not walk backward across a region boundary: the previous vector
    // element may be the end of another region, not this call's setup.
    if raw[j].end_addr() != expected || raw[j].opcode != OP_PUSH { break; }
    bytes += 2;
    expected = raw[j].addr;
  }
  bytes
}

fn is_control_transfer(opcode: instr::Opcode) -> bool {
  matches!(opcode,
    OP_RET | OP_RETF | OP_IRET | OP_JMP | OP_JMPF | OP_HLT
      | OP_JA | OP_JAE | OP_JB | OP_JBE | OP_JCXZ | OP_JE | OP_JG | OP_JGE
      | OP_JL | OP_JLE | OP_JNE | OP_JNO | OP_JNP | OP_JNS | OP_JO | OP_JP
      | OP_JS | OP_LOOP | OP_LOOPE | OP_LOOPNE)
}

/// Whether an instruction can change `SP`. The scan stops at these so an
/// unrelated later `add sp, imm` is never attributed to the call.
fn writes_sp(ins: &RawInstr) -> bool {
  if matches!(ins.opcode,
    OP_PUSH | OP_POP | OP_PUSHA | OP_POPA | OP_PUSHF | OP_POPF
      | OP_ENTER | OP_LEAVE | OP_CALL | OP_CALLF | OP_INT | OP_INTO
      | OP_IRET | OP_RET | OP_RETF) {
    return true;
  }
  let dst_is_sp = first_is_sp(ins);
  if dst_is_sp && matches!(ins.opcode,
    OP_MOV | OP_ADD | OP_ADC | OP_SUB | OP_SBB | OP_AND | OP_OR | OP_XOR
      | OP_INC | OP_DEC | OP_NEG | OP_NOT | OP_SHL | OP_SHR | OP_SAR
      | OP_ROL | OP_ROR | OP_RCL | OP_RCR | OP_LEA | OP_LDS | OP_LES) {
    return true;
  }
  ins.opcode == OP_XCHG
    && ins.operands.as_slice().iter().any(|op|
      matches!(op, Operand::Reg(instr::OperandReg(Reg::SP))))
}

fn build_all(cfg: &Config, binary: &Binary, specs: Vec<Spec<'_>>)
  -> (Vec<BuiltFunc>, Vec<FailedFunc>) {
  let mut built = vec![];
  let mut failed = vec![];
  for spec in specs {
    match build(cfg, binary, &spec) {
      Ok((ir, raw)) => built.push(BuiltFunc {
        name: spec.name,
        start: spec.start,
        end: spec.end,
        annotated_ret: spec.func.and_then(|f| f.ret.clone()),
        ir,
        raw,
      }),
      Err(err) => failed.push(FailedFunc {
        name: spec.name,
        start: spec.start,
        end: spec.end,
        note: err,
      }),
    }
  }
  (built, failed)
}

fn build(cfg: &Config, binary: &Binary, spec: &Spec<'_>) -> Result<(IR, Vec<RawInstr>), String> {
  build_ir_for_analysis(cfg, binary, spec, false)
}

/// Build the same IR the decompiler emits for `spec`, so inference and
/// codegen cannot silently diverge. Public so the ABI tests exercise this
/// pipeline instead of copying it.
pub fn build_ir_for_analysis(cfg: &Config, binary: &Binary, spec: &Spec<'_>, pin_all: bool)
  -> Result<(IR, Vec<RawInstr>), String> {
  for region in &spec.regions {
    if !binary.contains_region(region.start, region.end) {
      return Err(format!(
        "invalid region {}..{} (outside the loaded image)", region.start, region.end));
    }
  }
  let (instrs, _raw) = crate::spec::decode_regions(binary, spec)?;
  build_ir_from_instrs(cfg, binary, spec, instrs, pin_all)
}

fn build_ir_from_instrs(cfg: &Config, binary: &Binary, spec: &Spec<'_>,
                        instrs: Vec<RawInstr>, pin_all: bool)
  -> Result<(IR, Vec<RawInstr>), String> {
  let overlay = spec.start.is_overlay_addr();
  let mut ir = ir_build::build_from_instrs(&instrs, cfg, spec, binary, overlay, pin_all);
  opt::optimize(&mut ir);
  sym::symbolize(&mut ir, cfg);
  opt::forward_store_to_load(&mut ir);
  opt::optimize(&mut ir);
  opt::mem_symbol_to_ref(&mut ir);
  opt::optimize(&mut ir);
  crate::decompile::finalize_for_emission(&mut ir);
  Ok((ir, instrs))
}

/// Collect callees whose callers consume the high half of the return value.
/// Uses the same CFG-aware, liveness-checked analysis the tests exercise, so
/// a dead `mov bx,dx` cannot mark a return as `u32` and a read after a branch
/// still counts.
fn ir_dx_reads(caller: &BuiltFunc, out: &mut HashSet<String>) {
  let live = abi::semantic_live(&caller.ir);
  for blk in caller.ir.iter_blocks() {
    for r in caller.ir.iter_instrs(blk) {
      let Some(instr) = caller.ir.instr(r) else { continue; };
      if instr.opcode != IrOpcode::CallArgs { continue; }
      let Some(Ref::Func(idx)) = instr.operands.first().copied() else { continue; };
      let Some(name) = caller.ir.funcs.get(idx) else { continue; };
      if abi::caller_reads_dx_after_call(&caller.ir, &live, r) {
        out.insert(name.clone());
      }
    }
  }
}

fn direct_call_target(ins: &RawInstr) -> Option<SegOff> {
  for op in &ins.operands {
    match op {
      Operand::Rel(rel) => return Some(ins.rel_addr(rel)),
      Operand::Far(far) => return Some(SegOff { seg: Seg::Normal(far.seg), off: Off(far.off) }),
      _ => {}
    }
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::asm::decode::Decoder;

  fn decode(code: &[u8]) -> Vec<RawInstr> {
    let binary = crate::binary::Binary::from_raw(code, None);
    let mut decoder = Decoder::new(binary.region_iter(
      crate::segoff::SegOff { seg: crate::segoff::Seg::Normal(0), off: crate::segoff::Off(0) },
      crate::segoff::SegOff { seg: crate::segoff::Seg::Normal(0), off: crate::segoff::Off(code.len() as u16) }));
    let mut out = vec![];
    while let Some((ins, _raw)) = decoder.try_next().expect("decode") { out.push(ins); }
    out
  }

  #[test]
  fn ret_imm_is_callee_cleaned_args() {
    assert_eq!(ret_imm_args(&decode(&[0xC2, 0x04, 0x00])), Some(2));
    assert_eq!(ret_imm_args(&decode(&[0xC3])), None);
  }

  #[test]
  fn reachable_mixed_ret_immediates_are_not_trusted() {
    // jz over the `ret 4` to a plain `ret`: both paths are reachable.
    let code = [0x74, 0x03, 0xC2, 0x04, 0x00, 0xC3];
    assert_eq!(ret_imm_args(&decode(&code)), None);
  }

  #[test]
  fn ret_imm_after_a_terminator_is_still_trusted() {
    // Decoding continues past `ret`; the trailing bytes are unreachable.
    let code = [0xC2, 0x04, 0x00, 0xC2, 0x06, 0x00];
    assert_eq!(ret_imm_args(&decode(&code)), Some(2));
  }

  #[test]
  fn caller_stack_cleanup_is_measured() {
    let raw = decode(&[0xE8, 0x00, 0x00, 0x83, 0xC4, 0x04]);
    assert_eq!(cleanup_bytes(&raw, 0), Some(4));
  }

  #[test]
  fn cleanup_scan_stops_at_stack_writes() {
    // call; mov sp,bp; add sp,4
    let raw = decode(&[0xE8, 0x00, 0x00, 0x89, 0xEC, 0x83, 0xC4, 0x04]);
    assert_eq!(cleanup_bytes(&raw, 0), None);
    // call; leave; add sp,4
    let raw = decode(&[0xE8, 0x00, 0x00, 0xC9, 0x83, 0xC4, 0x04]);
    assert_eq!(cleanup_bytes(&raw, 0), None);
    // call; add sp,ax; add sp,4
    let raw = decode(&[0xE8, 0x00, 0x00, 0x01, 0xC4, 0x83, 0xC4, 0x04]);
    assert_eq!(cleanup_bytes(&raw, 0), None);
    // call; pusha; add sp,4
    let raw = decode(&[0xE8, 0x00, 0x00, 0x60, 0x83, 0xC4, 0x04]);
    assert_eq!(cleanup_bytes(&raw, 0), None);
  }

  #[test]
  fn cleanup_scan_is_bounded() {
    let raw = decode(&[0xE8, 0x00, 0x00, 0x90, 0x90, 0x90, 0x90, 0x83, 0xC4, 0x04]);
    assert_eq!(cleanup_bytes(&raw, 0), None);
    let raw = decode(&[0xE8, 0x00, 0x00, 0x90, 0x90, 0x90, 0x83, 0xC4, 0x04]);
    assert_eq!(cleanup_bytes(&raw, 0), Some(4));
  }

  #[test]
  fn contiguous_pushes_before_call_are_counted() {
    let raw = decode(&[0x50, 0x53, 0xE8, 0x00, 0x00]);
    assert_eq!(pushed_bytes_before(&raw, 2), 4);
    let raw = decode(&[0x50, 0x90, 0xE8, 0x00, 0x00]);
    assert_eq!(pushed_bytes_before(&raw, 2), 0);
  }

  #[test]
  fn reachability_does_not_fall_through_a_region_gap() {
    let mut raw = decode(&[0xE8, 0x00, 0x00, 0xC2, 0x04, 0x00]);
    raw[1].addr = SegOff { seg: Seg::Normal(0), off: Off(8) };
    assert_eq!(ret_imm_args(&raw), None,
      "the ret in a later region must not become the call's fallthrough");
  }

  #[test]
  fn cleanup_scan_does_not_cross_a_region_gap() {
    let mut raw = decode(&[0xE8, 0x00, 0x00, 0x83, 0xC4, 0x04]);
    raw[1].addr = SegOff { seg: Seg::Normal(0), off: Off(8) };
    assert_eq!(cleanup_bytes(&raw, 0), None,
      "an add sp in a later region is not this call's cleanup");
  }

  #[test]
  fn push_scan_does_not_cross_a_region_gap() {
    let mut raw = decode(&[0x50, 0x53, 0xE8, 0x00, 0x00]);
    raw[0].addr = SegOff { seg: Seg::Normal(0), off: Off(0x100) };
    assert_eq!(pushed_bytes_before(&raw, 2), 2,
      "only the contiguous push below the call may count");
  }

  #[test]
  fn trailing_plain_ret_does_not_invalidate_ret_imm() {
    let code = [0xC2, 0x04, 0x00, 0xC3];
    assert_eq!(ret_imm_args(&decode(&code)), Some(2));
  }

  #[test]
  fn far_call_target_is_resolved() {
    let raw = decode(&[0x9A, 0x34, 0x12, 0x00, 0x00]);
    assert_eq!(direct_call_target(&raw[0]),
               Some(SegOff { seg: Seg::Normal(0), off: Off(0x1234) }));
  }
}
