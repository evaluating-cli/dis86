use crate::decompile::ir::*;
use crate::decompile::sym;
use crate::types::Type;
use std::collections::{hash_map, HashMap, HashSet, VecDeque};

// Propagate operand through any ref opcodes
fn operand_propagate(ir: &IR, mut r: Ref) -> Ref {
  // Ref cycles arise from loop-carried locals: mem_symbol_to_ref turns a
  // read-modify-write local in a loop into read -> phi -> ... -> read.
  // Stop at the cycle instead of spinning; the remaining alias is resolved
  // (or named) by later passes.
  let mut seen = HashSet::new();
  loop {
    let Some(instr) = ir.instr(r) else { return r };
    if instr.opcode != Opcode::Ref { return r; }
    if !seen.insert(r) { return r; }
    r = instr.operands[0];
  }
}

/*
From:
--------------------------------------------
  t2 = xor t1 t1

To:
--------------------------------------------
  t2 = #0
*/
pub fn reduce_xor(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();
      if instr.opcode != Opcode::Xor || instr.operands[0] != instr.operands[1] {
        continue;
      }
      let k = ir.const_new(0);
      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = Opcode::Ref;
      instr.operands = vec![k];
    }
  }
}

/*
From:
--------------------------------------------
  t2 = or t1 t1

To:
--------------------------------------------
  t2 = ref t1
*/
pub fn reduce_trivial_or(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();
      if instr.opcode != Opcode::Or || instr.operands[0] != instr.operands[1] {
        continue;
      }
      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = Opcode::Ref;
      instr.operands = vec![instr.operands[0]];
    }
  }
}

/*
Fold constants only where the IR's 16-bit constant representation is sufficient
for the operation's full semantics. In particular, do not fold byte/32-bit
arithmetic, shifts, or multiplication here: those need explicit width/count
rules rather than inheriting i16 behavior accidentally.
*/
pub fn constant_folding(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();
      if instr.operands.len() != 2 { continue; }

      let (Some(lhs), Some(rhs)) = (
        ir.const_lookup(instr.operands[0]),
        ir.const_lookup(instr.operands[1]),
      ) else { continue; };

      let fold_word_arith = matches!(instr.typ, Type::U16 | Type::I16);
      let fold_bool = matches!(instr.typ, Type::U8 | Type::U16);
      let signed_values = match instr.compare_width {
        Some(CompareWidth::Byte) => Some((lhs as u8 as i8 as i16, rhs as u8 as i8 as i16)),
        Some(CompareWidth::Word) => Some((lhs, rhs)),
        None => None,
      };

      let result = match instr.opcode {
        Opcode::Add if fold_word_arith => lhs.wrapping_add(rhs),
        Opcode::Sub if fold_word_arith => lhs.wrapping_sub(rhs),
        Opcode::And if fold_word_arith => lhs & rhs,
        Opcode::Or  if fold_word_arith => lhs | rhs,
        Opcode::Xor if fold_word_arith => lhs ^ rhs,

        Opcode::Eq   if fold_bool => if lhs == rhs { 1 } else { 0 },
        Opcode::Neq  if fold_bool => if lhs != rhs { 1 } else { 0 },
        Opcode::Lt if fold_bool => {
          let Some((lhs, rhs)) = signed_values else { continue };
          if lhs < rhs { 1 } else { 0 }
        }
        Opcode::Leq if fold_bool => {
          let Some((lhs, rhs)) = signed_values else { continue };
          if lhs <= rhs { 1 } else { 0 }
        }
        Opcode::Gt if fold_bool => {
          let Some((lhs, rhs)) = signed_values else { continue };
          if lhs > rhs { 1 } else { 0 }
        }
        Opcode::Geq if fold_bool => {
          let Some((lhs, rhs)) = signed_values else { continue };
          if lhs >= rhs { 1 } else { 0 }
        }
        Opcode::ULt  if fold_bool => if (lhs as u16) < (rhs as u16) { 1 } else { 0 },
        Opcode::ULeq if fold_bool => if (lhs as u16) <= (rhs as u16) { 1 } else { 0 },
        Opcode::UGt  if fold_bool => if (lhs as u16) > (rhs as u16) { 1 } else { 0 },
        Opcode::UGeq if fold_bool => if (lhs as u16) >= (rhs as u16) { 1 } else { 0 },
        _ => continue,
      };

      let k = ir.const_new(result);
      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = Opcode::Ref;
      instr.operands = vec![k];
    }
  }
}

/*
From:
--------------------------------------------
  t36      = signext32  t34
  dx.2     = upper16    t36
  t37      = make32     dx.2                 t34

To:
--------------------------------------------
  t37      = signext32  t34
*/
pub fn reduce_make_32_signext_32(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let make32_ref = r;
      let make32 = ir.instr(make32_ref).unwrap();
      if make32.opcode != Opcode::Make32 { continue; }

      let upper16_ref = make32.operands[0];
      let Some(upper16) = ir.instr(upper16_ref) else { continue };
      if upper16.opcode != Opcode::Upper16 { continue; }

      let signext32_ref = upper16.operands[0];
      let Some(signext32) = ir.instr(signext32_ref) else { continue };
      if signext32.opcode != Opcode::SignExtTo32 { continue; }

      if make32.operands[1] == signext32.operands[0] {
        let instr = ir.instr_mut(make32_ref).unwrap();
        instr.opcode = Opcode::Ref;
        instr.operands = vec![signext32_ref];
      }
    }
  }
}

/*
From:
--------------------------------------------
  t2 = upper t1
  t3 = lower t1
  t4 = make32 t2 t3

To:
--------------------------------------------
  t4 = ref t1
*/
pub fn reduce_upper_lower_make32(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let Some((make32_instr, make32_ref)) = ir.instr_matches(r, Opcode::Make32) else {continue};
      let Some((upper_instr, _)) = ir.instr_matches(make32_instr.operands[0], Opcode::Upper16) else {continue};
      let Some((lower_instr, _)) = ir.instr_matches(make32_instr.operands[1], Opcode::Lower16) else {continue};
      if upper_instr.operands[0] != lower_instr.operands[0] { continue }

      let src = upper_instr.operands[0];
      let instr = ir.instr_mut(make32_ref).unwrap();
      instr.opcode = Opcode::Ref;
      instr.operands = vec![src];
    }
  }
}

/*
From:
--------------------------------------------
  t2 = upper t1
  t3 = lower t1
  t4 = or t3 t2
  t5 = eq t4 #0

To:
--------------------------------------------
  t4 = eq t1 #0
*/
pub fn reduce_equal_zero_32(ir: &mut IR) {
  let ops = &[Opcode::Eq, Opcode::Neq];
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let Some((eq_instr, eq_ref)) = ir.instr_matches_one(r, ops) else {continue};
      let Some(k) = ir.const_lookup(eq_instr.operands[1]) else {continue};
      if k != 0 { continue; }
      let Some((or_instr, _)) = ir.instr_matches(eq_instr.operands[0], Opcode::Or) else {continue};

      let mut upper = false;
      let mut lower = false;
      if ir.instr_matches(or_instr.operands[0], Opcode::Upper16).is_some() { upper = true; }
      if ir.instr_matches(or_instr.operands[0], Opcode::Lower16).is_some() { lower = true; }
      if ir.instr_matches(or_instr.operands[1], Opcode::Upper16).is_some() { upper = true; }
      if ir.instr_matches(or_instr.operands[1], Opcode::Lower16).is_some() { lower = true; }

      if !upper || !lower { continue; }
      let ref32_1 = ir.instr(or_instr.operands[0]).unwrap().operands[0];
      let ref32_2 = ir.instr(or_instr.operands[1]).unwrap().operands[0];
      if ref32_1 != ref32_2 { continue; }

      // rewrite
      let instr = ir.instr_mut(eq_ref).unwrap();
      instr.typ = Type::U32;
      instr.operands[0] = ref32_1;
    }
  }
}

/*
From:
--------------------------------------------
  t5 = phi t5 t1 t1 t5 t5
To:
--------------------------------------------
  t5 = ref t1
*/
pub fn reduce_phi_single_ref(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      if ir.instr(r).unwrap().opcode != Opcode::Phi { continue; }

      // propagate while checking conditions
      let mut operands = ir.instr(r).unwrap().operands.clone();
      let mut trivial = true;
      let mut single_ref = None;
      for j in 0..operands.len() {
        operands[j] = operand_propagate(ir, operands[j]);
        if operands[j] == r { continue; }
        match &single_ref {
          None => single_ref = Some(operands[j]),
          Some(s) => if *s != operands[j] {
            trivial = false;
          }
        }
      }
      ir.instr_mut(r).unwrap().operands = operands;

      // all operands the same? reduce to a mov
      if trivial && single_ref.is_some() {
        let vref = single_ref.unwrap();
        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = Opcode::Ref;
        instr.operands = vec![vref];
      }
    }
  }
}

