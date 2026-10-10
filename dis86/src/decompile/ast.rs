use crate::decompile::ir;
use crate::decompile::sym;
use crate::decompile::control_flow::{self, ControlFlow, Detail, ElemId};
use crate::types::*;
use crate::config::Config;
use std::collections::{HashMap, HashSet};

const OPT_DEFINE_TEMPS_AT_USE: bool = false;

type FlowIter<'a> = std::iter::Peekable<control_flow::ControlFlowIter<'a>>;

#[derive(Debug, Clone)]
pub struct VarDecl {
  pub typ: Type,
  pub names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct VarMap {
  pub typ: Type,
  pub name: String,
  pub mapping_expr: Expr,
}

#[derive(Debug, Clone)]
pub enum Expr {
  Unary(Box<UnaryExpr>),
  Binary(Box<BinaryExpr>),
  HexConst(u16),
  DecimalConst(i16),
  Name(String),
  Call(Box<Expr>, Vec<Expr>),
  Abstract(&'static str, Vec<Expr>),
  ArrayAccess(Box<Expr>, Box<Expr>),
  StructAccess(Box<Expr>, Box<Expr>),
  Deref(Box<Expr>),
  GuestMem(Box<Expr>, Box<Expr>, u8),
  Cast(Type, Box<Expr>),
  UnimplPhi,
  UnimplPin,
}

#[derive(Debug, Clone)]
pub enum UnaryOperator {
  Addr, Neg, LogicalNot, BitwiseNot,
}

impl UnaryOperator {
  pub fn as_operator_str(&self) -> &'static str {
    match self {
      UnaryOperator::Addr => "(u8*)&",
      UnaryOperator::Neg => "-",
      UnaryOperator::LogicalNot => "!",
      UnaryOperator::BitwiseNot => "~",
    }
  }
}

#[derive(Debug, Clone)]
pub struct UnaryExpr {
  pub op: UnaryOperator,
  pub rhs: Expr,
}

#[derive(Debug, Clone, Copy)]
pub enum BinaryOperator {
  Add, Sub, Shl, Shr, Mul, Div, Mod, And, Or, Xor, Eq, Neq, Gt, Geq, Lt, Leq,
}

impl BinaryOperator {
  pub fn as_operator_str(&self) -> &'static str {
    match self {
      BinaryOperator::Add => "+",
      BinaryOperator::Sub => "-",
      BinaryOperator::Shl => "<<",
      BinaryOperator::Shr => ">>",
      BinaryOperator::Mul => "*",
      BinaryOperator::Div => "/",
      BinaryOperator::Mod => "%",
      BinaryOperator::And => "&",
      BinaryOperator::Or  => "|",
      BinaryOperator::Xor => "^",
      BinaryOperator::Eq  => "==",
      BinaryOperator::Neq => "!=",
      BinaryOperator::Gt  => ">",
      BinaryOperator::Geq => ">=",
      BinaryOperator::Lt  => "<",
      BinaryOperator::Leq => "<=",
    }
  }

  fn invert(self) -> Option<Self> {
    match self {
      BinaryOperator::Eq => Some(BinaryOperator::Neq),
      BinaryOperator::Neq => Some(BinaryOperator::Eq),
      BinaryOperator::Gt => Some(BinaryOperator::Leq),
      BinaryOperator::Geq => Some(BinaryOperator::Lt),
      BinaryOperator::Lt => Some(BinaryOperator::Geq),
      BinaryOperator::Leq => Some(BinaryOperator::Gt),
      _ => None,
    }
  }
}

#[derive(Debug, Clone)]
pub struct BinaryExpr {
  pub op: BinaryOperator,
  pub lhs: Expr,
  pub rhs: Expr,
}

#[derive(Debug, Clone)]
pub struct Assign {
  pub decltype: Option<Type>,
  pub lhs: Expr,
  pub rhs: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Label(pub String); // fixme??

#[derive(Debug, Clone)]
pub struct CondGoto {
  pub cond: Expr,
  pub label_true: Label,
  pub label_false: Label,
}

#[derive(Debug, Clone)]
pub struct Goto {
  pub label: Label,
}

#[derive(Debug, Clone)]
pub enum ReturnType {
  Far,
  Near,
}

#[derive(Debug, Clone)]
pub struct Return {
  pub rt: ReturnType,
  pub vals: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub struct Loop {
  pub body: Block,
}

#[derive(Debug, Clone)]
pub struct If {
  pub cond: Expr,
  pub then_body: Block,
  pub else_body: Option<Block>,
}

#[derive(Debug, Clone)]
pub struct Switch {
  pub switch_val: Expr,
  pub cases: Vec<SwitchCase>,
  pub default: Option<Block>,
}

#[derive(Debug, Clone)]
pub struct SwitchCase {
  pub cases: Vec<Expr>,
  pub body: Block,
}

#[derive(Debug, Clone)]
pub enum Stmt {
  Label(Label),
  Instr(ir::Ref),
  Expr(Expr),
  Assign(Assign),
  CondGoto(CondGoto),
  Goto(Goto),
  Return(Return),
  Loop(Loop),
  If(If),
  Switch(Switch),
  Unreachable,
}

#[derive(Debug, Clone)]
pub struct Function {
  pub name: String,
  pub ret: Option<Type>,
  pub vardecls: Vec<VarDecl>,
  pub varmaps: Vec<VarMap>,
  pub frame_size: u16,
  pub body: Block,
}

#[derive(Debug, Default, Clone)]
pub struct Block(pub Vec<Stmt>);

struct Builder<'a> {
  cfg: &'a Config,
  ir: &'a ir::IR,
  cf: &'a ControlFlow,
  n_uses: HashMap<ir::Ref, usize>,
  temp_names: HashMap<ir::Ref, String>,
  temp_count: usize,
  frame_off_low: i16,  // less negative
  frame_off_high: i16, // more negative

  assigns: Vec<(String, Type)>,
  assigned: HashSet<String>,
  mappings: HashMap<String, (Type, Expr)>,
  layout_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GuestPointerOrigin {
  symbol: sym::SymbolRef,
  kind: GuestPtrKind,
  pointer_value: Option<ir::Ref>,
}

fn unary_expr(op: UnaryOperator, rhs: Expr) -> Expr {
  Expr::Unary(Box::new(UnaryExpr { op, rhs }))
}

fn binary_expr(op: BinaryOperator, lhs: Expr, rhs: Expr) -> Expr {
  Expr::Binary(Box::new(BinaryExpr { op, lhs, rhs }))
}

impl Block {
  fn push_stmt(&mut self, stmt: Stmt) {
    self.0.push(stmt);
  }
}

impl<'a> Builder<'a> {
  fn new(cfg: &'a Config, ir: &'a ir::IR, cf: &'a ControlFlow) -> Self {
    let n_uses = ir.compute_uses();
    Self {
      cfg,
      ir,
      cf,
      n_uses,
      temp_names: HashMap::new(),
      temp_count: 0,
      frame_off_low: i16::MIN+1,
      frame_off_high: 0,

      assigns: vec![],
      assigned: HashSet::new(),
      mappings: HashMap::new(),
      layout_error: None,
    }
  }

  fn lookup_uses(&self, r: ir::Ref) -> usize {
    *self.n_uses.get(&r).unwrap_or(&0)
  }

  fn ref_name(&mut self, r: ir::Ref) -> String {
    if let Some(n) = self.ir.names.get(&r) {
      return format!("{}_{}", n.0, n.1);
    }
    if let Some(n) = self.temp_names.get(&r) {
      return n.clone();
    }
    let name = format!("tmp_{}", self.temp_count);
    self.temp_count += 1;
    self.temp_names.insert(r, name.clone());
    name
  }

