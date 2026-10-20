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
  // from the first exit in ascending block order — not necessarily the
  // lowest-address instruction, since overlapping blocks from
  // mid-instruction jump targets are possible — and these others were
  // dropped, so the annotated call mode may not fit every exit path.
  // Repeats of the winning kind are normal (any multi-`ret` function)
  // and stay unrecorded; see the Next::Return handling below.
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
        let details = instr_details::instr_details(&instr, &binary, code_seg.primary.seg)?;

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
            } else if !code_seg.contains(target) {
              // The code ends with the segment: there is nothing to
              // decode at the fallthrough address.
              block.exits = vec![];
              block_complete = true;
            } else {
              addr = target;
              continue;
            }
          }
          Next::Return(ret) => {
            // First-wins on ascending block order (see WorkQueue::pop):
            // the first exit in block-pop order classifies the function.
            // Later exits of a *differing* kind are recorded instead of
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
          Next::Jump { targets, fallthrough } => {
            let mut followed = vec![];
            for tgt in &targets {
              // A branch target outside the analyzed segment has no code
              // to continue at: decoding there would read past the
              // segment's bytes (or from another segment entirely).
              // The diagnostic goes to stderr: stdout carries the
              // annotation report.
              if !code_seg.contains(*tgt) {
                eprintln!("WARN: branch target {} is outside the analyzed segment [{}, {}); it is not followed",
                  tgt, code_seg.start(), code_seg_end);
                continue;
              }
              workqueue.insert(*tgt);
              followed.push(*tgt);
            }
            // The implicit fallthrough of a conditional branch is where
            // the branch lands when it is not taken, not a branch
            // target: it is followed when it is inside the segment, and
            // ends the block quietly when the code simply runs out.
            if let Some(fallthrough) = fallthrough {
              if code_seg.contains(fallthrough) {
                workqueue.insert(fallthrough);
                followed.push(fallthrough);
              }
            }
            // Add the exits that were actually analyzed.
            block.exits = followed;
            block_complete = true;
          }
          Next::Transfer => {
            // Control leaves the function (a far jump into another
            // segment, or a jump with no static target): the block ends
            // here and the function's return kind stays unknown, which
            // the report states explicitly instead of guessing.
            block.exits = vec![];
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
    // `jz +1; ret; iret`: the first exit in ascending block order (the
    // `ret` at 0x02) wins the classification, but the differing `iret`
    // exit must be recorded, not silently dropped, so the diagnostic
    // can flag it.
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

  // #65 shape: entry jumps over embedded data bytes.
  // `jmp 0006; db 63,63,63; mov ax,4c00; int 21h; ret`.
  fn gap_binary() -> Binary {
    Binary::from_raw(&[0xE9, 0x03, 0x00, 0x63, 0x63, 0x63,
                       0xB8, 0x00, 0x4C, 0xCD, 0x21, 0xC3], None)
  }

  #[test]
  fn first_undecodable_addr_flags_jumped_over_data() {
    let binary = gap_binary();
    assert_eq!(first_undecodable_addr(&binary, segoff(0), segoff(12)), Some(segoff(3)));
    assert_eq!(first_undecodable_addr(&binary, segoff(6), segoff(12)), None);
    assert_eq!(first_undecodable_addr(&binary, segoff(6), segoff(6)), None, "empty range decodes cleanly");
  }

  #[test]
  fn first_undecodable_addr_is_total_on_bad_ranges() {
    // Out-of-contract ranges report nothing instead of panicking in
    // Binary::region: inverted ranges have nothing to decode, and a
    // cross-segment range is not linearly decodable as one segment.
    use crate::segoff::Off;
    let binary = gap_binary();
    assert_eq!(first_undecodable_addr(&binary, segoff(6), segoff(3)), None);
    let cross = SegOff { seg: Seg::Normal(1), off: Off(12) };
    assert_eq!(first_undecodable_addr(&binary, segoff(0), cross), None);
  }

  #[test]
  fn inferred_end_spans_gap_that_linear_decode_rejects() {
    // Analysis follows control flow past the data, so the inferred end
    // spans it — while the linear decode that --codeseg-name performs
    // over the same range fails at the gap. This is the #65 mismatch
    // the UNDECODABLE GAP diagnostic reports.
    let binary = gap_binary();
    let details = FuncDetails::build(segoff(0), None, &test_seg(12), &binary).unwrap();
    assert_eq!(details.end_addr_inferred, segoff(12));
    assert_eq!(first_undecodable_addr(&binary, details.start_addr, details.end_addr_inferred),
      Some(segoff(3)));
  }

  #[test]
  fn far_jump_ends_the_analysis_without_failing_it() {
    // `jmp far 1234:0100` (EA 00 01 34 12): the transfer leaves the
    // analyzed segment, so the block ends there. The analysis must
    // complete — it used to fail with "Unsupported branch instruction",
    // losing the function (issue #112) — and report no return kind,
    // because a transfer is not a return.
    let binary = Binary::from_raw(&[0xEA, 0x00, 0x01, 0x34, 0x12], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(5), &binary).unwrap();
    assert_eq!(details.end_addr_inferred, segoff(5));
    assert_eq!(details.return_kind, None, "a transfer is not a return");
  }

  #[test]
  fn same_segment_far_jump_is_followed_by_the_analysis() {
    // `jmp far 0000:0005` (EA 05 00 00 00) then `ret` at offset 5: CS
    // does not change, so the analysis continues at the target and sees
    // the return.
    let binary = Binary::from_raw(&[0xEA, 0x05, 0x00, 0x00, 0x00, 0xC3], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(6), &binary).unwrap();
    assert_eq!(details.return_kind, Some(ReturnKind::Near), "the analysis must reach the ret");
  }

  #[test]
  fn a_fallthrough_at_the_segment_end_ends_the_block() {
    // A segment whose last byte is not a terminator: the fallthrough
    // lands at the segment's end, where there is nothing to decode.
    // That used to unwrap an empty region and take the analysis down.
    let binary = Binary::from_raw(&[0x90], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(1), &binary).unwrap();
    assert_eq!(details.end_addr_inferred, segoff(1));
  }

  #[test]
  fn out_of_segment_branch_target_is_not_followed() {
    // `jmp +0x10` (EB 10) in a 2-byte segment: the target has no code
    // to continue at, and decoding there would read past the segment's
    // bytes.
    let binary = Binary::from_raw(&[0xEB, 0x10], None);
    let details = FuncDetails::build(segoff(0), None, &test_seg(2), &binary).unwrap();
    assert_eq!(details.end_addr_inferred, segoff(2));
    assert_eq!(details.return_kind, None, "the jump leaves the function");
  }
}

// FIXME: THIS FUNCTION IS WAY TOO COMPLICATED FOR ITS SIMPLE TASK: APIs NEED IMPROVEMENT
fn decode_one_instr(binary: &Binary, loc: SegOff, end: SegOff) -> Result<Instr, String> {
  let mut decoder = Decoder::new(binary.region_iter(loc, end));
  // Nothing to decode at this address (the region is empty): report it
  // instead of unwrapping a `None` and taking the analysis down.
  let Some((instr, _raw)) = decoder.try_next()? else {
    return Err(format!("no instruction at {}", loc));
  };
  Ok(instr)
}

/// First address in `[start, end)` that fails a linear decode, or `None`
/// when the whole range decodes. This mirrors `decompile_spec`'s decode
/// loop over the same range, so anything reported here is guaranteed to
/// abort `--codeseg-name` today: analysis follows control flow and its
/// inferred end can span bytes that are not code (embedded banners,
/// tables, or padding jumped over), while decompilation decodes the
/// declared range linearly. The operator signal is to audit/shorten the
/// suggested range around the reported address (#65).
pub fn first_undecodable_addr(binary: &Binary, start: SegOff, end: SegOff) -> Option<SegOff> {
  // Total on out-of-contract inputs instead of panicking in
  // Binary::region: an empty/inverted range has nothing to decode, and
  // a cross-segment range is not linearly decodable as one segment.
  if start.seg != end.seg || end <= start {
    return None;
  }
  let mut decoder = Decoder::new(binary.region_iter(start, end));
  let mut addr = start;
  loop {
    match decoder.try_next() {
      Ok(Some((instr, _raw))) => addr = instr.end_addr(),
      Ok(None) => return None,
      Err(_) => return Some(addr),
    }
  }
}
