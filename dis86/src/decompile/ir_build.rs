use crate::decompile::sym;
use crate::decompile::ir::*;
use crate::asm::instr;
use crate::asm::branch;
use crate::binary;
use crate::segoff::{Seg, Off, SegOff};
use crate::config::{self, Config};
use crate::diagnostics::{Cause, Diagnostic};
use crate::spec;
use crate::types::{Type, ArraySize};
use crate::asm::intel_syntax::instr_str;
use std::collections::{HashSet, HashMap};

const DEBUG: bool = false;

/// The destination of a branch instruction, as the IR builder needs it.
enum BranchTargets {
  /// Explicit targets plus, for branches that fall through, the implicit
  /// fallthrough address. The two are kept separate (rather than
  /// "fallthrough is last") so callers cannot range-check the wrong
  /// subset.
  Targets { targets: Vec<SegOff>, fallthrough: Option<SegOff> },
  /// A transfer with no statically known target: the block ends there.
  Unresolved(branch::UnresolvedTransfer),
}

#[derive(Debug, Clone, Copy)]
struct MarkerSite {
  addr: SegOff,
  opcode: instr::Opcode,
}

/// Source sites for IR instructions that render as the non-fatal
/// `UNIMPL_FLAGS` marker. Refs are stable across optimization, so scanning
/// the finalized IR against this map reports only markers that survived.
/// Reader sites name the instruction that reads flags; `UpdateFlags` sites
/// name the flag-writing instruction.
#[derive(Debug, Default)]
pub struct FlagMarkers {
  sites: HashMap<Ref, MarkerSite>,
}

impl FlagMarkers {
  fn record(&mut self, r: Ref, site: MarkerSite) {
    self.sites.insert(r, site);
  }

  pub fn diagnostics(&self, ir: &IR) -> Vec<Diagnostic> {
    let mut out = vec![];
    for blk in ir.iter_blocks() {
      for r in ir.iter_instrs(blk) {
        let Some(instr) = ir.instr(r) else { continue; };
        if !instr.opcode.is_flag_marker() { continue; }
        let site = self.sites.get(&r).copied();
        let (addr, opcode, message) = match site {
          Some(site) => (Some(site.addr), Some(site.opcode),
                         format!("UNIMPL_FLAGS at {} ({:?})", site.addr, site.opcode)),
          None => (None, None, "UNIMPL_FLAGS at unknown site".to_string()),
        };
        out.push(Diagnostic::new(Cause::UnimplFlags, addr, opcode, message));
      }
    }
    out.sort_by_key(|diagnostic| diagnostic.addr);
    out
  }
}

/// The outcome of resolving an indirect jump's target.
enum IndirectTargets {
  /// The config describes the jump table: these are its targets.
  Resolved(Vec<SegOff>),
  /// The jump is not of the config-described jump-table shape, so its
  /// target is only known at run time.
  NotTableShape,
  /// The jump is of that shape, but the config has no table for it: a
  /// missing annotation, not a run-time-only target.
  NoConfigRegion,
  /// The matching text-section region is declared data, not a jump table.
  DataRegion { name: String, start: SegOff, end: SegOff },
}

/// How a transfer that is not a relative branch is modeled.
enum Transfer {
  /// A far jump inside the function's own code segment: CS does not
  /// change, so it is a plain jump to `target` (which may sit outside
  /// the function's range).
  SameSegment { target: SegOff },
  /// A transfer the IR cannot express: the block ends here.
  Unmodelled,
}

fn operand_is_stack_reg(operand: &instr::Operand) -> bool {
  let instr::Operand::Reg(instr::OperandReg(reg)) = operand else { return false };
  reg == &instr::Reg::SP || reg == &instr::Reg::BP
}

/// Whether an instruction may leave the direction flag set: STD sets it
/// explicitly, and POPF/IRET restore it from a popped flags value. Both are
/// lowered to an opaque flags update and clear the per-point `df_clear`
/// state, so a later string op's direction is unknown. (RETF pops CS:IP only
/// — not flags — so it is correctly absent.)
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
  code_regions: Vec<config::FuncRegion>,
  binary: &'a binary::Binary,

  ir: IR,
  addrmap: HashMap<SegOff, BlockRef>,
  cur: BlockRef,
  special: Option<SpecialState>,

  overlay: bool,
  pin_all: bool,

  // Whether DF is provably clear (DF=0, the DOS ABI entry/exit convention)
  // at the current point of the build. CLD sets it, STD clears it, and
  // POPF (whose popped value is opaque) clears it. Block entry state comes
  // from the must-analysis in `build` (see `df_clear_entry`): an annotated
  // function entry assumes DF=0, and a join is clear only when every
  // predecessor leaves it clear.
  // String-op pointer updates advance only while this holds; otherwise
  // they degrade to a loud marker rather than silently running the wrong
  // way. The marker stays fatal (UNIMPL, not UNIMPL_FLAGS) deliberately:
  // the imprecision is in the SI/DI pointer *values*, not just the flags,
  // so the non-fatal flags marker would understate it.
  df_clear: bool,

  // DF entry clearness per block, converged by the must-analysis in `build`
  // and cached for the retained build. `df_exit_clear` records, from the most
  // recent pass, whether each block provably leaves DF clear; the two drive
  // the analysis. `df_entry_default` is the fallback for blocks absent from
  // `df_clear_entry` (false in the analysis probes, true when no DF writer
  // exists anywhere).
  df_clear_entry: HashMap<BlockRef, bool>,
  df_exit_clear: HashMap<BlockRef, bool>,
  df_entry_default: bool,

  // Carry-out value (a 0/1 u16 Ref) of the most recent flag-writing
  // instruction in the current basic block, or None when the value has not
  // been materialized yet. `carry_known` says whether a carry is currently
  // known at all; when it is set but `last_cf` is None the value lives in the
  // CF SSA variable and is pulled on first use (see `get_cf`). Producers set
  // both, and any flag writer that does not model its carry clears them.
  last_cf: Option<Ref>,
  carry_known: bool,

  // Carry (CF) is threaded across basic blocks as a real SSA value under a
  // synthetic `Name::Var`, so the phi machinery merges it at control-flow
  // joins. Only the carry-out *definition* is stored here; whether a given
  // block's entry carry is provably known is computed by a must-analysis
  // (see `build`) and cached in `carry_known_entry`. `exit_known` records,
  // from the most recent build pass, whether each block provably leaves a
  // known carry; the two drive the analysis.
  carry_known_entry: HashMap<BlockRef, bool>,
  exit_known: HashMap<BlockRef, bool>,

  // Fallback entry-knownness for blocks absent from `carry_known_entry`.
  // `build` runs one probe pass with this false and one with it true to
  // extract each block's carry transfer; the retained build leaves it false.
  entry_known_default: bool,

  // Diagnostics produced during a build pass. A pass is speculative (it runs
  // two or three times to fixpoint the carry analysis), so messages are
  // buffered and only the retained pass's warnings are printed, exactly once.
  warnings: Vec<Diagnostic>,

  // Live flag-marker origins, keyed by the marker instruction's stable Ref.
  flag_markers: FlagMarkers,
  cur_site: Option<MarkerSite>,

  // Known calls whose undeclared-but-dirty registers may be callee inputs.
  // Recorded while building; checked once against the finished IR in `build`
  // (see `check_regargs_setups`), so each call site warns at most once.
  regargs_candidates: Vec<RegargsCandidate>,

  // Return instructions awaiting their exit register write-back, with the
  // register values captured at the ret. Recorded while lowering and
  // materialised by `write_back_exit_registers` after `build_once` has sealed
  // every block: before sealing, `get_var` hands out incomplete phis, so an
  // untouched register looks dirty and a return value read through such a phi
  // hides that it reads the register. Capturing the values at the ret (rather
  // than re-reading the block afterwards) keeps instructions decoded after a
  // ret, which never execute, out of the exit state.
  ret_exits: Vec<RetExit>,
}

/// A return instruction and the register values live at it. The refs may be
/// incomplete phis; sealing populates them before `write_back_exit_registers`
/// tests them. See `ret_exits`.
struct RetExit {
  ret_ref: Ref,
  pending: Vec<(instr::Reg, Ref)>,
}

/// A known call with registers that hold a non-entry value but are absent from
/// the callee's `regargs` annotation. `missing` pairs the register with the SSA
/// value live at the call so the check can tell whether the caller reads it
/// again. See `check_regargs_setups` (issue #132).
#[derive(Clone)]
struct RegargsCandidate {
  blk: BlockRef,
  call_ref: Ref,
  call_addr: SegOff,
  callee: String,
  missing: Vec<(instr::Reg, Ref)>,
}

/// The SSA variable name under which CF is threaded across blocks. It renders
/// as a readable C local; it cannot collide with a register (those use
/// `Name::Reg`).
fn carry_var() -> Name {
  Name::Var("carry".to_string())
}

