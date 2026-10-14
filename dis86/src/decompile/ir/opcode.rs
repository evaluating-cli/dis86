#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Opcode {
  Nop,
  Pin,
  Ref,
  Phi,
  Unimpl,

  Add,
  Sub,
  Shl,
  Shr,    // signed
  UShr,   // unsigned
  And,
  Or,
  Xor,
  IMul,  // signed
  UMul,  // unsigned
  IDiv,  // signed
  UDiv,  // unsigned

  Neg,
  Not,  // bitwise

  SignExtTo16,
  SignExtTo32,

  Load8,
  Load16,
  Load32,
  Store8,
  Store16,
  Store32,
  ReadVar8,
  ReadVar16,
  ReadVar32,
  WriteVar8,
  WriteVar16,
  WriteVar32,
  ReadArr8,
  ReadArr16,
  ReadArr32,
  WriteArr8,
  WriteArr16,
  Lower16,     // |n: u32| => n as u16
  Upper16,     // |n: u32| => (n >> 16) as u16
  Make32,      // |high: u16, low: u16| => (high as u32) << 16 | (low as u32)

  UpdateFlags,
  EqFlags,     // Maps to: JE  / JZ
  NeqFlags,    // Maps to: JNE / JNZ
  GtFlags,     // Maps to: JG  / JNLE
  GeqFlags,    // Maps to: JGE / JNL
  LtFlags,     // Maps to: JL  / JNGE
  LeqFlags,    // Maps to: JLE / JNG
  UGtFlags,    // Maps to: JA  / JNBE
  UGeqFlags,   // Maps to: JAE / JNB  / JNC
  ULtFlags,    // Maps to: JB  / JNAE / JC
  ULeqFlags,   // Maps to: JBE / JNA
  SignFlags,   // Maps to: JS and inverted for JNS,

  Eq,          // Operator: == (any sign)
  Neq,         // Operator: != (any sign)
  Gt,          // Operator: >  (signed)
  Geq,         // Operator: >= (signed)
  Lt,          // Operator: <  (signed)
  Leq,         // Operator: <= (signed)
  UGt,         // Operator: >  (unsigned)
  UGeq,        // Operator: >= (unsigned)
  ULt,         // Operator: <  (unsigned)
  ULeq,        // Operator: <=  (unsigned)
  Sign,        // Is Signed?
  NotSign,     // Is not signed?

  CallFar,
  CallNear,
  // Indirect call with no statically known target: covers both near-indirect
  // (`call ax`, `call [mem]`) and far-indirect (`callf [mem]`) sites as one
  // explicit-imprecision marker. Always renders as CALL_FAR_INDIRECT in the
  // C output regardless of which form produced it; a refined near/far split
  // would need the target's segment to be resolvable at the call site.
  CallPtr,
  CallArgs,
  Int,

  // REP-prefixed string ops beyond the single-step forms. Each renders as
  // a runtime bulk macro (REP_MOVSB/... in hydra's machine.h) and is a
  // side effect; the deterministic register updates (CX->0, SI/DI +=
  // CX0*size) are modeled separately as ordinary IR values.
  RepMovs,
  RepStos,
  // REP LODS: a counted load loop that only reads memory; the value is the
  // final accumulator, and SI/CX updates are modeled separately.
  RepLods,
  // REP CMPS/SCAS: counted compare loops; the value is the final CX (the
  // loop stops on a match/mismatch or CX==0). The final comparison's flags
  // are data-dependent, so the flags are clobbered (loud). SI/DI updates are
  // modeled separately.
  RepCmps,
  RepScas,

  // Port I/O. In* read a port into AL/AX (a value), Out* write a value to a
  // port (a side effect). They render as hydra's INB/INW/OUTB/OUTW macros
  // and do not touch guest memory or FLAGS.
  In8,
  In16,
  Out8,
  Out16,

  // String port I/O. INS* read a port into ES:DI (a memory write); OUTS*
  // write DS:SI to a port (a port side effect, memory read only). The
  // deterministic SI/DI/CX updates are modeled separately, as with the
  // other string ops. The helpers execute the raw instruction, so the data
  // value is never materialized in the IR.
  Ins,
  Outs,
  RepIns,
  RepOuts,

  RetFar,
  RetNear,

  Jmp,
  Jne,
  JmpTbl,

  // TODO: HMMM.... Better Impl?
  AssertEven,
  AssertPos,
}


