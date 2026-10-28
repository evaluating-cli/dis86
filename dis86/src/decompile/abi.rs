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
    for operand in &instr.operands {
      if let Ref::Init(reg) = operand {
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
pub fn caller_reads_dx_after_call(ir: &IR, live: &HashSet<Ref>, call: Ref) -> bool {
  let Ref::Instr(blk, idx) = call else { return false; };
  let mut seen = HashSet::new();
  reads_dx_from(ir, live, blk, Some(idx + 1), &mut seen)
}

fn reads_dx_from(ir: &IR, live: &HashSet<Ref>, blk: BlockRef,
                 after: Option<usize>, seen: &mut HashSet<BlockRef>) -> bool {
  if !seen.insert(blk) { return false; }
  let start = after.unwrap_or(0);
  for (i, r) in ir.iter_instrs(blk).enumerate() {
    if i < start { continue; }
    let Some(instr) = ir.instr(r) else { continue; };
    if writes_dx(ir, instr) { return false; }
    if live.contains(&r) && instr.operands.iter().any(|o| *o == Ref::Init(instr::Reg::DX)) {
      return true;
    }
  }
  for succ in successors(ir, blk) {
    if reads_dx_from(ir, live, succ, None, seen) { return true; }
  }
  false
}

fn writes_dx(ir: &IR, instr: &Instr) -> bool {
  match instr.opcode {
    Opcode::CallArgs | Opcode::CallNear | Opcode::CallFar | Opcode::CallPtr | Opcode::Int => true,
    Opcode::WriteVar16 => {
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
