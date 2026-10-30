//! Calling-convention inference over the optimized SSA IR.
//!
//! Register inputs are found by a backward slice from the instructions that
//! can reach the emitted output, not by scanning every `Ref::Init`. The
//! distinction matters for prologues: `push bx` lowers to a local store of
//! `Init(BX)`, and a naive scan would call `BX` an input even when the
//! function only saves and restores it. Exit-state write-back is excluded for
//! the same reason; ordinary `PIN` writes (including `regargs` setup before a
//! call) are kept as roots so callee inputs propagate into callers.

use std::collections::HashSet;

use crate::asm::instr;
use crate::decompile::ir::{Attribute, BlockRef, Instr, IR, Opcode, Ref};

/// Registers that can carry a function argument in the configurations this
/// analysis supports. Frame/segment state (`SP`, `BP`, `SS`, `CS`) is not an
/// argument; `IP`/`FLAGS` are never pinned as `regargs`.
///
/// `DS`/`ES` are inputs only: this PR has no segment output model, so a
/// callee that changes them still drops the change at `ret` (issue #144).
/// Keep that asymmetry in mind when reading a `regargs` row.
const CANDIDATES: [instr::Reg; 8] = [
  instr::Reg::AX,
  instr::Reg::BX,
  instr::Reg::CX,
  instr::Reg::DX,
  instr::Reg::SI,
  instr::Reg::DI,
  instr::Reg::DS,
  instr::Reg::ES,
];

/// The registers a function reads before writing, limited to the values that
/// are observable through its return, its memory writes, or a call it makes.
///
/// `regargs` pins are roots, so a caller that sets up a callee's declared
/// register inputs inherits those values as its own inputs.
pub fn infer_register_inputs(ir: &IR) -> Vec<instr::Reg> {
  let live = semantic_live(ir);
  let mut found = HashSet::new();
  for r in live {
    let Some(instr) = ir.instr(r) else { continue; };
    let is_ret = matches!(instr.opcode, Opcode::RetNear | Opcode::RetFar);
    for operand in &instr.operands {
      if let Ref::Init(reg) = operand {
        // A `ret`'s AX/DX operands are the return expression, not caller
        // setup. An unannotated function returns DX:AX by default, and
        // counting those entry values would report the return registers as
        // `regargs`. The check is deliberately shallow: a value used by a
        // non-ret instruction (including a `Phi` merge that also feeds the
        // return) is still an input.
        if is_ret && matches!(reg, instr::Reg::AX | instr::Reg::DX) { continue; }
        let reg = storage_parent(*reg);
        if CANDIDATES.contains(&reg) { found.insert(reg); }
      }
    }
  }

  CANDIDATES.iter().copied().filter(|reg| found.contains(reg)).collect()
}

/// Instructions that can reach an observable effect, with exit-state and
/// stack-save roots filtered as in [`infer_register_inputs`].
pub fn semantic_live(ir: &IR) -> HashSet<Ref> {
  let reachable = reachable_blocks(ir);
  let mut roots = Vec::new();
  for blk in ir.iter_blocks() {
    if !reachable.contains(&blk) { continue; }
    for r in ir.iter_instrs(blk) {
      let Some(instr) = ir.instr(r) else { continue; };
      let is_ret = matches!(instr.opcode, Opcode::RetNear | Opcode::RetFar);
      let is_root = is_ret || (
        (instr.opcode.has_side_effects() || (instr.attrs & Attribute::PIN) != 0)
          && instr.opcode != Opcode::Pin
          && (instr.attrs & Attribute::EXIT_STATE) == 0
          && (instr.attrs & Attribute::STACK_SAVE) == 0
      );
      if is_root { roots.push(r); }
    }
  }

  let mut live = closure(ir, roots);

  // A push slot that is actually read (an outgoing stack argument, or a `pop`
  // whose value is used) is an observable value even though the `push` store
  // itself is not a root. Add back only those stack saves whose slot has a
  // live read; a save/restore pair whose restored value only feeds the exit
  // write-back must not report the saved register as an input.
  let reread = stack_saves_read_by_live(ir, &reachable, &live);
  if !reread.is_empty() {
    live.extend(closure(ir, reread));
  }
  live
}