impl Opcode {
  pub fn as_str(&self) -> &'static str {
    match self {
      Opcode::Nop         => "nop",
      Opcode::Pin         => "pin",
      Opcode::Ref         => "ref",
      Opcode::Phi         => "phi",
      Opcode::Unimpl      => "unimpl",
      Opcode::Sub         => "sub",
      Opcode::Add         => "add",
      Opcode::Shl         => "shl",
      Opcode::Shr         => "shr",
      Opcode::UShr        => "ushr",
      Opcode::And         => "and",
      Opcode::Or          => "or",
      Opcode::Xor         => "xor",
      Opcode::IMul        => "imul",
      Opcode::UMul        => "umul",
      Opcode::IDiv        => "idiv",
      Opcode::UDiv        => "udiv",
      Opcode::Neg         => "neg",
      Opcode::Not         => "not",
      Opcode::SignExtTo16 => "signext16",
      Opcode::SignExtTo32 => "signext32",
      //Opcode::AddrOf      => "addrof",
      Opcode::Load8       => "load8",
      Opcode::Load16      => "load16",
      Opcode::Load32      => "load32",
      Opcode::Store8      => "store8",
      Opcode::Store16     => "store16",
      Opcode::Store32     => "store32",
      Opcode::ReadVar8    => "readvar8",
      Opcode::ReadVar16   => "readvar16",
      Opcode::ReadVar32   => "readvar32",
      Opcode::WriteVar8   => "writevar8",
      Opcode::WriteVar16  => "writevar16",
      Opcode::WriteVar32  => "writevar32",
      Opcode::ReadArr8    => "readarr8",
      Opcode::ReadArr16   => "readarr16",
      Opcode::ReadArr32   => "readarr32",
      Opcode::WriteArr8   => "writearr8",
      Opcode::WriteArr16  => "writearr16",
      Opcode::Lower16     => "lower16",
      Opcode::Upper16     => "upper16",
      Opcode::Make32      => "make32",
      Opcode::UpdateFlags => "updf",
      Opcode::EqFlags     => "eqf",
      Opcode::NeqFlags    => "neqf",
      Opcode::GtFlags     => "gtf",
      Opcode::GeqFlags    => "geqf",
      Opcode::LtFlags     => "ltf",
      Opcode::LeqFlags    => "leqf",
      Opcode::UGtFlags    => "ugtf",
      Opcode::UGeqFlags   => "ugeqf",
      Opcode::ULtFlags    => "ultf",
      Opcode::ULeqFlags   => "uleqf",
      Opcode::SignFlags   => "signf",
      Opcode::Eq          => "eq",
      Opcode::Neq         => "neq",
      Opcode::Gt          => "gt",
      Opcode::Geq         => "geq",
      Opcode::Lt          => "lt",
      Opcode::Leq         => "leq",
      Opcode::UGt         => "ugt",
      Opcode::UGeq        => "ugeq",
      Opcode::ULt         => "ult",
      Opcode::ULeq        => "uleq",
      Opcode::Sign        => "sign",
      Opcode::NotSign     => "notsign",
      Opcode::CallFar     => "callfar",
      Opcode::CallNear    => "callnear",
      Opcode::CallPtr     => "callptr",
      Opcode::CallArgs    => "callargs",
      Opcode::Int         => "int",
      Opcode::RepMovs     => "repmovs",
      Opcode::RepStos     => "repstos",
      Opcode::RepLods     => "replods",
      Opcode::RepCmps     => "repcmps",
      Opcode::RepScas     => "repscas",
      Opcode::In8         => "in8",
      Opcode::In16        => "in16",
      Opcode::Out8        => "out8",
      Opcode::Out16       => "out16",
      Opcode::Ins         => "ins",
      Opcode::Outs        => "outs",
      Opcode::RepIns      => "repins",
      Opcode::RepOuts     => "repouts",
      Opcode::RetFar      => "retf",
      Opcode::RetNear     => "retn",
      Opcode::Jmp         => "jmp",
      Opcode::Jne         => "jne",
      Opcode::JmpTbl      => "jmptbl",

      Opcode::AssertEven => "assert_even",
      Opcode::AssertPos  => "assert_pos",
    }
  }
}

impl Opcode {
  pub fn is_load(&self) -> bool {
    match self {
      Opcode::Load8 => true,
      Opcode::Load16 => true,
      Opcode::Load32 => true,
      _ => false,
    }
  }

  pub fn is_store(&self) -> bool {
    match self {
      Opcode::Store8 => true,
      Opcode::Store16 => true,
      Opcode::Store32 => true,
      _ => false,
    }
  }

