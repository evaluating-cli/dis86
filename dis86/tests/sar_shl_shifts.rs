// Regression tests for the byte-SAR sign bug: `sar al,1` of 0xF0 must
// read AL as -16 (sign-extension restores bit 7 that the fragment mask
// dropped), not +240. Word operands, SHR, and the left shifts are
// unsigned and must keep their zero-extended shape.
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

// mov al,0xF0; sar al,1; ret - hardware: 0xF0 -> 0xF8 (CF=0). The old
// lowering emitted `(i16)240 >> 1` == 0x78, a silent sign loss.
#[test]
fn byte_sar_by_one_signs_the_source() {
  let source = decompile_raw(&[0xB0, 0xF0, 0xD0, 0xF8, 0xC3]);
  assert!(
    source.contains("(i16)(i8)240"),
    "byte SAR must sign-extend (not mask) its byte source:\n{source}"
  );
  assert!(
    source.contains("(i16)(i8)240 >> (i16)1"),
    "byte SAR must emit a signed shift:\n{source}"
  );
}

// sar ah,1 - the extension applies to fragments of registers too, not
// just immediates.
#[test]
fn byte_sar_of_register_signs_the_fragment() {
  let source = decompile_raw(&[0xD0, 0xFC, 0xC3]);
  assert!(
    source.contains("(i16)(i8)AH"),
    "SAR on a register fragment must sign-extend AH:\n{source}"
  );
}

// sar ax,1 - word operands keep the signed result shape from the older
// fix; the extension must not double-apply there.
#[test]
fn word_sar_stays_signed_without_extra_cast() {
  let source = decompile_raw(&[0xD1, 0xF8, 0xC3]);
  assert!(
    source.contains("(i16)AX >> (i16)1"),
    "word SAR must stay a signed 16-bit shift:\n{source}"
  );
  assert!(
    !source.contains("(i16)(i8)"),
    "word SAR must not sign-extend its 16-bit operand:\n{source}"
  );
}

// shr bl,1; mov al,bl; ret - 0xF0 -> 0x78: an unsigned shift must NOT
// sign-extend (the shift reads a zero-extended fragment).
#[test]
fn byte_shr_stays_unsigned() {
  let source = decompile_raw(&[0xD0, 0xEB, 0x88, 0xD8, 0xC3]);
  assert!(
    source.contains("(BL >> 1)") && !source.contains("(i8)"),
    "SHR must remain a zero-extended unsigned shift:\n{source}"
  );
}

// shl bl,1; mov al,bl; ret - the left shifts are direction-agnostic; no
// extension at all.
#[test]
fn byte_shl_is_untouched() {
  let source = decompile_raw(&[0xD0, 0xE3, 0x88, 0xD8, 0xC3]);
  assert!(
    source.contains("(BL << 1)") && !source.contains("(i8)"),
    "SHL must not gain an extension:\n{source}"
  );
}
