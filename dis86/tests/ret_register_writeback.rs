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
    regions: vec![],
  }
}

fn func_ret(name: &str, start: u16, end: u16, regargs: Option<Vec<Reg>>, ret: Type) -> Func {
  Func { ret: Some(ret), ..func(name, start, end, regargs) }
}

fn decompile_with_pin_all(code: &[u8], funcs: Vec<Func>, flavor: Flavor, pin_all: bool) -> String {
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
  let spec = Spec::from_config_name(&cfg, "main").expect("main function");

  let mut decoder = Decoder::new(binary.region_iter(spec.start, spec.end));
  let mut instr_list: Vec<Instr> = vec![];
  while let Some((ins, _raw)) = decoder.try_next().expect("decode") {
    instr_list.push(ins);
  }

  let mut ir = ir_build::build_from_instrs(&instr_list, &cfg, &spec, &binary, false, pin_all);
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
  gen::generate(&ast, flavor).expect("codegen")
}

fn decompile(code: &[u8], funcs: Vec<Func>, flavor: Flavor) -> String {
  decompile_with_pin_all(code, funcs, flavor, false)
}

fn standard(code: &[u8], funcs: Vec<Func>) -> String {
  decompile(code, funcs, Flavor::Standard)
}

fn find(source: &str, needle: &str) -> usize {
  source.find(needle).unwrap_or_else(|| panic!("missing {:?}:\n{source}", needle))
}

fn count(source: &str, needle: &str) -> usize {
  source.matches(needle).count()
}

/// The push/pop wrapper from the issue: save DI/SI, point them at a table, call,
/// restore, return. The pushes survive as stack stores today; the pops do not
/// reach the emitted C, so the caller sees the table pointers instead of the
/// restored values.
#[test]
fn ret_restores_saved_registers_around_a_known_call() {
  let code = [
    0x57,             // push di
    0x56,             // push si
    0xBE, 0x1E, 0x05, // mov si,0x051e
    0xBF, 0x30, 0x05, // mov di,0x0530
    0xE8, 0x03, 0x00, // call inner
    0x5E,             // pop si
    0x5F,             // pop di
    0xC3,             // ret
    0xC3,             // inner: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x000e, None),
    func("inner", 0x000e, 0x000f, Some(vec![])),
  ];
  let source = standard(&code, funcs);

  let call = find(&source, "inner(");
  let si = find(&source, "SI = ");
  let di = find(&source, "DI = ");
  assert!(si > call, "SI must be restored after the call:\n{source}");
  assert!(di > call, "DI must be restored after the call:\n{source}");
  let ret = source.rfind("return").expect("a return");
  assert!(si < ret, "restore must precede the return:\n{source}");
  assert!(di < ret, "restore must precede the return:\n{source}");
  // The pushes still materialise as the stack stores they always did.
  assert!(source.contains("= SI;"), "the SI save must survive:\n{source}");
  assert!(source.contains("= DI;"), "the DI save must survive:\n{source}");
}

/// BX is callee-saved in the caller model too, so a save/restore across a call
/// must reach the emitted exit state.
#[test]
fn ret_restores_bx_around_a_known_call() {
  let code = [
    0x53,             // push bx
    0xBB, 0x34, 0x12, // mov bx,0x1234
    0xE8, 0x02, 0x00, // call inner
    0x5B,             // pop bx
    0xC3,             // ret
    0xC3,             // inner: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x0009, None),
    func("inner", 0x0009, 0x000a, Some(vec![Reg::BX])),
  ];
  let source = standard(&code, funcs);
  assert!(
    source.contains("BX = _local"),
    "BX must be restored from the saved slot:\n{source}",
  );
  assert!(find(&source, "BX = _local") > find(&source, "inner("), "restore must follow the call:\n{source}");
}

