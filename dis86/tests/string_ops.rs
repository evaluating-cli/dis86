// Regression tests for single-step string-op pointer auto-update
// (issue #69, slice 2). Previously MOVS/STOS/LODS/SCAS/CMPS moved the
// element but never advanced SI/DI, so a hand-written `movsb` loop
// decompiled to C that copies the same byte forever (silent-wrong).
use dis86::asm::decode::Decoder;
use dis86::asm::instr::Instr;
use dis86::config::Config;
use dis86::decompile::ast::Function;
use dis86::decompile::control_flow::ControlFlow;
use dis86::decompile::gen::{self, Flavor};
use dis86::decompile::{ir, ir_build, opt, sym};
use dis86::segoff::{Off, Seg, SegOff};
use dis86::spec::Spec;
use dis86::types::TypeDatabase;

fn empty_config(types: std::rc::Rc<TypeDatabase>) -> Config {
  Config {
    types,
    structs: vec![],
    code_segs: vec![],
    funcs: vec![],
    indirects: vec![],
    globals: vec![],
    text_section: vec![],
  }
}

fn decompile_raw(code: &[u8]) -> String {
  let types = std::rc::Rc::new(TypeDatabase::new());
  let cfg = empty_config(types);
  let binary = dis86::binary::Binary::from_raw(code, None);
  let spec = Spec::from_start_and_end(
    Some(SegOff { seg: Seg::Normal(0), off: Off(0) }),
    Some(SegOff { seg: Seg::Normal(0), off: Off(code.len() as u16) }),
  );

  let mut decoder = Decoder::new(binary.region_iter(spec.start, spec.end));
  let mut instr_list: Vec<Instr> = vec![];
  while let Some((ins, _raw)) = decoder.try_next().expect("decode") {
    instr_list.push(ins);
  }

  let mut ir = ir_build::build_from_instrs(&instr_list, &cfg, &spec, &binary, false, false);
  opt::optimize(&mut ir);
  sym::symbolize(&mut ir, &cfg);
  opt::forward_store_to_load(&mut ir);
  opt::optimize(&mut ir);
  opt::mem_symbol_to_ref(&mut ir);
  opt::optimize(&mut ir);
  dis86::decompile::fuse::fuse_mem(&mut ir);
  opt::optimize(&mut ir);
  ir::fin::finalize(&mut ir);

  let cf = ControlFlow::from_ir(&ir);
  let ast = Function::from_ir(&cfg, "main", None, &ir, &cf).expect("ast");
  gen::generate(&ast, Flavor::Standard).expect("codegen")
}

// gen.rs renders both the abort-class value UNIMPL and the loud flags
// marker as `UNIMPL(...)`. A bare `UNIMPL()` statement is the marker; any
// surviving `UNIMPL(` after stripping it is a value-position abort.
fn has_value_unimpl(source: &str) -> bool {
  source.replace("UNIMPL()", "").contains("UNIMPL(")
}

// mov cx,4; L: movsb; dec cx; jnz L; ret
// The pointer update is modeled, so the emitted loop advances SI/DI and
// terminates instead of copying the same byte forever.
#[test]
fn movsb_loop_advances_pointers() {
  let code = [0xB9, 0x04, 0x00, 0xA4, 0x49, 0x75, 0xFC, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "no value-position UNIMPL:\n{source}");
  assert!(
    source.contains("si_2 + 1") && source.contains("di_2 + 1"),
    "both string pointers must advance by one per iteration:\n{source}"
  );
  assert!(
    source.contains("*PTR_8(ES, di_2) = *PTR_8(DS, si_2);"),
    "the byte move itself must remain:\n{source}"
  );
}

// movsw; mov ax,si; ret
// The 16-bit element advances the pointers by two, and the update is
// observable through a later SI read.
#[test]
fn movsw_advances_by_two_and_is_observable() {
  let code = [0xA5, 0x89, 0xF0, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "no value-position UNIMPL:\n{source}");
  assert!(
    source.contains("SI + 2"),
    "movsw must advance SI by the element size:\n{source}"
  );
}

// std; movsb; ret
// With a `std` in the function, DF is not provably clear, so the pointer
// update degrades to the loud marker instead of guessing the direction.
#[test]
fn std_makes_string_update_loud() {
  let code = [0xFD, 0xA4, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "must stay a loud marker, not a value abort:\n{source}");
  assert!(
    source.contains("UNIMPL() /* UNIMPL op */;"),
    "the uncertain direction must be flagged loudly:\n{source}"
  );
  assert!(
    source.contains("*PTR_8(ES, DI) = *PTR_8(DS, SI);"),
    "the observed single step still happens:\n{source}"
  );
}

// movsb; std; ret
// The step precedes the writer, so DF is still the ABI entry value (clear):
// the pointer update must be modeled, not poisoned by the later `std`.
#[test]
fn movsb_before_std_is_modeled() {
  let code = [0xA4, 0xFD, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "the pre-writer step must be modeled:\n{source}");
  assert!(
    source.contains("*PTR_8(ES, DI) = *PTR_8(DS, SI);"),
    "the step still happens:\n{source}"
  );
}

// movsb; std; mov ax,si; ret
// The advance is observable after the store, so pin the exact increment.
#[test]
fn movsb_before_std_advances_pointers() {
  let code = [0xA4, 0xFD, 0x89, 0xF0, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "the pre-writer step must be modeled:\n{source}");
  assert!(source.contains("SI + 1"), "SI must advance before the std:\n{source}");
}

