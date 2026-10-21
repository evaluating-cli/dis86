// Regression tests for issue #85: decoded-but-unlowered opcodes must not
// panic and abort the whole decompile. Unmodeled instructions degrade to
// loud markers (fatal UNIMPL only if executed, non-fatal UNIMPL_FLAGS for
// flag imprecision); commonly-used ones are lowered exactly.
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

// Every opcode from the #85 table (plus POPF, which was also unlowered but
// unlisted) must decompile without panicking.
#[test]
fn unlowered_opcodes_do_not_panic() {
  let cases: &[(&[u8], &str)] = &[
    (&[0xD7, 0xC3], "xlat"),
    (&[0x9F, 0xC3], "lahf"),
    (&[0x9E, 0xC3], "sahf"),
    (&[0x9C, 0xC3], "pushf"),
    (&[0x9D, 0xC3], "popf"),
    (&[0xF7, 0xF9, 0xC3], "idiv cx"),
    (&[0xF6, 0xFB, 0xC3], "idiv bl"),
    (&[0xC8, 0x00, 0x00, 0x00, 0xC3], "enter 0,0"),
    (&[0x37, 0xC3], "aaa"),
    (&[0x3F, 0xC3], "aas"),
    (&[0x27, 0xC3], "daa"),
    (&[0x2F, 0xC3], "das"),
    (&[0xF4, 0xC3], "hlt"),
    (&[0xCE, 0x90, 0xC3], "into"),
    (&[0x0F, 0x90, 0xC0, 0xC3], "seto"),
    (&[0x0F, 0x91, 0xC0, 0xC3], "setno"),
    (&[0x0F, 0x98, 0xC0, 0xC3], "sets"),
    (&[0x0F, 0x99, 0xC0, 0xC3], "setns"),
    (&[0x0F, 0x9A, 0xC0, 0xC3], "setp"),
    (&[0x0F, 0x9B, 0xC0, 0xC3], "setnp"),
  ];
  for (code, name) in cases {
    let source = decompile_raw(code);
    assert!(
      source.contains("NEAR") || source.contains("return"),
      "{name} must decompile to a function instead of panicking:\n{source}"
    );
  }
}

// xlat is exact: AL = DS:[BX + AL].
#[test]
fn xlat_lowers_to_indexed_byte_load() {
  let source = decompile_raw(&[0xD7, 0xC3]);
  assert!(
    source.contains("PTR_8(DS, BX + AL)"),
    "xlat must emit an indexed byte load:\n{source}"
  );
}

// idiv mirrors div with a signed division and divides the full 32-bit
// dividend without truncating it, sign-extends the 16-bit divisor
// (0x8001 is -32767, not +32769), and produces the exact remainder
// (truncation toward zero, `IDIV`'s rule) in DX rather than an unknown.
#[test]
fn idiv_lowers_to_signed_division() {
  let source = decompile_raw(&[0xF7, 0xF9, 0xC3]);
  assert!(
    source.contains("MAKE_32") && source.contains("/ (i32)") && source.contains("% (i32)"),
    "idiv cx must emit a signed DX:AX division and its exact remainder:\n{source}"
  );
  assert!(
    source.contains("(i32)(i16)CX"),
    "the 16-bit divisor must be sign-extended, not zero-extended:\n{source}"
  );
  assert!(
    !source.contains("UNIMPL("),
    "no unmodeled remainder marker may remain:\n{source}"
  );
}

// Byte idiv divides the 16-bit AX (AH:AL), not a 32-bit Make32; its
// divisor is a signed byte (sign-extended, not zero-extended); and the
// remainder is exact: AH holds the low byte of the signed remainder.
#[test]
fn byte_idiv_divides_ax_with_exact_remainder() {
  let source = decompile_raw(&[0xF6, 0xFB, 0xC3]);
  assert!(
    source.contains("(AH << 8) | AL"),
    "byte idiv must divide the 16-bit AX (AH:AL):\n{source}"
  );
  assert!(
    source.contains("(i16)(i8)BL"),
    "the byte divisor must be sign-extended, not zero-extended:\n{source}"
  );
  assert!(
    source.contains("% (i16)"),
    "the remainder must be computed exactly:\n{source}"
  );
  // Pin the half-register split: AH gets the remainder and AL the
  // quotient (the quotient term is the `& 255` one, the remainder feeds
  // the `<< 8` half). A swap would still contain `%`, so check the order.
  let rem = source.find("% (i16)").expect("remainder term");
  let sh = source.find(">> 8").expect("AH split");
  let quo = source.find("/ (i16)").expect("quotient term");
  assert!(
    rem < sh && sh < quo,
    "AH must take the remainder and AL the quotient:\n{source}"
  );
  assert!(
    !source.contains("UNIMPL("),
    "no unmodeled remainder marker may remain:\n{source}"
  );
}