/*
From:
--------------------------------------------
b1:
  t1 = op r1 r2 r3
       jmp b3

b2:
  t2 = op r1 r2 r3
       jmp b3

b3: (b1 b2 b3)
  t5 = phi t1 t2 t5

To:
--------------------------------------------
b3: (b1, b2)
  t5 = op r1 r2 r3
*/
pub fn reduce_phi_common_subexpr(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      if ir.instr(r).unwrap().opcode != Opcode::Phi { continue; }

      let mut operands = ir.instr(r).unwrap().operands.clone();

      // propagate all operands
      for oper in &mut operands {
        *oper = operand_propagate(ir, *oper);
      }

      // find first non-trivial to act as common
      let mut common = None;
      for oper in &operands {
        if *oper == r { continue; }
        common = Some(*oper);
        break;
      }
      let Some(common) = common else { continue };
      let Some(common_instr) = ir.instr(common).cloned() else { continue };

      // need to pessimize around side-effecting operations
      if common_instr.opcode.has_side_effects() { continue; }

      // Don't forward phis
      if common_instr.opcode == Opcode::Phi { continue; }

      // see if all non-trivial operands match
      let mut all_match = true;
      for oper in &operands {
        if *oper == r { continue; }
        let instr = ir.instr(*oper);
        if instr.is_none() || &common_instr != instr.unwrap() {
          all_match = false;
          break;
        }
      }

      // re-write the phi
      if all_match {
        //print!("\nRewrite '{}' to", crate::ir::display::instr_to_string(ir, r));
        *ir.instr_mut(r).unwrap() = common_instr;
        //print!(" ... '{}'", crate::ir::display::instr_to_string(ir, r));
      }
    }
  }
}

fn stack_ptr_const_oper(ir: &IR, vref: Ref) -> Option<(Ref, i16)> {
  let instr = ir.instr(vref)?;
  if instr.operands.len() != 2 { return None; }
  if (instr.attrs & Attribute::STACK_PTR) == 0 { return None; }

  let (nref, cref) = (instr.operands[0], instr.operands[1]);
  let Ref::Const(_) = cref else { return None };

  match instr.opcode {
    // NOTE: Stack-pointer arithmetic wraps at 16 bits (e.g. `sub sp, 0x8000`
    // materializes as const -32768, whose negation overflows i16), so fold
    // with wrapping ops: bit-identical to the guest computation.
    Opcode::Add => Some((nref, ir.const_lookup(cref).unwrap())),
    Opcode::Sub => Some((nref, ir.const_lookup(cref).unwrap().wrapping_neg())),
    _ => None,
  }
}

pub fn stack_ptr_accumulation(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for vref in ir.iter_instrs(b) {
      let Some((_, a)) = stack_ptr_const_oper(ir, vref) else { continue };

      let instr = ir.instr(vref).unwrap();
      let Some((nref, b)) = stack_ptr_const_oper(ir, instr.operands[0]) else { continue };

      let k = a.wrapping_add(b);
      if k > 0 {
        let cref = ir.const_new(k);
        let instr = ir.instr_mut(vref).unwrap();
        instr.opcode = Opcode::Add;
        instr.operands = vec![nref, cref];
      } else if k < 0 {
        let cref = ir.const_new(k.wrapping_neg());
        let instr = ir.instr_mut(vref).unwrap();
        instr.opcode = Opcode::Sub;
        instr.operands = vec![nref, cref];
      } else {
        let instr = ir.instr_mut(vref).unwrap();
        instr.opcode = Opcode::Ref;
        instr.operands = vec![nref];
      }
    }
  }
}

pub fn value_propagation(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      // Propagate all operands
      let mut operands = ir.instr(r).unwrap().operands.clone();
      for j in 0..operands.len() {
        operands[j] = operand_propagate(ir, operands[j]);
      }
      ir.instr_mut(r).unwrap().operands = operands;
    }
  }
}

pub fn deadcode_elimination(ir: &mut IR) {
  // Mark and Sweep DCE
  //   DCE has the same sort of problem as garbage-collection. If you implement it by
  //   removing code only when n_uses == 0, then you can never remove dead-cycles.
  //   This is the same problem as using a refcnt-based GC. By contrast, mark-and-sweep
  //   "just works"

  // First we populate the "root set" which we'll consider to be any side-effecting
  // operation. This may be a little pessimistic, but we consider it the responsibility
  // of other opt passes to "prove" that side-effects are not required and reduce them to
  // code that DCE can eliminate.

  let mut unprocessed = VecDeque::new();
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();
      if instr.opcode.has_side_effects() || (instr.attrs & Attribute::PIN) != 0 {
        unprocessed.push_back(r);
      }
    }
  }

  // Next, we build up the live-set by recursively processing the deps
  // of any live ref.. adding each to the liveset until we're done
  let mut live_refs = HashSet::new();
  while let Some(r) = unprocessed.pop_front() {
    if live_refs.get(&r).is_some() { continue; } // already processed
    live_refs.insert(r);
    // add all operands to the unprocessed lise
    if let Some(instr) = ir.instr(r) {
      for oper_ref in &instr.operands {
        unprocessed.push_back(*oper_ref);
      }
    }
  }

  // Lastly, use the live set to remove dead-code
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      if live_refs.get(&r).is_some() { continue; } // live
      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = Opcode::Nop;
      instr.operands = vec![];
    }
  }
}

/// Instructions that may be erased when a block they (transitively) depend on
/// is removed: pure data producers with no control-flow role and no PIN
/// marker. Anything else — side effects, pinned values, Phi nodes,
/// terminators — keeps the block alive instead.
fn is_nopable_consumer(instr: &Instr) -> bool {
  if instr.opcode.has_side_effects() {
    return false;
  }
  // Pinned values are live roots by design (see deadcode_elimination);
  // erasing one would violate the PIN contract.
  if (instr.attrs & Attribute::PIN) != 0 {
    return false;
  }
  // WriteArr* is not covered by has_side_effects (the same gap exists in
  // deadcode_elimination's root set) but performs an observable write; never
  // erase one. ReadArr* stays erasable, consistent with Load*.
  if matches!(instr.opcode,
    Opcode::WriteArr8 | Opcode::WriteArr16) {
    return false;
  }
  !matches!(instr.opcode,
    Opcode::Nop | Opcode::Phi |
    Opcode::Jmp | Opcode::Jne | Opcode::JmpTbl |
    Opcode::RetFar | Opcode::RetNear)
}

/// Remove `blkref` as a predecessor of `exit` at every recorded index,
/// returning the dropped Phi operands as `(phi, idx, operand)` so the repair
/// can be restored exactly (see deadblock_elimination). Indices must arrive
/// in descending order so earlier removals never shift later positions.
/// Phis shorter than the predecessor list (malformed input) are left alone.
fn repair_exit(ir: &mut IR, exit: BlockRef, idxs: &[usize]) -> Vec<(Ref, usize, Ref)> {
  let mut removed = vec![];
  for &idx in idxs {
    ir.block_mut(exit).preds.remove(idx);
    for r in ir.iter_instrs(exit) {
      let instr = ir.instr_mut(r).unwrap();
      if instr.opcode != Opcode::Phi { continue; }
      if instr.operands.len() <= idx { continue; }
      removed.push((r, idx, instr.operands.remove(idx)));
    }
  }
  removed
}

/// Exact inverse of `repair_exit`: re-insert predecessors and Phi operands in
/// ascending index order.
fn restore_exit(ir: &mut IR, blkref: BlockRef, exit: BlockRef, idxs: &[usize], mut removed: Vec<(Ref, usize, Ref)>) {
  for &idx in idxs.iter().rev() {
    ir.block_mut(exit).preds.insert(idx, blkref);
  }
  removed.sort_by_key(|(_, idx, _)| *idx);
  for (r, idx, op) in removed {
    ir.instr_mut(r).unwrap().operands.insert(idx, op);
  }
}