impl<'a> IRBuilder<'a> {
  fn new(cfg: &'a Config, instrs: &'a [instr::Instr], spec: &'a spec::Spec, binary: &'a binary::Binary,
         overlay: bool, pin_all: bool) -> Self {
    let mut this = Self {
      instrs,
      cfg,
      spec,
      code_regions: cfg.code_regions(&spec.regions),
      binary,

      ir: IR::new(cfg.types.clone()),
      addrmap: HashMap::new(),
      cur: BlockRef(0),
      special: None,

      overlay,
      pin_all,

      df_clear: true,
      df_clear_entry: HashMap::new(),
      df_exit_clear: HashMap::new(),
      df_entry_default: true,
      last_cf: None,
      carry_known: false,
      carry_known_entry: HashMap::new(),
      exit_known: HashMap::new(),
      entry_known_default: false,
      warnings: Vec::new(),
      flag_markers: FlagMarkers::default(),
      cur_site: None,
      regargs_candidates: Vec::new(),
      ret_exits: Vec::new(),
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

  fn jump_indirect_targets(&self, ins: &instr::Instr, m: &instr::OperandMem) -> IndirectTargets {
    // Matching jumps of the form: "jmp WORD PTR cs:[bx+0x6d7]"
    if m.sz != instr::Size::Size16 { return IndirectTargets::NotTableShape; }
    if m.sreg != instr::Reg::CS    { return IndirectTargets::NotTableShape; }
    let Some(_) = m.reg1 else      { return IndirectTargets::NotTableShape; };
    if !m.reg2.is_none()           { return IndirectTargets::NotTableShape; };
    let Some(off) = m.off else     { return IndirectTargets::NotTableShape; };

    // Construct a segoff address to the memory operand
    let addr = SegOff {
      seg: self.spec.start.seg, // code segment
      off: Off(off),
    };

    // Try to find a matching text segment region in config. A jump of
    // this shape whose table the config does not describe has no known
    // target: it stays loud instead of aborting the decompile of the
    // whole function (issue #112). The malformed-config panics below
    // stay: those describe a broken config, not an unsupported input.
    let Some(region) = self.cfg.text_region_lookup(addr, ins.addr) else {
      return IndirectTargets::NoConfigRegion;
    };
    let Some(typ) = &region.typ else {
      return IndirectTargets::DataRegion {
        name: region.name.clone(),
        start: region.start,
        end: region.end,
      };
    };

    // Unpack the array type
    let Type::Array(basetype, ArraySize::Known(len)) = typ else {
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

    IndirectTargets::Resolved(tgts)
  }

  /// Branch targets of `ins`: the explicit targets plus, for branches
  /// that fall through, the implicit fallthrough address. The two are
  /// returned separately (rather than "fallthrough is last") so callers
  /// cannot range-check the wrong subset.
  ///
  /// A branch whose target is not a relative offset (a far jump, or an
  /// indirect jump with no resolvable jump table) has no address to
  /// return, so it is reported as `Unresolved` instead of panicking:
  /// aborting the build here lost the whole function (issue #112).
  fn jump_targets(&self, ins: &instr::Instr) -> Option<BranchTargets> {
    // Special handling for some indirect jumps: a `jmp cs:[bx+off]` whose
    // target table the config describes resolves to a set of targets.
    if ins.opcode == instr::Opcode::OP_JMP {
      if let Some(instr::Operand::Mem(m)) = ins.operands.get(0) {
        if let IndirectTargets::Resolved(targets) = self.jump_indirect_targets(ins, m) {
          return Some(BranchTargets::Targets { targets, fallthrough: None });
        }
        // Otherwise the target is only known at run time: unresolved.
      }
    }

    match branch::branch_target(ins)? {
      branch::BranchTarget::Relative { target, fallthrough } =>
        Some(BranchTargets::Targets { targets: vec![target], fallthrough }),
      branch::BranchTarget::Unresolved(kind) =>
        Some(BranchTargets::Unresolved(kind)),
    }
  }

  /// Classify a transfer whose target is not a relative offset.
  ///
  /// A far jump loads CS with an explicit segment value. It is a jump
  /// within the function's own code segment only when that segment is
  /// the one the function lives in. In an overlay the segment value
  /// belongs to the original program's address space, which this
  /// segment model does not represent, so every far jump there is
  /// unmodelled.
  ///
  /// The comparison is numeric: the encoded segment is a runtime segment
  /// value, and the function's segment is the one it was loaded into.
  /// That is the same assumption the rest of the decompiler makes about
  /// segment values (see `process_callf`), and it is why the overlay
  /// case is refused rather than compared.
  fn transfer_model(&self, kind: branch::UnresolvedTransfer) -> Transfer {
    let branch::UnresolvedTransfer::FarDirect { seg, off } = kind else {
      return Transfer::Unmodelled;
    };
    // Belt and braces: `overlay` is derived from the start address
    // being an overlay address, so the segment match below already
    // rejects it.
    if self.overlay { return Transfer::Unmodelled; }
    let Seg::Normal(cur_seg) = self.spec.start.seg else { return Transfer::Unmodelled; };
    if seg != cur_seg { return Transfer::Unmodelled; }
    Transfer::SameSegment { target: SegOff { seg: self.spec.start.seg, off: Off(off) } }
  }

  fn append_instr(&mut self, typ: Type, opcode: Opcode, operands: Vec<Ref>) -> Ref {
    self.append_instr_with_attrs(typ, Attribute::NONE, opcode, operands)
  }

  fn make_instr(typ: Type, attrs: u8, opcode: Opcode, operands: Vec<Ref>) -> Instr {
    Instr {
      typ,
      compare_width: None,
      attrs,
      opcode,
      operands,
    }
  }

  fn append_instr_with_attrs(&mut self, typ: Type, attrs: u8, opcode: Opcode, operands: Vec<Ref>) -> Ref {
    let instr = Self::make_instr(typ, attrs, opcode, operands);
    let r = self.ir.block_instr_append(self.cur, instr);
    self.record_flag_marker(r, opcode);
    r
  }

  fn insert_instr_before(&mut self, blk: BlockRef, before: Ref, typ: Type, attrs: u8,
                         opcode: Opcode, operands: Vec<Ref>) -> Ref {
    let instr = Self::make_instr(typ, attrs, opcode, operands);
    let r = self.ir.block_instr_insert_before(blk, before, instr);
    self.record_flag_marker(r, opcode);
    r
  }

  fn record_flag_marker(&mut self, r: Ref, opcode: Opcode) {
    if opcode.is_flag_marker() {
      if let Some(site) = self.cur_site {
        self.flag_markers.record(r, site);
      }
    }
  }

  fn append_jmp(&mut self, next: BlockRef) {
    self.ir.block_mut(next).preds.push(self.cur);
    self.append_instr(Type::Void, Opcode::Jmp, vec![Ref::Block(next)]);
  }

  /// Lower a transfer the IR cannot model: a loud marker plus the end of
  /// the block, with no fallthrough edge.
  ///
  /// The block ends with a jump to an empty block, the established idiom
  /// for "control leaves the function here" (see
  /// `ir::methods::block_exits`): an empty block has no exits, so the code
  /// after the transfer becomes an unreachable block and is dropped,
  /// instead of being modelled as a fallthrough that never happens.
  fn append_unmodelled_transfer(&mut self, ins: &instr::Instr, cause: Cause, msg: &str) {
    self.warn_ins(cause, ins, msg);
    self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
    let trap = self.new_block(&format!("transfer_out_{:04x}", ins.addr.off.0));
    self.append_jmp(trap);
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
    // Record whether the block we are leaving provably has a known carry and
    // a clear DF; the driver's must-analyses read these to seed successors.
    self.exit_known.insert(self.cur, self.carry_known);
    self.df_exit_clear.insert(self.cur, self.df_clear);
    self.cur = bref;
    // Carry survives the join only when every predecessor defines it: the
    // analysis proves that (all preds' exit known). Do NOT resolve the CF
    // SSA value here — get_var would prepend a phi and break the builder's
    // "new block is empty" invariant. `get_cf` pulls it on first use.
    self.last_cf = None;
    self.carry_known = self.carry_known_entry.get(&bref).copied().unwrap_or(self.entry_known_default);
    // DF survives the join only when every predecessor leaves it clear (the
    // `build` must-analysis proves that). A block absent from the converged
    // map uses the probe/default fallback.
    self.df_clear = self.df_clear_entry.get(&bref).copied().unwrap_or(self.df_entry_default);
  }

  /// Whether a branch target lies inside any configured function region.
  /// Anything outside (including another segment) decodes to zero
  /// instructions and is dropped from the output.
  fn target_in_range(&self, tgt: SegOff) -> bool {
    self.code_regions.iter().any(|region| region.contains(tgt))
  }

  /// Whether an explicit branch target deserves the out-of-range WARN:
  /// outside the function regions and not the branch's own fallthrough (a
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

  /// Start a block at a hard region boundary. Unlike an ordinary fallthrough
  /// block, no edge is added: the bytes between regions are not code, so
  /// control cannot fall through into the next region.
  fn start_next_region_blk(&mut self, next: SegOff) {
    let next_bref = self.get_block(next);
    self.switch_blk(next_bref);
    assert!(self.ir.block_instr_count(self.cur) == 0);
  }

  /// A region that spans `spec.start` is split so the entry chunk decodes
  /// first. The pre-start chunk is emitted later even though execution can
  /// fall through from it into the entry, so restore that contiguous edge
  /// unless the chunk already ends in a control transfer.
  fn link_prestart_fallthrough(&mut self, block: BlockRef) {
    let has_transfer = matches!(
      self.ir.block_last_instr(block).map(|ins| ins.opcode),
      Some(Opcode::Jmp | Opcode::Jne | Opcode::JmpTbl | Opcode::RetFar | Opcode::RetNear));
    if has_transfer { return; }
    let target = self.addrmap.get(&self.spec.start).copied().unwrap_or(BlockRef(0));
    if target == block { return; }
    self.ir.block_mut(target).preds.push(block);
    self.ir.block_instr_append(block,
      Self::make_instr(Type::Void, Attribute::NONE, Opcode::Jmp, vec![Ref::Block(target)]));
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
        // Byte fragments, not 32-bit halves: Upper16/Lower16 would leave
        // Upper16 == 0 and Lower16 == full value for a 16-bit parent.
        let (upper, lower) = self.append_byte_split(cur);
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

  /// The carry currently in effect, or None when it is unknown. When it is
  /// known only via a cross-block merge (`carry_known` set, value not yet
  /// materialized) the CF SSA value is fetched now, which may prepend a phi
  /// into the current block — hence this is deferred to first use rather
  /// than done at block entry.
  fn get_cf(&mut self) -> Option<Ref> {
    if self.last_cf.is_none() && self.carry_known {
      let cf = self.ir.get_var(carry_var(), self.cur);
      self.last_cf = Some(cf);
    }
    self.last_cf
  }

  /// Record a known carry-out and publish it as the CF SSA value.
  fn set_carry_out(&mut self, cf: Ref) {
    self.last_cf = Some(cf);
    self.carry_known = true;
    self.ir.set_var(carry_var(), self.cur, cf);
  }

  /// The carry is no longer known (a flag writer we do not model ran).
  fn clear_carry(&mut self) {
    self.last_cf = None;
    self.carry_known = false;
  }

  /// Buffer a diagnostic for the current build pass. The carry analysis runs
  /// the build speculatively, so warnings are collected per pass and only the
  /// retained pass's are emitted (see `build`/`emit_warnings`).
  fn warn(&mut self, cause: Cause, addr: Option<SegOff>, opcode: Option<instr::Opcode>,
          msg: impl Into<String>) {
    self.warnings.push(Diagnostic::new(cause, addr, opcode, msg));
  }

  fn warn_ins(&mut self, cause: Cause, ins: &instr::Instr, msg: impl Into<String>) {
    self.warn(cause, Some(ins.addr), Some(ins.opcode), msg);
  }

  fn append_update_flags(&mut self, vref: Ref) {
    let old_flags = self.get_flags();
    let new_flags = self.append_instr(Type::U16, Opcode::UpdateFlags, vec![old_flags, vref]);
    self.set_flags(new_flags);
    // A flag write we do not model a carry for invalidates the tracked
    // carry; the producers below opt back in via append_update_flags_carry.
    self.clear_carry();
  }

  /// Like `append_update_flags`, but records the carry-out so a following
  /// ADC/SBB can consume it (see `last_cf`).
  fn append_update_flags_carry(&mut self, vref: Ref, cf: Ref) {
    let old_flags = self.get_flags();
    let new_flags = self.append_instr(Type::U16, Opcode::UpdateFlags, vec![old_flags, vref]);
    self.set_flags(new_flags);
    // Publish the carry-out as the CF SSA value so a following block can
    // merge it through a phi (see carry_var).
    self.set_carry_out(cf);
  }

  /// Clobber the opaque FLAGS value with the unknown initial flags, so a
  /// later flag-consuming branch degrades to the loud but non-fatal
  /// UNIMPL_FLAGS marker (mirrors the post-INT reset). Passing a `Void`
  /// `Unimpl` as the flags operand instead would emit a fatal `UNIMPL()`
  /// abort, which is not the "rather than aborting" contract (see ast.rs).
  fn clobber_flags(&mut self) {
    self.set_flags(Ref::Init(instr::Reg::FLAGS));
    self.clear_carry();
  }

  /// Like `clobber_flags`, but preserves a known carry-out (RCL/RCR/STC).
  fn clobber_flags_carry(&mut self, cf: Ref) {
    self.set_flags(Ref::Init(instr::Reg::FLAGS));
    self.set_carry_out(cf);
  }

  fn is_word(r: Ref, ir: &IR) -> bool {
    matches!(ir.instr(r).map(|i| i.typ.clone()), Some(Type::U16) | Some(Type::I16))
  }

  /// True when the ALU operand is 16-bit (word). Byte registers and
  /// `byte ptr` memory are 8-bit. This is read from the *instruction*, not
  /// the IR type: a byte register's incoming value is `Init`, which is
  /// typed `U16`, so the IR type cannot distinguish `add al,..` from
  /// `add ax,..` — and the carry bit differs between them.
  fn operand_is_word(oper: &instr::Operand) -> bool {
    match oper {
      instr::Operand::Reg(r) => !matches!(r.0,
        instr::Reg::AL | instr::Reg::CL | instr::Reg::DL | instr::Reg::BL |
        instr::Reg::AH | instr::Reg::CH | instr::Reg::DH | instr::Reg::BH),
      instr::Operand::Mem(m) => matches!(m.sz, instr::Size::Size16),
      _ => false,
    }
  }

  /// Operand width in bits (8 or 16) for a rotate destination, read from
  /// the instruction (the IR type cannot distinguish `al` from `ax`).
  /// `None` for a size the decoder never produces for rotates.
  fn operand_width_bits(oper: &instr::Operand) -> Option<i16> {
    match oper {
      instr::Operand::Reg(r) => Some(if Self::operand_is_word(&instr::Operand::Reg(*r)) { 16 } else { 8 }),
      instr::Operand::Mem(m) => match m.sz {
        instr::Size::Size8 => Some(8),
        instr::Size::Size16 => Some(16),
        _ => None,
      },
      _ => None,
    }
  }

  /// Carry-out of an ADD whose result is a full-width (u16) sum:
  /// `(a + b) < a`. `None` when `res` is not a u16 (byte ADD handles its
  /// carry off a widened sum in `set_flags_for_binary`).
  fn add_carry_out(&mut self, res: Ref, a: Ref) -> Option<Ref> {
    if !Self::is_word(res, &self.ir) { return None; }
    Some(self.append_instr(Type::U16, Opcode::ULt, vec![res, a]))
  }

  /// Borrow-out of a SUB/CMP: `a < b` (unsigned). The comparison itself
  /// needs no extra bit, so it is width-independent *provided the operands
  /// fit their width* — callers mask byte operands with `to_byte` first.
  fn sub_borrow_out(&mut self, a: Ref, b: Ref) -> Ref {
    self.append_instr(Type::U16, Opcode::ULt, vec![a, b])
  }

  /// Reduce a byte-register read to its low 8 bits before a width-sensitive
  /// compare. Byte fragments are 8-bit masked at the split, but
  /// `forward_store_to_load` forwards values without regard to width, so
  /// mask here defensively. ALU results re-mask on the byte write, but
  /// carries and borrows compare the raw operand. A never-written register
  /// (`Init`) and an 8-bit immediate (`Const`) are already within 8 bits.
  fn to_byte(&mut self, r: Ref) -> Ref {
    if matches!(r, Ref::Init(_) | Ref::Const(_)) { return r; }
    let mask = self.ir.const_new(0xFF);
    self.append_instr(Type::U16, Opcode::And, vec![r, mask])
  }

  /// Interpret an 8-bit fragment as signed and widen it to 16 bits. A
  /// fragment read is always masked, so its stored value is the
  /// zero-extended unsigned byte; the signed operations that read one
  /// (byte IDIV's divisor, byte IMUL's operands) need the sign back.
  fn sign_byte_to_word(&mut self, r: Ref) -> Ref {
    self.append_instr(Type::U16, Opcode::SignExtTo16, vec![r])
  }

  /// Interpret a 16-bit value as signed and widen it to 32 bits: the
  /// divisor of word IDIV is a signed half-word (0x8001 is -32767, not
  /// +32769). Renders the same double cast CWD's AX-to-DX:AX uses.
  fn sign_word_to_dword(&mut self, r: Ref) -> Ref {
    self.append_instr(Type::U32, Opcode::SignExtTo32, vec![r])
  }

  /// Update flags for a simple ALU binary op, tracking the carry-out where
  /// known: word ADD and any-width SUB/CMP enable carry chains, shifts
  /// with constant counts publish their last-bit-out, the logical ops
  /// clear CF, and anything else leaves it unknown.
  fn set_flags_for_binary(&mut self, opcode: Opcode, res: Ref, a: Ref, b: Ref, word: bool) {
    match opcode {
      Opcode::Add if word => match self.add_carry_out(res, a) {
        Some(cf) => self.append_update_flags_carry(res, cf),
        None => self.append_update_flags(res),
      },
      Opcode::Add => {
        // Byte ADD: the byte-typed result cannot express the carry (there is
        // no narrower compare), so read it off a widened u16 sum. Byte
        // operands may hold a full parent-register value, so mask to 8 bits
        // first; then `0xFF < a + b` is exactly the carry-out bit.
        let a = self.to_byte(a);
        let b = self.to_byte(b);
        let wide = self.append_instr(Type::U16, Opcode::Add, vec![a, b]);
        let max = self.ir.const_new(0xFF);
        let cf = self.append_instr(Type::U16, Opcode::ULt, vec![max, wide]);
        self.append_update_flags_carry(res, cf);
      }
      Opcode::Sub => {
        // Byte operands may hold a full parent-register value, so mask them
        // to 8 bits before the unsigned borrow compare.
        let (a, b) = if word { (a, b) } else { (self.to_byte(a), self.to_byte(b)) };
        let cf = self.sub_borrow_out(a, b);
        self.append_update_flags_carry(res, cf);
      }
      Opcode::And | Opcode::Or | Opcode::Xor => {
        // The logical ops clear CF (and OF).
        let zero = self.ir.const_new(0);
        self.append_update_flags_carry(res, zero);
      }
      Opcode::Shl | Opcode::UShr | Opcode::Shr => {
        // Shifts publish their carry-out (the last bit shifted out) for
        // constant counts: SHL carries bit (w-n), SHR/SAR carry bit (n-1)
        // (sign-independent). Gated to 1..=w: x86 masks the count to 5
        // bits, so e.g. `shl ax,16` genuinely shifts by 16 and carries
        // bit 0 (pos = w-n = 0). Variable counts and counts above w keep
        // the conservative clobber. ROL/ROR carry is not modeled (their
        // zero-count case preserves CF, which needs a conditional).
        // A zero count leaves every flag unchanged in HW (including CF),
        // so it is a full no-op for flags: preserve the tracked carry and
        // emit no update — mirroring the RCL/RCR zero-count path below.
        if self.ir.const_lookup(b) == Some(0) {
          return;
        }
        let w: i16 = if word { 16 } else { 8 };
        let cf = match self.ir.const_lookup(b) {
          Some(n) if (1..=w).contains(&n) => {
            let pos = if opcode == Opcode::Shl { w - n } else { n - 1 };
            let s = self.ir.const_new(pos);
            let shifted = self.append_instr(Type::U16, Opcode::UShr, vec![a, s]);
            let one = self.ir.const_new(1);
            Some(self.append_instr(Type::U16, Opcode::And, vec![shifted, one]))
          }
          _ => None,
        };
        match cf {
          Some(cf) => self.append_update_flags_carry(res, cf),
          None => self.append_update_flags(res),
        }
      }
      _ => self.append_update_flags(res),
    }
  }

  /// String-op pointer auto-update (`SI`/`DI` += element size), assuming
  /// the direction flag is clear (increment). Only called while `df_clear`
  /// holds, so the direction is provably clear (DOS ABI).
  fn advance_string_ptr(&mut self, reg: instr::Reg, size: i16) {
    let cur = self.ir.get_var(reg, self.cur);
    let step = self.ir.const_new(size);
    let next = self.append_instr(Type::U16, Opcode::Add, vec![cur, step]);
    self.append_asm_dst_reg(&instr::OperandReg(reg), next);
  }

  /// Finish a single-step string op: advance the pointers the op updates
  /// (with DF=0), or, when the direction is not provably clear at this
  /// point, degrade to a loud marker instead of a possibly-wrong-direction
  /// update.
  fn string_op_advance(&mut self, si: Option<&instr::Operand>, di: Option<&instr::Operand>) {
    if !self.df_clear {
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

  /// Segment register of a string memory operand (falling back to the
  /// instruction's implied segment).
  fn string_operand_seg(oper: &instr::Operand, fallback: instr::Reg) -> instr::Reg {
    match oper {
      instr::Operand::Mem(m) => m.sreg,
      _ => fallback,
    }
  }

  /// Lower a `rep` MOVS/STOS (direction provably clear): emit the runtime
  /// bulk macro over the pre-update values, then model the deterministic
  /// register updates in the SSA (`CX -> 0`, `SI/DI += CX0*size`). The
  /// macro takes value copies, so it never mutates the SSA pointers.
  fn lower_rep_string_op(&mut self, ins: &instr::Instr) {
    let cx = self.ir.get_var(instr::Reg::CX, self.cur);
    let zero = self.ir.const_new(0);
    match ins.opcode {
      instr::Opcode::OP_MOVS => {
        let dst = &ins.operands[0]; // ES:DI
        let src = &ins.operands[1]; // DS:SI (segment may be overridden)
        let size = Self::string_operand_size(dst);
        let ds = self.ir.get_var(Self::string_operand_seg(src, instr::Reg::DS), self.cur);
        let es = self.ir.get_var(Self::string_operand_seg(dst, instr::Reg::ES), self.cur);
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
        let es = self.ir.get_var(Self::string_operand_seg(dst, instr::Reg::ES), self.cur);
        let di = self.ir.get_var(instr::Reg::DI, self.cur);
        let val = self.append_asm_src_operand(&ins.operands[1]); // AL/AX
        let size_c = self.ir.const_new(size);
        self.append_instr(Type::Void, Opcode::RepStos, vec![es, di, cx, val, size_c]);
        let off = self.append_instr(Type::U16, Opcode::UMul, vec![cx, size_c]);
        let di_n = self.append_instr(Type::U16, Opcode::Add, vec![di, off]);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::DI), di_n);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::CX), zero);
      }
      instr::Opcode::OP_LODS => {
        // operands: [AL/AX, DS:SI]. Only reads memory: the result value is
        // the final accumulator, and SI/CX are updated deterministically.
        let dst = &ins.operands[0];
        let src = &ins.operands[1];
        let size = Self::string_operand_size(src);
        let typ = if size == 2 { Type::U16 } else { Type::U8 };
        let ds = self.ir.get_var(Self::string_operand_seg(src, instr::Reg::DS), self.cur);
        let si = self.ir.get_var(instr::Reg::SI, self.cur);
        let cur = self.append_asm_src_operand(dst);
        // The helper reads the last element (or returns `cur` when CX==0).
        let val = self.append_instr(typ, Opcode::RepLods, vec![ds, si, cx, cur]);
        self.append_asm_dst_operand(dst, val);
        let size_c = self.ir.const_new(size);
        let off = self.append_instr(Type::U16, Opcode::UMul, vec![cx, size_c]);
        let si_n = self.append_instr(Type::U16, Opcode::Add, vec![si, off]);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::SI), si_n);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::CX), zero);
      }
      instr::Opcode::OP_INS => {
        // rep ins: operands [ES:DI, DX]. The helper executes the raw rep
        // instruction (port read -> ES:DI); DI += CX0*size, CX -> 0.
        let dst = &ins.operands[0]; // ES:DI
        let size = Self::string_operand_size(dst);
        let es = self.ir.get_var(Self::string_operand_seg(dst, instr::Reg::ES), self.cur);
        let di = self.ir.get_var(instr::Reg::DI, self.cur);
        let port = self.append_asm_src_operand(&ins.operands[1]); // DX
        let size_c = self.ir.const_new(size);
        self.append_instr(Type::Void, Opcode::RepIns, vec![es, di, cx, port, size_c]);
        let off = self.append_instr(Type::U16, Opcode::UMul, vec![cx, size_c]);
        let di_n = self.append_instr(Type::U16, Opcode::Add, vec![di, off]);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::DI), di_n);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::CX), zero);
      }
      instr::Opcode::OP_OUTS => {
        // rep outs: operands [DX, DS:SI]. The helper executes the raw rep
        // instruction (DS:SI -> port); SI += CX0*size, CX -> 0.
        let src = &ins.operands[1]; // DS:SI
        let size = Self::string_operand_size(src);
        let ds = self.ir.get_var(Self::string_operand_seg(src, instr::Reg::DS), self.cur);
        let si = self.ir.get_var(instr::Reg::SI, self.cur);
        let port = self.append_asm_src_operand(&ins.operands[0]); // DX
        let size_c = self.ir.const_new(size);
        self.append_instr(Type::Void, Opcode::RepOuts, vec![ds, si, cx, port, size_c]);
        let off = self.append_instr(Type::U16, Opcode::UMul, vec![cx, size_c]);
        let si_n = self.append_instr(Type::U16, Opcode::Add, vec![si, off]);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::SI), si_n);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::CX), zero);
      }
      instr::Opcode::OP_CMPS | instr::Opcode::OP_SCAS => {
        // The helper runs the compare loop and returns the remaining CX;
        // SI/DI advance by (CX0 - cx_final)*size. The final comparison's
        // flags are data-dependent, so the flags are clobbered (loud).
        let cond = match ins.rep { Some(instr::Rep::NE) => 1i16, _ => 0 };
        let cond_c = self.ir.const_new(cond);
        let is_cmps = ins.opcode == instr::Opcode::OP_CMPS;
        let size = Self::string_operand_size(if is_cmps { &ins.operands[0] } else { &ins.operands[1] });
        let size_c = self.ir.const_new(size);
        let ret = if is_cmps {
          let dst = &ins.operands[0]; // ES:DI
          let src = &ins.operands[1]; // DS:SI
          let ds = self.ir.get_var(Self::string_operand_seg(src, instr::Reg::DS), self.cur);
          let si = self.ir.get_var(instr::Reg::SI, self.cur);
          let es = self.ir.get_var(Self::string_operand_seg(dst, instr::Reg::ES), self.cur);
          let di = self.ir.get_var(instr::Reg::DI, self.cur);
          self.append_instr(Type::U16, Opcode::RepCmps, vec![ds, si, es, di, cx, size_c, cond_c])
        } else {
          let dst = &ins.operands[1]; // ES:DI
          let es = self.ir.get_var(Self::string_operand_seg(dst, instr::Reg::ES), self.cur);
          let di = self.ir.get_var(instr::Reg::DI, self.cur);
          let val = self.append_asm_src_operand(&ins.operands[0]); // AL/AX
          self.append_instr(Type::U16, Opcode::RepScas, vec![es, di, cx, val, size_c, cond_c])
        };
        // iterated = CX0 - cx_final; SI/DI += iterated*size.
        let iterated = self.append_instr(Type::U16, Opcode::Sub, vec![cx, ret]);
        let off = self.append_instr(Type::U16, Opcode::UMul, vec![iterated, size_c]);
        if is_cmps {
          let si = self.ir.get_var(instr::Reg::SI, self.cur);
          let si_n = self.append_instr(Type::U16, Opcode::Add, vec![si, off]);
          self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::SI), si_n);
        }
        let di = self.ir.get_var(instr::Reg::DI, self.cur);
        let di_n = self.append_instr(Type::U16, Opcode::Add, vec![di, off]);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::DI), di_n);
        self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::CX), ret);
        // The comparison flags are unknown.
        self.clobber_flags();
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
    // The registers that can carry exit state are materialised as real writes
    // by `write_back_exit_registers`; a `Pin` here is ignored by AST emission,
    // so leaving them pinned too would only imply an exit-state marker that
    // never reaches the output. Pin the rest: AX is the return value (pinned
    // only for the debugging `pin_all`), and SP/BP belong to the frame
    // sequence. DS/ES/SS are written back when they change, so they must not be
    // replaced by an ignored `Pin` here.
    if self.pin_all {
      self.pin_register(instr::Reg::AX);
    }

    self.pin_register(instr::Reg::SP);
    self.pin_register(instr::Reg::BP);

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
    self.append_instr_with_attrs(Type::Void, Attribute::STACK_SAVE, Opcode::Store16,
                                 vec![ss, sp, vref]);
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
  fn heuristic_infer_call_arguments_by_context(&mut self, ins: &instr::Instr, warn_msg: &str) -> u16 {
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
      self.warn_ins(Cause::AbiArgsInference, ins,
                    format!("WARN: {}, inferred {} arg(s)... possibly erroneously", warn_msg, n));
      n
    } else {
      // If more bytes are cleaned up then pushed.. pessimize
      let n = 0;
      self.warn_ins(Cause::AbiArgsInference, ins,
                    format!("WARN: {}, failed to infer, assuming {} arg(s)...very likely erroneously", warn_msg, n));
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
      let (upper, lower) = self.append_byte_split(cur);
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

  /// Refresh the caller's view of a register the callee declares as an output
  /// or clobber. The `CALL_CONTRACT` marker's `Init(reg)` operand renders as
  /// the live C variable the callee has just changed; keeping the pre-call SSA
  /// value would make a later read observe stale state. The marker also keeps
  /// ABI inference from mistaking that callee output for a caller input. Return
  /// registers are handled by `save_return_value`, so do not overwrite those
  /// with an unknown live value.
  fn apply_call_contract(&mut self, func: &config::Func, ret_type: &Type) {
    let mut regs = HashSet::new();
    for list in [&func.regouts, &func.clobbers] {
      if let Some(list) = list {
        regs.extend(list.iter().copied());
      }
    }
    for reg in regs {
      if Self::is_return_register(ret_type, reg) { continue; }
      let vref = self.append_call_contract_reg(reg);
      self.ir.set_var(reg, self.cur, vref);
      let fragments = match reg {
        instr::Reg::AX => Some((instr::Reg::AL, instr::Reg::AH)),
        instr::Reg::BX => Some((instr::Reg::BL, instr::Reg::BH)),
        instr::Reg::CX => Some((instr::Reg::CL, instr::Reg::CH)),
        instr::Reg::DX => Some((instr::Reg::DL, instr::Reg::DH)),
        _ => None,
      };
      if let Some((lo, hi)) = fragments {
        let lo_ref = self.append_call_contract_reg(lo);
        let hi_ref = self.append_call_contract_reg(hi);
        self.ir.set_var(lo, self.cur, lo_ref);
        self.ir.set_var(hi, self.cur, hi_ref);
      }
    }
  }

  /// The post-call live value of a contracted register. The marker keeps ABI
  /// inference from treating the callee's output as a caller input, while
  /// `Init(reg)` still renders as the live C variable the callee changed.
  fn append_call_contract_reg(&mut self, reg: instr::Reg) -> Ref {
    self.append_instr_with_attrs(Type::U16, Attribute::CALL_CONTRACT,
                                 Opcode::Ref, vec![Ref::Init(reg)])
  }

  /// Whether `reg` is already defined by the call's return-value handling.
  fn is_return_register(ret_type: &Type, reg: instr::Reg) -> bool {
    match ret_type {
      Type::Void => false,
      Type::U8 | Type::I8 | Type::U16 | Type::I16 => reg == instr::Reg::AX,
      Type::U32 | Type::I32 | Type::Unknown => {
        matches!(reg, instr::Reg::AX | instr::Reg::DX)
      }
      _ => false,
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
    let ret_ref = self.append_instr(ret_type.clone(), Opcode::CallFarPtr, operands);
    self.save_return_value(ret_type, ret_ref);
  }

  fn process_call_known(&mut self, func: &config::Func, ins: &instr::Instr) -> Ref {
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
    self.apply_call_contract(func, &ret_type);

    if func.dont_pop_args {
      let sp = self.ir.get_var(instr::Reg::SP, self.cur);
      let k = self.ir.const_new((2*nargs) as i16);
      let sp = self.append_instr_with_attrs(Type::U16, Attribute::STACK_PTR, Opcode::Add, vec![sp, k]);
      self.ir.set_var(instr::Reg::SP, self.cur, sp);
    }

    ret_ref
  }

  /// Emit the callee's declared register inputs and return the dirty registers
  /// the annotation does *not* declare.
  ///
  /// The returned values are candidates for the #132 diagnostic: they exist at
  /// the call but only the callee could read them (the caller's own uses are
  /// checked later, on the finished IR, by `check_regargs_setups`).
  fn append_regargs(&mut self, regargs: &Option<Vec<instr::Reg>>) -> Vec<(instr::Reg, Ref)> {
    let Some(regargs) = regargs else { return vec![] };
    // Same hazard as `process_int` (issues #78/#101): each pinned write is a
    // guest register assignment in the emitted C, so a copy recorded as
    // `Ref::Init(src)` would observe the value its source gets *after* the
    // copy if the source were written first. Order the batch the way
    // `process_int` does, and skip registers still holding their entry value
    // (a pinned `AX = AX` is a no-op write).
    // Fragment regargs (`AL`/`AH`) alias their parent's storage. Normalize
    // them to the parent before the snapshot so the write targets a symbol
    // the register table actually has (it only knows full registers) and
    // duplicate fragments collapse to one write; the parent's current value
    // carries the fragment, since partial writes recompose it.
    let mut seen = HashSet::new();
    let mut pending = vec![];
    for &reg in regargs {
      let reg = Self::init_storage_parent(reg);
      if !seen.insert(reg) { continue; }
      let vref = self.ir.get_var(reg, self.cur);
      if vref == Ref::Init(reg) { continue; }
      pending.push((reg, vref));
    }
    for (reg, vref) in Self::order_guest_writes(&self.ir, &pending) {
      self.write_guest_register(reg, vref);
    }

    // Undeclared registers holding a non-entry value are the callee inputs the
    // annotation may have forgotten. `process_int` pins the same set for a
    // handler, which may read any of them; a known callee is trusted to read
    // only its declared regargs, so this is where that trust is checked.
    let declared: HashSet<instr::Reg> =
      regargs.iter().map(|reg| Self::init_storage_parent(*reg)).collect();
    let mut missing = vec![];
    for reg in [instr::Reg::AX, instr::Reg::BX, instr::Reg::CX, instr::Reg::DX,
                instr::Reg::SI, instr::Reg::DI, instr::Reg::DS, instr::Reg::ES] {
      if declared.contains(&reg) { continue; }
      let vref = self.ir.get_var(reg, self.cur);
      if Self::is_entry_value(&self.ir, vref, reg) { continue; }
      missing.push((reg, vref));
    }
    missing
  }

  /// Emit a guest register assignment. Unlike `pin_register` (whose `Pin` is
  /// transparent to AST emission, see ast.rs), this renders as a real write to
  /// the register variable: it is how a synchronised value becomes observable
  /// to the caller, the interrupt handler, or the machine's exit state.
  fn write_guest_register(&mut self, reg: instr::Reg, vref: Ref) -> Ref {
    let sym = self.ir.symbols.find_ref_by_name(sym::Table::Register, reg.info().name).unwrap();
    self.append_instr_with_attrs(Type::U16, Attribute::PIN, Opcode::WriteVar16, vec![Ref::Symbol(sym), vref])
  }

  /// The same guest write, placed immediately before `before` in `blk`. Used by
  /// the deferred exit write-back, which runs after lowering has moved on.
  fn write_guest_register_before(&mut self, blk: BlockRef, before: Ref,
                                 reg: instr::Reg, vref: Ref, extra_attrs: u8) -> Ref {
    let sym = self.ir.symbols.find_ref_by_name(sym::Table::Register, reg.info().name).unwrap();
    self.insert_instr_before(blk, before, Type::U16,
                             Attribute::PIN | Attribute::EXIT_STATE | extra_attrs,
                             Opcode::WriteVar16, vec![Ref::Symbol(sym), vref])
  }

  /// Materialise the function's exit register state at a `ret`.
  ///
  /// A bare `Pin` is not a register write (ast.rs drops it), so without this a
  /// function that saves registers, calls out, and restores them with `push`/
  /// `pop` returns having emitted only the pushes: the pops are SSA values with
  /// no consumer once the return is emitted, and callers that trust the
  /// callee-saved model (see `save_return_value`) then read the mid-function
  /// value instead of the restored one (issue #133).
  ///
  /// The set is the one callers assume the callee leaves intact: `BX`/`SI`/`DI`
  /// are the callee-saved set, and the caller keeps the pre-call `CX`/`DX`
  /// (`save_return_value` refreshes their fragments from the current value), so
  /// an explicit restore of those is exit state too. `AX` is the return value
  /// the return expression already writes, `DX` is skipped when it is the high
  /// half of that return value, and `SP`/`BP` belong to the frame sequence
  /// (`FRAME_LEAVE`). `DS`/`ES`/`SS` are also observable exit state when the
  /// function changes them, just like the general-purpose registers. Registers
  /// still holding their entry value are skipped: in the emitted model
  /// `Ref::Init(reg)` *is* the live register, so writing it would be a no-op
  /// (this also keeps a `push reg`/`pop reg` pair that folds away from gaining
  /// a write).
  ///
  /// A changed `SS` is emitted after the flavor's frame teardown: `FRAME_LEAVE`
  /// pops the saved BP through the entry stack segment, while the near/far
  /// return must observe the new one. Its value is frozen before the other
  /// exit writes when it reads a register they change, since the deferred
  /// write no longer follows `order_guest_writes`.
  ///
  /// The return expression can read a register this write-back rewrites
  /// (`mov ax,cx; dec cx; ret` returns the pre-decrement CX). `Ref::Init(reg)`
  /// renders as the live C variable, so such a value has to be frozen into a
  /// `MATERIALISE` temporary *before* the writes, or the return would observe
  /// the exit state instead of the value the caller receives. The snapshot is
  /// taken even when the value is the pending expression itself: the pending
  /// expression re-reads `Init(reg)`, which is exactly what the write changes.
  ///
  /// Runs after `build_once` has sealed every block (see the `ret_exits`
  /// field). The register values are captured while lowering, so an
  /// instruction decoded after the ret (padding, inline data, the next entry
  /// in the configured range) cannot become the exit state; those never
  /// execute, yet Step 1 lowers them into the ret's block. Only the entry
  /// test and the snapshots are deferred: an unsealed block's `get_var`
  /// returns an incomplete phi even for a register no path writes, so a
  /// lowering-time check cannot tell an entry value from a setup, and a return
  /// value read through such a phi does not look like a read of the register
  /// (its snapshot would be missed and the return would observe the exit
  /// state). Sealing populates those phis; the refs captured at the ret then
  /// resolve to the values they actually carry. The return operands come from
  /// the lowering-time `ret` too, not a fresh `get_var`: `pin_registers` can
  /// have replaced `AX` with a `Pin` under `pin_all`.
  fn write_back_exit_registers(&mut self) {
    let mut changed = HashSet::new();
    for RetExit { ret_ref, pending } in std::mem::take(&mut self.ret_exits) {
      let Ref::Instr(blk, _) = ret_ref else { continue; };

      // A contracted call already changed the live variable; there is no write
      // to emit, but the register is still part of this function's exit state
      // for contract diagnostics.
      changed.extend(pending.iter()
        .filter(|(_, vref)| Self::ref_has_call_contract(&self.ir, *vref))
        .map(|(reg, _)| *reg));

      let mut pending: Vec<_> = pending.into_iter()
        .filter(|(reg, vref)| !Self::is_entry_value(&self.ir, *vref, *reg))
        .collect();
      if pending.is_empty() { continue; }
      changed.extend(pending.iter().map(|(reg, _)| *reg));

      // Snapshot first, then insert the writes: each insert lands immediately
      // before the `ret`, leaving the snapshots ahead of the writes.
      let mut operands = self.ir.instr(ret_ref).unwrap().operands.clone();
      for v in operands.iter_mut() {
        if pending.iter().any(|(reg, _)| Self::ref_reads_init(&self.ir, *v, *reg)) {
          *v = self.insert_instr_before(blk, ret_ref, Type::U16,
                                        Attribute::MATERIALISE, Opcode::Ref, vec![*v]);
        }
      }
      self.ir.instr_mut(ret_ref).unwrap().operands = operands;

      // SS is written after the frame teardown, so freeze a value that reads a
      // register an earlier exit write changes. Otherwise the deferred write
      // would observe the new value instead of the one at the guest ret.
      if let Some(pos) = pending.iter().position(|(reg, _)| *reg == instr::Reg::SS) {
        let vref = pending[pos].1;
        let reads_pending = pending.iter().any(|(reg, _)| {
          *reg != instr::Reg::SS && Self::ref_reads_init(&self.ir, vref, *reg)
        });
        if reads_pending {
          pending[pos].1 = self.insert_instr_before(blk, ret_ref, Type::U16,
            Attribute::MATERIALISE, Opcode::Ref, vec![vref]);
        }
      }

      for (reg, vref) in Self::order_guest_writes(&self.ir, &pending) {
        let extra_attrs = if reg == instr::Reg::SS { Attribute::POST_FRAME } else { Attribute::NONE };
        self.write_guest_register_before(blk, ret_ref, reg, vref, extra_attrs);
      }
    }
    self.check_exit_contract(&changed);
  }

  /// Warn when an explicitly annotated caller/callee contract does not cover a
  /// register the function actually changes at a return. Correctness does not
  /// depend on this annotation: the write-back above runs regardless. The
  /// warning only helps keep the caller model honest when an operator opted
  /// into `regouts`/`clobbers`.
  fn check_exit_contract(&mut self, changed: &HashSet<instr::Reg>) {
    let Some(func) = self.spec.func else { return; };
    if func.regouts.is_none() && func.clobbers.is_none() { return; }

    let mut declared = HashSet::new();
    for list in [&func.regouts, &func.clobbers] {
      if let Some(list) = list {
        declared.extend(list.iter().copied());
      }
    }
    let mut missing: Vec<&str> = changed.iter()
      .filter(|reg| !declared.contains(reg))
      .map(|reg| reg.info().name)
      .collect();
    if missing.is_empty() { return; }
    missing.sort_unstable();
    let pronoun = if missing.len() == 1 { "it" } else { "them" };
    self.warn(Cause::ExitContractMismatch, Some(self.spec.start), None, format!(
      "WARN: {}: changes {} at return but its regouts/clobbers annotation does not declare {}",
      self.spec.name, missing.join(", "), pronoun));
  }

  /// The register values that may need write-back at a `ret`: the callee-saved
  /// set, the caller's `CX`/`DX`, and the segment registers. Captured while
  /// lowering, before any instructions after the `ret` are lowered into the
  /// same block (they never execute, so they must not be the exit state). `DX`
  /// is skipped when it is the high half of the return value, which the return
  /// expression already writes. `AX` is the return value (#134); a void return
  /// still leaves the caller modelling AX as preserved.
  fn exit_register_values(&mut self, dx_is_return: bool) -> Vec<(instr::Reg, Ref)> {
    let mut pending = vec![];
    for reg in [instr::Reg::BX, instr::Reg::CX, instr::Reg::DX, instr::Reg::SI, instr::Reg::DI,
                instr::Reg::ES, instr::Reg::SS, instr::Reg::DS] {
      if dx_is_return && reg == instr::Reg::DX { continue; }
      pending.push((reg, self.ir.get_var(reg, self.cur)));
    }
    pending
  }

  fn process_call_segoff(&mut self, addr: SegOff, mode: config::CallMode, ins: &instr::Instr) {
    if let Some(func) = self.cfg.func_lookup(addr) {
      // Known function
      if func.mode != mode {
        panic!("Found function but it's call mode doesn't match! Expected {:?}, Got {:?}", mode, func.mode);
      }
      let missing = self.append_regargs(&func.regargs);
      let call_ref = self.process_call_known(func, ins);
      if !missing.is_empty() {
        self.regargs_candidates.push(RegargsCandidate {
          blk: self.cur,
          call_ref,
          call_addr: ins.addr,
          callee: func.name.to_string(),
          missing,
        });
      }
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
    let ret_ref = self.append_instr(Type::Unknown, Opcode::CallNearPtr, operands);
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
    // have no static target; model them as near-indirect calls with
    // heuristically inferred stack args instead of aborting.
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

  /// Parent full register whose storage a byte fragment aliases (`AL`/`AH`
  /// read `AX` storage; see the `AL`/`AH` macros in hydra's machine.h).
  /// Full registers and non-fragmented registers map to themselves.
  fn init_storage_parent(reg: instr::Reg) -> instr::Reg {
    match reg {
      instr::Reg::AL | instr::Reg::AH => instr::Reg::AX,
      instr::Reg::BL | instr::Reg::BH => instr::Reg::BX,
      instr::Reg::CL | instr::Reg::CH => instr::Reg::CX,
      instr::Reg::DL | instr::Reg::DH => instr::Reg::DX,
      _ => reg,
    }
  }

  /// Whether `r` is still the register's entry value. A plain `Ref::Init` is,
  /// and so is a phi or copy that only ever carries entry values: at a control
  /// flow merge, `get_var` materializes `Phi(Init(SI), Init(SI))` for a
  /// register no path writes (`ir/methods.rs` leaves trivial phis in place),
  /// and such a value is not a setup.
  ///
  /// A phi with no operands is *not* entry: it is an incomplete phi, created
  /// while its block is still unsealed, and its operands are only known after
  /// sealing. Querying during lowering must treat it as dirty; the exit
  /// write-back and the #132 candidate check therefore run after `build_once`,
  /// when it can collapse.
  fn is_entry_value(ir: &IR, r: Ref, reg: instr::Reg) -> bool {
    let mut memo = HashMap::new();
    let mut visiting = HashSet::new();
    Self::is_entry_value_inner(ir, r, reg, &mut memo, &mut visiting)
  }

  fn is_entry_value_inner(ir: &IR, r: Ref, reg: instr::Reg,
                          memo: &mut HashMap<Ref, bool>, visiting: &mut HashSet<Ref>) -> bool {
    if let Some(known) = memo.get(&r) { return *known; }
    let result = match r {
      Ref::Init(other) => Self::init_storage_parent(other) == reg,
      Ref::Instr(_, _) => {
        if !visiting.insert(r) { return true; }
        let instr = ir.instr(r);
        let result = match instr {
          Some(instr) => match instr.opcode {
            Opcode::Phi | Opcode::Ref => !instr.operands.is_empty() && instr.operands.iter()
              .all(|operand| Self::is_entry_value_inner(ir, *operand, reg, memo, visiting)),
            _ => false,
          },
          None => false,
        };
        visiting.remove(&r);
        result
      }
      _ => false,
    };
    memo.insert(r, result);
    result
  }

  /// Whether `r` was produced by a known call's `regouts`/`clobbers`
  /// contract. Such a value renders as the live C variable but is not an entry
  /// value for exit-contract diagnostics.
  fn ref_has_call_contract(ir: &IR, r: Ref) -> bool {
    let mut stack = vec![r];
    let mut seen = HashSet::new();
    while let Some(cur) = stack.pop() {
      let Ref::Instr(_, _) = cur else { continue; };
      if !seen.insert(cur) { continue; }
      let Some(instr) = ir.instr(cur) else { continue; };
      if (instr.attrs & Attribute::CALL_CONTRACT) != 0 { return true; }
      stack.extend(instr.operands.iter().copied());
    }
    false
  }

  /// Whether the SSA value `r` (transitively) reads the live entry value
  /// of `target`: `Ref::Init(target)` renders as the mutable C variable for
  /// the register, so a materialized write carrying `r` must be emitted
  /// before any write that overwrites `target`. The walk covers indirect
  /// reads too (e.g. a recompose `Or` holding `And(Init(AX), 0xFF)`), not
  /// just a direct `Init` alias from `mov reg,reg`. Byte fragments alias
  /// their parent's storage (`Init(AL)` reads `AX`), so they count as
  /// reads of the parent full register.
  fn ref_reads_init(ir: &IR, r: Ref, target: instr::Reg) -> bool {
    let mut stack = vec![r];
    let mut seen: HashSet<Ref> = HashSet::new();
    while let Some(cur) = stack.pop() {
      if !seen.insert(cur) { continue; }
      match cur {
        Ref::Init(reg) => {
          if Self::init_storage_parent(reg) == target { return true; }
        }
        Ref::Instr(_, _) => {
          let Some(instr) = ir.instr(cur) else { continue; };
          stack.extend(instr.operands.iter().copied());
        }
        Ref::Const(_) | Ref::Symbol(_) | Ref::Func(_) | Ref::Block(_) => (),
      }
    }
    false
  }

  /// Order batched guest writes so copies observe program-order values.
  /// Emitting `WriteVar16` in a fixed register order is wrong when a
  /// register was copied while its source still held the entry value
  /// (`mov bx,ax` records `BX = Init(AX)`, a live read): writing the
  /// source first lets the copy observe the new value. A dest whose value
  /// reads `Init(X)` is emitted before `X`'s write (stable topological
  /// order over the fixed register order). A dependency cycle (not
  /// expected from `mov` copies, which collapse to the same entry ref)
  /// falls back to fixed order for its members rather than looping.
  fn order_guest_writes(ir: &IR, pending: &[(instr::Reg, Ref)]) -> Vec<(instr::Reg, Ref)> {
    let mut remaining: Vec<(instr::Reg, Ref)> = pending.to_vec();
    let mut ordered: Vec<(instr::Reg, Ref)> = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
      // Eligible: no *other* remaining write reads this candidate's register.
      let pos = remaining.iter().position(|(reg, _)| {
        !remaining.iter().any(|(other_reg, vref)| {
          *other_reg != *reg && Self::ref_reads_init(ir, *vref, *reg)
        })
      });
      // Cycle fallback: keep fixed order for the rest.
      let idx = pos.unwrap_or(0);
      ordered.push(remaining.remove(idx));
    }
    ordered
  }

  fn process_int(&mut self, ins: &instr::Instr) {
    // An INT handler may read any general-purpose register, so first make the
    // live register values explicit as pinned guest writes. Unlike appending
    // them as operands of `Int` (its emitter renders only operands[0]; see
    // ast.rs), WriteVar16 against the Register symbol renders as a real guest
    // write and keeps each setup value live through DCE. Registers still at
    // their initial (unknown) value are skipped: writing them would emit a
    // no-op `BX = BX`. DF is deliberately left alone: the DOS convention
    // (see docs/8086_flags.txt: DF=0 on entry, restored on exit) is that
    // callees and interrupt handlers preserve DF, and CALL callees are
    // treated the same (process_calln leaves `df_clear` intact) — so the
    // per-point `df_clear` state survives the call. Clearing here but not
    // on CALL would be inconsistent; both trust the same ABI premise.
    let mut pending = vec![];
    for reg in [instr::Reg::AX, instr::Reg::BX, instr::Reg::CX, instr::Reg::DX,
                instr::Reg::SI, instr::Reg::DI, instr::Reg::DS, instr::Reg::ES] {
      let vref = self.ir.get_var(reg, self.cur);
      if vref == Ref::Init(reg) { continue; }
      pending.push((reg, vref));
    }
    // Copies must observe program-order values (issue #78): a batched write
    // whose value reads another pending register's entry value goes first.
    for (reg, vref) in Self::order_guest_writes(&self.ir, &pending) {
      self.write_guest_register(reg, vref);
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
    // FLAGS is now unknown, so any tracked carry is invalid.
    self.clear_carry();
  }

  fn append_cond_jump(&mut self, ins: &instr::Instr, compare_opcode: Opcode) {
    let instr::Operand::Rel(rel) = &ins.operands[0] else { panic!("Expected relative offset operand for JMP") };

    let false_blk = self.get_block(ins.end_addr());
    let true_blk = self.get_block(ins.rel_addr(rel));

    let flags = self.get_flags();
    let cond = self.append_instr(Type::U16, compare_opcode, vec![flags]);

    self.append_jne(cond, true_blk, false_blk);
  }

  /// `Jcc` for a flag reader with no negated IR opcode: compose
  /// `Eq(reader(flags), 0)` the way JNS does. The reader renders as the
  /// non-fatal UNIMPL_FLAGS when its flags input is imprecise (see ast.rs),
  /// so the negated forms inherit that marker class instead of aborting.
  fn append_cond_jump_negated(&mut self, ins: &instr::Instr, compare_opcode: Opcode) {
    let instr::Operand::Rel(rel) = &ins.operands[0] else { panic!("Expected relative offset operand for JMP") };

    let false_blk = self.get_block(ins.end_addr());
    let true_blk = self.get_block(ins.rel_addr(rel));

    let flags = self.get_flags();
    let reader = self.append_instr(Type::U16, compare_opcode, vec![flags]);
    let z = self.ir.const_new(0);
    let cond = self.append_instr(Type::U16, Opcode::Eq, vec![reader, z]);

    self.append_jne(cond, true_blk, false_blk);
  }

  fn append_cond_set(&mut self, ins: &instr::Instr, compare_opcode: Opcode) {
    let flags = self.get_flags();
    let cond = self.append_instr(Type::U8, compare_opcode, vec![flags]);
    self.append_asm_dst_operand(&ins.operands[0], cond);
  }

  /// `SETcc` for a flag reader with no negated IR opcode: compose
  /// `Eq(reader(flags), 0)` the way JNS does. The reader renders as the
  /// non-fatal UNIMPL_FLAGS when its flags input is imprecise (see ast.rs),
  /// so the negated forms inherit that marker class instead of aborting.
  fn append_cond_set_negated(&mut self, ins: &instr::Instr, compare_opcode: Opcode) {
    let flags = self.get_flags();
    let reader = self.append_instr(Type::U8, compare_opcode, vec![flags]);
    let z = self.ir.const_new(0);
    let cond = self.append_instr(Type::U8, Opcode::Eq, vec![reader, z]);
    self.append_asm_dst_operand(&ins.operands[0], cond);
  }

  fn append_upper_lower_split(&mut self, r: Ref) -> (Ref, Ref) {
    let upper = self.append_instr(Type::U16, Opcode::Upper16, vec![r]);
    let lower = self.append_instr(Type::U16, Opcode::Lower16, vec![r]);
    (upper, lower)
  }

  /// Split a 16-bit register value into genuine 8-bit fragments.
  /// Unlike `append_upper_lower_split` (32 -> two 16-bit halves via
  /// Upper16/Lower16), this masks to a byte on both sides:
  /// `AL = v & 0xFF`, `AH = (v >> 8) & 0xFF`.
  fn append_byte_split(&mut self, r: Ref) -> (Ref, Ref) {
    let mask = self.ir.const_new(0xFF);
    let eight = self.ir.const_new(8);
    let shifted = self.append_instr(Type::U16, Opcode::UShr, vec![r, eight]);
    let upper = self.append_instr(Type::U16, Opcode::And, vec![shifted, mask]);
    let lower = self.append_instr(Type::U16, Opcode::And, vec![r, mask]);
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

  /// Lower DIV (`signed == false`, `UDiv`/`URem`) and IDIV (`signed ==
  /// true`, `IDiv`/`IRem`). Word form divides `DX:AX` by `r/m16`
  /// (quotient `AX`, remainder `DX`); byte form divides `AX` by `r/m8`
  /// (quotient `AL`, remainder `AH`). The quotient and the remainder
  /// are both exact: C truncates division toward zero with the
  /// dividend's sign, which is the x86 IDIV rule, and the remainder
  /// lands in the register the format table lists (DX/AH). The signed
  /// divisor is a narrower value than the divide it feeds - IDIV r/m16
  /// divides by the signed *16* bits of the divisor and IDIV r/m8 by
  /// the signed *8* bits - so those are sign-extended before the
  /// divide; a zero-extended divisor would read 0x8001 as +32769
  /// instead of -32767. Flags are architecturally undefined and
  /// clobbered to the loud-but-non-fatal unknown marker. Divide
  /// overflow (#DE) is not modeled.
  fn lower_div(&mut self, ins: &instr::Instr, signed: bool) {
    let upper_in = self.append_asm_src_operand(&ins.operands[0]);
    let lower_in = self.append_asm_src_operand(&ins.operands[1]);
    let divisor = self.append_asm_src_operand(&ins.operands[2]);
    let qop = if signed { Opcode::IDiv } else { Opcode::UDiv };
    let rop = if signed { Opcode::IRem } else { Opcode::URem };
    if Self::operand_is_word(&ins.operands[2]) {
      let dividend = self.append_instr(Type::U32, Opcode::Make32, vec![upper_in, lower_in]);
      let divisor = if signed { self.sign_word_to_dword(divisor) } else { divisor };
      let quotient = self.append_instr(Type::U32, qop, vec![dividend, divisor]);
      let (_, lower_out) = self.append_upper_lower_split(quotient);
      self.append_asm_dst_operand(&ins.operands[1], lower_out);
      let rem = self.append_instr(Type::U32, rop, vec![dividend, divisor]);
      let (_, rem_lo) = self.append_upper_lower_split(rem);
      // DX is the 16-bit remainder; a signed result carries its
      // sign-extension in the low half (0xFFFFFFFF -> 0xFFFF), so the
      // lower split is right for both signed and unsigned.
      self.append_asm_dst_operand(&ins.operands[0], rem_lo);
    } else {
      // Byte form: the dividend is AH:AL, not a 32-bit Make32.
      let hi = self.to_byte(upper_in);
      let lo = self.to_byte(lower_in);
      let eight = self.ir.const_new(8);
      let shifted = self.append_instr(Type::U16, Opcode::Shl, vec![hi, eight]);
      let dividend = self.append_instr(Type::U16, Opcode::Or, vec![shifted, lo]);
      let divisor = self.to_byte(divisor);
      let divisor = if signed { self.sign_byte_to_word(divisor) } else { divisor };
      let quotient = self.append_instr(Type::U16, qop, vec![dividend, divisor]);
      let (_, lower_out) = self.append_byte_split(quotient);
      self.append_asm_dst_operand(&ins.operands[1], lower_out);
      let rem = self.append_instr(Type::U16, rop, vec![dividend, divisor]);
      let (upper_out, _) = self.append_byte_split(rem);
      self.append_asm_dst_operand(&ins.operands[0], upper_out);
    }
    self.clobber_flags();
  }

  fn append_asm_instr(&mut self, ins: &instr::Instr) {
    self.cur_site = Some(MarkerSite { addr: ins.addr, opcode: ins.opcode });
    self.append_asm_instr_inner(ins);
    self.cur_site = None;
  }

  fn append_asm_instr_inner(&mut self, ins: &instr::Instr) {
    //println!("## {}", intel_syntax::format(ins, &[], false).unwrap());

    // Consume the push-cs marker first: it only describes the *immediately*
    // following instruction (the `push cs; call near` far-call idiom). Taking
    // it before the REP early-return below keeps e.g.
    // `push cs; rep movs; call near` from misclassifying the later call.
    let special = self.special.take();

    // REP-prefixed string instructions execute a counted loop. MOVS/STOS/
    // LODS/INS/OUTS are modeled exactly (runtime helper + deterministic
    // register updates); CMPS/SCAS model CX/SI/DI exactly but clobber flags
    // (the final comparison is data-dependent). Any rep with a possibly-set
    // direction flag stays loud.
    if ins.rep.is_some() {
      // Only 8/16-bit MOVS/STOS/LODS/CMPS/SCAS/INS/OUTS are modeled; a wider
      // (unreachable) form falls back to the loud marker rather than
      // emitting the wrong-width macro. LODS/SCAS/OUTS carry the memory
      // operand second, the others first.
      let size_oper = match ins.opcode {
        instr::Opcode::OP_LODS | instr::Opcode::OP_SCAS | instr::Opcode::OP_OUTS => &ins.operands[1],
        _ => &ins.operands[0],
      };
      let modeled = matches!(ins.opcode,
          instr::Opcode::OP_MOVS | instr::Opcode::OP_STOS | instr::Opcode::OP_LODS |
          instr::Opcode::OP_CMPS | instr::Opcode::OP_SCAS |
          instr::Opcode::OP_INS | instr::Opcode::OP_OUTS)
        && matches!(size_oper,
          instr::Operand::Mem(m) if matches!(m.sz, instr::Size::Size8 | instr::Size::Size16));
      if !modeled || !self.df_clear {
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
      // NEG writes CF (= operand != 0); NOT leaves the flags intact. A
      // following ADC/SBB must not consume a pre-NEG carry.
      if ins.opcode == instr::Opcode::OP_NEG {
        self.clear_carry();
      }
      return;
    }

    // process simple binary operations
    if let Some(opcode) = simple_binary_operation(ins.opcode) {
      let attr = if operand_is_stack_reg(&ins.operands[0]) { Attribute::STACK_PTR } else { Attribute::NONE };
      let word = Self::operand_is_word(&ins.operands[0]);
      let a = self.append_asm_src_operand(&ins.operands[0]);
      let b = self.append_asm_src_operand(&ins.operands[1]);
      // SAR lowers to the signed shift, but a byte destination holds a
      // zero-extended fragment: the sign the shift has to propagate is
      // the low byte's bit 7, which the mask threw away (`sar al,1` of
      // 0xF0 must read -16, not 240). Restore it before the shift.
      let a = if opcode == Opcode::Shr && !word {
        self.sign_byte_to_word(a)
      } else { a };
      let typ = self.deduce_type_binary(a, b);
      let vref = self.append_instr_with_attrs(typ, attr, opcode, vec![a, b]);
      self.append_asm_dst_operand(&ins.operands[0], vref);
      self.set_flags_for_binary(opcode, vref, a, b, word);
      return;
    }

    // handle less standard operations
    match &ins.opcode {
      instr::Opcode::OP_NOP => (),
      instr::Opcode::OP_SBB => {
        // SBB consumes the incoming carry (borrow). When the CF producer is
        // a tracked carry in this block, lower exactly as `a - b - cf`;
        // byte operands are masked to 8 bits (see to_byte). An unknown carry
        // stays loud rather than guessing.
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        match self.get_cf() {
          Some(cf) => {
            let word = Self::operand_is_word(&ins.operands[0]);
            let (a, b) = if word { (a, b) } else { (self.to_byte(a), self.to_byte(b)) };
            let t = self.append_instr(Type::U16, Opcode::Sub, vec![a, b]);
            let vref = self.append_instr(Type::U16, Opcode::Sub, vec![t, cf]);
            self.append_asm_dst_operand(&ins.operands[0], vref);
            // borrow_out = (a < b) | (t < cf)
            let b1 = self.sub_borrow_out(a, b);
            let b2 = self.sub_borrow_out(t, cf);
            let borrow = self.append_instr(Type::U16, Opcode::Or, vec![b1, b2]);
            self.append_update_flags_carry(vref, borrow);
          }
          None => {
            let vref = self.append_instr(typ, Opcode::Unimpl, vec![a, b]);
            self.append_asm_dst_operand(&ins.operands[0], vref);
            self.append_update_flags(vref);
          }
        }
      }
      instr::Opcode::OP_RCL | instr::Opcode::OP_RCR => {
        // RCL/RCR rotate *through* carry. With a constant count and a known
        // carry-in the value is exact via a (w+1)-bit rotate built on u32
        // ops: X = (cf<<w)|v, X' = rotated, result = X' & mask,
        // cf_out = bit w. Variable counts (CL), unknown carries, and
        // counts above w (where the CPU's count masking/modulo is
        // ambiguous to model) stay loud.
        let is_rcl = ins.opcode == instr::Opcode::OP_RCL;
        let width = Self::operand_width_bits(&ins.operands[0]);
        let count = match &ins.operands[1] {
          instr::Operand::Imm(imm) => Some(imm.val as i32),
          _ => None,
        };
        match (width, count, self.get_cf()) {
          (Some(w), Some(raw), Some(cf_in)) if (0..=w as i32).contains(&raw) => {
            let v = self.append_asm_src_operand(&ins.operands[0]);
            if raw == 0 {
              // Full no-op: result unchanged, CF preserved.
              self.append_asm_dst_operand(&ins.operands[0], v);
              self.clobber_flags_carry(cf_in);
            } else {
              let n = raw as i16;
              let zero = self.ir.const_new(0);
              let w_c = self.ir.const_new(w);
              // X = (cf << w) | v  (u32)
              let cf_wide = self.append_instr(Type::U32, Opcode::Make32, vec![zero, cf_in]);
              let cf_at_w = self.append_instr(Type::U32, Opcode::Shl, vec![cf_wide, w_c]);
              let v_wide = self.append_instr(Type::U32, Opcode::Make32, vec![zero, v]);
              let x = self.append_instr(Type::U32, Opcode::Or, vec![v_wide, cf_at_w]);
              let n_c = self.ir.const_new(n);
              let comp = self.ir.const_new((w + 1) - n);
              let (a, b) = if is_rcl {
                (self.append_instr(Type::U32, Opcode::Shl, vec![x, n_c]),
                 self.append_instr(Type::U32, Opcode::UShr, vec![x, comp]))
              } else {
                (self.append_instr(Type::U32, Opcode::UShr, vec![x, n_c]),
                 self.append_instr(Type::U32, Opcode::Shl, vec![x, comp]))
              };
              let xp = self.append_instr(Type::U32, Opcode::Or, vec![a, b]);
              let result = self.append_instr(Type::U16, Opcode::Lower16, vec![xp]);
              self.append_asm_dst_operand(&ins.operands[0], result);
              let hi = self.append_instr(Type::U32, Opcode::UShr, vec![xp, w_c]);
              let hi_lo = self.append_instr(Type::U16, Opcode::Lower16, vec![hi]);
              let one = self.ir.const_new(1);
              let cf_out = self.append_instr(Type::U16, Opcode::And, vec![hi_lo, one]);
              self.clobber_flags_carry(cf_out);
            }
          }
          _ => {
            let a = self.append_asm_src_operand(&ins.operands[0]);
            let b = self.append_asm_src_operand(&ins.operands[1]);
            let typ = self.deduce_type_binary(a, b);
            let vref = self.append_instr(typ, Opcode::Unimpl, vec![a, b]);
            self.append_asm_dst_operand(&ins.operands[0], vref);
            self.append_update_flags(vref);
          }
        }
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
        self.clobber_flags();
      }
      instr::Opcode::OP_ADC => {
        // ADC consumes the incoming carry. When the CF producer is a tracked
        // carry in this block, lower exactly as `a + b + cf`; operands of
        // either width work because the sum is computed in a u16 (the byte
        // result assigned to a byte destination truncates). Byte operands are
        // masked to 8 bits first (see to_byte). An unknown carry stays loud.
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        match self.get_cf() {
          Some(cf) => {
            let word = Self::operand_is_word(&ins.operands[0]);
            let (a, b) = if word { (a, b) } else { (self.to_byte(a), self.to_byte(b)) };
            let t = self.append_instr(Type::U16, Opcode::Add, vec![a, b]);
            let vref = self.append_instr(Type::U16, Opcode::Add, vec![t, cf]);
            self.append_asm_dst_operand(&ins.operands[0], vref);
            // The carry-out bit depends on the operand width, not the u16 the
            // sum is computed in: word operands carry out of bit 16, byte
            // operands out of bit 8. Using the bit-16 form for bytes would
            // always yield 0 (a + b + cf <= 511), silently dropping the
            // carry into a following ADC/SBB.
            let carry = if word {
              // carry_out = (t < a) | (vref < t)
              let c1 = self.add_carry_out(t, a).unwrap();
              let c2 = self.add_carry_out(vref, t).unwrap();
              self.append_instr(Type::U16, Opcode::Or, vec![c1, c2])
            } else {
              // carry_out = 0xFF < vref
              let max = self.ir.const_new(0xFF);
              self.append_instr(Type::U16, Opcode::ULt, vec![max, vref])
            };
            self.append_update_flags_carry(vref, carry);
          }
          None => {
            let vref = self.append_instr(typ, Opcode::Unimpl, vec![a, b]);
            self.append_asm_dst_operand(&ins.operands[0], vref);
            self.append_update_flags(vref);
          }
        }
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
        let ret_vals = self.return_vals();
        let dx_is_return = ret_vals.len() == 2;
        self.pin_registers();
        let ret_ref = self.append_instr(Type::Void, Opcode::RetFar, ret_vals);
        let pending = self.exit_register_values(dx_is_return);
        self.ret_exits.push(RetExit { ret_ref, pending });
      }
      instr::Opcode::OP_RET => {
        let ret_vals = self.return_vals();
        let dx_is_return = ret_vals.len() == 2;
        self.pin_registers();
        let ret_ref = self.append_instr(Type::Void, Opcode::RetNear, ret_vals);
        let pending = self.exit_register_values(dx_is_return);
        self.ret_exits.push(RetExit { ret_ref, pending });
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
        self.clear_carry(); // MUL/IMUL write CF/OF; no modeled carry
      }
      instr::Opcode::OP_IMUL => {
        // IMUL has many forms; this handles the full-product ones the
        // 0xF6/0xF7 tables produce, and the truncating 0x69/0x6B forms
        // are OP_IMUL_TRUNC above. Word is `AX * r/m16` split over
        // DX:AX; byte is `AL * r/m8`, a signed 16-bit product in AX.
        if Self::operand_is_word(&ins.operands[2]) {
          assert!(
            matches!(ins.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::DX))) &&
            matches!(ins.operands[1], instr::Operand::Reg(instr::OperandReg(instr::Reg::AX))));
          // Word IMUL is signed over the *16-bit* operands: sign-extend
          // them so the U32 product is a genuine signed 32-bit value. The
          // AST casts a U32 multiply's operands to i32 (right only when
          // they already hold signed values); zero-extending 0xFFFF to
          // 65535 would put 0001:FFFE in DX:AX for -1 * 2.
          let lhs_ref = self.append_asm_src_operand(&ins.operands[1]);
          let rhs_ref = self.append_asm_src_operand(&ins.operands[2]);
          let lhs = self.sign_word_to_dword(lhs_ref);
          let rhs = self.sign_word_to_dword(rhs_ref);
          let res = self.append_instr(Type::U32, Opcode::IMul, vec![lhs, rhs]);
          let (upper, lower) = self.append_upper_lower_split(res);
          self.append_asm_dst_operand(&ins.operands[0], upper);
          self.append_asm_dst_operand(&ins.operands[1], lower);
        } else {
          assert!(
            matches!(ins.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::AX))) &&
            matches!(ins.operands[1], instr::Operand::Reg(instr::OperandReg(instr::Reg::AL))));
          // The byte operands are signed values: a fragment holds its
          // byte masked, so 0xF0 reads +240 and the product would
          // answer for `mul` instead of for `imul`.
          let lhs_ref = self.append_asm_src_operand(&ins.operands[1]);
          let rhs_ref = self.append_asm_src_operand(&ins.operands[2]);
          let lhs = self.sign_byte_to_word(lhs_ref);
          let rhs = self.sign_byte_to_word(rhs_ref);
          let res = self.append_instr(Type::U16, Opcode::IMul, vec![lhs, rhs]);
          self.append_asm_dst_operand(&ins.operands[0], res);
        }
        self.clear_carry(); // MUL/IMUL write CF/OF; no modeled carry
      }
      instr::Opcode::OP_MUL => {
        // The byte form's product is `AL * r/m8` - 16 bits, all of AX -
        // and the word form's is `AX * r/m16` split over DX:AX.
        if Self::operand_is_word(&ins.operands[2]) {
          assert!(
            matches!(ins.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::DX))) &&
            matches!(ins.operands[1], instr::Operand::Reg(instr::OperandReg(instr::Reg::AX))));
          let lhs = self.append_asm_src_operand(&ins.operands[1]);
          let rhs = self.append_asm_src_operand(&ins.operands[2]);
          let res = self.append_instr(Type::U32, Opcode::UMul, vec![lhs, rhs]);
          let (upper, lower) = self.append_upper_lower_split(res);
          self.append_asm_dst_operand(&ins.operands[0], upper);
          self.append_asm_dst_operand(&ins.operands[1], lower);
        } else {
          assert!(
            matches!(ins.operands[0], instr::Operand::Reg(instr::OperandReg(instr::Reg::AX))) &&
            matches!(ins.operands[1], instr::Operand::Reg(instr::OperandReg(instr::Reg::AL))));
          let lhs = self.append_asm_src_operand(&ins.operands[1]);
          let rhs = self.append_asm_src_operand(&ins.operands[2]);
          let res = self.append_instr(Type::U16, Opcode::UMul, vec![lhs, rhs]);
          self.append_asm_dst_operand(&ins.operands[0], res);
        }
        self.clear_carry(); // MUL/IMUL write CF/OF; no modeled carry
      }
      instr::Opcode::OP_INC => {
        let attr = if operand_is_stack_reg(&ins.operands[0]) { Attribute::STACK_PTR } else { Attribute::NONE };
        let one = self.ir.const_new(1);
        let vref = self.append_asm_src_operand(&ins.operands[0]);
        let typ = self.deduce_type_binary(vref, one);
        let vref = self.append_instr_with_attrs(typ, attr, Opcode::Add, vec![vref, one]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        // INC/DEC update ZF/SF/OF/AF but leave CF intact.
        let cf = self.last_cf;
        let known = self.carry_known;
        self.append_update_flags(vref);
        self.last_cf = cf;
        self.carry_known = known;
      }
      instr::Opcode::OP_DEC => {
        let attr = if operand_is_stack_reg(&ins.operands[0]) { Attribute::STACK_PTR } else { Attribute::NONE };
        let one = self.ir.const_new(1);
        let vref = self.append_asm_src_operand(&ins.operands[0]);
        let typ = self.deduce_type_binary(vref, one);
        let vref = self.append_instr_with_attrs(typ, attr, Opcode::Sub, vec![vref, one]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        // DEC, like INC, leaves CF intact.
        let cf = self.last_cf;
        let known = self.carry_known;
        self.append_update_flags(vref);
        self.last_cf = cf;
        self.carry_known = known;
      }
      instr::Opcode::OP_JMP => {
        match ins.operands.get(0) {
          Some(instr::Operand::Mem(m)) => {
            // Special handling for some indirect jumps: a `jmp cs:[bx+off]`
            // whose target table the config describes lowers to a jump
            // table. Any other indirect jump has no statically known
            // target, so it ends the block loudly instead of aborting
            // the whole decompile (issue #112).
            match self.jump_indirect_targets(ins, m) {
              IndirectTargets::Resolved(targets) => {
                let Some(reg) = m.reg1 else {
                  panic!("Shouldn't fail.. should be checked by jump_indirect_targets already");
                };
                let reg_ref = self.ir.get_var(reg, self.cur);
                self.append_jmptbl(reg_ref, targets);
              }
              // A jump of the config-described jump-table shape whose
              // table the config does not have is a missing annotation,
              // not a run-time-only target: say so, so the operator can
              // add the region.
              IndirectTargets::NoConfigRegion => {
                self.append_unmodelled_transfer(ins, Cause::IndirectJumpNoTable, &format!(
                  "WARN: Unmodelled indirect jump '{}' at {} — the config has no jump table for it; emitting UNIMPL marker",
                  instr_str(ins), ins.addr));
              }
              IndirectTargets::NotTableShape => {
                self.append_unmodelled_transfer(ins, Cause::IndirectJumpRuntimeTarget, &format!(
                  "WARN: Unmodelled indirect jump '{}' at {} — target is only known at run time; emitting UNIMPL marker",
                  instr_str(ins), ins.addr));
              }
              IndirectTargets::DataRegion { name, start, end } => {
                self.append_unmodelled_transfer(ins, Cause::IndirectJumpNoTable, &format!(
                  "WARN: Unmodelled indirect jump '{}' at {} — operand lands on declared data '{}' [{}, {}) and is not a jump table; emitting UNIMPL marker",
                  instr_str(ins), ins.addr, name, start, end));
              }
            }
          }
          Some(instr::Operand::Rel(rel)) => {
            let blkref = self.get_block(ins.rel_addr(rel));
            self.append_jmp(blkref);
          }
          // `jmp <reg>` (e.g. `jmp ax`): the target is a register value,
          // known only at run time.
          _ => {
            self.append_unmodelled_transfer(ins, Cause::IndirectJumpRuntimeTarget, &format!(
              "WARN: Unmodelled indirect jump '{}' at {} — target is only known at run time; emitting UNIMPL marker",
              instr_str(ins), ins.addr));
          }
        }
      }

      instr::Opcode::OP_JMPF => {
        // A far jump transfers control to another segment (or to a target
        // read from memory), which the IR cannot express: hydra has no
        // far-jump primitive and the IR has no cross-segment control
        // flow. A far jump inside the function's own code segment does
        // not change CS, so it lowers to a plain jump; every other far
        // jump ends the block loudly (issue #112).
        let kind = branch::unresolved_transfer(ins);
        match self.transfer_model(kind) {
          Transfer::SameSegment { target } => {
            let blkref = self.get_block(target);
            self.append_jmp(blkref);
          }
          Transfer::Unmodelled => {
            // A far jump is either a direct one (an explicit segment) or
            // an indirect one (the target is read from memory); the
            // near-indirect kind never reaches this arm.
            let msg = match kind {
              branch::UnresolvedTransfer::FarDirect { seg, off } => format!(
                "WARN: Unmodelled far jump '{}' at {} — target {:04x}:{:04x} is not in the function's code segment; emitting UNIMPL marker",
                instr_str(ins), ins.addr, seg, off),
              branch::UnresolvedTransfer::FarIndirect => format!(
                "WARN: Unmodelled far jump '{}' at {} — target is read from memory at run time; emitting UNIMPL marker",
                instr_str(ins), ins.addr),
              branch::UnresolvedTransfer::NearIndirect => unreachable!("a far jump is never near-indirect"),
            };
            self.append_unmodelled_transfer(ins, Cause::UnmodelledFarJump, &msg);
          }
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

      instr::Opcode::OP_JO  => self.append_cond_jump(ins, Opcode::OverflowFlags),
      instr::Opcode::OP_JNO => self.append_cond_jump_negated(ins, Opcode::OverflowFlags),
      instr::Opcode::OP_JP  => self.append_cond_jump(ins, Opcode::ParityFlags),
      instr::Opcode::OP_JNP => self.append_cond_jump_negated(ins, Opcode::ParityFlags),

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
        // A call may clobber flags (unmodeled): the tracked carry is invalid.
        self.clear_carry();
        self.process_callf(ins);
      }
      instr::Opcode::OP_CALL => {
        let cs_pushed = matches!(special, Some(SpecialState::PushCS));
        self.clear_carry();
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
        // TEST clears CF (like the logical ops).
        let zero = self.ir.const_new(0);
        self.append_update_flags_carry(vref, zero);
      }
      instr::Opcode::OP_CMP => {
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        let vref = self.append_instr(typ, Opcode::Sub, vec![a, b]);
        let word = Self::operand_is_word(&ins.operands[0]);
        let (a, b) = if word { (a, b) } else { (self.to_byte(a), self.to_byte(b)) };
        let cf = self.sub_borrow_out(a, b);
        self.append_update_flags_carry(vref, cf);
      }
      instr::Opcode::OP_CWD => {
        let src = self.append_asm_src_operand(&ins.operands[1]);
        let vref = self.append_instr(Type::U32, Opcode::SignExtTo32, vec![src]);
        let upper = self.append_instr(Type::U16, Opcode::Upper16, vec![vref]);
        self.append_asm_dst_operand(&ins.operands[0], upper);
      }
      instr::Opcode::OP_DIV => self.lower_div(ins, false),
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
        // ins: operands [ES:DI, DX]. The helper executes the raw instruction
        // (port read -> ES:DI); DI advances by size. A possibly-set direction
        // flag makes the advance direction unknown, so stay loud.
        if !self.df_clear {
          self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
        } else {
          let dst = &ins.operands[0]; // ES:DI
          let size = Self::string_operand_size(dst);
          let es = self.ir.get_var(Self::string_operand_seg(dst, instr::Reg::ES), self.cur);
          let di = self.ir.get_var(instr::Reg::DI, self.cur);
          let port = self.append_asm_src_operand(&ins.operands[1]); // DX
          let size_c = self.ir.const_new(size);
          self.append_instr(Type::Void, Opcode::Ins, vec![es, di, port, size_c]);
          let di_n = self.append_instr(Type::U16, Opcode::Add, vec![di, size_c]);
          self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::DI), di_n);
        }
      }
      instr::Opcode::OP_OUTS => {
        // outs: operands [DX, DS:SI]. The helper executes the raw instruction
        // (DS:SI -> port); SI advances by size. A possibly-set direction flag
        // makes the advance direction unknown, so stay loud.
        if !self.df_clear {
          self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
        } else {
          let src = &ins.operands[1]; // DS:SI
          let size = Self::string_operand_size(src);
          let ds = self.ir.get_var(Self::string_operand_seg(src, instr::Reg::DS), self.cur);
          let si = self.ir.get_var(instr::Reg::SI, self.cur);
          let port = self.append_asm_src_operand(&ins.operands[0]); // DX
          let size_c = self.ir.const_new(size);
          self.append_instr(Type::Void, Opcode::Outs, vec![ds, si, port, size_c]);
          let si_n = self.append_instr(Type::U16, Opcode::Add, vec![si, size_c]);
          self.append_asm_dst_reg(&instr::OperandReg(instr::Reg::SI), si_n);
        }
      }
      instr::Opcode::OP_STD => {
        // Sets DF: string ops after this point (in this block) no longer
        // have a provably clear direction. Emitted as a non-fatal side
        // effect; the per-point `df_clear` state drives the models.
        self.df_clear = false;
        self.append_instr(Type::Void, Opcode::Std, vec![]);
      }
      instr::Opcode::OP_STC | instr::Opcode::OP_CLC | instr::Opcode::OP_CMC => {
        // These set/clear/complement CF. The IR carries one opaque flags
        // value, so redefining it here makes a subsequent jb/jae degrade to
        // UNIMPL_FLAGS rather than silently reading a stale carry. The
        // known carry-out is still tracked for a following ADC/SBB.
        let cf = match ins.opcode {
          instr::Opcode::OP_STC => Some(self.ir.const_new(1)),
          instr::Opcode::OP_CLC => Some(self.ir.const_new(0)),
          // CMC complements the tracked carry; an unknown carry stays unknown.
          _ => match self.get_cf() {
            Some(prev) => {
              let one = self.ir.const_new(1);
              Some(self.append_instr(Type::U16, Opcode::Xor, vec![prev, one]))
            }
            None => None,
          },
        };
        match cf {
          Some(cf) => self.clobber_flags_carry(cf),
          None => self.clobber_flags(),
        }
      }
      instr::Opcode::OP_LODS => {
        let src = self.append_asm_src_operand(&ins.operands[1]);
        self.append_asm_dst_operand(&ins.operands[0], src);
        self.string_op_advance(Some(&ins.operands[1]), None);
      }
      instr::Opcode::OP_STI => {
        self.append_instr(Type::Void, Opcode::Sti, vec![]);
      }
      instr::Opcode::OP_CLI => {
        self.append_instr(Type::Void, Opcode::Cli, vec![]);
      }
      instr::Opcode::OP_IN => {
        // `in AL/AX, port`: operands[0] = destination, operands[1] = port
        // (immediate or DX). Reads a port; no FLAGS/memory effect.
        let port = self.append_asm_src_operand(&ins.operands[1]);
        let (typ, op) = match &ins.operands[0] {
          instr::Operand::Reg(instr::OperandReg(instr::Reg::AL)) => (Type::U8, Opcode::In8),
          instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)) => (Type::U16, Opcode::In16),
          other => panic!("Unsupported IN destination: {:?}", other),
        };
        let v = self.append_instr(typ, op, vec![port]);
        self.append_asm_dst_operand(&ins.operands[0], v);
      }
      instr::Opcode::OP_OUT => {
        // `out port, AL/AX`: operands[0] = port, operands[1] = source.
        let port = self.append_asm_src_operand(&ins.operands[0]);
        let val = self.append_asm_src_operand(&ins.operands[1]);
        let op = match &ins.operands[1] {
          instr::Operand::Reg(instr::OperandReg(instr::Reg::AL)) => Opcode::Out8,
          instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)) => Opcode::Out16,
          other => panic!("Unsupported OUT source: {:?}", other),
        };
        self.append_instr(Type::Void, op, vec![port, val]);
      }
      instr::Opcode::OP_CLD => {
        // Clears DF: string ops after this point (in this block) have a
        // provably clear direction even when a STD exists elsewhere.
        self.df_clear = true;
        self.append_instr(Type::Void, Opcode::Cld, vec![]);
      }
      instr::Opcode::OP_CBW => {
        let src = self.append_asm_src_operand(&ins.operands[1]);
        let ext = self.append_instr(Type::U16, Opcode::SignExtTo16, vec![src]);
        self.append_asm_dst_operand(&ins.operands[0], ext);
      }
      instr::Opcode::OP_XLAT => {
        // AL = DS:[BX + AL]; flags are preserved. Operands are
        // [AL, DS, BX]; routing the result through the AL destination
        // re-splits the byte fragments.
        let base = self.append_asm_src_operand(&ins.operands[2]);
        let idx = self.append_asm_src_operand(&ins.operands[0]);
        let typ = self.deduce_type_binary(base, idx);
        let addr = self.append_instr(typ, Opcode::Add, vec![base, idx]);
        let seg = self.append_asm_src_operand(&ins.operands[1]);
        let v = self.append_instr(Type::U8, Opcode::Load8, vec![seg, addr]);
        self.append_asm_dst_operand(&ins.operands[0], v);
      }
      instr::Opcode::OP_IDIV => self.lower_div(ins, true),
      instr::Opcode::OP_ENTER => {
        // operands: [BP, IMM16 frame size, IMM8 nesting level]. Level 0
        // (the overwhelmingly common case) is push BP; BP = SP; SP -= frame.
        // Deeper nesting copies whole frame chains and stays loud.
        let level = match &ins.operands[2] {
          instr::Operand::Imm(imm) => imm.val,
          _ => 1, // Unreachable via decode; fall through to the marker.
        };
        if level != 0 {
          self.warn_ins(Cause::Unimpl, ins,
                        format!("Unimpl ENTER nesting level {level} at {} — emitting UNIMPL marker", ins.addr));
          self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
        } else {
          let bp = self.ir.get_var(instr::Reg::BP, self.cur);
          self.append_push(bp);
          let sp = self.ir.get_var(instr::Reg::SP, self.cur);
          self.ir.set_var(instr::Reg::BP, self.cur, sp);
          let frame = self.append_asm_src_operand(&ins.operands[1]);
          let sp = self.append_instr_with_attrs(Type::U16, Attribute::STACK_PTR, Opcode::Sub, vec![sp, frame]);
          self.ir.set_var(instr::Reg::SP, self.cur, sp);
        }
      }
      instr::Opcode::OP_SETS  => self.append_cond_set(ins, Opcode::SignFlags),
      instr::Opcode::OP_SETO  => self.append_cond_set(ins, Opcode::OverflowFlags),
      instr::Opcode::OP_SETP  => self.append_cond_set(ins, Opcode::ParityFlags),
      instr::Opcode::OP_SETNS => self.append_cond_set_negated(ins, Opcode::SignFlags),
      instr::Opcode::OP_SETNO => self.append_cond_set_negated(ins, Opcode::OverflowFlags),
      instr::Opcode::OP_SETNP => self.append_cond_set_negated(ins, Opcode::ParityFlags),
      instr::Opcode::OP_LAHF => {
        // AH = low flags byte. Flags are one opaque value, so this stores
        // it to AH; SAHF round-trips it back exactly, and any branch on
        // the result degrades to UNIMPL_FLAGS.
        let flags = self.get_flags();
        self.append_asm_dst_operand(&ins.operands[0], flags);
      }
      instr::Opcode::OP_SAHF => {
        // Flags = AH (the LAHF mirror). The new flags are data-dependent,
        // so following branches degrade to loud UNIMPL_FLAGS.
        let a = self.append_asm_src_operand(&ins.operands[0]);
        self.append_update_flags(a);
      }
      instr::Opcode::OP_PUSHF => {
        // Push the opaque flags value; POPF round-trips it back.
        let flags = self.get_flags();
        self.append_push(flags);
      }
      instr::Opcode::OP_POPF => {
        // Flags = popped value, which may change DF too: string ops after
        // this point (in this block) no longer have a provably clear
        // direction. Following branches degrade to UNIMPL_FLAGS.
        self.df_clear = false;
        let v = self.append_pop();
        self.append_update_flags(v);
      }
      instr::Opcode::OP_AAA | instr::Opcode::OP_AAS => {
        // BCD adjust of AL/AH: a distinct explicit unknown per fragment
        // (any use traps at runtime; sharing one SSA value would let
        // `cmp al,ah` fold), flags defined-but-unknown.
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let b = self.append_asm_src_operand(&ins.operands[1]);
        let typ = self.deduce_type_binary(a, b);
        self.warn_ins(Cause::Unimpl, ins,
                      format!("Unimpl opcode {:?} at {} — emitting UNIMPL markers", ins.opcode, ins.addr));
        let lo = self.append_instr(typ.clone(), Opcode::Unimpl, vec![a, b]);
        self.append_asm_dst_operand(&ins.operands[0], lo);
        let hi = self.append_instr(typ, Opcode::Unimpl, vec![a, b]);
        self.append_asm_dst_operand(&ins.operands[1], hi);
        self.append_update_flags(lo);
      }
      instr::Opcode::OP_DAA | instr::Opcode::OP_DAS => {
        // BCD adjust of AL: explicit unknown, flags defined-but-unknown.
        let a = self.append_asm_src_operand(&ins.operands[0]);
        let typ = self.deduce_type_unary(a);
        self.warn_ins(Cause::Unimpl, ins,
                      format!("Unimpl opcode {:?} at {} — emitting UNIMPL marker", ins.opcode, ins.addr));
        let vref = self.append_instr(typ, Opcode::Unimpl, vec![a]);
        self.append_asm_dst_operand(&ins.operands[0], vref);
        self.append_update_flags(vref);
      }
      instr::Opcode::OP_IRET => {
        // IRET restores CS:IP and FLAGS, so DF becomes unknown for any code
        // that follows it in the block; the return itself is not modeled.
        self.df_clear = false;
        self.warn_ins(Cause::Unimpl, ins,
                      format!("Unimpl opcode {:?} at {} — emitting UNIMPL marker", ins.opcode, ins.addr));
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      instr::Opcode::OP_HLT | instr::Opcode::OP_INTO | instr::Opcode::OP_INVAL => {
        // Halt / overflow-trap / internal-invalid: no continuation model.
        // A fatal marker (only if executed) beats aborting the whole
        // decompile at build time. INTO over-approximates: it traps only
        // on an untracked overflow, but staying loud forces the model gap
        // visible instead of silently succeeding.
        self.warn_ins(Cause::Unimpl, ins,
                      format!("Unimpl opcode {:?} at {} — emitting UNIMPL marker", ins.opcode, ins.addr));
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
      _ => {
        // Loud-but-non-fatal fallback (issue #85): a single unlowered
        // instruction must not abort the whole decompile. The build-time
        // warning names the opcode; the runtime UNIMPL() trap fires only
        // if the instruction actually executes.
        self.warn_ins(Cause::Unimpl, ins,
                      format!("Unimpl opcode {:?} at {} — emitting UNIMPL marker", ins.opcode, ins.addr));
        self.append_instr(Type::Void, Opcode::Unimpl, vec![]);
      }
    }
  }

  fn build(&mut self) {
    // CF and DF threading across blocks are both must-analyses: a producer's
    // carry-out is known only if its own carry-in was, and a block's entry DF
    // is clear only if *every* predecessor leaves it clear (the annotated
    // function entry assumes DF=0). Both results feed the IR, so it must be
    // built with the converged entry state.
    //
    // Rebuilding the whole IR once per CFG hop would be O(blocks^2). Instead,
    // each analysis extracts every block's transfer from two extreme probe
    // passes — all entries unknown, then all known/clear. A block whose exit
    // flips with the assumption merely forwards its entry (identity);
    // otherwise its exit is constant. The analyses then run over these
    // summaries alone. With both CF and DF active the IR is therefore built
    // five times at most — two DF probes, two carry probes, and the retained
    // build — each probe set skipped when its consumer/writer pair is absent.
    //
    // DF converges first: its transfer never depends on carry, so it needs no
    // carry seed. Carry converges second with the DF map, so its probes model
    // the same string-op direction guards the retained build will use — the
    // loud rep path clobbers flags, so a probe that assumed DF clear would
    // disagree with the final IR.
    let cfg = self.cfg;
    let instrs = self.instrs;
    let spec = self.spec;
    let binary = self.binary;
    let overlay = self.overlay;
    let pin_all = self.pin_all;

    // Entry-knownness only reaches the IR through get_cf (ADC/SBB/RCL/RCR/
    // CMC) and DF clearness only through the string-op direction guards.
    // Without a consumer the respective analysis cannot change the output.
    let has_carry_consumer = instrs.iter().any(|i| matches!(i.opcode,
      instr::Opcode::OP_ADC | instr::Opcode::OP_SBB |
      instr::Opcode::OP_RCL | instr::Opcode::OP_RCR |
      instr::Opcode::OP_CMC));
    let has_df_consumer = instrs.iter().any(|i| matches!(i.opcode,
      instr::Opcode::OP_MOVS | instr::Opcode::OP_STOS | instr::Opcode::OP_LODS |
      instr::Opcode::OP_CMPS | instr::Opcode::OP_SCAS |
      instr::Opcode::OP_INS | instr::Opcode::OP_OUTS));
    let has_df_writer = instrs.iter().any(|i| opcode_may_set_df(i.opcode));

    // Converge DF entry clearness first (see above). Only a DF writer can
    // make a later block's entry unclear, and only a string op consumes it,
    // so the analysis is skipped when either is absent (then DF stays clear).
    let df_entry = if has_df_consumer && has_df_writer {
      Some(analyze_df(cfg, instrs, spec, binary, overlay, pin_all))
    } else {
      None
    };

    // The retained build. Without a DF writer DF stays clear everywhere;
    // otherwise only the entry is clear unless the analysis proves more.
    let mut retained = IRBuilder::new(cfg, instrs, spec, binary, overlay, pin_all);
    match &df_entry {
      Some(entry) => {
        retained.df_clear_entry = entry.clone();
        retained.df_entry_default = false;
      }
      None => retained.df_entry_default = !has_df_writer,
    }
    if has_carry_consumer {
      retained.carry_known_entry =
        analyze_carry(cfg, instrs, spec, binary, overlay, pin_all, df_entry.as_ref());
    }
    retained.build_once();
    retained.write_back_exit_registers();
    retained.check_regargs_setups();

    *self = retained;
  }

  /// Emit the retained build pass's buffered diagnostics (see `warn`).
  fn emit_warnings(&self) {
    for w in &self.warnings {
      eprintln!("{}", w.message);
    }
  }

  /// Warn when a known call sets up a register its `regargs` does not declare
  /// and nothing live in the caller reads that value (issue #132).
  ///
  /// Such a value exists only for the callee: nothing observable consumes it,
  /// so dead-code elimination removes the setup and the callee reads whatever
  /// the register held before. That outcome is silent otherwise, so the
  /// operator gets a diagnostic instead. The check runs on the finished raw IR
  /// (after `build_once`), so "does anything read it" is a def-use question
  /// over the instructions that will actually survive into the output
  /// (`live_refs`). A use before the call (for example copying the value into
  /// a declared regarg) counts: the value is not dropped. `regargs: None` and
  /// calls in unreachable blocks are not checked; there is no emitted call to
  /// warn about. So are values that are just a call's return: AX/DX after a
  /// call hold its result, not a caller setup for the next one.
  fn check_regargs_setups(&mut self) {
    if self.regargs_candidates.is_empty() { return; }
    let reachable = self.reachable_blocks();
    let live = self.live_refs(&reachable);
    let reads = self.live_reads(&live);
    for cand in self.regargs_candidates.clone() {
      if !reachable.contains(&cand.blk) { continue; }
      // Candidates are recorded while blocks are unsealed, so a merge-block
      // phi looked dirty then. Now that every phi has its operands, drop the
      // ones that turned out to carry only entry values.
      let missing: Vec<(instr::Reg, Ref)> = cand.missing.iter()
        .filter(|(reg, vref)| !Self::is_entry_value(&self.ir, *vref, *reg))
        .filter(|(_, vref)| !Self::is_call_result(&self.ir, *vref))
        .copied()
        .collect();
      if missing.is_empty() { continue; }
      let dropped: Vec<&str> = missing.iter()
        .filter(|(_, vref)| !reads.contains(vref))
        .map(|(reg, _)| reg.info().name)
        .collect();
      if dropped.is_empty() { continue; }
      let pronoun = if dropped.len() == 1 { "it" } else { "them" };
      self.warn(Cause::RegargsSetupDropped, Some(cand.call_addr), None, format!(
        "WARN: {}: call to {} at {} sets up {} but the callee's regargs does not declare {}; the setup is dropped and the callee reads the stale register value",
        self.spec.name, cand.callee, cand.call_addr, dropped.join(", "), pronoun));
    }
  }

  /// The blocks control flow can reach from the entry block (`BlockRef(0)`,
  /// created first by `IRBuilder::new`). Calls in the rest are never emitted,
  /// so their unpopulated phis must not produce warnings.
  fn reachable_blocks(&self) -> HashSet<BlockRef> {
    let mut reachable = HashSet::new();
    let mut stack = vec![BlockRef(0)];
    while let Some(blk) = stack.pop() {
      if !reachable.insert(blk) { continue; }
      stack.extend(self.ir.block_exits(blk));
    }
    reachable
  }

  /// The instructions in reachable blocks whose values reach the emitted
  /// output, mirroring the roots `deadcode_elimination` uses (`opt.rs`):
  /// side-effecting or `PIN`. Two roots are not evidence that the emitted C
  /// uses a value: `Pin` (the AST drops it, see ast.rs) and the exit-state
  /// write-backs (they preserve the caller's `ret` state, not this call's
  /// inputs). Leaves them out so the diagnostic does not mistake either for a
  /// caller read.
  fn live_refs(&self, reachable: &HashSet<BlockRef>) -> HashSet<Ref> {
    let mut roots = vec![];
    for blk in self.ir.iter_blocks() {
      if !reachable.contains(&blk) { continue; }
      for r in self.ir.iter_instrs(blk) {
        let Some(instr) = self.ir.instr(r) else { continue; };
        let root = (instr.opcode.has_side_effects() || (instr.attrs & Attribute::PIN) != 0)
          && instr.opcode != Opcode::Pin
          && (instr.attrs & Attribute::EXIT_STATE) == 0;
        if root { roots.push(r); }
      }
    }

    let mut live = HashSet::new();
    while let Some(r) = roots.pop() {
      if !live.insert(r) { continue; }
      let Some(instr) = self.ir.instr(r) else { continue; };
      for operand in &instr.operands {
        if matches!(operand, Ref::Instr(_, _)) { roots.push(*operand); }
      }
    }
    live
  }

  /// Every value that a live instruction reads, directly or through a derived
  /// value (a byte fragment, a recompose, a copy). A setup used before the
  /// call is a real consumer too, so this is not restricted to the call's
  /// successors the way a "still live afterwards" question would be.
  fn live_reads(&self, live: &HashSet<Ref>) -> HashSet<Ref> {
    let mut reads = HashSet::new();
    let mut stack = vec![];
    for r in live {
      if let Some(instr) = self.ir.instr(*r) {
        // A contract refresh reads the live register, not the caller setup
        // that may have preceded the call.
        if (instr.attrs & Attribute::CALL_CONTRACT) != 0 { continue; }
        stack.extend(instr.operands.iter().copied());
      }
    }
    while let Some(r) = stack.pop() {
      if !reads.insert(r) { continue; }
      if let Some(instr) = self.ir.instr(r) {
        if (instr.attrs & Attribute::CALL_CONTRACT) != 0 { continue; }
        stack.extend(instr.operands.iter().copied());
      }
    }
    reads
  }

  /// Whether `r` is a call's return value or a pure projection of one
  /// (`save_return_value` splits a U32 result with Upper16/Lower16 and
  /// refreshes byte fragments with masks and shifts). When the caller ignores
  /// such a result it is exit state, not a setup for the next call, so it must
  /// not be reported as a dropped callee input.
  fn is_call_result(ir: &IR, r: Ref) -> bool {
    let mut memo = HashMap::new();
    let mut visiting = HashSet::new();
    Self::is_call_result_inner(ir, r, &mut memo, &mut visiting)
  }

  fn is_call_result_inner(ir: &IR, r: Ref, memo: &mut HashMap<Ref, bool>,
                          visiting: &mut HashSet<Ref>) -> bool {
    if let Some(known) = memo.get(&r) { return *known; }
    if !visiting.insert(r) { return false; }
    let result = match r {
      Ref::Instr(_, _) => match ir.instr(r) {
        Some(instr) => match instr.opcode {
          Opcode::CallNear | Opcode::CallFar | Opcode::CallNearPtr | Opcode::CallFarPtr
            | Opcode::CallArgs => true,
          Opcode::Ref | Opcode::Upper16 | Opcode::Lower16 => instr.operands.iter()
            .any(|operand| Self::is_call_result_inner(ir, *operand, memo, visiting)),
          Opcode::And | Opcode::Or | Opcode::UShr | Opcode::Shl => {
            let mut saw_value = false;
            let mut all_from_call = true;
            for operand in &instr.operands {
              if matches!(operand, Ref::Const(_)) { continue; }
              saw_value = true;
              all_from_call &= Self::is_call_result_inner(ir, *operand, memo, visiting);
            }
            saw_value && all_from_call
          }
          _ => false,
        },
        None => false,
      },
      _ => false,
    };
    visiting.remove(&r);
    memo.insert(r, result);
    result
  }

  fn build_once(&mut self) {
    // Step 0: Direction-flag facts. DF is written only by STD and the
    // flag-restoring POPF/IRET (CLD clears it). An annotated function entry
    // assumes DF=0 (the DOS ABI convention, docs/8086_flags.txt); later block
    // entries are seeded by the must-analysis in `build`.
    self.df_clear = true;

    // Step 1: Infer basic-block boundaries
    let mut block_start = HashSet::new();
    let first_addr = self.instrs.first().map(|ins| ins.addr);
    let mut region_boundaries = HashSet::new();
    for region in &self.code_regions {
      if Some(region.start) != first_addr {
        block_start.insert(region.start);
        region_boundaries.insert(region.start);
      }
    }
    let mut last_was_jump = false;
    for ins in self.instrs {
      if last_was_jump {
        block_start.insert(ins.addr);
        last_was_jump = false;
      }
      let Some(branch_targets) = self.jump_targets(ins) else { continue };
      match branch_targets {
        BranchTargets::Targets { targets: explicit, fallthrough } => {
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
              self.warn_ins(Cause::BranchOutOfRange, ins,
                            format!("WARN: {}: branch at {} targets {} outside function {}; the edge is dropped from the output",
                self.spec.name, ins.addr, tgt, self.spec.range_description_for(&self.code_regions)));
            }
          }
        }
        BranchTargets::Unresolved(kind) => {
          // A transfer with no statically known target: no block is created
          // for it. The lowering ends the block at the transfer (loudly), so
          // the code after it becomes a separate, unreachable block.
          if let Transfer::SameSegment { target } = self.transfer_model(kind) {
            // A far jump inside the function's own segment is a plain jump
            // to that address: it needs a block like any other target.
            block_start.insert(target);
            if self.out_of_range_edge(target, None) {
              self.warn_ins(Cause::BranchOutOfRange, ins,
                            format!("WARN: {}: branch at {} targets {} outside function {}; the edge is dropped from the output",
                self.spec.name, ins.addr, target, self.spec.range_description_for(&self.code_regions)));
            }
          }
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
    let entry_region = self.code_regions.iter()
      .find(|region| region.contains(self.spec.start))
      .copied();
    let mut prestart_last = None;
    for ins in self.instrs {
      if DEBUG { println!("DEBUG: {}", instr_str(ins)); }
      if block_start.get(&ins.addr).is_some() {
        if region_boundaries.contains(&ins.addr) {
          self.start_next_region_blk(ins.addr);
        } else {
          self.start_next_blk(ins.addr);
        }
      }
      if let Some(region) = entry_region {
        if ins.addr.seg == region.start.seg
          && ins.addr.off >= region.start.off
          && ins.addr.off < self.spec.start.off
        {
          prestart_last = Some((ins.end_addr(), self.cur));
        }
      }
      self.append_asm_instr(ins);
    }
    if let Some((last_end, block)) = prestart_last {
      if last_end == self.spec.start {
        self.link_prestart_fallthrough(block);
      }
    }
    // The last block is never switched away from; record its carry/DF exit.
    self.exit_known.insert(self.cur, self.carry_known);
    self.df_exit_clear.insert(self.cur, self.df_clear);

    // Step 4: walk blocks and seal them
    for blkref in self.ir.iter_blocks() {
      if !self.ir.block(blkref).sealed {
        self.ir.seal_block(blkref);
      }
    }
  }
}

