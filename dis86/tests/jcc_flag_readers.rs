// Regression tests for issue #108: JO/JNO/JP/JNP were the last conditional
// jumps with no lowering arm, so they fell through to the loud-but-fatal
// UNIMPL() fallback. They are known operations whose only missing input is
// the OF/PF value, so they now lower through the non-fatal
// OverflowFlags/ParityFlags readers — an imprecise FLAGS input renders as
// UNIMPL_FLAGS (warn and return 0 at runtime) instead of aborting the function.
//
// The direct forms (JO/JP) read the flag; the negated forms (JNO/JNP) compose
// `Eq(reader, 0)` because no negated reader exists, the same way JNS composes
// `Eq(SignFlags, 0)` and the SETNO/SETNP forms do (tests/setcc_flag_readers.rs).
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

/// Every OF/PF conditional jump, as (encoding, mnemonic, negated?). The four
/// encodings are `<jcc> +1; nop; ret`, so both the taken and fallthrough paths
/// reach the return.
const OF_PF_JCC_CASES: &[(&[u8], &str, bool)] = &[
  (&[0x70, 0x01, 0x90, 0xC3], "jo", false),
  (&[0x71, 0x01, 0x90, 0xC3], "jno", true),
  (&[0x7A, 0x01, 0x90, 0xC3], "jp", false),
  (&[0x7B, 0x01, 0x90, 0xC3], "jnp", true),
];

// With the opaque entry FLAGS value, all four OF/PF jumps must emit the
// non-fatal flag marker and never the fatal operation marker (issue #108:
// JO/JNO/JP/JNP were the only conditional jumps that aborted).
#[test]
fn every_of_pf_jcc_is_non_fatal() {
  for &(code, name, _negated) in OF_PF_JCC_CASES {
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

// The direct/negated reader mapping (JO/JP read OF/PF directly; JNO/JNP compose
// `Eq(reader, 0)`) is pinned at the IR level in
// `ir_build::tests::of_pf_jumps_lower_to_non_fatal_flag_readers`, where block
// structuring cannot invert the condition.

// A jump after a real flag producer must still be non-fatal. This asserts only
// the marker *class* (not the value): OF/PF have no folding pass today, so the
// reader stays UNIMPL_FLAGS, but a future producer model is free to fold it
// away without violating this contract.
#[test]
fn jcc_after_a_flag_producer_stays_non_fatal() {
  for (code, name) in [
    (&[0x01, 0xD8, 0x70, 0x01, 0x90, 0xC3][..], "jo after add ax,bx"),
    (&[0x01, 0xD8, 0x7A, 0x01, 0x90, 0xC3][..], "jp after add ax,bx"),
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