  /// Whether this op may write arbitrary memory without carrying a
  /// `Store` operand: calls (callee side effects), interrupts (handlers),
  /// and the REP bulk string macros. Memory-reasoning passes
  /// (store-to-load forwarding, symbol SSA lifting) must treat these as
  /// barriers, otherwise an intervening write is invisible and a stale
  /// value propagates.
  pub fn may_clobber_memory(&self) -> bool {
    match self {
      Opcode::CallFar | Opcode::CallNear | Opcode::CallPtr | Opcode::CallArgs => true,
      Opcode::Int => true,
      Opcode::RepMovs | Opcode::RepStos => true,
      // INS writes guest memory (the port's data is stored at ES:DI).
      Opcode::Ins | Opcode::RepIns => true,
      _ => false,
    }
  }

  pub fn is_mem_op(&self) -> bool {
    match self {
      Opcode::Load8 => true,
      Opcode::Load16 => true,
      Opcode::Load32 => true,
      Opcode::Store8 => true,
      Opcode::Store16 => true,
      Opcode::Store32 => true,
      Opcode::ReadVar8 => true,
      Opcode::ReadVar16 => true,
      Opcode::ReadVar32 => true,
      Opcode::WriteVar8 => true,
      Opcode::WriteVar16 => true,
      Opcode::WriteVar32 => true,
      _ => false,
    }
  }

  pub fn is_call(&self) -> bool {
    match self {
      Opcode::CallFar | Opcode::CallNear | Opcode::CallPtr | Opcode::CallArgs => true,
      _ => false,
    }
  }

  pub fn operation_size(&self) -> u16 {
    match self {
      Opcode::Load8 => 1,
      Opcode::Load16 => 2,
      Opcode::Load32 => 4,
      Opcode::Store8 => 1,
      Opcode::Store16 => 2,
      _ => panic!("invalid"),
    }
  }

  pub fn has_no_result(&self) -> bool {
    match self {
      Opcode::Nop => true,
      //Opcode::Pin => true,
      Opcode::Store8 => true,
      Opcode::Store16 => true,
      Opcode::Out8 => true,
      Opcode::Out16 => true,
      Opcode::Ins => true,
      Opcode::Outs => true,
      Opcode::RepIns => true,
      Opcode::RepOuts => true,
      Opcode::WriteVar8 => true,
      Opcode::WriteVar16 => true,
      Opcode::WriteVar32 => true,
      Opcode::RetFar => true,
      Opcode::RetNear => true,
      Opcode::Jmp => true,
      Opcode::Jne => true,
      Opcode::JmpTbl => true,
      _ => false,
    }
  }

  pub fn maybe_unused(&self) -> bool {
    match self {
      Opcode::Nop => true,
      Opcode::Pin => true,
      Opcode::Store8 => true,
      Opcode::Store16 => true,
      Opcode::WriteVar8 => true,
      Opcode::WriteVar16 => true,
      Opcode::WriteVar32 => true,
      Opcode::CallFar => true,
      Opcode::CallNear => true,
      Opcode::CallPtr => true,
      Opcode::CallArgs => true,
      Opcode::Int => true,
      Opcode::RetFar => true,
      Opcode::RetNear => true,
      Opcode::Jmp => true,
      Opcode::Jne => true,
      Opcode::JmpTbl => true,
      _ => false,
    }
  }

  pub fn has_side_effects(&self) -> bool {
    match self {
      Opcode::Pin => true,
      Opcode::Store8 => true,
      Opcode::Store16 => true,
      Opcode::WriteVar8 => true,
      Opcode::WriteVar16 => true,
      Opcode::WriteVar32 => true,
      Opcode::CallFar => true,
      Opcode::CallNear => true,
      Opcode::CallPtr => true,
      Opcode::CallArgs => true,
      Opcode::Int => true,
      Opcode::RepMovs => true,
      Opcode::RepStos => true,
      Opcode::In8 => true,
      Opcode::In16 => true,
      Opcode::Out8 => true,
      Opcode::Out16 => true,
      Opcode::Ins => true,
      Opcode::Outs => true,
      Opcode::RepIns => true,
      Opcode::RepOuts => true,
      Opcode::RetFar => true,
      Opcode::RetNear => true,
      Opcode::Jmp => true,
      Opcode::Jne => true,
      Opcode::JmpTbl => true,
      Opcode::AssertEven => true,
      Opcode::AssertPos => true,
      // An Unimpl marker is an explicit statement that precision was dropped
      // here. Without this it is dead code (no consumers by construction) and
      // DCE would erase the marker before codegen, silently hiding the gap.
      Opcode::Unimpl => true,
      _ => false,
    }
  }
}
