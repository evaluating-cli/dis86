use crate::decompile::ir::*;
use crate::decompile::sym;
use crate::types::Type;
use std::collections::{hash_map, HashMap, HashSet, VecDeque};

// Propagate operand through any ref opcodes
fn operand_propagate(ir: &IR, mut r: Ref) -> Ref {
  loop {
    let Some(instr) = ir.instr(r) else { return r };
    if instr.opcode != Opcode::Ref { return r; }
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

      let result = match instr.opcode {
        Opcode::Add if fold_word_arith => lhs.wrapping_add(rhs),
        Opcode::Sub if fold_word_arith => lhs.wrapping_sub(rhs),
        Opcode::And if fold_word_arith => lhs & rhs,
        Opcode::Or  if fold_word_arith => lhs | rhs,
        Opcode::Xor if fold_word_arith => lhs ^ rhs,

        Opcode::Eq   if fold_bool => if lhs == rhs { 1 } else { 0 },
        Opcode::Neq  if fold_bool => if lhs != rhs { 1 } else { 0 },
        Opcode::Lt   if fold_bool => if lhs < rhs { 1 } else { 0 },
        Opcode::Leq  if fold_bool => if lhs <= rhs { 1 } else { 0 },
        Opcode::Gt   if fold_bool => if lhs > rhs { 1 } else { 0 },
        Opcode::Geq  if fold_bool => if lhs >= rhs { 1 } else { 0 },
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
    Opcode::Add => Some((nref, ir.const_lookup(cref).unwrap())),
    Opcode::Sub => Some((nref, -ir.const_lookup(cref).unwrap())),
    _ => None,
  }
}

pub fn stack_ptr_accumulation(ir: &mut IR) {
  for b in ir.iter_blocks() {
    for vref in ir.iter_instrs(b) {
      let Some((_, a)) = stack_ptr_const_oper(ir, vref) else { continue };

      let instr = ir.instr(vref).unwrap();
      let Some((nref, b)) = stack_ptr_const_oper(ir, instr.operands[0]) else { continue };

      let k = a+b;
      if k > 0 {
        let cref = ir.const_new(k);
        let instr = ir.instr_mut(vref).unwrap();
        instr.opcode = Opcode::Add;
        instr.operands = vec![nref, cref];
      } else if k < 0 {
        let cref = ir.const_new(-k);
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

pub fn deadblock_elimination(ir: &mut IR) {
  // A dead block is one with no preds
  for blkref in ir.iter_blocks() {
    let blk = ir.block(blkref);
    if blkref == BlockRef(0) { continue; } // entry block is always alive
    if blk.preds.len() > 0 { continue; }

    // Need to remove ourself as a pred from any target blocks
    for exit in ir.block_exits(blkref) {
      // Find pred_idx
      let exit_blk = ir.block_mut(exit);
      let mut pred_idx = None;
      for (i, p) in exit_blk.preds.iter().enumerate() {
        if *p == blkref {
          pred_idx = Some(i);
          break;
        }
      }
      let pred_idx = pred_idx.unwrap();

      // Remove index from pred and all phis
      exit_blk.preds.remove(pred_idx);
      for r in ir.iter_instrs(exit) {
        let instr = ir.instr_mut(r).unwrap();
        if instr.opcode != Opcode::Phi { continue; }
        instr.operands.remove(pred_idx);
      }
    }

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
      let upd_instr = ir.instr(upd_ref).unwrap();
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
      let upd_instr = ir.instr(upd_ref).unwrap();
      if upd_instr.opcode != Opcode::UpdateFlags { continue; }

      let pred_ref = upd_instr.operands[1];
      let pred_instr = ir.instr(pred_ref).unwrap();

      if pred_instr.opcode == Opcode::Sub {
        // cmp <a>, <b>
        // jg <tgt>
        let lhs = pred_instr.operands[0];
        let rhs = pred_instr.operands[1];

        let instr = ir.instr_mut(r).unwrap();
        instr.opcode = opcode_new;
        instr.operands = vec![lhs, rhs];
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
      let Some(0) = ir.const_lookup(inner_rhs) else { continue; };

      let Some(&(_, flipped)) = cmp_ops.iter().find(|&&(op, _)| op == inner_instr.opcode) else { continue };

      let new_op = if outer_op == Opcode::Eq { flipped } else { inner_instr.opcode };
      let instr = ir.instr_mut(r).unwrap();
      instr.opcode = new_op;
      instr.operands = vec![inner_lhs, inner_rhs];
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
      attrs: Attribute::NONE,
      opcode,
      operands,
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
    let cases = [
      (Opcode::Eq,   -1, -1, 1),
      (Opcode::Neq,  -1,  0, 1),
      (Opcode::Lt,   -1,  0, 1),
      (Opcode::Leq,   0,  0, 1),
      (Opcode::Gt,   -1,  0, 0),
      (Opcode::Geq,   0, -1, 1),
      (Opcode::ULt,  -1,  0, 0),
      (Opcode::ULeq,  0,  0, 1),
      (Opcode::UGt,  -1,  0, 1),
      (Opcode::UGeq,  0, -1, 0),
    ];

    for typ in [Type::U8, Type::U16] {
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
}
