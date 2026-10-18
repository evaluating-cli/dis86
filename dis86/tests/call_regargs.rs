// Regression tests for call-site register-argument materialization
// (issues #78/#101). The pinned guest writes that carry a callee's `regargs`
// must be emitted in program order, exactly like `process_int`'s writes:
// otherwise a copy recorded as `Ref::Init(src)` reads the value its source
// was given *after* the copy (`mov bx,ax; mov ah,6; call f` must pass the
// pre-recompose AX in BX).
use dis86::asm::decode::Decoder;
use dis86::asm::instr::{Instr, Reg};
use dis86::config::{self, Config, Func};
use dis86::decompile::ast::Function;
use dis86::decompile::control_flow::ControlFlow;
use dis86::decompile::gen::{self, Flavor};
use dis86::decompile::{ir, ir_build, opt, sym};
use dis86::segoff::{Off, Seg, SegOff};
use dis86::spec::Spec;
use dis86::types::TypeDatabase;
use std::rc::Rc;

fn seg_off(off: u16) -> SegOff {
  SegOff { seg: Seg::Normal(0), off: Off(off) }
}

fn func(name: &str, start: u16, end: u16, regargs: Option<Vec<Reg>>) -> Func {
  Func {
    name: name.to_string(),
    start: seg_off(start),
    end: Some(seg_off(end)),
    entry: None,
    mode: config::CallMode::Near,
    ret: None,
    args: None,
    regargs,
    dont_pop_args: false,
  }
}

// Runs the pipeline app.rs uses (up to and including C generation) for the
// function named `test`, with a near callee `f` at `callee_at` whose regargs
// are annotated as given.
fn decompile_call_site(code: &[u8], callee_at: u16, regargs: Vec<Reg>) -> String {
  let types = Rc::new(TypeDatabase::new());
  let cfg = Config {
    types,
    structs: vec![],
    code_segs: vec![],
    funcs: vec![
      func("test", 0, callee_at, None),
      func("f", callee_at, callee_at + 1, Some(regargs)),
    ],
    indirects: vec![],
    globals: vec![],
    text_section: vec![],
  };
  let binary = dis86::binary::Binary::from_raw(code, None);
  let spec = Spec::from_config_name(&cfg, "test").expect("spec for 'test'");

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
  let ast = Function::from_ir(&cfg, "test", None, &ir, &cf).expect("ast");
  gen::generate(&ast, Flavor::Standard).expect("codegen")
}

fn line_index(out: &str, needle: &str) -> usize {
  out.lines()
    .position(|l| l.trim_start().starts_with(needle))
    .unwrap_or_else(|| panic!("expected a statement starting with {:?} in:\n{}", needle, out))
}

// mov bx,ax; mov ah,6; call f; mov ax,bx; ret   (f at 0000:000a, regargs AX,BX)
#[test]
fn call_site_copy_observes_program_order_value() {
  let code = [
    0x89, 0xC3,       // mov bx,ax
    0xB4, 0x06,       // mov ah,6
    0xE8, 0x03, 0x00, // call f (0000:000a)
    0x89, 0xD8,       // mov ax,bx
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let out = decompile_call_site(&code, 0x0a, vec![Reg::AX, Reg::BX]);
  let bx = line_index(&out, "BX = ");
  let ax = line_index(&out, "AX = ");
  assert!(
    bx < ax,
    "BX must capture AX before the AH=6 recompose is materialized:\n{}",
    out
  );
}

// The annotation order must not decide the emitted order: with regargs
// "BX,AX" the correct order happens to be the annotation order, so this
// guards against a fix (or a regression) that just sorts by annotation.
#[test]
fn call_site_order_is_independent_of_annotation_order() {
  let code = [
    0x89, 0xC3,       // mov bx,ax
    0xB4, 0x06,       // mov ah,6
    0xE8, 0x03, 0x00, // call f (0000:000a)
    0x89, 0xD8,       // mov ax,bx
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let out = decompile_call_site(&code, 0x0a, vec![Reg::BX, Reg::AX]);
  let bx = line_index(&out, "BX = ");
  let ax = line_index(&out, "AX = ");
  assert!(
    bx < ax,
    "BX must capture AX before the AH=6 recompose is materialized:\n{}",
    out
  );
}

// Byte-fragment aliasing: `mov cl,al` records CL as a live read of AX's
// storage, so CX must be materialized before AX is overwritten.
// mov cl,al; mov ax,0x1234; call f; ret   (f at 0000:000a, regargs AX,CX)
#[test]
fn call_site_copy_observes_byte_fragment_aliasing() {
  let code = [
    0x88, 0xC1,       // mov cl,al
    0xB8, 0x34, 0x12, // mov ax,0x1234
    0xE8, 0x02, 0x00, // call f (0000:000a)
    0xC3,             // ret
    0x90,             // pad
    0xC3,             // f: ret
  ];
  let out = decompile_call_site(&code, 0x0a, vec![Reg::AX, Reg::CX]);
  let cx = line_index(&out, "CX = ");
  let ax = line_index(&out, "AX = 0x1234");
  assert!(
    cx < ax,
    "CX must capture the old AL before AX is overwritten:\n{}",
    out
  );
}

// A regarg still holding its entry value is not written at all: a pinned
// `BX = BX` would be a no-op guest write.
#[test]
fn call_site_skips_unwritten_regargs() {
  let code = [
    0xB8, 0x01, 0x00, // mov ax,1
    0xE8, 0x01, 0x00, // call f (0000:0007)
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let out = decompile_call_site(&code, 0x07, vec![Reg::AX, Reg::BX]);
  assert!(out.contains("AX = 1"), "AX is written, so it must be materialized:\n{}", out);
  assert!(!out.contains("BX = BX"), "BX still holds its entry value; writing it is a no-op:\n{}", out);
}