/// Whether the reachable part of the function contains a return.
pub fn has_reachable_ret(ir: &IR) -> bool {
  let reachable = reachable_blocks(ir);
  ir.iter_blocks().any(|blk| {
    reachable.contains(&blk) && ir.iter_instrs(blk).any(|r| {
      matches!(ir.instr(r).map(|i| i.opcode), Some(Opcode::RetNear | Opcode::RetFar))
    })
  })
}

/// Whether `call` is followed by a live read of `DX` before any write to it
/// or another call. This is the caller-observed half of return inference: a
/// 16-bit return leaves `DX` unknown, so only a caller that actually consumes
/// the high half justifies `u32`.
///
/// A `U32`/`Unknown` return is represented in the caller as
/// `DX = Upper16(call)`, so a read often uses that derived ref rather than
/// `Init(DX)`. Both forms count.
pub fn caller_reads_dx_after_call(ir: &IR, live: &HashSet<Ref>, call: Ref) -> bool {
  // A call in an unreachable block is not a live root, but its fallthrough can
  // join a block that is reachable another way; do not attribute that block's
  // DX read to this dead call.
  if !live.contains(&call) { return false; }
  let Ref::Instr(blk, idx) = call else { return false; };
  let dx_result = dx_result_of_call(ir, call);
  let mut seen = HashSet::new();
  reads_dx_from(ir, live, blk, Some(idx + 1), dx_result, &mut seen)
}

/// The high half of a `U32`/`Unknown` call result, created by
/// `save_return_value` as `Upper16(call)`.
fn dx_result_of_call(ir: &IR, call: Ref) -> Option<Ref> {
  for blk in ir.iter_blocks() {
    for r in ir.iter_instrs(blk) {
      let Some(instr) = ir.instr(r) else { continue; };
      if instr.opcode == Opcode::Upper16 && instr.operands.first() == Some(&call) {
        return Some(r);
      }
    }
  }
  None
}

fn reads_dx_from(ir: &IR, live: &HashSet<Ref>, blk: BlockRef,
                 after: Option<usize>, dx_result: Option<Ref>,
                 seen: &mut HashSet<BlockRef>) -> bool {
  if !seen.insert(blk) { return false; }
  let start = after.unwrap_or(0);
  for (i, r) in ir.iter_instrs(blk).enumerate() {
    if i < start { continue; }
    let Some(instr) = ir.instr(r) else { continue; };
    if writes_dx(ir, instr) { return false; }
    if live.contains(&r) {
      if instr.operands.iter().any(|o| derives_from_reg(ir, *o, instr::Reg::DX)) {
        return true;
      }
      if let Some(result) = dx_result {
        if instr.operands.iter().any(|o| derives_from(ir, *o, result)) {
          return true;
        }
      }
    }
  }
  for succ in successors(ir, blk) {
    if reads_dx_from(ir, live, succ, None, dx_result, seen) { return true; }
  }
  false
}

/// Whether `value` is `target` or a pure projection of it (the byte splits
/// and copies `save_return_value` emits around a call result).
fn derives_from(ir: &IR, value: Ref, target: Ref) -> bool {
  let mut seen = HashSet::new();
  derives_from_inner(ir, value, target, &mut seen)
}

fn derives_from_inner(ir: &IR, value: Ref, target: Ref,
                      seen: &mut HashSet<Ref>) -> bool {
  if value == target { return true; }
  if !seen.insert(value) { return false; }
  let Some(instr) = ir.instr(value) else { return false; };
  if !is_projection(instr.opcode) { return false; }
  instr.operands.iter().any(|o| derives_from_inner(ir, *o, target, seen))
}

/// Whether `value` is a register or a pure projection of its byte fragments.
/// `save_return_value` re-splits `DX` into `DL`/`DH` after a call, so a byte
/// read is still a read of the parent register.
fn derives_from_reg(ir: &IR, value: Ref, reg: instr::Reg) -> bool {
  let mut seen = HashSet::new();
  derives_from_reg_inner(ir, value, reg, &mut seen)
}

fn derives_from_reg_inner(ir: &IR, value: Ref, reg: instr::Reg,
                          seen: &mut HashSet<Ref>) -> bool {
  match value {
    Ref::Init(r) => storage_parent(r) == reg,
    Ref::Instr(_, _) => {
      if !seen.insert(value) { return false; }
      let Some(instr) = ir.instr(value) else { return false; };
      if !is_projection(instr.opcode) { return false; }
      instr.operands.iter().any(|o| derives_from_reg_inner(ir, *o, reg, seen))
    }
    _ => false,
  }
}