/// Seed a probe builder with the converged DF map so its string-op direction
/// guards match the retained build. With no map (no DF writer, or no consumer)
/// DF is provably clear everywhere.
fn seed_df<'a>(builder: &mut IRBuilder<'a>, df_entry: Option<&HashMap<BlockRef, bool>>) {
  if let Some(entry) = df_entry {
    builder.df_clear_entry = entry.clone();
    builder.df_entry_default = false;
  }
}

/// Converge the per-block carry entry-knownness (the CF must-analysis; see
/// `IRBuilder::build`). Least fixpoint: unknown starts false and only grows.
/// `df_entry` is the converged DF map (see `analyze_df`); the probes seed it so
/// a DF-unclear `rep` in the summary clobbers flags exactly as it will in the
/// retained build.
fn analyze_carry<'a>(
  cfg: &'a Config,
  instrs: &'a [instr::Instr],
  spec: &'a spec::Spec<'a>,
  binary: &'a binary::Binary,
  overlay: bool,
  pin_all: bool,
  df_entry: Option<&HashMap<BlockRef, bool>>,
) -> HashMap<BlockRef, bool> {
  // Pass 1: assume no block entry carries a known value (the conservative
  // default), keeping the IR only for its per-block summaries.
  let mut probe = IRBuilder::new(cfg, instrs, spec, binary, overlay, pin_all);
  seed_df(&mut probe, df_entry);
  probe.build_once();
  let exit_unknown = probe.exit_known.clone();
  let blocks: Vec<BlockRef> = probe.ir.iter_blocks().collect();
  let preds: HashMap<BlockRef, Vec<BlockRef>> = blocks
    .iter()
    .map(|b| (*b, probe.ir.block(*b).preds.clone()))
    .collect();

  // Pass 2: assume every block entry carries a known value, to distinguish
  // an identity transfer from a constant one.
  let mut probe2 = IRBuilder::new(cfg, instrs, spec, binary, overlay, pin_all);
  probe2.entry_known_default = true;
  seed_df(&mut probe2, df_entry);
  probe2.build_once();
  let exit_known = probe2.exit_known.clone();

  // Must-analysis over the per-block summaries (no IR rebuilds). Constant
  // transfers drop out immediately; identity transfers forward the
  // predecessor's entry knownness. Unknown starts false and only grows, so
  // the fixpoint terminates.
  let mut in_known: HashMap<BlockRef, bool> = blocks.iter().map(|b| (*b, false)).collect();
  loop {
    let mut changed = false;
    for b in &blocks {
      // The function entry block has no CF predecessor; its carry-in is
      // always unknown.
      let known = if *b == BlockRef(0) {
        false
      } else {
        let ps = &preds[b];
        !ps.is_empty() && ps.iter().all(|p| {
          let base = exit_unknown.get(p).copied().unwrap_or(false);
          if Some(&base) == exit_known.get(p) {
            base // constant transfer
          } else {
            in_known.get(p).copied().unwrap_or(false) // identity transfer
          }
        })
      };
      if in_known[b] != known {
        in_known.insert(*b, known);
        changed = true;
      }
    }
    if !changed {
      break;
    }
  }
  in_known
}

