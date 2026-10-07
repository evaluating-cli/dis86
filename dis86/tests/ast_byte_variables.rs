use dis86::asm::instr::Reg;
use dis86::config::{Config, Global};
use dis86::decompile::ast::{Assign, Block, Expr, Function, Stmt};
use dis86::decompile::control_flow::ControlFlow;
use dis86::decompile::gen::{self, Flavor};
use dis86::decompile::ir::{self, Attribute, Instr, Opcode, Ref};
use dis86::decompile::sym;
use dis86::types::{Type, TypeDatabase};
use std::rc::Rc;

fn empty_config(types: Rc<TypeDatabase>) -> Config {
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

#[test]
fn guest_pointer_expressions_emit_segmented_loads_and_stores() {
  let func = Function {
    name: "guest_pointer".into(),
    ret: None,
    vardecls: vec![],
    varmaps: vec![],
    frame_size: 0,
    body: Block(vec![
      Stmt::Assign(Assign {
        decltype: None,
        lhs: Expr::GuestMem(Box::new(Expr::Name("ES".into())), Box::new(Expr::Name("off".into())), 2),
        rhs: Expr::HexConst(0x1234),
      }),
      Stmt::Expr(Expr::GuestMem(Box::new(Expr::Name("SS".into())), Box::new(Expr::Name("off".into())), 2)),
    ]),
  };
  let source = gen::generate(&func, Flavor::Hydra { dgroup_seg: None }).unwrap();
  assert!(source.contains("STORE_16(ES, off, 0x1234);"), "missing segmented pointer store:\n{source}");
  assert!(source.contains("LOAD_16(SS, off)"), "missing segmented pointer read:\n{source}");
}

#[test]
fn annotated_near_es_pointer_reads_and_writes_use_segmented_memory() {
  use dis86::types::GuestPtrKind;

  let types = Rc::new(TypeDatabase::new());
  let mut cfg = empty_config(types.clone());
  cfg.globals.push(Global {
    name: "g_pointer".into(),
    offset: 0x0100,
    typ: Type::GuestPtr(Box::new(Type::U16), GuestPtrKind::NearEs),
  });
  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");
  let p_off = ir.const_new(0x0100);
  let pointer = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::DS), p_off]);
  let value = ir.const_new(0x1234);
  append(&mut ir, blk, Type::Void, Opcode::Store16, vec![Ref::Init(Reg::ES), pointer, value]);
  let loaded = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::ES), pointer]);
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![loaded]);

  sym::symbolize_globals(&mut ir, &cfg);
  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "near_es_access", None, &ir, &cf).unwrap();
  let source = gen::generate(&func, Flavor::Hydra { dgroup_seg: None }).unwrap();
  assert!(source.contains("tmp_0 = g_pointer;"), "pointer field should be read as its guest-width value:\n{source}");
  assert!(source.contains("STORE_16(ES, tmp_0, 0x1234);"), "missing ES pointer store:\n{source}");
  assert!(source.contains("LOAD_16(ES, tmp_0)"), "missing ES pointer read:\n{source}");
}

#[test]
fn pointer_annotation_on_struct_member_reaches_indirect_accesses() {
  use dis86::config::{Struct as ConfigStruct, StructMember};
  use dis86::types::GuestPtrKind;

  let mut type_db = TypeDatabase::new();
  let layout = ConfigStruct {
    name: "pointer_holder_t".into(),
    size: 2,
    members: vec![StructMember {
      name: "ptr".into(),
      typ: Type::GuestPtr(Box::new(Type::U16), GuestPtrKind::NearEs),
      off: 0,
    }],
  };
  type_db.append_struct(&layout);
  let typ = type_db.parse_type("pointer_holder_t").unwrap();
  let types = Rc::new(type_db);
  let mut cfg = empty_config(types.clone());
  cfg.structs.push(layout);
  cfg.globals.push(Global { name: "g_holder".into(), offset: 0x0100, typ });

  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");
  let pointer_slot = ir.const_new(0x0100);
  let pointer = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::DS), pointer_slot]);
  let value = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::ES), pointer]);
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![value]);

  sym::symbolize_globals(&mut ir, &cfg);
  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "struct_pointer_access", None, &ir, &cf).unwrap();
  let source = gen::generate(&func, Flavor::Hydra { dgroup_seg: None }).unwrap();
  assert!(source.contains("g_holder.ptr"), "expected annotated struct pointer field read:\n{source}");
  assert!(source.contains("LOAD_16(ES,"), "struct pointer annotation must select ES for dereference:\n{source}");
}