  fn ref_to_unary_expr(&mut self, r: ir::Ref, depth: usize, hex_const: bool, _inverted: &mut bool) -> Option<Expr> {
    let instr = self.ir.instr(r).unwrap();

    let (ast_op, signed) = match instr.opcode {
      ir::Opcode::Neg  => (UnaryOperator::Neg, false),
      ir::Opcode::Not  => (UnaryOperator::BitwiseNot, false),
      _ => return None,
    };

    // TODO: IMPLEMENT INVERTED FOR UNARY EXPR

    let mut rhs = self.ref_to_expr_hex(instr.operands[0], depth+1, hex_const);

    if signed {
      rhs = Expr::Cast(Type::I16, Box::new(rhs));
    }

    Some(unary_expr(ast_op, rhs))
  }

  fn ref_to_binary_expr(&mut self, r: ir::Ref, depth: usize, hex_const: bool, inverted: &mut bool) -> Option<Expr> {
    let instr = self.ir.instr(r).unwrap();

    let (mut ast_op, signed) = match instr.opcode {
      ir::Opcode::Add  => (BinaryOperator::Add,  false),
      ir::Opcode::Sub  => (BinaryOperator::Sub,  false),
      ir::Opcode::IMul => (BinaryOperator::Mul,  true),
      ir::Opcode::UMul => (BinaryOperator::Mul,  false),
      ir::Opcode::IDiv => (BinaryOperator::Div,  true),
      ir::Opcode::UDiv => (BinaryOperator::Div,  false),
      ir::Opcode::And  => (BinaryOperator::And,  false),
      ir::Opcode::Or   => (BinaryOperator::Or,   false),
      ir::Opcode::Xor  => (BinaryOperator::Xor,  false),
      ir::Opcode::Shl  => (BinaryOperator::Shl,  false),
      ir::Opcode::Shr  => (BinaryOperator::Shr,  true),
      ir::Opcode::UShr => (BinaryOperator::Shr,  false),
      ir::Opcode::Eq   => (BinaryOperator::Eq,   false),
      ir::Opcode::Neq  => (BinaryOperator::Neq,  false),
      ir::Opcode::Gt   => (BinaryOperator::Gt,   true),
      ir::Opcode::Geq  => (BinaryOperator::Geq,  true),
      ir::Opcode::Lt   => (BinaryOperator::Lt,   true),
      ir::Opcode::Leq  => (BinaryOperator::Leq,  true),
      ir::Opcode::UGt  => (BinaryOperator::Gt,   false),
      ir::Opcode::UGeq => (BinaryOperator::Geq,  false),
      ir::Opcode::ULt  => (BinaryOperator::Lt,   false),
      ir::Opcode::ULeq => (BinaryOperator::Leq,  false),
      _ => return None,
    };

    // Try to invert the operation if requested
    if *inverted {
      if let Some(op) = ast_op.invert() {
        *inverted = false;
        ast_op = op;
      }
    }

    let mut lhs = self.ref_to_expr_hex(instr.operands[0], depth+1, hex_const);
    let mut rhs = self.ref_to_expr_hex(instr.operands[1], depth+1, hex_const);

    if signed {
      lhs = Expr::Cast(Type::I16, Box::new(lhs));
      rhs = Expr::Cast(Type::I16, Box::new(rhs));
    }

    Some(Expr::Binary(Box::new(BinaryExpr {
      op: ast_op,
      lhs,
      rhs,
    })))
  }

  fn ref_to_expr(&mut self, r: ir::Ref, depth: usize) -> Expr {
    self.ref_to_expr_2(r, depth, false)
  }

  fn ref_to_expr_hex(&mut self, r: ir::Ref, depth: usize, hex_const: bool) -> Expr {
    let mut inverted = false;
    self.ref_to_expr_impl(r, depth, hex_const, &mut inverted)
  }

  // FIXME: CLEANUP AND RENAME
  fn ref_to_expr_2(&mut self, r: ir::Ref, depth: usize, mut inverted: bool) -> Expr {
    let expr = self.ref_to_expr_impl(r, depth, false, &mut inverted);
    let expr = if inverted {
      Expr::Unary(Box::new(UnaryExpr{op: UnaryOperator::LogicalNot, rhs: expr}))
    } else {
      expr
    };
    expr
  }