// Word IMUL is signed over its 16-bit operands, so a masked 0xFFFF is -1:
// the U32 product must be built from sign-extended operands (casting a
// zero-extended 65535 to i32 would give DX:AX = 0001:FFFE for -1 * 2).
#[test]
fn word_imul_signs_both_operands() {
  let source = decompile_raw(&[0xF7, 0xE9, 0xC3]);
  assert!(
    source.contains("(i32)(i16)AX") && source.contains("(i32)(i16)CX"),
    "word imul must sign-extend both operands before the multiply:\n{source}"
  );
  assert!(
    source.contains("MAKE_32"),
    "word imul must split the product over DX:AX:\n{source}"
  );
}

// Word MUL is unsigned: the operands must NOT be sign-extended.
#[test]
fn word_mul_stays_unsigned() {
  let source = decompile_raw(&[0xF7, 0xE1, 0xC3]);
  assert!(
    source.contains("AX * CX") && !source.contains("(i16)"),
    "word mul must multiply the zero-extended registers:\n{source}"
  );
}

// Byte MUL/IMUL were an unmodeled form: the old word-only assert rejected
// the AX/AL operand pair and panicked the build. The byte product is
// `AL * r/m8`, a signed product for IMUL, all 16 bits of AX.
#[test]
fn byte_mul_and_imul_lower() {
  let mul = decompile_raw(&[0xF6, 0xE3, 0xC3]);
  assert!(
    mul.contains("AL * BL"),
    "byte mul must emit the unsigned AL * BL product:\n{mul}"
  );
  let imul = decompile_raw(&[0xF6, 0xEB, 0xC3]);
  assert!(
    imul.contains("(i8)AL") && imul.contains("(i8)BL"),
    "byte imul must sign-extend both operands:\n{imul}"
  );
}

// div clobbers flags (architecturally undefined): a branch after div must
// not fold against a pre-division flag producer.
#[test]
fn div_clobbers_flags_for_following_branch() {
  let code = [
    0x2B, 0xC0, // sub ax,ax
    0xF7, 0xF1, // div cx
    0x74, 0x01, // jz +1
    0x90,       // nop
    0xC3,       // ret
  ];
  let source = decompile_raw(&code);
  assert!(
    !source.contains("AX == AX"),
    "post-DIV flags must not fold to the pre-DIV `sub ax,ax` result:\n{source}"
  );
  assert!(
    source.contains("UNIMPL_FLAGS"),
    "post-DIV flags must degrade to the loud UNIMPL_FLAGS marker:\n{source}"
  );
}

// aaa produces distinct unknowns per fragment so al/ah never compare equal.
#[test]
fn aaa_writes_distinct_unknowns_per_fragment() {
  let source = decompile_raw(&[0x37, 0xC3]);
  assert_eq!(
    source.matches("UNIMPL(AL, AH)").count(),
    2,
    "aaa must write a distinct unknown to AL and AH:\n{source}"
  );
}

// The SETcc (issue #103) and OF/PF Jcc (issue #108) marker contracts live in
// tests/setcc_flag_readers.rs and tests/jcc_flag_readers.rs: they read a
// possibly-unknown flag non-fatally.

// lahf stores the opaque flags to AH; hlt is a fatal-only-if-executed marker.
#[test]
fn lahf_preserves_flags_hlt_stays_loud() {
  let source = decompile_raw(&[0x9F, 0xC3]);
  assert!(
    source.contains("FLAGS"),
    "lahf must materialize the flags value into AH:\n{source}"
  );
  let hlt = decompile_raw(&[0xF4, 0xC3]);
  assert!(
    hlt.contains("UNIMPL()"),
    "hlt must stay a loud marker instead of panicking the build:\n{hlt}"
  );
}
