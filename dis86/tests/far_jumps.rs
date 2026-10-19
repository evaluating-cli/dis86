// Regression tests for issue #112: a branch whose target is not a plain
// relative offset (a far jump, or an indirect jump with no resolvable
// jump table) used to panic in `ir_build::jump_targets`, which lost the
// whole function: the panic happened during block inference, before the
// loud-but-non-fatal UNIMPL fallback of issue #85 could run.
//
// A far jump inside the function's own code segment does not change CS,
// so it is a plain jump to that address. Every other transfer (a far
// jump into another segment, or a jump whose target is only known at run
// time) cannot be expressed in the IR: the block ends there with a loud
// marker instead of a fallthrough edge that never happens.
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

// `jmp far 0000:0009` (EA 09 00 00 00), a decoy `mov ax,1; ret` on the
// fallthrough path, and `mov ax,2; ret` at the jump target.
//
// CS does not change, so the far jump is a plain jump to offset 9: the
// taken arm must survive, and the fallthrough decoy must be dropped as
// unreachable (not silently kept as a second path).
#[test]
fn same_segment_far_jump_takes_the_target_arm() {
  let code = [0xEA, 0x09, 0x00, 0x00, 0x00, 0xB8, 0x01, 0x00, 0xC3, 0xB8, 0x02, 0x00, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "a same-segment far jump is modellable:\n{source}");
  assert!(source.contains("MAKE_32(DX, 2)"), "the taken arm must survive:\n{source}");
  assert!(!source.contains("MAKE_32(DX, 1)"), "the fallthrough decoy is unreachable:\n{source}");
}

// `jmp far 1234:0100` (EA 00 01 34 12): the target is in another
// segment, which the IR cannot express (hydra has no far-jump
// primitive). The transfer must be loud and must not invent a
// fallthrough edge to the code after it.
#[test]
fn cross_segment_far_jump_is_loud_and_drops_the_fallthrough() {
  let code = [0xEA, 0x00, 0x01, 0x34, 0x12, 0xB8, 0x01, 0x00, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL()"), "an unmodellable transfer must be loud:\n{source}");
  assert!(!source.contains("MAKE_32(DX, 1)"), "the fallthrough must not be emitted:\n{source}");
}

// `jmp far [0x1234]` (FF 2E 34 12): the far target is read from memory
// at run time, so there is no address to jump to.
#[test]
fn indirect_far_jump_is_loud() {
  let code = [0xFF, 0x2E, 0x34, 0x12, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL()"), "an unmodellable transfer must be loud:\n{source}");
}

// `jmp [bx]` (FF 27) and `jmp ax` (FF E0): the near target is only known
// at run time. These panicked at the same place as the far jumps.
#[test]
fn indirect_near_jump_is_loud() {
  for code in [[0xFF, 0x27, 0xC3], [0xFF, 0xE0, 0xC3]] {
    let source = decompile_raw(&code);
    assert!(source.contains("UNIMPL()"), "an unmodellable transfer must be loud:\n{source}");
  }
}

// `jmp cs:[bx+0x6d7]` (2E FF A7 D7 06) is the shape of the
// config-described jump table. Without the table in the config the
// target is unknown; that used to abort the whole function.
#[test]
fn jump_table_without_a_config_region_is_loud() {
  let code = [0x2E, 0xFF, 0xA7, 0xD7, 0x06, 0xC3];
  let source = decompile_raw(&code);
  assert!(source.contains("UNIMPL()"), "an unmodellable transfer must be loud:\n{source}");
}

// `jmp far 0000:0100` (EA 00 01 00 00) with a target outside the
// function's range: the same treatment a near jump out of range gets
// (the edge is dropped with a warning), so the function still
// decompiles.
#[test]
fn far_jump_outside_the_function_range_is_not_fatal() {
  let code = [0xEA, 0x00, 0x01, 0x00, 0x00, 0xC3];
  let source = decompile_raw(&code);

  assert!(!source.contains("UNIMPL"), "an out-of-range target is dropped, not unmodelled:\n{source}");
  assert!(!source.contains("MAKE_32(DX, 1)"), "the function must end at the jump:\n{source}");
}

// The transfer must not cost the function the code before it. A store is
// used because it is observable: a value that only feeds the (never
// reached) return would be dead-code-eliminated.
#[test]
fn far_jump_keeps_the_code_before_it() {
  let code = [0xA3, 0x34, 0x12, 0xEA, 0x00, 0x01, 0x34, 0x12, 0xC3];
  let source = decompile_raw(&code);

  assert!(source.contains("UNIMPL()"), "an unmodellable transfer must be loud:\n{source}");
  assert!(source.contains("PTR_16(DS, 0x1234) = AX"), "the code before the jump must survive:\n{source}");
}
