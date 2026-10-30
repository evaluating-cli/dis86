use dis86::asm::decode::Decoder;
use dis86::asm::instr::Instr;
use dis86::config::{CallMode, Config, Func, Indirect};
use dis86::decompile::ast::Function;
use dis86::decompile::control_flow::ControlFlow;
use dis86::decompile::gen::{self, Flavor};
use dis86::decompile::{fuse, ir, ir_build, opt, sym};
use dis86::segoff::{Off, Seg, SegOff};
use dis86::spec::Spec;
use dis86::types::{Type, TypeDatabase};

fn func(start: u16, end: u16) -> Func {
  Func {
    name: "main".to_string(),
    start: SegOff { seg: Seg::Normal(0), off: Off(start) },
    end: Some(SegOff { seg: Seg::Normal(0), off: Off(end) }),
    entry: None,
    mode: CallMode::Near,
    ret: None,
    args: Some(0),
    regargs: None,
    dont_pop_args: false,
  }
}

fn decompile(code: &[u8], end: u16, indirects: Vec<Indirect>, flavor: Flavor) -> String {
  let types = std::rc::Rc::new(TypeDatabase::new());
  let cfg = Config {
    types,
    structs: vec![],
    code_segs: vec![],
    funcs: vec![func(0, end)],
    indirects,
    globals: vec![],
    text_section: vec![],
  };
  let binary = dis86::binary::Binary::from_raw(code, None);
  let spec = Spec::from_config_name(&cfg, "main").expect("main function");

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
  fuse::fuse_mem(&mut ir);
  opt::optimize(&mut ir);
  ir::fin::finalize(&mut ir);

  let cf = ControlFlow::from_ir(&ir);
  let ast = Function::from_ir(&cfg, "main", None, &ir, &cf).expect("ast");
  gen::generate(&ast, flavor).expect("codegen")
}

fn standard(code: &[u8], end: u16, indirects: Vec<Indirect>) -> String {
  decompile(code, end, indirects, Flavor::Standard)
}

#[test]
fn register_indirect_call_uses_the_near_macro() {
  let source = standard(&[0xFF, 0xD1, 0xC3], 3, vec![]);
  assert!(source.contains("CALL_NEAR_INDIRECT(CX)"), "source:\n{source}");
  assert!(!source.contains("CALL_FAR_INDIRECT"), "source:\n{source}");
}

#[test]
fn memory_indirect_call_keeps_the_16_bit_operand() {
  let source = standard(&[0xFF, 0x57, 0x02, 0xC3], 4, vec![]);
  assert!(source.contains("CALL_NEAR_INDIRECT("), "source:\n{source}");
  assert!(source.contains("PTR_16"), "source:\n{source}");
  assert!(!source.contains("PTR_32"), "source:\n{source}");
  assert!(!source.contains("CALL_FAR_INDIRECT"), "source:\n{source}");
}

#[test]
fn memory_far_indirect_call_keeps_the_32_bit_operand() {
  let source = standard(&[0xFF, 0x5F, 0x02, 0xC3], 4, vec![]);
  assert!(source.contains("CALL_FAR_INDIRECT("), "source:\n{source}");
  assert!(source.contains("PTR_32"), "source:\n{source}");
  assert!(!source.contains("PTR_16"), "source:\n{source}");
  assert!(!source.contains("CALL_NEAR_INDIRECT"), "source:\n{source}");
}

#[test]
fn configured_far_indirect_return_survives_the_split() {
  let source = standard(
    &[0xFF, 0x5F, 0x02, 0xC3],
    4,
    vec![Indirect {
      addr: SegOff { seg: Seg::Normal(0), off: Off(0) },
      ret: Type::U32,
      args: 0,
    }],
  );
  assert!(source.contains("CALL_FAR_INDIRECT("), "source:\n{source}");
  assert!(source.contains("u32 tmp_"), "U32 return should use a 32-bit call result:\n{source}");
  assert!(source.contains("MAKE_32("), "U32 return should split the call result:\n{source}");
}

#[test]
fn both_indirect_forms_render_in_hydra_output() {
  let near = decompile(
    &[0xFF, 0xD1, 0xC3],
    3,
    vec![],
    Flavor::Hydra { relocatable_segs: vec![] },
  );
  let far = decompile(
    &[0xFF, 0x5F, 0x02, 0xC3],
    4,
    vec![],
    Flavor::Hydra { relocatable_segs: vec![] },
  );
  assert!(near.contains("CALL_NEAR_INDIRECT(CX)"), "source:\n{near}");
  assert!(far.contains("CALL_FAR_INDIRECT("), "source:\n{far}");
}
