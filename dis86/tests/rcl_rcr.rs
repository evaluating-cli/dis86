// Regression tests for RCL/RCR lowering (issue #69): with a constant count
// and a known carry-in they lower exactly via a (w+1)-bit rotate over u32
// ops; variable counts and unknown carries stay loud. Rotates must not emit
// a fatal UNIMPL abort for their flags clobber.
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

// stc; rcl ax,1; ret
#[test]
fn stc_rcl_by_one_is_exact() {
  let source = decompile_raw(&[0xF9, 0xD1, 0xD0, 0xC3]);
  assert!(!source.contains("UNIMPL"), "no abort marker for an exact rotate:\n{source}");
  assert!(source.contains("<< 1)") && source.contains(">> 16)"),
    "RCL by 1 shifts the 17-bit value by 1 and 16:\n{source}");
}

// clc; rcr ax,1; ret
#[test]
fn clc_rcr_by_one_is_exact() {
  let source = decompile_raw(&[0xF8, 0xD1, 0xD8, 0xC3]);
  assert!(!source.contains("UNIMPL"), "no abort marker:\n{source}");
  assert!(source.contains(">> 1)") && source.contains("<< 16)"),
    "RCR by 1 shifts the 17-bit value right by 1 and left by 16:\n{source}");
}

// stc; rcl ax,3; ret
#[test]
fn rcl_count_uses_w_plus_one() {
  let source = decompile_raw(&[0xF9, 0xC1, 0xD0, 0x03, 0xC3]);
  assert!(!source.contains("UNIMPL"), "no abort marker:\n{source}");
  assert!(source.contains("<< 3)") && source.contains(">> 14)"),
    "RCL by n shifts by n and (17 - n):\n{source}");
}

// add ax,bx; rcl cx,1; mov ax,cx; ret
#[test]
fn rcl_uses_the_producer_carry() {
  let source = decompile_raw(&[0x01, 0xD8, 0xD1, 0xD1, 0x89, 0xC8, 0xC3]);
  assert!(!source.contains("UNIMPL"), "no abort marker:\n{source}");
  assert!(source.contains("(AX + BX) < AX"), "the carry-in must be the add's carry:\n{source}");
}

// stc; rcl ax,1; rcl bx,1; mov ax,bx; ret
#[test]
fn rcl_carries_out_into_the_next_rotate() {
  let source = decompile_raw(&[0xF9, 0xD1, 0xD0, 0xD1, 0xD3, 0x89, 0xD8, 0xC3]);
  assert!(!source.contains("UNIMPL"), "no abort marker:\n{source}");
  // The second rotate's carry-in is the first rotate's carry-out (bit 16).
  assert!(source.contains(">> 16) & 1"), "the prior carry-out must feed the next rotate:\n{source}");
}

// mov ax,1; rcl ax,1; ret
#[test]
fn rcl_without_known_carry_is_loud() {
  let source = decompile_raw(&[0xB8, 0x01, 0x00, 0xD1, 0xD0, 0xC3]);
  assert!(source.contains("UNIMPL("), "unknown carry must stay loud:\n{source}");
}

// rcl ax,cl; ret
#[test]
fn rcl_variable_count_is_loud() {
  let source = decompile_raw(&[0xD3, 0xD0, 0xC3]);
  assert!(source.contains("UNIMPL(AX, CL)"), "a CL count must stay loud:\n{source}");
}

// stc; rcl ax,1; jz +1; nop; ret
// The flags after a rotate are unknown, but the degradation must be the
// non-fatal UNIMPL_FLAGS marker, never a fatal UNIMPL abort.
#[test]
fn rotate_flag_branch_degrades_non_fatally() {
  let source = decompile_raw(&[0xF9, 0xD1, 0xD0, 0x74, 0x01, 0x90, 0xC3]);
  assert!(source.contains("UNIMPL_FLAGS"), "the flag branch must be loud:\n{source}");
  assert!(!source.replace("UNIMPL_FLAGS", "").contains("UNIMPL("),
    "the flags marker must not be a fatal UNIMPL:\n{source}");
}
