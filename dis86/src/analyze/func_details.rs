use super::code_segment::CodeSegment;
use super::workqueue::WorkQueue;
use crate::segoff::SegOff;
use crate::binary::Binary;
use crate::asm::instr::Instr;
use crate::asm::decode::Decoder;
use crate::asm::intel_syntax::instr_str;
use std::collections::BTreeSet;
use std::fmt;

use crate::analyze::instr_details::{self, Next, Call};
pub use crate::analyze::instr_details::ReturnKind;

const DEBUG: bool = false;
//const DEBUG: bool = true;

pub struct FuncDetails {
  pub start_addr:        SegOff,
  pub end_addr_inferred: SegOff,
  pub direct_calls:      BTreeSet<SegOff>,
  pub indirect_calls:    usize,
  // None when no return instruction was observed (noreturn helper or
  // tail-jump exit): the call mode is not inferable from analysis and
  // must not be guessed here. (An IRET-ending function instead reports
  // Some(ReturnKind::Interrupt); see Next::Return handling below.)
  // analyze.rs surfaces None as an explicit RET_UNKNOWN suggestion flag
  // for the operator to resolve.
  pub return_kind:       Option<ReturnKind>,
  // Return kinds observed on exits other than the one `return_kind`
  // reports (first-wins on ascending block order). Non-empty means the
  // function has exits of differing kinds: the classified kind came
  // from the lowest-address exit and these others were dropped, so the
  // annotated call mode may not fit every exit path. Repeats of the
  // winning kind are normal (any multi-`ret` function) and stay
  // unrecorded; see the Next::Return handling below.
  pub ignored_return_kinds: Vec<ReturnKind>,
}

impl fmt::Display for FuncDetails {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    writeln!(f, "start_addr:        {}", self.start_addr)?;
    writeln!(f, "end_addr_inferred: {}", self.end_addr_inferred)?;
    write!(f,   "direct_calls:      [")?;
    for (i, c) in self.direct_calls.iter().enumerate() {
      if i != 0 { write!(f, ", ")?; }
      write!(f, "{}", c)?;
    }
    writeln!(f, "]")?;
    writeln!(f, "indirect_calls:    {}", self.indirect_calls)?;
    match self.return_kind {
      Some(kind) => writeln!(f, "return_kind:       {}", kind)?,
      None       => writeln!(f, "return_kind:       unknown")?,
    }
    if !self.ignored_return_kinds.is_empty() {
      let others: Vec<String> = self.ignored_return_kinds.iter().map(|k| k.to_string()).collect();
      writeln!(f, "ignored_return_kinds: [{}]", others.join(", "))?;
    }
    Ok(())
  }
}

struct Block {
  start: SegOff,
  exits: Vec<SegOff>,
  instrs: Vec<Instr>,
}

impl fmt::Display for Block {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "Block {} | exits: [", self.start)?;
    for (i, exit) in self.exits.iter().enumerate() {
      if i != 0 { write!(f, ", ")?; }
      write!(f, "{}", exit)?;
    }
    writeln!(f, "]")?;
    writeln!(f, "------------------------------------")?;
    for instr in &self.instrs {
      writeln!(f, "{} | {}", instr.addr, instr_str(instr))?;
    }
    Ok(())
  }
}