  // depth==0 instruction itself (must generate)
  // depth==1 operand of another instruction (may generate)
  // FIXME: CLEANUP AND RENAME
  fn ref_to_expr_impl(&mut self, r: ir::Ref, depth: usize, hex_const: bool, inverted: &mut bool) -> Expr {
    match self.ir.const_lookup(r) {
      Some(k) => {
        if hex_const || k >= 256 || k <= -256 {
          return Expr::HexConst(k as u16);
        } else {
          return Expr::DecimalConst(k as i16);
        }
      }
      None => (),
    }
    if let ir::Ref::Init(reg) = r {
      let name = if reg == crate::asm::instr::Reg::SP {
        "SP0".to_string()
      } else {
        reg.info().name.to_string()
      };
      return Expr::Name(name);
    }

    let instr = self.ir.instr(r).unwrap();
    if depth != 0 && (self.lookup_uses(r) != 1 || instr.opcode.is_call()) {
      let name = self.ref_name(r);
      return Expr::Name(name);
    }

    assert!(matches!(r, ir::Ref::Instr(_, _)));
    if let Some(expr) = self.ref_to_unary_expr(r, depth, hex_const, inverted) {
      return expr;
    }
    if let Some(expr) = self.ref_to_binary_expr(r, depth, hex_const, inverted) {
      return expr;
    }

    match instr.opcode {
      ir::Opcode::Ref => {
        self.ref_to_expr(instr.operands[0], depth+1)
      }
      ir::Opcode::Pin => {
        // Pin marks a value for register allocation; transparent in C.
        self.ref_to_expr(instr.operands[0], depth+1)
      }
      ir::Opcode::Load8 => {
        if let Some(origin) = self.guest_pointer_origin(instr.operands[1], 0)
          .filter(|origin| self.guest_pointer_pair_matches(*origin, instr.operands[0])) {
          let (seg, off) = self.guest_pointer_address(origin, instr.operands[0], instr.operands[1], depth);
          return Expr::GuestMem(Box::new(seg), Box::new(off), 1);
        }
        let seg = self.ref_to_expr_hex(instr.operands[0], depth+1, true);
        let off = self.ref_to_expr_hex(instr.operands[1], depth+1, true);
        Expr::Deref(Box::new(Expr::Abstract("PTR_8", vec![seg, off])))
      }
      ir::Opcode::Load16 => {
        if let Some(origin) = self.guest_pointer_origin(instr.operands[1], 0)
          .filter(|origin| self.guest_pointer_pair_matches(*origin, instr.operands[0])) {
          let (seg, off) = self.guest_pointer_address(origin, instr.operands[0], instr.operands[1], depth);
          return Expr::GuestMem(Box::new(seg), Box::new(off), 2);
        }
        let seg = self.ref_to_expr_hex(instr.operands[0], depth+1, true);
        let off = self.ref_to_expr_hex(instr.operands[1], depth+1, true);
        Expr::Deref(Box::new(Expr::Abstract("PTR_16", vec![seg, off])))
      }
      ir::Opcode::Load32 => {
        if let Some(origin) = self.guest_pointer_origin(instr.operands[1], 0)
          .filter(|origin| self.guest_pointer_pair_matches(*origin, instr.operands[0])) {
          let (seg, off) = self.guest_pointer_address(origin, instr.operands[0], instr.operands[1], depth);
          return Expr::GuestMem(Box::new(seg), Box::new(off), 4);
        }
        let seg = self.ref_to_expr_hex(instr.operands[0], depth+1, true);
        let off = self.ref_to_expr_hex(instr.operands[1], depth+1, true);
        Expr::Deref(Box::new(Expr::Abstract("PTR_32", vec![seg, off])))
      }
      ir::Opcode::Upper16 => {
        let lhs = self.ref_to_expr_hex(instr.operands[0], depth+1, hex_const);
        Expr::Cast(Type::U16, Box::new(Expr::Binary(Box::new(BinaryExpr {
          op: BinaryOperator::Shr,
          lhs,
          rhs: Expr::DecimalConst(16),
        }))))
      }
      ir::Opcode::Lower16 => {
        let lhs = self.ref_to_expr_hex(instr.operands[0], depth+1, hex_const);
        Expr::Cast(Type::U16, Box::new(lhs))
      }
      ir::Opcode::ReadVar8 => {
        self.symbol_to_expr(instr.operands[0].unwrap_symbol())
      }
      ir::Opcode::ReadVar16 => {
        self.symbol_to_expr(instr.operands[0].unwrap_symbol())
      }
      ir::Opcode::ReadVar32 => {
        self.symbol_to_expr(instr.operands[0].unwrap_symbol())
      }
      ir::Opcode::CallArgs => {
        let funcidx = instr.operands[0].unwrap_func();
        let funcname = self.ir.funcs[funcidx].clone();
        let mut args = vec![];
        for a in &instr.operands[1..] {
          args.push(self.ref_to_expr(*a, depth+1));
        }
        Expr::Call(Box::new(Expr::Name(funcname)), args)
      }
      ir::Opcode::CallFar => {
        let exprs: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr_hex(*r, depth+1, true)).collect();
        Expr::Abstract("CALL_FAR", exprs)
      }
      ir::Opcode::CallNear => {
        let exprs: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr_hex(*r, depth+1, true)).collect();
        Expr::Abstract("CALL_NEAR", exprs)
      }
      ir::Opcode::CallPtr => {
        let exprs: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr_hex(*r, depth+1, true)).collect();
        Expr::Abstract("CALL_FAR_INDIRECT", exprs)
      }
      ir::Opcode::Phi => {
        // generally handled by jmp, but other expressions that use a phi might end up here
        // so, we can simply return our refname
        Expr::Name(self.ref_name(r))
      }
      ir::Opcode::Make32 => {
        let exprs: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr(*r, depth+1)).collect();
        Expr::Abstract("MAKE_32", exprs)
      }
      ir::Opcode::SignExtTo16 => {
        assert!(instr.operands.len() == 1);
        let rhs = self.ref_to_expr(instr.operands[0], depth+1);
        // TODO: VERIFY THIS IS A U16
        Expr::Cast(Type::I16, Box::new(Expr::Cast(Type::I8, Box::new(rhs))))
      }
      ir::Opcode::SignExtTo32 => {
        assert!(instr.operands.len() == 1);
        let rhs = self.ref_to_expr(instr.operands[0], depth+1);
        // TODO: VERIFY THIS IS A U16
        Expr::Cast(Type::I32, Box::new(Expr::Cast(Type::I16, Box::new(rhs))))
      }
      ir::Opcode::Sign => {
        let lhs = self.ref_to_expr(instr.operands[0], depth+1);
        binary_expr(BinaryOperator::Neq,
                    binary_expr(BinaryOperator::Shr, lhs, Expr::DecimalConst(15)),
                    Expr::DecimalConst(0))
      }
      ir::Opcode::NotSign => {
        let lhs = self.ref_to_expr(instr.operands[0], depth+1);
        binary_expr(BinaryOperator::Eq,
                    binary_expr(BinaryOperator::Shr, lhs, Expr::DecimalConst(15)),
                    Expr::DecimalConst(0))
      }
      ir::Opcode::Unimpl => {
        let exprs: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr(*r, depth+1)).collect();
        Expr::Abstract("UNIMPL", exprs)
      }
      ir::Opcode::UpdateFlags => {
        // The flags value is consumed as data (e.g. `pushf`, or a flags
        // definition surviving optimization into a value position). There is
        // no C-level flags value, so mark the imprecision explicitly rather
        // than aborting the whole function.
        let exprs: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr(*r, depth+1)).collect();
        Expr::Abstract("UNIMPL_FLAGS", exprs)
      }
      ir::Opcode::EqFlags | ir::Opcode::NeqFlags | ir::Opcode::GtFlags | ir::Opcode::GeqFlags |
      ir::Opcode::LtFlags | ir::Opcode::LeqFlags | ir::Opcode::UGtFlags | ir::Opcode::UGeqFlags |
      ir::Opcode::ULtFlags | ir::Opcode::ULeqFlags | ir::Opcode::SignFlags => {
        // A flag test that survived `simplify_branch_conds` (e.g. its flag
        // producer is unknown or relational-over-non-logical). Conditions
        // built from these stay explicit in the C output via UNIMPL_FLAGS.
        let exprs: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr(*r, depth+1)).collect();
        Expr::Abstract("UNIMPL_FLAGS", exprs)
      }
      opcode @ _ => {
        panic!("Unimplemented {:?} in ast converter", opcode);
      }
    }
  }

  fn guest_pointer_origin(&self, r: ir::Ref, depth: usize) -> Option<GuestPointerOrigin> {
    let mut found = HashSet::new();
    let mut visited = HashMap::new();
    self.collect_guest_pointer_origins(r, depth, &mut found, &mut visited);
    if found.len() == 1 { found.into_iter().next() } else { None }
  }

  fn collect_guest_pointer_origins(&self, r: ir::Ref, depth: usize, found: &mut HashSet<GuestPointerOrigin>, visited: &mut HashMap<ir::Ref, usize>) {
    if depth > 24 { return; }
    // Depth-aware memo: the reachable-origin set is monotonic in `found`,
    // and exploration at depth d covers everything within 24-d steps, so a
    // ref previously explored at a shallower-or-equal depth is a superset
    // and never needs re-exploration. Without this, deep shared expression
    // DAGs (stack-pointer arithmetic chains) re-explore shared
    // subexpressions exponentially (branching up to depth 24). Stored depth
    // keeps this sound across the depth cutoff and cyclic use-chains.
    match visited.get(&r) {
      Some(prior) if *prior <= depth => return,
      _ => { visited.insert(r, depth); }
    }
    match r {
      ir::Ref::Symbol(symref) => if let Type::GuestPtr(_, kind) = symref.get_type(&self.ir.symbols) {
        found.insert(GuestPointerOrigin { symbol: symref, kind: *kind, pointer_value: None });
      } else if let Some(kind) = self.annotated_pointer_kind(symref) {
        found.insert(GuestPointerOrigin { symbol: symref, kind, pointer_value: None });
      },
      ir::Ref::Instr(_, _) => {
        let Some(i) = self.ir.instr(r) else { return; };
        if matches!(i.opcode, ir::Opcode::ReadVar8 | ir::Opcode::ReadVar16 | ir::Opcode::ReadVar32) {
          if let Some(ir::Ref::Symbol(symref)) = i.operands.first().copied() {
            let kind = match symref.get_type(&self.ir.symbols) {
              Type::GuestPtr(_, kind) => Some(*kind),
              _ => self.annotated_pointer_kind(symref),
            };
            if let Some(kind) = kind {
              found.insert(GuestPointerOrigin { symbol: symref, kind, pointer_value: Some(r) });
            }
          }
          return;
        }
        if matches!(i.opcode, ir::Opcode::Ref | ir::Opcode::Lower16 | ir::Opcode::Upper16) {
          if let Some(operand) = i.operands.first() { self.collect_guest_pointer_origins(*operand, depth+1, found, visited); }
          return;
        }
        for operand in &i.operands {
          self.collect_guest_pointer_origins(*operand, depth+1, found, visited);
        }
      }
      _ => (),
    }
  }

  fn annotated_pointer_kind(&self, symref: sym::SymbolRef) -> Option<GuestPtrKind> {
    if symref.region.off < 0 { return None; }
    self.find_pointer_kind(
      symref.get_type(&self.ir.symbols),
      symref.region.off as usize,
      symref.region.sz as usize,
      0,
    )
  }

  fn find_pointer_kind(&self, typ: &Type, off: usize, size: usize, depth: usize) -> Option<GuestPtrKind> {
    if depth > 24 { return None; }
    match typ {
      Type::GuestPtr(_, kind) if off == 0 && typ.size_in_bytes() == Some(size) => Some(*kind),
      Type::Array(base, ArraySize::Known(len)) => {
        let base_size = base.size_in_bytes()?;
        if base_size == 0 { return None; }
        let index = off / base_size;
        let inner_off = off % base_size;
        if index >= *len || inner_off + size > base_size { return None; }
        self.find_pointer_kind(base, inner_off, size, depth+1)
      }
      Type::Struct(struct_ref) => {
        let layout = self.cfg.types.lookup_struct(*struct_ref)?;
        let end = off.checked_add(size)?;
        for member in &layout.members {
          let member_start = member.off as usize;
          let member_end = member_start.checked_add(member.typ.size_in_bytes()?)?;
          if member_start <= off && end <= member_end {
            return self.find_pointer_kind(&member.typ, off-member_start, size, depth+1);
          }
        }
        None
      }
      _ => None,
    }
  }

  fn guest_pointer_pair_matches(&self, origin: GuestPointerOrigin, seg_ref: ir::Ref) -> bool {
    // Near pointers carry an implicit segment, so the IR segment must be the
    // register the annotation selects for. A mismatch (wrong annotation,
    // segment override, or reused offset expression) keeps the raw address
    // path instead of silently substituting a segment.
    use crate::asm::instr::Reg;
    match origin.kind {
      GuestPtrKind::Near => seg_ref == ir::Ref::Init(Reg::DS),
      GuestPtrKind::NearSs => seg_ref == ir::Ref::Init(Reg::SS),
      GuestPtrKind::NearEs => seg_ref == ir::Ref::Init(Reg::ES),
      GuestPtrKind::Far => self.guest_pointer_origin(seg_ref, 0).map(|seg_origin| seg_origin == origin && seg_origin.kind == GuestPtrKind::Far).unwrap_or(false),
    }
  }

  fn guest_pointer_address(&mut self, origin: GuestPointerOrigin, seg_ref: ir::Ref, off_ref: ir::Ref, depth: usize) -> (Expr, Expr) {
    let off_expr = self.ref_to_expr_hex(off_ref, depth+1, true);
    match origin.kind {
      GuestPtrKind::Near => (Expr::Name("DS".into()), off_expr),
      GuestPtrKind::NearSs => (Expr::Name("SS".into()), off_expr),
      GuestPtrKind::NearEs => (Expr::Name("ES".into()), off_expr),
      GuestPtrKind::Far => {
        (self.ref_to_expr_hex(seg_ref, depth+1, true), off_expr)
      }
    }
  }

  fn symbol_to_expr(&mut self, symref: sym::SymbolRef) -> Expr {
    let sym = symref.def(&self.ir.symbols);

    // grow the frame?
    if symref.table() == sym::Table::Local {

      let start_off = sym.off;

      let typ = symref.get_type(&self.ir.symbols);
      let sz: i16 = match typ {
        Type::U8 => 1,
        Type::U16 => 2,
        Type::U32 => 4,
        _ => panic!("Unsupported type: {:?}", typ),
      };

      // Overflow here would need a local within 4 bytes of i16::MAX, which no
      // bp-relative frame can produce; widen_frame (i32 comparisons) is what
      // guards the realistic hazard, a local at i16::MIN from `sub sp,0x8000`.
      let end_off = start_off.checked_add(sz)
        .unwrap_or_else(|| panic!("local extent overflows i16: start {} size {}", start_off, sz));
      let (low, high) = widen_frame(self.frame_off_low, self.frame_off_high, start_off, end_off);
      self.frame_off_low = low;
      self.frame_off_high = high;

      //println!("{} | start_off: {}, end_off: {}", sym.name, start_off, end_off);
    }

    if (symref.table() == sym::Table::Local || symref.table() == sym::Table::Param) &&
      self.mappings.get(&sym.name).is_none()
    {
      let ss = crate::asm::instr::Reg::SS;
      let sp = crate::asm::instr::Reg::SP;

      // Use SP0 instead of SP so that its immutable everywhere in the function
      let mut sp0 = sp.info().name.to_string();
      sp0.push('0');

      let seg = Expr::Name(ss.info().name.to_string());
      let off = Expr::Binary(Box::new(BinaryExpr {
        op: BinaryOperator::Add,
        lhs: Expr::Name(sp0),
        rhs: Expr::HexConst(sym.off as u16),
      }));

      // self.decls.push(VarDecl {
      //   typ: symref.to_type(),
      //   names: vec![sym.name.clone()],
      //   mem_mapping: Some(Expr::Deref(Box::new(Expr::Abstract("PTR_16", vec![seg, off])))),
      // })

      let typ = symref.get_type(&self.ir.symbols);
      let ptr_sz = match typ {
        Type::U8 => "PTR_8",
        Type::U16 => "PTR_16",
        Type::U32 => "PTR_32",
        _ => panic!("Unsupported type: {:?}", typ),
      };

      let impl_expr = Expr::Deref(Box::new(Expr::Abstract(ptr_sz, vec![seg, off])));
      let typ = symref.get_type(&self.ir.symbols).clone();
      self.mappings.insert(sym.name.clone(), (typ, impl_expr));
    }

    let expr = Expr::Name(sym.name.clone());
    let typ = symref.get_type(&self.ir.symbols);
    //println!("enter symbol_to_expr_recurse");
    let r = self.symbol_to_expr_recurse(expr, typ, symref.region, &sym.name);
    //println!("leave symbol_to_expr_recurse");
    r
  }

  fn symbol_to_expr_recurse(&mut self, mut expr: Expr, typ: &Type, mut access: sym::Region, symbol: &str) -> Expr {
    // FIXME: Unify this and the "access" code

    // println!("symbol_to_expr_recurse");
    // println!("  expr:   {:?}", expr);
    // println!("  type:   {:?}", typ);
    // println!("  access: {:?}", access);
    if !typ.is_primitive() {
      match typ {
        Type::Array(basetype, len) => {
          let ArraySize::Known(len) = len else {
            self.layout_error.get_or_insert_with(|| format!("Annotated access to {} uses an array with unknown bound ({})", symbol, typ));
            return expr;
          };
          let Some(basetype_sz) = basetype.size_in_bytes() else {
            self.layout_error.get_or_insert_with(|| format!("Cannot determine element size for annotated access to {} ({})", symbol, typ));
            return expr;
          };
          if basetype_sz == 0 || access.off < 0 {
            self.layout_error.get_or_insert_with(|| format!("Invalid byte offset {} for annotated access to {} ({})", access.off, symbol, typ));
            return expr;
          }
          let idx = access.off as usize / basetype_sz;
          let element_off = access.off as usize % basetype_sz;
          if idx >= *len || element_off + access.sz as usize > basetype_sz || access.off as usize + access.sz as usize > typ.size_in_bytes().unwrap_or(0) {
            self.layout_error.get_or_insert_with(|| format!("Access byte range {}..{} is outside annotated {} layout {}", access.off, access.off + access.sz as i32, symbol, typ));
            return expr;
          }

          let idx_expr = if idx <= i16::MAX as usize { Expr::DecimalConst(idx as i16) } else { Expr::HexConst(idx as u16) };
          let expr = Expr::ArrayAccess(
            Box::new(expr),
            Box::new(idx_expr));

          access.off -= (idx * basetype_sz) as i32;

          // recurse
          return self.symbol_to_expr_recurse(expr, basetype, access, symbol);
        }
        Type::Struct(struct_ref) => {
          let Some((access_start, access_end)) = checked_byte_range(access.off, access.sz) else {
            self.layout_error.get_or_insert_with(|| format!("Invalid byte offset {} for annotated access to {} ({})", access.off, symbol, typ));
            return expr;
          };
          let Some(s) = self.cfg.types.lookup_struct(*struct_ref) else {
            self.layout_error.get_or_insert_with(|| format!("Missing struct layout for annotated access to {} ({})", symbol, typ));
            return expr;
          };
          for mbr in &s.members {
            let mbr_start = mbr.off as usize;
            let Some(mbr_size) = mbr.typ.size_in_bytes() else {
              self.layout_error.get_or_insert_with(|| format!("Unknown member size for annotated access to {}.{}", symbol, mbr.name));
              return expr;
            };
            let mbr_end = mbr_start + mbr_size;
            if !(mbr_start <= access_start && access_end <= mbr_end) { continue; }

            // Found!!
            let expr = Expr::StructAccess(
              Box::new(expr),
              Box::new(Expr::Name(mbr.name.clone())));

            access.off -= mbr.off as i32;

            // recurse
            return self.symbol_to_expr_recurse(expr, &mbr.typ, access, symbol);
          }
          self.layout_error.get_or_insert_with(|| format!("Access byte range {}..{} does not fit any member of annotated {} ({})", access.off, access.off + access.sz as i32, symbol, typ));
          return expr;
        }
        Type::GuestPtr(_, _) => {
          // A pointer value itself is represented by its guest-width integer storage.
          if access.off != 0 || access.sz as usize != typ.size_in_bytes().unwrap_or(0) {
            self.layout_error.get_or_insert_with(|| format!("Partial access to annotated pointer {} is unsupported", symbol));
          }
          return expr;
        }
        _ => {
          self.layout_error.get_or_insert_with(|| format!("Unsupported annotated layout type for {} ({})", symbol, typ));
          return expr;
        }
      }
    }

    // Base-case of a primitive
    let Some(typ_size) = typ.size_in_bytes() else {
      self.layout_error.get_or_insert_with(|| format!("Unknown annotated access size for {} ({})", symbol, typ));
      return expr;
    };
    if access.off != 0 || access.sz as usize != typ_size {
      expr = Expr::Unary(Box::new(UnaryExpr {
        op: UnaryOperator::Addr,
        rhs: expr,
      }));
      if access.off != 0 {
        expr = Expr::Binary(Box::new(BinaryExpr {
          op: BinaryOperator::Add,
          lhs: expr,
          rhs: Expr::HexConst(access.off as u16),
        }));
      }
      let t = match access.sz {
        1 => Type::U8,
        2 => Type::U16,
        4 => Type::U32,
        _ => {
          self.layout_error.get_or_insert_with(|| format!("Unsupported access width {} for annotated {} ({})", access.sz, symbol, typ));
          return expr;
        },
      };
      expr = Expr::Cast(Type::ptr(t), Box::new(expr));
      expr = Expr::Deref(Box::new(expr));
    }
    expr
  }

  fn make_label(&self, id: ElemId) -> Label {
    // The target may have been folded into a structured elem; descend to its
    // entry basic block, mirroring label_blocks_by_demand. (Goto chains are
    // cut by the visited set; such IR is already degenerate.)
    let mut id = id;
    let mut seen = HashSet::new();
    let bb = loop {
      if !seen.insert(id) {
        panic!("Cyclic goto chain while resolving label for {:?}", id);
      }
      let elem = self.cf.elem(id);
      match &elem.detail {
        Detail::BasicBlock(bb) => break bb,
        Detail::Goto(g) => { id = g.target; }
        Detail::ElemBlock(e) => { id = e.entry; }
        Detail::Loop(l) => { id = l.entry; }
        Detail::If(i) => { id = i.entry; }
        Detail::Switch(s) => { id = s.entry; }
      }
    };
    Label(format!("{}", self.ir.block(bb.blkref).name))
  }

  fn assign(&mut self, blk: &mut Block, typ: Type, name: &str, rhs: Expr) {
    let decltype = if OPT_DEFINE_TEMPS_AT_USE {
      Some(typ)
    } else {
      if self.assigned.get(name).is_none() {
        self.assigns.push((name.to_string(), typ));
        self.assigned.insert(name.to_string());
      }
      None
    };

    blk.push_stmt(Stmt::Assign(Assign {
      decltype,
      lhs: Expr::Name(name.to_string()),
      rhs,
    }));
  }

  fn emit_phis(&mut self, blk: &mut Block, src: ir::BlockRef, dst: ir::BlockRef) {
    // first, which pred is the src block?
    let mut idx = None;
    for (i, pred) in self.ir.block(dst).preds.iter().enumerate() {
      if *pred == src {
        idx = Some(i);
        break;
      }
    }
    let idx = idx.unwrap();

    // next, for each phi, generate code for the pred idx
    for r in self.ir.iter_instrs(dst) {
      let instr = self.ir.instr(r).unwrap();
      if instr.opcode != ir::Opcode::Phi { continue };

      let name = self.ref_name(r);
      let rvalue = self.ref_to_expr(instr.operands[idx], 1);
      self.assign(blk, instr.typ.clone(), &name, rvalue);
    }
  }

  // Returns a jump condition expr if the block ends in a conditional branch
  #[must_use]
  fn emit_blk(&mut self, blk: &mut Block, bref: ir::BlockRef, inverted_cond: bool) -> Option<Expr> {
    // An empty block is a (tail-)jump whose target lies outside the
    // decompiled range: control flow leaves the function here. It carries no
    // statements; any edge is supplied by the caller's control-flow Jump.
    if self.ir.block_instr_count(bref) == 0 {
      return None;
    }
    for r in self.ir.iter_instrs(bref) {
      let instr = self.ir.instr(r).unwrap();
      match instr.opcode {
        ir::Opcode::Nop => continue,
        ir::Opcode::Phi => continue, // handled by jmp
        ir::Opcode::Pin => continue, // ignored
        ir::Opcode::RetFar => {
          let vals: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr(*r, 1)).collect();
          blk.push_stmt(Stmt::Return(Return{rt: ReturnType::Far, vals}));
          return None;
        }
        ir::Opcode::RetNear => {
          let vals: Vec<_> = instr.operands.iter().map(|r| self.ref_to_expr(*r, 1)).collect();
          blk.push_stmt(Stmt::Return(Return{rt: ReturnType::Near, vals}));
          return None;
        }
        ir::Opcode::Jmp => {
          let ir::Ref::Block(dst) = instr.operands[0] else { panic!("Expected block ref for jmp instr") };
          self.emit_phis(blk, bref, dst);
          return None;
        }
        ir::Opcode::Jne => {
          // TODO: Maybe verify that there are no phis in the target block? This should be gaurenteed by
          // the ir finalize, but it's probably good to do sanity checks
          let cond = self.ref_to_expr_2(instr.operands[0], 1, inverted_cond);
          return Some(cond);
        }
        ir::Opcode::JmpTbl => {
          // TODO: Maybe verify that there are no phis in the target block? This should be gaurenteed by
          // the ir finalize, but it's probably good to do sanity checks
          let idx = self.ref_to_expr(instr.operands[0], 1);
          return Some(idx);
        }
        ir::Opcode::WriteVar8 => {
          let lhs = self.symbol_to_expr(instr.operands[0].unwrap_symbol());
          let rhs = self.ref_to_expr(instr.operands[1], 1);
          blk.push_stmt(Stmt::Assign(Assign { decltype: None, lhs, rhs }));
        }
        ir::Opcode::WriteVar16 => {
          let lhs = self.symbol_to_expr(instr.operands[0].unwrap_symbol());
          let rhs = self.ref_to_expr(instr.operands[1], 1);
          blk.push_stmt(Stmt::Assign(Assign { decltype: None, lhs, rhs }));
        }
        ir::Opcode::WriteVar32 => {
          let lhs = self.symbol_to_expr(instr.operands[0].unwrap_symbol());
          let rhs = self.ref_to_expr(instr.operands[1], 1);
          blk.push_stmt(Stmt::Assign(Assign { decltype: None, lhs, rhs }));
        }
        ir::Opcode::Store8 => {
          let lhs = if let Some(origin) = self.guest_pointer_origin(instr.operands[1], 0)
            .filter(|origin| self.guest_pointer_pair_matches(*origin, instr.operands[0])) {
            let (seg, off) = self.guest_pointer_address(origin, instr.operands[0], instr.operands[1], 1);
            Expr::GuestMem(Box::new(seg), Box::new(off), 1)
          } else {
            let seg = self.ref_to_expr_hex(instr.operands[0], 1, true);
            let off = self.ref_to_expr_hex(instr.operands[1], 1, true);
            Expr::Deref(Box::new(Expr::Abstract("PTR_8", vec![seg, off])))
          };
          let rhs = self.ref_to_expr(instr.operands[2], 1);
          blk.push_stmt(Stmt::Assign(Assign { decltype: None, lhs, rhs }));
        }
        ir::Opcode::Store16 => {
          let lhs = if let Some(origin) = self.guest_pointer_origin(instr.operands[1], 0)
            .filter(|origin| self.guest_pointer_pair_matches(*origin, instr.operands[0])) {
            let (seg, off) = self.guest_pointer_address(origin, instr.operands[0], instr.operands[1], 1);
            Expr::GuestMem(Box::new(seg), Box::new(off), 2)
          } else {
            let seg = self.ref_to_expr_hex(instr.operands[0], 1, true);
            let off = self.ref_to_expr_hex(instr.operands[1], 1, true);
            Expr::Deref(Box::new(Expr::Abstract("PTR_16", vec![seg, off])))
          };
          let rhs = self.ref_to_expr(instr.operands[2], 1);
          blk.push_stmt(Stmt::Assign(Assign { decltype: None, lhs, rhs }));
        }
        ir::Opcode::AssertEven => {
          let val = self.ref_to_expr(instr.operands[0], 1);
          let cond = Expr::Binary(Box::new(BinaryExpr {
            op: BinaryOperator::Eq,
            lhs: Expr::Binary(Box::new(BinaryExpr {
              op: BinaryOperator::Mod,
              lhs: val,
              rhs: Expr::DecimalConst(2),
            })),
            rhs: Expr::DecimalConst(0),
          }));
          blk.push_stmt(Stmt::Expr(Expr::Abstract("assert", vec![cond])));
        }
        ir::Opcode::AssertPos => {
          let val = self.ref_to_expr(instr.operands[0], 1);
          let cond = Expr::Binary(Box::new(BinaryExpr {
            op: BinaryOperator::Geq,
            lhs: Expr::Cast(Type::I16, Box::new(val)),
            rhs: Expr::DecimalConst(0),
          }));
          blk.push_stmt(Stmt::Expr(Expr::Abstract("assert", vec![cond])));
        }
        ir::Opcode::Int => {
          let num = self.ref_to_expr_hex(instr.operands[0], 0, true);
          blk.push_stmt(Stmt::Expr(Expr::Abstract("INT", vec![num])));
        }
        _ => {
          let uses = self.n_uses.get(&r).cloned().unwrap_or(0);
          if uses != 1 || instr.opcode.is_call() {
            let rvalue = self.ref_to_expr(r, 0);
            let typ = self.ir.instr(r).unwrap().typ.clone();
            if typ == Type::Void {
              blk.push_stmt(Stmt::Expr(rvalue));
            } else {
              let name = self.ref_name(r);
              self.assign(blk, typ, &name, rvalue);
            }
          }
        }
      }
    }
    // No terminator found (e.g. a stub ending in a call whose fallthrough
    // jump was eliminated as dead): control leaves the function here, so
    // there is no branch condition to return. Consistent with
    // block_exits() returning no exits for such blocks.
    None
  }

  fn emit_jump(&mut self, blk: &mut Block, jump: control_flow::Jump, cond: Option<Expr>) {
    match jump {
      control_flow::Jump::None => (),
      control_flow::Jump::UncondFallthrough => (),
      control_flow::Jump::UncondTarget(tgt) => {
        let label = self.make_label(tgt);
        blk.push_stmt(Stmt::Goto(Goto { label }));
      }
      control_flow::Jump::CondTargetTrue(tgt) => {
        // Unreachable by construction: only basic blocks carry conditional
        // jumps, and a block with a conditional jump has two exits, so it is
        // non-empty and ends in the conditional branch emitted above. Kept as
        // belt-and-braces; guessing a condition would be worse.
        let cond = cond.unwrap_or_else(|| Expr::Abstract("UNIMPL", vec![]));
        let label = self.make_label(tgt);
        let goto = Stmt::Goto(Goto{label});
        let then_body = Block(vec![goto]);
        blk.push_stmt(Stmt::If(If {cond, then_body, else_body: None }));
      }
      control_flow::Jump::CondTargetFalse(tgt) => {
        // Unreachable by construction: see CondTargetTrue above.
        let cond = cond.unwrap_or_else(|| Expr::Abstract("UNIMPL", vec![]));
        let label = self.make_label(tgt);
        let goto = Stmt::Goto(Goto{label});
        let then_body = Block(vec![goto]);
        // NOTE: cond already inverted before call!
        blk.push_stmt(Stmt::If(If {cond: cond, then_body, else_body: None }));
      }
      control_flow::Jump::CondTargetBoth(tgt_true, tgt_false) => {
        // Unreachable by construction: see CondTargetTrue above.
        let cond = cond.unwrap_or_else(|| Expr::Abstract("UNIMPL", vec![]));
        let label_true = self.make_label(tgt_true);
        let label_false = self.make_label(tgt_false);
        blk.push_stmt(Stmt::CondGoto(CondGoto { cond, label_true, label_false }));
      }
      control_flow::Jump::Table(_tgts) => {
        panic!("All JumpTable should be converted to Switch in control flow analysis");
      }
      //control_flow::Jump::Continue => {}
    }
  }

  fn convert_basic_block(&mut self, blk: &mut Block, iter: &mut FlowIter, _depth: usize) {
    let Some(bb_elt) = iter.next() else { panic!("expected basic block element") };
    let Detail::BasicBlock(bb) = &bb_elt.elem.detail else { panic!("expected basic block element") };

    if bb.labeled {
      let label = self.make_label(bb_elt.id);
      blk.push_stmt(Stmt::Label(label));
    }

    let jump = bb_elt.elem.jump.clone().unwrap();
    let cond = self.emit_blk(blk, bb.blkref, jump.cond_inverted());
    self.emit_jump(blk, jump, cond);
  }

  fn convert_loop(&mut self, blk: &mut Block, iter: &mut FlowIter, depth: usize) {
    let Some(loop_elt) = iter.next() else { panic!("expected loop element") };
    let Detail::Loop(_) = &loop_elt.elem.detail else { panic!("expected loop element") };

    let body = self.convert_body(iter, depth+1);
    blk.push_stmt(Stmt::Loop(Loop { body }));
    self.emit_jump(blk, loop_elt.elem.jump.clone().unwrap(), None);
  }

  fn convert_ifstmt(&mut self, blk: &mut Block, iter: &mut FlowIter, depth: usize) {
    let Some(ifstmt_elt) = iter.next() else { panic!("expected ifstmt element") };
    let Detail::If(ifstmt) = &ifstmt_elt.elem.detail else { panic!("expected ifstmt element") };

    let Detail::BasicBlock(bb) = &self.cf.elem(ifstmt.entry).detail else { panic!("expected ifstmt entry to be a basic-block") };
    if bb.labeled {
      let label = self.make_label(ifstmt.entry);
      blk.push_stmt(Stmt::Label(label));
    }
    let cond = match self.emit_blk(blk, bb.blkref, ifstmt.inverted) {
      Some(cond) => cond,
      // Unreachable by construction: infer_if requires exactly two exits, so
      // the entry block is non-empty and ends in the conditional branch
      // emitted above. Kept as belt-and-braces.
      None => Expr::Abstract("UNIMPL", vec![]),
    };

    let has_else = ifstmt.else_body.is_some();
    // The iterator gives sibling if arms the same depth. Stop the then-arm
    // explicitly at the first element laid out in the else-arm, otherwise
    // convert_body consumes both streams as one body.
    let else_start = ifstmt.else_body.as_ref()
      .and_then(|else_body| else_body.layout.first().copied());
    let then_body = self.convert_body_until(iter, depth+1, else_start);
    let else_body = if has_else {
      Some(self.convert_body(iter, depth+1))
    } else {
      None
    };
    blk.push_stmt(Stmt::If(If { cond, then_body, else_body }));
    self.emit_jump(blk, ifstmt_elt.elem.jump.clone().unwrap(), None);
  }

  fn convert_switch(&mut self, blk: &mut Block, iter: &mut FlowIter, depth: usize) {
    let Some(sw_elt) = iter.next() else { panic!("expected switch element") };
    let Detail::Switch(sw) = &sw_elt.elem.detail else { panic!("expected switch element") };

    let Detail::BasicBlock(bb) = &self.cf.elem(sw.entry).detail else { panic!("expected switch entry to be a basic-block") };
    if bb.labeled {
      let label = self.make_label(sw.entry);
      blk.push_stmt(Stmt::Label(label));
    }
    let Some(select) = self.emit_blk(blk, bb.blkref, false) else {
      panic!("expected switch entry to end in a jump table idx expr");
    };

    let mut cases = vec![];
    let mut map: HashMap<Label, usize> = HashMap::new(); // Label -> case-idx

    let mut idx = 0;
    while let Some(elt) = iter.peek() {
      if elt.depth <= depth {
        break;
      }
      assert!(elt.depth == depth+1);
      let elt = iter.next().unwrap();

      match &elt.elem.detail {
        Detail::Goto(g) => {
          let label = self.make_label(g.target);
          let case_idx = map.get(&label).cloned().unwrap_or_else(|| {
            let case_idx = cases.len();
            map.insert(label.clone(), case_idx);
            let mut body = Block::default();
            body.push_stmt(Stmt::Goto(Goto { label }));
            cases.push(SwitchCase {
              cases: vec![],
              body,
            });
            case_idx
          });
          cases[case_idx].cases.push(Expr::DecimalConst(idx as i16));
        }
        Detail::ElemBlock(_) => {
          let body = self.convert_body(iter, elt.depth+1);
          cases.push(SwitchCase {
            cases: vec![Expr::DecimalConst(idx as i16)],
            body,
          });
        }
        _ => panic!("Unexpected elem detail type in switch body: {:?}", elt.elem),
      }

      idx += 1;
    }

    let mut default = Block::default();
    default.push_stmt(Stmt::Unreachable);

    blk.push_stmt(Stmt::Switch(Switch {
      switch_val: select,
      cases,
      default: Some(default),
    }));
  }

  fn convert_body(&mut self, iter: &mut FlowIter, depth: usize) -> Block {
    self.convert_body_until(iter, depth, None)
  }

  fn convert_body_until(&mut self, iter: &mut FlowIter, depth: usize,
                        stop_at: Option<ElemId>) -> Block {
    let mut blk = Block::default();

    while let Some(elt) = iter.peek() {
      if elt.depth == depth && Some(elt.id) == stop_at {
        break;
      }
      assert!(elt.depth <= depth);
      if elt.depth < depth {
        break;
      }
      match &elt.elem.detail {
        Detail::BasicBlock(_) => self.convert_basic_block(&mut blk, iter, depth),
        Detail::Loop(_) => self.convert_loop(&mut blk, iter, depth),
        Detail::If(_) => self.convert_ifstmt(&mut blk, iter, depth),
        Detail::Switch(_) => self.convert_switch(&mut blk, iter, depth),
        _ => panic!("Unknown detail type: {:?}", elt.elem.detail),
      };
    }

    blk
  }

  fn build(&mut self, name: &str, ret: Option<Type>) -> Result<Function, String> {
    let mut iter = self.cf.iter().peekable();
    let body = self.convert_body(&mut iter, 0);
    assert!(iter.next().is_none());
    if let Some(err) = self.layout_error.take() { return Err(err); }

    // Group all decls by type to save codegen space
    // let mut type_map: HashMap<Type, usize> = HashMap::new();
    // let mut decls = vec![];
    // let mut mem_mapped: HashMap<String, VarDecl> = HashMap::new();
    // for d in &self.decls {
    //   if d.mem_mapping.is_some() {
    //     assert!(d.names.len() == 1);
    //     let name = &d.names[0];
    //     if mem_mapped.get(name).is_none() {
    //       mem_mapped.insert(name.clone(), d.clone());
    //     }
    //     continue;
    //   }
    //   let idx = match type_map.get(&d.typ) {
    //     Some(idx) => *idx,
    //     None => {
    //       let idx = decls.len();
    //       decls.push(VarDecl { typ: d.typ.clone(), names: vec![], mem_mapping: None });
    //       type_map.insert(d.typ.clone(), idx);
    //       idx
    //     }
    //   };
    //   for n in &d.names {
    //     decls[idx].names.push(n.clone());
    //   }
    // }
    // for n in itertools::sorted(mem_mapped.keys()) {
    //   decls.push(mem_mapped.get(n).unwrap().clone());
    // }

    let mut vardecls = vec![];
    let mut type_map: HashMap<Type, usize> = HashMap::new();
    for (name, typ) in std::mem::replace(&mut self.assigns, vec![]) {
      let idx = match type_map.get(&typ) {
        Some(idx) => *idx,
        None => {
          let idx = vardecls.len();
          vardecls.push(VarDecl { typ: typ.clone(), names: vec![] });
          type_map.insert(typ, idx);
          idx
        }
      };
      vardecls[idx].names.push(name);
    }

    let mut varmaps = vec![];
    for name in itertools::sorted(self.mappings.keys()) {
      let (typ, expr) = self.mappings.get(name).unwrap();
      varmaps.push(VarMap {
        typ: typ.clone(),
        name: name.to_string(),
        mapping_expr: expr.clone(),
      });
    }

    //println!("low_off: {}, high_off: {}", self.frame_off_low, self.frame_off_high);

    // Ground the frame in every stack slot seen during symbolization, not
    // just slots surviving into expressions: optimizer passes routinely
    // eliminate push-save stores/loads, which would otherwise leave a ragged
    // frame whose shallowest surviving local ends below the return-address
    // slot. Table types come from infer_type_from_size, so only 1/2/4.
    let (frame_off_low, frame_off_high, frame_size) =
      ground_frame(self.frame_off_low, self.frame_off_high, self.ir.symbols.local_extents().into_iter())?;
    self.frame_off_low = frame_off_low;
    self.frame_off_high = frame_off_high;

    Ok(Function {
      name: name.to_string(),
      ret,
      vardecls,
      varmaps,
      frame_size,
      body,
    })
  }
}

