// Regression tests for direction/interrupt flag control (issue #69). STD/
// CLD/STI/CLI change DF/IF only, never the arithmetic flags, so they lower
// to non-fatal side-effecting macros (CLD()/STD()/CLI()/STI()) and leave the
// modeled FLAGS value (and thus a following ADC/SBB) intact.
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

// cld; ret
#[test]
fn cld_is_non_fatal() {
  let source = decompile_raw(&[0xFC, 0xC3]);
  assert!(!source.contains("UNIMPL"), "CLD must not abort:\n{source}");
  assert!(source.contains("CLD()"), "CLD must render as the macro:\n{source}");
}

// std; ret
#[test]
fn std_is_non_fatal() {
  let source = decompile_raw(&[0xFD, 0xC3]);
  assert!(!source.contains("UNIMPL"), "STD must not abort:\n{source}");
  assert!(source.contains("STD()"), "STD must render as the macro:\n{source}");
}

// cli; sti; ret
#[test]
fn cli_sti_are_non_fatal() {
  let source = decompile_raw(&[0xFA, 0xFB, 0xC3]);
  assert!(!source.contains("UNIMPL"), "CLI/STI must not abort:\n{source}");
  assert!(source.contains("CLI()"), "CLI must render as the macro:\n{source}");
  assert!(source.contains("STI()"), "STI must render as the macro:\n{source}");
}

// stc; std; adc cx,dx; mov ax,cx; ret
// STD only touches DF, so the tracked CF=1 from STC still reaches the ADC.
#[test]
fn std_preserves_arithmetic_flags() {
  let code = [0xF9, 0xFD, 0x11, 0xD1, 0x89, 0xC8, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("STD()"), "STD must appear:\n{source}");
  assert!(!source.contains("UNIMPL"), "STD must not clobber arithmetic flags:\n{source}");
  assert!(source.contains("(CX + DX) + 1"), "the STC carry must survive STD:\n{source}");
}

// cld; rep movsb; ret
// CLD clears DF and is not even tracked by the df_may_be_set guard, so the
// REP form still models (the guard only trips on STD/POPF/IRET).
#[test]
fn cld_lets_rep_movsb_model() {
  let code = [0xFC, 0xF3, 0xA4, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("CLD()"), "CLD must appear:\n{source}");
  assert!(source.contains("REP_MOVSB"), "CLD must not force REP loud:\n{source}");
  assert!(!source.contains("UNIMPL"), "CLD+rep must be modeled:\n{source}");
}

// std; rep movsb; ret
// STD may leave DF set, so the REP form stays loud (unchanged fail-closed).
#[test]
fn std_keeps_rep_movsb_loud() {
  let code = [0xFD, 0xF3, 0xA4, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("STD()"), "STD must appear:\n{source}");
  assert!(source.contains("UNIMPL("), "STD+rep must stay loud:\n{source}");
}