/// Converge the per-block DF entry clearness (the DF must-analysis; see
/// `IRBuilder::build`). Returns the map consumed by both the retained build
/// and `analyze_carry`'s probes, so all three agree on each string op's
/// direction guard.
fn analyze_df<'a>(
  cfg: &'a Config,
  instrs: &'a [instr::Instr],
  spec: &'a spec::Spec<'a>,
  binary: &'a binary::Binary,
  overlay: bool,
  pin_all: bool,
) -> HashMap<BlockRef, bool> {
  // Pass A: the entry is clear (contract), every other block starts
  // not-clear. Its summaries and predecessor lists drive the fixpoint.
  let mut probe_a = IRBuilder::new(cfg, instrs, spec, binary, overlay, pin_all);
  probe_a.df_entry_default = false;
  probe_a.build_once();
  let exit_a = probe_a.df_exit_clear.clone();
  let blocks: Vec<BlockRef> = probe_a.ir.iter_blocks().collect();
  let preds: HashMap<BlockRef, Vec<BlockRef>> = blocks
    .iter()
    .map(|b| (*b, probe_a.ir.block(*b).preds.clone()))
    .collect();

  // Pass B: assume every block entry is clear, to tell an identity transfer
  // (exit follows the entry) from a constant one. DF transfer is independent
  // of carry, so this probe needs no carry seed.
  let mut probe_b = IRBuilder::new(cfg, instrs, spec, binary, overlay, pin_all);
  probe_b.df_entry_default = true;
  probe_b.build_once();
  let exit_b = probe_b.df_exit_clear.clone();

  // Must-analysis over the per-block summaries (greatest fixpoint): a block
  // entry is clear only if every predecessor's exit is, but an identity
  // transfer forwards the block's own entry, so a loop back-edge that
  // preserves DF is still clear. Start from clear and shrink; a least
  // fixpoint (as for carry) would lose that loop-carried case.
  let mut entry_clear: HashMap<BlockRef, bool> = blocks
    .iter()
    .map(|b| (*b, *b != BlockRef(0) && !preds[b].is_empty()))
    .collect();
  loop {
    let mut changed = false;
    for b in &blocks {
      let clear = if *b == BlockRef(0) {
        true // an annotated function entry assumes DF=0
      } else {
        let ps = &preds[b];
        !ps.is_empty() && ps.iter().all(|p| {
          let base = exit_a.get(p).copied().unwrap_or(false);
          if exit_b.get(p).copied().unwrap_or(false) == base {
            base // constant transfer
          } else {
            entry_clear.get(p).copied().unwrap_or(false) // identity transfer
          }
        })
      };
      if entry_clear[b] != clear {
        entry_clear.insert(*b, clear);
        changed = true;
      }
    }
    if !changed {
      break;
    }
  }
  entry_clear
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
  bld.emit_warnings();
  bld.ir
}