/// The caller-saved direction of the same idiom: an explicit `pop cx`/`pop dx`
/// before the return is exit state (the caller keeps the pre-call CX/DX, see
/// `save_return_value`), so it has to be emitted too.
#[test]
fn ret_restores_explicitly_popped_caller_saved_registers() {
  let code = [
    0x51,             // push cx
    0x52,             // push dx
    0xB9, 0x11, 0x11, // mov cx,0x1111
    0xBA, 0x22, 0x22, // mov dx,0x2222
    0xE8, 0x03, 0x00, // call inner
    0x5A,             // pop dx
    0x59,             // pop cx
    0xC3,             // ret
    0xC3,             // inner: ret
  ];
  let funcs = vec![
    func_ret("main", 0x0000, 0x000e, None, Type::U16),
    func("inner", 0x000e, 0x000f, Some(vec![])),
  ];
  let source = standard(&code, funcs);

  let call = find(&source, "inner(");
  let cx = find(&source, "CX = ");
  let dx = find(&source, "DX = ");
  assert!(cx > call, "CX must be restored after the call:\n{source}");
  assert!(dx > call, "DX must be restored after the call:\n{source}");
  // The return type is U16, so DX is not the return high half: it is written
  // back exactly once.
  assert_eq!(count(&source, "DX = "), 1, "DX must be written back once:\n{source}");
}

/// A restore that puts the register back to its entry value after a call that
/// materialised the register must still be emitted: the live C variable was
/// overwritten, so "back to entry" is a real write, not a no-op.
#[test]
fn ret_restores_entry_value_after_a_materialising_call() {
  let code = [
    0x56,             // push si
    0xBE, 0x34, 0x12, // mov si,0x1234
    0xE8, 0x02, 0x00, // call inner
    0x5E,             // pop si
    0xC3,             // ret
    0xC3,             // inner: ret
  ];
  let funcs = vec![
    func("main", 0x0000, 0x0009, None),
    func("inner", 0x0009, 0x000a, Some(vec![Reg::SI])),
  ];
  let source = standard(&code, funcs);

  let setup = find(&source, "SI = 0x1234");
  let restored = source.rfind("SI = ").expect("a SI write");
  assert!(restored > setup, "the restore must follow the setup:\n{source}");
  assert!(
    !source[restored..].starts_with("SI = SI"),
    "the restore cannot be a self-assignment:\n{source}",
  );
  assert!(
    source[restored..].starts_with("SI = _local"),
    "the restore must read the saved slot:\n{source}",
  );
}

/// Registers the function never changes must not gain writes.
#[test]
fn ret_does_not_write_registers_that_never_changed() {
  let source = standard(&[0xC3], vec![func("main", 0x0000, 0x0001, None)]);
  for reg in ["BX", "CX", "DX", "SI", "DI"] {
    assert_eq!(count(&source, &format!("{reg} = ")), 0, "{reg} must not be written:\n{source}");
  }
}

/// A merge block is unsealed while lowering, so an untouched callee-saved
/// register looks dirty there. The exit write-back runs after sealing: it must
/// not invent writes, and a return value that reads the untouched register
/// must not be snapshotted.
#[test]
fn ret_through_a_merge_does_not_write_untouched_registers() {
  let code = [
    0x85, 0xC0,       // test ax,ax
    0x75, 0x03,       // jnz +3 (to 0x0007)
    0xBB, 0x01, 0x00, // mov bx,1
    0x89, 0xF0,       // 0x0007: mov ax,si
    0xC3,             // ret
  ];
  let source = standard(&code, vec![func_ret("main", 0x0000, 0x000a, None, Type::U16)]);
  assert!(source.contains("return SI;"), "the untouched SI is returned directly:\n{source}");
  for reg in ["CX", "DX", "SI", "DI"] {
    assert_eq!(count(&source, &format!("{reg} = ")), 0, "{reg} must not be written:\n{source}");
  }
  assert_eq!(count(&source, "= SI;"), 0, "the return must not be snapshotted:\n{source}");
}

/// The deferral must not drop a snapshot that is genuinely needed at a merge:
/// the return value reads SI's entry value while one path writes SI.
#[test]
fn ret_through_a_merge_still_snapshots_a_dirty_register() {
  let code = [
    0x89, 0xF0,       // mov ax,si
    0x85, 0xC0,       // test ax,ax
    0x74, 0x03,       // jz +3 (to 0x0009)
    0xBE, 0x01, 0x00, // mov si,1
    0xC3,             // 0x0009: ret
  ];
  let source = standard(&code, vec![func_ret("main", 0x0000, 0x000a, None, Type::U16)]);
  let snapshot = find(&source, "tmp_0 = SI;");
  let write_back = find(&source, "SI = ");
  assert!(snapshot < write_back, "the return value must be captured before the write:\n{source}");
  let ret = source.rfind("return").expect("a return");
  let ret_line = &source[ret..];
  assert!(ret_line.contains("tmp_0"), "the return must read the snapshot:\n{source}");
  assert!(!ret_line.contains("SI"), "the return must not read the rewritten SI:\n{source}");
}

