//! Shared branch classification.
//!
//! The decompiler (`decompile/ir_build`) and the analyzer
//! (`analyze/instr_details`) both need the same three answers: which
//! opcodes are branches, where a branch's relative target lives, and
//! whether control also falls through to the next instruction. They used
//! to keep private copies of that table. The copies drifted (the analyzer
//! lost `LOOPE`/`LOOPNE`, so `--analyze` did not treat `loopne` as a
//! branch), and the decompiler's copy panicked on every branch whose
//! operand was not a relative offset, which lost the whole function.
//!
//! This module is the single copy of the table, and it never panics: a
//! branch the caller cannot resolve to an address is reported as
//! [`BranchTarget::Unresolved`].

use crate::asm::instr::{Instr, Opcode, Operand};
use crate::segoff::SegOff;

/// The shape of a branch instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchShape {
  /// Index of the operand holding the relative target.
  pub oper_num: usize,
  /// Whether the branch also falls through to the next instruction.
  pub fallthrough: bool,
}

/// The branch shape of `op`, or `None` when `op` is not a branch.
///
/// `LOOP`/`LOOPE`/`LOOPNE`/`JCXZ` carry a hidden `CX` operand, so their
/// relative target is operand 1, not operand 0.
pub fn branch_shape(op: Opcode) -> Option<BranchShape> {
  let (oper_num, fallthrough) = match op {
    Opcode::OP_JA   => (0, true),
    Opcode::OP_JAE  => (0, true),
    Opcode::OP_JB   => (0, true),
    Opcode::OP_JBE  => (0, true),
    Opcode::OP_JCXZ => (1, true),
    Opcode::OP_JE   => (0, true),
    Opcode::OP_JG   => (0, true),
    Opcode::OP_JGE  => (0, true),
    Opcode::OP_JL   => (0, true),
    Opcode::OP_JLE  => (0, true),
    Opcode::OP_JMP  => (0, false),
    Opcode::OP_JMPF => (0, false),
    Opcode::OP_JNE  => (0, true),
    Opcode::OP_JNO  => (0, true),
    Opcode::OP_JNP  => (0, true),
    Opcode::OP_JNS  => (0, true),
    Opcode::OP_JO   => (0, true),
    Opcode::OP_JP   => (0, true),
    Opcode::OP_JS   => (0, true),
    Opcode::OP_LOOP => (1, true),
    Opcode::OP_LOOPE => (1, true),
    Opcode::OP_LOOPNE => (1, true),
    _ => return None,
  };
  Some(BranchShape { oper_num, fallthrough })
}

/// A transfer whose destination is not a plain relative offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedTransfer {
  /// `jmp far seg:off` — an explicit far target.
  FarDirect { seg: u16, off: u16 },
  /// `jmp far [mem]` — a far target read from memory at run time.
  FarIndirect,
  /// `jmp [mem]` / `jmp reg` — a near target that is only known at run
  /// time, with no jump table the caller can resolve.
  NearIndirect,
}

/// The destination of a branch instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchTarget {
  /// A relative branch: `target` is the branch destination and
  /// `fallthrough` the implicit next instruction (conditional branches
  /// only).
  Relative {
    target: SegOff,
    fallthrough: Option<SegOff>,
  },
  /// A transfer the caller cannot resolve to a single address.
  Unresolved(UnresolvedTransfer),
}

/// The destination of `ins`, or `None` when `ins` is not a branch.
///
/// Never panics: a branch whose target operand is not a relative offset
/// is reported as [`BranchTarget::Unresolved`] instead.
pub fn branch_target(ins: &Instr) -> Option<BranchTarget> {
  let shape = branch_shape(ins.opcode)?;
  match ins.operands.get(shape.oper_num) {
    Some(Operand::Rel(rel)) => Some(BranchTarget::Relative {
      target: ins.rel_addr(rel),
      fallthrough: shape.fallthrough.then(|| ins.end_addr()),
    }),
    _ => Some(BranchTarget::Unresolved(unresolved_transfer(ins))),
  }
}

/// Classify a branch whose target is not a relative offset.
pub fn unresolved_transfer(ins: &Instr) -> UnresolvedTransfer {
  match ins.opcode {
    Opcode::OP_JMPF => match ins.operands.get(0) {
      Some(Operand::Far(far)) => UnresolvedTransfer::FarDirect { seg: far.seg, off: far.off },
      _ => UnresolvedTransfer::FarIndirect,
    },
    // A near jump with a register or memory operand: the target is only
    // known at run time (or is a jump table the caller may resolve from
    // the config, which it tries first).
    _ => UnresolvedTransfer::NearIndirect,
  }
}