#[test]
fn annotated_far_pointer_splits_segment_and_offset_for_memory_access() {
  use dis86::types::GuestPtrKind;

  let types = Rc::new(TypeDatabase::new());
  let mut cfg = empty_config(types.clone());
  cfg.globals.push(Global {
    name: "g_far_pointer".into(),
    offset: 0x0100,
    typ: Type::GuestPtr(Box::new(Type::U16), GuestPtrKind::Far),
  });
  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");
  let p_off = ir.const_new(0x0100);
  let pointer = append(&mut ir, blk, Type::U32, Opcode::Load32, vec![Ref::Init(Reg::DS), p_off]);
  let seg = append(&mut ir, blk, Type::U16, Opcode::Upper16, vec![pointer]);
  let off = append(&mut ir, blk, Type::U16, Opcode::Lower16, vec![pointer]);
  let loaded = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![seg, off]);
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![loaded]);

  sym::symbolize_globals(&mut ir, &cfg);
  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "far_access", None, &ir, &cf).unwrap();
  let source = gen::generate(&func, Flavor::Hydra { dgroup_seg: None }).unwrap();
  assert!(source.contains("LOAD_16("), "missing segmented far-pointer load:\n{source}");
  assert!(source.contains("g_far_pointer"), "far pointer must remain a guest-width field:\n{source}");
  assert!(!source.contains("g_far_pointer->"), "far pointer must not become a host pointer:\n{source}");
}

#[test]
fn far_pointer_offset_with_unrelated_segment_keeps_raw_memory_path() {
  use dis86::types::GuestPtrKind;

  let types = Rc::new(TypeDatabase::new());
  let mut cfg = empty_config(types.clone());
  cfg.globals.push(Global {
    name: "g_far_pointer_mismatch".into(),
    offset: 0x0100,
    typ: Type::GuestPtr(Box::new(Type::U16), GuestPtrKind::Far),
  });
  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");
  let p_off = ir.const_new(0x0100);
  let pointer = append(&mut ir, blk, Type::U32, Opcode::Load32, vec![Ref::Init(Reg::DS), p_off]);
  let off = append(&mut ir, blk, Type::U16, Opcode::Lower16, vec![pointer]);
  let loaded = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::DS), off]);
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![loaded]);

  sym::symbolize_globals(&mut ir, &cfg);
  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "far_segment_mismatch", None, &ir, &cf).unwrap();
  let source = gen::generate(&func, Flavor::Hydra { dgroup_seg: None }).unwrap();
  assert!(source.contains("PTR_16(DS,"), "unpaired far-pointer segment must keep the original address path:\n{source}");
  assert!(!source.contains("LOAD_16("), "mismatched far pointer must not substitute its segment:\n{source}");
}

#[test]
fn near_es_pointer_with_unrelated_segment_keeps_raw_memory_path() {
  use dis86::types::GuestPtrKind;

  let types = Rc::new(TypeDatabase::new());
  let mut cfg = empty_config(types.clone());
  cfg.globals.push(Global {
    name: "g_near_es_pointer_mismatch".into(),
    offset: 0x0100,
    typ: Type::GuestPtr(Box::new(Type::U16), GuestPtrKind::NearEs),
  });
  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");
  let p_off = ir.const_new(0x0100);
  let pointer = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::DS), p_off]);
  let loaded = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::DS), pointer]);
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![loaded]);

  sym::symbolize_globals(&mut ir, &cfg);
  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "near_es_segment_mismatch", None, &ir, &cf).unwrap();
  let source = gen::generate(&func, Flavor::Hydra { dgroup_seg: None }).unwrap();
  assert!(source.contains("PTR_16(DS,"), "unpaired near-pointer segment must keep the original address path:\n{source}");
  assert!(!source.contains("LOAD_16("), "mismatched near pointer must not substitute its segment:\n{source}");
}

#[test]
fn nested_array_annotation_generates_c_dimension_order() {
  let types = Rc::new(TypeDatabase::new());
  let mut cfg = empty_config(types.clone());
  cfg.globals.push(Global {
    name: "g_grid".into(),
    offset: 0x0100,
    typ: Type::Array(Box::new(Type::Array(Box::new(Type::U16), dis86::types::ArraySize::Known(4))), dis86::types::ArraySize::Known(3)),
  });
  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");
  let cell = ir.const_new(0x010c); // ((1 * 4) + 2) * sizeof(u16)
  let loaded = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::DS), cell]);
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![loaded]);

  sym::symbolize_globals(&mut ir, &cfg);
  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "grid_access", None, &ir, &cf).unwrap();
  let source = gen::generate(&func, Flavor::Hydra { dgroup_seg: None }).unwrap();
  assert!(source.contains("g_grid[1][2]"), "expected row/column C indexing:\n{source}");
}