pub fn deadblock_elimination(ir: &mut IR) {
  // A dead block is one with no preds
  for blkref in ir.iter_blocks() {
    let blk = ir.block(blkref);
    if blkref == BlockRef(0) { continue; } // entry block is always alive
    if blk.preds.len() > 0 { continue; }

    // Snapshot the outgoing edges up front: if the sweep below decides to
    // keep the block, the exit repair is restored exactly, so successors'
    // preds/Phis never drift out of sync with the block's own edges. An exit
    // that is already gone (removed earlier in this pass) or that does not
    // list us (truncated edge, stale CFG) has nothing to repair and is
    // skipped with a diagnostic. Duplicate edges to one exit (e.g. JmpTbl)
    // share a single entry holding every predecessor index, descending, so
    // removal never shifts positions.
    let mut exits: Vec<(BlockRef, Vec<usize>)> = vec![];
    for exit in ir.block_exits(blkref) {
      if ir.blocks[exit.0].is_none() {
        eprintln!("WARN: deadblock_elimination: unreachable '{}' ({:?}) targets removed '{}' ({:?}); skipping repair",
          ir.block(blkref).name, blkref, exit.0, exit);
        continue;
      }
      if exits.iter().any(|(e, _)| *e == exit) {
        continue;
      }
      // Find all pred_idx
      let mut idxs: Vec<usize> = ir.block(exit).preds.iter().enumerate()
        .filter_map(|(i, p)| (*p == blkref).then_some(i)).collect();
      if idxs.is_empty() {
        eprintln!("WARN: deadblock_elimination: unreachable '{}' ({:?}) targets '{}' ({:?}) without a recorded edge; skipping repair",
          ir.block(blkref).name, blkref, ir.block(exit).name, exit);
        continue;
      }
      idxs.sort_by(|a, b| b.cmp(a));
      exits.push((exit, idxs));
    }
    let mut removed_phis = vec![];
    for (exit, idxs) in &exits {
      removed_phis.push(repair_exit(ir, *exit, idxs));
    }

    // Sweep any remaining references into the removed block. Pure-data
    // consumers are erased transitively (their users join the walk); if
    // anything observable still depends on the block, the block is kept and
    // diagnosed instead of violating `block_remove`'s no-references contract.
    // NOTE: single-pass and order-dependent by design: a consumer living in
    // another dead block removed later in this loop still forces a keep (a
    // missed optimization, never a correctness issue).
    let mut to_nop = vec![];
    let mut seen = HashSet::new();
    let mut worklist = ir.block_ref_users(blkref);
    let mut keep = false;
    while let Some(r) = worklist.pop() {
      if !seen.insert(r) { continue; }
      // NOTE: `instr()` panics on refs into removed blocks rather than
      // returning None, so this only skips non-instruction refs, which never
      // enter the worklist; kept as defense in depth.
      let Some(instr) = ir.instr(r) else { continue };
      if !is_nopable_consumer(instr) {
        keep = true;
        break;
      }
      to_nop.push(r);
      for b in ir.iter_blocks() {
        // Self-block references die with the block; only outside users matter.
        if b == blkref {
          continue;
        }
        for u in ir.iter_instrs(b) {
          let Some(ui) = ir.instr(u) else { continue };
          if ui.opcode == Opcode::Nop { continue; }
          if ui.operands.contains(&r) {
            worklist.push(u);
          }
        }
      }
    }
    let still_pred = ir.iter_blocks().any(|b| ir.block(b).preds.contains(&blkref));
    if keep || still_pred {
      for ((exit, idxs), removed) in exits.into_iter().zip(removed_phis) {
        restore_exit(ir, blkref, exit, &idxs, removed);
      }
      let users = ir.block_ref_users(blkref);
      eprintln!("WARN: deadblock_elimination: keeping unreachable '{}' ({:?}): still referenced by {} instruction(s) (e.g. {:?}); skipping removal",
        ir.block(blkref).name, blkref, users.len(), users.first());
      continue;
    }

    for r in &to_nop {
      let instr = ir.instr_mut(*r).unwrap();
      instr.opcode = Opcode::Nop;
      instr.operands = vec![];
    }
    ir.drop_defs_into_block(blkref);
    ir.drop_incomplete_phis_into_block(blkref);
    ir.block_remove(blkref);
  }
}

fn allow_cse(opcode: Opcode) -> bool {
  match opcode {
    Opcode::Add => true,
    Opcode::Sub => true,
    Opcode::Shl => true,
    Opcode::Shr => true,
    Opcode::UShr => true,
    Opcode::And => true,
    Opcode::Or => true,
    Opcode::Xor => true,
    Opcode::IMul => true,
    Opcode::UMul => true,
    Opcode::IDiv => true,
    Opcode::UDiv => true,
    Opcode::Neg => true,
    Opcode::SignExtTo32 => true,
    Opcode::Lower16 => true,
    Opcode::Upper16 => true,
    Opcode::Make32 => true,
    Opcode::UpdateFlags => true,
    Opcode::EqFlags => true,
    Opcode::NeqFlags => true,
    Opcode::GtFlags => true,
    Opcode::GeqFlags => true,
    Opcode::LtFlags => true,
    Opcode::LeqFlags => true,
    Opcode::UGtFlags => true,
    Opcode::UGeqFlags => true,
    Opcode::ULtFlags => true,
    Opcode::ULeqFlags => true,
    Opcode::Eq => true,
    Opcode::Neq => true,
    Opcode::Gt => true,
    Opcode::Geq => true,
    Opcode::Lt => true,
    Opcode::Leq => true,
    Opcode::UGt => true,
    Opcode::UGeq => true,
    Opcode::ULt => true,
    Opcode::ULeq => true,
    _ => false,
  }
}

pub fn common_subexpression_elimination(ir: &mut IR) {
  for b in ir.iter_blocks() {
    let mut prev = HashMap::new();
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();
      if !allow_cse(instr.opcode) { continue; }

      let prev_ref = match prev.entry(instr.clone()) {
        hash_map::Entry::Vacant(x) => {
          x.insert(r);
          continue;
        }
        hash_map::Entry::Occupied(x) => *x.get(),
      };

      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = Opcode::Ref;
      instr.operands = vec![prev_ref];
    }
  }
}

pub fn forward_store_to_load(ir: &mut IR) {
  let prev_lookup = |ir: &IR, prev_stores: &[Ref], seg, off| -> Option<Ref> {
    for store_ref in prev_stores.iter().rev() {
      let store_instr = ir.instr(*store_ref).unwrap();
      // FIXME: Need to pessimize to account for possible aliasing
      if seg == store_instr.operands[0] && off == store_instr.operands[1] {
        return Some(store_instr.operands[2]);
      }
    }
    None
  };

  for b in ir.iter_blocks() {
    // Don't forward across blocks!!
    let mut prev_stores = vec![];
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();
      if instr.opcode.is_store() {
        prev_stores.push(r);
      }
      if !instr.opcode.is_load() { continue; }
      let seg = instr.operands[0];
      let off = instr.operands[1];
      let Some(store_val) = prev_lookup(ir, &prev_stores, seg, off) else {continue };

      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = Opcode::Ref;
      instr.operands = vec![store_val];
    }
  }
}

pub fn mem_symbol_to_ref(ir: &mut IR) {
  // FIXME: Need to pessimize with escape-analysis
  // TODO: Expand the scope of this.. only handing 16-bit symbols and operations

  // Unseal all the blocks so phi nodes generate correctly
  ir.unseal_all_blocks();

  // Pass 1: Write -> Ref
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();

      // FIXME: THIS IS WRONG.. WE SHOULD USE THESE TO PROVE NON-ESCAPE... E.G. IF A STACK REFERENCE
      // IS NOT MARKED "MAY_ESCAPE" IT MIGHT STILL ESCAPE IF IT ANOTHER INSTR USES THE ADDRESS AND
      // IS MARKED "MAY_ESCAPE" ... BUT FOR NOW IT'S AN EASY WAY TO SEPERATE LOCAL VARS FROM TEMPORARY
      // PUSH/POP STACK SLOTS (WE WANT TO PESSIMIZE THE FORMER AND OPTIMIZE THE LATER). THIS BIG
      // COMMENT EXISTS TO REMIND THE FUTURE DEBUGGER OF PAST LAZINESS
      if (instr.attrs & Attribute::MAY_ESCAPE) != 0 { continue }; // don't life any memory ref that might escape

      if instr.opcode == Opcode::WriteVar16 {
        let Ref::Symbol(symref) = &instr.operands[0] else { continue };
        if symref.table() != sym::Table::Local { continue; }
        if symref.def(&ir.symbols).size != 2 { continue; }

        let name = Name::Var(symref.name(&ir.symbols));

        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = Opcode::Ref;
        instr.operands = vec![instr.operands[1]];

        // Add the def
        ir.set_var(name, b, r);
      } else if instr.opcode == Opcode::ReadVar16 {
        let Ref::Symbol(symref) = &instr.operands[0] else { continue };
        if symref.table() != sym::Table::Local { continue; }
        if symref.def(&ir.symbols).size != 2 { continue; }

        let name = Name::Var(symref.name(&ir.symbols));
        let vref = ir.get_var(name, b);

        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = Opcode::Ref;
        instr.operands = vec![vref];
      }
    }
  }

  // Re-seal all the blocks so phi nodes generate correctly
  ir.seal_all_blocks();
}

