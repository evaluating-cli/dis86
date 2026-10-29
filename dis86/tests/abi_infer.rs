use dis86::asm::instr::Reg;
use dis86::config::{CallMode, Config, Func};
use dis86::decompile::abi;
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
  dis86::app_infer::build_ir_for_analysis(cfg, &binary, &spec, false)
    .expect("build analysis IR").0
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

#[test]
fn dead_dx_move_is_not_return_evidence() {
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0x89, 0xD3,       // mov bx,dx (dead: bx is never used)
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("dead_dx_caller", 0, 6, Some(Type::U16), None);
  let callee = func("dead_dx_callee", 0x10, 0x19, Some(Type::U16), None);
  let cfg = config(vec![caller.clone(), callee]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(!abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}

#[test]
fn dx_read_after_conditional_branch_is_evidence() {
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0x74, 0x02,       // jz +2 -> 0x0007 (the ret)
    0x89, 0xD0,       // mov ax,dx
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("branch_dx_caller", 0, 8, Some(Type::U16), None);
  let callee = func("branch_dx_callee", 0x10, 0x19, Some(Type::U16), None);
  let cfg = config(vec![caller.clone(), callee]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}

#[test]
fn out_of_range_branch_still_reports_ret_and_inputs() {
  // #73/#140 shape: the explicit target is dropped as an empty block, but the
  // fallthrough path still reaches the ret and reads BX.
  let code = [
    0x74, 0x04, // jz +4 -> target 0x0006, outside [0, 5)
    0x89, 0xD8, // mov ax,bx
    0xC3,       // ret
  ];
  let f = func("out_of_range", 0, 5, Some(Type::U16), None);
  let cfg = config(vec![f.clone()]);
  assert_eq!(infer(&code, &f, &cfg), vec![Reg::BX]);
  let ir = build_ir(&code, &f, &cfg);
  assert!(abi::has_reachable_ret(&ir), "the fallthrough ret must stay reachable");
}

#[test]
fn segment_register_read_is_an_input() {
  let code = [
    0x8C, 0xD8, // mov ax,ds
    0xC3,       // ret
  ];
  let f = func("reads_ds", 0, 3, Some(Type::U16), None);
  let cfg = config(vec![f.clone()]);
  assert_eq!(infer(&code, &f, &cfg), vec![Reg::DS]);
}

#[test]
fn unannotated_ret_does_not_report_return_registers_as_inputs() {
  // With no return annotation the IR models DX:AX as the return value. Those
  // entry values are the caller's exit state, not regargs.
  let code = [0xC3];
  let f = func("unannotated", 0, 1, None, None);
  let cfg = config(vec![f.clone()]);
  assert!(infer(&code, &f, &cfg).is_empty());
}

#[test]
fn u32_return_caller_dx_read_is_evidence() {
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0x89, 0xD0,       // mov ax,dx
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("u32_dx_caller", 0, 6, Some(Type::U16), None);
  let callee = func("u32_dx_callee", 0x10, 0x19, Some(Type::U32), None);
  let cfg = config(vec![caller.clone(), callee]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}

#[test]
fn unannotated_return_caller_dx_read_is_evidence() {
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0x89, 0xD0,       // mov ax,dx
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("unknown_dx_caller", 0, 6, Some(Type::U16), None);
  let callee = func("unknown_dx_callee", 0x10, 0x19, None, None);
  let cfg = config(vec![caller.clone(), callee]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}

#[test]
fn u32_return_dead_dx_move_is_not_evidence() {
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0x89, 0xD3,       // mov bx,dx (dead)
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let caller = func("u32_dead_caller", 0, 6, Some(Type::U16), None);
  let callee = func("u32_dead_callee", 0x10, 0x19, Some(Type::U32), None);
  let cfg = config(vec![caller.clone(), callee]);
  let ir = build_ir(&code, &caller, &cfg);
  let live = abi::semantic_live(&ir);
  assert!(!abi::caller_reads_dx_after_call(&ir, &live, first_call(&ir)));
}
