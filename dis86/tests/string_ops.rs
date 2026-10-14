// Regression tests for single-step string-op pointer auto-update
// (issue #69, slice 2). Previously MOVS/STOS/LODS/SCAS/CMPS moved the
// element but never advanced SI/DI, so a hand-written `movsb` loop
// decompiled to C that copies the same byte forever (silent-wrong).
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

// gen.rs renders both the abort-class value UNIMPL and the loud flags
// marker as `UNIMPL(...)`. A bare `UNIMPL()` statement is the marker; any
// surviving `UNIMPL(` after stripping it is a value-position abort.
fn has_value_unimpl(source: &str) -> bool {
  source.replace("UNIMPL()", "").contains("UNIMPL(")
}

// mov cx,4; L: movsb; dec cx; jnz L; ret
// The pointer update is modeled, so the emitted loop advances SI/DI and
// terminates instead of copying the same byte forever.
#[test]
fn movsb_loop_advances_pointers() {
  let code = [0xB9, 0x04, 0x00, 0xA4, 0x49, 0x75, 0xFC, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "no value-position UNIMPL:\n{source}");
  assert!(
    source.contains("si_2 + 1") && source.contains("di_2 + 1"),
    "both string pointers must advance by one per iteration:\n{source}"
  );
  assert!(
    source.contains("*PTR_8(ES, di_2) = *PTR_8(DS, si_2);"),
    "the byte move itself must remain:\n{source}"
  );
}

// movsw; mov ax,si; ret
// The 16-bit element advances the pointers by two, and the update is
// observable through a later SI read.
#[test]
fn movsw_advances_by_two_and_is_observable() {
  let code = [0xA5, 0x89, 0xF0, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "no value-position UNIMPL:\n{source}");
  assert!(
    source.contains("SI + 2"),
    "movsw must advance SI by the element size:\n{source}"
  );
}

// std; movsb; ret
// With a `std` in the function, DF is not provably clear, so the pointer
// update degrades to the loud marker instead of guessing the direction.
#[test]
fn std_makes_string_update_loud() {
  let code = [0xFD, 0xA4, 0xC3];
  let source = decompile_raw(&code);

  assert!(!has_value_unimpl(&source), "must stay a loud marker, not a value abort:\n{source}");
  assert!(
    source.contains("UNIMPL() /* UNIMPL op */;"),
    "the uncertain direction must be flagged loudly:\n{source}"
  );
  assert!(
    source.contains("*PTR_8(ES, DI) = *PTR_8(DS, SI);"),
    "the observed single step still happens:\n{source}"
  );
}
