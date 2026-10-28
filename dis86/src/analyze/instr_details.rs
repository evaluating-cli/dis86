use crate::asm::instr::{Instr, Opcode, Operand};
use crate::asm::branch;
use crate::asm::intel_syntax::instr_str;
use crate::segoff::{SegOff, Seg, Off};
use crate::binary::Binary;
use std::fmt;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ReturnKind {
  Near,
  Far,
  Interrupt,
}

impl fmt::Display for ReturnKind {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      ReturnKind::Near      => write!(f, "near"),
      ReturnKind::Far       => write!(f, "far"),
      ReturnKind::Interrupt => write!(f, "interrupt"),
    }
  }
}

pub enum Next {
  Fallthrough(SegOff),     // ordinary instruction fallthrough
  Return(ReturnKind),      // return instr (block terminator)
  // Jump targets (block terminator) plus the implicit fallthrough of a
  // conditional branch. The two are kept apart: a fallthrough is not a
  // branch target, and it must not be reported as one.
  Jump { targets: Vec<SegOff>, fallthrough: Option<SegOff> },
  // Control leaves the function: the block ends here and the analysis
  // does not follow the transfer. A far jump into another segment, or a
  // jump whose target is only known at run time, has no address in the
  // analyzed segment to continue at.
  Transfer,
}

pub enum Call {
  Direct(SegOff),
  Indirect,
}

fn determine_callf(ins: &Instr, binary: &Binary) -> Result<Call, String> {
  // FIXME: UNIFY WITH ir_build::process_callf
  if let Operand::Far(far) = &ins.operands[0] {
    let seg = if ins.addr.seg.is_overlay() {
      // Far calls from overlays need to be remapped
      binary.remap_to_segment(far.seg)
    } else {
      // Otherwise: Normal
      Seg::Normal(far.seg)
    };
    let addr = SegOff { seg, off: Off(far.off) };
    Ok(Call::Direct(addr))
  } else if let Operand::Mem(_) = &ins.operands[0] {
    Ok(Call::Indirect)
  } else {
    Err(format!("Unsupported operand to CALLF for '{}'", instr_str(ins)))
  }
}

fn determine_calln(ins: &Instr, _binary: &Binary) -> Result<Call, String> {
  if let Operand::Rel(rel) = &ins.operands[0] {
    let addr = ins.rel_addr(rel);
    Ok(Call::Direct(addr))
  } else if let Operand::Mem(_) = &ins.operands[0] {
    Ok(Call::Indirect)
  } else if let Operand::Reg(_) = &ins.operands[0] {
    // Register-indirect near calls (`call cx`) have no static target;
    // counted as indirect like memory-indirect calls. Matches the
    // decompiler, which models them via CallPtr (ir_build.rs).
    Ok(Call::Indirect)
  } else {
    Err(format!("Unsupported operand to CALL for '{}'", instr_str(ins)))
  }
}

pub struct InstrDetails {
  pub next: Next,
  pub call: Option<Call>,
}

/// What happens after `ins` when it is a branch.
///
/// The branch classification is shared with the decompiler (see
/// `asm::branch`), so the two cannot drift apart: this module used to
/// keep its own copy of the table, which lost `LOOPE`/`LOOPNE` (so
/// `--analyze` did not treat `loopne` as a branch) and reported every far
/// jump as a hard error, losing the whole function's analysis.
fn jump_targets(ins: &Instr, analyzed_seg: Seg) -> Option<Next> {
  match branch::branch_target(ins)? {
    branch::BranchTarget::Relative { target, fallthrough } =>
      Some(Next::Jump { targets: vec![target], fallthrough }),
    branch::BranchTarget::Unresolved(kind) => Some(unresolved_next(ins, kind, analyzed_seg)),
  }
}