/*
From:
--------------------------------------------
  flags.10 = u16      updf       flags.8              dx.2
  t9       = u16      signf      flags.10

To:
--------------------------------------------
  flags.10 = u16      updf       flags.8              dx.2
  t9       = u16      sign       dx.2
*/
pub fn simplify_sign_conds(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();
      if instr.opcode != Opcode::SignFlags { continue; }

      // let sign_ref = instr.operands[0];
      // let sign_instr = ir.instr(sign_ref).unwrap();
      // if sign_instr.opcode != Opcode::SignFlags { continue; }

      let upd_ref = instr.operands[0];
      // Flags can be a non-instruction ref (e.g. the initial FLAGS value, which
      // a post-INT reset restores): such a sign source is unknown, not foldable.
      let Some(upd_instr) = ir.instr(upd_ref) else { continue };
      if upd_instr.opcode != Opcode::UpdateFlags { continue; }

      let lhs = upd_instr.operands[1];

      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = Opcode::Sign;
      instr.operands = vec![lhs];
    }
  }
}

pub fn simplify_branch_conds(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();

      let opcode_new = match instr.opcode {
        Opcode::EqFlags  => Opcode::Eq,
        Opcode::NeqFlags => Opcode::Neq,
        Opcode::GtFlags  => Opcode::Gt,
        Opcode::GeqFlags => Opcode::Geq,
        Opcode::LtFlags  => Opcode::Lt,
        Opcode::LeqFlags => Opcode::Leq,
        Opcode::UGtFlags  => Opcode::UGt,
        Opcode::UGeqFlags => Opcode::UGeq,
        Opcode::ULtFlags  => Opcode::ULt,
        Opcode::ULeqFlags => Opcode::ULeq,
        _ => continue,
      };

      let opcode_eq = opcode_new == Opcode::Eq || opcode_new == Opcode::Neq;
      let opcode_above = opcode_new == Opcode::UGt;
      let opcode_lt = opcode_new == Opcode::Lt;
      let opcode_ge = opcode_new == Opcode::Geq;

      let upd_ref = instr.operands[0];
      let Some(upd_instr) = ir.instr(upd_ref) else { continue };
      if upd_instr.opcode != Opcode::UpdateFlags { continue; }

      let pred_ref = upd_instr.operands[1];
      let Some(pred_instr) = ir.instr(pred_ref) else { continue };

      // ZF is set from the result being zero by the ALU ops below, so
      // je/jne after any of them is exactly a comparison against zero.
      // (Excluded: Mul/Div leave ZF undefined; Unimpl producers are unknown;
      // shifts leave flags unchanged when the count is zero, and the count
      // is usually not a nonzero constant here. Sub/And/Or keep their
      // established arms below.)
      let zf_exact_producer = matches!(pred_instr.opcode,
        Opcode::Add | Opcode::Xor | Opcode::Neg);

      // Logical ops (and/xor) additionally clear OF and CF, so signed
      // *and* unsigned relational tests are exactly comparisons vs zero
      // (with ja/jbe degenerating to jne/je since CF == 0).
      // (Or keeps its established arms below, including the Sign forms.)
      let logical_producer = matches!(pred_instr.opcode,
        Opcode::And | Opcode::Xor);

      if opcode_eq && zf_exact_producer {
        // add/xor/shl/... <v>; je/jne <tgt> is exactly <v> ==/!= 0.
        let z = ir.const_new(0);
        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = opcode_new;
        instr.operands = vec![pred_ref, z];
      }

      else if logical_producer && !opcode_eq {
        // and/xor <v>; jcc <tgt> for any relational cc: exactly vs zero.
        // Like the Sub arm below: a byte-typed producer needs the Byte width
        // so codegen sign/zero-extends correctly (a U8 0xFF is -1 as i8).
        let compare_width = match pred_instr.typ {
          Type::U8 | Type::I8 => Some(CompareWidth::Byte),
          Type::U16 | Type::I16 => Some(CompareWidth::Word),
          _ => None,
        };
        let z = ir.const_new(0);
        let rel_opcode = match opcode_new {
          Opcode::UGt => Opcode::Neq,  // ja  <=> jne (CF == 0)
          Opcode::ULeq => Opcode::Eq,  // jbe <=> je  (CF == 0)
          // UGeq (jae) is always taken and ULt (jb) never taken here;
          // leave those for the explicit UNIMPL_FLAGS marker in astgen.
          Opcode::UGeq | Opcode::ULt => continue,
          _ => opcode_new,
        };
        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = rel_opcode;
        instr.operands = vec![pred_ref, z];
        instr.compare_width = compare_width;
      }

      else if pred_instr.opcode == Opcode::Sub {
        // cmp <a>, <b>
        // jg <tgt>
        let lhs = pred_instr.operands[0];
        let rhs = pred_instr.operands[1];
        let compare_width = match pred_instr.typ {
          Type::U8 | Type::I8 => Some(CompareWidth::Byte),
          Type::U16 | Type::I16 => Some(CompareWidth::Word),
          _ => None,
        };

        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = opcode_new;
        instr.operands = vec![lhs, rhs];
        instr.compare_width = compare_width;
      }

      else if pred_instr.opcode == Opcode::And && opcode_eq {
        // test <a>, <b>
        // je <tgt>
        let z = ir.const_new(0);
        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = opcode_new;
        instr.operands = vec![pred_ref, z];
      }

      else if pred_instr.opcode == Opcode::Or && opcode_eq { //&& pred_instr.operands[0] == pred_instr.operands[1] {
        // or <a>, <b>
        // je <tgt>
        let z = ir.const_new(0);
        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = opcode_new;
        instr.operands = vec![pred_ref, z];
      }

      else if pred_instr.opcode == Opcode::Or && opcode_above { //&& pred_instr.operands[0] == pred_instr.operands[1] {
        // or <a>, <b>
        // ja <tgt>   (equivalent to "jne <tgt>" after the or)
        let z = ir.const_new(0);
        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = Opcode::Neq;
        instr.operands = vec![pred_ref, z];
      }

      else if pred_instr.opcode == Opcode::Or && opcode_lt { //&& pred_instr.operands[0] == pred_instr.operands[1] {
        // or <a>, <b>
        // jl <tgt>   (equivalent to "jump if signed")
        let z = ir.const_new(0);
        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = Opcode::Sign;
        instr.operands = vec![pred_ref, z];
      }

      else if pred_instr.opcode == Opcode::Or && opcode_ge { //&& pred_instr.operands[0] == pred_instr.operands[1] {
        // or <a>, <b>
        // jge <tgt>   (equivalent to "jump if not signed")
        let z = ir.const_new(0);
        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = Opcode::NotSign;
        instr.operands = vec![pred_ref, z];
      }
    }
  }
}

/*
From:
--------------------------------------------
  t2 = eq  t1  #0
  t3 = eq  t2  #0

To:
--------------------------------------------
  t3 = neq t1  #0

Simplifies a comparison-result test against zero. Comparison opcodes produce
0/1 values, so `(x cmp 0) == 0` is the inverse comparison and
`(x cmp 0) != 0` is the original comparison.
*/
pub fn simplify_chained_comparisons(ir: &mut IR) {
  let cmp_ops: &[(Opcode, Opcode)] = &[
    (Opcode::Eq,   Opcode::Neq),
    (Opcode::Neq,  Opcode::Eq),
    (Opcode::Gt,   Opcode::Leq),
    (Opcode::Geq,  Opcode::Lt),
    (Opcode::Lt,   Opcode::Geq),
    (Opcode::Leq,  Opcode::Gt),
    (Opcode::UGt,  Opcode::ULeq),
    (Opcode::UGeq, Opcode::ULt),
    (Opcode::ULt,  Opcode::UGeq),
    (Opcode::ULeq, Opcode::UGt),
  ];

  for b in ir.iter_blocks() {
    for r in ir.iter_instrs(b) {
      let instr = ir.instr(r).unwrap();

      let outer_op = match instr.opcode {
        Opcode::Eq  => Opcode::Eq,
        Opcode::Neq => Opcode::Neq,
        _ => continue,
      };
      if instr.operands.len() != 2 { continue; }

      let outer_rhs = instr.operands[1];
      let Some(0) = ir.const_lookup(outer_rhs) else { continue; };

      let inner_ref = instr.operands[0];
      let Some(inner_instr) = ir.instr(inner_ref) else { continue };
      // Comparisons are binary. Besides preventing out-of-bounds access, an
      // exact arity check avoids rewriting malformed IR by silently ignoring
      // extra operands.
      if inner_instr.operands.len() != 2 { continue; }

      let inner_lhs = inner_instr.operands[0];
      let inner_rhs = inner_instr.operands[1];
      let compare_width = inner_instr.compare_width;
      let Some(0) = ir.const_lookup(inner_rhs) else { continue; };

      let Some(&(_, flipped)) = cmp_ops.iter().find(|&&(op, _)| op == inner_instr.opcode) else { continue };

      let new_op = if outer_op == Opcode::Eq { flipped } else { inner_instr.opcode };
      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = new_op;
      instr.operands = vec![inner_lhs, inner_rhs];
      instr.compare_width = compare_width;
    }
  }
}