/// A `push reg`/`pop reg` pair that folds back to the entry value gains no
/// write: the emitted register was never changed, so the restore is a no-op.
#[test]
fn ret_does_not_write_a_save_restore_that_folds_to_entry() {
  let code = [
    0x56, // push si
    0x5E, // pop si
    0xC3, // ret
  ];
  let source = standard(&code, vec![func("main", 0x0000, 0x0003, None)]);
  assert_eq!(count(&source, "SI = "), 0, "a folded save/restore must not add a write:\n{source}");
}

/// DX is the high half of a 32-bit return value; the return expression already
/// carries it, so the write-back must not duplicate it.
#[test]
fn ret_does_not_write_dx_when_it_is_the_return_high_half() {
  let code = [
    0xBA, 0x07, 0x00, // mov dx,7
    0xB8, 0x03, 0x00, // mov ax,3
    0xC3,             // ret
  ];
  let source = standard(&code, vec![func_ret("main", 0x0000, 0x0007, None, Type::U32)]);
  assert!(source.contains("MAKE_32(7, 3)"), "DX must travel in the return value:\n{source}");
  assert_eq!(count(&source, "DX = "), 0, "DX must not be written twice:\n{source}");
  assert_eq!(count(&source, "AX = "), 0, "AX must not be written twice:\n{source}");

  // With a 16-bit return type DX is not return state, so the dirty value is
  // written back exactly once.
  let source = standard(&code, vec![func_ret("main", 0x0000, 0x0007, None, Type::U16)]);
  assert_eq!(count(&source, "DX = "), 1, "DX must be written back once:\n{source}");
  assert!(source.contains("return 3;"), "AX stays the return value:\n{source}");
}

/// Every return path is a function exit, so each one writes back its own state.
#[test]
fn every_return_path_writes_back_exit_state() {
  let code = [
    0xBE, 0x01, 0x00, // mov si,1
    0x74, 0x01,       // jz +1
    0xC3,             // ret  (SI=1)
    0xBE, 0x02, 0x00, // mov si,2
    0xC3,             // ret  (SI=2)
  ];
  let source = standard(&code, vec![func("main", 0x0000, 0x000a, None)]);
  assert_eq!(count(&source, "SI = "), 2, "each return path needs its own write:\n{source}");
}

/// A segment register the function never changes must not gain a write.
#[test]
fn unchanged_segment_registers_are_not_written_back() {
  let code = [
    0x1E, // push ds
    0x5E, // pop si
    0xC3, // ret
  ];
  let source = standard(&code, vec![func("main", 0x0000, 0x0003, None)]);
  assert!(source.contains("SI = DS"), "the general-purpose restore is emitted:\n{source}");
  for seg in ["DS = ", "ES = ", "SS = "] {
    assert_eq!(count(&source, &format!("{seg}")), 0, "{seg} must not be written back:\n{source}");
  }
}

/// A changed DS is part of the function's observable exit state.
#[test]
fn changed_ds_is_written_back_in_both_flavours() {
  let code = [
    0xB8, 0x07, 0x06, // mov ax,0x0607
    0x8E, 0xD8,       // mov ds,ax
    0xC3,             // ret
  ];
  for flavor in [Flavor::Standard, Flavor::Hydra { relocatable_segs: vec![] }] {
    let source = decompile(&code, vec![func("main", 0x0000, 0x0006, None)], flavor);
    let ds = find(&source, "DS = ");
    assert!(source[ds..].contains("0x607"), "DS must carry the changed value:\n{source}");
  }
}

/// A changed ES is caller-visible even without `pin_all`.
#[test]
fn changed_es_is_written_back() {
  let code = [
    0xB8, 0x34, 0x12, // mov ax,0x1234
    0x8E, 0xC0,       // mov es,ax
    0xC3,             // ret
  ];
  let source = standard(&code, vec![func("main", 0x0000, 0x0006, None)]);
  let es = find(&source, "ES = ");
  assert!(source[es..].contains("0x1234"), "ES must carry the changed value:\n{source}");
}

