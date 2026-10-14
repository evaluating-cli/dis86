// Regression tests for REP LODS lowering (issue #69): the counted load loop
// becomes a runtime helper returning the final accumulator, with SI/CX
// updates modeled in the SSA. A possibly-set direction flag stays loud.
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

// mov cx,3; rep lodsw; mov dx,si; ret
#[test]
fn rep_lodsw_returns_final_and_advances_si() {
  let code = [
    0xB9, 0x03, 0x00, // mov cx,3
    0xF3, 0xAD,       // rep lodsw
    0x89, 0xF2,       // mov dx,si (observe SI)
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "no imprecision marker:\n{source}");
  assert!(source.contains("REP_LODSW(DS, SI, 3, AX)"), "the counted load must use the helper:\n{source}");
  assert!(source.contains("SI + ") && source.contains("3 * 2"), "SI must advance by count*2:\n{source}");
}

// mov cx,4; rep lodsb; mov dx,si; ret
#[test]
fn rep_lodsb_advances_by_one() {
  let code = [
    0xB9, 0x04, 0x00, // mov cx,4
    0xF3, 0xAC,       // rep lodsb
    0x89, 0xF2,       // mov dx,si
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "no imprecision marker:\n{source}");
  assert!(source.contains("REP_LODSB(DS, SI, 4, AL)"), "byte load helper:\n{source}");
  assert!(source.contains("4 * 1"), "SI advances by count*1:\n{source}");
}

// std; mov cx,4; rep lodsb; ret
#[test]
fn std_makes_rep_lods_loud() {
  let code = [0xFD, 0xB9, 0x04, 0x00, 0xF3, 0xAC, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL()"), "a possible std must stay loud:\n{source}");
}
