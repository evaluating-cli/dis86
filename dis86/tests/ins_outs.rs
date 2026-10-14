// Regression tests for INS/OUTS string port I/O lowering (issue #69): the
// byte/word single and REP forms become runtime helpers that execute the raw
// instruction; the deterministic DI/SI/CX updates are modeled exactly. A
// possibly-set direction flag keeps the whole class loud.
use dis86::asm::decode::Decoder;
use dis86::asm::instr::{Instr, Opcode};
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

fn decode_all(code: &[u8]) -> (Vec<Instr>, dis86::binary::Binary, Spec) {
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
  (instr_list, binary, spec)
}

fn decompile_raw(code: &[u8]) -> String {
  let types = std::rc::Rc::new(TypeDatabase::new());
  let cfg = empty_config(types);
  let (instr_list, binary, spec) = decode_all(code);
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

// The decoder consumes no modrm byte for the implied-operand forms.
#[test]
fn ins_outs_decode_without_modrm() {
  let (ins, _, _) = decode_all(&[0x6C]);
  assert_eq!(ins.len(), 1);
  assert_eq!(ins[0].opcode, Opcode::OP_INS);
  assert_eq!(ins[0].n_bytes, 1, "insb is a single byte");

  let (ins, _, _) = decode_all(&[0x6F, 0xC3]);
  assert_eq!(ins[0].opcode, Opcode::OP_OUTS);
  assert_eq!(ins[0].n_bytes, 1, "outsw does not swallow the ret");
  assert_eq!(ins[1].opcode, Opcode::OP_RET);
}

// mov di,0x1234; insb; mov ax,di; ret
#[test]
fn insb_models_di_advance() {
  let code = [0xBF, 0x34, 0x12, 0x6C, 0x89, 0xF8, 0xC3];
  let source = decompile_raw(&code);
  assert!(!source.contains("UNIMPL("), "no abort marker:\n{source}");
  assert!(source.contains("INSB(ES, 0x1234, DX)"), "byte helper with ES:DI:\n{source}");
  assert!(source.contains("MAKE_32(DX, 0x1235)"), "DI must advance by 1:\n{source}");
}

// mov di,0x1234; insw; mov ax,di; ret
#[test]
fn insw_models_di_advance() {
  let code = [0xBF, 0x34, 0x12, 0x6D, 0x89, 0xF8, 0xC3];
  let source = decompile_raw(&code);
  assert!(source.contains("INSW(ES, 0x1234, DX)"), "word helper:\n{source}");
  assert!(source.contains("MAKE_32(DX, 0x1236)"), "DI must advance by 2:\n{source}");
}

// mov si,0x2000; outsb; mov ax,si; ret
#[test]
fn outsb_models_si_advance() {
  let code = [0xBE, 0x00, 0x20, 0x6E, 0x89, 0xF0, 0xC3];
  let source = decompile_raw(&code);
  assert!(!source.contains("UNIMPL("), "no abort marker:\n{source}");
  assert!(source.contains("OUTSB(DS, 0x2000, DX)"), "byte helper with DS:SI:\n{source}");
  assert!(source.contains("MAKE_32(DX, 0x2001)"), "SI must advance by 1:\n{source}");
}

// mov cx,4; rep insb; mov ax,di; ret
#[test]
fn rep_insb_models_cx_and_di() {
  let code = [0xB9, 0x04, 0x00, 0xF3, 0x6C, 0x89, 0xF8, 0xC3];
  let source = decompile_raw(&code);
  assert!(!source.contains("UNIMPL("), "no abort marker:\n{source}");
  assert!(source.contains("REP_INSB(ES, DI, 4, DX)"), "rep byte helper:\n{source}");
  assert!(source.contains("DI + (4 * 1)"), "DI advances by CX*size:\n{source}");
}

// mov cx,4; rep outsw; mov ax,si; ret
#[test]
fn rep_outsw_models_cx_and_si() {
  let code = [0xB9, 0x04, 0x00, 0xF3, 0x6F, 0x89, 0xF0, 0xC3];
  let source = decompile_raw(&code);
  assert!(source.contains("REP_OUTSW(DS, SI, 4, DX)"), "rep word helper:\n{source}");
  assert!(source.contains("SI + (4 * 2)"), "SI advances by CX*size:\n{source}");
}

// std; insb; outsb; ret -- a possibly-set direction flag keeps the class loud.
#[test]
fn std_keeps_ins_outs_loud() {
  let code = [0xFD, 0x6C, 0x6E, 0xC3];
  let source = decompile_raw(&code);
  assert!(!source.contains("INSB("), "no INS helper when DF may be set:\n{source}");
  assert!(!source.contains("OUTSB("), "no OUTS helper when DF may be set:\n{source}");
  assert!(source.contains("UNIMPL()"), "must stay loud:\n{source}");
}
