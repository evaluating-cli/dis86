use crate::decompile::sym;
use crate::decompile::ir::*;
use crate::asm::instr;
use crate::binary;
use crate::segoff::{Seg, Off, SegOff};
use crate::config::{self, Config};
use crate::spec;
use crate::types::{Type, ArraySize};
use crate::asm::intel_syntax::instr_str;
use std::collections::{HashSet, HashMap};

const DEBUG: bool = false;

fn operand_is_stack_reg(operand: &instr::Operand) -> bool {
  let instr::Operand::Reg(instr::OperandReg(reg)) = operand else { return false };
  reg == &instr::Reg::SP || reg == &instr::Reg::BP
}

/// Whether an instruction may leave the direction flag set: STD sets it
/// explicitly, and POPF/IRET restore it from a popped flags value. (The
/// latter are not lowered yet — they panic — but including them keeps the
/// string-op direction guard sound if/when they are.)
fn opcode_may_set_df(op: instr::Opcode) -> bool {
  matches!(op, instr::Opcode::OP_STD | instr::Opcode::OP_POPF | instr::Opcode::OP_IRET)
}

fn simple_binary_operation(opcode: instr::Opcode) -> Option<Opcode> {
  match opcode {
    instr::Opcode::OP_ADD => Some(Opcode::Add),
    instr::Opcode::OP_SUB => Some(Opcode::Sub),
    instr::Opcode::OP_SHL => Some(Opcode::Shl),
    instr::Opcode::OP_SAR => Some(Opcode::Shr),
    instr::Opcode::OP_SHR => Some(Opcode::UShr),
    instr::Opcode::OP_AND => Some(Opcode::And),
    instr::Opcode::OP_OR => Some(Opcode::Or),
    instr::Opcode::OP_XOR => Some(Opcode::Xor),
    _ => None,
  }
}

fn simple_unary_operation(opcode: instr::Opcode) -> Option<Opcode> {
  match opcode {
    instr::Opcode::OP_NEG => Some(Opcode::Neg),
    instr::Opcode::OP_NOT => Some(Opcode::Not),
    _ => None,
  }
}

enum SpecialState {
  PushCS, // CS register was pushed in the last instruction
}

struct IRBuilder<'a> {
  instrs: &'a [instr::Instr],
  cfg: &'a Config,
  spec: &'a spec::Spec<'a>,
  binary: &'a binary::Binary,

  ir: IR,
  addrmap: HashMap<SegOff, BlockRef>,
  cur: BlockRef,
  special: Option<SpecialState>,

  overlay: bool,
  pin_all: bool,

  // Whether the decoded function may leave the direction flag set/unknown.
  // DF is written only by CLD/STD (and by the flag-restoring POPF/IRET,
  // which are not lowered yet but must be covered when they are). With
  // none present the direction is provably clear (DF=0, the DOS ABI
  // entry/exit convention) and string-op pointer updates advance;
  // otherwise the direction is unknown and those updates degrade to a
  // loud marker rather than silently running the wrong way.
  df_may_be_set: bool,
}

impl<'a> IRBuilder<'a> {
  fn new(cfg: &'a Config, instrs: &'a [instr::Instr], spec: &'a spec::Spec, binary: &'a binary::Binary,
         overlay: bool, pin_all: bool) -> Self {
    let mut this = Self {
      instrs,
      cfg,
      spec,
      binary,

      ir: IR::new(cfg.types.clone()),
      addrmap: HashMap::new(),
      cur: BlockRef(0),
      special: None,

      overlay,
      pin_all,

      df_may_be_set: false,
    };

    // Create and seal the entry block
    let entry = this.new_block("entry");
    this.ir.seal_block(entry);

    // Set initial register values
    this.ir.set_var(instr::Reg::AX, this.cur, Ref::Init(instr::Reg::AX));
    this.ir.set_var(instr::Reg::CX, this.cur, Ref::Init(instr::Reg::CX));
    this.ir.set_var(instr::Reg::DX, this.cur, Ref::Init(instr::Reg::DX));
    this.ir.set_var(instr::Reg::BX, this.cur, Ref::Init(instr::Reg::BX));

    this.ir.set_var(instr::Reg::SP, this.cur, Ref::Init(instr::Reg::SP));
    this.ir.set_var(instr::Reg::BP, this.cur, Ref::Init(instr::Reg::BP));
    this.ir.set_var(instr::Reg::SI, this.cur, Ref::Init(instr::Reg::SI));
    this.ir.set_var(instr::Reg::DI, this.cur, Ref::Init(instr::Reg::DI));

    this.ir.set_var(instr::Reg::ES, this.cur, Ref::Init(instr::Reg::ES));
    this.ir.set_var(instr::Reg::CS, this.cur, Ref::Init(instr::Reg::CS));
    this.ir.set_var(instr::Reg::SS, this.cur, Ref::Init(instr::Reg::SS));
    this.ir.set_var(instr::Reg::DS, this.cur, Ref::Init(instr::Reg::DS));

    // Seed the byte fragments too: append_asm_dst_reg reads the sibling
    // when recomposing, and an unseeded fragment would materialize as an
    // operand-less Phi whose name is never declared in the output.
    for reg in [instr::Reg::AL, instr::Reg::AH,
                instr::Reg::BL, instr::Reg::BH,
                instr::Reg::CL, instr::Reg::CH,
                instr::Reg::DL, instr::Reg::DH] {
      this.ir.set_var(reg, this.cur, Ref::Init(reg));
    }

    this.ir.set_var(instr::Reg::IP, this.cur, Ref::Init(instr::Reg::IP));
    this.ir.set_var(instr::Reg::FLAGS, this.cur, Ref::Init(instr::Reg::FLAGS));

    this
  }

  fn new_block(&mut self, name: &str) -> BlockRef {
    self.ir.add_block(name)
  }

  fn get_block(&mut self, effective: SegOff) -> BlockRef {
    *self.addrmap.get(&effective).unwrap()
  }

  fn jump_indirect_targets(&self, ins: &instr::Instr, m: &instr::OperandMem) -> Option<Vec<SegOff>> {
    // Matching jumps of the form: "jmp WORD PTR cs:[bx+0x6d7]"
    if m.sz != instr::Size::Size16 { return None; }
    if m.sreg != instr::Reg::CS    { return None; }
    let Some(_) = m.reg1 else      { return None; };
    if !m.reg2.is_none()           { return None; }
    let Some(off) = m.off else     { return None; };

    // Construct a segoff address to the memory operand
    let addr = SegOff {
      seg: self.spec.start.seg, // code segment
      off: Off(off),
    };

    // Try to find a matching text segment region in config
    let region = self.cfg.text_region_lookup(addr, ins.addr).unwrap_or_else(
      || panic!("Failed to find text section region ({}) for: '{}' at '{}'", addr, instr_str(ins), ins.addr));

    // Unpack the array type
    let Type::Array(basetype, ArraySize::Known(len)) = &region.typ else {
      panic!("Expected text segment region to be an array of known length ({}) for: '{}'", region.name, instr_str(ins));
    };

    // Sanity check type
    if basetype.as_ref() != &Type::U16 {
      panic!("Expected text segment region to be an array with basetype u16, got ({}) for: '{}'", region.name, instr_str(ins));
    }

    // Sanity check range
    if region.start.add_offset(2 * *len as u16) != region.end {
      panic!("Expected text segment region with a length that is consistent with the region size, got ({}) for: '{}'", region.name, instr_str(ins));
    }

    // Grab the bytes to the region from the raw binary
    let dat = self.binary.region(region.start, region.end);

    // Process them into branch targets
    let mut tgts = vec![];
    for i in 0..*len {
      let off = Off(u16::from_le_bytes(dat[2*i .. 2*i+2].try_into().unwrap()));
      tgts.push(SegOff { seg: self.spec.start.seg, off });
    }

    Some(tgts)
  }

  /// Branch targets of `ins`: the explicit targets plus, for branches
  /// that fall through, the implicit fallthrough address. The two are
  /// returned separately (rather than "fallthrough is last") so callers
  /// cannot range-check the wrong subset.
  fn jump_targets(&self, ins: &instr::Instr) -> Option<(Vec<SegOff>, Option<SegOff>)> {
    // Special handling for some indirect jumps
    if ins.opcode == instr::Opcode::OP_JMP {
      if let instr::Operand::Mem(m) = &ins.operands[0] {
        if let Some(targets) = self.jump_indirect_targets(ins, m) {
          return Some((targets, None));
        }
        // If none.. we want to fall through so we hit the panic below
      }
    }

    // Filter for branch instructions
    let (oper_num, fallthrough) = match &ins.opcode {
      instr::Opcode::OP_JA   => (0, true),
      instr::Opcode::OP_JAE  => (0, true),
      instr::Opcode::OP_JB   => (0, true),
      instr::Opcode::OP_JBE  => (0, true),
      instr::Opcode::OP_JCXZ => (1, true),
      instr::Opcode::OP_JE   => (0, true),
      instr::Opcode::OP_JG   => (0, true),
      instr::Opcode::OP_JGE  => (0, true),
      instr::Opcode::OP_JL   => (0, true),
      instr::Opcode::OP_JLE  => (0, true),
      instr::Opcode::OP_JMP  => (0, false),
      instr::Opcode::OP_JMPF => (0, false),
      instr::Opcode::OP_JNE  => (0, true),
      instr::Opcode::OP_JNO  => (0, true),
      instr::Opcode::OP_JNP  => (0, true),
      instr::Opcode::OP_JNS  => (0, true),
      instr::Opcode::OP_JO   => (0, true),
      instr::Opcode::OP_JP   => (0, true),
      instr::Opcode::OP_JS   => (0, true),
      instr::Opcode::OP_LOOP => (1, true),
      instr::Opcode::OP_LOOPE => (1, true),
      instr::Opcode::OP_LOOPNE => (1, true),
      _ => return None,
    };

    let mut targets = vec![];

    match &ins.operands[oper_num] {
      instr::Operand::Rel(rel) => {
        targets.push(ins.rel_addr(rel));
      }
      _ => panic!("Unsupported branch instruction: '{}' | {:?}", instr_str(ins), ins.operands[oper_num]),
    };

    let fallthrough = fallthrough.then(|| ins.end_addr());

    Some((targets, fallthrough))
  }

  fn append_instr(&mut self, typ: Type, opcode: Opcode, operands: Vec<Ref>) -> Ref {
    self.append_instr_with_attrs(typ, Attribute::NONE, opcode, operands)
  }

  fn append_instr_with_attrs(&mut self, typ: Type, attrs: u8, opcode: Opcode, operands: Vec<Ref>) -> Ref {
    let instr = Instr {
      typ,
      compare_width: None,
      attrs,
      opcode,
      operands,
    };

    self.ir.block_instr_append(self.cur, instr)
  }

  fn append_jmp(&mut self, next: BlockRef) {
    self.ir.block_mut(next).preds.push(self.cur);
    self.append_instr(Type::Void, Opcode::Jmp, vec![Ref::Block(next)]);
  }

  fn append_jne(&mut self, cond: Ref, true_blk: BlockRef, false_blk: BlockRef) {
    self.ir.block_mut(true_blk).preds.push(self.cur);
    self.ir.block_mut(false_blk).preds.push(self.cur);
    self.append_instr(Type::Void, Opcode::Jne, vec![
      cond,
      Ref::Block(true_blk),
      Ref::Block(false_blk)]);
  }

  fn append_jmptbl(&mut self, reg_ref: Ref, targets: Vec<SegOff>) {
    // NOTE: The reg value will have been scaled up to do the memory access, we need to "de-scale" it
    // This is technically not gaurenteed to be safe so we insert some "asserts"
    self.append_instr(Type::Void, Opcode::AssertPos, vec![reg_ref]);
    self.append_instr(Type::Void, Opcode::AssertEven, vec![reg_ref]);
    // Then scale it down..
    let k = self.ir.const_new(1);
    let typ = self.deduce_type_binary(reg_ref, k);
    let idx = self.append_instr(typ, Opcode::UShr, vec![reg_ref, k]);
    let mut opers = vec![idx];
    for tgt in targets {
      let blkref = self.get_block(tgt);
      self.ir.block_mut(blkref).preds.push(self.cur);
      opers.push(Ref::Block(blkref));
    }
    self.append_instr(Type::Void, Opcode::JmpTbl, opers);
  }

  fn switch_blk(&mut self, bref: BlockRef) {
    self.cur = bref;
  }

  /// Whether a branch target lies inside the function's decoded
  /// `[start, end)` range. Anything outside (including another segment)
  /// decodes to zero instructions and is dropped from the output.
  fn target_in_range(&self, tgt: SegOff) -> bool {
    tgt.seg == self.spec.start.seg && tgt.off >= self.spec.start.off && tgt.off < self.spec.end.off
  }

  /// Whether an explicit branch target deserves the out-of-range WARN:
  /// outside `[start, end)` and not the branch's own fallthrough (a
  /// branch-to-next like `jz $+2` links both arms to the existing
  /// fallthrough block, so no edge is dropped). Pure so tests can pin
  /// the warn/skip matrix without capturing stderr.
  fn out_of_range_edge(&self, tgt: SegOff, fallthrough: Option<SegOff>) -> bool {
    Some(tgt) != fallthrough && !self.target_in_range(tgt)
  }

