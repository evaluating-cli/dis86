//! SST 80286 capability audit: classified expectations for every upstream
//! `v1_real_mode` form, derived from emu86's step.rs dispatch arms (the
//! authoritative IMPLEMENTED-opcode list) and instr_fmt.rs INSTR_TBL (decode
//! knowledge). See plan.md phase P2.
//!
//! `cap` is authoritative for what emu86 can actually *execute*: a form is
//! `Implemented` only if `step()` has a dispatch arm that does not panic for
//! the form's operand widths. `scope` selects the conservative v1 fetch list.
//! `flags_umask` overrides the runner's default flag mask where the Intel
//! 80286 documentation declares flag bits UNDEFINED after the operation.

use crate::asm::decode::decode_one;
use crate::region::RegionIter;
use crate::segoff::SegOff;

/// What emu86 can do with the form's opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
  /// `step()` has a working dispatch arm (cited in `note`).
  Implemented,
  /// Decodes via instr_fmt but `step()` has no arm -> PANIC bucket.
  DecodeOnly,
  /// OP_INVAL / absent from INSTR_TBL -> DECODE_ERR bucket.
  Undecodable,
}

impl Capability {
  pub fn name(&self) -> &'static str {
    match self {
      Capability::Implemented => "Implemented",
      Capability::DecodeOnly => "DecodeOnly",
      Capability::Undecodable => "Undecodable",
    }
  }
}

/// Why a form is outside the conservative v1 scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
  /// REP / string-op family (MOVS/CMPS/STOS/LODS/SCAS/INS/OUTS).
  RepString,
  /// LOCK / segment-override prefixes.
  PrefixLock,
  /// IN/OUT port I/O.
  Io,
  /// INT/INTO/IRET/HLT family.
  IntHlt,
  /// Not step-implemented (or deliberately excluded) by emu86.
  NotImplemented,
}

/// Conservative-subset membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
  V1,
  Deferred(DeferReason),
}

impl Scope {
  pub fn name(&self) -> &'static str {
    match self {
      Scope::V1 => "V1",
      Scope::Deferred(DeferReason::RepString) => "DEFERRED(REP/string)",
      Scope::Deferred(DeferReason::PrefixLock) => "DEFERRED(LOCK/prefix)",
      Scope::Deferred(DeferReason::Io) => "DEFERRED(I/O)",
      Scope::Deferred(DeferReason::IntHlt) => "DEFERRED(INT/HLT)",
      Scope::Deferred(DeferReason::NotImplemented) => "DEFERRED(not impl)",
    }
  }
}

/// Classified expectation for one upstream form.
pub struct FormPolicy {
  /// Upstream filename stem (without the `.MOO` extension), e.g. "80.3".
  pub file: &'static str,
  /// Canonical probe bytes: opcode + modrm (with the group number in the reg
  /// field) + zero padding, fed to the real emu86 decoder by `decode_check`.
  pub opcode_bytes: &'static [u8],
  pub cap: Capability,
  pub scope: Scope,
  /// Per-form FLAGS comparison mask; `None` means `DEFAULT_FLAGS_UMASK`.
  pub flags_umask: Option<u16>,
  /// Human note; cites the step.rs arm line for `Implemented` forms.
  pub note: &'static str,
}

