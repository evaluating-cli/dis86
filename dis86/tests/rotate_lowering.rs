// Regression tests for ROL/ROR lowering (issue #69, slice 1): rotates
// without carry-in lower value-exactly to shifts+or instead of the
// abort-class UNIMPL macro, while flags stay an opaque (loud) marker.
// RCL/RCR still need carry dataflow and stay UNIMPL (pinned below).
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

// Runs the pipeline app.rs uses, up to and including C generation.
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

// mov ax,0x1234; rol ax,1; mov [0x100],ax; ret
// The rotate must reach the output as shifts, not the abort macro. The
// store keeps the rotated value observable (a dead AX would be
// legitimately eliminated).
#[test]
fn rol_imm_decompiles_without_unimpl() {
  let code = [
    0xB8, 0x34, 0x12, // mov ax,0x1234
    0xC1, 0xC0, 0x01, // rol ax,1
    0xA3, 0x00, 0x01, // mov [0x100],ax
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  // The bare `UNIMPL()` *statement* is the intended loud flags marker
  // (same as `stc` produces); what must be gone is an operand-carrying
  // `UNIMPL(..)` in value position, which aborts the native run.
  assert!(!source.contains("UNIMPL(("), "ROL value must not be UNIMPL:\n{source}");
  assert!(source.contains("<<"), "ROL must lower to a left shift:\n{source}");
  assert!(source.contains(">>"), "ROL must lower to a right shift:\n{source}");
}

// mov bl,7; ror bl,cl; mov [0x100],bl; ret
// Register count and 8-bit width must flow through the same shape.
#[test]
fn ror_cl_decompiles_without_unimpl() {
  let code = [
    0xB3, 0x07,       // mov bl,7
    0xD3, 0xCB,       // ror bl,cl
    0x88, 0x1E, 0x00, 0x01, // mov [0x100],bl
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL(("), "ROR value must not be UNIMPL:\n{source}");
  assert!(source.contains("<<"), "ROR must lower to a left shift:\n{source}");
  assert!(source.contains(">>"), "ROR must lower to a right shift:\n{source}");
}

// mov ax,0x1234; rcl ax,1; ret
// RCL rotates through carry (needs CF dataflow): still UNIMPL.
#[test]
fn rcl_still_emits_unimpl() {
  let code = [
    0xB8, 0x34, 0x12, // mov ax,0x1234
    0xC1, 0xD0, 0x01, // rcl ax,1
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL("), "RCL still needs the carry slice:\n{source}");
}