#[test]
fn annotated_member_layout_failure_returns_contextual_error() {
  use dis86::config::{Struct as ConfigStruct, StructMember};

  let mut type_db = TypeDatabase::new();
  let layout = ConfigStruct {
    name: "byte_pair_t".into(),
    size: 2,
    members: vec![
      StructMember { name: "lo".into(), typ: Type::U8, off: 0 },
      StructMember { name: "hi".into(), typ: Type::U8, off: 1 },
    ],
  };
  type_db.append_struct(&layout);
  let typ = type_db.parse_type("byte_pair_t").unwrap();
  let types = Rc::new(type_db);
  let mut cfg = empty_config(types.clone());
  cfg.structs.push(layout);
  cfg.globals.push(Global { name: "g_pair".into(), offset: 0x0100, typ });

  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");
  let off = ir.const_new(0x0100);
  let loaded = append(&mut ir, blk, Type::U16, Opcode::Load16, vec![Ref::Init(Reg::DS), off]);
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![loaded]);
  sym::symbolize_globals(&mut ir, &cfg);
  let cf = ControlFlow::from_ir(&ir);
  let err = Function::from_ir(&cfg, "bad_member_access", None, &ir, &cf).unwrap_err();
  assert!(err.contains("g_pair"), "missing symbol context in error: {err}");
  assert!(err.contains("member"), "missing layout context in error: {err}");
}

fn append(ir: &mut ir::IR, blk: ir::BlockRef, typ: Type, opcode: Opcode, operands: Vec<Ref>) -> Ref {
  ir.block_instr_append(blk, Instr {
    typ,
    compare_width: None,
    attrs: Attribute::NONE,
    opcode,
    operands,
  })
}

#[test]
fn byte_stack_symbol_reads_and_writes_emit_ptr8_mapping() {
  let types = Rc::new(TypeDatabase::new());
  let cfg = empty_config(types.clone());
  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");

  let three = ir.const_new(3);
  let addr = append(
    &mut ir,
    blk,
    Type::U16,
    Opcode::Sub,
    vec![Ref::Init(Reg::SP), three],
  );
  let value = ir.const_new(0x12);
  let store = append(
    &mut ir,
    blk,
    Type::Void,
    Opcode::Store8,
    vec![Ref::Init(Reg::SS), addr, value],
  );
  let load = append(
    &mut ir,
    blk,
    Type::U8,
    Opcode::Load8,
    vec![Ref::Init(Reg::SS), addr],
  );
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![load]);

  sym::symbolize_stack(&mut ir);
  assert_eq!(ir.instr(store).unwrap().opcode, Opcode::WriteVar8);
  assert_eq!(ir.instr(load).unwrap().opcode, Opcode::ReadVar8);

  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "byte_stack", None, &ir, &cf).unwrap();

  assert_eq!(func.varmaps.len(), 1);
  assert_eq!(func.varmaps[0].typ, Type::U8);
  match &func.varmaps[0].mapping_expr {
    Expr::Deref(inner) => match inner.as_ref() {
      Expr::Abstract(name, _) => assert_eq!(*name, "PTR_8"),
      other => panic!("expected PTR_8 mapping, got {:?}", other),
    },
    other => panic!("expected dereferenced byte mapping, got {:?}", other),
  }

  assert!(func.body.0.iter().any(|stmt| matches!(stmt, Stmt::Assign(_))));
  assert!(func.body.0.iter().any(|stmt| matches!(stmt, Stmt::Return(_))));
}

#[test]
fn byte_global_store_emits_symbol_assignment() {
  let types = Rc::new(TypeDatabase::new());
  let mut cfg = empty_config(types.clone());
  cfg.globals.push(Global {
    name: "g_byte".to_string(),
    offset: 0x1234,
    typ: Type::U8,
  });

  let mut ir = ir::IR::new(types);
  let blk = ir.add_block("entry");
  let off = ir.const_new(0x1234);
  let value = ir.const_new(0x34);
  let store = append(
    &mut ir,
    blk,
    Type::Void,
    Opcode::Store8,
    vec![Ref::Init(Reg::DS), off, value],
  );
  append(&mut ir, blk, Type::Void, Opcode::RetNear, vec![]);

  sym::symbolize_globals(&mut ir, &cfg);
  assert_eq!(ir.instr(store).unwrap().opcode, Opcode::WriteVar8);

  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "byte_global", None, &ir, &cf).unwrap();

  let assignment = func.body.0.iter().find_map(|stmt| match stmt {
    Stmt::Assign(assign) => Some(assign),
    _ => None,
  }).expect("expected byte global assignment");

  match &assignment.lhs {
    Expr::Name(name) => assert_eq!(name, "g_byte"),
    other => panic!("expected global symbol lhs, got {:?}", other),
  }
}