impl Function {
  pub fn from_ir(cfg: &Config, name: &str, ret: Option<Type>, ir: &ir::IR, ctrlflow: &ControlFlow) -> Result<Self, String> {
    Builder::new(cfg, ir, ctrlflow).build(name, ret)
  }
}

/// Converts a signed byte offset plus access size into a checked `start..end`
/// byte range. Returns `None` for negative offsets or on overflow, so callers
/// never perform a wrapping `as usize` cast on untrusted offsets.
fn checked_byte_range(off: i32, sz: u16) -> Option<(usize, usize)> {
  if off < 0 { return None; }
  let start = off as usize;
  Some((start, start.checked_add(sz as usize)?))
}

/// Widens a frame's (low, high) offsets to cover one local extent.
///
/// `low` is the least-negative (shallowest) local end offset seen so far and
/// `high` the most-negative (deepest) local start offset. Comparisons run in
/// i32 so an extent at i16::MIN (e.g. a `sub sp,0x8000` frame) cannot panic
/// on `abs()` the way a direct i16 `abs()` would in debug builds.
fn widen_frame(low: i16, high: i16, start: i16, end: i16) -> (i16, i16) {
  let high = if (start as i32).abs() > (high as i32).abs() { start } else { high };
  let low = if (end as i32).abs() < (low as i32).abs() { end } else { low };
  (low, high)
}

