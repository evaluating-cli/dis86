use dis86::asm::instr::Reg;
use dis86::config::{Config, Global};
use dis86::decompile::ast::{Block, Expr, Function, Stmt};
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

fn append(ir: &mut ir::IR, blk: ir::BlockRef, typ: Type, opcode: Opcode, operands: Vec<Ref>) -> Ref {
  ir.block_instr_append(blk, Instr {
    typ,
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
  let func = Function::from_ir(&cfg, "byte_stack", None, &ir, &cf);

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
  let func = Function::from_ir(&cfg, "byte_global", None, &ir, &cf);

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
  let func = Function::from_ir(&cfg, "diamond", None, &ir, &cf);
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