#[test]
fn reducible_diamond_generates_if_else_code() {
  let types = Rc::new(TypeDatabase::new());
  let cfg = empty_config(types.clone());
  let mut ir = ir::IR::new(types);
  let entry = ir.add_block("entry");
  let then_blk = ir.add_block("then");
  let else_blk = ir.add_block("else");
  let join = ir.add_block("join");

  let cond = ir.const_new(1);
  ir.block_mut(then_blk).preds.push(entry);
  ir.block_mut(else_blk).preds.push(entry);
  append(
    &mut ir,
    entry,
    Type::Void,
    Opcode::Jne,
    vec![cond, Ref::Block(then_blk), Ref::Block(else_blk)],
  );
  let then_effect = ir.const_new(0x21);
  append(&mut ir, then_blk, Type::Void, Opcode::Int, vec![then_effect]);
  let else_effect = ir.const_new(0x22);
  append(&mut ir, else_blk, Type::Void, Opcode::Int, vec![else_effect]);
  ir.block_mut(join).preds.extend([then_blk, else_blk]);
  append(&mut ir, then_blk, Type::Void, Opcode::Jmp, vec![Ref::Block(join)]);
  append(&mut ir, else_blk, Type::Void, Opcode::Jmp, vec![Ref::Block(join)]);
  append(&mut ir, join, Type::Void, Opcode::RetNear, vec![]);

  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "diamond", None, &ir, &cf).unwrap();
  let if_stmt = func.body.0.iter().find_map(|stmt| match stmt {
    Stmt::If(if_stmt) => Some(if_stmt),
    _ => None,
  }).expect("expected a structured if statement");
  fn int_operands(body: &Block) -> Vec<u16> {
    body.0.iter().filter_map(|stmt| match stmt {
      Stmt::Expr(Expr::Abstract("INT", args)) => match args.as_slice() {
        [Expr::HexConst(value)] => Some(*value),
        [Expr::DecimalConst(value)] => Some(*value as u16),
        _ => None,
      },
      _ => None,
    }).collect()
  }
  assert_eq!(int_operands(&if_stmt.then_body), vec![0x21]);
  assert_eq!(int_operands(if_stmt.else_body.as_ref().unwrap()), vec![0x22]);

  let source = gen::generate(&func, Flavor::Standard).unwrap();
  assert!(source.contains("if ("), "missing if in generated source:\n{source}");
  assert!(source.contains(" else "), "missing else in generated source:\n{source}");
}

#[test]
fn reducible_if_with_empty_then_arm_generates_one_arm_if() {
  let types = Rc::new(TypeDatabase::new());
  let cfg = empty_config(types.clone());
  let mut ir = ir::IR::new(types);
  let entry = ir.add_block("entry");
  let arm_a = ir.add_block("arm_a");
  let arm_b = ir.add_block("arm_b");
  let join = ir.add_block("join");

  let cond = ir.const_new(1);
  ir.block_mut(join).preds.push(entry);
  ir.block_mut(arm_a).preds.push(entry);
  ir.block_mut(arm_b).preds.push(arm_a);
  append(
    &mut ir,
    entry,
    Type::Void,
    Opcode::Jne,
    vec![cond, Ref::Block(join), Ref::Block(arm_a)],
  );
  let first_effect = ir.const_new(0x31);
  append(&mut ir, arm_a, Type::Void, Opcode::Int, vec![first_effect]);
  let second_effect = ir.const_new(0x32);
  append(&mut ir, arm_b, Type::Void, Opcode::Int, vec![second_effect]);
  ir.block_mut(join).preds.push(arm_b);
  append(&mut ir, arm_a, Type::Void, Opcode::Jmp, vec![Ref::Block(arm_b)]);
  append(&mut ir, arm_b, Type::Void, Opcode::Jmp, vec![Ref::Block(join)]);
  append(&mut ir, join, Type::Void, Opcode::RetNear, vec![]);

  let cf = ControlFlow::from_ir(&ir);
  let func = Function::from_ir(&cfg, "empty_then_arm", None, &ir, &cf).unwrap();
  let if_stmt = func.body.0.iter().find_map(|stmt| match stmt {
    Stmt::If(if_stmt) => Some(if_stmt),
    _ => None,
  }).expect("expected a structured if statement");
  assert!(if_stmt.else_body.is_none(), "direct-to-join arm should not become an else body");
  let int_operands = if_stmt.then_body.0.iter().filter_map(|stmt| match stmt {
    Stmt::Expr(Expr::Abstract("INT", args)) => match args.as_slice() {
      [Expr::HexConst(value)] => Some(*value),
      [Expr::DecimalConst(value)] => Some(*value as u16),
      _ => None,
    },
    _ => None,
  }).collect::<Vec<_>>();
  assert_eq!(int_operands, vec![0x31, 0x32]);

  let source = gen::generate(&func, Flavor::Standard).unwrap();
  assert!(source.contains("if ("), "missing if in generated source:\n{source}");
}