/// Grounds a stack frame in the extents of the function's locals.
///
/// `low`/`high` are seeded with the offsets accumulated while symbolizing
/// expressions; grounding over every local extent (including locals the
/// optimizer stripped from expressions) can only widen them, so the seeded
/// values are a lower bound on the frame. On return `low` is the least-negative
/// (shallowest) local end offset and `high` the most-negative (deepest) local
/// start offset; the returned size is the frame depth minus the 2-byte
/// return-address slot at [-2, 0).
///
/// Errors instead of panicking when the extents cannot form a valid frame, so
/// one malformed function degrades to a skipped function rather than aborting
/// a batch decompile.
fn ground_frame(
  mut low: i16,
  mut high: i16,
  extents: impl Iterator<Item = (i32, u16)>,
) -> Result<(i16, i16, u16), String> {
  for (start, sz) in extents {
    let start = i16::try_from(start)
      .map_err(|_| format!("frame local start offset out of i16 range: {}", start))?;
    let sz = i16::try_from(sz)
      .map_err(|_| format!("frame local size out of i16 range: {}", sz))?;
    let end = start.checked_add(sz)
      .ok_or_else(|| format!("frame local extent overflows i16: start {} size {}", start, sz))?;
    (low, high) = widen_frame(low, high, start, end);
  }

  if high == 0 { return Ok((low, high, 0)); } // no frame
  if low < -2 {
    // A local extends below the 2-byte return-address slot at [-2, 0).
    return Err(format!("frame offsets are below the return address location (-2): shallowest local ends at {}", low));
  }
  if low < high {
    return Err(format!("inconsistent frame extents: shallowest local end {} is below deepest local start {}", low, high));
  }
  let depth = (high as i32).abs();
  if depth < 2 {
    return Err(format!("frame too shallow for a return address slot: depth {}", depth));
  }
  Ok((low, high, (depth - 2) as u16))
}

