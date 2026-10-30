use crate::asm::instr::Opcode;
use crate::segoff::SegOff;
use std::fmt;

/// Why a translation diagnostic was emitted. `message` remains the
/// human-readable text; aggregation keys on this closed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cause {
  Unimpl,
  BranchOutOfRange,
  IndirectJumpNoTable,
  IndirectJumpRuntimeTarget,
  UnmodelledFarJump,
  DecodeError,
  AbiArgsInference,
  ExitContractMismatch,
  RegargsSetupDropped,
  CodegenFailure,
  ConfigError,
  InternalPanic,
}

impl Cause {
  /// A blocker is a site where translation lost information or stopped.
  /// Advisory warnings (ABI contracts, regargs) are deliberately excluded.
  pub fn is_blocker(self) -> bool {
    matches!(self,
      Cause::Unimpl
        | Cause::BranchOutOfRange
        | Cause::IndirectJumpNoTable
        | Cause::IndirectJumpRuntimeTarget
        | Cause::UnmodelledFarJump
        | Cause::DecodeError)
  }

  /// A failure prevents the function from completing the requested pipeline.
  pub fn is_failure(self) -> bool {
    matches!(self,
      Cause::DecodeError | Cause::CodegenFailure | Cause::ConfigError | Cause::InternalPanic)
  }

  pub fn label(self) -> &'static str {
    match self {
      Cause::Unimpl => "UNIMPL",
      Cause::BranchOutOfRange => "BRANCH_OUT_OF_RANGE",
      Cause::IndirectJumpNoTable => "INDIRECT_JUMP_NO_TABLE",
      Cause::IndirectJumpRuntimeTarget => "INDIRECT_JUMP_RUNTIME_TARGET",
      Cause::UnmodelledFarJump => "UNMODELLED_FAR_JUMP",
      Cause::DecodeError => "DECODE_ERROR",
      Cause::AbiArgsInference => "ABI_ARGS_INFERENCE",
      Cause::ExitContractMismatch => "EXIT_CONTRACT_MISMATCH",
      Cause::RegargsSetupDropped => "REGARGS_SETUP_DROPPED",
      Cause::CodegenFailure => "CODEGEN_FAILURE",
      Cause::ConfigError => "CONFIG_ERROR",
      Cause::InternalPanic => "INTERNAL_PANIC",
    }
  }
}

/// A structured translation or ABI warning. The message is kept verbatim so
/// default stderr output remains unchanged.
#[derive(Debug, Clone)]
pub struct Diagnostic {
  pub cause: Cause,
  pub addr: Option<SegOff>,
  pub opcode: Option<Opcode>,
  pub message: String,
}

impl Diagnostic {
  pub fn new(cause: Cause, addr: Option<SegOff>, opcode: Option<Opcode>,
             message: impl Into<String>) -> Self {
    Diagnostic { cause, addr, opcode, message: message.into() }
  }

  pub fn contains(&self, needle: &str) -> bool {
    self.message.contains(needle)
  }

  pub fn starts_with(&self, prefix: &str) -> bool {
    self.message.starts_with(prefix)
  }
}

impl fmt::Display for Diagnostic {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", self.message)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn blocker_and_failure_classes_are_disjoint_for_advisories() {
    for cause in [
      Cause::Unimpl,
      Cause::BranchOutOfRange,
      Cause::IndirectJumpNoTable,
      Cause::IndirectJumpRuntimeTarget,
      Cause::UnmodelledFarJump,
    ] {
      assert!(cause.is_blocker(), "{} must be a blocker", cause.label());
      assert!(!cause.is_failure(), "{} must not be a failure", cause.label());
    }
    assert!(Cause::DecodeError.is_blocker());
    assert!(Cause::DecodeError.is_failure());
    for cause in [Cause::AbiArgsInference, Cause::ExitContractMismatch, Cause::RegargsSetupDropped] {
      assert!(!cause.is_blocker(), "{} is advisory", cause.label());
      assert!(!cause.is_failure(), "{} is advisory", cause.label());
    }
    for cause in [Cause::CodegenFailure, Cause::ConfigError, Cause::InternalPanic] {
      assert!(!cause.is_blocker(), "{} is failure-only", cause.label());
      assert!(cause.is_failure(), "{} is a failure", cause.label());
    }
  }
}