impl FuncDetails {
  pub fn build(func_start: SegOff, end: Option<SegOff>, code_seg: &CodeSegment, binary: &Binary) -> Result<FuncDetails, String> {
    assert!(func_start >= code_seg.start());
    let code_seg_end = code_seg.end();

    let mut workqueue = WorkQueue::new();
    workqueue.insert(func_start);

    let mut largest_addr = func_start;
    let mut direct_calls = BTreeSet::new();
    let mut indirect_calls = 0;
    let mut return_kind = None;
    let mut ignored_return_kinds: Vec<ReturnKind> = vec![];

    // Iterate over blocks
    while let Some(loc) = workqueue.pop() {
      let mut block = Block {
        start: loc,
        exits: vec![], // not yet known
        instrs: vec![],
      };

      let mut addr = loc;

      // Iterate over instructions in the block (until we reach a terminator)
      let mut block_complete = false;
      while !block_complete {
        // Decode next instruction
        let instr = decode_one_instr(&binary, addr, code_seg_end)?;
        let end_addr = instr.end_addr();
        if end_addr > largest_addr { largest_addr = end_addr; }

        if DEBUG { println!("INSTR | {} | {}", instr.addr, instr_str(&instr)); }

        // Add instr to the block
        block.instrs.push(instr);

        // Compute instr details
        let details = instr_details::instr_details(&instr, &binary)?;

        // Handle calls
        match &details.call {
          Some(Call::Direct(addr)) => {
            if DEBUG { println!("Call to {}", addr); }
            direct_calls.insert(*addr);
          }
          Some(Call::Indirect) => {
            if DEBUG { println!("Indirect call"); }
            indirect_calls += 1;
          }
          None => (),
        }

        // Figure out what to do next
        match details.next {
          Next::Fallthrough(target) => {
            // Respect the annotated (exclusive) end bound: a fallthrough
            // that reaches or passes it is the function exit (e.g. a stub
            // ending in a call followed by data). Without this, discovery
            // walks off into bytes that are not code.
            if end.is_some_and(|end| target >= end) {
              block.exits = vec![];
              block_complete = true;
            } else {
              addr = target;
              continue;
            }
          }
          Next::Return(ret) => {
            // First-wins on ascending block order (see WorkQueue::pop):
            // the lowest-address exit classifies the function. Later
            // exits of a *differing* kind are recorded instead of
            // silently dropped, so the diagnostic can flag a function
            // whose annotated call mode may not fit every exit path.
            // Repeats of the winning kind are normal and unrecorded.
            match return_kind {
              None => return_kind = Some(ret),
              Some(winning) if winning != ret && !ignored_return_kinds.contains(&ret) => {
                ignored_return_kinds.push(ret);
              }
              _ => (),
            }
            block.exits = vec![];
            block_complete = true;
          }
          Next::Jump(targets) => {
            for tgt in &targets {
              workqueue.insert(*tgt);
            }
            // Add exits to block
            block.exits = targets;
            block_complete = true;
          }
        }
      }

      if DEBUG { println!("{}", block); }
    }

    Ok(FuncDetails {
      start_addr: func_start,
      end_addr_inferred: largest_addr,
      direct_calls,
      indirect_calls,
      // No defaulting: a function with no observed return leaves return_kind
      // as None (see the field doc). Guessing Near here would silently
      // mislabel genuine far-noreturn functions as near in the generated
      // configs; analyze.rs reports the unknown kind explicitly instead.
      return_kind,
      ignored_return_kinds,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use super::super::code_segment::{CodeSegment, Region};
  use crate::segoff::{Seg, Off};

  fn test_seg(size: u32) -> CodeSegment {
    CodeSegment {
      primary: Region { seg: Seg::Normal(0), skip_off: 0, size },
      stub: None,
    }
  }

  fn segoff(off: u16) -> SegOff {
    SegOff { seg: Seg::Normal(0), off: Off(off) }
  }

  #[test]
  fn analysis_respects_annotated_end_bound() {
    // `call +1; arpl ...`: a stub ending in a call followed by data bytes.
    // The 0x63
    // byte is not decodable here, so unbounded analysis must fail while
    // analysis bounded at the call's end must stop cleanly.
    let binary = Binary::from_raw(&[0xE8, 0x01, 0x00, 0x63], None);
    let seg = test_seg(4);

    assert!(FuncDetails::build(segoff(0), None, &seg, &binary).is_err());

    let bounded = FuncDetails::build(segoff(0), Some(segoff(3)), &seg, &binary).unwrap();
    assert_eq!(bounded.end_addr_inferred, segoff(3));
    assert_eq!(bounded.indirect_calls, 0);
    assert_eq!(bounded.direct_calls.len(), 1);
  }

  #[test]
  fn ret_yields_near_return_kind() {
    let binary = Binary::from_raw(&[0xC3], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(1), &binary).unwrap();
    assert_eq!(details.return_kind, Some(ReturnKind::Near));
  }

  #[test]
  fn retf_yields_far_return_kind() {
    let binary = Binary::from_raw(&[0xCB], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(1), &binary).unwrap();
    assert_eq!(details.return_kind, Some(ReturnKind::Far));
  }

  #[test]
  fn noreturn_function_reports_unknown_return_kind() {
    // A `jmp` to self never reaches a return instruction. The analyzer must
    // report the kind as unknown rather than guessing Near (which mislabeled
    // genuine far-noreturn functions as near in the generated configs).
    let binary = Binary::from_raw(&[0xEB, 0xFE], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(2), &binary).unwrap();
    assert_eq!(details.return_kind, None);
  }

  #[test]
  fn iret_terminates_function_as_interrupt() {
    // An interrupt handler ends at the IRET: discovery must stop there
    // (end == 1) instead of decoding the trailing undecodable bytes, and
    // the kind must be Interrupt rather than unknown.
    let binary = Binary::from_raw(&[0xCF, 0xFF, 0xFF], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(3), &binary).unwrap();
    assert_eq!(details.return_kind, Some(ReturnKind::Interrupt));
    assert_eq!(details.end_addr_inferred, segoff(1));
  }

  #[test]
  fn mixed_return_kinds_record_ignored_not_winning() {
    // `jz +1; ret; iret`: the lowest-address exit (the `ret` at 0x02)
    // wins the classification, but the differing `iret` exit must be
    // recorded, not silently dropped, so the diagnostic can flag it.
    let binary = Binary::from_raw(&[0x74, 0x01, 0xC3, 0xCF], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(4), &binary).unwrap();
    assert_eq!(details.return_kind, Some(ReturnKind::Near));
    assert_eq!(details.ignored_return_kinds, vec![ReturnKind::Interrupt]);
  }

  #[test]
  fn ret_retf_mix_records_ignored_far() {
    // The pre-existing RET/RETF variant of the same rule: `jz +1; ret;
    // retf` classifies Near and records the dropped Far exit.
    let binary = Binary::from_raw(&[0x74, 0x01, 0xC3, 0xCB], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(4), &binary).unwrap();
    assert_eq!(details.return_kind, Some(ReturnKind::Near));
    assert_eq!(details.ignored_return_kinds, vec![ReturnKind::Far]);
  }

  #[test]
  fn repeated_winning_kind_records_nothing() {
    // Repeats of the winning kind are normal (any multi-`ret`
    // function): `jz +1; ret; ret` must record no ignored kinds.
    let binary = Binary::from_raw(&[0x74, 0x01, 0xC3, 0xC3], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(4), &binary).unwrap();
    assert_eq!(details.return_kind, Some(ReturnKind::Near));
    assert!(details.ignored_return_kinds.is_empty());
  }
}

// FIXME: THIS FUNCTION IS WAY TOO COMPLICATED FOR ITS SIMPLE TASK: APIs NEED IMPROVEMENT
fn decode_one_instr(binary: &Binary, loc: SegOff, end: SegOff) -> Result<Instr, String> {
  let mut decoder = Decoder::new(binary.region_iter(loc, end));
  let (instr, _raw) = decoder.try_next()?.unwrap();
  Ok(instr)
}