#[cfg(test)]
mod tests {
  use super::{checked_byte_range, ground_frame, widen_frame};

  #[test]
  fn negative_offsets_are_rejected() {
    assert_eq!(checked_byte_range(-1, 2), None);
    assert_eq!(checked_byte_range(i32::MIN, u16::MAX), None);
  }

  #[test]
  fn valid_ranges_pass_through() {
    assert_eq!(checked_byte_range(0, 2), Some((0, 2)));
    assert_eq!(checked_byte_range(4, 2), Some((4, 6)));
  }

  #[test]
  fn overflowing_ranges_are_rejected() {
    // i32::MAX + u16::MAX only overflows usize on 32-bit targets; elsewhere it
    // must pass through with the exact sum.
    #[cfg(target_pointer_width = "32")]
    assert_eq!(checked_byte_range(i32::MAX, u16::MAX), None);
    #[cfg(not(target_pointer_width = "32"))]
    assert_eq!(
      checked_byte_range(i32::MAX, u16::MAX),
      Some((i32::MAX as usize, i32::MAX as usize + u16::MAX as usize))
    );
  }

  const NO_LOCALS: [(i32, u16); 0] = [];

  #[test]
  fn ground_frame_widens_to_eliminated_push_save_locals() {
    // Regression: a push-save frame where the optimizer eliminated the
    // si/di save stores leaves only the word locals [-6,-4) and [-8,-6) in
    // expressions, seeding low at -4 (which alone would trip the "below the
    // return address location" error). Grounding over every local extent --
    // including the surviving-in-symbols push saves at [-2,0) and [-4,-2) --
    // restores low = 0 and a well-formed frame.
    let extents = [(-2i32, 2u16), (-4, 2), (-6, 2), (-8, 2)];
    let (low, high, frame_size) =
      ground_frame(-4, -6, extents.into_iter()).expect("push-save-only frame");
    assert_eq!((low, high, frame_size), (0, -8, 6));
  }

