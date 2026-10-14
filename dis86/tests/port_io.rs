// Regression tests for port I/O lowering (issue #69): IN/OUT render as the
// hydra INB/INW/OUTB/OUTW primitives instead of the abort-class UNIMPL.
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

// in al,dx; ret
#[test]
fn in_al_dx_uses_inb() {
  let source = decompile_raw(&[0xEC, 0xC3]);
  assert!(!source.contains("UNIMPL"), "IN must not be UNIMPL:\n{source}");
  assert!(source.contains("INB(DX)"), "byte port read must use INB:\n{source}");
}

// in ax,0x60; ret
#[test]
fn in_ax_imm_uses_inw() {
  let source = decompile_raw(&[0xE5, 0x60, 0xC3]);
  assert!(!source.contains("UNIMPL"), "IN must not be UNIMPL:\n{source}");
  assert!(source.contains("INW(96)"), "word port read must use INW with the decimal port:\n{source}");
}

// out dx,ax; ret
#[test]
fn out_dx_ax_uses_outw() {
  let source = decompile_raw(&[0xEF, 0xC3]);
  assert!(!source.contains("UNIMPL"), "OUT must not be UNIMPL:\n{source}");
  assert!(source.contains("OUTW(DX, AX);"), "word port write must use OUTW:\n{source}");
}

// out 0x20,al; ret
#[test]
fn out_imm_al_uses_outb() {
  let source = decompile_raw(&[0xE6, 0x20, 0xC3]);
  assert!(!source.contains("UNIMPL"), "OUT must not be UNIMPL:\n{source}");
  assert!(source.contains("OUTB(32, AL);"), "byte port write must use OUTB:\n{source}");
}

// in al,dx; out dx,al; ret
// Both reads and writes are side effects and must survive optimization.
#[test]
fn port_io_survives_optimization() {
  let source = decompile_raw(&[0xEC, 0xEE, 0xC3]);
  assert!(!source.contains("UNIMPL"), "no UNIMPL:\n{source}");
  assert!(source.contains("INB(DX)"), "the read must remain:\n{source}");
  assert!(source.contains("OUTB(DX,"), "the write must remain:\n{source}");
}