  fn start_next_blk(&mut self, next: SegOff) {
    let next_bref = self.get_block(next);

    // Make sure the last instruction is a jump or ret
    match self.ir.block_last_instr(self.cur).map(|ins| ins.opcode) {
      Some(Opcode::Jmp) => (),
      Some(Opcode::Jne) => (),
      Some(Opcode::JmpTbl) => (),
      Some(Opcode::RetFar) => (),
      Some(Opcode::RetNear) => (),
      _ => self.append_jmp(next_bref), // need to append a trailing jump
    }

    // Switch to the next block
    self.switch_blk(next_bref);
    assert!(self.ir.block_instr_count(self.cur) == 0);
  }
}

/////////////////////////////////////////////////////////////////////////////////////

impl IRBuilder<'_> {
  fn append_asm_src_reg(&mut self, reg: &instr::OperandReg) -> Ref {
    self.ir.get_var(reg.0, self.cur)
  }

  fn append_asm_dst_reg(&mut self, reg: &instr::OperandReg, mut vref: Ref) {
    if self.pin_all {
      vref = self.append_instr_with_attrs(Type::U16, Attribute::PIN, Opcode::Ref, vec![vref]);
    }
    self.ir.set_var(reg.0, self.cur, vref);
    // Keep overlapping registers coherent: partial writes recompose the
    // full register and full writes split into the fragments. Without
    // this, a fragment read after an overlapping write sees a stale
    // version (e.g. `mov al,[bx]` ... `call toupper` ... `mov [bx],al`
    // must store the call result). BX/SI/DI/BP have no
    // fragments: nothing more to do.
    let (full, lo, hi) = match reg.0 {
      instr::Reg::AL | instr::Reg::AH => (instr::Reg::AX, instr::Reg::AL, instr::Reg::AH),
      instr::Reg::BL | instr::Reg::BH => (instr::Reg::BX, instr::Reg::BL, instr::Reg::BH),
      instr::Reg::CL | instr::Reg::CH => (instr::Reg::CX, instr::Reg::CL, instr::Reg::CH),
      instr::Reg::DL | instr::Reg::DH => (instr::Reg::DX, instr::Reg::DL, instr::Reg::DH),
      instr::Reg::AX | instr::Reg::BX | instr::Reg::CX | instr::Reg::DX => {
        let cur = self.ir.get_var(reg.0, self.cur);
        let (upper, lower) = self.append_upper_lower_split(cur);
        let (f_lo, f_hi) = match reg.0 {
          instr::Reg::AX => (instr::Reg::AL, instr::Reg::AH),
          instr::Reg::BX => (instr::Reg::BL, instr::Reg::BH),
          instr::Reg::CX => (instr::Reg::CL, instr::Reg::CH),
          _ => (instr::Reg::DL, instr::Reg::DH),
        };
        self.ir.set_var(f_lo, self.cur, lower);
        self.ir.set_var(f_hi, self.cur, upper);
        return;
      }
      _ => return,
    };
    // An 8-bit destination only keeps the low byte: a 16-bit source moved
    // into a fragment would otherwise leak its high byte into the pair's
    // high byte through the Or below. Mask at the write so every fragment
    // value reaching the recompose is genuinely 8 bits wide.
    let mask = self.ir.const_new(0xFF);
    let masked = self.append_instr(Type::U16, Opcode::And, vec![vref, mask]);
    self.ir.set_var(reg.0, self.cur, masked);
    let lo_v = self.ir.get_var(lo, self.cur);
    let hi_v = self.ir.get_var(hi, self.cur);
    let (use_lo, use_hi) = if reg.0 == lo { (masked, hi_v) } else { (lo_v, masked) };
    let eight = self.ir.const_new(8);
    let shifted = self.append_instr(Type::U16, Opcode::Shl, vec![use_hi, eight]);
    let composed = self.append_instr(Type::U16, Opcode::Or, vec![shifted, use_lo]);
    self.ir.set_var(full, self.cur, composed);
  }

  fn compute_mem_address(&mut self, mem: &instr::OperandMem) -> Ref {
    let mut refs = vec![];
    if let Some(reg) = mem.reg2 {
      refs.push(self.ir.get_var(reg, self.cur));
    }
    if let Some(reg) = mem.reg1 {
      refs.push(self.ir.get_var(reg, self.cur));
    }
    if let Some(off) = mem.off {
      refs.push(self.ir.const_new(off as i16));
    }

    let attr = if mem.sreg == instr::Reg::SS { Attribute::STACK_PTR } else { Attribute::NONE };

    if refs.len() == 1 {
      refs[0]
    } else if refs.len() == 2 {
      let typ = self.deduce_type_binary(refs[0], refs[1]);
      self.append_instr_with_attrs(typ, attr, Opcode::Add, vec![refs[0], refs[1]])
    } else {
      assert!(refs.len() == 3);
      let typ = self.deduce_type_binary(refs[0], refs[1]);
      let lhs = self.append_instr_with_attrs(typ, attr, Opcode::Add, vec![refs[0], refs[1]]);
      let typ = self.deduce_type_binary(lhs, refs[2]);
      self.append_instr_with_attrs(typ, attr, Opcode::Add, vec![lhs, refs[2]])
    }
  }

  fn append_asm_src_mem(&mut self, mem: &instr::OperandMem) -> Ref {
    let addr = self.compute_mem_address(mem);
    let seg = self.ir.get_var(mem.sreg, self.cur);

    let (typ, opcode) = match mem.sz {
      instr::Size::Size8 => (Type::U8, Opcode::Load8),
      instr::Size::Size16 => (Type::U16, Opcode::Load16),
      instr::Size::Size32 => (Type::U32, Opcode::Load32),
    };
    self.append_instr_with_attrs(typ, Attribute::MAY_ESCAPE, opcode, vec![seg, addr])
  }

  fn append_asm_dst_mem(&mut self, mem: &instr::OperandMem, vref: Ref) {
    let addr = self.compute_mem_address(mem);
    let seg = self.ir.get_var(mem.sreg, self.cur);

    let opcode = match mem.sz {
      instr::Size::Size8 => Opcode::Store8,
      instr::Size::Size16 => Opcode::Store16,
      _ => panic!("32-bit stores not supported"),
    };

    let mut attr = Attribute::MAY_ESCAPE;
    if self.pin_all {
      attr |= Attribute::PIN;
    }
    self.append_instr_with_attrs(Type::Void, attr, opcode, vec![seg, addr, vref]);
  }

  fn append_asm_src_imm(&mut self, imm: &instr::OperandImm) -> Ref {
    // TODO: Is it okay to sign-ext away the size here??
    // let k: i32 = match imm.sz {
    //   instr::Size::Size8 => (imm.val as i8).into(),
    //   instr::Size::Size16 => (imm.val as i16).into(),
    //   _ => panic!("32-bit immediates not supported"),
    // };
    self.ir.const_new(imm.val as i16)
  }

  fn append_asm_src_rel(&mut self, _rel: &instr::OperandRel) -> Ref {
    panic!("unimpl");
  }

  fn append_asm_src_far(&mut self, _far: &instr::OperandFar) -> Ref {
    panic!("unimpl");
  }

  fn append_asm_src_operand(&mut self, oper: &instr::Operand) -> Ref {
    match oper {
      instr::Operand::Reg(reg) => self.append_asm_src_reg(reg),
      instr::Operand::Mem(mem) => self.append_asm_src_mem(mem),
      instr::Operand::Imm(imm) => self.append_asm_src_imm(imm),
      instr::Operand::Rel(rel) => self.append_asm_src_rel(rel),
      instr::Operand::Far(far) => self.append_asm_src_far(far),
    }
  }

  fn append_asm_dst_operand(&mut self, oper: &instr::Operand, vref: Ref) {
    match oper {
      instr::Operand::Reg(reg) => self.append_asm_dst_reg(reg, vref),
      instr::Operand::Mem(mem) => self.append_asm_dst_mem(mem, vref),
      instr::Operand::Imm(_)   => panic!("Should never have a destination imm"),
      instr::Operand::Rel(_)   => panic!("Should never have a destination rel"),
      instr::Operand::Far(_)   => panic!("Should never have a destination far"),
    };
  }

  fn get_flags(&mut self) -> Ref {
    self.ir.get_var(instr::Reg::FLAGS, self.cur)
  }

  fn set_flags(&mut self, vref: Ref) {
    self.ir.set_var(instr::Reg::FLAGS, self.cur, vref);
  }

  fn append_update_flags(&mut self, vref: Ref) {
    let old_flags = self.get_flags();
    let new_flags = self.append_instr(Type::U16, Opcode::UpdateFlags, vec![old_flags, vref]);
    self.set_flags(new_flags);
  }

  /// String-op pointer auto-update (`SI`/`DI` += element size), assuming
  /// the direction flag is clear (increment). Only called when
  /// `!df_may_be_set`, so the direction is provably clear (DOS ABI).
  fn advance_string_ptr(&mut self, reg: instr::Reg, size: i16) {
    let cur = self.ir.get_var(reg, self.cur);
    let step = self.ir.const_new(size);
    let next = self.append_instr(Type::U16, Opcode::Add, vec![cur, step]);
    self.append_asm_dst_reg(&instr::OperandReg(reg), next);
  }

  /// Finish a single-step string op: advance the pointers the op updates
  /// (with DF=0), or, when the function contains a `std`, degrade to a
  /// loud marker instead of a possibly-wrong-direction update.
  fn string_op_advance(&mut self, si: Option<&instr::Operand>, di: Option<&instr::Operand>) {
    if self.df_may_be_set {
      self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      return;
    }
    if let Some(oper) = si {
      let sz = Self::string_operand_size(oper);
      self.advance_string_ptr(instr::Reg::SI, sz);
    }
    if let Some(oper) = di {
      let sz = Self::string_operand_size(oper);
      self.advance_string_ptr(instr::Reg::DI, sz);
    }
  }

  /// Element size (bytes) of a string operand.
  fn string_operand_size(oper: &instr::Operand) -> i16 {
    match oper {
      instr::Operand::Mem(m) => match m.sz {
        instr::Size::Size8 => 1,
        instr::Size::Size16 => 2,
        instr::Size::Size32 => 4,
      },
      _ => 1,
    }
  }

  /// Lower a `rep` MOVS/STOS (direction provably clear): emit the runtime
  /// bulk macro over the pre-update values, then model the deterministic
  /// register updates in the SSA (`CX -> 0`, `SI/DI += CX0*size`). The
  /// macro takes value copies, so it never mutates the SSA pointers.
  fn lower_rep_string_op(&mut self, ins: &instr::Instr) {
    let cx = self.ir.get_var(instr::Reg::CX, self.cur);
    let zero = self.ir.const_new(0);
    let seg_of = |oper: &instr::Operand, fallback: instr::Reg| match oper {
      instr::Operand::Mem(m) => m.sreg,
      _ => fallback,
    };
    match ins.opcode {
      instr::Opcode::OP_MOVS => {
        let dst = &ins.operands[0]; // ES:DI
        let src = &ins.operands[1]; // DS:SI (segment may be overridden)
        let size = Self::string_operand_size(dst);
        let ds = self.ir.get_var(seg_of(src, instr::Reg::DS), self.cur);
        let es = self.ir.get_var(seg_of(dst, instr::Reg::ES), self.cur);
        let si = self.ir.get_var(instr::Reg::SI, self.cur);
        let di = self.ir.get_var(instr::Reg::DI, self.cur);
        let size_c = self.ir.const_new(size);
        self.append_instr(Type::Void, Opcode::RepMovs, vec![ds, si, es, di, cx, size_c]);
        let off = self.append_instr(Type::U16, Opcode::UMul, vec![cx, size_c]);
        let si_n = self.append_instr(Type::U16, Opcode::Add, vec![si, off]);
        let di_n = self.append_instr(Type::U16, Opcode::Add, vec![di, off]);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::SI), si_n);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::DI), di_n);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::CX), zero);
      }
      instr::Opcode::OP_STOS => {
        let dst = &ins.operands[0]; // ES:DI
        let size = Self::string_operand_size(dst);
        let es = self.ir.get_var(seg_of(dst, instr::Reg::ES), self.cur);
        let di = self.ir.get_var(instr::Reg::DI, self.cur);
        let val = self.append_asm_src_operand(&ins.operands[1]); // AL/AX
        let size_c = self.ir.const_new(size);
        self.append_instr(Type::Void, Opcode::RepStos, vec![es, di, cx, val, size_c]);
        let off = self.append_instr(Type::U16, Opcode::UMul, vec![cx, size_c]);
        let di_n = self.append_instr(Type::U16, Opcode::Add, vec![di, off]);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::DI), di_n);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::CX), zero);
      }
      _ => unreachable!("lower_rep_string_op called for {:?}", ins.opcode),
    }
  }

  fn pin_register(&mut self, reg: instr::Reg) {
    let vref = self.ir.get_var(reg, self.cur);
    let pin = self.append_instr(Type::Void, Opcode::Pin, vec![vref]);
    self.ir.set_var(reg, self.cur, pin);
  }

  fn return_vals(&mut self) -> Vec<Ref> {
    let ax = self.ir.get_var(instr::Reg::AX, self.cur);
    let dx = self.ir.get_var(instr::Reg::DX, self.cur);
    let ax_dx = vec![ax, dx];

    let Some(func) = self.spec.func else {
      // Assume worst case: DX:AX return
      return ax_dx;
    };

    let Some(ret) = &func.ret else {
      // Assume worst case: DX:AX return
      return ax_dx;
    };

    // Use type based return information
    match ret {
      Type::Void => vec![], // no return value
      Type::U8 | Type::I8 | Type::U16 | Type::I16 => vec![ax],
      Type::U32 | Type::I32  => vec![ax, dx],
      _ => panic!("Unsupported function return type: {}", ret),
    }
  }

  fn pin_registers(&mut self) {
    // Caller-saved: so don't bother.. unless pin_all
    if self.pin_all {
      self.pin_register(instr::Reg::AX);
      self.pin_register(instr::Reg::CX);
      self.pin_register(instr::Reg::DX);
      self.pin_register(instr::Reg::BX);
      self.pin_register(instr::Reg::ES);
    }

    self.pin_register(instr::Reg::SP);
    self.pin_register(instr::Reg::BP);
    self.pin_register(instr::Reg::SI);
    self.pin_register(instr::Reg::DI);

    self.pin_register(instr::Reg::SS);
    self.pin_register(instr::Reg::DS);

    // ??? Technically we should have this ???
    //self.pin_register(instr::Reg::FLAGS);

    // What would this even mean??
    //self.pin_register(instr::Reg::CS);
    //self.pin_register(instr::Reg::IP);
  }

  fn append_push(&mut self, vref: Ref) {
    let ss = self.ir.get_var(instr::Reg::SS, self.cur);
    let sp = self.ir.get_var(instr::Reg::SP, self.cur);
    let k = self.ir.const_new(2);

    // decrement SP
    let sp = self.append_instr_with_attrs(Type::U16, Attribute::STACK_PTR, Opcode::Sub, vec![sp, k]);
    self.ir.set_var(instr::Reg::SP, self.cur, sp);

    // store to SS:SP
    self.append_instr(Type::Void, Opcode::Store16, vec![ss, sp, vref]);
  }

  fn append_pop(&mut self) -> Ref {
    let ss = self.ir.get_var(instr::Reg::SS, self.cur);
    let sp = self.ir.get_var(instr::Reg::SP, self.cur);
    let k = self.ir.const_new(2);

    let val = self.append_instr(Type::U16, Opcode::Load16, vec![ss, sp]);
    let sp = self.append_instr_with_attrs(Type::U16, Attribute::STACK_PTR, Opcode::Add, vec![sp, k]);
    self.ir.set_var(instr::Reg::SP, self.cur, sp);

    val
  }

  // Try to infer the number of args by looking at the surrounding context for
  // typical stack manipulation associated with the 8086 dos calling conventions
  fn heuristic_infer_call_arguments_by_context(&self, ins: &instr::Instr, warn_msg: &str) -> u16 {
    let idx = offset_from(self.instrs, ins);

    let mut bytes_pushed_before = 0;
    for i in (0..idx).rev() {
      if self.instrs[i].opcode != instr::Opcode::OP_PUSH { break; }
      bytes_pushed_before += 2;
    }

    let mut bytes_cleanup_after = 0;
    if idx+1 < self.instrs.len() {
      let cleanup = &self.instrs[idx+1];
      match cleanup.opcode {
        instr::Opcode::OP_POP => {
          if matches!(cleanup.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::CX))) {
            bytes_cleanup_after = 2;
          }
        }
        instr::Opcode::OP_ADD => {
        let matches =
            matches!(cleanup.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::SP))) &&
            matches!(cleanup.operands[1], instr::Operand::Imm(_));
          if matches {
            let instr::Operand::Imm(imm) = cleanup.operands[1] else { unreachable!() };
            bytes_cleanup_after = imm.val;
          }
        }
        _ => (),
      }
    }

    if bytes_pushed_before >= bytes_cleanup_after {
      assert!(bytes_cleanup_after%2 == 0);
      let n = bytes_cleanup_after/2;
      eprintln!("WARN: {}, inferred {} arg(s)... possibly erroneously", warn_msg, n);
      n
    } else {
      // If more bytes are cleaned up then pushed.. pessimize
      let n = 0;
      eprintln!("WARN: {}, failed to infer, assuming {} arg(s)...very likely erroneously", warn_msg, n);
      n
    }
  }

  fn load_args_from_stack(&mut self, n: u16) -> Vec<Ref> {
    let mut args = vec![];
    let ss = self.ir.get_var(instr::Reg::SS, self.cur);
    let sp = self.ir.get_var(instr::Reg::SP, self.cur);
    for i in 0..n {
      let mut off = sp;
      if i != 0 {
        let k = self.ir.const_new((2*i) as i16);
        off = self.append_instr_with_attrs(Type::U16, Attribute::STACK_PTR, Opcode::Add, vec![sp, k]);
      }
      let val = self.append_instr(Type::U16, Opcode::Load16, vec![ss, off]);
      args.push(val);
    }
    args
  }

  fn save_return_value(&mut self, ret_type: &Type, ret_ref: Ref) {
    match ret_type {
      Type::Void => (), // nothing to do
      Type::U16 => {
        self.ir.set_var(instr::Reg::AX, self.cur, ret_ref);
      }
      Type::U32 | Type::Unknown => {  // Assume worst-case u32 for unknown
        let (upper, lower) = self.append_upper_lower_split(ret_ref);
        self.ir.set_var(instr::Reg::DX, self.cur, upper);
        self.ir.set_var(instr::Reg::AX, self.cur, lower);
      }
      _ => panic!("Unsupported function return type: {}", ret_type),
    }
    // Calls clobber AX/CX/DX (Watcom caller-saved); refresh the
    // partial-register views so later fragment uses see post-call values
    // instead of stale pre-call versions (e.g. `call toupper` followed by
    // `mov [bx],al` must store the call result).
    // BX/SI/DI/BP are callee-saved: untouched.
    for reg in [instr::Reg::AX, instr::Reg::CX, instr::Reg::DX] {
      let cur = self.ir.get_var(reg, self.cur);
      let (upper, lower) = self.append_upper_lower_split(cur);
      let (lo, hi) = match reg {
        instr::Reg::AX => (instr::Reg::AL, instr::Reg::AH),
        instr::Reg::CX => (instr::Reg::CL, instr::Reg::CH),
        instr::Reg::DX => (instr::Reg::DL, instr::Reg::DH),
        _ => unreachable!(),
      };
      self.ir.set_var(lo, self.cur, lower);
      self.ir.set_var(hi, self.cur, upper);
    }
  }

  fn process_callf_indirect(&mut self, ins: &instr::Instr) {
    let (ret_type, args) = if let Some(indirect) = self.cfg.indirect_lookup(ins.addr) {
      (&indirect.ret, indirect.args)
    } else {
      let nargs = self.heuristic_infer_call_arguments_by_context(ins, &format!(
        "Unknown ptr call from '{}' as binary loc {}", instr_str(ins), ins.addr));
      (&Type::Unknown, nargs)
    };

    let addr = self.append_asm_src_operand(&ins.operands[0]);
    let mut operands = vec![addr];
    operands.append(&mut self.load_args_from_stack(args));
    let ret_ref = self.append_instr(ret_type.clone(), Opcode::CallPtr, operands);
    self.save_return_value(ret_type, ret_ref);
  }

  fn process_call_known(&mut self, func: &config::Func, ins: &instr::Instr) {
    let idx = self.ir.funcs.len();
    self.ir.funcs.push(func.name.to_string());

    let nargs = func.args.unwrap_or_else(|| {
      self.heuristic_infer_call_arguments_by_context(ins,
        &format!("Far call to {} with unknown args", func.name))
    });

    let mut operands = vec![Ref::Func(idx)];
    operands.append(&mut self.load_args_from_stack(nargs));

    let ret_type = func.return_type_defaulted();
    let ret_ref = self.append_instr(ret_type.clone(), Opcode::CallArgs, operands);
    self.save_return_value(&ret_type, ret_ref);

    if func.dont_pop_args {
      let sp = self.ir.get_var(instr::Reg::SP, self.cur);
      let k = self.ir.const_new((2*nargs) as i16);
      let sp = self.append_instr_with_attrs(Type::U16, Attribute::STACK_PTR, Opcode::Add, vec![sp, k]);
      self.ir.set_var(instr::Reg::SP, self.cur, sp);
    }
  }

  fn append_regargs(&mut self, regargs: &Option<Vec<instr::Reg>>) {
    let Some(regargs) = regargs else { return };
    for reg in regargs {
      let vref = self.ir.get_var(reg, self.cur);
      let sym = self.ir.symbols.find_ref_by_name(sym::Table::Register, reg.info().name).unwrap();
      self.append_instr_with_attrs(Type::U16, Attribute::PIN, Opcode::WriteVar16, vec![Ref::Symbol(sym), vref]);
    }
  }

  fn process_call_segoff(&mut self, addr: SegOff, mode: config::CallMode, ins: &instr::Instr) {
    if let Some(func) = self.cfg.func_lookup(addr) {
      // Known function
      if func.mode != mode {
        panic!("Found function but it's call mode doesn't match! Expected {:?}, Got {:?}", mode, func.mode);
      }
      self.append_regargs(&func.regargs);
      self.process_call_known(func, ins);
    } else {
      // Unknown function
      let ret_type = Type::Unknown;
      let nargs = self.heuristic_infer_call_arguments_by_context(ins,
                    &format!("Unknown call to {}", addr));

      // FIXME: Can we do unwrap_normal() ??
      let seg = match addr.seg {
        Seg::Normal(num) => self.ir.const_new(num as i16),
        Seg::Overlay(num) => self.ir.const_new(-(num as i16)),
      };
      //let seg = self.ir.const_new(addr.seg.unwrap_normal() as i16);
      let off = self.ir.const_new(addr.off.0 as i16);

      let mut operands = vec![seg, off];
      operands.append(&mut self.load_args_from_stack(nargs));
      let opcode = match mode {
        config::CallMode::Far => Opcode::CallFar,
        config::CallMode::Near => Opcode::CallNear,
      };
      let ret_ref = self.append_instr(ret_type.clone(), opcode, operands);
      self.save_return_value(&ret_type, ret_ref);
    }
  }

  fn process_calln_indirect(&mut self, ins: &instr::Instr) {
    let nargs = self.heuristic_infer_call_arguments_by_context(ins, &format!(
      "Unknown near ptr call from '{}' as binary loc {}", instr_str(ins), ins.addr));

    let addr = self.append_asm_src_operand(&ins.operands[0]);
    let mut operands = vec![addr];
    operands.append(&mut self.load_args_from_stack(nargs));
    let ret_ref = self.append_instr(Type::Unknown, Opcode::CallPtr, operands);
    self.save_return_value(&Type::Unknown, ret_ref);
  }

  fn process_callf(&mut self, ins: &instr::Instr) {
    let instr::Operand::Far(far) = &ins.operands[0] else {
      return self.process_callf_indirect(ins);
    };
    let seg = if self.overlay {
      // Far calls from overlays need to be remapped
      self.binary.remap_to_segment(far.seg)
    } else {
      // Otherwise: Normal
      Seg::Normal(far.seg)
    };
    //let seg = Seg::Normal(far.seg);
    let addr = SegOff { seg, off: Off(far.off) };
    self.process_call_segoff(addr, config::CallMode::Far, ins);
  }

  fn process_calln(&mut self, ins: &instr::Instr, cs_pushed: bool) {
    // Register- or memory-indirect near calls (`call ax`, `call [bx]`)
    // have no static target; model them like far-indirect calls (CallPtr
    // with heuristically inferred stack args) instead of aborting.
    let instr::Operand::Rel(rel) = &ins.operands[0] else {
      return self.process_calln_indirect(ins);
    };
    let addr = ins.rel_addr(rel);
    if cs_pushed {
      // If CS was previously pushed, then the function returns
      // via "retf", which means its juat a far-call in disguise.
      // So, POP CS back off the stack and let's pretend it's a normal
      // far call
      self.append_pop();
      self.process_call_segoff(addr, config::CallMode::Far, ins);
    } else {
      // Otherwise, we assume it's actually a near call
      self.process_call_segoff(addr, config::CallMode::Near, ins);
    }
  }

  fn process_int(&mut self, ins: &instr::Instr) {
    // An INT handler may read any general-purpose register, so first make the
    // live register values explicit as pinned guest writes. Unlike appending
    // them as operands of `Int` (its emitter renders only operands[0]; see
    // ast.rs), WriteVar16 against the Register symbol renders as a real guest
    // write and keeps each setup value live through DCE. Registers still at
    // their initial (unknown) value are skipped: writing them would emit a
    // no-op `BX = BX`.
    for reg in [instr::Reg::AX, instr::Reg::BX, instr::Reg::CX, instr::Reg::DX,
                instr::Reg::SI, instr::Reg::DI, instr::Reg::DS, instr::Reg::ES] {
      let vref = self.ir.get_var(reg, self.cur);
      if vref == Ref::Init(reg) { continue; }
      let sym = self.ir.symbols.find_ref_by_name(sym::Table::Register, reg.info().name).unwrap();
      self.append_instr_with_attrs(Type::U16, Attribute::PIN, Opcode::WriteVar16, vec![Ref::Symbol(sym), vref]);
    }

    let num = self.append_asm_src_operand(&ins.operands[0]);
    self.append_instr(Type::Void, Opcode::Int, vec![num]);

    // The handler may clobber any general-purpose register and leaves FLAGS
    // undefined to the decompiler, so reset the full consumed set (plus the
    // byte fragments, mirroring the entry seeding) to the initial/unknown
    // register value: post-INT reads then see the live register rather than
    // stale pre-INT setup (e.g. AH=2Ch returns the time in CX/DX), and a
    // following branch degrades to the loud UNIMPL_FLAGS marker instead of
    // silently folding against pre-INT flags (e.g. `sub ax,ax; int 21h; jz`).
    // SP/BP/SS/CS/IP are frame/machine state an INT preserves, so they are
    // left intact.
    for reg in [instr::Reg::AX, instr::Reg::BX, instr::Reg::CX, instr::Reg::DX,
                instr::Reg::SI, instr::Reg::DI, instr::Reg::DS, instr::Reg::ES,
                instr::Reg::AL, instr::Reg::AH, instr::Reg::BL, instr::Reg::BH,
                instr::Reg::CL, instr::Reg::CH, instr::Reg::DL, instr::Reg::DH,
                instr::Reg::FLAGS] {
      self.ir.set_var(reg, self.cur, Ref::Init(reg));
    }
  }

  fn append_cond_jump(&mut self, ins: &instr::Instr, compare_opcode: Opcode) {
    let instr::Operand::Rel(rel) = &ins.operands[0] else { panic!("Expected relative offset operand for JMP") };

    let false_blk = self.get_block(ins.end_addr());
    let true_blk = self.get_block(ins.rel_addr(rel));

    let flags = self.get_flags();
    let cond = self.append_instr(Type::U16, compare_opcode, vec![flags]);

    self.append_jne(cond, true_blk, false_blk);
  }

  fn append_cond_set(&mut self, ins: &instr::Instr, compare_opcode: Opcode) {
    let flags = self.get_flags();
    let cond = self.append_instr(Type::U8, compare_opcode, vec![flags]);
    self.append_asm_dst_operand(&ins.operands[0], cond);
  }

  fn append_upper_lower_split(&mut self, r: Ref) -> (Ref, Ref) {
    let upper = self.append_instr(Type::U16, Opcode::Upper16, vec![r]);
    let lower = self.append_instr(Type::U16, Opcode::Lower16, vec![r]);
    (upper, lower)
  }

  fn deduce_type_unary(&mut self, a: Ref) -> Type {
    //println!("a: {:?}", a);
    match a {
      Ref::Instr(_, _) => self.ir.instr(a).unwrap().typ.clone(),
      Ref::Init(_) => Type::U16,
      _ => Type::Unknown,
    }
  }

  fn deduce_type_binary(&mut self, a: Ref, b: Ref) -> Type {
    let a_typ = self.deduce_type_unary(a);
    let b_typ = self.deduce_type_unary(b);
    if a_typ == b_typ { a_typ }
    else if a_typ == Type::Unknown { b_typ }
    else if b_typ == Type::Unknown { a_typ }
    else { Type::Unknown }
  }

  fn append_asm_instr(&mut self, ins: &instr::Instr) {
    //println!("## {}", intel_syntax::format(ins, &[], false).unwrap());

    // Consume the push-cs marker first: it only describes the *immediately*
    // following instruction (the `push cs; call near` far-call idiom). Taking
    // it before the REP early-return below keeps e.g.
    // `push cs; rep movs; call near` from misclassifying the later call.
    let special = self.special.take();

    // REP-prefixed string instructions execute a counted loop. MOVS/STOS
    // are modeled exactly (runtime bulk macro + CX->0 and SI/DI += CX0*size
    // updates); LODS/CMPS/SCAS/INS/OUTS still lower to Unimpl (their
    // result is data-dependent). Any of them with a possibly-set direction
    // flag stays loud rather than guessing the direction.
    if ins.rep.is_some() {
      let modeled = matches!(ins.opcode, instr::Opcode::OP_MOVS | instr::Opcode::OP_STOS);
      if !modeled || self.df_may_be_set {
        // Flags are conservatively clobbered because e.g. `repe scasb`
        // defines the flags its caller tests.
        let vref = self.append_instr(Type::U16, Opcode::Unimpl, vec![]);
        self.append_update_flags(vref);
        return;
      }
      self.lower_rep_string_op(ins);
      return;
    }

    // process simple unary operations
    if let Some(opcode) = simple_unary_operation(ins.opcode) {
      let a = self.append_asm_src_operand(&ins.operands[0]);
      let typ = self.deduce_type_unary(a);
      let vref = self.append_instr(typ, opcode, vec![a]);
      self.append_asm_dst_operand(&ins.operands[0], vref);
      return;
    }

    // process simple binary operations
    if let Some(opcode) = simple_binary_operation(ins.opcode) {
      let attr = if operand_is_stack_reg(&ins.operands[0]) { Attribute::STACK_PTR } else { Attribute::NONE };
      let a = self.append_asm_src_operand(&ins.operands[0]);
      let b = self.append_asm_src_operand(&ins.operands[1]);
      let typ = self.deduce_type_binary(a, b);
      let vref = self.append_instr_with_attrs(typ, attr, opcode, vec![a, b]);
      self.append_asm_dst_operand(&ins.operands[0], vref);
      self.append_update_flags(vref);
      return;
    }

    // handle less standard operations
    match &ins.opcode {
      instr::Opcode::OP_NOP => (),
      instr::Opcode::OP_SBB => {
        // FIXME: WE DON'T HAVE  A GREAT WAY TO IMPL SBC AT THE MOMENT BECAUSE IT NEEDS TO CORRECTLY
        // CONSUME THE CARRY FLAG FROM A PREVIOUS SUBTRACT THAT GENERATED A BORROW. BUT, AT THE MOMENT,
        // WE USE "UpdateFlags" AS A PLACEHOLDER FOR ARBITRARY FLAGS UPDATES WITHOUT BREAKING DOWN INTO
        // THE SPECIFIC FLAGS AFFECTED. SO FOR NOW, WE'LL JAM IT THROUGH "Unimpl" AND MAKE THE USER
        // DEAL WITH IT MANUALLY
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        let vref = self.append_instr(typ, Opcode::Unimpl, vec![a, b]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        self.append_update_flags(vref);
      }
      instr::Opcode::OP_RCL | instr::Opcode::OP_RCR => {
        // FIXME: RCL/RCR rotate *through* carry, so the value needs the
        // incoming CF from the opaque flags value (#69 follow-up slice).
        // ROL/ROR below take no carry in and are lowered exactly.
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        let vref = self.append_instr(typ, Opcode::Unimpl, vec![a, b]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        self.append_update_flags(vref);
      }
      instr::Opcode::OP_ROL | instr::Opcode::OP_ROR => {
        // #69 slice 1: ROL/ROR take no carry *in*, so the value is exact
        // as shifts+or: rol(v,c) = (v<<n)|(v>>(w-n)) with n = c&(w-1),
        // which subsumes x86's 5-bit count mask (w is a power of 2). The
        // complementary shift is masked the same way so a zero count
        // yields (v<<0)|(v>>0) = v without a shift-by-width, which IR
        // reasoning cannot justify (only C promotion makes that defined).
        // Only the flags (CF/OF) are approximated: clobber them with an
        // opaque marker (STC/CLC/CMC precedent) so a following branch
        // degrades to loud UNIMPL_FLAGS, never silent. A zero count is
        // conservatively clobbered too (real HW preserves flags there,
        // but that shape is vanishingly rare outside padding).
        let is_rol = ins.opcode == instr::Opcode::OP_ROL;
        let width: Option<i16> = match &ins.operands[0] {
          instr::Operand::Reg(r) => match r.0 {
            instr::Reg::AL | instr::Reg::CL | instr::Reg::DL | instr::Reg::BL |
            instr::Reg::AH | instr::Reg::CH | instr::Reg::DH | instr::Reg::BH => Some(8),
            _ => Some(16),
          },
          instr::Operand::Mem(m) => match m.sz {
            instr::Size::Size8 => Some(8),
            instr::Size::Size16 => Some(16),
            // Unreachable via decode (no 0x66 operand-size prefix; the
            // fmt table yields RM8/RM16 rotate dests only): degrade
            // gracefully like RCL/RCR rather than panicking.
            _ => None,
          },
          _ => None,
        };
        let Some(width) = width else {
          let a = self.append_asm_src_operand(&ins.operands[0]);
          let b = self.append_asm_src_operand(&ins.operands[1]);
          let typ = self.deduce_type_binary(a, b);
          let vref = self.append_instr(typ, Opcode::Unimpl, vec![a, b]);
          self.append_asm_dst_operand(&ins.operands[0], vref);
          self.append_update_flags(vref);
          return;
        };
        let v = self.append_asm_src_operand(&ins.operands[0]);
        let c = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(v, c);
        let mask = self.ir.const_new(width - 1);
        let w = self.ir.const_new(width);
        let n = self.append_instr(Type::U16, Opcode::And, vec![c, mask]);
        let diff = self.append_instr(Type::U16, Opcode::Sub, vec![w, n]);
        let m = self.append_instr(Type::U16, Opcode::And, vec![diff, mask]);
        let (fwd, bwd) = if is_rol {
          (self.append_instr(typ.clone(), Opcode::Shl, vec![v, n]),
           self.append_instr(typ.clone(), Opcode::UShr, vec![v, m]))
        } else {
          (self.append_instr(typ.clone(), Opcode::UShr, vec![v, n]),
           self.append_instr(typ.clone(), Opcode::Shl, vec![v, m]))
        };
        let vref = self.append_instr(typ, Opcode::Or, vec![fwd, bwd]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        let marker = self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
        self.append_update_flags(marker);
      }
      instr::Opcode::OP_ADC => {
        // FIXME: SIMILAR FOR ADC ... WE SIMPLY DON'T HAVE A GREAT WAY TO CAPTURE THE SEMANTICS
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        let vref = self.append_instr(typ, Opcode::Unimpl, vec![a, b]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        self.append_update_flags(vref);
      }
      instr::Opcode::OP_LOOP | instr::Opcode::OP_LOOPE | instr::Opcode::OP_LOOPNE => {
        // LOOPcc decrements CX without modifying FLAGS. LOOPE/LOOPNE test the
        // ZF value produced by an earlier instruction, so preserve the current
        // flags SSA value before updating CX.
        let flags = match ins.opcode {
          instr::Opcode::OP_LOOPE | instr::Opcode::OP_LOOPNE => Some(self.get_flags()),
          _ => None,
        };
        let cx = self.append_asm_src_operand(&ins.operands[0]);
        let one = self.ir.const_new(1);
        let cx = self.append_instr(Type::U16, Opcode::Sub, vec![cx, one]);
        self.append_asm_dst_operand(&ins.operands[0], cx);

        let instr::Operand::Rel(rel) = &ins.operands[1] else { panic!("Expected relative offset operand for LOOPcc") };
        let false_blk = self.get_block(ins.end_addr());
        let true_blk = self.get_block(ins.rel_addr(rel));

        let z = self.ir.const_new(0);
        let cx_nonzero = self.append_instr(Type::U16, Opcode::Neq, vec![cx, z]);
        let cond = match ins.opcode {
          instr::Opcode::OP_LOOP => cx_nonzero,
          instr::Opcode::OP_LOOPE => {
            let zf_set = self.append_instr(Type::U16, Opcode::EqFlags, vec![flags.unwrap()]);
            self.append_instr(Type::U16, Opcode::And, vec![cx_nonzero, zf_set])
          }
          instr::Opcode::OP_LOOPNE => {
            let zf_clear = self.append_instr(Type::U16, Opcode::NeqFlags, vec![flags.unwrap()]);
            self.append_instr(Type::U16, Opcode::And, vec![cx_nonzero, zf_clear])
          }
          _ => unreachable!(),
        };

        self.append_jne(cond, true_blk, false_blk);
      }
      instr::Opcode::OP_XCHG => {
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        self.append_asm_dst_operand(&ins.operands[0], b);
        self.append_asm_dst_operand(&ins.operands[1], a);
      }
      instr::Opcode::OP_PUSH => {
        if matches!(ins.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::CS))) {
          assert!(self.special.is_none());
          self.special = Some(SpecialState::PushCS);
        }
        let a = self.append_asm_src_operand(&ins.operands[0]);
        self.append_push(a);
      }
      instr::Opcode::OP_POP => {
        let vref = self.append_pop();
        self.append_asm_dst_operand(&ins.operands[0], vref);
      }
      instr::Opcode::OP_PUSHA => {
        // PUSHA stores the value SP had before the first push.
        let original_sp = self.ir.get_var(instr::Reg::SP, self.cur);
        for reg in [instr::Reg::AX, instr::Reg::CX, instr::Reg::DX, instr::Reg::BX] {
          let vref = self.ir.get_var(reg, self.cur);
          self.append_push(vref);
        }
        self.append_push(original_sp);
        for reg in [instr::Reg::BP, instr::Reg::SI, instr::Reg::DI] {
          let vref = self.ir.get_var(reg, self.cur);
          self.append_push(vref);
        }
      }
      instr::Opcode::OP_POPA => {
        // POPA consumes all eight stack words, but discards the saved-SP word.
        for reg in [instr::Reg::DI, instr::Reg::SI, instr::Reg::BP] {
          let vref = self.append_pop();
          self.ir.set_var(reg, self.cur, vref);
        }
        let _saved_sp = self.append_pop();
        // Route through append_asm_dst_reg so the byte fragments are
        // re-split from the restored full values (a raw set_var would
        // leave them holding pre-pusha versions).
        for reg in [instr::Reg::BX, instr::Reg::DX, instr::Reg::CX, instr::Reg::AX] {
          let vref = self.append_pop();
          self.append_asm_dst_reg(&instr::OperandReg(reg), vref);
        }
      }
      instr::Opcode::OP_LEAVE => {
        // mov sp, bp
        let vref = self.ir.get_var(instr::Reg::BP, self.cur);
        self.ir.set_var(instr::Reg::SP, self.cur, vref);
        // pop bp
        let vref = self.append_pop();
        self.ir.set_var(instr::Reg::BP, self.cur, vref);
      }
      instr::Opcode::OP_RETF => {
        self.pin_registers();
        let ret_vals = self.return_vals();
        self.append_instr(Type::Void, Opcode::RetFar, ret_vals);

      }
      instr::Opcode::OP_RET => {
        self.pin_registers();
        let ret_vals = self.return_vals();
        self.append_instr(Type::Void, Opcode::RetNear, ret_vals);
      }
      instr::Opcode::OP_MOV => {
        let vref = self.append_asm_src_operand(&ins.operands[1]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
      }
      instr::Opcode::OP_IMUL_TRUNC => {
        // IMUL has many forms, let's be conservative for now and hard sanity check the version we need
        assert!(
          matches!(ins.operands[0], instr::Operand::Reg(_)) &&
          matches!(ins.operands[1], instr::Operand::Reg(_)) &&
          matches!(ins.operands[2], instr::Operand::Imm(_)));

        let lhs = self.append_asm_src_operand(&ins.operands[1]);
        let rhs = self.append_asm_src_operand(&ins.operands[2]);
        let typ = self.deduce_type_binary(lhs, rhs);
        let res = self.append_instr(typ, Opcode::IMul, vec![lhs, rhs]);
        self.append_asm_dst_operand(&ins.operands[0], res);
      }
      instr::Opcode::OP_IMUL => {
        // IMUL has many forms, let's be conservative for now and hard sanity check the version we need
        assert!(
          ins.operands.len() == 3 &&
          matches!(ins.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::DX))) &&
          matches!(ins.operands[1], instr::Operand::Reg(instr::OperandReg(instr::Reg::AX))));

        let lhs = self.append_asm_src_operand(&ins.operands[1]);
        let rhs = self.append_asm_src_operand(&ins.operands[2]);
        let res = self.append_instr(Type::U32, Opcode::IMul, vec![lhs, rhs]);
        let (upper, lower) = self.append_upper_lower_split(res);
        self.append_asm_dst_operand(&ins.operands[0], upper);
        self.append_asm_dst_operand(&ins.operands[1], lower);
      }
      instr::Opcode::OP_MUL => {
        // Sanity check the form we have
        assert!(
          matches!(ins.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::DX))) &&
          matches!(ins.operands[1], instr::Operand::Reg(instr::OperandReg(instr::Reg::AX))));

        let lhs = self.append_asm_src_operand(&ins.operands[1]);
        let rhs = self.append_asm_src_operand(&ins.operands[2]);
        let res = self.append_instr(Type::U32, Opcode::UMul, vec![lhs, rhs]);
        let (upper, lower) = self.append_upper_lower_split(res);
        self.append_asm_dst_operand(&ins.operands[0], upper);
        self.append_asm_dst_operand(&ins.operands[1], lower);
     }
      instr::Opcode::OP_INC => {
        let attr = if operand_is_stack_reg(&ins.operands[0]) { Attribute::STACK_PTR } else { Attribute::NONE };
        let one = self.ir.const_new(1);
        let vref = self.append_asm_src_operand(&ins.operands[0]);
        let typ = self.deduce_type_binary(vref, one);
        let vref = self.append_instr_with_attrs(typ, attr, Opcode::Add, vec![vref, one]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        self.append_update_flags(vref);
      }
      instr::Opcode::OP_DEC => {
        let attr = if operand_is_stack_reg(&ins.operands[0]) { Attribute::STACK_PTR } else { Attribute::NONE };
        let one = self.ir.const_new(1);
        let vref = self.append_asm_src_operand(&ins.operands[0]);
        let typ = self.deduce_type_binary(vref, one);
        let vref = self.append_instr_with_attrs(typ, attr, Opcode::Sub, vec![vref, one]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        self.append_update_flags(vref);
      }
      instr::Opcode::OP_JMP => {
        match &ins.operands[0] {
          instr::Operand::Mem(m) => {
            // Special handling for some indirect jumps
            let Some(targets) = self.jump_indirect_targets(ins, m) else {
              panic!("Indirect jump form not currently supported for '{}'", instr_str(ins));
            };
            let Some(reg) = m.reg1 else {
              panic!("Shouldn't fail.. should be checked by jump_indirect_targets already");
            };
            let reg_ref = self.ir.get_var(reg, self.cur);
            self.append_jmptbl(reg_ref, targets);
          }
          instr::Operand::Rel(rel) => {
            let blkref = self.get_block(ins.rel_addr(rel));
            self.append_jmp(blkref);
          }
          _ => panic!("Unsupported JMP operand for '{}'", instr_str(ins)),
        }
      }

      instr::Opcode::OP_JCXZ  => {
        let instr::Operand::Rel(rel) = &ins.operands[1] else { panic!("Expected relative offset operand for JCXZ") };

        let false_blk = self.get_block(ins.end_addr());
        let true_blk = self.get_block(ins.rel_addr(rel));

        let cx = self.append_asm_src_operand(&ins.operands[0]);
        let z = self.ir.const_new(0);
        let cond = self.append_instr(Type::U16, Opcode::Eq, vec![cx, z]);

        self.append_jne(cond, true_blk, false_blk);
      }

      instr::Opcode::OP_JS  => {
        let instr::Operand::Rel(rel) = &ins.operands[0] else { panic!("Expected relative offset operand for JS") };

        let false_blk = self.get_block(ins.end_addr());
        let true_blk = self.get_block(ins.rel_addr(rel));

        let flags = self.get_flags();
        let sign = self.append_instr(Type::U16, Opcode::SignFlags, vec![flags]);
        let z = self.ir.const_new(0);
        let cond = self.append_instr(Type::U16, Opcode::Neq, vec![sign, z]);

        self.append_jne(cond, true_blk, false_blk);
      }

      instr::Opcode::OP_JNS  => {
        let instr::Operand::Rel(rel) = &ins.operands[0] else { panic!("Expected relative offset operand for JNS") };

        let false_blk = self.get_block(ins.end_addr());
        let true_blk = self.get_block(ins.rel_addr(rel));

        let flags = self.get_flags();
        let sign = self.append_instr(Type::U16, Opcode::SignFlags, vec![flags]);
        let z = self.ir.const_new(0);
        let cond = self.append_instr(Type::U16, Opcode::Eq, vec![sign, z]);

        self.append_jne(cond, true_blk, false_blk);
      }

      instr::Opcode::OP_JE  => self.append_cond_jump(ins, Opcode::EqFlags),
      instr::Opcode::OP_JNE => self.append_cond_jump(ins, Opcode::NeqFlags),
      instr::Opcode::OP_JG  => self.append_cond_jump(ins, Opcode::GtFlags),
      instr::Opcode::OP_JGE => self.append_cond_jump(ins, Opcode::GeqFlags),
      instr::Opcode::OP_JL  => self.append_cond_jump(ins, Opcode::LtFlags),
      instr::Opcode::OP_JLE => self.append_cond_jump(ins, Opcode::LeqFlags),
      instr::Opcode::OP_JA  => self.append_cond_jump(ins, Opcode::UGtFlags),
      instr::Opcode::OP_JAE => self.append_cond_jump(ins, Opcode::UGeqFlags),
      instr::Opcode::OP_JB  => self.append_cond_jump(ins, Opcode::ULtFlags),
      instr::Opcode::OP_JBE => self.append_cond_jump(ins, Opcode::ULeqFlags),

      instr::Opcode::OP_SETE  => self.append_cond_set(ins, Opcode::EqFlags),
      instr::Opcode::OP_SETNE => self.append_cond_set(ins, Opcode::NeqFlags),
      instr::Opcode::OP_SETG  => self.append_cond_set(ins, Opcode::GtFlags),
      instr::Opcode::OP_SETGE => self.append_cond_set(ins, Opcode::GeqFlags),
      instr::Opcode::OP_SETL  => self.append_cond_set(ins, Opcode::LtFlags),
      instr::Opcode::OP_SETLE => self.append_cond_set(ins, Opcode::LeqFlags),
      instr::Opcode::OP_SETA  => self.append_cond_set(ins, Opcode::UGtFlags),
      instr::Opcode::OP_SETAE => self.append_cond_set(ins, Opcode::UGeqFlags),
      instr::Opcode::OP_SETB  => self.append_cond_set(ins, Opcode::ULtFlags),
      instr::Opcode::OP_SETBE => self.append_cond_set(ins, Opcode::ULeqFlags),

      instr::Opcode::OP_CALLF => {
        self.process_callf(ins);
      }
      instr::Opcode::OP_CALL => {
        let cs_pushed = matches!(special, Some(SpecialState::PushCS));
        self.process_calln(ins, cs_pushed);
      }
      instr::Opcode::OP_INT => {
        self.process_int(ins);
      }
      instr::Opcode::OP_LEA => {
        let instr::Operand::Mem(mem) = &ins.operands[1] else {
          panic!("Expected LEA to have a mem operand");
        };

        let addr = self.compute_mem_address(mem);

        // NOTE: Another weird 8086 quirk, the sreg here is meaningless (lol).
        //       LEA is only computing a 16-bit offset, not a full seg:off address
        //       This is only the case because LEA strictly DOES NOT dereference.

        self.append_asm_dst_operand(&ins.operands[0], addr);
      }
      instr::Opcode::OP_LES | instr::Opcode::OP_LDS => {
        let vref = self.append_asm_src_operand(&ins.operands[2]);
        let (upper, lower) = self.append_upper_lower_split(vref);
        self.append_asm_dst_operand(&ins.operands[0], upper);
        self.append_asm_dst_operand(&ins.operands[1], lower);
      }
      instr::Opcode::OP_TEST => {
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        let vref = self.append_instr(typ, Opcode::And, vec![a, b]);
        self.append_update_flags(vref);
      }
      instr::Opcode::OP_CMP => {
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        let vref = self.append_instr(typ, Opcode::Sub, vec![a, b]);
        self.append_update_flags(vref);
      }
      instr::Opcode::OP_CWD => {
        let src = self.append_asm_src_operand(&ins.operands[1]);
        let vref = self.append_instr(Type::U32, Opcode::SignExtTo32, vec![src]);
        let upper = self.append_instr(Type::U16, Opcode::Upper16, vec![vref]);
        self.append_asm_dst_operand(&ins.operands[0], upper);
      }
      instr::Opcode::OP_DIV => {
        let upper_in = self.append_asm_src_operand(&ins.operands[0]);
        let lower_in = self.append_asm_src_operand(&ins.operands[1]);
        let divisor = self.append_asm_src_operand(&ins.operands[2]);
        let dividend = self.append_instr(Type::U32, Opcode::Make32, vec![upper_in, lower_in]);
        let quotient = self.append_instr(Type::U32, Opcode::UDiv, vec![dividend, divisor]);
        let (upper_out, lower_out) = self.append_upper_lower_split(quotient);
        self.append_asm_dst_operand(&ins.operands[0], upper_out);
        self.append_asm_dst_operand(&ins.operands[1], lower_out);
      }
      instr::Opcode::OP_STOS => {
        let src = self.append_asm_src_operand(&ins.operands[1]);
        self.append_asm_dst_operand(&ins.operands[0], src);
        self.string_op_advance(None, Some(&ins.operands[0]));
      }
      instr::Opcode::OP_MOVS => {
        // Single-step move string (REP form is handled above). The SI/DI
        // auto-update is modeled with DF=0; a hand-written `movsb` loop
        // would otherwise copy the same byte forever.
        let src = self.append_asm_src_operand(&ins.operands[1]);
        self.append_asm_dst_operand(&ins.operands[0], src);
        self.string_op_advance(Some(&ins.operands[1]), Some(&ins.operands[0]));
      }
      instr::Opcode::OP_CMPS => {
        // Compare DS:SI against ES:DI (i.e. operands[1] minus operands[0]).
        let dst = self.append_asm_src_operand(&ins.operands[0]);
        let src = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(src, dst);
        let vref = self.append_instr(typ, Opcode::Sub, vec![src, dst]);
        self.append_update_flags(vref);
        self.string_op_advance(Some(&ins.operands[1]), Some(&ins.operands[0]));
      }
      instr::Opcode::OP_SCAS => {
        // Compare AL/AX against ES:DI (i.e. operands[0] minus operands[1]).
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        let vref = self.append_instr(typ, Opcode::Sub, vec![a, b]);
        self.append_update_flags(vref);
        self.string_op_advance(None, Some(&ins.operands[1]));
      }
      instr::Opcode::OP_INS => {
        // Neither the value nor the SI/DI update is modeled; INS does not
        // affect flags, so the flags value must be left intact.
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_OUTS => {
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_STD => {
        // Direction flag is not modeled at all; no modeled value is affected.
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_STC | instr::Opcode::OP_CLC | instr::Opcode::OP_CMC => {
        // These set/clear/complement CF. The IR carries one opaque flags
        // value, so redefining it here makes a subsequent jb/jae degrade to
        // UNIMPL_FLAGS rather than silently reading a stale carry.
        let marker = self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
        self.append_update_flags(marker);
      }
      instr::Opcode::OP_LODS => {
        let src = self.append_asm_src_operand(&ins.operands[1]);
        self.append_asm_dst_operand(&ins.operands[0], src);
        self.string_op_advance(Some(&ins.operands[1]), None);
      }
      instr::Opcode::OP_STI => {
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_CLI => {
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_IN => {
        self.append_instr(Type::U8, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_OUT => {
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_CLD => {
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_CBW => {
        let src = self.append_asm_src_operand(&ins.operands[1]);
        let ext = self.append_instr(Type::U16, Opcode::SignExtTo16, vec![src]);
        self.append_asm_dst_operand(&ins.operands[0], ext);
      }
      _ => panic!("Unimpl opcode: {:?}", ins.opcode),
    }
  }

  fn build(&mut self) {
    // Step 0: Direction-flag guard. DF is written only by CLD/STD and the
    // flag-restoring POPF/IRET; with none present the direction is provably
    // clear (see the df_may_be_set field).
    self.df_may_be_set = self.instrs.iter().any(|i| opcode_may_set_df(i.opcode));

    // Step 1: Infer basic-block boundaries
    let mut block_start = HashSet::new();
    let mut last_was_jump = false;
    for ins in self.instrs {
      if last_was_jump {
        block_start.insert(ins.addr);
        last_was_jump = false;
      }
      let Some((explicit, fallthrough)) = self.jump_targets(ins) else { continue };
      // Every target — including an implicit fallthrough — needs a block:
      // conditional lowerings resolve ins.end_addr() via get_block, which
      // panics on a missing entry when the branch is the last decoded
      // instruction. Only explicit targets are range-checked below.
      for tgt in explicit.iter().chain(fallthrough.iter()) {
        block_start.insert(*tgt);
      }
      // Explicit branch targets must lie inside the function's [start,
      // end): anything outside decodes to zero instructions, becomes an
      // empty block, and is silently dropped from the output. Warn loudly
      // instead of hiding the lost edge; the drop semantics are unchanged.
      for tgt in &explicit {
        if self.out_of_range_edge(*tgt, fallthrough) {
          eprintln!("WARN: {}: branch at {} targets {} outside function range [{}, {}); the edge is dropped from the output",
            self.spec.name, ins.addr, tgt, self.spec.start, self.spec.end);
        }
      }
      last_was_jump = true;
    }

    // Step 2: Create all the blocks we should encounter
    let mut addr_ordered: Vec<_> = block_start.iter().collect();
    addr_ordered.sort();
    for addr in addr_ordered {
      let bref = self.new_block(&format!("addr_{}", addr.off));
      self.addrmap.insert(*addr, bref);
    }

    // Step 3: iterate each instruction, building each block
    for ins in self.instrs {
      if DEBUG { println!("DEBUG: {}", instr_str(ins)); }
      if block_start.get(&ins.addr).is_some() {
        self.start_next_blk(ins.addr);
      }
      self.append_asm_instr(ins);
    }

    // Step 4: walk blocks and seal them
    for blkref in self.ir.iter_blocks() {
      if !self.ir.block(blkref).sealed {
        self.ir.seal_block(blkref);
      }
    }
  }
}

/// Returns the offset of `elt` into `slice`.
fn offset_from<T>(slice: &[T], elt: &T) -> usize {
  let elt_addr = elt as *const T as usize;
  let slice_addr = slice.as_ptr() as usize;
  if elt_addr < slice_addr { panic!("elt not in slice!"); }

  let off = elt_addr - slice_addr;
  if (off % std::mem::size_of::<T>()) != 0 { panic!("offset is not a multiple of element size"); }

  let n = off / std::mem::size_of::<T>();
  if n >= slice.len() { panic!("index is out of range"); }

  // sanity
  debug_assert!(&slice[n] as *const _ == elt as *const _);

  n
}

pub fn build_from_instrs(instrs: &[instr::Instr], cfg: &Config, spec: &spec::Spec<'_>, binary: &binary::Binary, overlay: bool, pin_all: bool) -> IR {
  let mut bld = IRBuilder::new(cfg, instrs, spec, binary, overlay, pin_all);
  bld.build();
  bld.ir
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::types::TypeDatabase;
  use crate::util::arrayvec::ArrayVec;
  use std::rc::Rc;

  fn test_builder<'a>(cfg: &'a Config, instrs: &'a [instr::Instr], spec: &'a spec::Spec<'a>, binary: &'a binary::Binary) -> IRBuilder<'a> {
    IRBuilder::new(cfg, instrs, spec, binary, false, false)
  }

  fn int_instr(num: u16) -> instr::Instr {
    let mut operands = ArrayVec::new();
    operands.push(instr::Operand::Imm(instr::OperandImm { sz: instr::Size::Size8, val: num }));
    instr::Instr {
      rep: None,
      opcode: instr::Opcode::OP_INT,
      operands,
      addr: SegOff { seg: Seg::Normal(0), off: Off(0) },
      n_bytes: 2,
      intel_hidden_operand_bitmask: 0,
    }
  }

  #[test]
  fn partial_writes_recompose_full_register() {
    // mov ax, K then mov al, M must leave AX as (HI<<8)|M, not a bare
    // store of M: a later fragment read would otherwise see a stale
    // version (e.g. setups elided across calls going stale).
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    // Full write splits into fragments.
    let full = b.ir.const_new(0x1234);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AX), full);
    let al = b.ir.get_var(instr::Reg::AL, cur);
    let ah = b.ir.get_var(instr::Reg::AH, cur);
    assert_eq!(b.ir.instr(al).unwrap().opcode, Opcode::Lower16);
    assert_eq!(b.ir.instr(ah).unwrap().opcode, Opcode::Upper16);
    // Partial write recomposes instead of bare-storing.
    let lo_new = b.ir.const_new(0x56);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AL), lo_new);
    let ax = b.ir.get_var(instr::Reg::AX, cur);
    let ax_instr = b.ir.instr(ax).expect("AX must be recomposed, not a bare constant");
    assert_eq!(ax_instr.opcode, Opcode::Or);
    assert_eq!(ax_instr.operands.len(), 2);
    // The incoming byte is masked to 8 bits at the write, so the Or
    // carries And(lo_new, 0xFF) rather than lo_new itself.
    let masked = ax_instr.operands.iter().find_map(|r| {
      let i = b.ir.instr(*r)?;
      (i.opcode == Opcode::And && i.operands.contains(&lo_new)).then_some(*r)
    }).expect("recomposition must carry the masked new LO");
    assert!(ax_instr.operands.contains(&masked));
    assert!(
      ax_instr.operands.iter().any(|r| b.ir.instr(*r).map(|i| i.opcode) == Some(Opcode::Shl)),
      "recomposition must shift the preserved HI"
    );
  }

  #[test]
  fn undefined_sibling_register_seeds_initial_value() {
    // A fragment read with no prior write must resolve to the initial
    // register value (a machine.h byte alias), never to an
    // operand-less Phi whose name is never declared in the output.
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    assert_eq!(b.ir.get_var(instr::Reg::AL, cur), Ref::Init(instr::Reg::AL));
    assert_eq!(b.ir.get_var(instr::Reg::AH, cur), Ref::Init(instr::Reg::AH));
    assert_eq!(b.ir.get_var(instr::Reg::CL, cur), Ref::Init(instr::Reg::CL));
    assert_eq!(b.ir.get_var(instr::Reg::DH, cur), Ref::Init(instr::Reg::DH));
  }

  #[test]
  fn first_fragment_write_has_no_undeclared_phi() {
    // `mov ah, 9` as the first write to the pair: the recomposed AX
    // must reference Init(AL), not a Phi over the never-written sibling.
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    let nine = b.ir.const_new(9);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AH), nine);
    let ax = b.ir.get_var(instr::Reg::AX, cur);
    let ax_instr = b.ir.instr(ax).expect("AX must be recomposed from the first fragment write");
    assert_eq!(ax_instr.opcode, Opcode::Or);
    assert!(
      ax_instr.operands.contains(&Ref::Init(instr::Reg::AL)),
      "untouched sibling must be the initial register value, not a Phi"
    );
  }

  #[test]
  fn partial_write_masks_wide_source() {
    // `mov cl, dx` with DX holding 16 bits: the fragment keeps only the
    // low byte, so the recomposed CX is (HI<<8)|(DX&0xFF), never leaking
    // DX's high byte into the pair.
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    let wide = b.ir.const_new(0x12AB);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::CL), wide);
    let cl = b.ir.get_var(instr::Reg::CL, cur);
    let cl_instr = b.ir.instr(cl).expect("fragment must hold the masked write");
    assert_eq!(cl_instr.opcode, Opcode::And);
    assert!(cl_instr.operands.contains(&wide), "mask must apply to the incoming value");
    let cx = b.ir.get_var(instr::Reg::CX, cur);
    let cx_instr = b.ir.instr(cx).expect("CX must be recomposed");
    assert_eq!(cx_instr.opcode, Opcode::Or);
    assert!(cx_instr.operands.contains(&cl), "recomposition must carry the masked LO");
  }

  #[test]
  fn calls_refresh_partial_register_views() {
    // Calls clobber AX/CX/DX: after save_return_value the fragment
    // views must be splits of the post-call value, not stale
    // pre-call versions (call f + mov [bx],al must store the result).
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    let stale_lo = b.ir.const_new(0x11);
    b.ir.set_var(instr::Reg::AL, cur, stale_lo);
    let ret = b.ir.const_new(0x3344);
    b.save_return_value(&Type::U16, ret);
    let ax = b.ir.get_var(instr::Reg::AX, cur);
    assert_eq!(ax, ret);
    let al = b.ir.get_var(instr::Reg::AL, cur);
    let al_instr = b.ir.instr(al).expect("AL must be refreshed post-call, not stale");
    assert_eq!(al_instr.opcode, Opcode::Lower16);
    assert_eq!(al_instr.operands, vec![ret]);
    let ah = b.ir.get_var(instr::Reg::AH, cur);
    let ah_instr = b.ir.instr(ah).expect("AH must be refreshed post-call, not stale");
    assert_eq!(ah_instr.opcode, Opcode::Upper16);
    assert_eq!(ah_instr.operands, vec![ret]);
  }

  #[test]
  fn int_materializes_register_setup_and_resets_clobbers() {
    // Issue #66: an INT handler may read registers, so their live values must
    // become pinned guest writes (keeping the setup alive through DCE), and
    // AX/FLAGS must be invalidated afterwards.
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;

    // mov ah, 9 (recomposes AX) and mov bx, 0xbeef, plus a flags producer so
    // the pre-INT flags are not the initial value.
    let nine = b.ir.const_new(9);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AH), nine);
    let beef = b.ir.const_new(0xbeefu16 as i16);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::BX), beef);
    let k = b.ir.const_new(0x1234);
    b.append_update_flags(k);
    assert_ne!(b.ir.get_var(instr::Reg::FLAGS, cur), Ref::Init(instr::Reg::FLAGS));

    b.append_asm_instr(&int_instr(0x21));

    // Clobbers reset to the initial/unknown marker: every consumed GPR, the
    // byte fragments, and FLAGS.
    for reg in [instr::Reg::AX, instr::Reg::BX, instr::Reg::CX, instr::Reg::DX,
                instr::Reg::SI, instr::Reg::DI, instr::Reg::DS, instr::Reg::ES,
                instr::Reg::AL, instr::Reg::AH, instr::Reg::BL, instr::Reg::BH,
                instr::Reg::CL, instr::Reg::CH, instr::Reg::DL, instr::Reg::DH,
                instr::Reg::FLAGS] {
      assert_eq!(b.ir.get_var(reg, cur), Ref::Init(reg), "{reg:?} must be unknown after INT");
    }

    // The setup is materialized as pinned guest register writes.
    let mut writes = vec![];
    for r in b.ir.iter_instrs(cur) {
      let i = b.ir.instr(r).unwrap();
      if i.opcode != Opcode::WriteVar16 { continue; }
      if let Ref::Symbol(s) = i.operands[0] {
        let name = s.name(&b.ir.symbols);
        if name == "AX" || name == "BX" {
          writes.push((name.to_string(), r));
        }
      }
    }
    assert!(writes.iter().any(|(n, _)| n == "AX"), "INT must write the AX setup to the guest register");
    assert!(writes.iter().any(|(n, _)| n == "BX"), "INT must write the BX setup to the guest register");

    // And those writes survive DCE: this is the #66 bug.
    crate::decompile::opt::deadcode_elimination(&mut b.ir);
    for (name, r) in &writes {
      assert_ne!(b.ir.instr(*r).unwrap().opcode, Opcode::Nop, "{name} guest write must survive DCE");
    }
  }

  fn jmp_instr(opcode: instr::Opcode, addr_off: u16, n_bytes: u16, rel: u16) -> instr::Instr {
    let mut operands = ArrayVec::new();
    operands.push(instr::Operand::Rel(instr::OperandRel { val: rel }));
    instr::Instr {
      rep: None,
      opcode,
      operands,
      addr: SegOff { seg: Seg::Normal(0), off: Off(addr_off) },
      n_bytes,
      intel_hidden_operand_bitmask: 0,
    }
  }

  fn test_spec(start: u16, end: u16, code: &[u8]) -> (Rc<TypeDatabase>, Config, binary::Binary, spec::Spec<'static>) {
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(code, None);
    let spec = spec::Spec::from_start_and_end(
      Some(SegOff { seg: Seg::Normal(0), off: Off(start) }),
      Some(SegOff { seg: Seg::Normal(0), off: Off(end) }));
    (types, cfg, binary, spec)
  }

  #[test]
  fn target_in_range_accepts_start_and_rejects_end_and_other_seg() {
    let (_types, cfg, binary, spec) = test_spec(0x10, 0x20, &[0xC3]);
    let b = test_builder(&cfg, &[], &spec, &binary);
    let in_range = SegOff { seg: Seg::Normal(0), off: Off(0x10) };
    let mid = SegOff { seg: Seg::Normal(0), off: Off(0x1A) };
    let end = SegOff { seg: Seg::Normal(0), off: Off(0x20) };
    let past = SegOff { seg: Seg::Normal(0), off: Off(0x21) };
    let other_seg = SegOff { seg: Seg::Normal(1), off: Off(0x12) };
    assert!(b.target_in_range(in_range), "range start is inclusive");
    assert!(b.target_in_range(mid));
    assert!(!b.target_in_range(end), "range end is exclusive");
    assert!(!b.target_in_range(past));
    assert!(!b.target_in_range(other_seg), "far targets are outside the range");
  }

  #[test]
  fn out_of_range_edge_skips_fallthrough_targets() {
    // Warn/skip matrix for the #67 diagnostic: only genuinely dropped
    // edges warn. A branch-to-next (`jz $+2` padding) links both arms to
    // the existing fallthrough block, so it stays silent even at end.
    let (_types, cfg, binary, spec) = test_spec(0x10, 0x20, &[0xC3]);
    let b = test_builder(&cfg, &[], &spec, &binary);
    let seg = Seg::Normal(0);
    let at = |off| SegOff { seg, off: Off(off) };
    assert!(!b.out_of_range_edge(at(0x10), None), "in-range start is silent");
    assert!(!b.out_of_range_edge(at(0x1A), Some(at(0x1B))), "in-range target is silent");
    assert!(b.out_of_range_edge(at(0x20), None), "end-exclusive target warns");
    assert!(b.out_of_range_edge(at(0x25), Some(at(0x12))), "out-of-range target warns");
    assert!(!b.out_of_range_edge(at(0x20), Some(at(0x20))), "branch-to-next at end is silent");
    assert!(!b.out_of_range_edge(at(0x15), Some(at(0x15))), "branch-to-next mid-range is silent");
    assert!(b.out_of_range_edge(SegOff { seg: Seg::Normal(1), off: Off(0x12) }, None), "far target warns");
  }

  #[test]
  fn branch_to_next_at_end_links_both_arms_silently() {
    // `jz $+2` padding as the last instruction: the explicit target
    // equals the fallthrough (spec.end), so no edge is dropped — both
    // arms link to the one fallthrough block.
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x0004, &[0xC3]);
    // JNE at 0x0002, 2 bytes: end 0x0004; rel 0 -> target == fallthrough.
    let instrs = [jmp_instr(instr::Opcode::OP_JNE, 0x0002, 2, 0)];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    let fall = SegOff { seg: Seg::Normal(0), off: Off(0x0004) };
    let blk = *b.addrmap.get(&fall).expect("fallthrough block exists");
    assert_eq!(b.ir.block(blk).preds.len(), 2, "both arms link to the fallthrough block");
  }

  #[test]
  fn out_of_range_branch_target_warns_but_still_builds() {
    // #67: `jmp 0x000a` from inside [0x0000, 0x0009) decodes to zero
    // instructions, so its block is empty and the edge is dropped from
    // the output. build() must now WARN (stderr, exercised here) while
    // preserving the drop: no panic, empty target block, no exits.
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x0009, &[0xC3]);
    // JMP at 0x0006, 3 bytes: end 0x0009 + rel 1 -> target 0x000a.
    let instrs = [jmp_instr(instr::Opcode::OP_JMP, 0x0006, 3, 1)];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    assert!(!b.target_in_range(SegOff { seg: Seg::Normal(0), off: Off(0x000a) }));
    b.build();
    let tgt = SegOff { seg: Seg::Normal(0), off: Off(0x000a) };
    let blk = *b.addrmap.get(&tgt).expect("out-of-range target still gets a block");
    assert_eq!(b.ir.block_instr_count(blk), 0, "target block must stay empty (drop semantics preserved)");
    assert!(b.ir.block_exits(blk).is_empty());
  }

  #[test]
  fn in_range_branch_target_builds_without_warning_path() {
    // Control: a JMP whose target lies inside the range takes the same
    // code path with target_in_range true and links the edge normally.
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x0008, &[0xC3]);
    // JMP at 0x0000, 2 bytes: rel 2 -> target 0x0004.
    let instrs = [jmp_instr(instr::Opcode::OP_JMP, 0x0000, 2, 2)];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    assert!(b.target_in_range(SegOff { seg: Seg::Normal(0), off: Off(0x0004) }));
    b.build();
    let tgt = SegOff { seg: Seg::Normal(0), off: Off(0x0004) };
    let blk = *b.addrmap.get(&tgt).expect("in-range target gets a block");
    // The single JMP ran in the entry block (b.cur never moved), so the
    // target's sole predecessor must be the entry block.
    assert_eq!(b.ir.block(blk).preds, vec![b.cur], "in-range edge links normally");
  }

  #[test]
  fn decoded_out_of_range_jmp_warns_and_builds() {
    // The #67 shape through the real decoder: `mov ax,1; jmp 0x000a`
    // with the function range ending at 0x0006. The WARN fires on
    // stderr (exercised here, not asserted); the build and a full
    // optimize must complete without panicking.
    use crate::asm::decode::Decoder;
    let code: &[u8] = &[0xB8, 0x01, 0x00, 0xE9, 0x04, 0x00];
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x0006, code);
    let mut decoder = Decoder::new(binary.region_iter(spec.start, spec.end));
    let mut instr_list: Vec<instr::Instr> = vec![];
    while let Some((ins, _raw)) = decoder.try_next().expect("decode") {
      instr_list.push(ins);
    }
    assert_eq!(instr_list.len(), 2);
    assert_eq!(instr_list[1].opcode, instr::Opcode::OP_JMP);
    let instr::Operand::Rel(rel) = instr_list[1].operands[0] else {
      panic!("JMP must carry a relative target");
    };
    assert_eq!(instr_list[1].rel_addr(&rel),
      SegOff { seg: Seg::Normal(0), off: Off(0x000a) }, "repro shape: jmp targets 0x000a");
    let mut ir = build_from_instrs(&instr_list, &cfg, &spec, &binary, false, false);
    crate::decompile::opt::optimize(&mut ir);
    // The dropped edge still materializes as exactly one empty exit-less
    // block (the 0x000a stub); the mov lowering keeps entry non-empty.
    let empties: Vec<_> = ir.iter_blocks()
      .filter(|b| ir.block_instr_count(*b) == 0 && ir.block_exits(*b).is_empty())
      .collect();
    assert_eq!(empties.len(), 1, "exactly one empty exit-less block (the dropped 0x000a edge)");
  }

  #[test]
  fn trailing_conditional_branch_fallthrough_still_gets_a_block() {
    // Regression: excluding the fallthrough from block_start made
    // get_block(ins.end_addr()) panic when a conditional branch is the
    // last decoded instruction. The fallthrough (here spec.end, a normal
    // function exit) must still get a block, silently.
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x0004, &[0xC3]);
    // JNE at 0x0002, 2 bytes: end 0x0004 == spec.end; rel wraps to target 0x0000.
    let instrs = [jmp_instr(instr::Opcode::OP_JNE, 0x0002, 2, 0xFFFC)];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    let fall = SegOff { seg: Seg::Normal(0), off: Off(0x0004) };
    let blk = *b.addrmap.get(&fall).expect("fallthrough at spec.end still gets a block");
    assert_eq!(b.ir.block_instr_count(blk), 0, "fallthrough block is empty");
    assert!(b.ir.block_exits(blk).is_empty());
  }

  fn rotate_instr(opcode: instr::Opcode, dst: instr::Reg, count: instr::Operand) -> instr::Instr {
    let mut operands = ArrayVec::new();
    operands.push(instr::Operand::Reg(instr::OperandReg(dst)));
    operands.push(count);
    instr::Instr {
      rep: None,
      opcode,
      operands,
      addr: SegOff { seg: Seg::Normal(0), off: Off(0) },
      n_bytes: 3,
      intel_hidden_operand_bitmask: 0,
    }
  }

  fn imm_count(val: u16) -> instr::Operand {
    instr::Operand::Imm(instr::OperandImm { sz: instr::Size::Size8, val })
  }

  fn bit_test_builder(code: &[u8]) -> (Rc<TypeDatabase>, Config, binary::Binary, spec::Spec<'static>) {
    test_spec(0x0000, code.len() as u16, code)
  }

  /// Collect the single instruction with `opcode` in `blk`.
  fn only_instr(ir: &IR, blk: BlockRef, opcode: Opcode) -> Ref {
    let found: Vec<Ref> = ir.iter_instrs(blk)
      .filter(|r| ir.instr(*r).unwrap().opcode == opcode)
      .collect();
    assert_eq!(found.len(), 1, "expected exactly one {opcode:?} in block");
    found[0]
  }

  #[test]
  fn rol_imm_lowers_to_masked_shifts_or() {
    // #69 slice 1: `rol ax,1` is value-exact as (v<<n)|(v>>(w-n))
    // with n = c & (w-1). Flags are clobbered opaque so a following
    // branch degrades to loud UNIMPL_FLAGS, never silent.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    let ins = rotate_instr(instr::Opcode::OP_ROL, instr::Reg::AX, imm_count(1));
    b.append_asm_instr(&ins);

    let or = only_instr(&b.ir, cur, Opcode::Or);
    let or_len = b.ir.instr(or).unwrap().operands.len();
    let shl = b.ir.instr(b.ir.instr(or).unwrap().operands[0]).unwrap();
    let ushr = b.ir.instr(b.ir.instr(or).unwrap().operands[1]).unwrap();
    assert_eq!(shl.opcode, Opcode::Shl, "ROL leads with Shl");
    assert_eq!(ushr.opcode, Opcode::UShr, "ROL trails with UShr");
    // Both shifts read the initial AX. The count is masked twice: n
    // masks the raw count, m masks the complementary shift so a zero
    // count yields (v<<0)|(v>>0) without a shift-by-width.
    assert_eq!(shl.operands[0], Ref::Init(instr::Reg::AX));
    assert_eq!(ushr.operands[0], Ref::Init(instr::Reg::AX));
    let ands: Vec<Ref> = b.ir.iter_instrs(cur)
      .filter(|r| b.ir.instr(*r).unwrap().opcode == Opcode::And)
      .collect();
    assert_eq!(ands.len(), 2, "count mask plus complementary-shift mask");
    let n = *ands.iter().find(|r| {
      let i = b.ir.instr(**r).unwrap();
      b.ir.const_lookup(i.operands[0]) == Some(1) && b.ir.const_lookup(i.operands[1]) == Some(15)
    }).expect("count mask And [1, 15] present");
    let m = *ands.iter().find(|r| **r != n).expect("second And present");
    let m_instr = b.ir.instr(m).unwrap();
    let sub = b.ir.instr(m_instr.operands[0]).unwrap();
    assert_eq!(sub.opcode, Opcode::Sub);
    assert_eq!(b.ir.const_lookup(sub.operands[0]), Some(16));
    assert_eq!(sub.operands[1], n, "complementary shift derives from the masked count");
    assert_eq!(b.ir.const_lookup(m_instr.operands[1]), Some(15), "complementary shift is masked too");
    assert_eq!(shl.operands[1], n);
    assert_eq!(ushr.operands[1], m);
    // Destination and flags.
    assert_eq!(b.ir.get_var(instr::Reg::AX, cur), or, "ROL writes the Or back to AX");
    let f = b.get_flags();
    let flags = b.ir.instr(f).unwrap();
    assert_eq!(flags.opcode, Opcode::UpdateFlags);
    let marker = b.ir.instr(flags.operands[1]).unwrap();
    assert_eq!(marker.opcode, Opcode::Unimpl);
    assert!(marker.operands.is_empty(), "flags clobber is an operand-less marker");
  }

  #[test]
  fn ror_cl_count_uses_byte_width_reg_count() {
    // `ror bl,cl`: 8-bit width (mask 7, w 8), CL flows in as the count
    // value, and the Or leads with UShr for ROR.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    let cl = instr::Operand::Reg(instr::OperandReg(instr::Reg::CL));
    let ins = rotate_instr(instr::Opcode::OP_ROR, instr::Reg::BL, cl);
    b.append_asm_instr(&ins);

    // Byte-dest writes recompose the full register, which adds its own
    // Or/And shape to the block: pick the rotate instrs by operand
    // pattern, not by count, so the test is robust to
    // recomposition/CSE changes.
    let ors: Vec<Ref> = b.ir.iter_instrs(cur)
      .filter(|r| b.ir.instr(*r).unwrap().opcode == Opcode::Or)
      .collect();
    let or = *ors.iter().find(|r| {
      let i = b.ir.instr(**r).unwrap();
      b.ir.instr(i.operands[0]).unwrap().opcode == Opcode::UShr
        && b.ir.instr(i.operands[1]).unwrap().opcode == Opcode::Shl
    }).expect("rotate Or [UShr, Shl] present");
    let or_len = b.ir.instr(or).unwrap().operands.len();
    // The BX recomposition masks with its own And (0xFF): pick the
    // rotate And by its count operand and mask const.
    let and = b.ir.iter_instrs(cur)
      .filter(|r| b.ir.instr(*r).unwrap().opcode == Opcode::And)
      .find(|r| {
        let i = b.ir.instr(*r).unwrap();
        i.operands[0] == Ref::Init(instr::Reg::CL) && b.ir.const_lookup(i.operands[1]) == Some(7)
      })
      .expect("rotate And [CL, 7] present");
    let and_instr = b.ir.instr(and).unwrap();
    assert_eq!(b.ir.const_lookup(and_instr.operands[1]), Some(7), "8-bit rotate masks with 7");
    let sub = only_instr(&b.ir, cur, Opcode::Sub);
    assert_eq!(b.ir.const_lookup(b.ir.instr(sub).unwrap().operands[0]), Some(8));
    // The destination read flows through the recomposition to the rotate:
    // BL reads as the masked And over the recomposed BX, whose other
    // half carries the rotate Or.
    let bl_var = b.ir.get_var(instr::Reg::BL, cur);
    let bl = b.ir.instr(bl_var).unwrap();
    assert_eq!(bl.opcode, Opcode::And);
    assert_eq!(b.ir.const_lookup(bl.operands[1]), Some(0xFF));
    assert_eq!(bl.operands[0], or, "BL carries the rotate Or");
    assert_eq!(or_len, 2);
  }

  #[test]
  fn rcl_stays_unimpl_pending_carry_slice() {
    // RCL/RCR rotate through carry, which needs CF dataflow (a later
    // #69 slice): pin the deferred behavior so the split is explicit.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    let ins = rotate_instr(instr::Opcode::OP_RCL, instr::Reg::AX, imm_count(1));
    b.append_asm_instr(&ins);

    let unimpl = only_instr(&b.ir, cur, Opcode::Unimpl);
    assert_eq!(b.ir.get_var(instr::Reg::AX, cur), unimpl, "RCL still yields Unimpl");
    assert!(b.ir.iter_instrs(cur).all(|r| b.ir.instr(r).unwrap().opcode != Opcode::Or),
      "RCL must not use the exact ROL/ROR shape");
  }

  fn string_mem(sz: instr::Size, sreg: instr::Reg, ptr: instr::Reg) -> instr::Operand {
    instr::Operand::Mem(instr::OperandMem { sz, sreg, reg1: Some(ptr), reg2: None, off: None })
  }

  fn string_instr(opcode: instr::Opcode, operands: Vec<instr::Operand>) -> instr::Instr {
    let mut o = ArrayVec::new();
    for oper in operands { o.push(oper); }
    instr::Instr {
      rep: None,
      opcode,
      operands: o,
      addr: SegOff { seg: Seg::Normal(0), off: Off(0) },
      n_bytes: 1,
      intel_hidden_operand_bitmask: 0,
    }
  }

  /// The `Add` ref that `get_var(reg)` resolves to, if it is the
  /// string-op increment by `size`.
  fn assert_advanced(b: &mut IRBuilder<'_>, reg: instr::Reg, size: i16) {
    let v = b.ir.get_var(reg, b.cur);
    let i = b.ir.instr(v).expect("pointer must be a computed value");
    assert_eq!(i.opcode, Opcode::Add, "{reg:?} must be advanced by Add");
    assert_eq!(i.operands[0], Ref::Init(reg), "{reg:?} advances from its incoming value");
    assert_eq!(b.ir.const_lookup(i.operands[1]), Some(size), "{reg:?} must advance by the element size");
  }

  #[test]
  fn movs_advances_si_and_di_by_element_size() {
    // `movsw`: the silent-wrongness fix (#69 slice 2). Without the update
    // a hand-written movs loop copies the same byte forever.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let ins = string_instr(instr::Opcode::OP_MOVS, vec![
      string_mem(instr::Size::Size16, instr::Reg::ES, instr::Reg::DI),
      string_mem(instr::Size::Size16, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);
    assert_advanced(&mut b, instr::Reg::SI, 2);
    assert_advanced(&mut b, instr::Reg::DI, 2);
  }

  #[test]
  fn movsb_advances_by_one() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let ins = string_instr(instr::Opcode::OP_MOVS, vec![
      string_mem(instr::Size::Size8, instr::Reg::ES, instr::Reg::DI),
      string_mem(instr::Size::Size8, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);
    assert_advanced(&mut b, instr::Reg::SI, 1);
    assert_advanced(&mut b, instr::Reg::DI, 1);
  }

  #[test]
  fn stos_advances_only_di() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let ins = string_instr(instr::Opcode::OP_STOS, vec![
      string_mem(instr::Size::Size16, instr::Reg::ES, instr::Reg::DI),
      instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)),
    ]);
    b.append_asm_instr(&ins);
    assert_advanced(&mut b, instr::Reg::DI, 2);
    assert_eq!(b.ir.get_var(instr::Reg::SI, b.cur), Ref::Init(instr::Reg::SI), "STOS must not touch SI");
  }

  #[test]
  fn lods_advances_only_si() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let ins = string_instr(instr::Opcode::OP_LODS, vec![
      instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)),
      string_mem(instr::Size::Size16, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);
    assert_advanced(&mut b, instr::Reg::SI, 2);
    assert_eq!(b.ir.get_var(instr::Reg::DI, b.cur), Ref::Init(instr::Reg::DI), "LODS must not touch DI");
  }

  #[test]
  fn scas_advances_only_di() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let ins = string_instr(instr::Opcode::OP_SCAS, vec![
      instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)),
      string_mem(instr::Size::Size16, instr::Reg::ES, instr::Reg::DI),
    ]);
    b.append_asm_instr(&ins);
    assert_advanced(&mut b, instr::Reg::DI, 2);
    assert_eq!(b.ir.get_var(instr::Reg::SI, b.cur), Ref::Init(instr::Reg::SI), "SCAS must not touch SI");
  }

  #[test]
  fn cmps_advances_si_and_di() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let ins = string_instr(instr::Opcode::OP_CMPS, vec![
      string_mem(instr::Size::Size16, instr::Reg::ES, instr::Reg::DI),
      string_mem(instr::Size::Size16, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);
    assert_advanced(&mut b, instr::Reg::SI, 2);
    assert_advanced(&mut b, instr::Reg::DI, 2);
  }

  #[test]
  fn string_op_with_std_degrades_loud_without_guessing_direction() {
    // DF may be set by a `std` elsewhere in the function, so the
    // direction is not provably clear: advance nothing and emit the
    // loud marker instead of a possibly-wrong-direction update.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.df_may_be_set = true;
    let ins = string_instr(instr::Opcode::OP_MOVS, vec![
      string_mem(instr::Size::Size16, instr::Reg::ES, instr::Reg::DI),
      string_mem(instr::Size::Size16, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);
    assert_eq!(b.ir.get_var(instr::Reg::SI, b.cur), Ref::Init(instr::Reg::SI), "must not guess the direction");
    assert_eq!(b.ir.get_var(instr::Reg::DI, b.cur), Ref::Init(instr::Reg::DI));
    let _ = only_instr(&b.ir, b.cur, Opcode::Unimpl);
  }

  #[test]
  fn build_marks_df_may_be_set_from_decoded_instructions() {
    // `std; ret` decoded: build() must set the guard so string ops in
    // this function degrade loud. Without std, the guard stays clear.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xFD, 0xC3]);
    let mut ins = string_instr(instr::Opcode::OP_STD, vec![]);
    ins.n_bytes = 1;
    let ret = string_instr(instr::Opcode::OP_RET, vec![]);
    let instrs = [ins, ret];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    assert!(b.df_may_be_set, "build must detect STD in the decoded function");
  }

  #[test]
  fn may_set_df_covers_std_popf_and_iret() {
    // STD sets DF; POPF/IRET restore it. If any of those gain lowering,
    // the string-op direction guard must already treat them as DF writers.
    assert!(opcode_may_set_df(instr::Opcode::OP_STD));
    assert!(opcode_may_set_df(instr::Opcode::OP_POPF));
    assert!(opcode_may_set_df(instr::Opcode::OP_IRET));
    assert!(!opcode_may_set_df(instr::Opcode::OP_CLD), "CLD clears DF");
    assert!(!opcode_may_set_df(instr::Opcode::OP_MOVS));
    assert!(!opcode_may_set_df(instr::Opcode::OP_RET));
  }

  fn rep_string_instr(opcode: instr::Opcode, operands: Vec<instr::Operand>) -> instr::Instr {
    let mut ins = string_instr(opcode, operands);
    ins.rep = Some(instr::Rep::EQ);
    ins
  }

  /// The `Add(dst, UMul(count, size))` value that `get_var(reg)` resolves
  /// to, and the count it multiplies.
  fn assert_rep_advanced(b: &mut IRBuilder<'_>, reg: instr::Reg, size: i16) -> Ref {
    let v = b.ir.get_var(reg, b.cur);
    let add = b.ir.instr(v).expect("pointer must be recomputed");
    assert_eq!(add.opcode, Opcode::Add, "{reg:?} must advance by Add");
    assert_eq!(add.operands[0], Ref::Init(reg));
    let mul = add.operands[1];
    let mul_i = b.ir.instr(mul).unwrap();
    assert_eq!(mul_i.opcode, Opcode::UMul, "advance is count*size");
    assert_eq!(b.ir.const_lookup(mul_i.operands[1]), Some(size));
    mul_i.operands[0]
  }

  #[test]
  fn rep_movs_models_cx_zero_and_pointer_advance() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cx = b.ir.const_new(5);
    b.ir.set_var(instr::Reg::CX, b.cur, cx);
    let ins = rep_string_instr(instr::Opcode::OP_MOVS, vec![
      string_mem(instr::Size::Size16, instr::Reg::ES, instr::Reg::DI),
      string_mem(instr::Size::Size16, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);

    let _ = only_instr(&b.ir, b.cur, Opcode::RepMovs);
    let cx_after = b.ir.get_var(instr::Reg::CX, b.cur);
    assert_eq!(b.ir.const_lookup(cx_after), Some(0), "CX must be consumed to 0");
    let si_mul = assert_rep_advanced(&mut b, instr::Reg::SI, 2);
    let di_mul = assert_rep_advanced(&mut b, instr::Reg::DI, 2);
    assert_eq!(si_mul, cx, "SI advance must be the pre-update count");
    assert_eq!(di_mul, cx, "DI advance must be the pre-update count");
  }

  #[test]
  fn rep_stos_models_di_advance_without_touching_si() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cx = b.ir.const_new(3);
    b.ir.set_var(instr::Reg::CX, b.cur, cx);
    let ins = rep_string_instr(instr::Opcode::OP_STOS, vec![
      string_mem(instr::Size::Size16, instr::Reg::ES, instr::Reg::DI),
      instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)),
    ]);
    b.append_asm_instr(&ins);

    let _ = only_instr(&b.ir, b.cur, Opcode::RepStos);
    let cx_after = b.ir.get_var(instr::Reg::CX, b.cur);
    assert_eq!(b.ir.const_lookup(cx_after), Some(0));
    let di_mul = assert_rep_advanced(&mut b, instr::Reg::DI, 2);
    assert_eq!(di_mul, cx);
    assert_eq!(b.ir.get_var(instr::Reg::SI, b.cur), Ref::Init(instr::Reg::SI), "STOS must not touch SI");
  }

  #[test]
  fn rep_with_possible_std_degrades_loud() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.df_may_be_set = true;
    let cx = b.ir.const_new(5);
    b.ir.set_var(instr::Reg::CX, b.cur, cx);
    let ins = rep_string_instr(instr::Opcode::OP_MOVS, vec![
      string_mem(instr::Size::Size16, instr::Reg::ES, instr::Reg::DI),
      string_mem(instr::Size::Size16, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);

    let _ = only_instr(&b.ir, b.cur, Opcode::Unimpl);
    assert!(b.ir.iter_instrs(b.cur).all(|r| b.ir.instr(r).unwrap().opcode != Opcode::RepMovs),
      "must not emit the assumed-direction macro");
    assert_eq!(b.ir.get_var(instr::Reg::CX, b.cur), cx, "CX must be left untouched");
  }

  #[test]
  fn rep_lods_stays_loud() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let ins = rep_string_instr(instr::Opcode::OP_LODS, vec![
      instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)),
      string_mem(instr::Size::Size16, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);

    let _ = only_instr(&b.ir, b.cur, Opcode::Unimpl);
  }
}
