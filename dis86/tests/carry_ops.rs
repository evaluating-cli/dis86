// Regression tests for carry-in lowering (issue #69). ADC/SBB are lowered
// exactly when the incoming carry is a tracked carry-out of a previous
// instruction in the same basic block, for both byte and word operands;
// cross-block carries stay loud.
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

// add ax,bx; adc cx,dx; mov ax,cx; ret
// ADC lowers to `(CX + DX) + carry(AX+BX)` exactly.
#[test]
fn add_adc_chain_is_exact() {
  let code = [0x01, 0xD8, 0x11, 0xD1, 0x89, 0xC8, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "word ADC chain must be exact:\n{source}");
  assert!(
    source.contains("(AX + BX) < AX"),
    "the carry must come from the preceding add:\n{source}"
  );
  assert!(source.contains("CX + DX"), "ADC value must include the carry-in:\n{source}");
}

// sub ax,bx; sbb cx,dx; mov ax,cx; ret
// SBB lowers to `(CX - DX) - borrow(AX<BX)` exactly.
#[test]
fn sub_sbb_chain_is_exact() {
  let code = [0x29, 0xD8, 0x19, 0xD1, 0x89, 0xC8, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "word SBB chain must be exact:\n{source}");
  assert!(source.contains("(AX < BX)"), "the borrow must come from the preceding sub:\n{source}");
  assert!(source.contains("CX - DX"), "SBB value must include the borrow-in:\n{source}");
}

// stc; adc cx,dx; mov ax,cx; ret
// STC sets CF=1, so the ADC adds the literal carry.
#[test]
fn stc_then_adc_uses_known_carry() {
  let code = [0xF9, 0x11, 0xD1, 0x89, 0xC8, 0xC3];
  let source = decompile_raw(&code);

  // The bare `UNIMPL()` is STC's opaque-flags marker (unchanged); the ADC
  // itself must be exact.
  assert!(!source.replace("UNIMPL()", "").contains("UNIMPL("),
    "STC+ADC must have no value-position UNIMPL:\n{source}");
  assert!(source.contains("(CX + DX) + 1"), "STC must contribute carry 1:\n{source}");
}

// add al,bl; adc al,cl; ret
// Byte ADC lowers with the 8-bit carry read off a widened sum: the carry is
// `0xFF < (AL + BL)` (a+b promoted to int), so it is not the 16-bit carry.
#[test]
fn byte_adc_is_exact() {
  let code = [0x00, 0xD8, 0x10, 0xC8, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "byte ADC must be exact:\n{source}");
  assert!(source.contains("255 <"), "byte carry must use the 8-bit bound:\n{source}");
}

// add al,bl; adc al,cl; adc al,dl; ret
// The middle ADC's carry-out must feed the last ADC as bit 8 (`0xFF < sum`),
// not the bit-16 form that is always 0 for byte operands. Two distinct 8-bit
// carry comparisons must appear (the add's and the middle adc's).
#[test]
fn byte_adc_carry_chain_is_exact() {
  let code = [0x00, 0xD8, 0x10, 0xC8, 0x10, 0xD0, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "byte ADC chain must be exact:\n{source}");
  assert!(
    source.matches("255 <").count() >= 2,
    "the chained byte carry must be bit 8, not the always-0 bit-16 form:\n{source}"
  );
}

// sub al,bl; sbb al,cl; ret
// Byte SBB lowers with the width-independent borrow.
#[test]
fn byte_sbb_is_exact() {
  let code = [0x28, 0xD8, 0x18, 0xC8, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "byte SBB must be exact:\n{source}");
  assert!(source.contains("AL < BL"), "byte borrow must come from the sub:\n{source}");
}

// add ax,bx; jz +1; nop; adc cx,dx; mov ax,cx; ret
// The carry does not cross the control-flow merge: stay loud.
#[test]
fn cross_block_adc_stays_loud() {
  let code = [0x01, 0xD8, 0x74, 0x01, 0x90, 0x11, 0xD1, 0x89, 0xC8, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL("), "cross-block ADC must stay loud:\n{source}");
}

// add ax,bx; mov cx,dx; adc si,di; mov ax,si; ret
// A non-flag instruction between producer and consumer preserves the carry.
#[test]
fn non_flag_instruction_preserves_carry() {
  let code = [
    0x01, 0xD8,             // add ax,bx
    0x89, 0xD1,             // mov cx,dx
    0x11, 0xFE,             // adc si,di
    0x89, 0xF0,             // mov ax,si
    0xC3,                   // ret
  ];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "mov must preserve the tracked carry:\n{source}");
  assert!(source.contains("(AX + BX) < AX"), "the carry must still come from the add:\n{source}");
}

// add ax,bx; neg dx; sbb cx,dx; mov ax,cx; ret
// NEG writes CF, so the tracked carry from the ADD is stale: the SBB must
// stay loud rather than silently subtract the ADD's carry.
#[test]
fn neg_invalidates_tracked_carry() {
  let code = [
    0x01, 0xD8,             // add ax,bx
    0xF7, 0xDA,             // neg dx
    0x19, 0xD1,             // sbb cx,dx
    0x89, 0xC8,             // mov ax,cx
    0xC3,                   // ret
  ];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL("), "a stale carry after neg must stay loud:\n{source}");
  assert!(!source.contains("(AX + BX) < AX"), "must not reuse the pre-neg carry:\n{source}");
}