  #[test]
  fn ground_frame_accepts_single_local() {
    // A word at [-2,0) is exactly the return-address slot: no room for locals
    // beyond it, so the frame is zero-sized but well-formed.
    let (low, high, frame_size) = ground_frame(i16::MIN + 1, 0, [(-2i32, 2u16)].into_iter()).unwrap();
    assert_eq!((low, high, frame_size), (0, -2, 0));
  }

  #[test]
  fn ground_frame_no_locals_means_no_frame() {
    assert_eq!(ground_frame(i16::MIN + 1, 0, NO_LOCALS.into_iter()).unwrap(), (i16::MIN + 1, 0, 0));
  }

  #[test]
  fn ground_frame_rejects_local_below_return_address_slot() {
    // A word at [-5,-3) ends below the 2-byte return-address slot at [-2,0).
    let err = ground_frame(i16::MIN + 1, 0, [(-5i32, 2u16)].into_iter()).unwrap_err();
    assert!(err.contains("below the return address location"), "unexpected error: {err}");
  }

  #[test]
  fn ground_frame_rejects_shallow_frame() {
    // A lone byte local at [-1,0) leaves no room for the return-address slot;
    // the pre-extraction code produced a garbage (wrapped) frame size here.
    let err = ground_frame(i16::MIN + 1, 0, [(-1i32, 1u16)].into_iter()).unwrap_err();
    assert!(err.contains("too shallow for a return address slot"), "unexpected error: {err}");
  }