/// SS is stack state, but it is still exit state. `FRAME_LEAVE` must pop the
/// saved BP through the entry stack segment, while the return must see the new
/// one, so the SS write belongs between them.
#[test]
fn changed_ss_is_written_back_after_the_hydra_frame() {
  let code = [
    0xB8, 0x34, 0x12, // mov ax,0x1234
    0x8E, 0xD0,       // mov ss,ax
    0xC3,             // ret
  ];
  let source = decompile(
    &code,
    vec![func("main", 0x0000, 0x0006, None)],
    Flavor::Hydra { relocatable_segs: vec![] },
  );
  let ss = find(&source, "SS = ");
  assert!(source[ss..].contains("0x1234"), "SS must carry the changed value:\n{source}");
  assert!(find(&source, "FRAME_LEAVE()") < ss, "SS must follow the frame teardown:\n{source}");
  assert!(ss < find(&source, "RETURN_NEAR()"), "SS must precede the return:\n{source}");
}

/// A deferred SS write must not observe an exit write that ran first. Here SS
/// takes the entry BX, and BX changes afterward at the same ret, so the value
/// has to be frozen before either write.
#[test]
fn changed_ss_does_not_observe_a_later_bx_write() {
  let code = [
    0x8E, 0xD3,       // mov ss,bx
    0xBB, 0x78, 0x56, // mov bx,0x5678
    0xC3,             // ret
  ];
  let source = decompile(
    &code,
    vec![func("main", 0x0000, 0x0006, None)],
    Flavor::Hydra { relocatable_segs: vec![] },
  );
  let temp = find(&source, " = BX;");
  assert!(temp < find(&source, "BX = 0x5678"), "SS entry value must be frozen before BX changes:\n{source}");
  let ss = find(&source, "SS = ");
  assert!(find(&source, "FRAME_LEAVE()") < ss, "SS must follow the frame teardown:\n{source}");
  assert!(!source[ss..].starts_with("SS = BX;"), "the deferred SS write must use the frozen value:\n{source}");
  assert!(!source[ss..].contains("0x5678"), "SS must not pick up the later BX value:\n{source}");
}

/// `pin_all` must not replace changed segment SSA with a `Pin` that AST
/// emission drops.
#[test]
fn pin_all_keeps_changed_segment_exit_state() {
  let code = [
    0xB8, 0x07, 0x06, // mov ax,0x0607
    0x8E, 0xD8,       // mov ds,ax
    0xC3,             // ret
  ];
  let source = decompile_with_pin_all(
    &code,
    vec![func("main", 0x0000, 0x0006, None)],
    Flavor::Standard,
    true,
  );
  assert!(source.contains("DS = "), "the changed DS write survives pin_all:\n{source}");
  assert!(source.contains("0x607"), "the DS value is materialised:\n{source}");
}

/// A return expression that reads DS must see the value before the exit write
/// changes it.
#[test]
fn ret_value_reading_ds_is_snapshotted_before_the_segment_write() {
  let code = [
    0x8C, 0xD8, // mov ax,ds
    0x8E, 0xDB, // mov ds,bx
    0xC3,       // ret
  ];
  let source = standard(&code, vec![func_ret("main", 0x0000, 0x0005, None, Type::U16)]);
  let snapshot = find(&source, "tmp_0 = DS;");
  let write_back = find(&source, "DS = BX;");
  assert!(snapshot < write_back, "the return value must be captured first:\n{source}");
  let ret = source.rfind("return").expect("a return");
  let ret_line = &source[ret..];
  assert!(ret_line.contains("tmp_0"), "the return must read the snapshot:\n{source}");
  assert!(!ret_line.contains("DS"), "the return must not read the rewritten DS:\n{source}");
}

/// Segment writes participate in the same program-order dependency handling as
/// the general-purpose registers: DS reads the pre-write BX.
#[test]
fn segment_write_observes_the_pre_write_gpr_value() {
  let code = [
    0x8E, 0xDB,       // mov ds,bx
    0xBB, 0x34, 0x12, // mov bx,0x1234
    0xC3,             // ret
  ];
  let source = standard(&code, vec![func("main", 0x0000, 0x0006, None)]);
  let ds = find(&source, "DS = BX;");
  let bx = find(&source, "BX = 0x1234;");
  assert!(ds < bx, "DS must read BX before BX's own exit write:\n{source}");
}