// std; cld; movsb; mov ax,si; ret
// CLD re-establishes clear before the step; the writer is local to the block.
#[test]
fn cld_after_std_models_the_following_step() {
  let code = [0xFD, 0xFC, 0xA4, 0x89, 0xF0, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "cld must restore the modeled direction:\n{source}");
  assert!(source.contains("SI + 1"), "SI must advance by one:\n{source}");
}

// std; cld; movsb; loop $-3; ret
// The loop back-edge targets the movsb; both paths into it leave DF clear, so
// the loop-carried entry is clear (greatest fixpoint) and the body models.
#[test]
fn clear_loop_body_is_modeled_across_the_back_edge() {
  let code = [0xFD, 0xFC, 0xA4, 0xE2, 0xFD, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "a clear loop body must be modeled:\n{source}");
  assert!(
    source.contains("si_2 + 1") && source.contains("di_2 + 1"),
    "both pointers must advance in the loop:\n{source}"
  );
}

// movsb; loop $-3; std; ret
// The back-edge targets the ENTRY block: `switch_blk` must not erase the
// contract-provided entry seed (issue #102's caveat). The trailing `std` is
// required to exercise the path — with no DF writer the entry is trivially
// clear and an erasing reset would be invisible.
#[test]
fn back_edge_to_the_entry_block_stays_clear() {
  let code = [0xA4, 0xE2, 0xFD, 0xFD, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "the entry-seeded loop must be modeled:\n{source}");
  assert!(
    source.contains("si_2 + 1") && source.contains("di_2 + 1"),
    "both pointers must advance in the loop:\n{source}"
  );
}

// cld; movsb; loop $-3; std; movsb; ret
// Forward loop first, backward copy later: the loop is clear; only the step
// after the in-function std stays loud.
#[test]
fn forward_loop_and_later_std_poison_only_the_later_step() {
  let code = [0xFC, 0xA4, 0xE2, 0xFD, 0xFD, 0xA4, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "no value-position abort:\n{source}");
  assert_eq!(
    source.matches("UNIMPL() /* UNIMPL op */").count(), 1,
    "only the step after std is unknown:\n{source}"
  );
}

// cld; jz +2; std; nop; movsb; ret
// One arm sets DF, so the merge is genuinely unknown: stay loud.
#[test]
fn join_with_a_df_setting_arm_stays_loud() {
  let code = [0xFC, 0x74, 0x02, 0xFD, 0x90, 0xA4, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "must stay a loud marker:\n{source}");
  assert!(
    source.contains("UNIMPL() /* UNIMPL op */;"),
    "a DF-setting predecessor must keep the merge loud:\n{source}"
  );
}

// std; movsb; loop $-3; ret
// The loop is entered through a DF-setting block, so its body stays loud.
#[test]
fn unclear_loop_body_stays_loud() {
  let code = [0xFD, 0xA4, 0xE2, 0xFD, 0xC3];
  let source = decompile_raw(&code);

  assert!(
    source.contains("UNIMPL() /* UNIMPL op */;"),
    "an unknown loop entry must stay loud:\n{source}"
  );
}

// popf; movsb; mov ax,si; ret
// POPF restores opaque flags, so a following step's direction is unknown.
#[test]
fn popf_before_a_step_stays_loud() {
  let code = [0x9D, 0xA4, 0x89, 0xF0, 0xC3];
  let source = decompile_raw(&code);

  assert!(
    source.contains("UNIMPL() /* UNIMPL op */;"),
    "an opaque popf must keep the step loud:\n{source}"
  );
}

// movsb; iret; ret
// The step precedes the IRET (which restores FLAGS), so it is modeled; only
// the IRET itself is loud.
#[test]
fn step_before_iret_is_modeled() {
  let code = [0xA4, 0xCF, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "no value-position abort:\n{source}");
  assert_eq!(
    source.matches("UNIMPL() /* UNIMPL op */").count(), 1,
    "only the IRET is unknown:\n{source}"
  );
}

// movsb; popf; mov ax,si; ret
// The step precedes the popf, so it is modeled; only later steps are unknown.
#[test]
fn step_before_popf_is_modeled() {
  let code = [0xA4, 0x9D, 0x89, 0xF0, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "the pre-popf step must be modeled:\n{source}");
  assert!(source.contains("SI + 1"), "SI must advance before the popf:\n{source}");
}

// stc; std; jz +2; jmp $; rep movsb; jz $; adc ax,bx; ret
// `stc` leaves a known carry and `std` an unclear DF; the `rep` block is the
// only path to the `adc`. Because DF is unclear there, the retained build
// lowers the rep through the loud path, which clobbers flags. The carry
// must-analysis is seeded with the DF map so it reaches the same conclusion:
// without the seed it would model the rep, forward the `stc` carry into the
// `adc`, and disagree with the IR it summarizes. Pin the conservative result.
#[test]
fn df_unclear_rep_clears_carry_for_a_later_adc() {
  let code = [0xF9, 0xFD, 0x74, 0x02, 0xEB, 0x00, 0xF3, 0xA4, 0x74, 0x00, 0x11, 0xD8, 0xC3];
  let source = decompile_raw(&code);

  assert!(
    source.contains("UNIMPL() /* UNIMPL op */"),
    "the loud rep must be flagged:\n{source}"
  );
  assert!(
    has_value_unimpl(&source),
    "the adc after the flag-clobbering rep must degrade loud:\n{source}"
  );
  assert!(
    !source.contains("(AX + BX) + 1"),
    "the adc must not reuse the pre-rep carry:\n{source}"
  );
}