/// Build the IR without printing diagnostics, returning them for an aggregate
/// report. Callers that want the historical stderr behavior use
/// `build_from_instrs` instead.
pub fn build_from_instrs_with_diagnostics(
  instrs: &[instr::Instr], cfg: &Config, spec: &spec::Spec<'_>, binary: &binary::Binary,
  overlay: bool, pin_all: bool,
) -> (IR, Vec<Diagnostic>, FlagMarkers) {
  let mut bld = IRBuilder::new(cfg, instrs, spec, binary, overlay, pin_all);
  bld.build();
  (bld.ir, bld.warnings, bld.flag_markers)
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
    // Full write splits into genuine 8-bit fragments.
    let full = b.ir.const_new(0x1234);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AX), full);
    let al = b.ir.get_var(instr::Reg::AL, cur);
    let ah = b.ir.get_var(instr::Reg::AH, cur);
    let al_instr = b.ir.instr(al).expect("AL must be a masked byte fragment");
    let ah_instr = b.ir.instr(ah).expect("AH must be a masked byte fragment");
    assert_eq!(al_instr.opcode, Opcode::And, "AL must be And(full, 0xFF)");
    assert_eq!(ah_instr.opcode, Opcode::And, "AH must be And(UShr(full, 8), 0xFF)");
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

  fn eval_const_u16(ir: &crate::decompile::ir::IR, r: Ref) -> Option<u16> {
    match r {
      Ref::Const(_) => ir.const_lookup(r).map(|v| v as u16),
      Ref::Instr(_, _) => {
        let i = ir.instr(r)?;
        match i.opcode {
          Opcode::Ref if i.operands.len() == 1 => eval_const_u16(ir, i.operands[0]),
          Opcode::And | Opcode::Or | Opcode::Shl | Opcode::UShr | Opcode::Shr
            if i.operands.len() == 2 =>
          {
            let a = eval_const_u16(ir, i.operands[0])? as i32;
            let b = eval_const_u16(ir, i.operands[1])? as i32;
            let v = match i.opcode {
              Opcode::And => a & b,
              Opcode::Or => a | b,
              Opcode::Shl => (a << b) & 0xFFFF,
              Opcode::UShr => ((a as u16 as u32) >> (b as u32)) as i32,
              Opcode::Shr => ((a as i16 as i32) >> (b as u32)) & 0xFFFF,
              _ => unreachable!(),
            };
            Some((v & 0xFFFF) as u16)
          }
          _ => None,
        }
      }
      _ => None,
    }
  }

  #[test]
  fn full_write_then_ah_preserves_low_byte() {
    // Issue #77 case 1: `mov ax,0x0607; mov ah,0x4A` must give 0x4A07,
    // not 0x4E07 (high byte of the preserved low fragment leaking into AH).
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    let full = b.ir.const_new(0x0607);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AX), full);
    let hi_new = b.ir.const_new(0x4A);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AH), hi_new);
    let ax = b.ir.get_var(instr::Reg::AX, cur);
    let v = eval_const_u16(&b.ir, ax).expect("AX must fold to a constant for constant inputs");
    assert_eq!(v, 0x4A07, "mov ah must replace the high byte, preserving only the low byte");
  }

  #[test]
  fn full_write_then_al_preserves_high_byte() {
    // Issue #77 case 2: `mov ax,0x1234; mov al,0x56` must give 0x1256,
    // not 0x0056 (preserved AH lost because Upper16(0x1234) == 0).
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    let full = b.ir.const_new(0x1234);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AX), full);
    let lo_new = b.ir.const_new(0x56);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AL), lo_new);
    let ax = b.ir.get_var(instr::Reg::AX, cur);
    let v = eval_const_u16(&b.ir, ax).expect("AX must fold to a constant for constant inputs");
    assert_eq!(v, 0x1256, "mov al must replace the low byte, preserving the high byte");
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
    // Fragments are genuine 8-bit values (And-masked), not Lower16/Upper16.
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
    assert_eq!(al_instr.opcode, Opcode::And);
    assert!(al_instr.operands.contains(&ret));
    let ah = b.ir.get_var(instr::Reg::AH, cur);
    let ah_instr = b.ir.instr(ah).expect("AH must be refreshed post-call, not stale");
    assert_eq!(ah_instr.opcode, Opcode::And);
    // AH goes through UShr(ret, 8) before the mask.
    let shr = ah_instr.operands.iter().find_map(|r| b.ir.instr(*r)).expect("AH mask input");
    assert_eq!(shr.opcode, Opcode::UShr);
    assert!(shr.operands.contains(&ret));
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

  #[test]
  fn int_materializes_copies_before_source_overwrite() {
    // Issue #78: `mov bx,ax` records BX as Init(AX), a live read of the
    // mutable guest variable. A later `mov ah,6` overwrites AX, so the
    // batched INT writes must emit BX first: `BX = AX(entry)` before
    // `AX = recompose`, never the reverse.
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(Some(SegOff { seg: Seg::Normal(0), off: Off(0) }), Some(SegOff { seg: Seg::Normal(0), off: Off(1) }));
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cur = b.cur;
    // mov bx,ax while AX still holds the entry value.
    let ax_entry = b.ir.get_var(instr::Reg::AX, cur);
    assert_eq!(ax_entry, Ref::Init(instr::Reg::AX));
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::BX), ax_entry);
    // mov ah,6 recomposes AX.
    let six = b.ir.const_new(6);
    b.append_asm_dst_reg(&instr::OperandReg(instr::Reg::AH), six);
    b.append_asm_instr(&int_instr(0x21));
    // Block order of the guest writes: BX must precede AX.
    let order: Vec<String> = b.ir.iter_instrs(cur).filter_map(|r| {
      let i = b.ir.instr(r)?;
      if i.opcode != Opcode::WriteVar16 { return None; }
      if let Ref::Symbol(s) = i.operands[0] {
        Some(s.name(&b.ir.symbols).to_string())
      } else { None }
    }).collect();
    let bx_pos = order.iter().position(|n| n == "BX").expect("BX must be materialized");
    let ax_pos = order.iter().position(|n| n == "AX").expect("AX must be materialized");
    assert!(bx_pos < ax_pos, "copy BX = AX(entry) must precede the AX overwrite: {order:?}");
    // And the BX write must carry the entry value, not the recomposed AX.
    let bx_write = b.ir.iter_instrs(cur).find(|r| {
      let i = b.ir.instr(*r).unwrap();
      i.opcode == Opcode::WriteVar16
        && matches!(i.operands[0], Ref::Symbol(s) if s.name(&b.ir.symbols) == "BX")
    }).unwrap();
    assert_eq!(
      b.ir.instr(bx_write).unwrap().operands[1],
      Ref::Init(instr::Reg::AX),
      "BX must capture the entry AX"
    );
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
  fn target_in_range_accepts_region_union_and_rejects_gaps() {
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let so = |off| SegOff { seg: Seg::Normal(0), off: Off(off) };
    let spec = spec::Spec {
      func: None,
      name: "split".to_string(),
      start: so(0x10),
      end: so(0x28),
      regions: vec![
        config::FuncRegion { start: so(0x10), end: so(0x18) },
        config::FuncRegion { start: so(0x20), end: so(0x28) },
      ],
    };
    let b = test_builder(&cfg, &[], &spec, &binary);
    assert!(b.target_in_range(so(0x10)), "first region start is inclusive");
    assert!(b.target_in_range(so(0x17)), "first region end is exclusive");
    assert!(!b.target_in_range(so(0x18)));
    assert!(!b.target_in_range(so(0x1f)), "gap is outside every region");
    assert!(b.target_in_range(so(0x20)), "second region start is inclusive");
    assert!(b.target_in_range(so(0x27)));
    assert!(!b.target_in_range(so(0x28)), "second region end is exclusive");
    assert!(!b.target_in_range(SegOff { seg: Seg::Normal(1), off: Off(0x12) }));
  }

  #[test]
  fn region_boundary_does_not_emit_a_fallthrough_jump() {
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let so = |off| SegOff { seg: Seg::Normal(0), off: Off(off) };
    let spec = spec::Spec {
      func: None,
      name: "split".to_string(),
      start: so(0x10),
      end: so(0x21),
      regions: vec![
        config::FuncRegion { start: so(0x10), end: so(0x11) },
        config::FuncRegion { start: so(0x20), end: so(0x21) },
      ],
    };
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x10, 1, vec![
        instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)),
        instr::Operand::Reg(instr::OperandReg(instr::Reg::BX)),
      ]),
      located_instr(instr::Opcode::OP_MOV, 0x20, 1, vec![
        instr::Operand::Reg(instr::OperandReg(instr::Reg::CX)),
        instr::Operand::Reg(instr::OperandReg(instr::Reg::DX)),
      ]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    assert!(b.ir.block_exits(BlockRef(0)).is_empty(),
      "region end must not link to the next region");
    let second = *b.addrmap.get(&so(0x20)).expect("second region gets a block");
    assert!(b.ir.block_instr_count(second) > 0, "second region decoded into its own block");
    assert!(b.ir.block(second).preds.is_empty(), "no false fallthrough predecessor");
  }

  #[test]
  fn prestart_chunk_falls_through_to_the_entry() {
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0x90; 6], None);
    let so = |off| SegOff { seg: Seg::Normal(0), off: Off(off) };
    let spec = spec::Spec {
      func: None,
      name: "split".to_string(),
      start: so(3),
      end: so(6),
      regions: vec![config::FuncRegion { start: so(0), end: so(6) }],
    };
    // The entry is `jmp 0`; the pre-start chunk is `mov ax,1`, which must
    // fall through to the entry at 0x0003.
    let instrs = [
      jmp_instr(instr::Opcode::OP_JMP, 3, 3, 0xFFFA),
      located_instr(instr::Opcode::OP_MOV, 0, 3, vec![
        instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)),
        instr::Operand::Imm(instr::OperandImm { sz: instr::Size::Size16, val: 1 }),
      ]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    let prestart = *b.addrmap.get(&so(0)).expect("pre-start block exists");
    assert_eq!(b.ir.block_exits(prestart), vec![BlockRef(0)],
      "the pre-start chunk must link to the entry block");
    assert_eq!(b.ir.block(BlockRef(0)).name, "entry");
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
  fn of_pf_jumps_lower_to_non_fatal_flag_readers() {
    // #108: JO/JNO/JP/JNP must read OF/PF through the non-fatal reader
    // opcodes instead of falling to the fatal Unimpl fallback. The direct
    // forms use the reader as the branch condition; the negated forms compose
    // Eq(reader, 0), matching JNS and the SET* forms.
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x0004, &[0xC3]);
    for (opcode, expected_reader, negated) in [
      (instr::Opcode::OP_JO, Opcode::OverflowFlags, false),
      (instr::Opcode::OP_JNO, Opcode::OverflowFlags, true),
      (instr::Opcode::OP_JP, Opcode::ParityFlags, false),
      (instr::Opcode::OP_JNP, Opcode::ParityFlags, true),
    ] {
      let instrs = [jmp_instr(opcode, 0x0000, 2, 1)];
      let mut b = test_builder(&cfg, &instrs, &spec, &binary);
      b.build();

      let mut jne_ref = None;
      for blk in b.ir.iter_blocks() {
        for r in b.ir.iter_instrs(blk) {
          assert_ne!(b.ir.instr(r).unwrap().opcode, Opcode::Unimpl, "{opcode:?} must not emit a fatal Unimpl marker");
          if b.ir.instr(r).unwrap().opcode == Opcode::Jne {
            jne_ref = Some(r);
          }
        }
      }
      let jne = b.ir.instr(jne_ref.expect("jump must lower to a Jne")).unwrap();
      let cond_ref = jne.operands[0];
      let reader_ref = if negated {
        let cond = b.ir.instr(cond_ref).unwrap();
        assert_eq!(cond.opcode, Opcode::Eq, "{opcode:?} negated form must compose Eq(reader, 0)");
        cond.operands[0]
      } else {
        cond_ref
      };
      let reader = b.ir.instr(reader_ref).unwrap();
      assert_eq!(reader.opcode, expected_reader, "{opcode:?} must read its flag non-fatally");

      // The true block is the jump target (offset 3); the false block is the
      // fallthrough (offset 2). A true/false swap would keep the condition
      // shape but invert the control flow, so pin the wiring too.
      let true_blk = jne.operands[1].unwrap_block();
      let false_blk = jne.operands[2].unwrap_block();
      assert_eq!(b.ir.block(true_blk).name, "addr_0003", "{opcode:?} true edge must be the jump target");
      assert_eq!(b.ir.block(false_blk).name, "addr_0002", "{opcode:?} false edge must be the fallthrough");
    }
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
    // The carry analysis runs the build speculatively; each diagnostic must
    // still be reported exactly once, not once per pass.
    assert_eq!(b.warnings.len(), 1, "the out-of-range WARN must be emitted once: {:?}", b.warnings);
    assert!(b.warnings[0].contains("outside function range"));
    assert_eq!(b.warnings[0].cause, Cause::BranchOutOfRange);
    assert_eq!(b.warnings[0].addr, Some(SegOff { seg: Seg::Normal(0), off: Off(0x0006) }));
    assert_eq!(b.warnings[0].opcode, Some(instr::Opcode::OP_JMP));
    let tgt = SegOff { seg: Seg::Normal(0), off: Off(0x000a) };
    let blk = *b.addrmap.get(&tgt).expect("out-of-range target still gets a block");
    assert_eq!(b.ir.block_instr_count(blk), 0, "target block must stay empty (drop semantics preserved)");
    assert!(b.ir.block_exits(blk).is_empty());
  }

  #[test]
  fn warnings_are_not_duplicated_across_analysis_passes() {
    // A carry consumer (the ADC) forces the speculative multi-pass build, so
    // each diagnostic must still be reported once, not once per pass.
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x0006, &[0xC3]);
    let instrs = [
      alu_instr(instr::Opcode::OP_ADC, instr::Reg::AX, instr::Reg::BX),
      // JMP at 0x0002, 3 bytes: end 0x0005 + rel 1 -> target 0x0006 (out of range).
      jmp_instr(instr::Opcode::OP_JMP, 0x0002, 3, 1),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    assert_eq!(
      b.warnings.iter().filter(|w| w.contains("outside function range")).count(), 1,
      "the out-of-range WARN must be emitted once across analysis passes: {:?}", b.warnings);
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
    // The full-width AX write also splits into genuine 8-bit fragments
    // (two Ands with 0xFF); only the rotate masks use 15.
    let rotate_ands: Vec<Ref> = ands.iter().copied()
      .filter(|r| b.ir.const_lookup(b.ir.instr(*r).unwrap().operands[1]) == Some(15)
        || b.ir.const_lookup(b.ir.instr(*r).unwrap().operands[0]) == Some(15))
      .collect();
    assert_eq!(rotate_ands.len(), 2, "count mask plus complementary-shift mask");
    let n = *rotate_ands.iter().find(|r| {
      let i = b.ir.instr(**r).unwrap();
      b.ir.const_lookup(i.operands[0]) == Some(1) && b.ir.const_lookup(i.operands[1]) == Some(15)
    }).expect("count mask And [1, 15] present");
    let m = *rotate_ands.iter().find(|r| **r != n).expect("second And present");
    let m_instr = b.ir.instr(m).unwrap();
    let sub = b.ir.instr(m_instr.operands[0]).unwrap();
    assert_eq!(sub.opcode, Opcode::Sub);
    assert_eq!(b.ir.const_lookup(sub.operands[0]), Some(16));
    assert_eq!(sub.operands[1], n, "complementary shift derives from the masked count");
    assert_eq!(b.ir.const_lookup(m_instr.operands[1]), Some(15), "complementary shift is masked too");
    assert_eq!(shl.operands[1], n);
    assert_eq!(ushr.operands[1], m);
    // Destination and flags. The flags are clobbered to the unknown initial
    // value (not an Unimpl marker), so a later flag branch degrades to the
    // non-fatal UNIMPL_FLAGS rather than a fatal UNIMPL abort.
    assert_eq!(b.ir.get_var(instr::Reg::AX, cur), or, "ROL writes the Or back to AX");
    assert_eq!(b.get_flags(), Ref::Init(instr::Reg::FLAGS), "ROT clobbers flags to unknown");
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
  fn cld_after_std_restores_exact_string_model() {
    // Issue #86: DF is tracked per point within a block, so a `cld`
    // re-establishes the provably-clear direction and the following
    // string op is modeled exactly even when a STD exists elsewhere.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.df_clear = false;
    b.append_asm_instr(&string_instr(instr::Opcode::OP_CLD, vec![]));
    let ins = string_instr(instr::Opcode::OP_MOVS, vec![
      string_mem(instr::Size::Size8, instr::Reg::ES, instr::Reg::DI),
      string_mem(instr::Size::Size8, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);
    assert_advanced(&mut b, instr::Reg::SI, 1);
    assert_advanced(&mut b, instr::Reg::DI, 1);
    assert!(
      b.ir.iter_instrs(b.cur).all(|r| b.ir.instr(r).unwrap().opcode != Opcode::Unimpl),
      "no loud marker once CLD re-establishes DF=0"
    );
  }

  #[test]
  fn std_poison_string_ops_until_cld() {
    // The other half of #86: after a STD with no intervening CLD, the
    // direction is genuinely unknown and the marker must stay.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&string_instr(instr::Opcode::OP_STD, vec![]));
    assert!(!b.df_clear, "STD must clear the provably-clear direction");
    let ins = string_instr(instr::Opcode::OP_MOVS, vec![
      string_mem(instr::Size::Size8, instr::Reg::ES, instr::Reg::DI),
      string_mem(instr::Size::Size8, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);
    let _ = only_instr(&b.ir, b.cur, Opcode::Unimpl);
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
    b.df_clear = false;
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
  fn build_detects_df_writer_from_decoded_instructions() {
    // `std; ret` decoded: build() must treat the standard as a DF writer, so
    // later block entries fall back to not-clear (which keeps their string ops
    // loud). Without std there is no writer and the fallback stays clear.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xFD, 0xC3]);
    let mut ins = string_instr(instr::Opcode::OP_STD, vec![]);
    ins.n_bytes = 1;
    let ret = string_instr(instr::Opcode::OP_RET, vec![]);
    let instrs = [ins, ret];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    assert!(!b.df_entry_default, "build must detect the DF writer (STD)");
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

  fn alu_instr(opcode: instr::Opcode, dst: instr::Reg, src: instr::Reg) -> instr::Instr {
    let mut operands = ArrayVec::new();
    operands.push(instr::Operand::Reg(instr::OperandReg(dst)));
    operands.push(instr::Operand::Reg(instr::OperandReg(src)));
    instr::Instr {
      rep: None,
      opcode,
      operands,
      addr: SegOff { seg: Seg::Normal(0), off: Off(0) },
      n_bytes: 2,
      intel_hidden_operand_bitmask: 0,
    }
  }

  #[test]
  fn alu_carry_tracking_sets_uses_preserves_and_clears() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);

    // add ax,bx sets a known carry.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AX, instr::Reg::BX));
    assert!(b.last_cf.is_some(), "word add must set the carry");
    let cf1 = b.last_cf.unwrap();

    // adc cx,dx consumes it and re-derives a carry.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADC, instr::Reg::CX, instr::Reg::DX));
    assert!(b.last_cf.is_some(), "adc must re-derive the carry");
    let cf2 = b.last_cf.unwrap();
    assert_ne!(cf2, cf1);

    // mov does not touch flags: the carry is preserved.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_MOV, instr::Reg::AX, instr::Reg::BX));
    assert_eq!(b.last_cf, Some(cf2), "mov must preserve the tracked carry");

    // and clears CF to 0.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_AND, instr::Reg::AX, instr::Reg::BX));
    assert_eq!(b.ir.const_lookup(b.last_cf.unwrap()), Some(0), "and clears CF");

    // a shift leaves the carry unknown.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_SHL, instr::Reg::AX, instr::Reg::CX));
    assert!(b.last_cf.is_none(), "shift leaves the carry unknown");
  }

  #[test]
  fn shift_imm_publishes_carry_out_for_adc() {
    // Issue #86 (related note): `shl ax,1` carries bit 15, so a following
    // ADC must lower exactly instead of degrading to UNIMPL_FLAGS.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&rotate_instr(instr::Opcode::OP_SHL, instr::Reg::AX, imm_count(1)));
    let cf = b.last_cf.expect("shl ax,1 must track bit 15 as the carry");
    let and = b.ir.instr(cf).unwrap();
    assert_eq!(and.opcode, Opcode::And, "carry-out must be masked to one bit");
    assert_eq!(b.ir.const_lookup(and.operands[1]), Some(1));
    let shr = b.ir.instr(and.operands[0]).unwrap();
    assert_eq!(shr.opcode, Opcode::UShr, "carry-out must shift the source down");
    assert_eq!(b.ir.const_lookup(shr.operands[1]), Some(15), "shl by 1 carries bit 15");
    // A following ADC consumes the tracked carry exactly.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADC, instr::Reg::BX, instr::Reg::CX));
    assert!(b.last_cf.is_some(), "adc after shl must lower exactly with the shift carry");
  }

  #[test]
  fn shift_imm_carry_positions_cover_widths_and_directions() {
    // SHR/SAR carry bit (n-1); byte SHL carries bit (8-n).
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&rotate_instr(instr::Opcode::OP_SHR, instr::Reg::AX, imm_count(1)));
    let cf = b.last_cf.expect("shr ax,1 must track bit 0");
    let and = b.ir.instr(cf).unwrap();
    assert_eq!(and.opcode, Opcode::And);
    let shr = b.ir.instr(and.operands[0]).unwrap();
    assert_eq!(shr.opcode, Opcode::UShr);
    assert_eq!(b.ir.const_lookup(shr.operands[1]), Some(0), "shr by 1 carries bit 0");
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&rotate_instr(instr::Opcode::OP_SHL, instr::Reg::AL, imm_count(2)));
    let cf = b.last_cf.expect("shl al,2 must track bit 6");
    let and = b.ir.instr(cf).unwrap();
    let shr = b.ir.instr(and.operands[0]).unwrap();
    assert_eq!(b.ir.const_lookup(shr.operands[1]), Some(6), "byte shl by 2 carries bit 6");
  }

  #[test]
  fn shift_overwide_and_variable_counts_stay_loud() {
    // Counts above w have no single defined bit, and CL counts are runtime
    // values: both keep the clobber. (Zero counts preserve flags — see the
    // next test — so they are not in this list.)
    let cl = instr::Operand::Reg(instr::OperandReg(instr::Reg::CL));
    for (opcode, count) in [
      (instr::Opcode::OP_SHL, imm_count(20)),
      (instr::Opcode::OP_SHL, cl),
      (instr::Opcode::OP_SHR, imm_count(20)),
    ] {
      let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
      let instrs: &[instr::Instr] = &[];
      let mut b = test_builder(&cfg, instrs, &spec, &binary);
      b.append_asm_instr(&rotate_instr(opcode, instr::Reg::AX, count));
      assert!(b.last_cf.is_none(), "{opcode:?} with this count must leave the carry unknown");
    }
  }

  #[test]
  fn shift_zero_count_preserves_flags_and_carry() {
    // HW leaves every flag unchanged on a zero-count shift, so a tracked
    // carry (here from STC) must survive untouched, with no flag update.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&string_instr(instr::Opcode::OP_STC, vec![]));
    let cf_before = b.last_cf.expect("stc must track CF=1");
    assert_eq!(b.ir.const_lookup(cf_before), Some(1));
    let flags_before = b.get_flags();
    let updates_before = b.ir.iter_instrs(b.cur)
      .filter(|r| b.ir.instr(*r).unwrap().opcode == Opcode::UpdateFlags)
      .count();
    b.append_asm_instr(&rotate_instr(instr::Opcode::OP_SHL, instr::Reg::AX, imm_count(0)));
    assert_eq!(b.last_cf, Some(cf_before), "shl ax,0 must preserve the tracked carry");
    assert_eq!(b.get_flags(), flags_before, "shl ax,0 must emit no flag update");
    let updates_after = b.ir.iter_instrs(b.cur)
      .filter(|r| b.ir.instr(*r).unwrap().opcode == Opcode::UpdateFlags)
      .count();
    assert_eq!(updates_after, updates_before, "shl ax,0 must emit no flag update");
  }

  #[test]
  fn byte_add_tracks_carry_via_widened_sum() {
    // The byte result cannot express the 8-bit carry, so the carry is read
    // off a widened (u16) sum: `0xFF < (a + b)` (the operands are
    // C-integer-promoted, so the sum is the full value, not the byte).
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AL, instr::Reg::BL));
    let cf = b.last_cf.expect("byte add must track a carry");
    let cmp = b.ir.instr(cf).unwrap();
    assert_eq!(cmp.opcode, Opcode::ULt);
    assert_eq!(b.ir.const_lookup(cmp.operands[0]), Some(0xFF), "carry is `0xFF < sum`");
    let sum = b.ir.instr(cmp.operands[1]).unwrap();
    assert_eq!(sum.opcode, Opcode::Add);
    assert!(matches!(sum.typ, Type::U16), "the summed operands must be widened to u16");
  }

  #[test]
  fn byte_adc_tracks_bit8_carry() {
    // A byte ADC's carry-out is bit 8 (`0xFF < a + b + cf`), NOT the bit-16
    // `(t < a) | (vref < t)` form used for word operands: with byte inputs
    // that form is always 0, silently dropping the carry into a following
    // ADC/SBB in a carry chain.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    // Seed a known carry so the ADC takes the exact path, then consume it.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AL, instr::Reg::BL));
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADC, instr::Reg::AL, instr::Reg::CL));
    let cf = b.last_cf.expect("byte adc must track a carry");
    let cmp = b.ir.instr(cf).unwrap();
    assert_eq!(cmp.opcode, Opcode::ULt, "byte ADC carry-out must be `0xFF < sum`, not an Or");
    assert_eq!(b.ir.const_lookup(cmp.operands[0]), Some(0xFF), "carry is `0xFF < sum`");
  }

  #[test]
  fn byte_add_carry_masks_wide_fragment() {
    // Byte fragments are 8-bit masked at the split, but the byte ADD carry
    // still masks defensively via `to_byte`: the carry widens `a + b` and
    // compares against 0xFF, so an unmasked operand would report a carry
    // whenever the high bits are set.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_MOV, instr::Reg::AX, instr::Reg::BX));
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AL, instr::Reg::BL));
    let cf = b.last_cf.expect("byte add must track a carry");
    let cmp = b.ir.instr(cf).unwrap();
    assert_eq!(cmp.opcode, Opcode::ULt);
    let wide = b.ir.instr(cmp.operands[1]).expect("widened sum");
    assert_eq!(wide.opcode, Opcode::Add);
    // The `a` operand is the wide AL fragment; it must be masked to 8 bits.
    let am = b.ir.instr(wide.operands[0]).expect("byte operand must be masked");
    assert_eq!(am.opcode, Opcode::And, "wide byte fragment must be masked with 0xFF");
    assert_eq!(b.ir.const_lookup(am.operands[1]), Some(0xFF));
  }

  #[test]
  fn block_boundary_seeds_carry_from_entry_knownness() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AX, instr::Reg::BX));
    assert!(b.last_cf.is_some());
    let next = b.ir.add_block("next");
    // Without a proven entry carry the new block starts unknown.
    b.switch_blk(next);
    assert!(!b.carry_known, "unknown entry carry must not be inherited");
    assert!(b.last_cf.is_none());

    // When the analysis proves the entry carry, the block inherits it (the
    // value itself is resolved lazily from the CF SSA on first use).
    let next2 = b.ir.add_block("next2");
    b.carry_known_entry.insert(next2, true);
    b.switch_blk(next2);
    assert!(b.carry_known, "a proven entry carry must be inherited");
  }

  #[test]
  fn block_boundary_seeds_df_from_entry_clearness() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    // A DF writer exists, so the probe/default fallback is not-clear.
    b.df_entry_default = false;
    b.df_clear = true;
    let next = b.ir.add_block("next");
    b.switch_blk(next);
    assert!(!b.df_clear, "unknown entry DF must not be inherited");

    // When the analysis proves the entry clear, the block inherits it.
    let next2 = b.ir.add_block("next2");
    b.df_clear_entry.insert(next2, true);
    b.switch_blk(next2);
    assert!(b.df_clear, "a proven-clear entry DF must be inherited");
  }

  #[test]
  fn producer_publishes_carry_as_ssa() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AX, instr::Reg::BX));
    let cf = b.last_cf.expect("add must track a carry");
    assert_eq!(b.ir.get_var(carry_var(), b.cur), cf, "carry must be published as CF SSA");
  }

  fn mul_instr() -> instr::Instr {
    // `mul bx` = DX:AX = AX * BX.
    let mut operands = ArrayVec::new();
    operands.push(instr::Operand::Reg(instr::OperandReg(instr::Reg::DX)));
    operands.push(instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)));
    operands.push(instr::Operand::Reg(instr::OperandReg(instr::Reg::BX)));
    instr::Instr {
      rep: None,
      opcode: instr::Opcode::OP_MUL,
      operands,
      addr: SegOff { seg: Seg::Normal(0), off: Off(0) },
      n_bytes: 2,
      intel_hidden_operand_bitmask: 0,
    }
  }

  #[test]
  fn carry_writers_without_a_modeled_carry_invalidate_it() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);

    // NEG writes CF: a following ADC/SBB must not reuse the pre-NEG carry.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AX, instr::Reg::BX));
    assert!(b.last_cf.is_some());
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_NEG, instr::Reg::AX, instr::Reg::AX));
    assert!(b.last_cf.is_none(), "neg writes CF");

    // MUL writes CF: same.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AX, instr::Reg::BX));
    assert!(b.last_cf.is_some());
    b.append_asm_instr(&mul_instr());
    assert!(b.last_cf.is_none(), "mul writes CF");

    // NOT does not touch flags: the carry is preserved.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_ADD, instr::Reg::AX, instr::Reg::BX));
    let cf = b.last_cf.unwrap();
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_NOT, instr::Reg::AX, instr::Reg::AX));
    assert_eq!(b.last_cf, Some(cf), "not leaves flags intact");

    // INC does not touch CF either: preserved.
    b.append_asm_instr(&alu_instr(instr::Opcode::OP_INC, instr::Reg::AX, instr::Reg::AX));
    assert_eq!(b.last_cf, Some(cf), "inc preserves CF");
  }

  #[test]
  fn may_clobber_memory_covers_calls_int_and_rep() {
    for op in [Opcode::CallFar, Opcode::CallNear, Opcode::CallNearPtr, Opcode::CallFarPtr,
               Opcode::CallArgs, Opcode::Int, Opcode::RepMovs, Opcode::RepStos,
               Opcode::Ins, Opcode::RepIns] {
      assert!(op.may_clobber_memory(), "{op:?} must be a memory barrier");
    }
    for op in [Opcode::Load8, Opcode::Store8, Opcode::Add, Opcode::RetNear, Opcode::Unimpl,
               Opcode::Outs, Opcode::RepOuts] {
      assert!(!op.may_clobber_memory(), "{op:?} must not be a memory barrier");
    }
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
    b.df_clear = false;
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
  fn rep_lods_is_modeled() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let instrs: &[instr::Instr] = &[];
    let mut b = test_builder(&cfg, instrs, &spec, &binary);
    let cx = b.ir.const_new(3);
    b.ir.set_var(instr::Reg::CX, b.cur, cx);
    let ins = rep_string_instr(instr::Opcode::OP_LODS, vec![
      instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)),
      string_mem(instr::Size::Size16, instr::Reg::DS, instr::Reg::SI),
    ]);
    b.append_asm_instr(&ins);

    let _ = only_instr(&b.ir, b.cur, Opcode::RepLods);
    let cx_after = b.ir.get_var(instr::Reg::CX, b.cur);
    assert_eq!(b.ir.const_lookup(cx_after), Some(0), "CX consumed to 0");
    let _ = assert_rep_advanced(&mut b, instr::Reg::SI, 2);
  }

  /// Decode `code` into instructions for a builder over the same range.
  fn decode_all(binary: &binary::Binary, spec: &spec::Spec) -> Vec<instr::Instr> {
    use crate::asm::decode::Decoder;
    let mut decoder = Decoder::new(binary.region_iter(spec.start, spec.end));
    let mut instrs = vec![];
    while let Some((ins, _raw)) = decoder.try_next().expect("decode") {
      instrs.push(ins);
    }
    instrs
  }

  #[test]
  fn same_segment_far_jump_lowers_to_a_jump() {
    // `jmp far 0000:0009` does not change CS, so it is a plain jump to
    // that address: the IR must carry a Jmp to the target block, with no
    // imprecision marker (issue #112).
    let code: &[u8] = &[0xEA, 0x09, 0x00, 0x00, 0x00, 0xB8, 0x01, 0x00, 0xC3, 0xB8, 0x02, 0x00, 0xC3];
    let (_types, cfg, binary, spec) = bit_test_builder(code);
    let instrs = decode_all(&binary, &spec);
    assert_eq!(instrs[0].opcode, instr::Opcode::OP_JMPF);

    let ir = build_from_instrs(&instrs, &cfg, &spec, &binary, false, false);
    let entry = ir.iter_blocks().next().expect("entry block");
    let last = ir.block_last(entry);
    assert_eq!(ir.instr(last).unwrap().opcode, Opcode::Jmp, "a same-segment far jump is a plain jump");
    let target = ir.instr(last).unwrap().operands[0].unwrap_block();
    assert_eq!(ir.block(target).name, "addr_0009", "the jump must target the far offset");
    assert!(ir.iter_instrs(entry).all(|r| ir.instr(r).unwrap().opcode != Opcode::Unimpl),
      "a modellable transfer must not be marked imprecise");
  }

  #[test]
  fn cross_segment_far_jump_ends_the_block_without_a_fallthrough() {
    // A far jump into another segment cannot be expressed in the IR: the
    // block ends with a loud marker and a jump to an empty block (the
    // "control leaves the function" idiom), so the code after it is not
    // reachable through a fallthrough edge (issue #112).
    let code: &[u8] = &[0xEA, 0x00, 0x01, 0x34, 0x12, 0xB8, 0x01, 0x00, 0xC3];
    let (_types, cfg, binary, spec) = bit_test_builder(code);
    let instrs = decode_all(&binary, &spec);
    assert_eq!(instrs[0].opcode, instr::Opcode::OP_JMPF);

    let ir = build_from_instrs(&instrs, &cfg, &spec, &binary, false, false);
    let entry = ir.iter_blocks().next().expect("entry block");
    let _ = only_instr(&ir, entry, Opcode::Unimpl);
    let last = ir.block_last(entry);
    assert_eq!(ir.instr(last).unwrap().opcode, Opcode::Jmp, "the block must end with a jump");
    let trap = ir.instr(last).unwrap().operands[0].unwrap_block();
    assert_eq!(ir.block_instr_count(trap), 0, "the transfer target must be an empty block");
    assert!(ir.block_exits(trap).is_empty(), "an empty block has no exits: control leaves the function");

    // The code after the transfer is a separate block with no
    // predecessor: it is unreachable, not a fallthrough.
    let after = ir.iter_blocks().find(|b| ir.block(*b).name == "addr_0005").expect("the fallthrough block");
    assert!(ir.block(after).preds.is_empty(), "the transfer must not create a fallthrough edge");
  }

  #[test]
  fn unmodelled_jump_warnings_carry_the_warn_prefix() {
    // The diagnostics are the product of this change: they must be
    // greppable on a batch run, like the sibling #67 warning.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xFF, 0x27, 0xC3]);
    let instrs = decode_all(&binary, &spec);
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    assert!(b.warnings.iter().any(|w| w.cause == Cause::IndirectJumpRuntimeTarget
                                     && w.contains("target is only known at run time")),
      "an indirect jump must report a run-time-only target: {:?}", b.warnings);
    assert!(b.warnings.iter().all(|w| w.starts_with("WARN: ")),
      "every diagnostic must carry the WARN prefix: {:?}", b.warnings);
  }

  #[test]
  fn unimpl_warning_carries_the_source_opcode_and_address() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xF4, 0xC3]);
    let instrs = decode_all(&binary, &spec);
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    let d = b.warnings.iter().find(|d| d.cause == Cause::Unimpl)
      .unwrap_or_else(|| panic!("no UNIMPL diagnostic: {:?}", b.warnings));
    assert_eq!(d.addr, Some(SegOff { seg: Seg::Normal(0), off: Off(0) }));
    assert_eq!(d.opcode, Some(instr::Opcode::OP_HLT));
  }

  #[test]
  fn diagnostics_build_api_returns_the_retained_pass_diagnostics() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xF4, 0xC3]);
    let instrs = decode_all(&binary, &spec);
    let (_ir, diagnostics, _markers) =
      build_from_instrs_with_diagnostics(&instrs, &cfg, &spec, &binary, false, false);
    let d = diagnostics.iter().find(|d| d.cause == Cause::Unimpl)
      .unwrap_or_else(|| panic!("no UNIMPL diagnostic: {:?}", diagnostics));
    assert_eq!(d.addr, Some(SegOff { seg: Seg::Normal(0), off: Off(0) }));
    assert_eq!(d.opcode, Some(instr::Opcode::OP_HLT));
  }

  #[test]
  fn flag_marker_diagnostics_keep_the_source_opcode_and_address() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0x0F, 0x90, 0xC0, 0x50, 0xC3]);
    let instrs = decode_all(&binary, &spec);
    let (mut ir, _diagnostics, markers) =
      build_from_instrs_with_diagnostics(&instrs, &cfg, &spec, &binary, false, false);
    crate::decompile::finalize_for_emission(&mut ir);

    let diagnostics = markers.diagnostics(&ir);
    let diagnostic = diagnostics.iter().find(|d| d.cause == Cause::UnimplFlags)
      .unwrap_or_else(|| panic!("no UNIMPL_FLAGS diagnostic: {:?}", diagnostics));
    assert_eq!(diagnostic.addr, Some(SegOff { seg: Seg::Normal(0), off: Off(0) }));
    assert_eq!(diagnostic.opcode, Some(instr::Opcode::OP_SETO));
  }

  #[test]
  fn optimized_away_flag_markers_are_not_reported() {
    let (_types, cfg, binary, spec) = bit_test_builder(&[0x01, 0xD8, 0x74, 0x01, 0x90, 0xC3]);
    let instrs = decode_all(&binary, &spec);
    let (mut ir, _diagnostics, markers) =
      build_from_instrs_with_diagnostics(&instrs, &cfg, &spec, &binary, false, false);
    crate::decompile::finalize_for_emission(&mut ir);

    assert!(markers.diagnostics(&ir).is_empty(),
      "a simplified branch must not leave a stale UNIMPL_FLAGS count");
  }

  #[test]
  fn a_jump_without_a_config_table_names_the_missing_annotation() {
    // `jmp cs:[bx+0x6d7]` (2E FF A7 D7 06) is the shape of the
    // config-described jump table. Without the table the target is
    // unknown, and the diagnostic must say so: that wording is the
    // operator's cue to add the region.
    let (_types, cfg, binary, spec) = bit_test_builder(&[0x2E, 0xFF, 0xA7, 0xD7, 0x06, 0xC3]);
    let instrs = decode_all(&binary, &spec);
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    assert!(b.warnings.iter().any(|w| w.cause == Cause::IndirectJumpNoTable
                                     && w.contains("the config has no jump table for it")),
      "the config-gap diagnostic must be specific: {:?}", b.warnings);
    assert!(!b.warnings.iter().any(|w| w.contains("only known at run time")),
      "the config-gap case must not be reported as a run-time-only target: {:?}", b.warnings);
  }

  #[test]
  fn a_far_jump_in_an_overlay_is_unmodelled() {
    // In an overlay the encoded segment belongs to the original
    // program's address space, which this segment model does not
    // represent, so a far jump there cannot be resolved.
    let types = Rc::new(TypeDatabase::new());
    let cfg = Config { types: types.clone(), structs: vec![], code_segs: vec![], funcs: vec![], indirects: vec![], globals: vec![], text_section: vec![] };
    let binary = binary::Binary::from_raw(&[0xC3], None);
    let spec = spec::Spec::from_start_and_end(
      Some(SegOff { seg: Seg::Overlay(0), off: Off(0) }),
      Some(SegOff { seg: Seg::Overlay(0), off: Off(1) }));
    let b = IRBuilder::new(&cfg, &[], &spec, &binary, true, false);
    let kind = crate::asm::branch::UnresolvedTransfer::FarDirect { seg: 0, off: 0x10 };
    assert!(matches!(b.transfer_model(kind), Transfer::Unmodelled),
      "an overlay far jump must not be treated as a same-segment jump");
  }

  #[test]
  fn jump_targets_never_panics_on_a_non_relative_operand() {
    // Every branch in the table must survive an operand that is not a
    // relative offset: the old code panicked on anything but `Rel`,
    // which lost the whole function (issue #112).
    use crate::asm::branch;
    let (_types, cfg, binary, spec) = bit_test_builder(&[0xC3]);
    let b = test_builder(&cfg, &[], &spec, &binary);
    let opcodes = [
      instr::Opcode::OP_JA, instr::Opcode::OP_JAE, instr::Opcode::OP_JB, instr::Opcode::OP_JBE,
      instr::Opcode::OP_JCXZ, instr::Opcode::OP_JE, instr::Opcode::OP_JG, instr::Opcode::OP_JGE,
      instr::Opcode::OP_JL, instr::Opcode::OP_JLE, instr::Opcode::OP_JMP, instr::Opcode::OP_JMPF,
      instr::Opcode::OP_JNE, instr::Opcode::OP_JNO, instr::Opcode::OP_JNP, instr::Opcode::OP_JNS,
      instr::Opcode::OP_JO, instr::Opcode::OP_JP, instr::Opcode::OP_JS,
      instr::Opcode::OP_LOOP, instr::Opcode::OP_LOOPE, instr::Opcode::OP_LOOPNE,
    ];
    for op in opcodes {
      let mut operands = ArrayVec::new();
      operands.push(instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)));
      // LOOPcc/JCXZ carry a hidden CX operand, so their relative target
      // is operand 1.
      if branch::branch_shape(op).map(|s| s.oper_num) == Some(1) {
        operands.push(instr::Operand::Reg(instr::OperandReg(instr::Reg::AX)));
      }
      let ins = instr::Instr {
        rep: None,
        opcode: op,
        operands,
        addr: SegOff { seg: Seg::Normal(0), off: Off(0) },
        n_bytes: 2,
        intel_hidden_operand_bitmask: 0,
      };
      let targets = b.jump_targets(&ins);
      assert!(targets.is_some(), "{op:?} must be recognized as a branch");
      assert!(matches!(targets, Some(BranchTargets::Unresolved(_))),
        "{op:?} with a non-relative operand must be unresolved, not a target");
    }
  }

  fn operand(reg: instr::Reg) -> instr::Operand {
    instr::Operand::Reg(instr::OperandReg(reg))
  }

  fn imm_operand(val: u16) -> instr::Operand {
    instr::Operand::Imm(instr::OperandImm { sz: instr::Size::Size16, val })
  }

  fn rel_operand(val: u16) -> instr::Operand {
    instr::Operand::Rel(instr::OperandRel { val })
  }

  fn located_instr(opcode: instr::Opcode, addr_off: u16, n_bytes: u16,
                   operands: Vec<instr::Operand>) -> instr::Instr {
    let mut ops = ArrayVec::new();
    for op in operands { ops.push(op); }
    instr::Instr {
      rep: None,
      opcode,
      operands: ops,
      addr: SegOff { seg: Seg::Normal(0), off: Off(addr_off) },
      n_bytes,
      intel_hidden_operand_bitmask: 0,
    }
  }

  #[test]
  fn exit_contract_warns_about_an_undeclared_segment_change() {
    let code = &[
      0xB8, 0x07, 0x06, // mov ax,0x0607
      0x8E, 0xD8,       // mov ds,ax
      0xC3,             // ret
    ];
    let (_types, mut cfg, binary, _) = test_spec(0x0000, 0x0006, code);
    cfg.funcs.push(config::Func {
      name: "main".to_string(),
      start: SegOff { seg: Seg::Normal(0), off: Off(0) },
      end: Some(SegOff { seg: Seg::Normal(0), off: Off(6) }),
      entry: None,
      mode: config::CallMode::Near,
      ret: Some(Type::U16),
      args: Some(0),
      regargs: None,
      regouts: Some(vec![instr::Reg::SI]),
      clobbers: None,
      dont_pop_args: false,
      regions: vec![],
    });
    let spec = spec::Spec::from_config_name(&cfg, "main").expect("main");
    let instrs = decode_all(&binary, &spec);
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    let contract_warnings: Vec<_> = b.warnings.iter()
      .filter(|w| w.contains("regouts/clobbers"))
      .collect();
    assert_eq!(contract_warnings.len(), 1,
      "the undeclared DS change must warn exactly once: {:?}", b.warnings);
    assert!(contract_warnings[0].contains("DS"),
      "the warning must name the changed register: {:?}", b.warnings);
  }

  #[test]
  fn exit_contract_does_not_warn_when_none_are_declared() {
    let code = &[
      0xB8, 0x07, 0x06, // mov ax,0x0607
      0x8E, 0xD8,       // mov ds,ax
      0xC3,             // ret
    ];
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x0006, code);
    let instrs = decode_all(&binary, &spec);
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();
    assert!(!b.warnings.iter().any(|w| w.contains("regouts/clobbers")),
      "annotations are opt-in: {:?}", b.warnings);
  }

  #[test]
  fn exit_contract_warns_about_a_reg_changed_by_a_contract_call() {
    let code = &[
      0xE8, 0xFD, 0x00, // call callee at 0x0100
      0xC3,             // ret
    ];
    let (_types, mut cfg, binary, _) =
      test_spec_with_callee_contract(0x0000, 0x0004, code, 0x0100, None, None,
                                     Some(vec![instr::Reg::SI]));
    cfg.funcs.push(config::Func {
      name: "main".to_string(),
      start: SegOff { seg: Seg::Normal(0), off: Off(0) },
      end: Some(SegOff { seg: Seg::Normal(0), off: Off(4) }),
      entry: None,
      mode: config::CallMode::Near,
      ret: Some(Type::U16),
      args: Some(0),
      regargs: None,
      regouts: Some(vec![]),
      clobbers: None,
      dont_pop_args: false,
      regions: vec![],
    });
    let spec = spec::Spec::from_config_name(&cfg, "main").expect("main");
    let instrs = decode_all(&binary, &spec);
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    let contract_warnings: Vec<_> = b.warnings.iter()
      .filter(|w| w.contains("regouts/clobbers"))
      .collect();
    assert_eq!(contract_warnings.len(), 1,
      "the nested SI clobber must warn exactly once: {:?}", b.warnings);
    assert!(contract_warnings[0].contains("SI"),
      "the warning must name the changed register: {:?}", b.warnings);
  }

  /// A caller range plus one configured near callee at `callee_start`.
  fn test_spec_with_callee(
    start: u16, end: u16, code: &[u8],
    callee_start: u16, callee_regargs: Option<Vec<instr::Reg>>,
  ) -> (Rc<TypeDatabase>, Config, binary::Binary, spec::Spec<'static>) {
    test_spec_with_callee_contract(start, end, code, callee_start, callee_regargs,
                                   None, None)
  }

  fn test_spec_with_callee_contract(
    start: u16, end: u16, code: &[u8],
    callee_start: u16, callee_regargs: Option<Vec<instr::Reg>>,
    callee_regouts: Option<Vec<instr::Reg>>,
    callee_clobbers: Option<Vec<instr::Reg>>,
  ) -> (Rc<TypeDatabase>, Config, binary::Binary, spec::Spec<'static>) {
    let (types, mut cfg, binary, spec) = test_spec(start, end, code);
    cfg.funcs.push(config::Func {
      name: "callee".to_string(),
      start: SegOff { seg: Seg::Normal(0), off: Off(callee_start) },
      end: Some(SegOff { seg: Seg::Normal(0), off: Off(callee_start + 1) }),
      entry: None,
      mode: config::CallMode::Near,
      ret: None,
      args: Some(0),
      regargs: callee_regargs,
      regouts: callee_regouts,
      clobbers: callee_clobbers,
      dont_pop_args: false,
      regions: vec![],
    });
    (types, cfg, binary, spec)
  }

  fn regargs_warning_count(b: &IRBuilder) -> usize {
    b.warnings.iter().filter(|w| w.contains("regargs")).count()
  }

  /// The issue #132 shape: the caller sets up SI for the callee, restores it
  /// afterwards, and the annotation does not declare SI. The setup dies at the
  /// call; the build must say so instead of dropping it silently.
  #[test]
  fn incomplete_regargs_warns_about_a_dropped_setup() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0009, &[0xC3], 0x0100, Some(vec![instr::Reg::AX]));
    let instrs = [
      located_instr(instr::Opcode::OP_PUSH, 0x0000, 1, vec![operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_MOV, 0x0001, 3,
        vec![operand(instr::Reg::SI), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_CALL, 0x0004, 3, vec![rel_operand(0x00f9)]),
      located_instr(instr::Opcode::OP_POP, 0x0007, 1, vec![operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_RET, 0x0008, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 1, "one warning for the dropped SI setup: {:?}", b.warnings);
    let w = &b.warnings[0];
    assert!(w.starts_with("WARN: "), "diagnostics carry the WARN prefix: {w}");
    assert!(w.contains("SI"), "the dropped register must be named: {w}");
    assert!(w.contains("callee"), "the callee must be named: {w}");
    assert!(w.contains("0000:0004"), "the call site must be named: {w}");
  }

  /// Declaring the register keeps the setup and produces no diagnostic.
  #[test]
  fn complete_regargs_does_not_warn() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0009, &[0xC3], 0x0100,
        Some(vec![instr::Reg::AX, instr::Reg::SI]));
    let instrs = [
      located_instr(instr::Opcode::OP_PUSH, 0x0000, 1, vec![operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_MOV, 0x0001, 3,
        vec![operand(instr::Reg::SI), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_CALL, 0x0004, 3, vec![rel_operand(0x00f9)]),
      located_instr(instr::Opcode::OP_POP, 0x0007, 1, vec![operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_RET, 0x0008, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 0, "complete regargs must stay quiet: {:?}", b.warnings);
  }

  /// A value the caller reads after the call has a caller purpose, so it is
  /// not treated as a callee-only setup.
  #[test]
  fn register_live_after_the_call_does_not_warn() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0009, &[0xC3], 0x0100, Some(vec![]));
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x0000, 3,
        vec![operand(instr::Reg::SI), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_CALL, 0x0003, 3, vec![rel_operand(0x00fa)]),
      located_instr(instr::Opcode::OP_MOV, 0x0006, 2,
        vec![operand(instr::Reg::AX), operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_RET, 0x0008, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 0, "a live value is not a dropped setup: {:?}", b.warnings);
  }

  /// A contract-refreshed value is the callee's output, not a read of the
  /// caller setup that preceded the call, so it must not hide a dropped
  /// regargs setup.
  #[test]
  fn contracted_clobber_does_not_mask_a_regargs_setup() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee_contract(0x0000, 0x0009, &[0xC3], 0x0100,
        Some(vec![]), None, Some(vec![instr::Reg::BX]));
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x0000, 3,
        vec![operand(instr::Reg::BX), imm_operand(0x1111)]),
      located_instr(instr::Opcode::OP_CALL, 0x0003, 3, vec![rel_operand(0x00fa)]),
      located_instr(instr::Opcode::OP_MOV, 0x0006, 2,
        vec![operand(instr::Reg::AX), operand(instr::Reg::BX)]),
      located_instr(instr::Opcode::OP_RET, 0x0008, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 1,
      "the contract clobber must not mask the dropped BX setup: {:?}", b.warnings);
    assert!(b.warnings[0].contains("BX"), "the warning must name BX: {}", b.warnings[0]);
  }

  /// `regargs: None` means no annotation to be incomplete: stay silent.
  #[test]
  fn missing_regargs_annotation_does_not_warn() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0009, &[0xC3], 0x0100, None);
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x0000, 3,
        vec![operand(instr::Reg::SI), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_CALL, 0x0003, 3, vec![rel_operand(0x00fa)]),
      located_instr(instr::Opcode::OP_POP, 0x0006, 1, vec![operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_RET, 0x0007, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 0, "no annotation, no warning: {:?}", b.warnings);
  }

  /// The new exit-state write-back at `ret` materialises the setup value; that
  /// is not a caller read, so a call immediately before the return still warns.
  #[test]
  fn exit_state_write_back_is_not_a_caller_read() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0007, &[0xC3], 0x0100, Some(vec![]));
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x0000, 3,
        vec![operand(instr::Reg::SI), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_CALL, 0x0003, 3, vec![rel_operand(0x00fa)]),
      located_instr(instr::Opcode::OP_RET, 0x0006, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 1, "the exit write must not mask the dropped setup: {:?}", b.warnings);
  }

  /// The carry analysis rebuilds the IR speculatively; the diagnostic must be
  /// reported once for the retained pass, like the other build warnings.
  #[test]
  fn regargs_warning_is_not_duplicated_across_analysis_passes() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x000b, &[0xC3], 0x0100, Some(vec![instr::Reg::AX]));
    let instrs = [
      // ADC forces the carry analysis (and therefore multi-pass builds).
      located_instr(instr::Opcode::OP_ADC, 0x0000, 2,
        vec![operand(instr::Reg::AX), operand(instr::Reg::BX)]),
      located_instr(instr::Opcode::OP_PUSH, 0x0002, 1, vec![operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_MOV, 0x0003, 3,
        vec![operand(instr::Reg::SI), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_CALL, 0x0006, 3, vec![rel_operand(0x00f7)]),
      located_instr(instr::Opcode::OP_POP, 0x0009, 1, vec![operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_RET, 0x000a, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 1, "the SI diagnostic must be emitted once: {:?}", b.warnings);
  }

  /// A diamond merge creates phis for every register the call's annotation
  /// leaves out; when no path writes the register those phis carry only entry
  /// values and must not be reported as setups.
  #[test]
  fn call_at_a_merge_with_untouched_registers_does_not_warn() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x000b, &[0xC3], 0x0100, Some(vec![instr::Reg::DX]));
    let instrs = [
      located_instr(instr::Opcode::OP_CMP, 0x0000, 2,
        vec![operand(instr::Reg::AX), operand(instr::Reg::BX)]),
      jmp_instr(instr::Opcode::OP_JNE, 0x0002, 2, 3),
      located_instr(instr::Opcode::OP_MOV, 0x0004, 3,
        vec![operand(instr::Reg::DX), imm_operand(1)]),
      located_instr(instr::Opcode::OP_CALL, 0x0007, 3, vec![rel_operand(0x00f6)]),
      located_instr(instr::Opcode::OP_RET, 0x000a, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 0,
      "registers untouched on every path are not setups: {:?}", b.warnings);
  }

  /// The exit write-back runs after sealing. Before that, every non-entry
  /// block's `get_var` yields an incomplete phi, so a merge looked dirty for
  /// every callee-saved register; the write-back must not emit no-op writes.
  #[test]
  fn ret_at_a_merge_does_not_write_untouched_registers() {
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x000a, &[0u8; 0x000a]);
    let instrs = [
      alu_instr(instr::Opcode::OP_CMP, instr::Reg::AX, instr::Reg::BX),
      jmp_instr(instr::Opcode::OP_JNE, 0x0002, 2, 3),
      located_instr(instr::Opcode::OP_MOV, 0x0004, 3,
        vec![operand(instr::Reg::BX), imm_operand(1)]),
      located_instr(instr::Opcode::OP_MOV, 0x0007, 2,
        vec![operand(instr::Reg::AX), operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_RET, 0x0009, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    let mut writes: HashMap<String, usize> = HashMap::new();
    let mut materialised = 0;
    for blk in b.ir.iter_blocks() {
      for r in b.ir.iter_instrs(blk) {
        let instr = b.ir.instr(r).unwrap();
        if instr.attrs & Attribute::MATERIALISE != 0 { materialised += 1; }
        if instr.opcode != Opcode::WriteVar16 { continue; }
        let Ref::Symbol(dst) = instr.operands[0] else { continue; };
        *writes.entry(dst.name(&b.ir.symbols).to_string()).or_insert(0) += 1;
      }
    }
    assert_eq!(writes.get("BX"), Some(&1), "BX is written on one path: {writes:?}");
    for reg in ["CX", "DX", "SI", "DI"] {
      assert!(!writes.contains_key(reg), "{reg} is untouched on every path: {writes:?}");
    }
    assert_eq!(materialised, 0, "an entry-value return needs no snapshot");
    let exit_state = b.ir.iter_blocks().flat_map(|blk| b.ir.iter_instrs(blk))
      .filter(|r| b.ir.instr(*r).is_some_and(|i| i.attrs & Attribute::EXIT_STATE != 0))
      .count();
    assert_eq!(exit_state, 1, "only the BX write is exit state");
  }

  /// Decoding continues past a `ret` (padding, inline data, the next entry in
  /// the configured range) and Step 1 does not start a block there, so those
  /// instructions are lowered into the same block even though they never
  /// execute. They must not define the exit state: the write-back uses the
  /// register values captured at the ret.
  #[test]
  fn ret_at_a_merge_with_code_after_it_does_not_write_untouched_registers() {
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x000d, &[0u8; 0x000d]);
    let instrs = [
      alu_instr(instr::Opcode::OP_CMP, instr::Reg::AX, instr::Reg::BX),
      jmp_instr(instr::Opcode::OP_JNE, 0x0002, 2, 3),
      located_instr(instr::Opcode::OP_MOV, 0x0004, 3,
        vec![operand(instr::Reg::BX), imm_operand(1)]),
      located_instr(instr::Opcode::OP_MOV, 0x0007, 2,
        vec![operand(instr::Reg::AX), operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_RET, 0x0009, 1, vec![]),
      located_instr(instr::Opcode::OP_MOV, 0x000a, 3,
        vec![operand(instr::Reg::SI), imm_operand(2)]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    let mut writes: HashMap<String, usize> = HashMap::new();
    for blk in b.ir.iter_blocks() {
      for r in b.ir.iter_instrs(blk) {
        let instr = b.ir.instr(r).unwrap();
        if instr.opcode != Opcode::WriteVar16 { continue; }
        let Ref::Symbol(dst) = instr.operands[0] else { continue; };
        *writes.entry(dst.name(&b.ir.symbols).to_string()).or_insert(0) += 1;
      }
    }
    assert_eq!(writes.get("BX"), Some(&1), "BX is written on one path: {writes:?}");
    for reg in ["CX", "DX", "SI", "DI"] {
      assert!(!writes.contains_key(reg), "{reg} is untouched at the ret: {writes:?}");
    }
    let exit_state = b.ir.iter_blocks().flat_map(|blk| b.ir.iter_instrs(blk))
      .filter(|r| b.ir.instr(*r).is_some_and(|i| i.attrs & Attribute::EXIT_STATE != 0))
      .count();
    assert_eq!(exit_state, 1, "only the BX write is exit state");
  }

  /// A register that is dirty at the ret keeps the value captured there even
  /// when unreachable code after the ret writes the same register.
  #[test]
  fn ret_at_a_merge_keeps_the_exit_value_not_the_code_after_it() {
    let (_types, cfg, binary, spec) = test_spec(0x0000, 0x000d, &[0u8; 0x000d]);
    let instrs = [
      alu_instr(instr::Opcode::OP_CMP, instr::Reg::AX, instr::Reg::BX),
      jmp_instr(instr::Opcode::OP_JNE, 0x0002, 2, 3),
      located_instr(instr::Opcode::OP_MOV, 0x0004, 3,
        vec![operand(instr::Reg::BX), imm_operand(1)]),
      located_instr(instr::Opcode::OP_MOV, 0x0007, 2,
        vec![operand(instr::Reg::AX), operand(instr::Reg::SI)]),
      located_instr(instr::Opcode::OP_RET, 0x0009, 1, vec![]),
      located_instr(instr::Opcode::OP_MOV, 0x000a, 3,
        vec![operand(instr::Reg::BX), imm_operand(2)]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    let mut value = None;
    for blk in b.ir.iter_blocks() {
      for r in b.ir.iter_instrs(blk) {
        let instr = b.ir.instr(r).unwrap();
        if instr.opcode != Opcode::WriteVar16 { continue; }
        let Ref::Symbol(dst) = instr.operands[0] else { continue; };
        if dst.name(&b.ir.symbols) == "BX" { value = Some(instr.operands[1]); }
      }
    }
    let value = value.expect("BX is callee-saved and set on one path");
    assert!(matches!(value, Ref::Instr(_, _)),
      "the exit value is the merge phi captured at the ret, not the later write: {value:?}");
  }

  /// A phi that carries a real write on one path is a genuine setup and must
  /// still be reported; the trivial-phi fix must not suppress it.
  #[test]
  fn a_phi_with_one_real_write_still_warns() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x000b, &[0xC3], 0x0100, Some(vec![instr::Reg::DX]));
    let instrs = [
      located_instr(instr::Opcode::OP_CMP, 0x0000, 2,
        vec![operand(instr::Reg::AX), operand(instr::Reg::BX)]),
      jmp_instr(instr::Opcode::OP_JNE, 0x0002, 2, 3),
      located_instr(instr::Opcode::OP_MOV, 0x0004, 3,
        vec![operand(instr::Reg::SI), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_CALL, 0x0007, 3, vec![rel_operand(0x00f6)]),
      located_instr(instr::Opcode::OP_RET, 0x000a, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 1,
      "the path that sets SI still makes it a setup: {:?}", b.warnings);
    assert!(b.warnings[0].contains("SI"), "the warning must name SI: {}", b.warnings[0]);
  }

  /// A caller read of a byte fragment is a read of the setup it was split
  /// from, even though the operand is a derived `And` ref, not the setup ref.
  #[test]
  fn a_fragment_read_after_the_call_does_not_warn() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0009, &[0xC3], 0x0100, Some(vec![]));
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x0000, 3,
        vec![operand(instr::Reg::BX), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_CALL, 0x0003, 3, vec![rel_operand(0x00fa)]),
      located_instr(instr::Opcode::OP_MOV, 0x0006, 2,
        vec![operand(instr::Reg::AL), operand(instr::Reg::BL)]),
      located_instr(instr::Opcode::OP_RET, 0x0008, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 0,
      "reading BL uses the BX setup: {:?}", b.warnings);
  }

  /// A changed DS emits an exit-state write at the return, but that write is
  /// not a caller read of a DS setup whose value it only preserves.
  #[test]
  fn a_ret_exit_write_does_not_mask_a_ds_setup() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0006, &[0xC3], 0x0100, Some(vec![]));
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x0000, 2,
        vec![operand(instr::Reg::DS), operand(instr::Reg::AX)]),
      located_instr(instr::Opcode::OP_CALL, 0x0002, 3, vec![rel_operand(0x00fb)]),
      located_instr(instr::Opcode::OP_RET, 0x0005, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 1,
      "the ret exit write must not hide the DS setup: {:?}", b.warnings);
    assert!(b.warnings[0].contains("DS"), "the warning must name DS: {}", b.warnings[0]);
  }

  /// `save_return_value` refreshes the CX byte fragments after every call from
  /// the pre-call value; that dead refresh is not a caller read, so a CX setup
  /// overwritten right after the call is still reported.
  #[test]
  fn a_dead_cx_refresh_does_not_mask_the_setup() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x000a, &[0xC3], 0x0100, Some(vec![]));
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x0000, 3,
        vec![operand(instr::Reg::CX), imm_operand(5)]),
      located_instr(instr::Opcode::OP_CALL, 0x0003, 3, vec![rel_operand(0x00fa)]),
      located_instr(instr::Opcode::OP_MOV, 0x0006, 3,
        vec![operand(instr::Reg::CX), imm_operand(6)]),
      located_instr(instr::Opcode::OP_RET, 0x0009, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 1,
      "the overwritten CX setup must be reported: {:?}", b.warnings);
    assert!(b.warnings[0].contains("CX"), "the warning must name CX: {}", b.warnings[0]);
  }

  /// A previous call's return value is caller-saved exit state, not a setup for
  /// the next call. Ignoring the result must not make the next call warn about
  /// AX/DX.
  #[test]
  fn a_previous_call_result_is_not_a_dropped_setup() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0007, &[0xC3], 0x0100, Some(vec![]));
    let instrs = [
      located_instr(instr::Opcode::OP_CALL, 0x0000, 3, vec![rel_operand(0x00fd)]),
      located_instr(instr::Opcode::OP_CALL, 0x0003, 3, vec![rel_operand(0x00fa)]),
      located_instr(instr::Opcode::OP_RET, 0x0006, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 0,
      "a stale call result is not a callee setup: {:?}", b.warnings);
  }

  /// A scratch value consumed while preparing a declared regarg is not a
  /// dropped setup for the callee: the live pinned write reads it.
  #[test]
  fn a_copy_into_a_declared_regarg_is_not_a_dropped_setup() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0009, &[0xC3], 0x0100, Some(vec![instr::Reg::BX]));
    let instrs = [
      located_instr(instr::Opcode::OP_MOV, 0x0000, 3,
        vec![operand(instr::Reg::AX), imm_operand(0x1234)]),
      located_instr(instr::Opcode::OP_MOV, 0x0003, 2,
        vec![operand(instr::Reg::BX), operand(instr::Reg::AX)]),
      located_instr(instr::Opcode::OP_CALL, 0x0005, 3, vec![rel_operand(0x00f8)]),
      located_instr(instr::Opcode::OP_RET, 0x0008, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 0,
      "AX is consumed by the live BX write: {:?}", b.warnings);
  }

  /// A call in a block that control flow cannot reach is not an emitted call,
  /// so it must not warn about the register "state" of dead code.
  #[test]
  fn a_call_in_an_unreachable_block_does_not_warn() {
    let (_types, cfg, binary, spec) =
      test_spec_with_callee(0x0000, 0x0006, &[0xC3], 0x0100, Some(vec![]));
    let instrs = [
      jmp_instr(instr::Opcode::OP_JMP, 0x0000, 2, 3),
      located_instr(instr::Opcode::OP_CALL, 0x0002, 3, vec![rel_operand(0x00fb)]),
      located_instr(instr::Opcode::OP_RET, 0x0005, 1, vec![]),
    ];
    let mut b = test_builder(&cfg, &instrs, &spec, &binary);
    b.build();

    assert_eq!(regargs_warning_count(&b), 0,
      "a jump-over call is not emitted: {:?}", b.warnings);
  }
}