/// The continuation of a branch that is not a relative one.
///
/// A far jump into the analyzed segment is a jump to that address: CS
/// does not change, so the address is in the same segment the analysis
/// walks. Every other transfer (a far jump into another segment, or a
/// jump whose target is only known at run time) leaves the analyzed
/// segment, so the block ends there: following it would decode bytes
/// from the wrong address space.
fn unresolved_next(ins: &Instr, kind: branch::UnresolvedTransfer, analyzed_seg: Seg) -> Next {
  // The diagnostics go to stderr: `--analyze` writes the annotation
  // report to stdout, and that report is a fragment to be pasted into a
  // config source.
  let branch::UnresolvedTransfer::FarDirect { seg: far_seg, off } = kind else {
    eprintln!("WARN: {}: jump target is only known at run time; the analysis stops the block here",
      instr_str(ins));
    return Next::Transfer;
  };
  // Only a normal (non-overlay) segment can be compared: in an overlay
  // the segment value in the code belongs to the original program's
  // address space, which the analysis does not model.
  let Seg::Normal(cur_seg) = analyzed_seg else {
    eprintln!("WARN: {}: far jump leaves the analyzed segment; the analysis stops the block here",
      instr_str(ins));
    return Next::Transfer;
  };
  if far_seg != cur_seg {
    eprintln!("WARN: {}: far jump to {:04x}:{:04x} leaves the analyzed segment; the analysis stops the block here",
      instr_str(ins), far_seg, off);
    return Next::Transfer;
  }
  Next::Jump { targets: vec![SegOff { seg: analyzed_seg, off: Off(off) }], fallthrough: None }
}

