use dis86::asm::decode::Decoder;
use dis86::asm::instr::Reg;
use dis86::config::{CallMode, Config, Func};
use dis86::decompile::ir_build;
use dis86::decompile::{abi, opt, sym};
use dis86::segoff::{Off, Seg, SegOff};
use dis86::spec::Spec;
use dis86::types::{Type, TypeDatabase};

fn func(name: &str, start: u16, end: u16, ret: Option<Type>, regargs: Option<Vec<Reg>>) -> Func {
  Func {
    name: name.to_string(),
    start: SegOff { seg: Seg::Normal(0), off: Off(start) },
    end: Some(SegOff { seg: Seg::Normal(0), off: Off(end) }),
    entry: None,
    mode: CallMode::Near,
    ret,
    args: Some(0),
    regargs,
    dont_pop_args: false,
  }
}

fn config(funcs: Vec<Func>) -> Config {
  Config {
    types: std::rc::Rc::new(TypeDatabase::new()),
    structs: vec![],
    code_segs: vec![],
    funcs,
    indirects: vec![],
    globals: vec![],
    text_section: vec![],
  }
}

fn build_ir(code: &[u8], func: &Func, cfg: &Config) -> dis86::decompile::ir::IR {
  let binary = dis86::binary::Binary::from_raw(code, None);
  let spec = Spec::from_func(func);
  let mut decoder = Decoder::new(binary.region_iter(spec.start, spec.end));
  let mut instrs = vec![];
  while let Some((ins, _raw)) = decoder.try_next().expect("decode") {
    instrs.push(ins);
  }
  let mut ir = ir_build::build_from_instrs(&instrs, cfg, &spec, &binary, false, false);
  opt::optimize(&mut ir);
  sym::symbolize(&mut ir, cfg);
  opt::forward_store_to_load(&mut ir);
  opt::optimize(&mut ir);
  opt::mem_symbol_to_ref(&mut ir);
  opt::optimize(&mut ir);
  ir
}

fn infer(code: &[u8], func: &Func, cfg: &Config) -> Vec<Reg> {
  abi::infer_register_inputs(&build_ir(code, func, cfg))
}

fn first_call(ir: &dis86::decompile::ir::IR) -> dis86::decompile::ir::Ref {
  for blk in ir.iter_blocks() {
    for r in ir.iter_instrs(blk) {
      if ir.instr(r).unwrap().opcode == dis86::decompile::ir::Opcode::CallArgs {
        return r;
      }
    }
  }
  panic!("no call found");
}

#[test]
fn save_and_restore_only_is_not_an_input() {
  let code = [
    0x53,             // push bx
    0x51,             // push cx
    0x52,             // push dx
    0x56,             // push si
    0x5E,             // pop si
    0x5A,             // pop dx
    0x59,             // pop cx
    0x5B,             // pop bx
    0xC3,             // ret
  ];
  let f = func("save_only", 0, 9, Some(Type::Void), None);
  let cfg = config(vec![f.clone()]);
  let got = infer(&code, &f, &cfg);
  assert!(got.is_empty(), "unexpected inputs: {:?}", got);
}

#[test]
fn value_used_before_restore_is_an_input() {
  let code = [
    0x53,             // push bx
    0x89, 0xD8,       // mov ax,bx
    0x5B,             // pop bx
    0xC3,             // ret
  ];
  let f = func("uses_bx", 0, 5, Some(Type::U16), None);
  let cfg = config(vec![f.clone()]);
  assert_eq!(infer(&code, &f, &cfg), vec![Reg::BX]);
}

#[test]
fn stack_local_value_loaded_later_is_an_input() {
  let code = [
    0x89, 0x5E, 0xFE, // mov [bp-2],bx
    0x8B, 0x46, 0xFE, // mov ax,[bp-2]
    0xC3,             // ret
  ];
  let f = func("stack_local", 0, 7, Some(Type::U16), None);
  let cfg = config(vec![f.clone()]);
  assert_eq!(infer(&code, &f, &cfg), vec![Reg::BX]);
}

#[test]
fn exit_state_write_is_not_an_input() {
  let code = [
    0xB8, 0x05, 0x00, // mov ax,5
    0xBB, 0x07, 0x00, // mov bx,7
    0xC3,             // ret
  ];
  let f = func("writes_exit_state", 0, 7, Some(Type::Void), None);
  let cfg = config(vec![f.clone()]);
  assert!(infer(&code, &f, &cfg).is_empty());
}

