// Regression tests for REP CMPS/SCAS lowering (issue #69): the counted
// compare loops become runtime helpers returning the remaining CX, with
// SI/DI/CX modeled exactly; the (data-dependent) comparison flags are
// clobbered to the non-fatal UNIMPL_FLAGS marker.
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

// mov cx,5; repne scasb; mov ax,cx; mov dx,di; ret
#[test]
fn repne_scasb_models_cx_and_di() {
  let code = [
    0xB9, 0x05, 0x00, // mov cx,5
    0xF2, 0xAE,       // repne scasb
    0x89, 0xC8,       // mov ax,cx
    0x89, 0xFA,       // mov dx,di
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL("), "no value abort marker:\n{source}");
  assert!(source.contains("REPNE_SCASB(ES, DI, 5, AL)"), "the compare loop must use the helper:\n{source}");
  assert!(source.contains("= DI + ") && source.contains("(5 - "),
    "DI must advance by (CX0 - cx_final):\n{source}");
}

// mov cx,4; repe cmpsb; mov ax,cx; mov dx,si; ret
#[test]
fn repe_cmpsb_models_cx_and_si() {
  let code = [
    0xB9, 0x04, 0x00, // mov cx,4
    0xF3, 0xA6,       // repe cmpsb
    0x89, 0xC8,       // mov ax,cx
    0x89, 0xF2,       // mov dx,si
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL("), "no value abort marker:\n{source}");
  assert!(source.contains("REPE_CMPSB(DS, SI, ES, DI, 4)"), "the compare loop must use the helper:\n{source}");
}

// mov cx,3; repne scasw; mov ax,cx; ret
#[test]
fn repne_scasw_uses_the_word_helper() {
  let code = [0xB9, 0x03, 0x00, 0xF2, 0xAF, 0x89, 0xC8, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("REPNE_SCASW(ES, DI, 3, AX)"), "word compare helper:\n{source}");
}

// mov cx,4; repe cmpsb; jz +1; nop; ret
// The final comparison's flags are unknown, so the branch degrades to the
// non-fatal UNIMPL_FLAGS marker, not a fatal UNIMPL abort.
#[test]
fn rep_cmps_flag_branch_degrades_non_fatally() {
  let code = [0xB9, 0x04, 0x00, 0xF3, 0xA6, 0x74, 0x01, 0x90, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL_FLAGS"), "the flag branch must be loud:\n{source}");
  assert!(!source.replace("UNIMPL_FLAGS", "").contains("UNIMPL("),
    "the flags marker must not be a fatal UNIMPL:\n{source}");
}

// std; mov cx,4; repe cmpsb; ret
#[test]
fn std_makes_rep_cmps_loud() {
  let code = [0xFD, 0xB9, 0x04, 0x00, 0xF3, 0xA6, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL()"), "a possible std must stay loud:\n{source}");
}
