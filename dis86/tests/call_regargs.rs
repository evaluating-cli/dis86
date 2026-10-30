use dis86::asm::decode::Decoder;
use dis86::asm::instr::{Instr, Reg};
use dis86::config::{CallMode, Config, Func};
use dis86::decompile::ast::Function;
use dis86::decompile::control_flow::ControlFlow;
use dis86::decompile::gen::{self, Flavor};
use dis86::decompile::{ir, ir_build, opt, sym};
use dis86::segoff::{Off, Seg, SegOff};
use dis86::spec::Spec;
use dis86::types::{Type, TypeDatabase};

fn func(name: &str, start: u16, end: u16, regargs: Option<Vec<Reg>>) -> Func {
  Func {
    name: name.to_string(),
    start: SegOff { seg: Seg::Normal(0), off: Off(start) },
    end: Some(SegOff { seg: Seg::Normal(0), off: Off(end) }),
    entry: None,
    mode: CallMode::Near,
    ret: None,
    args: Some(0),
    regargs,
    regouts: None,
    clobbers: None,
    dont_pop_args: false,
  }
}

fn func_contract(name: &str, start: u16, end: u16, ret: Type,
                 regouts: Option<Vec<Reg>>, clobbers: Option<Vec<Reg>>) -> Func {
  Func {
    ret: Some(ret),
    regouts,
    clobbers,
    ..func(name, start, end, None)
  }
}

fn decompile_with_funcs(code: &[u8], main_end: u16, funcs: Vec<Func>) -> String {
  let types = std::rc::Rc::new(TypeDatabase::new());
  let cfg = Config {
    types,
    structs: vec![],
    code_segs: vec![],
    funcs,
    indirects: vec![],
    globals: vec![],
    text_section: vec![],
  };
  let binary = dis86::binary::Binary::from_raw(code, None);
  let spec = Spec::from_start_and_end(
    Some(SegOff { seg: Seg::Normal(0), off: Off(0) }),
    Some(SegOff { seg: Seg::Normal(0), off: Off(main_end) }),
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

fn find(source: &str, needle: &str) -> usize {
  source.find(needle).unwrap_or_else(|| panic!("missing {:?}:\n{source}", needle))
}

#[test]
fn call_regargs_preserve_full_register_copy_order() {
  let code = [
    0x89, 0xC3,       // mov bx,ax
    0xB4, 0x06,       // mov ah,6
    0xE8, 0x03, 0x00, // call f
    0x89, 0xD8,       // mov ax,bx
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x000a, None),
    func("f", 0x000a, 0x000b, Some(vec![Reg::AX, Reg::BX])),
  ];
  let source = decompile_with_funcs(&code, 0x000a, funcs);

  let copy = find(&source, "BX = AX");
  let recompose = find(&source, "AX = (6 << 8) | AL");
  assert!(copy < recompose, "BX copy must precede the AH recompose:\n{source}");
}

#[test]
fn call_regargs_order_is_independent_of_annotation_order() {
  let code = [
    0x89, 0xC3,       // mov bx,ax
    0xB4, 0x06,       // mov ah,6
    0xE8, 0x03, 0x00, // call f
    0x89, 0xD8,       // mov ax,bx
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x000a, None),
    func("f", 0x000a, 0x000b, Some(vec![Reg::BX, Reg::AX])),
  ];
  let source = decompile_with_funcs(&code, 0x000a, funcs);

  let copy = find(&source, "BX = AX");
  let recompose = find(&source, "AX = (6 << 8) | AL");
  assert!(
    copy < recompose,
    "BX copy must follow program order, not the annotation order:\n{source}"
  );
}