/// The Hydra flavour places GPR exit writes before the frame teardown and the
/// return, so the machine observes the restored register state. SS is the
/// exception and is covered by the frame-order test above.
#[test]
fn hydra_flavour_writes_exit_state_before_the_return() {
  let code = [
    0x56,             // push si
    0xBE, 0x34, 0x12, // mov si,0x1234
    0xE8, 0x02, 0x00, // call inner
    0x5E,             // pop si
    0xC3,             // ret
    0xC3,             // inner: ret
  ];
  let source = decompile(
    &code,
    vec![
      func("main", 0x0000, 0x0009, None),
      func("inner", 0x0009, 0x000a, Some(vec![Reg::SI])),
    ],
    Flavor::Hydra { relocatable_segs: vec![] },
  );
  let restore = find(&source, "SI = _local");
  assert!(restore > find(&source, "inner(m)"), "restore must follow the call:\n{source}");
  assert!(restore < find(&source, "FRAME_LEAVE()"), "restore must precede the frame teardown:\n{source}");
  assert!(restore < find(&source, "RETURN_NEAR()"), "restore must precede the return:\n{source}");
}

/// The return value may read a register that the exit write-back is about to
/// change. `mov ax,cx; dec cx; ret` returns the *pre-decrement* CX, so the
/// write-back must not make the return expression observe the new value.
#[test]
fn ret_value_reading_a_written_register_is_snapshotted() {
  let code = [
    0x89, 0xC8, // mov ax,cx
    0x49,       // dec cx
    0xC3,       // ret
  ];
  let source = standard(&code, vec![func_ret("main", 0x0000, 0x0004, None, Type::U16)]);
  let snapshot = find(&source, "tmp_0 = CX;");
  let write_back = find(&source, "CX = CX - 1;");
  assert!(snapshot < write_back, "the return value must be captured before the write:\n{source}");
  let ret = source.rfind("return").expect("a return");
  let ret_line = &source[ret..];
  assert!(ret_line.contains("tmp_0"), "the return must read the snapshot:\n{source}");
  assert!(!ret_line.contains("CX"), "the return must not read the rewritten CX:\n{source}");
}

/// The same hazard through an indirect read: the return expression dereferences
/// SI, and `add si,2` runs as exit state before the return.
#[test]
fn ret_value_reading_a_pointer_register_is_snapshotted() {
  let code = [
    0x8B, 0x04,       // mov ax,[si]
    0x83, 0xC6, 0x02, // add si,2
    0xC3,             // ret
  ];
  let source = standard(&code, vec![func_ret("main", 0x0000, 0x0006, None, Type::U16)]);
  let snapshot = find(&source, "tmp_0 = ");
  let write_back = find(&source, "SI = SI + 2;");
  assert!(snapshot < write_back, "the dereference must be captured before the write:\n{source}");
  let ret = source.rfind("return").expect("a return");
  let ret_line = &source[ret..];
  assert!(ret_line.contains("tmp_0"), "the return must read the snapshot:\n{source}");
  assert!(!ret_line.contains("SI"), "the return must not read the rewritten SI:\n{source}");
}

/// The Hydra flavour assigns AX instead of returning, but the snapshot must
/// still be taken before the exit write and used by that assignment.
#[test]
fn hydra_uses_the_snapshot_for_the_return_value() {
  let code = [
    0x89, 0xC8, // mov ax,cx
    0x49,       // dec cx
    0xC3,       // ret
  ];
  let source = decompile(
    &code,
    vec![func_ret("main", 0x0000, 0x0004, None, Type::U16)],
    Flavor::Hydra { relocatable_segs: vec![] },
  );
  let snapshot = find(&source, "tmp_0 = CX;");
  let write_back = find(&source, "CX = CX - 1;");
  let ret_assign = find(&source, "AX = tmp_0;");
  assert!(snapshot < write_back, "the value must be captured before the write:\n{source}");
  assert!(write_back < ret_assign, "the return value must follow the write:\n{source}");
  assert!(ret_assign < find(&source, "RETURN_NEAR()"), "the assignment must precede the return:\n{source}");
}
