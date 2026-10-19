// Regression tests for issue #103: SETO/SETNO/SETP/SETNP are known
// operations whose only missing input is the OF/PF value. They lower through
// the non-fatal OverflowFlags/ParityFlags readers, so an imprecise FLAGS
// input renders as UNIMPL_FLAGS (warn and return 0 at runtime) instead of
// the fatal UNIMPL() abort. The other twelve SETcc already used that marker
// class; these four were the only ones that aborted.
//
// The direct forms (SETS/SETO/SETP) read the flag; the negated forms
// (SETNS/SETNO/SETNP) compose `Eq(reader, 0)` because no negated reader
// exists, the same way JNS composes `Eq(SignFlags, 0)`.
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

/// Every SETcc variant, as (encoding, mnemonic, negated?). `seto al` is
/// 0F 90 C0; the family is contiguous (0F 90..0F 9F), so the negation bit is
/// the low bit of the second opcode byte.
const SETCC_CASES: &[(&[u8], &str, bool)] = &[
  (&[0x0F, 0x90, 0xC0, 0xC3], "seto", false),
  (&[0x0F, 0x91, 0xC0, 0xC3], "setno", true),
  (&[0x0F, 0x92, 0xC0, 0xC3], "setb", false),
  (&[0x0F, 0x93, 0xC0, 0xC3], "setae", true),
  (&[0x0F, 0x94, 0xC0, 0xC3], "sete", false),
  (&[0x0F, 0x95, 0xC0, 0xC3], "setne", true),
  (&[0x0F, 0x96, 0xC0, 0xC3], "setbe", false),
  (&[0x0F, 0x97, 0xC0, 0xC3], "seta", true),
  (&[0x0F, 0x98, 0xC0, 0xC3], "sets", false),
  (&[0x0F, 0x99, 0xC0, 0xC3], "setns", true),
  (&[0x0F, 0x9A, 0xC0, 0xC3], "setp", false),
  (&[0x0F, 0x9B, 0xC0, 0xC3], "setnp", true),
  (&[0x0F, 0x9C, 0xC0, 0xC3], "setl", false),
  (&[0x0F, 0x9D, 0xC0, 0xC3], "setge", true),
  (&[0x0F, 0x9E, 0xC0, 0xC3], "setle", false),
  (&[0x0F, 0x9F, 0xC0, 0xC3], "setg", true),
];

// With the opaque entry FLAGS value, all sixteen SETcc variants must emit the
// non-fatal flag marker and never the fatal operation marker (issue #103:
// SETO/SETNO/SETP/SETNP were the only four that aborted).
#[test]
fn every_setcc_is_non_fatal() {
  for &(code, name, _negated) in SETCC_CASES {
    let source = decompile_raw(code);
    assert!(
      source.contains("UNIMPL_FLAGS"),
      "{name} must read its (possibly unknown) flag non-fatally:\n{source}"
    );
    assert!(
      !source.contains("UNIMPL("),
      "{name} must not abort with a fatal UNIMPL marker:\n{source}"
    );
  }
}

// Overflow and parity now have reader opcodes, so they render like the
// sign-flag family: the direct forms read the flag, the negated forms
// compose `Eq(flag, 0)`.
#[test]
fn overflow_and_parity_render_like_the_other_flag_readers() {
  for (code, name, direct) in [
    (&[0x0F, 0x90, 0xC0, 0xC3][..], "seto", true),
    (&[0x0F, 0x91, 0xC0, 0xC3][..], "setno", false),
    (&[0x0F, 0x9A, 0xC0, 0xC3][..], "setp", true),
    (&[0x0F, 0x9B, 0xC0, 0xC3][..], "setnp", false),
  ] {
    let source = decompile_raw(code);
    assert!(
      source.contains("UNIMPL_FLAGS(FLAGS)"),
      "{name} must read the opaque FLAGS value:\n{source}"
    );
    if direct {
      assert!(
        !source.contains("== 0"),
        "{name} must read the flag directly, not compose a negation:\n{source}"
      );
    } else {
      assert!(
        source.contains("== 0"),
        "{name} must compose Eq(flag, 0):\n{source}"
      );
    }
  }
}

// A SETcc after a real flag producer must still be non-fatal. This asserts
// only the marker *class* (not the value): OF/PF have no folding pass today,
// so the reader stays UNIMPL_FLAGS, but a future producer model is free to
// fold it away without violating this contract.
#[test]
fn setcc_after_a_flag_producer_stays_non_fatal() {
  for (code, name) in [
    (&[0x01, 0xD8, 0x0F, 0x90, 0xC0, 0xC3][..], "seto after add ax,bx"),
    (&[0x01, 0xD8, 0x0F, 0x9A, 0xC0, 0xC3][..], "setp after add ax,bx"),
  ] {
    let source = decompile_raw(code);
    assert!(
      source.contains("UNIMPL_FLAGS"),
      "{name} must keep the non-fatal flag-reader marker:\n{source}"
    );
    assert!(
      !source.contains("UNIMPL("),
      "{name} must not abort with a fatal UNIMPL marker:\n{source}"
    );
  }
}

// The reader feeds the same destination path as the other SETcc forms,
// including a memory destination. Cover direct and negated OF/PF forms
// (`seto/setno/setp/setnp byte [bx]`), not just the direct `seto`.
#[test]
fn setcc_to_memory_is_non_fatal() {
  for (code, name) in [
    (&[0x0F, 0x90, 0x07, 0xC3][..], "seto byte [bx]"),
    (&[0x0F, 0x91, 0x07, 0xC3][..], "setno byte [bx]"),
    (&[0x0F, 0x9A, 0x07, 0xC3][..], "setp byte [bx]"),
    (&[0x0F, 0x9B, 0x07, 0xC3][..], "setnp byte [bx]"),
  ] {
    let source = decompile_raw(code);
    assert!(
      source.contains("PTR_8(DS, BX)"),
      "{name} must store through the guest pointer:\n{source}"
    );
    assert!(
      source.contains("UNIMPL_FLAGS"),
      "{name} must use the non-fatal flag marker:\n{source}"
    );
    assert!(
      !source.contains("UNIMPL("),
      "{name} must not abort:\n{source}"
    );
  }
}