fn is_projection(opcode: Opcode) -> bool {
  matches!(opcode,
    Opcode::Ref | Opcode::Upper16 | Opcode::Lower16
      | Opcode::And | Opcode::Or | Opcode::UShr | Opcode::Shl)
}

fn writes_dx(ir: &IR, instr: &Instr) -> bool {
  match instr.opcode {
    Opcode::CallArgs | Opcode::CallNear | Opcode::CallFar
      | Opcode::CallNearPtr | Opcode::CallFarPtr | Opcode::Int => true,
    Opcode::WriteVar16 => {
      // Exit-state write-backs preserve the callee's DX value for its caller;
      // they do not clobber the value this function is still using.
      if (instr.attrs & Attribute::EXIT_STATE) != 0 { return false; }
      match instr.operands.first() {
        Some(Ref::Symbol(sym)) => {
          sym.table() == crate::decompile::sym::Table::Register
            && sym.name(&ir.symbols) == instr::Reg::DX.info().name
        }
        _ => false,
      }
    }
    _ => false,
  }
}

fn successors(ir: &IR, blk: BlockRef) -> Vec<BlockRef> {
  let mut out = ir.block_exits(blk);
  for b in ir.iter_blocks() {
    if ir.block(b).preds.contains(&blk) && !out.contains(&b) { out.push(b); }
  }
  out
}

fn closure(ir: &IR, mut roots: Vec<Ref>) -> HashSet<Ref> {
  let mut live = HashSet::new();
  while let Some(r) = roots.pop() {
    if !live.insert(r) { continue; }
    let Some(instr) = ir.instr(r) else { continue; };
    for operand in &instr.operands {
      if matches!(operand, Ref::Instr(_, _)) { roots.push(*operand); }
    }
  }
  live
}

fn stack_saves_read_by_live(ir: &IR, reachable: &HashSet<BlockRef>,
                            live: &HashSet<Ref>) -> Vec<Ref> {
  let mut symbols = HashSet::new();
  let mut memory = HashSet::new();
  for &r in live {
    let Some(instr) = ir.instr(r) else { continue; };
    match instr.opcode {
      Opcode::ReadVar16 => {
        if let Some(Ref::Symbol(sym)) = instr.operands.first() { symbols.insert(*sym); }
      }
      Opcode::Load8 | Opcode::Load16 | Opcode::Load32 => {
        if let (Some(seg), Some(addr)) = (instr.operands.first(), instr.operands.get(1)) {
          memory.insert((*seg, *addr));
        }
      }
      _ => {}
    }
  }

  let mut extra = vec![];
  for blk in ir.iter_blocks() {
    if !reachable.contains(&blk) { continue; }
    for r in ir.iter_instrs(blk) {
      if live.contains(&r) { continue; }
      let Some(instr) = ir.instr(r) else { continue; };
      if (instr.attrs & Attribute::STACK_SAVE) == 0 { continue; }
      let reread = match instr.opcode {
        Opcode::WriteVar16 => {
          matches!(instr.operands.first(), Some(Ref::Symbol(sym)) if symbols.contains(sym))
        }
        Opcode::Store8 | Opcode::Store16 | Opcode::Store32 => {
          match (instr.operands.first(), instr.operands.get(1)) {
            (Some(seg), Some(addr)) => memory.contains(&(*seg, *addr)),
            _ => false,
          }
        }
        _ => false,
      };
      if reread { extra.push(r); }
    }
  }
  extra
}

fn reachable_blocks(ir: &IR) -> HashSet<BlockRef> {
  let mut reachable = HashSet::new();
  let mut stack = vec![BlockRef(0)];
  while let Some(blk) = stack.pop() {
    if !reachable.insert(blk) { continue; }
    stack.extend(ir.block_exits(blk));
  }
  reachable
}

/// Byte fragments alias their parent's storage; a partial read is still an
/// input to the full register in the emitted C.
fn storage_parent(reg: instr::Reg) -> instr::Reg {
  match reg {
    instr::Reg::AL | instr::Reg::AH => instr::Reg::AX,
    instr::Reg::BL | instr::Reg::BH => instr::Reg::BX,
    instr::Reg::CL | instr::Reg::CH => instr::Reg::CX,
    instr::Reg::DL | instr::Reg::DH => instr::Reg::DX,
    _ => reg,
  }
}