/// The details of `ins` as seen by an analysis walking `analyzed_seg`.
/// The segment is the one the analysis walks, not the one the
/// instruction was decoded from: what a far jump's encoded segment is
/// compared against is the analyzed segment.
pub fn instr_details(ins: &Instr, binary: &Binary, analyzed_seg: Seg) -> Result<InstrDetails, String> {
  if let Some(next) = jump_targets(ins, analyzed_seg) {
    return Ok(InstrDetails { next, call: None });
  }

  let mut call = None;
  let mut ret = None;

  // NOTE: Need to complete this. We don't support everything because we're being overly conservative.
  // Need to carefully think about the other instructions before adding them.
  match ins.opcode {
    Opcode::OP_AAA        => (),
    Opcode::OP_AAS        => (),
    Opcode::OP_ADC        => (),
    Opcode::OP_ADD        => (),
    Opcode::OP_AND        => (),
    Opcode::OP_CALL       => call = Some(determine_calln(ins, binary)?),
    Opcode::OP_CALLF      => call = Some(determine_callf(ins, binary)?),
    Opcode::OP_CBW        => (),
    Opcode::OP_CLC        => (),
    Opcode::OP_CLD        => (),
    Opcode::OP_CLI        => (),
    Opcode::OP_CMC        => (),
    Opcode::OP_CMP        => (),
    Opcode::OP_CMPS       => (),
    Opcode::OP_CWD        => (),
    Opcode::OP_DAA        => (),
    Opcode::OP_DAS        => (),
    Opcode::OP_DEC        => (),
    Opcode::OP_DIV        => (),
    Opcode::OP_ENTER      => (),
    // Opcode::OP_HLT     => (),
    Opcode::OP_IDIV       => (),
    Opcode::OP_IMUL       => (),
    Opcode::OP_IMUL_TRUNC => (),
    Opcode::OP_IN         => (),
    Opcode::OP_INC        => (),
    Opcode::OP_INS        => (),
    Opcode::OP_INT        => (),
    // Opcode::OP_INTO    => (),
    // Opcode::OP_INVAL   => (),
    Opcode::OP_IRET       => ret = Some(ReturnKind::Interrupt),
    // Opcode::OP_JA      => (),
    // Opcode::OP_JAE     => (),
    // Opcode::OP_JB      => (),
    // Opcode::OP_JBE     => (),
    // Opcode::OP_JCXZ    => (),
    // Opcode::OP_JE      => (),
    // Opcode::OP_JG      => (),
    // Opcode::OP_JGE     => (),
    // Opcode::OP_JL      => (),
    // Opcode::OP_JLE     => (),
    // Opcode::OP_JMP     => (),
    // Opcode::OP_JMPF    => (),
    // Opcode::OP_JNE     => (),
    // Opcode::OP_JNO     => (),
    // Opcode::OP_JNP     => (),
    // Opcode::OP_JNS     => (),
    // Opcode::OP_JO      => (),
    // Opcode::OP_JP      => (),
    // Opcode::OP_JS      => (),
    Opcode::OP_LAHF       => (),
    Opcode::OP_LDS        => (),
    Opcode::OP_LEA        => (),
    Opcode::OP_LEAVE      => (),
    Opcode::OP_LES        => (),
    Opcode::OP_LODS       => (),
    // Opcode::OP_LOOP    => (),
    // Opcode::OP_LOOPE   => (),
    // Opcode::OP_LOOPNE  => (),
    Opcode::OP_MOV        => (),
    Opcode::OP_MOVS       => (),
    Opcode::OP_MUL        => (),
    Opcode::OP_NEG        => (),
    Opcode::OP_NOP        => (),
    Opcode::OP_NOT        => (),
    Opcode::OP_OR         => (),
    Opcode::OP_OUT        => (),
    Opcode::OP_OUTS       => (),
    Opcode::OP_POP        => (),
    Opcode::OP_POPA       => (),
    Opcode::OP_POPF       => (),
    Opcode::OP_PUSH       => (),
    Opcode::OP_PUSHA      => (),
    Opcode::OP_PUSHF      => (),
    Opcode::OP_RCL        => (),
    Opcode::OP_RCR        => (),
    Opcode::OP_RET        => ret = Some(ReturnKind::Near),
    Opcode::OP_RETF       => ret = Some(ReturnKind::Far),
    Opcode::OP_ROL        => (),
    Opcode::OP_ROR        => (),
    Opcode::OP_SAHF       => (),
    Opcode::OP_SAR        => (),
    Opcode::OP_SBB        => (),
    Opcode::OP_SCAS       => (),
    Opcode::OP_SETO       => (),
    Opcode::OP_SETNO      => (),
    Opcode::OP_SETA       => (),
    Opcode::OP_SETAE      => (),
    Opcode::OP_SETB       => (),
    Opcode::OP_SETBE      => (),
    Opcode::OP_SETE       => (),
    Opcode::OP_SETG       => (),
    Opcode::OP_SETGE      => (),
    Opcode::OP_SETL       => (),
    Opcode::OP_SETLE      => (),
    Opcode::OP_SETP       => (),
    Opcode::OP_SETS       => (),
    Opcode::OP_SETNE      => (),
    Opcode::OP_SETNP      => (),
    Opcode::OP_SETNS      => (),
    Opcode::OP_SHL        => (),
    Opcode::OP_SHR        => (),
    Opcode::OP_STC        => (),
    Opcode::OP_STD        => (),
    Opcode::OP_STI        => (),
    Opcode::OP_STOS       => (),
    Opcode::OP_SUB        => (),
    Opcode::OP_TEST       => (),
    Opcode::OP_XCHG       => (),
    Opcode::OP_XLAT       => (),
    Opcode::OP_XOR        => (),

    _ => return Err(format!("Unsupported instruction for analysis: {}", instr_str(ins))),
  };

  if let Some(ret) = ret {
    return Ok(InstrDetails { next: Next::Return(ret), call: None });
  }

  Ok(InstrDetails { next: Next::Fallthrough(ins.end_addr()), call })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::asm::decode::Decoder;
  use crate::segoff::{Seg, Off};

  fn details_for(bytes: &[u8]) -> InstrDetails {
    details_for_seg(bytes, Seg::Normal(0))
  }

  /// The details of the first instruction of `bytes`, as seen by an
  /// analysis walking segment `seg`. The instruction itself is decoded
  /// from a normal segment: what the far-jump classification compares is
  /// the segment the analysis walks, not the one the bytes live in.
  fn details_for_seg(bytes: &[u8], seg: Seg) -> InstrDetails {
    let binary = Binary::from_raw(bytes, None);
    let decode_seg = Seg::Normal(0);
    let start = SegOff { seg: decode_seg, off: Off(0) };
    let end = SegOff { seg: decode_seg, off: Off(bytes.len() as u16) };
    let mut decoder = Decoder::new(binary.region_iter(start, end));
    let (instr, _raw) = decoder.try_next().unwrap().unwrap();
    instr_details(&instr, &binary, seg).unwrap()
  }

  #[test]
  fn register_indirect_call_counts_as_indirect() {
    // `call cx` (FF D1) has no static target; it must not error, matching
    // the decompiler which models it via CallPtr.
    let d = details_for(&[0xFF, 0xD1, 0xC3]);
    assert!(matches!(d.call, Some(Call::Indirect)));
  }

  #[test]
  fn memory_indirect_call_counts_as_indirect() {
    // `call far [0x0000]` (FF 1E 0000).
    let d = details_for(&[0xFF, 0x1E, 0x00, 0x00, 0xC3]);
    assert!(matches!(d.call, Some(Call::Indirect)));
  }

  #[test]
  fn relative_call_is_direct() {
    // `call +1` (E8 01 00) targets the `ret` at offset 3.
    let d = details_for(&[0xE8, 0x01, 0x00, 0xC3]);
    assert!(matches!(d.call, Some(Call::Direct(_))));
  }

  #[test]
  fn unsupported_opcode_is_a_result_error_not_a_panic() {
    // HLT decodes, but the conservative analyzer table does not model it.
    // It must return an error so preflight can report it instead of unwinding.
    let binary = Binary::from_raw(&[0xF4], None);
    let start = SegOff { seg: Seg::Normal(0), off: Off(0) };
    let end = SegOff { seg: Seg::Normal(0), off: Off(1) };
    let mut decoder = Decoder::new(binary.region_iter(start, end));
    let (instr, _raw) = decoder.try_next().unwrap().unwrap();
    let err = match instr_details(&instr, &binary, Seg::Normal(0)) {
      Err(err) => err,
      Ok(_) => panic!("unsupported opcode must return an error"),
    };
    assert!(err.contains("Unsupported instruction"), "unexpected error: {}", err);
  }

  #[test]
  fn iret_yields_interrupt_return() {
    // `iret` (CF) terminates the block as an interrupt return, not a
    // fallthrough: decoding must not continue past it.
    let d = details_for(&[0xCF]);
    assert!(matches!(d.next, Next::Return(ReturnKind::Interrupt)));
  }

  #[test]
  fn loopcc_variants_are_branches() {
    // The analyzer kept its own copy of the branch table, which lost
    // `LOOPE`/`LOOPNE`: they were not branches at all, and the analysis
    // panicked on them ("UNIMPL Opcode for loopne") instead of following
    // the branch (issue #112).
    for opcode_bytes in [[0xE0, 0xFE], [0xE1, 0xFE], [0xE2, 0xFE], [0xE3, 0xFE]] {
      let d = details_for(&[opcode_bytes[0], opcode_bytes[1], 0xC3]);
      let Next::Jump { targets, fallthrough } = d.next else {
        panic!("loopcc must be a branch");
      };
      assert_eq!(targets.len(), 1, "loopcc has one target");
      assert!(fallthrough.is_some(), "loopcc has a fallthrough");
    }
  }

  #[test]
  fn same_segment_far_jump_is_followed() {
    // `jmp far 0000:0005` (EA 05 00 00 00) does not change CS, so the
    // analysis continues at that address.
    let d = details_for(&[0xEA, 0x05, 0x00, 0x00, 0x00]);
    let Next::Jump { targets, fallthrough } = d.next else {
      panic!("a same-segment far jump must be a jump");
    };
    assert_eq!(targets, vec![SegOff { seg: Seg::Normal(0), off: Off(5) }]);
    assert_eq!(fallthrough, None, "a far jump does not fall through");
  }

  #[test]
  fn a_conditional_branch_keeps_its_fallthrough_apart() {
    // `jz +1` (74 01): the target and the fallthrough are both
    // destinations, but the fallthrough is not a branch target.
    let d = details_for(&[0x74, 0x01, 0xC3]);
    let Next::Jump { targets, fallthrough } = d.next else {
      panic!("a conditional branch must be a jump");
    };
    assert_eq!(targets, vec![SegOff { seg: Seg::Normal(0), off: Off(3) }]);
    assert_eq!(fallthrough, Some(SegOff { seg: Seg::Normal(0), off: Off(2) }));
  }

  #[test]
  fn cross_segment_far_jump_ends_the_block() {
    // `jmp far 1234:0100` (EA 00 01 34 12) leaves the analyzed
    // segment: the analysis stops the block there instead of decoding
    // in another segment's address space.
    let d = details_for(&[0xEA, 0x00, 0x01, 0x34, 0x12]);
    assert!(matches!(d.next, Next::Transfer));
  }

  #[test]
  fn far_jump_in_an_overlay_ends_the_block() {
    // In an overlay the segment value in the code belongs to the
    // original program's address space, so the analysis cannot resolve
    // the target and stops the block.
    let d = details_for_seg(&[0xEA, 0x05, 0x00, 0x00, 0x00], Seg::Overlay(0));
    assert!(matches!(d.next, Next::Transfer));
  }

  #[test]
  fn indirect_far_jump_ends_the_block() {
    // `jmp far [0x1234]` (FF 2E 34 12): the target is read from memory
    // at run time, so there is no address to continue at.
    let d = details_for(&[0xFF, 0x2E, 0x34, 0x12]);
    assert!(matches!(d.next, Next::Transfer));
  }
}