#[test]
fn save_and_restore_around_call_is_not_an_input() {
  let code = [
    0x53,             // push bx
    0x51,             // push cx
    0xE8, 0x0B, 0x00, // call callee (0 args) at 0x0010; next IP is 0x0005
    0x59,             // pop cx
    0x5B,             // pop bx
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xC3,             // callee: ret
  ];
  let caller = func("call_save_caller", 0, 8, Some(Type::Void), None);
  let callee = func("call_save_callee", 0x10, 0x11, Some(Type::Void), None);
  let cfg = config(vec![caller.clone(), callee]);
  assert!(infer(&code, &caller, &cfg).is_empty());
}

#[test]
fn stack_argument_value_propagates_into_caller() {
  let code = [
    0x53,             // push bx
    0xE8, 0x0C, 0x00, // call callee(1 arg) at 0x0010; next IP is 0x0004
    0x83, 0xC4, 0x02, // add sp,2
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xC3,             // callee: ret
  ];
  let caller = func("stack_arg_caller", 0, 8, Some(Type::Void), None);
  let callee = func("stack_arg_callee", 0x10, 0x11, Some(Type::Void), None);
  let mut callee = callee;
  callee.args = Some(1);
  let cfg = config(vec![caller.clone(), callee]);
  assert_eq!(infer(&code, &caller, &cfg), vec![Reg::BX]);
}

#[test]
fn escaped_local_initializer_is_an_input() {
  let code = [
    0x89, 0x5E, 0xFE, // mov [bp-2],bx
    0x8D, 0x46, 0xFE, // lea ax,[bp-2]
    0x50,             // push ax
    0xE8, 0x06, 0x00, // call callee(1 arg) at 0x0010; next IP is 0x000a
    0x83, 0xC4, 0x02, // add sp,2
    0xC3,             // ret
    0x90, 0x90,       // padding to 0x0010
    0xC3,             // callee: ret
  ];
  let caller = func("escaped_caller", 0, 14, Some(Type::Void), None);
  let mut callee = func("escaped_callee", 0x10, 0x11, Some(Type::Void), None);
  callee.args = Some(1);
  let cfg = config(vec![caller.clone(), callee]);
  assert_eq!(infer(&code, &caller, &cfg), vec![Reg::BX]);
}

#[test]
fn callee_regargs_propagate_into_caller() {
  let code = [
    0x89, 0xD8,       // mov ax,bx
    0xE8, 0x0B, 0x00, // call callee (0x0010); next IP is 0x0005
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xC3,             // callee: ret
  ];
  let caller = func("caller", 0, 6, Some(Type::Void), None);
  let callee = func("callee", 0x10, 0x11, Some(Type::U16), Some(vec![Reg::AX]));
  let cfg = config(vec![caller.clone(), callee]);
  assert_eq!(infer(&code, &caller, &cfg), vec![Reg::BX]);
}

#[test]
fn caller_dx_read_marks_u32_candidate() {
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0x89, 0xD0,       // mov ax,dx
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("dx_caller", 0, 6, Some(Type::U16), None);
  let callee = func("dx_callee", 0x10, 0x19, Some(Type::U16), None);
  let cfg = config(vec![caller.clone(), callee.clone()]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}

#[test]
fn caller_ignoring_dx_does_not_mark_u32_candidate() {
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0xB8, 0x01, 0x00, // mov ax,1
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("ignore_caller", 0, 6, Some(Type::U16), None);
  let callee = func("ignore_callee", 0x10, 0x19, Some(Type::U16), None);
  let cfg = config(vec![caller.clone(), callee]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(!abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}

#[test]
fn caller_restoring_saved_dx_is_not_a_u32_candidate() {
  let code = [
    0x52,             // push dx
    0xE8, 0x0C, 0x00, // call callee at 0x0010; next IP is 0x0004
    0x5A,             // pop dx
    0x89, 0xD0,       // mov ax,dx
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("restore_caller", 0, 7, Some(Type::U16), None);
  let callee = func("restore_callee", 0x10, 0x19, Some(Type::U16), None);
  let cfg = config(vec![caller.clone(), callee]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(!abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}

#[test]
fn intervening_call_clobbers_dx_before_read() {
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0xE8, 0x0A, 0x00, // call callee at 0x0010; next IP is 0x0006
    0x89, 0xD0,       // mov ax,dx
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("clobber_caller", 0, 9, Some(Type::U16), None);
  let callee = func("clobber_callee", 0x10, 0x19, Some(Type::U16), None);
  let cfg = config(vec![caller.clone(), callee]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(!abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}

#[test]
fn function_without_reachable_ret_has_no_return() {
  let code = [0xEB, 0xFE]; // jmp $
  let f = func("spin", 0, 2, Some(Type::U16), None);
  let cfg = config(vec![f.clone()]);
  let ir = build_ir(&code, &f, &cfg);
  assert!(!abi::has_reachable_ret(&ir));
}