#[test]
fn call_regargs_preserve_byte_fragment_aliasing() {
  let code = [
    0x88, 0xC1,       // mov cl,al
    0xB8, 0x34, 0x12, // mov ax,0x1234
    0xE8, 0x02, 0x00, // call f
    0xC3,             // ret
    0x90,             // pad
    0xC3,             // f: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x000a, None),
    func("f", 0x000a, 0x000b, Some(vec![Reg::AX, Reg::CX])),
  ];
  let source = decompile_with_funcs(&code, 0x000a, funcs);

  let copy = find(&source, "CX = ");
  let overwrite = find(&source, "AX = 0x1234");
  assert!(copy < overwrite, "CX must capture the old AL before AX is overwritten:\n{source}");
}

#[test]
fn call_regargs_preserve_fragment_copy_order() {
  let code = [
    0x89, 0xC3,       // mov bx,ax
    0xB0, 0x07,       // mov al,7
    0xE8, 0x01, 0x00, // call f
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x0008, None),
    func("f", 0x0008, 0x0009, Some(vec![Reg::AL, Reg::BX])),
  ];
  let source = decompile_with_funcs(&code, 0x0008, funcs);

  let copy = find(&source, "BX = AX");
  let parent_write = find(&source, "AX = ");
  assert!(copy < parent_write, "BX copy must precede the AX parent write:\n{source}");
}

#[test]
fn call_regargs_skip_unchanged_registers() {
  let code = [
    0x89, 0xC3,       // mov bx,ax
    0xB4, 0x06,       // mov ah,6
    0xE8, 0x03, 0x00, // call f
    0x89, 0xD8,       // mov ax,bx
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x000a, None),
    func("f", 0x000a, 0x000b, Some(vec![Reg::AX, Reg::BX, Reg::CX])),
  ];
  let source = decompile_with_funcs(&code, 0x000a, funcs);

  assert!(!source.contains("CX = CX"), "unchanged CX must not be pinned:\n{source}");
}

#[test]
fn call_regargs_dedupe_repeated_registers() {
  let code = [
    0xB8, 0x34, 0x12, // mov ax,0x1234
    0xE8, 0x01, 0x00, // call f
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x0007, None),
    func("f", 0x0007, 0x0008, Some(vec![Reg::AX, Reg::AX])),
  ];
  let source = decompile_with_funcs(&code, 0x0007, funcs);

  assert_eq!(
    source.matches("AX = 0x1234").count(),
    1,
    "repeated regarg must emit exactly one write:\n{source}"
  );
}

#[test]
fn call_contract_refreshes_declared_clobbers() {
  let code = [
    0xBB, 0x11, 0x11, // mov bx,0x1111
    0xE8, 0x03, 0x00, // call f
    0x89, 0xD8,       // mov ax,bx
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let funcs = vec![
    func_contract("main", 0x0000, 0x0009, Type::U16, None, None),
    func_contract("f", 0x0009, 0x000a, Type::U16, None, Some(vec![Reg::BX])),
  ];
  let source = decompile_with_funcs(&code, 0x0009, funcs);

  let call = find(&source, "f(");
  assert!(source[call..].contains("BX"), "post-call BX must be live:\n{source}");
  assert!(!source[call..].contains("0x1111"), "post-call BX must not be the pre-call constant:\n{source}");
}

#[test]
fn call_contract_refreshes_declared_regouts() {
  let code = [
    0x8E, 0xDB,       // mov ds,bx
    0xE8, 0x03, 0x00, // call f
    0x8C, 0xD8,       // mov ax,ds
    0xC3,             // ret
    0xC3,             // f: ret
  ];
  let funcs = vec![
    func_contract("main", 0x0000, 0x0008, Type::U16, None, None),
    func_contract("f", 0x0008, 0x0009, Type::U16, Some(vec![Reg::DS]), None),
  ];
  let source = decompile_with_funcs(&code, 0x0008, funcs);

  let call = find(&source, "f(");
  assert!(source[call..].contains("DS"), "post-call DS must be live:\n{source}");
  assert!(!source[call..].contains("BX"), "post-call DS must not be the pre-call alias:\n{source}");
}
