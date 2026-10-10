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
  // must not be guessed here. (An IRET-ending function is usually
  // classified instead, by whatever RET/RETF decoding reaches past the
  // IRET — see the Interrupt-arm comment in analyze.rs.) analyze.rs
  // surfaces None as an explicit RET_UNKNOWN suggestion flag for the
  // operator to resolve.
  pub return_kind:       Option<ReturnKind>,
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
            if return_kind.is_none() {
              return_kind = Some(ret);
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
}

// FIXME: THIS FUNCTION IS WAY TOO COMPLICATED FOR ITS SIMPLE TASK: APIs NEED IMPROVEMENT
fn decode_one_instr(binary: &Binary, loc: SegOff, end: SegOff) -> Result<Instr, String> {
  let mut decoder = Decoder::new(binary.region_iter(loc, end));
  let (instr, _raw) = decoder.try_next()?.unwrap();
  Ok(instr)
}
