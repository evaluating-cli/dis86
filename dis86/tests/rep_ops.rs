// Regression tests for REP-prefixed MOVS/STOS lowering (issue #69, slice 3).
// These are modeled exactly (runtime bulk macro + deterministic CX/SI/DI
// updates); REP LODS/CMPS/SCAS and any rep with a possibly-set direction
// flag stay loud.
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

// mov cx,4; rep movsb; mov ax,si; mov dx,di; ret
// The whole counted copy lowers to the runtime macro, and CX/SI/DI are
// updated deterministically (SI and DI advance by CX0*size).
#[test]
fn rep_movsb_emits_macro_and_advances_pointers() {
  let code = [0xB9, 0x04, 0x00, 0xF3, 0xA4, 0x89, 0xF0, 0x89, 0xFA, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "no imprecision marker for modeled rep:\n{source}");
  assert!(
    source.contains("REP_MOVSB(DS, SI, ES, DI, 4);"),
    "the counted copy must be the runtime macro with the pre-update values:\n{source}"
  );
  assert!(
    source.contains("= SI + ") && source.contains("= DI + "),
    "both pointers must advance by CX0*size:\n{source}"
  );
  assert!(source.contains("4 * 1"), "byte size 1 times the count:\n{source}");
}

// mov cx,3; rep stosw; mov ax,di; ret
// Word stores use REP_STOSW and advance DI by count*2.
#[test]
fn rep_stosw_emits_macro_and_advances_di() {
  let code = [0xB9, 0x03, 0x00, 0xF3, 0xAB, 0x89, 0xF8, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "no imprecision marker for modeled rep:\n{source}");
  assert!(
    source.contains("REP_STOSW(ES, DI, 3, AX);"),
    "the word fill must be the runtime macro with the value:\n{source}"
  );
  assert!(source.contains("= DI + "), "DI must advance:\n{source}");
  assert!(source.contains("3 * 2"), "word size 2 times the count:\n{source}");
}

// mov cx,2; rep lodsb; ret
// LODS/CMPS/SCAS are data-dependent and not modeled yet: stay loud.
#[test]
fn rep_lods_stays_loud() {
  let code = [0xB9, 0x02, 0x00, 0xF3, 0xAC, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL()"), "rep lods must stay loud:\n{source}");
  assert!(!source.contains("REP_"), "must not emit a bulk macro:\n{source}");
}

// std; mov cx,4; rep movsb; ret
// With a std in the function the direction may be set, so the rep stays
// loud instead of running the copy the wrong way.
#[test]
fn std_makes_rep_stay_loud() {
  let code = [0xFD, 0xB9, 0x04, 0x00, 0xF3, 0xA4, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL()"), "rep with a possible std must stay loud:\n{source}");
  assert!(!source.contains("REP_MOVSB("), "must not emit the assumed-direction macro:\n{source}");
}

// The modeled rep must not clobber flags: `sub ax,ax; rep movsb; jz; ret`
// must still fold the branch against the pre-rep `sub` (movs/stos don't
// touch FLAGS), unlike the old Unimpl lowering.
#[test]
fn modeled_rep_preserves_flags() {
  let code = [
    0x2B, 0xC0,             // sub ax,ax
    0xB9, 0x04, 0x00,       // mov cx,4
    0xF3, 0xA4,             // rep movsb
    0x74, 0x01,             // jz +1
    0x90,                   // nop
    0xC3,                   // ret
  ];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "modeled rep must not touch flags:\n{source}");
  assert!(
    source.contains("AX - AX") && source.contains("AX != AX"),
    "the branch must fold against the pre-rep `sub ax,ax` producer:\n{source}"
  );
}