/// Classified inventory of all 327 upstream `v1_real_mode` entries (326 MOO
/// test forms + `metadata.json`, which upstream lists but is not a test file).
pub const FORM_POLICIES: &[FormPolicy] = &[
  FormPolicy { file: "00", opcode_bytes: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD (step.rs:462)" },
  FormPolicy { file: "01", opcode_bytes: &[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD (step.rs:462)" },
  FormPolicy { file: "02", opcode_bytes: &[0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD (step.rs:462)" },
  FormPolicy { file: "03", opcode_bytes: &[0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD (step.rs:462)" },
  FormPolicy { file: "04", opcode_bytes: &[0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD (step.rs:462)" },
  FormPolicy { file: "05", opcode_bytes: &[0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD (step.rs:462)" },
  FormPolicy { file: "06", opcode_bytes: &[0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH seg (step.rs:254)" },
  FormPolicy { file: "07", opcode_bytes: &[0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP seg (step.rs:263)" },
  FormPolicy { file: "08", opcode_bytes: &[0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR (step.rs:459)" },
  FormPolicy { file: "09", opcode_bytes: &[0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR (step.rs:459)" },
  FormPolicy { file: "0A", opcode_bytes: &[0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR (step.rs:459)" },
  FormPolicy { file: "0B", opcode_bytes: &[0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR (step.rs:459)" },
  FormPolicy { file: "0C", opcode_bytes: &[0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR (step.rs:459)" },
  FormPolicy { file: "0D", opcode_bytes: &[0x0D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR (step.rs:459)" },
  FormPolicy { file: "0E", opcode_bytes: &[0x0E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH seg (step.rs:254)" },
  FormPolicy { file: "10", opcode_bytes: &[0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC (step.rs:463)" },
  FormPolicy { file: "11", opcode_bytes: &[0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC (step.rs:463)" },
  FormPolicy { file: "12", opcode_bytes: &[0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC (step.rs:463)" },
  FormPolicy { file: "13", opcode_bytes: &[0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC (step.rs:463)" },
  FormPolicy { file: "14", opcode_bytes: &[0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC (step.rs:463)" },
  FormPolicy { file: "15", opcode_bytes: &[0x15, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC (step.rs:463)" },
  FormPolicy { file: "16", opcode_bytes: &[0x16, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH seg (step.rs:254)" },
  FormPolicy { file: "17", opcode_bytes: &[0x17, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP seg (step.rs:263)" },
  FormPolicy { file: "18", opcode_bytes: &[0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB (step.rs:465)" },
  FormPolicy { file: "19", opcode_bytes: &[0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB (step.rs:465)" },
  FormPolicy { file: "1A", opcode_bytes: &[0x1A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB (step.rs:465)" },
  FormPolicy { file: "1B", opcode_bytes: &[0x1B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB (step.rs:465)" },
  FormPolicy { file: "1C", opcode_bytes: &[0x1C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB (step.rs:465)" },
  FormPolicy { file: "1D", opcode_bytes: &[0x1D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB (step.rs:465)" },
  FormPolicy { file: "1E", opcode_bytes: &[0x1E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH seg (step.rs:254)" },
  FormPolicy { file: "1F", opcode_bytes: &[0x1F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP seg (step.rs:263)" },
  FormPolicy { file: "20", opcode_bytes: &[0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND (step.rs:458)" },
  FormPolicy { file: "21", opcode_bytes: &[0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND (step.rs:458)" },
  FormPolicy { file: "22", opcode_bytes: &[0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND (step.rs:458)" },
  FormPolicy { file: "23", opcode_bytes: &[0x23, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND (step.rs:458)" },
  FormPolicy { file: "24", opcode_bytes: &[0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND (step.rs:458)" },
  FormPolicy { file: "25", opcode_bytes: &[0x25, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND (step.rs:458)" },
  FormPolicy { file: "27", opcode_bytes: &[0x27, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x07D7), note: "DAA/DAS/AAA/AAS decode but have NO step arm -> PANIC; umask 0x07D7 recorded per Intel flag definitions" },
  FormPolicy { file: "28", opcode_bytes: &[0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB (step.rs:464)" },
  FormPolicy { file: "29", opcode_bytes: &[0x29, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB (step.rs:464)" },
  FormPolicy { file: "2A", opcode_bytes: &[0x2A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB (step.rs:464)" },
  FormPolicy { file: "2B", opcode_bytes: &[0x2B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB (step.rs:464)" },
  FormPolicy { file: "2C", opcode_bytes: &[0x2C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB (step.rs:464)" },
  FormPolicy { file: "2D", opcode_bytes: &[0x2D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB (step.rs:464)" },
  FormPolicy { file: "2F", opcode_bytes: &[0x2F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x07D7), note: "DAA/DAS/AAA/AAS decode but have NO step arm -> PANIC; umask 0x07D7 recorded per Intel flag definitions" },
  FormPolicy { file: "30", opcode_bytes: &[0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR (step.rs:460)" },
  FormPolicy { file: "31", opcode_bytes: &[0x31, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR (step.rs:460)" },
  FormPolicy { file: "32", opcode_bytes: &[0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR (step.rs:460)" },
  FormPolicy { file: "33", opcode_bytes: &[0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR (step.rs:460)" },
  FormPolicy { file: "34", opcode_bytes: &[0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR (step.rs:460)" },
  FormPolicy { file: "35", opcode_bytes: &[0x35, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR (step.rs:460)" },
  FormPolicy { file: "37", opcode_bytes: &[0x37, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x0011), note: "DAA/DAS/AAA/AAS decode but have NO step arm -> PANIC; umask 0x0011 recorded per Intel flag definitions" },
  FormPolicy { file: "38", opcode_bytes: &[0x38, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP (step.rs:441)" },
  FormPolicy { file: "39", opcode_bytes: &[0x39, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP (step.rs:441)" },
  FormPolicy { file: "3A", opcode_bytes: &[0x3A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP (step.rs:441)" },
  FormPolicy { file: "3B", opcode_bytes: &[0x3B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP (step.rs:441)" },
  FormPolicy { file: "3C", opcode_bytes: &[0x3C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP (step.rs:441)" },
  FormPolicy { file: "3D", opcode_bytes: &[0x3D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP (step.rs:441)" },
  FormPolicy { file: "3F", opcode_bytes: &[0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x0011), note: "DAA/DAS/AAA/AAS decode but have NO step arm -> PANIC; umask 0x0011 recorded per Intel flag definitions" },
  FormPolicy { file: "40", opcode_bytes: &[0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC r16 (step.rs:455)" },
  FormPolicy { file: "41", opcode_bytes: &[0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC r16 (step.rs:455)" },
  FormPolicy { file: "42", opcode_bytes: &[0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC r16 (step.rs:455)" },
  FormPolicy { file: "43", opcode_bytes: &[0x43, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC r16 (step.rs:455)" },
  FormPolicy { file: "44", opcode_bytes: &[0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC r16 (step.rs:455)" },
  FormPolicy { file: "45", opcode_bytes: &[0x45, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC r16 (step.rs:455)" },
  FormPolicy { file: "46", opcode_bytes: &[0x46, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC r16 (step.rs:455)" },
  FormPolicy { file: "47", opcode_bytes: &[0x47, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC r16 (step.rs:455)" },
  FormPolicy { file: "48", opcode_bytes: &[0x48, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC r16 (step.rs:456)" },
  FormPolicy { file: "49", opcode_bytes: &[0x49, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC r16 (step.rs:456)" },
  FormPolicy { file: "4A", opcode_bytes: &[0x4A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC r16 (step.rs:456)" },
  FormPolicy { file: "4B", opcode_bytes: &[0x4B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC r16 (step.rs:456)" },
  FormPolicy { file: "4C", opcode_bytes: &[0x4C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC r16 (step.rs:456)" },
  FormPolicy { file: "4D", opcode_bytes: &[0x4D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC r16 (step.rs:456)" },
  FormPolicy { file: "4E", opcode_bytes: &[0x4E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC r16 (step.rs:456)" },
  FormPolicy { file: "4F", opcode_bytes: &[0x4F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC r16 (step.rs:456)" },
  FormPolicy { file: "50", opcode_bytes: &[0x50, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH r16 (step.rs:254)" },
  FormPolicy { file: "51", opcode_bytes: &[0x51, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH r16 (step.rs:254)" },
  FormPolicy { file: "52", opcode_bytes: &[0x52, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH r16 (step.rs:254)" },
  FormPolicy { file: "53", opcode_bytes: &[0x53, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH r16 (step.rs:254)" },
  FormPolicy { file: "54", opcode_bytes: &[0x54, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH r16 (step.rs:254)" },
  FormPolicy { file: "55", opcode_bytes: &[0x55, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH r16 (step.rs:254)" },
  FormPolicy { file: "56", opcode_bytes: &[0x56, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH r16 (step.rs:254)" },
  FormPolicy { file: "57", opcode_bytes: &[0x57, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH r16 (step.rs:254)" },
  FormPolicy { file: "58", opcode_bytes: &[0x58, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP r16 (step.rs:263)" },
  FormPolicy { file: "59", opcode_bytes: &[0x59, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP r16 (step.rs:263)" },
  FormPolicy { file: "5A", opcode_bytes: &[0x5A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP r16 (step.rs:263)" },
  FormPolicy { file: "5B", opcode_bytes: &[0x5B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP r16 (step.rs:263)" },
  FormPolicy { file: "5C", opcode_bytes: &[0x5C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP r16 (step.rs:263)" },
  FormPolicy { file: "5D", opcode_bytes: &[0x5D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP r16 (step.rs:263)" },
  FormPolicy { file: "5E", opcode_bytes: &[0x5E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP r16 (step.rs:263)" },
  FormPolicy { file: "5F", opcode_bytes: &[0x5F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP r16 (step.rs:263)" },
  FormPolicy { file: "60", opcode_bytes: &[0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "PUSHA/POPA decode but have NO step arm -> PANIC" },
  FormPolicy { file: "61", opcode_bytes: &[0x61, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "PUSHA/POPA decode but have NO step arm -> PANIC" },
  FormPolicy { file: "62", opcode_bytes: &[0x62, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Undecodable, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "BOUND is OP_INVAL (instr_fmt.rs:453) -> DECODE_ERR" },
  FormPolicy { file: "68", opcode_bytes: &[0x68, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH imm16 (step.rs:254)" },
  FormPolicy { file: "69", opcode_bytes: &[0x69, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "IMUL r16,rm16,imm16 (step.rs:472); CF/OF defined, rest undefined (Intel 80286)" },
  FormPolicy { file: "6A", opcode_bytes: &[0x6A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH imm8 (step.rs:254)" },
  FormPolicy { file: "6B", opcode_bytes: &[0x6B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "IMUL r16,rm16,imm8 (step.rs:472); CF/OF defined, rest undefined (Intel 80286)" },
  FormPolicy { file: "6C", opcode_bytes: &[0x6C, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "INS/OUTS decode but have NO step arm (step.rs lacks OP_INS/OP_OUTS) -> PANIC; string I/O, deferred" },
  FormPolicy { file: "6D", opcode_bytes: &[0x6D, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "INS/OUTS decode but have NO step arm (step.rs lacks OP_INS/OP_OUTS) -> PANIC; string I/O, deferred" },
  FormPolicy { file: "6E", opcode_bytes: &[0x6E, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "INS/OUTS decode but have NO step arm (step.rs lacks OP_INS/OP_OUTS) -> PANIC; string I/O, deferred" },
  FormPolicy { file: "6F", opcode_bytes: &[0x6F, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "INS/OUTS decode but have NO step arm (step.rs lacks OP_INS/OP_OUTS) -> PANIC; string I/O, deferred" },
  FormPolicy { file: "70", opcode_bytes: &[0x70, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "71", opcode_bytes: &[0x71, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "72", opcode_bytes: &[0x72, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "73", opcode_bytes: &[0x73, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "74", opcode_bytes: &[0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "75", opcode_bytes: &[0x75, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "76", opcode_bytes: &[0x76, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "77", opcode_bytes: &[0x77, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "78", opcode_bytes: &[0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "79", opcode_bytes: &[0x79, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "7A", opcode_bytes: &[0x7A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "7B", opcode_bytes: &[0x7B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "7C", opcode_bytes: &[0x7C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "7D", opcode_bytes: &[0x7D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "7E", opcode_bytes: &[0x7E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "7F", opcode_bytes: &[0x7F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "Jcc rel8 (step.rs:424)" },
  FormPolicy { file: "80.0", opcode_bytes: &[0x80, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD grp1 (step.rs:462)" },
  FormPolicy { file: "80.1", opcode_bytes: &[0x80, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR grp1 (step.rs:459)" },
  FormPolicy { file: "80.2", opcode_bytes: &[0x80, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC grp1 (step.rs:463)" },
  FormPolicy { file: "80.3", opcode_bytes: &[0x80, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB grp1 (step.rs:465)" },
  FormPolicy { file: "80.4", opcode_bytes: &[0x80, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND grp1 (step.rs:458)" },
  FormPolicy { file: "80.5", opcode_bytes: &[0x80, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB grp1 (step.rs:464)" },
  FormPolicy { file: "80.6", opcode_bytes: &[0x80, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR grp1 (step.rs:460)" },
  FormPolicy { file: "80.7", opcode_bytes: &[0x80, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP grp1 (step.rs:441)" },
  FormPolicy { file: "81.0", opcode_bytes: &[0x81, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD grp1 (step.rs:462)" },
  FormPolicy { file: "81.1", opcode_bytes: &[0x81, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR grp1 (step.rs:459)" },
  FormPolicy { file: "81.2", opcode_bytes: &[0x81, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC grp1 (step.rs:463)" },
  FormPolicy { file: "81.3", opcode_bytes: &[0x81, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB grp1 (step.rs:465)" },
  FormPolicy { file: "81.4", opcode_bytes: &[0x81, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND grp1 (step.rs:458)" },
  FormPolicy { file: "81.5", opcode_bytes: &[0x81, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB grp1 (step.rs:464)" },
  FormPolicy { file: "81.6", opcode_bytes: &[0x81, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR grp1 (step.rs:460)" },
  FormPolicy { file: "81.7", opcode_bytes: &[0x81, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP grp1 (step.rs:441)" },
  FormPolicy { file: "82.0", opcode_bytes: &[0x82, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD grp1 (step.rs:462)" },
  FormPolicy { file: "82.1", opcode_bytes: &[0x82, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR grp1 (step.rs:459)" },
  FormPolicy { file: "82.2", opcode_bytes: &[0x82, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC grp1 (step.rs:463)" },
  FormPolicy { file: "82.3", opcode_bytes: &[0x82, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB grp1 (step.rs:465)" },
  FormPolicy { file: "82.4", opcode_bytes: &[0x82, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND grp1 (step.rs:458)" },
  FormPolicy { file: "82.5", opcode_bytes: &[0x82, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB grp1 (step.rs:464)" },
  FormPolicy { file: "82.6", opcode_bytes: &[0x82, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR grp1 (step.rs:460)" },
  FormPolicy { file: "82.7", opcode_bytes: &[0x82, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP grp1 (step.rs:441)" },
  FormPolicy { file: "83.0", opcode_bytes: &[0x83, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADD grp1 (step.rs:462)" },
  FormPolicy { file: "83.1", opcode_bytes: &[0x83, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "OR grp1 (step.rs:459)" },
  FormPolicy { file: "83.2", opcode_bytes: &[0x83, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ADC grp1 (step.rs:463)" },
  FormPolicy { file: "83.3", opcode_bytes: &[0x83, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SBB grp1 (step.rs:465)" },
  FormPolicy { file: "83.4", opcode_bytes: &[0x83, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "AND grp1 (step.rs:458)" },
  FormPolicy { file: "83.5", opcode_bytes: &[0x83, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SUB grp1 (step.rs:464)" },
  FormPolicy { file: "83.6", opcode_bytes: &[0x83, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XOR grp1 (step.rs:460)" },
  FormPolicy { file: "83.7", opcode_bytes: &[0x83, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CMP grp1 (step.rs:441)" },
  FormPolicy { file: "84", opcode_bytes: &[0x84, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "TEST rm8,r8 (step.rs:448)" },
  FormPolicy { file: "85", opcode_bytes: &[0x85, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "TEST rm16,r16 (step.rs:448)" },
  FormPolicy { file: "86", opcode_bytes: &[0x86, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r8,rm8 (step.rs:488)" },
  FormPolicy { file: "87", opcode_bytes: &[0x87, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r16,rm16 (step.rs:488)" },
  FormPolicy { file: "88", opcode_bytes: &[0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV rm8,r8 (step.rs:253)" },
  FormPolicy { file: "89", opcode_bytes: &[0x89, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV rm16,r16 (step.rs:253)" },
  FormPolicy { file: "8A", opcode_bytes: &[0x8A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,rm8 (step.rs:253)" },
  FormPolicy { file: "8B", opcode_bytes: &[0x8B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,rm16 (step.rs:253)" },
  FormPolicy { file: "8C", opcode_bytes: &[0x8C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV rm16,sreg (step.rs:253)" },
  FormPolicy { file: "8D", opcode_bytes: &[0x8D, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "LEA r16,m (step.rs:353)" },
  FormPolicy { file: "8E", opcode_bytes: &[0x8E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV sreg,rm16 (step.rs:253)" },
  FormPolicy { file: "8F", opcode_bytes: &[0x8F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POP rm16 (step.rs:263)" },
  FormPolicy { file: "90", opcode_bytes: &[0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "NOP (step.rs:508)" },
  FormPolicy { file: "91", opcode_bytes: &[0x91, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r16,AX (step.rs:488)" },
  FormPolicy { file: "92", opcode_bytes: &[0x92, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r16,AX (step.rs:488)" },
  FormPolicy { file: "93", opcode_bytes: &[0x93, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r16,AX (step.rs:488)" },
  FormPolicy { file: "94", opcode_bytes: &[0x94, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r16,AX (step.rs:488)" },
  FormPolicy { file: "95", opcode_bytes: &[0x95, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r16,AX (step.rs:488)" },
  FormPolicy { file: "96", opcode_bytes: &[0x96, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r16,AX (step.rs:488)" },
  FormPolicy { file: "97", opcode_bytes: &[0x97, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XCHG r16,AX (step.rs:488)" },
  FormPolicy { file: "98", opcode_bytes: &[0x98, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CBW (step.rs:495); no flags affected (Intel 80286)" },
  FormPolicy { file: "99", opcode_bytes: &[0x99, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CWD (step.rs:502); no flags affected (Intel 80286)" },
  FormPolicy { file: "9A", opcode_bytes: &[0x9A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CALLF far (step.rs:295)" },
  FormPolicy { file: "9B", opcode_bytes: &[0x9B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Undecodable, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "WAIT is OP_INVAL (instr_fmt.rs:538) -> DECODE_ERR" },
  FormPolicy { file: "9C", opcode_bytes: &[0x9C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSHF (step.rs:264)" },
  FormPolicy { file: "9D", opcode_bytes: &[0x9D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "POPF (step.rs:269)" },
  FormPolicy { file: "9E", opcode_bytes: &[0x9E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "LAHF/SAHF decode but have NO step arm -> PANIC" },
  FormPolicy { file: "9F", opcode_bytes: &[0x9F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "LAHF/SAHF decode but have NO step arm -> PANIC" },
  FormPolicy { file: "A0", opcode_bytes: &[0xA0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV AL,moffs8 (step.rs:253)" },
  FormPolicy { file: "A1", opcode_bytes: &[0xA1, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV AX,moffs16 (step.rs:253)" },
  FormPolicy { file: "A2", opcode_bytes: &[0xA2, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV moffs8,AL (step.rs:253)" },
  FormPolicy { file: "A3", opcode_bytes: &[0xA3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV moffs16,AX (step.rs:253)" },
  FormPolicy { file: "A4", opcode_bytes: &[0xA4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "MOVS m8 implemented (step.rs:244) but REP/string family deferred per plan" },
  FormPolicy { file: "A5", opcode_bytes: &[0xA5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "MOVS m16 implemented (step.rs:244) but REP/string family deferred per plan" },
  FormPolicy { file: "A6", opcode_bytes: &[0xA6, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "CMPS m8 implemented (step.rs:245) but REP/string family deferred per plan" },
  FormPolicy { file: "A7", opcode_bytes: &[0xA7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "CMPS m16 implemented (step.rs:245) but REP/string family deferred per plan" },
  FormPolicy { file: "A8", opcode_bytes: &[0xA8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "TEST AL,imm8 (step.rs:448)" },
  FormPolicy { file: "A9", opcode_bytes: &[0xA9, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "TEST AX,imm16 (step.rs:448)" },
  FormPolicy { file: "AA", opcode_bytes: &[0xAA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "STOS m8 implemented (step.rs:243) but REP/string family deferred per plan" },
  FormPolicy { file: "AB", opcode_bytes: &[0xAB, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "STOS m16 implemented (step.rs:243) but REP/string family deferred per plan" },
  FormPolicy { file: "AC", opcode_bytes: &[0xAC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "LODS m8 implemented (step.rs:368) but REP/string family deferred per plan" },
  FormPolicy { file: "AD", opcode_bytes: &[0xAD, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "LODS m16 implemented (step.rs:368) but REP/string family deferred per plan" },
  FormPolicy { file: "AE", opcode_bytes: &[0xAE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "SCAS m8 implemented (step.rs:242) but REP/string family deferred per plan" },
  FormPolicy { file: "AF", opcode_bytes: &[0xAF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::RepString), flags_umask: None, note: "SCAS m16 implemented (step.rs:242) but REP/string family deferred per plan" },
  FormPolicy { file: "B0", opcode_bytes: &[0xB0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,imm8 (step.rs:253)" },
  FormPolicy { file: "B1", opcode_bytes: &[0xB1, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,imm8 (step.rs:253)" },
  FormPolicy { file: "B2", opcode_bytes: &[0xB2, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,imm8 (step.rs:253)" },
  FormPolicy { file: "B3", opcode_bytes: &[0xB3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,imm8 (step.rs:253)" },
  FormPolicy { file: "B4", opcode_bytes: &[0xB4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,imm8 (step.rs:253)" },
  FormPolicy { file: "B5", opcode_bytes: &[0xB5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,imm8 (step.rs:253)" },
  FormPolicy { file: "B6", opcode_bytes: &[0xB6, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,imm8 (step.rs:253)" },
  FormPolicy { file: "B7", opcode_bytes: &[0xB7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r8,imm8 (step.rs:253)" },
  FormPolicy { file: "B8", opcode_bytes: &[0xB8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,imm16 (step.rs:253)" },
  FormPolicy { file: "B9", opcode_bytes: &[0xB9, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,imm16 (step.rs:253)" },
  FormPolicy { file: "BA", opcode_bytes: &[0xBA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,imm16 (step.rs:253)" },
  FormPolicy { file: "BB", opcode_bytes: &[0xBB, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,imm16 (step.rs:253)" },
  FormPolicy { file: "BC", opcode_bytes: &[0xBC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,imm16 (step.rs:253)" },
  FormPolicy { file: "BD", opcode_bytes: &[0xBD, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,imm16 (step.rs:253)" },
  FormPolicy { file: "BE", opcode_bytes: &[0xBE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,imm16 (step.rs:253)" },
  FormPolicy { file: "BF", opcode_bytes: &[0xBF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV r16,imm16 (step.rs:253)" },
  FormPolicy { file: "C0.0", opcode_bytes: &[0xC0, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ROL grp2 (step.rs:469)" },
  FormPolicy { file: "C0.1", opcode_bytes: &[0xC0, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ROR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "C0.2", opcode_bytes: &[0xC0, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCL grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "C0.3", opcode_bytes: &[0xC0, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "C0.4", opcode_bytes: &[0xC0, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "C0.5", opcode_bytes: &[0xC0, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHR grp2 (step.rs:467)" },
  FormPolicy { file: "C0.6", opcode_bytes: &[0xC0, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "C0.7", opcode_bytes: &[0xC0, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SAR grp2 (step.rs:468)" },
  FormPolicy { file: "C1.0", opcode_bytes: &[0xC1, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ROL grp2 (step.rs:469)" },
  FormPolicy { file: "C1.1", opcode_bytes: &[0xC1, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ROR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "C1.2", opcode_bytes: &[0xC1, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCL grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "C1.3", opcode_bytes: &[0xC1, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "C1.4", opcode_bytes: &[0xC1, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "C1.5", opcode_bytes: &[0xC1, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHR grp2 (step.rs:467)" },
  FormPolicy { file: "C1.6", opcode_bytes: &[0xC1, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "C1.7", opcode_bytes: &[0xC1, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SAR grp2 (step.rs:468)" },
  FormPolicy { file: "C2", opcode_bytes: &[0xC2, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "RET imm16 (step.rs:324)" },
  FormPolicy { file: "C3", opcode_bytes: &[0xC3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "RET (step.rs:324)" },
  FormPolicy { file: "C4", opcode_bytes: &[0xC4, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "LES/LDS implemented (step.rs:346) but segmented addressing deferred per plan" },
  FormPolicy { file: "C5", opcode_bytes: &[0xC5, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "LES/LDS implemented (step.rs:346) but segmented addressing deferred per plan" },
  FormPolicy { file: "C6", opcode_bytes: &[0xC6, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV rm8,imm8 (step.rs:253)" },
  FormPolicy { file: "C7", opcode_bytes: &[0xC7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "MOV rm16,imm16 (step.rs:253)" },
  FormPolicy { file: "C8", opcode_bytes: &[0xC8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ENTER asserts level==0 (step.rs:306-307); LEAVE implemented (step.rs:316); deferred from v1" },
  FormPolicy { file: "C9", opcode_bytes: &[0xC9, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ENTER asserts level==0 (step.rs:306-307); LEAVE implemented (step.rs:316); deferred from v1" },
  FormPolicy { file: "CA", opcode_bytes: &[0xCA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "RETF imm16 (step.rs:334)" },
  FormPolicy { file: "CB", opcode_bytes: &[0xCB, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "RETF (step.rs:334)" },
  FormPolicy { file: "CC", opcode_bytes: &[0xCC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::IntHlt), flags_umask: None, note: "INT implemented (step.rs:284) but INT-family deferred per plan" },
  FormPolicy { file: "CD", opcode_bytes: &[0xCD, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::IntHlt), flags_umask: None, note: "INT implemented (step.rs:284) but INT-family deferred per plan" },
  FormPolicy { file: "CE", opcode_bytes: &[0xCE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::IntHlt), flags_umask: None, note: "INTO decodes but has NO step arm -> PANIC" },
  FormPolicy { file: "CF", opcode_bytes: &[0xCF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::IntHlt), flags_umask: None, note: "IRET implemented (step.rs:285) but INT-family deferred per plan" },
  FormPolicy { file: "D0.0", opcode_bytes: &[0xD0, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ROL grp2 (step.rs:469)" },
  FormPolicy { file: "D0.1", opcode_bytes: &[0xD0, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ROR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D0.2", opcode_bytes: &[0xD0, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCL grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D0.3", opcode_bytes: &[0xD0, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D0.4", opcode_bytes: &[0xD0, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "D0.5", opcode_bytes: &[0xD0, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHR grp2 (step.rs:467)" },
  FormPolicy { file: "D0.6", opcode_bytes: &[0xD0, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "D0.7", opcode_bytes: &[0xD0, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SAR grp2 (step.rs:468)" },
  FormPolicy { file: "D1.0", opcode_bytes: &[0xD1, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ROL grp2 (step.rs:469)" },
  FormPolicy { file: "D1.1", opcode_bytes: &[0xD1, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ROR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D1.2", opcode_bytes: &[0xD1, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCL grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D1.3", opcode_bytes: &[0xD1, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D1.4", opcode_bytes: &[0xD1, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "D1.5", opcode_bytes: &[0xD1, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHR grp2 (step.rs:467)" },
  FormPolicy { file: "D1.6", opcode_bytes: &[0xD1, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "D1.7", opcode_bytes: &[0xD1, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SAR grp2 (step.rs:468)" },
  FormPolicy { file: "D2.0", opcode_bytes: &[0xD2, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ROL grp2 (step.rs:469)" },
  FormPolicy { file: "D2.1", opcode_bytes: &[0xD2, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ROR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D2.2", opcode_bytes: &[0xD2, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCL grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D2.3", opcode_bytes: &[0xD2, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D2.4", opcode_bytes: &[0xD2, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "D2.5", opcode_bytes: &[0xD2, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHR grp2 (step.rs:467)" },
  FormPolicy { file: "D2.6", opcode_bytes: &[0xD2, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "D2.7", opcode_bytes: &[0xD2, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SAR grp2 (step.rs:468)" },
  FormPolicy { file: "D3.0", opcode_bytes: &[0xD3, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "ROL grp2 (step.rs:469)" },
  FormPolicy { file: "D3.1", opcode_bytes: &[0xD3, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ROR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D3.2", opcode_bytes: &[0xD3, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCL grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D3.3", opcode_bytes: &[0xD3, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "RCR grp2 decodes but NO step arm (only SHL/SHR/SAR/ROL implemented) -> PANIC" },
  FormPolicy { file: "D3.4", opcode_bytes: &[0xD3, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "D3.5", opcode_bytes: &[0xD3, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHR grp2 (step.rs:467)" },
  FormPolicy { file: "D3.6", opcode_bytes: &[0xD3, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SHL grp2 (step.rs:466)" },
  FormPolicy { file: "D3.7", opcode_bytes: &[0xD3, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "SAR grp2 (step.rs:468)" },
  FormPolicy { file: "D4", opcode_bytes: &[0xD4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Undecodable, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x00C4), note: "AAM/AAD are OP_INVAL (instr_fmt.rs:637-638) -> DECODE_ERR; umask 0x00C4 recorded (SF/ZF/PF defined per Intel)" },
  FormPolicy { file: "D5", opcode_bytes: &[0xD5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Undecodable, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x00C4), note: "AAM/AAD are OP_INVAL (instr_fmt.rs:637-638) -> DECODE_ERR; umask 0x00C4 recorded (SF/ZF/PF defined per Intel)" },
  FormPolicy { file: "D6", opcode_bytes: &[0xD6, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Undecodable, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "undocumented D6 is OP_INVAL (instr_fmt.rs:639) -> DECODE_ERR" },
  FormPolicy { file: "D7", opcode_bytes: &[0xD7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "XLAT (step.rs:275)" },
  FormPolicy { file: "D8", opcode_bytes: &[0xD8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Undecodable, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "ESC/co-processor D8 is OP_INVAL (instr_fmt.rs:641) -> DECODE_ERR" },
  FormPolicy { file: "E0", opcode_bytes: &[0xE0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "LOOPNE (step.rs:422)" },
  FormPolicy { file: "E1", opcode_bytes: &[0xE1, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "LOOPE (step.rs:421)" },
  FormPolicy { file: "E2", opcode_bytes: &[0xE2, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "LOOP (step.rs:420)" },
  FormPolicy { file: "E3", opcode_bytes: &[0xE3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "JCXZ (step.rs:415)" },
  FormPolicy { file: "E4", opcode_bytes: &[0xE4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::Io), flags_umask: None, note: "IN/OUT implemented (step.rs:382/388) but I/O deferred per plan" },
  FormPolicy { file: "E5", opcode_bytes: &[0xE5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::Io), flags_umask: None, note: "IN/OUT implemented (step.rs:382/388) but I/O deferred per plan" },
  FormPolicy { file: "E6", opcode_bytes: &[0xE6, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::Io), flags_umask: None, note: "IN/OUT implemented (step.rs:382/388) but I/O deferred per plan" },
  FormPolicy { file: "E7", opcode_bytes: &[0xE7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::Io), flags_umask: None, note: "IN/OUT implemented (step.rs:382/388) but I/O deferred per plan" },
  FormPolicy { file: "E8", opcode_bytes: &[0xE8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CALL rel16 (step.rs:286)" },
  FormPolicy { file: "E9", opcode_bytes: &[0xE9, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "JMP rel16 (step.rs:401)" },
  FormPolicy { file: "EA", opcode_bytes: &[0xEA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "JMPF far (step.rs:410)" },
  FormPolicy { file: "EB", opcode_bytes: &[0xEB, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "JMP rel8 (step.rs:401)" },
  FormPolicy { file: "EC", opcode_bytes: &[0xEC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::Io), flags_umask: None, note: "IN/OUT implemented (step.rs:382/388) but I/O deferred per plan" },
  FormPolicy { file: "ED", opcode_bytes: &[0xED, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::Io), flags_umask: None, note: "IN/OUT implemented (step.rs:382/388) but I/O deferred per plan" },
  FormPolicy { file: "EE", opcode_bytes: &[0xEE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::Io), flags_umask: None, note: "IN/OUT implemented (step.rs:382/388) but I/O deferred per plan" },
  FormPolicy { file: "EF", opcode_bytes: &[0xEF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::Deferred(DeferReason::Io), flags_umask: None, note: "IN/OUT implemented (step.rs:382/388) but I/O deferred per plan" },
  FormPolicy { file: "F4", opcode_bytes: &[0xF4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::IntHlt), flags_umask: None, note: "HLT decodes but has NO step arm -> PANIC; tests terminate with F4 so never stepped, but F4-form itself unexecutable" },
  FormPolicy { file: "F5", opcode_bytes: &[0xF5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "CMC decodes (OP_CMC) but has NO step arm -> PANIC; plan lists CMC as a v1 flag-op but emu86 lacks it" },
  FormPolicy { file: "F6.0", opcode_bytes: &[0xF6, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "TEST grp3 (step.rs:448)" },
  FormPolicy { file: "F6.1", opcode_bytes: &[0xF6, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "TEST grp3 (step.rs:448)" },
  FormPolicy { file: "F6.2", opcode_bytes: &[0xF6, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "NOT grp3 (step.rs:461)" },
  FormPolicy { file: "F6.3", opcode_bytes: &[0xF6, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "NEG grp3 (step.rs:457)" },
  FormPolicy { file: "F6.4", opcode_bytes: &[0xF6, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x0801), note: "8-bit MUL/IMUL decodes but step.rs op_multiply asserts 16-bit (step.rs:161-163) -> PANIC; umask 0x0801 recorded (CF/OF defined per Intel)" },
  FormPolicy { file: "F6.5", opcode_bytes: &[0xF6, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x0801), note: "8-bit MUL/IMUL decodes but step.rs op_multiply asserts 16-bit (step.rs:161-163) -> PANIC; umask 0x0801 recorded (CF/OF defined per Intel)" },
  FormPolicy { file: "F6.6", opcode_bytes: &[0xF6, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x0000), note: "8-bit DIV/IDIV decodes but step.rs divmod asserts u32/u16 lhs/rhs (alu.rs:129-130) -> PANIC; umask 0x0000 recorded (flags undefined per Intel)" },
  FormPolicy { file: "F6.7", opcode_bytes: &[0xF6, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::DecodeOnly, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: Some(0x0000), note: "8-bit DIV/IDIV decodes but step.rs divmod asserts u32/u16 lhs/rhs (alu.rs:129-130) -> PANIC; umask 0x0000 recorded (flags undefined per Intel)" },
  FormPolicy { file: "F7.0", opcode_bytes: &[0xF7, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "TEST grp3 (step.rs:448)" },
  FormPolicy { file: "F7.1", opcode_bytes: &[0xF7, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "TEST grp3 (step.rs:448)" },
  FormPolicy { file: "F7.2", opcode_bytes: &[0xF7, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "NOT grp3 (step.rs:461)" },
  FormPolicy { file: "F7.3", opcode_bytes: &[0xF7, 0xC0 | (3 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "NEG grp3 (step.rs:457)" },
  FormPolicy { file: "F7.4", opcode_bytes: &[0xF7, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: Some(0x0801), note: "MUL grp3 (step.rs:471); CF/OF defined, rest UNDEFINED (Intel 80286) -> umask 0x0801" },
  FormPolicy { file: "F7.5", opcode_bytes: &[0xF7, 0xC0 | (5 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: Some(0x0801), note: "IMUL grp3 (step.rs:472); CF/OF defined, rest UNDEFINED (Intel 80286) -> umask 0x0801" },
  FormPolicy { file: "F7.6", opcode_bytes: &[0xF7, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: Some(0x0000), note: "DIV grp3 (step.rs:475); all status flags UNDEFINED (Intel 80286) -> umask 0x0000" },
  FormPolicy { file: "F7.7", opcode_bytes: &[0xF7, 0xC0 | (7 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: Some(0x0000), note: "IDIV grp3 (step.rs:475); all status flags UNDEFINED (Intel 80286) -> umask 0x0000" },
  FormPolicy { file: "F8", opcode_bytes: &[0xF8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CLC (step.rs:365)" },
  FormPolicy { file: "F9", opcode_bytes: &[0xF9, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "STC (step.rs:366)" },
  FormPolicy { file: "FA", opcode_bytes: &[0xFA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CLI (step.rs:363)" },
  FormPolicy { file: "FB", opcode_bytes: &[0xFB, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "STI (step.rs:364)" },
  FormPolicy { file: "FC", opcode_bytes: &[0xFC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CLD (step.rs:361)" },
  FormPolicy { file: "FD", opcode_bytes: &[0xFD, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "STD (step.rs:362)" },
  FormPolicy { file: "FE.0", opcode_bytes: &[0xFE, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC rm8 (step.rs:455)" },
  FormPolicy { file: "FE.1", opcode_bytes: &[0xFE, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC rm8 (step.rs:456)" },
  FormPolicy { file: "FF.0", opcode_bytes: &[0xFF, 0xC0 | (0 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "INC grp5 (step.rs:455)" },
  FormPolicy { file: "FF.1", opcode_bytes: &[0xFF, 0xC0 | (1 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "DEC grp5 (step.rs:456)" },
  FormPolicy { file: "FF.2", opcode_bytes: &[0xFF, 0xC0 | (2 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CALL grp5 (step.rs:286)" },
  FormPolicy { file: "FF.3", opcode_bytes: &[0xFF, 0x1E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "CALLF grp5 (step.rs:295)" },
  FormPolicy { file: "FF.4", opcode_bytes: &[0xFF, 0xC0 | (4 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "JMP grp5 (step.rs:401)" },
  FormPolicy { file: "FF.5", opcode_bytes: &[0xFF, 0x2E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "JMPF grp5 (step.rs:410)" },
  FormPolicy { file: "FF.6", opcode_bytes: &[0xFF, 0xC0 | (6 << 3), 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], cap: Capability::Implemented, scope: Scope::V1, flags_umask: None, note: "PUSH grp5 (step.rs:254)" },
  FormPolicy { file: "metadata.json", opcode_bytes: &[], cap: Capability::Undecodable, scope: Scope::Deferred(DeferReason::NotImplemented), flags_umask: None, note: "not a MOO test form; upstream metadata.json blob (counted to match the 327-entry upstream listing)" },
];

/// Look up the policy for an upstream filename stem (with or without `.MOO`).
pub fn policy_for_file(stem: &str) -> Option<&'static FormPolicy> {
  let stem = stem.strip_suffix(".MOO").unwrap_or(stem);
  FORM_POLICIES.iter().find(|p| p.file == stem)
}

/// The conservative v1 fetch list: forms that are both `V1`-scoped and
/// step-implemented.
pub fn v1_form_files() -> Vec<&'static str> {
  FORM_POLICIES
    .iter()
    .filter(|p| p.scope == Scope::V1 && p.cap == Capability::Implemented)
    .map(|p| p.file)
    .collect()
}

/// Runtime decode probe through the real emu86 decoder: true iff the form's
/// canonical probe bytes decode to an instruction (i.e. `cap` is not
/// `Undecodable`). DecodeOnly vs Implemented is not distinguishable from the
/// decoder alone; that distinction is documented per-entry in `note` and comes
/// from a manual read of step.rs's match arms.
pub fn decode_check(file: &FormPolicy) -> bool {
  let addr = SegOff::new(0, 0);
  let mut bin = RegionIter::new(file.opcode_bytes, addr);
  matches!(decode_one(&mut bin), Ok(Some(_)))
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashSet;

  /// Every stem in the upstream `v1_real_mode` listing at the pinned commit
  /// (37c73caf53dcd22d3dd369ff09305d13d117a4fe), captured via the GitHub
  /// contents API on 2026-08-15. Kept as a const so the coverage check is
  /// hermetic (no network in tests).
  pub const UPSTREAM_STEMS: &[&str] = &[
  "00",
  "01",
  "02",
  "03",
  "04",
  "05",
  "06",
  "07",
  "08",
  "09",
  "0A",
  "0B",
  "0C",
  "0D",
  "0E",
  "10",
  "11",
  "12",
  "13",
  "14",
  "15",
  "16",
  "17",
  "18",
  "19",
  "1A",
  "1B",
  "1C",
  "1D",
  "1E",
  "1F",
  "20",
  "21",
  "22",
  "23",
  "24",
  "25",
  "27",
  "28",
  "29",
  "2A",
  "2B",
  "2C",
  "2D",
  "2F",
  "30",
  "31",
  "32",
  "33",
  "34",
  "35",
  "37",
  "38",
  "39",
  "3A",
  "3B",
  "3C",
  "3D",
  "3F",
  "40",
  "41",
  "42",
  "43",
  "44",
  "45",
  "46",
  "47",
  "48",
  "49",
  "4A",
  "4B",
  "4C",
  "4D",
  "4E",
  "4F",
  "50",
  "51",
  "52",
  "53",
  "54",
  "55",
  "56",
  "57",
  "58",
  "59",
  "5A",
  "5B",
  "5C",
  "5D",
  "5E",
  "5F",
  "60",
  "61",
  "62",
  "68",
  "69",
  "6A",
  "6B",
  "6C",
  "6D",
  "6E",
  "6F",
  "70",
  "71",
  "72",
  "73",
  "74",
  "75",
  "76",
  "77",
  "78",
  "79",
  "7A",
  "7B",
  "7C",
  "7D",
  "7E",
  "7F",
  "80.0",
  "80.1",
  "80.2",
  "80.3",
  "80.4",
  "80.5",
  "80.6",
  "80.7",
  "81.0",
  "81.1",
  "81.2",
  "81.3",
  "81.4",
  "81.5",
  "81.6",
  "81.7",
  "82.0",
  "82.1",
  "82.2",
  "82.3",
  "82.4",
  "82.5",
  "82.6",
  "82.7",
  "83.0",
  "83.1",
  "83.2",
  "83.3",
  "83.4",
  "83.5",
  "83.6",
  "83.7",
  "84",
  "85",
  "86",
  "87",
  "88",
  "89",
  "8A",
  "8B",
  "8C",
  "8D",
  "8E",
  "8F",
  "90",
  "91",
  "92",
  "93",
  "94",
  "95",
  "96",
  "97",
  "98",
  "99",
  "9A",
  "9B",
  "9C",
  "9D",
  "9E",
  "9F",
  "A0",
  "A1",
  "A2",
  "A3",
  "A4",
  "A5",
  "A6",
  "A7",
  "A8",
  "A9",
  "AA",
  "AB",
  "AC",
  "AD",
  "AE",
  "AF",
  "B0",
  "B1",
  "B2",
  "B3",
  "B4",
  "B5",
  "B6",
  "B7",
  "B8",
  "B9",
  "BA",
  "BB",
  "BC",
  "BD",
  "BE",
  "BF",
  "C0.0",
  "C0.1",
  "C0.2",
  "C0.3",
  "C0.4",
  "C0.5",
  "C0.6",
  "C0.7",
  "C1.0",
  "C1.1",
  "C1.2",
  "C1.3",
  "C1.4",
  "C1.5",
  "C1.6",
  "C1.7",
  "C2",
  "C3",
  "C4",
  "C5",
  "C6",
  "C7",
  "C8",
  "C9",
  "CA",
  "CB",
  "CC",
  "CD",
  "CE",
  "CF",
  "D0.0",
  "D0.1",
  "D0.2",
  "D0.3",
  "D0.4",
  "D0.5",
  "D0.6",
  "D0.7",
  "D1.0",
  "D1.1",
  "D1.2",
  "D1.3",
  "D1.4",
  "D1.5",
  "D1.6",
  "D1.7",
  "D2.0",
  "D2.1",
  "D2.2",
  "D2.3",
  "D2.4",
  "D2.5",
  "D2.6",
  "D2.7",
  "D3.0",
  "D3.1",
  "D3.2",
  "D3.3",
  "D3.4",
  "D3.5",
  "D3.6",
  "D3.7",
  "D4",
  "D5",
  "D6",
  "D7",
  "D8",
  "E0",
  "E1",
  "E2",
  "E3",
  "E4",
  "E5",
  "E6",
  "E7",
  "E8",
  "E9",
  "EA",
  "EB",
  "EC",
  "ED",
  "EE",
  "EF",
  "F4",
  "F5",
  "F6.0",
  "F6.1",
  "F6.2",
  "F6.3",
  "F6.4",
  "F6.5",
  "F6.6",
  "F6.7",
  "F7.0",
  "F7.1",
  "F7.2",
  "F7.3",
  "F7.4",
  "F7.5",
  "F7.6",
  "F7.7",
  "F8",
  "F9",
  "FA",
  "FB",
  "FC",
  "FD",
  "FE.0",
  "FE.1",
  "FF.0",
  "FF.1",
  "FF.2",
  "FF.3",
  "FF.4",
  "FF.5",
  "FF.6",
  "metadata.json",
];

  #[test]
  fn table_covers_exact_upstream_listing() {
    let table: HashSet<&str> = FORM_POLICIES.iter().map(|p| p.file).collect();
    let expected: HashSet<&str> = UPSTREAM_STEMS.iter().copied().collect();
    assert_eq!(table, expected, "form table must match the upstream listing exactly");
    assert_eq!(FORM_POLICIES.len(), UPSTREAM_STEMS.len());
  }

  #[test]
  fn no_duplicate_stems() {
    let mut seen = HashSet::new();
    for p in FORM_POLICIES {
      assert!(seen.insert(p.file), "duplicate stem: {}", p.file);
    }
  }

  #[test]
  fn every_v1_file_is_implemented() {
    for p in FORM_POLICIES {
      if p.scope == Scope::V1 {
        assert_eq!(p.cap, Capability::Implemented, "V1 form {} must be Implemented", p.file);
      }
    }
  }

  #[test]
  fn deferred_not_implemented_has_note() {
    for p in FORM_POLICIES {
      if p.scope == Scope::Deferred(DeferReason::NotImplemented) {
        assert!(!p.note.is_empty(), "Deferred(NotImplemented) {} needs a note", p.file);
      }
    }
  }

  #[test]
  fn v1_list_is_implemented_and_unique() {
    let list = v1_form_files();
    let mut seen = HashSet::new();
    for f in &list {
      let p = policy_for_file(f).unwrap();
      assert_eq!(p.scope, Scope::V1);
      assert_eq!(p.cap, Capability::Implemented);
      assert!(seen.insert(*f), "dup in v1 list: {}", f);
    }
    assert_eq!(list.len(), 258, "expected 258 conservative-v1 forms");
  }

  #[test]
  fn probe_agrees_with_capability_for_every_form() {
    for p in FORM_POLICIES {
      let expected = p.cap != Capability::Undecodable;
      let got = decode_check(p);
      assert_eq!(
        got, expected,
        "decode probe mismatch for {}: probe says decodable={}",
        p.file, got
      );
    }
  }

  #[test]
  fn probe_decodes_implemented_form_and_rejects_undecodable() {
    // 04 = ADD AL,imm8: decodes, and emu86 step.rs:462 implements it.
    let add = policy_for_file("04").unwrap();
    assert_eq!(add.cap, Capability::Implemented);
    assert!(decode_check(add));

    // 62 = BOUND: OP_INVAL in INSTR_TBL -> DECODE_ERR, probe must fail.
    let bound = policy_for_file("62").unwrap();
    assert_eq!(bound.cap, Capability::Undecodable);
    assert!(!decode_check(bound));

    // DecodeOnly forms decode but must not be advertised as Implemented.
    let cmc = policy_for_file("F5").unwrap();
    assert_eq!(cmc.cap, Capability::DecodeOnly);
    assert!(decode_check(cmc));
  }

  #[test]
  fn policy_for_file_accepts_moo_suffix() {
    assert_eq!(policy_for_file("04").unwrap().file, policy_for_file("04.MOO").unwrap().file);
    assert!(policy_for_file("nope").is_none());
  }

  #[test]
  fn umask_overrides_are_computed_per_intel_docs() {
    // MUL/IMUL: CF(0x001)+OF(0x800); DIV/IDIV: all undefined.
    assert_eq!(policy_for_file("F7.4").unwrap().flags_umask, Some(0x0801));
    assert_eq!(policy_for_file("F7.5").unwrap().flags_umask, Some(0x0801));
    assert_eq!(policy_for_file("F7.6").unwrap().flags_umask, Some(0x0000));
    assert_eq!(policy_for_file("F7.7").unwrap().flags_umask, Some(0x0000));
    // DAA/DAS drop only OF; AAA/AAS keep only CF+AF.
    assert_eq!(policy_for_file("27").unwrap().flags_umask, Some(0x07D7));
    assert_eq!(policy_for_file("2F").unwrap().flags_umask, Some(0x07D7));
    assert_eq!(policy_for_file("37").unwrap().flags_umask, Some(0x0011));
    assert_eq!(policy_for_file("3F").unwrap().flags_umask, Some(0x0011));
    // Everything else defaults to the runner mask.
    assert_eq!(policy_for_file("04").unwrap().flags_umask, None);
    assert_eq!(policy_for_file("F7.0").unwrap().flags_umask, None);
  }
}