const N_OPT_PASSES: usize = 5;
pub fn optimize(ir: &mut IR) {
  deadblock_elimination(ir);
  for _ in 0..N_OPT_PASSES {
    reduce_xor(ir);
    reduce_make_32_signext_32(ir);
    reduce_upper_lower_make32(ir);
    reduce_equal_zero_32(ir);
    reduce_phi_single_ref(ir);
    reduce_phi_common_subexpr(ir);
    simplify_branch_conds(ir);
    simplify_sign_conds(ir);
    // note: reduce_trivial_or() after simplify_branch_conds() is important
    reduce_trivial_or(ir);
    simplify_chained_comparisons(ir);
    constant_folding(ir);
    stack_ptr_accumulation(ir);
    value_propagation(ir);
    common_subexpression_elimination(ir);
    value_propagation(ir);
  }
  deadcode_elimination(ir);
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::types::TypeDatabase;
  use std::rc::Rc;

  fn test_ir() -> (IR, BlockRef) {
    let mut ir = IR::new(Rc::new(TypeDatabase::new()));
    let blk = ir.add_block("entry");
    (ir, blk)
  }

  fn append_typed(ir: &mut IR, blk: BlockRef, typ: Type, opcode: Opcode, operands: Vec<Ref>) -> Ref {
    ir.block_instr_append(blk, Instr {
      typ,
      compare_width: None,
      attrs: Attribute::NONE,
      opcode,
      operands,
    })
  }

  fn append_comparison(
    ir: &mut IR,
    blk: BlockRef,
    result_type: Type,
    compare_width: Option<CompareWidth>,
    opcode: Opcode,
    lhs: Ref,
    rhs: Ref,
  ) -> Ref {
    ir.block_instr_append(blk, Instr {
      typ: result_type,
      compare_width,
      attrs: Attribute::NONE,
      opcode,
      operands: vec![lhs, rhs],
    })
  }

  fn append(ir: &mut IR, blk: BlockRef, opcode: Opcode, operands: Vec<Ref>) -> Ref {
    append_typed(ir, blk, Type::U16, opcode, operands)
  }

  #[test]
  fn chained_comparison_inverts_eq_zero_and_preserves_neq_zero() {
    let pairs = [
      (Opcode::Eq, Opcode::Neq),
      (Opcode::Neq, Opcode::Eq),
      (Opcode::Gt, Opcode::Leq),
      (Opcode::Geq, Opcode::Lt),
      (Opcode::Lt, Opcode::Geq),
      (Opcode::Leq, Opcode::Gt),
      (Opcode::UGt, Opcode::ULeq),
      (Opcode::UGeq, Opcode::ULt),
      (Opcode::ULt, Opcode::UGeq),
      (Opcode::ULeq, Opcode::UGt),
    ];

    for (inner_op, inverse_op) in pairs {
      for (outer_op, expected_op) in [(Opcode::Eq, inverse_op), (Opcode::Neq, inner_op)] {
        let (mut ir, blk) = test_ir();
        let x = ir.const_new(5);
        let zero = ir.const_new(0);
        let inner = append(&mut ir, blk, inner_op, vec![x, zero]);
        let outer = append(&mut ir, blk, outer_op, vec![inner, zero]);

        simplify_chained_comparisons(&mut ir);

        let instr = ir.instr(outer).unwrap();
        assert_eq!(instr.opcode, expected_op);
        assert_eq!(instr.operands, vec![x, zero]);
      }
    }
  }

  #[test]
  fn chained_signed_byte_comparison_preserves_width_for_constant_folding() {
    let (mut ir, blk) = test_ir();
    let lhs = ir.const_new(0x80);
    let zero = ir.const_new(0);
    let inner = append_comparison(
      &mut ir, blk, Type::U8, Some(CompareWidth::Byte), Opcode::Lt, lhs, zero,
    );
    let outer = append(&mut ir, blk, Opcode::Eq, vec![inner, zero]);

    simplify_chained_comparisons(&mut ir);

    let rewritten = ir.instr(outer).unwrap();
    assert_eq!(rewritten.opcode, Opcode::Geq);
    assert_eq!(rewritten.compare_width, Some(CompareWidth::Byte));
    assert_eq!(rewritten.operands, vec![lhs, zero]);

    constant_folding(&mut ir);

    let folded = ir.instr(outer).unwrap();
    assert_eq!(folded.opcode, Opcode::Ref);
    assert_eq!(ir.const_lookup(folded.operands[0]), Some(0));
  }

  #[test]
  fn chained_comparison_requires_zero_on_both_rhs_operands() {
    for (inner_rhs_val, outer_rhs_val) in [(1, 0), (0, 1)] {
      let (mut ir, blk) = test_ir();
      let x = ir.const_new(5);
      let inner_rhs = ir.const_new(inner_rhs_val);
      let outer_rhs = ir.const_new(outer_rhs_val);
      let inner = append(&mut ir, blk, Opcode::Eq, vec![x, inner_rhs]);
      let outer = append(&mut ir, blk, Opcode::Eq, vec![inner, outer_rhs]);

      simplify_chained_comparisons(&mut ir);

      let instr = ir.instr(outer).unwrap();
      assert_eq!(instr.opcode, Opcode::Eq);
      assert_eq!(instr.operands, vec![inner, outer_rhs]);
    }
  }

  #[test]
  fn chained_comparison_skips_malformed_inner_arity() {
    for operands_len in [0, 1, 3] {
      let (mut ir, blk) = test_ir();
      let zero = ir.const_new(0);
      let mut operands = vec![zero; operands_len];
      if operands_len >= 2 { operands[1] = zero; }
      let inner = append(&mut ir, blk, Opcode::Eq, operands);
      let outer = append(&mut ir, blk, Opcode::Eq, vec![inner, zero]);

      simplify_chained_comparisons(&mut ir);

      let instr = ir.instr(outer).unwrap();
      assert_eq!(instr.opcode, Opcode::Eq);
      assert_eq!(instr.operands, vec![inner, zero]);
    }
  }


  #[test]
  fn constant_folding_folds_word_arithmetic_and_bitwise_ops() {
    let cases = [
      (Opcode::Add, 0x7fff, 1, i16::MIN),
      (Opcode::Sub, i16::MIN, 1, i16::MAX),
      (Opcode::And, -1, 0x00ff, 0x00ff),
      (Opcode::Or,  0x0f00, 0x00f0, 0x0ff0),
      (Opcode::Xor, 0x0ff0, 0x00ff, 0x0f0f),
    ];

    for typ in [Type::U16, Type::I16] {
      for (opcode, lhs_val, rhs_val, expected) in cases {
        let (mut ir, blk) = test_ir();
        let lhs = ir.const_new(lhs_val);
        let rhs = ir.const_new(rhs_val);
        let folded = append_typed(&mut ir, blk, typ.clone(), opcode, vec![lhs, rhs]);

        constant_folding(&mut ir);

        let instr = ir.instr(folded).unwrap();
        assert_eq!(instr.opcode, Opcode::Ref);
        assert_eq!(ir.const_lookup(instr.operands[0]), Some(expected));
        assert_eq!(instr.typ, typ);
      }
    }
  }

  #[test]
  fn constant_folding_folds_signed_and_unsigned_comparisons() {
    let equality_cases = [
      (Opcode::Eq,   -1, -1, 1),
      (Opcode::Neq,  -1,  0, 1),
      (Opcode::ULt,  -1,  0, 0),
      (Opcode::ULeq,  0,  0, 1),
      (Opcode::UGt,  -1,  0, 1),
      (Opcode::UGeq,  0, -1, 0),
    ];

    for typ in [Type::U8, Type::U16] {
      for (opcode, lhs_val, rhs_val, expected) in equality_cases {
        let (mut ir, blk) = test_ir();
        let lhs = ir.const_new(lhs_val);
        let rhs = ir.const_new(rhs_val);
        let folded = append_typed(&mut ir, blk, typ.clone(), opcode, vec![lhs, rhs]);

        constant_folding(&mut ir);

        let instr = ir.instr(folded).unwrap();
        assert_eq!(instr.opcode, Opcode::Ref);
        assert_eq!(ir.const_lookup(instr.operands[0]), Some(expected));
        assert_eq!(instr.typ, typ);
      }
    }

    let signed_byte_cases = [
      (Opcode::Lt, 0x80, 0x7f, 1),
      (Opcode::Leq, -1, 0, 1),
      (Opcode::Gt, 0x7f, 0xff, 1),
      (Opcode::Geq, -128, 0xff, 0),
    ];
    for (opcode, lhs_val, rhs_val, expected) in signed_byte_cases {
      let (mut ir, blk) = test_ir();
      let lhs = ir.const_new(lhs_val);
      let rhs = ir.const_new(rhs_val);
      let folded = append_comparison(
        &mut ir, blk, Type::U8, Some(CompareWidth::Byte), opcode, lhs, rhs,
      );

      constant_folding(&mut ir);

      let instr = ir.instr(folded).unwrap();
      assert_eq!(instr.opcode, Opcode::Ref);
      assert_eq!(ir.const_lookup(instr.operands[0]), Some(expected));
      assert_eq!(instr.typ, Type::U8);
    }

    let signed_word_cases = [
      (Opcode::Lt, -1, 0, 1),
      (Opcode::Leq, 0, 0, 1),
      (Opcode::Gt, -1, 0, 0),
      (Opcode::Geq, 0, -1, 1),
    ];
    for result_type in [Type::U8, Type::U16] {
      for (opcode, lhs_val, rhs_val, expected) in signed_word_cases {
        let (mut ir, blk) = test_ir();
        let lhs = ir.const_new(lhs_val);
        let rhs = ir.const_new(rhs_val);
        let folded = append_comparison(
          &mut ir,
          blk,
          result_type.clone(),
          Some(CompareWidth::Word),
          opcode,
          lhs,
          rhs,
        );

        constant_folding(&mut ir);

        let instr = ir.instr(folded).unwrap();
        assert_eq!(instr.opcode, Opcode::Ref);
        assert_eq!(ir.const_lookup(instr.operands[0]), Some(expected));
        assert_eq!(instr.typ, result_type);
      }
    }
  }

  #[test]
  fn constant_folding_leaves_signed_comparisons_with_unknown_width_unfolded() {
    let (mut ir, blk) = test_ir();
    let lhs = ir.const_new(-1);
    let rhs = ir.const_new(0);
    let cmp = append_comparison(&mut ir, blk, Type::U16, None, Opcode::Lt, lhs, rhs);

    constant_folding(&mut ir);

    let instr = ir.instr(cmp).unwrap();
    assert_eq!(instr.opcode, Opcode::Lt);
    assert_eq!(instr.operands, vec![lhs, rhs]);
  }

  #[test]
  fn simplify_branch_conditions_preserves_compare_operand_width() {
    for (operand_type, expected_width) in [
      (Type::U8, CompareWidth::Byte),
      (Type::I8, CompareWidth::Byte),
      (Type::U16, CompareWidth::Word),
      (Type::I16, CompareWidth::Word),
    ] {
      for result_type in [Type::U8, Type::U16] {
        let (mut ir, blk) = test_ir();
        let lhs = ir.const_new(0x80);
        let rhs = ir.const_new(0x7f);
        let sub = append_typed(&mut ir, blk, operand_type.clone(), Opcode::Sub, vec![lhs, rhs]);
        let flags = Ref::Init(crate::asm::instr::Reg::FLAGS);
        let update = append_typed(&mut ir, blk, Type::U16, Opcode::UpdateFlags, vec![flags, sub]);
        let condition = append_typed(&mut ir, blk, result_type.clone(), Opcode::LtFlags, vec![update]);

        simplify_branch_conds(&mut ir);

        let instr = ir.instr(condition).unwrap();
        assert_eq!(instr.opcode, Opcode::Lt);
        assert_eq!(instr.compare_width, Some(expected_width));
        assert_eq!(instr.typ, result_type);
        assert_eq!(instr.operands, vec![lhs, rhs]);
      }
    }

    let (mut ir, blk) = test_ir();
    let lhs = ir.const_new(1);
    let rhs = ir.const_new(2);
    let sub = append_typed(&mut ir, blk, Type::Unknown, Opcode::Sub, vec![lhs, rhs]);
    let flags = Ref::Init(crate::asm::instr::Reg::FLAGS);
    let update = append_typed(&mut ir, blk, Type::U16, Opcode::UpdateFlags, vec![flags, sub]);
    let condition = append_typed(&mut ir, blk, Type::U16, Opcode::LtFlags, vec![update]);

    simplify_branch_conds(&mut ir);

    assert_eq!(ir.instr(condition).unwrap().compare_width, None);
  }

  #[test]
  fn constant_folding_skips_width_sensitive_arithmetic() {
    for typ in [Type::U8, Type::I8, Type::U32, Type::I32, Type::Unknown] {
      let (mut ir, blk) = test_ir();
      let lhs = ir.const_new(1);
      let rhs = ir.const_new(2);
      let op = append_typed(&mut ir, blk, typ.clone(), Opcode::Add, vec![lhs, rhs]);

      constant_folding(&mut ir);

      let instr = ir.instr(op).unwrap();
      assert_eq!(instr.opcode, Opcode::Add);
      assert_eq!(instr.typ, typ);
    }
  }

  #[test]
  fn constant_folding_leaves_shifts_and_multiplication_for_separate_width_rules() {
    for opcode in [Opcode::Shl, Opcode::Shr, Opcode::UShr, Opcode::IMul, Opcode::UMul] {
      let (mut ir, blk) = test_ir();
      let lhs = ir.const_new(3);
      let rhs = ir.const_new(2);
      let op = append(&mut ir, blk, opcode, vec![lhs, rhs]);

      constant_folding(&mut ir);

      assert_eq!(ir.instr(op).unwrap().opcode, opcode);
    }
  }

  #[test]
  fn constant_folding_requires_exact_binary_arity() {
    for operands_len in [0, 1, 3] {
      let (mut ir, blk) = test_ir();
      let zero = ir.const_new(0);
      let op = append(&mut ir, blk, Opcode::Add, vec![zero; operands_len]);

      constant_folding(&mut ir);

      assert_eq!(ir.instr(op).unwrap().opcode, Opcode::Add);
    }
  }

  #[test]
  fn chained_comparison_skips_non_comparison_inner_opcode() {
    let (mut ir, blk) = test_ir();
    let x = ir.const_new(5);
    let zero = ir.const_new(0);
    let inner = append(&mut ir, blk, Opcode::Add, vec![x, zero]);
    let outer = append(&mut ir, blk, Opcode::Eq, vec![inner, zero]);

    simplify_chained_comparisons(&mut ir);

    let instr = ir.instr(outer).unwrap();
    assert_eq!(instr.opcode, Opcode::Eq);
    assert_eq!(instr.operands, vec![inner, zero]);
  }

  #[test]
  fn unimpl_marker_survives_dead_code_elimination() {
    // A standalone UNIMPL marker (e.g. from `stc`, which touches CF) has no
    // consumers by construction. It must survive DCE so the imprecision
    // reaches the generated C; otherwise the gap is silently erased.
    let (mut ir, blk) = test_ir();
    let marker = append_typed(&mut ir, blk, Type::Void, Opcode::Unimpl, vec![]);
    let one = ir.const_new(1);
    let two = ir.const_new(2);
    let dead = append(&mut ir, blk, Opcode::Add, vec![one, two]);

    deadcode_elimination(&mut ir);

    assert_eq!(ir.instr(marker).unwrap().opcode, Opcode::Unimpl);
    assert_eq!(ir.instr(dead).unwrap().opcode, Opcode::Nop);
  }

  // Builds `producer -> UpdateFlags -> cond` and returns (producer, cond).
  fn flags_over(
    ir: &mut IR,
    blk: BlockRef,
    producer: Opcode,
    cond: Opcode,
  ) -> (Ref, Ref) {
    let lhs = ir.const_new(0x00ff);
    let rhs = ir.const_new(0x0f0f);
    let operands = match producer {
      Opcode::Neg => vec![lhs],
      _ => vec![lhs, rhs],
    };
    let prod = append_typed(ir, blk, Type::U16, producer, operands);
    let flags = Ref::Init(crate::asm::instr::Reg::FLAGS);
    let update = append_typed(ir, blk, Type::U16, Opcode::UpdateFlags, vec![flags, prod]);
    let condition = append_typed(ir, blk, Type::U16, cond, vec![update]);
    (prod, condition)
  }

  #[test]
  fn zf_exact_producers_simplify_equality_tests_against_zero() {
    // add/xor/neg set ZF from the result being zero, so a following je/jne
    // is exactly a comparison of the result against zero.
    for producer in [Opcode::Add, Opcode::Xor, Opcode::Neg] {
      for (cond, expected) in [(Opcode::EqFlags, Opcode::Eq), (Opcode::NeqFlags, Opcode::Neq)] {
        let (mut ir, blk) = test_ir();
        let (prod, condition) = flags_over(&mut ir, blk, producer, cond);

        simplify_branch_conds(&mut ir);

        let instr = ir.instr(condition).unwrap();
        assert_eq!(instr.opcode, expected, "{producer:?} producer with {cond:?}");
        assert_eq!(ir.const_lookup(instr.operands[1]), Some(0), "rhs must be the zero constant");
        assert_eq!(instr.operands[0], prod, "lhs must be the producer value");
      }
    }
  }

  #[test]
  fn shifts_are_not_zf_exact_producers() {
    // A shift with a zero count leaves flags unchanged (Intel), so je/jne
    // after a variable-count shift is not provably a test against zero and
    // must stay a UNIMPL_FLAGS marker instead of being rewritten.
    for producer in [Opcode::Shl, Opcode::Shr, Opcode::UShr] {
      let (mut ir, blk) = test_ir();
      let (_prod, condition) = flags_over(&mut ir, blk, producer, Opcode::EqFlags);

      simplify_branch_conds(&mut ir);

      assert_eq!(ir.instr(condition).unwrap().opcode, Opcode::EqFlags, "{producer:?} producer");
    }
  }

  #[test]
  fn logical_relational_keeps_byte_producer_width() {
    // A byte-typed `and` (e.g. `and byte ptr [si], 0xF`) followed by a signed
    // relational jump must carry the Byte width, or codegen zero-extends a
    // U8 0xFF to 255 instead of sign-extending to -1.
    let (mut ir, blk) = test_ir();
    let lhs = ir.const_new(0x00ff);
    let rhs = ir.const_new(0x0f0f);
    let prod = append_typed(&mut ir, blk, Type::U8, Opcode::And, vec![lhs, rhs]);
    let flags = Ref::Init(crate::asm::instr::Reg::FLAGS);
    let update = append_typed(&mut ir, blk, Type::U16, Opcode::UpdateFlags, vec![flags, prod]);
    let condition = append_typed(&mut ir, blk, Type::U16, Opcode::LtFlags, vec![update]);

    simplify_branch_conds(&mut ir);

    let instr = ir.instr(condition).unwrap();
    assert_eq!(instr.opcode, Opcode::Lt);
    assert_eq!(instr.compare_width, Some(CompareWidth::Byte));
    assert_eq!(instr.operands[0], prod);
    assert_eq!(ir.const_lookup(instr.operands[1]), Some(0));
  }

  #[test]
  fn logical_producers_simplify_relational_tests_against_zero() {
    // and/xor clear OF and CF in addition to setting ZF, so relational tests
    // become comparisons against zero, with ja/jbe degenerating to jne/je.
    for producer in [Opcode::And, Opcode::Xor] {
      for (cond, expected) in [
        (Opcode::UGtFlags, Opcode::Neq),  // ja  <=> jne (CF == 0)
        (Opcode::ULeqFlags, Opcode::Eq),  // jbe <=> je  (CF == 0)
        (Opcode::LtFlags, Opcode::Lt),    // signed forms pass through
        (Opcode::GeqFlags, Opcode::Geq),
      ] {
        let (mut ir, blk) = test_ir();
        let (prod, condition) = flags_over(&mut ir, blk, producer, cond);

        simplify_branch_conds(&mut ir);

        let instr = ir.instr(condition).unwrap();
        assert_eq!(instr.opcode, expected, "{producer:?} producer with {cond:?}");
        assert_eq!(instr.operands[0], prod, "lhs must be the producer value");
        assert_eq!(ir.const_lookup(instr.operands[1]), Some(0), "rhs must be the zero constant");
      }
    }
  }

  #[test]
  fn logical_producers_leave_always_or_never_taken_tests_marked() {
    // jae is always taken and jb never taken after a logical op (CF == 0).
    // These stay as UNIMPL_FLAGS markers rather than being mis-simplified.
    for cond in [Opcode::UGeqFlags, Opcode::ULtFlags] {
      for producer in [Opcode::And, Opcode::Xor] {
        let (mut ir, blk) = test_ir();
        let (_prod, condition) = flags_over(&mut ir, blk, producer, cond);

        simplify_branch_conds(&mut ir);

        assert_eq!(ir.instr(condition).unwrap().opcode, cond, "{producer:?} producer with {cond:?}");
      }
    }
  }

  #[test]
  fn mul_and_div_are_not_zf_exact_producers() {
    // Mul/Div leave ZF undefined, so equality tests after them must stay as
    // UNIMPL_FLAGS markers instead of being rewritten against zero.
    for producer in [Opcode::IMul, Opcode::UMul, Opcode::IDiv, Opcode::UDiv] {
      let (mut ir, blk) = test_ir();
      let (_prod, condition) = flags_over(&mut ir, blk, producer, Opcode::EqFlags);

      simplify_branch_conds(&mut ir);

      assert_eq!(ir.instr(condition).unwrap().opcode, Opcode::EqFlags, "{producer:?} producer");
    }
  }

  #[test]
  fn sign_cond_on_initial_flags_is_not_folded() {
    // Flags can be a non-instruction ref: the initial FLAGS value, which a
    // post-INT reset restores (issue #66). A `js` over such a source is
    // unknown, so it must stay a SignFlags marker rather than panicking on
    // the removed UpdateFlags assumption.
    let (mut ir, blk) = test_ir();
    let condition = append_typed(
      &mut ir, blk, Type::U16, Opcode::SignFlags, vec![Ref::Init(crate::asm::instr::Reg::FLAGS)]);

    simplify_sign_conds(&mut ir);

    assert_eq!(ir.instr(condition).unwrap().opcode, Opcode::SignFlags);
  }

  // Builds the issue #64 fixture: a live pure-data consumer holding an
  // operand into a block with zero preds. Returns (ir, entry, dead, consumer).
  fn dangling_ref_fixture() -> (IR, BlockRef, BlockRef, Ref) {
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // never targeted: zero preds
    let seven = ir.const_new(7);
    let dead_val = append_typed(&mut ir, dead, Type::U16, Opcode::Add, vec![seven, seven]);
    let k = ir.const_new(4);
    let consumer = ir.block_instr_append(entry, Instr {
      typ: Type::U16,
      compare_width: None,
      attrs: Attribute::STACK_PTR,
      opcode: Opcode::Add,
      operands: vec![dead_val, k],
    });
    (ir, entry, dead, consumer)
  }

  #[test]
  fn deadblock_elimination_repairs_dangling_data_refs() {
    // Issue #64: a live pure-data consumer of a removed block must be erased,
    // not left dangling to panic a later pass in IR::block.
    let (mut ir, _entry, dead, consumer) = dangling_ref_fixture();

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_none(), "dead block must have been removed");
    assert_eq!(ir.instr(consumer).unwrap().opcode, Opcode::Nop,
      "consumer of the removed block must be erased");

    // The exact issue backtrace: stack_ptr_accumulation dereferenced the
    // dangling ref through IR::instr -> IR::block.
    stack_ptr_accumulation(&mut ir);
  }

  #[test]
  fn deadblock_dangling_ref_survives_full_optimize() {
    let (mut ir, _entry, dead, _consumer) = dangling_ref_fixture();

    optimize(&mut ir);

    assert!(ir.blocks[dead.0].is_none(), "dead block must have been removed");
  }

  #[test]
  fn deadblock_keeps_block_with_side_effecting_consumer() {
    // A store consuming a dead block's value is observable: the block must be
    // kept (with a WARN) rather than removed, leaving no dangling refs.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    let dead_val = append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let seg = Ref::Init(crate::asm::instr::Reg::SS);
    let off = ir.const_new(0x10);
    let store = append_typed(&mut ir, entry, Type::Void, Opcode::Store16, vec![seg, off, dead_val]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_some(), "referenced dead block must be kept");
    assert_eq!(ir.instr(store).unwrap().opcode, Opcode::Store16);
    // Kept blocks leave valid refs behind for later passes.
    stack_ptr_accumulation(&mut ir);
    optimize(&mut ir);
  }

  #[test]
  fn deadblock_keeps_block_with_branch_consumer() {
    // A branch condition consuming a dead block's value is observable.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    let dead_val = append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let taken = ir.add_block("taken");
    let fall = ir.add_block("fall");
    ir.block_mut(taken).preds.push(entry);
    ir.block_mut(fall).preds.push(entry);
    let jne = append_typed(&mut ir, entry, Type::Void, Opcode::Jne,
      vec![dead_val, Ref::Block(taken), Ref::Block(fall)]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_some(), "referenced dead block must be kept");
    assert_eq!(ir.instr(jne).unwrap().opcode, Opcode::Jne);
    optimize(&mut ir);
  }

  #[test]
  fn deadblock_drops_var_mappings_into_removed_block() {
    // A var mapping resolving into a removed block must be dropped so get_var
    // can never return a dangling ref.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    let dead_val = append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    ir.set_var(crate::asm::instr::Reg::AX, entry, dead_val);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_none(), "dead block must have been removed");
    assert!(!ir.block(entry).defs.values().any(|v| *v == dead_val),
      "mapping into the removed block must be dropped");
  }

  #[test]
  fn deadblock_erases_transitive_pure_consumers() {
    // dead -> A (pure) -> B (pure): the whole cone is erased with the block.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    let dead_val = append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let k = ir.const_new(4);
    let a = append(&mut ir, entry, Opcode::Add, vec![dead_val, k]);
    let b = append(&mut ir, entry, Opcode::Xor, vec![a, a]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_none(), "dead block must have been removed");
    assert_eq!(ir.instr(a).unwrap().opcode, Opcode::Nop, "direct consumer must be erased");
    assert_eq!(ir.instr(b).unwrap().opcode, Opcode::Nop, "transitive consumer must be erased");
    optimize(&mut ir);
  }

  #[test]
  fn deadblock_keep_path_restores_exit_repair() {

    // A kept block must leave its successors' preds/Phis exactly as found:
    // the exit repair is restored, not left half-applied.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    let dead_val = append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let target = ir.add_block("target");
    ir.block_mut(target).preds.push(dead);
    append_typed(&mut ir, dead, Type::Void, Opcode::Jmp, vec![Ref::Block(target)]);
    let seg = Ref::Init(crate::asm::instr::Reg::SS);
    let off = ir.const_new(0x10);
    append_typed(&mut ir, entry, Type::Void, Opcode::Store16, vec![seg, off, dead_val]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_some(), "referenced dead block must be kept");
    assert_eq!(ir.block(target).preds, vec![dead],
      "exit repair must be restored on the keep path");
    optimize(&mut ir);
  }

  #[test]
  fn deadblock_keeps_block_with_transitive_side_effecting_consumer() {
    // dead -> A (pure) -> B (store): the observable tail keeps the block.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    let dead_val = append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let k = ir.const_new(4);
    let a = append(&mut ir, entry, Opcode::Add, vec![dead_val, k]);
    let seg = Ref::Init(crate::asm::instr::Reg::SS);
    let off = ir.const_new(0x10);
    let store = append_typed(&mut ir, entry, Type::Void, Opcode::Store16, vec![seg, off, a]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_some(), "referenced dead block must be kept");
    assert_eq!(ir.instr(a).unwrap().opcode, Opcode::Add, "nothing may be erased on the keep path");
    assert_eq!(ir.instr(store).unwrap().opcode, Opcode::Store16);
    optimize(&mut ir);
  }

  #[test]
  fn deadblock_removes_self_referencing_block() {
    // References from inside the dead block itself die with the block and
    // must not force a keep: a self-looping unreachable block is removed.
    let (mut ir, _entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    append_typed(&mut ir, dead, Type::Void, Opcode::Jmp, vec![Ref::Block(dead)]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_none(), "self-referencing dead block must be removed");
    optimize(&mut ir);
  }

  #[test]
  fn deadblock_skips_exit_without_recorded_edge() {
    // A dead block jumping to a block that does not list it as a predecessor
    // (truncated edge, stale CFG) must not panic on the missing pred index.
    let (mut ir, _entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let ghost = ir.add_block("ghost"); // no preds recorded
    append_typed(&mut ir, dead, Type::Void, Opcode::Jmp, vec![Ref::Block(ghost)]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_none(), "dead block must be removed");
    optimize(&mut ir);
  }

  #[test]
  fn deadblock_skips_short_phi_operands() {

    // A Phi shorter than the predecessor list (malformed input) must not
    // panic the operand removal; it is left alone.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    let dead_val = append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let exit = ir.add_block("exit");
    ir.block_mut(exit).preds.push(entry);
    ir.block_mut(exit).preds.push(dead);
    let x = ir.const_new(0x11);
    let phi = append_typed(&mut ir, exit, Type::U16, Opcode::Phi, vec![x]);
    append_typed(&mut ir, dead, Type::Void, Opcode::Jmp, vec![Ref::Block(exit)]);
    // Keep dead_val referenced only by the block itself so removal proceeds.
    let _ = dead_val;

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_none(), "dead block must be removed");
    assert_eq!(ir.block(exit).preds, vec![entry]);
    assert_eq!(ir.instr(phi).unwrap().operands, vec![x], "short Phi must be left alone");
    optimize(&mut ir);
  }

  #[test]
  fn deadblock_repairs_duplicate_edges_together() {
    // Duplicate edges to one exit (e.g. JmpTbl) share a single repair entry:
    // repairing each occurrence independently would corrupt the survivor.
    // The exit itself stays alive via its other predecessor.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let exit = ir.add_block("exit");
    ir.block_mut(exit).preds.push(entry);
    ir.block_mut(exit).preds.push(dead);
    ir.block_mut(exit).preds.push(dead);
    // Phi positions mirror preds ([entry, dead, dead]), so the surviving
    // operand must be the entry's own.
    let en = ir.const_new(0x11);
    let d0 = ir.const_new(0x22);
    let d1 = ir.const_new(0x33);
    let phi = append_typed(&mut ir, exit, Type::U16, Opcode::Phi, vec![en, d0, d1]);
    let idx = ir.const_new(0);
    append_typed(&mut ir, dead, Type::Void, Opcode::JmpTbl, vec![idx, Ref::Block(exit), Ref::Block(exit)]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_none(), "dead block must be removed");
    assert_eq!(ir.block(exit).preds, vec![entry], "both duplicate preds must be repaired");
    assert_eq!(ir.instr(phi).unwrap().operands, vec![en], "both duplicate Phi operands must be repaired");
    optimize(&mut ir);
  }

  #[test]
  fn exit_repair_restore_roundtrips_interleaved_duplicates() {
    // Direct repair/restore check with interleaved preds: removal runs
    // descending so positions never shift; restore runs ascending.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead");
    let exit = ir.add_block("exit");
    ir.block_mut(exit).preds.push(dead);
    ir.block_mut(exit).preds.push(entry);
    ir.block_mut(exit).preds.push(dead);
    let d0 = ir.const_new(0x11);
    let e = ir.const_new(0x22);
    let d1 = ir.const_new(0x33);
    let phi = append_typed(&mut ir, exit, Type::U16, Opcode::Phi, vec![d0, e, d1]);

    let removed = repair_exit(&mut ir, exit, &[2, 0]);
    assert_eq!(ir.block(exit).preds, vec![entry]);
    assert_eq!(ir.instr(phi).unwrap().operands, vec![e]);
    restore_exit(&mut ir, dead, exit, &[2, 0], removed);
    assert_eq!(ir.block(exit).preds, vec![dead, entry, dead]);
    assert_eq!(ir.instr(phi).unwrap().operands, vec![d0, e, d1]);
  }

  #[test]
  fn deadblock_keeps_block_with_writearr_consumer() {
    // WriteArr performs an observable write (not covered by
    // has_side_effects): it must keep the block, like Store.
    let (mut ir, entry) = test_ir();
    let dead = ir.add_block("dead"); // zero preds
    let seven = ir.const_new(7);
    let dead_val = append(&mut ir, dead, Opcode::Add, vec![seven, seven]);
    let arr = Ref::Init(crate::asm::instr::Reg::SI);
    let off = ir.const_new(0x10);
    let write = append_typed(&mut ir, entry, Type::Void, Opcode::WriteArr16, vec![arr, off, dead_val]);

    deadblock_elimination(&mut ir);

    assert!(ir.blocks[dead.0].is_some(), "referenced dead block must be kept");
    assert_eq!(ir.instr(write).unwrap().opcode, Opcode::WriteArr16);
  }
}