  #[test]
  fn ground_frame_rejects_unrepresentable_extents() {
    // Out-of-i16-range extents must error, not wrap like the old `as i16` cast.
    let err = ground_frame(i16::MIN + 1, 0, [(-40000i32, 2u16)].into_iter()).unwrap_err();
    assert!(err.contains("start offset out of i16 range"), "unexpected error: {err}");
    let err = ground_frame(i16::MIN + 1, 0, [(-4i32, 40000u16)].into_iter()).unwrap_err();
    assert!(err.contains("size out of i16 range"), "unexpected error: {err}");
  }

  #[test]
  fn widen_frame_survives_i16_min_extent() {
    // Regression: a `sub sp,0x8000` frame puts a local at i16::MIN, whose
    // direct i16 abs() panics in debug builds. i32 comparisons must not.
    assert_eq!(widen_frame(i16::MIN + 1, 0, i16::MIN, i16::MIN + 2), (i16::MIN + 2, i16::MIN));
  }

  #[test]
  fn widen_frame_leaves_superset_unchanged() {
    assert_eq!(widen_frame(-2, -8, -6, -4), (-2, -8));
  }

  #[test]
  fn widen_frame_widens_both_directions() {
    assert_eq!(widen_frame(-4, -6, -10, -1), (-1, -10));
  }
}
