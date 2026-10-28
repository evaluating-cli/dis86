//! Preflight validation for dis86 configuration tables.
//!
//! The decompiler assumes its configuration ranges are real code inside the
//! loaded image. When that assumption is wrong, the failure tends to surface
//! late, often as a panic in `Binary::region`, a raw decode error, or an
//! `ir_build` call-mode assertion. This module checks the assumptions up front
//! and reports the smallest useful set of diagnostics before a batch run.

use crate::analyze::code_segment::{CodeSegment, CodeSegments, Region};
use crate::analyze::func_details::{first_undecodable_addr, FuncDetails, ReturnKind};
use crate::asm::branch::{self, BranchTarget, UnresolvedTransfer};
use crate::asm::decode::Decoder;
use crate::binary::Binary;
use crate::config::{CallMode, Config, Func};
use crate::segoff::{Seg, SegOff};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
  Error,
  Warning,
}

impl fmt::Display for Severity {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Severity::Error => write!(f, "ERROR"),
      Severity::Warning => write!(f, "WARN"),
    }
  }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
  pub severity: Severity,
  pub addr: Option<SegOff>,
  pub message: String,
}

impl fmt::Display for Diagnostic {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self.addr {
      Some(addr) => write!(f, "{} {}: {}", self.severity, addr, self.message),
      None => write!(f, "{}: {}", self.severity, self.message),
    }
  }
}

#[derive(Debug, Default)]
pub struct Report {
  pub diagnostics: Vec<Diagnostic>,
}

impl Report {
  pub fn error(&mut self, addr: Option<SegOff>, message: impl Into<String>) {
    self.diagnostics.push(Diagnostic {
      severity: Severity::Error,
      addr,
      message: message.into(),
    });
  }

  pub fn warning(&mut self, addr: Option<SegOff>, message: impl Into<String>) {
    self.diagnostics.push(Diagnostic {
      severity: Severity::Warning,
      addr,
      message: message.into(),
    });
  }

  pub fn errors(&self) -> usize {
    self.diagnostics.iter().filter(|d| d.severity == Severity::Error).count()
  }

  pub fn warnings(&self) -> usize {
    self.diagnostics.iter().filter(|d| d.severity == Severity::Warning).count()
  }

  pub fn has_errors(&self) -> bool {
    self.errors() != 0
  }

  pub fn should_fail(&self, strict: bool) -> bool {
    self.has_errors() || (strict && self.warnings() != 0)
  }

  pub fn print(&self) {
    for d in &self.diagnostics {
      eprintln!("{}", d);
    }
    eprintln!("config validation: {} error(s), {} warning(s)", self.errors(), self.warnings());
  }
}

#[derive(Debug, Clone)]
pub enum ValidationScope {
  All,
  Function(String),
  CodeSegment(String),
}

/// Validate the configured tables against `binary`.
///
/// Structural and address errors are fatal. Mode conflicts and unresolved
/// targets are diagnostics: the caller decides whether warnings fail the run
/// (see `Report::should_fail`).
pub fn validate_config(cfg: &Config, binary: &Binary, scope: &ValidationScope) -> Report {
  let mut report = Report::default();

  validate_code_segments(cfg, &mut report);
  validate_text_sections(cfg, binary, &mut report);
  validate_indirects(cfg, binary, &mut report);

  let selected: Vec<&Func> = match scope {
    ValidationScope::All => cfg.funcs.iter().collect(),
    ValidationScope::Function(name) => match cfg.func_lookup_by_name(name) {
      Some(func) => vec![func],
      None => {
        report.error(None, format!("function '{}' is not defined in the config", name));
        return report;
      }
    },
    ValidationScope::CodeSegment(name) => match cfg.code_seg_lookup_by_name(name) {
      Some(code_seg) => cfg.func_lookup_by_seg(code_seg.seg),
      None => {
        report.error(None, format!("code segment '{}' is not defined in the config", name));
        return report;
      }
    },
  };

  if selected.is_empty() {
    if let ValidationScope::CodeSegment(name) = scope {
      report.warning(None, format!("code segment '{}' has no configured functions", name));
    }
  }

  let mz_segments = if binary.exe().is_some() {
    match CodeSegments::from_binary(binary) {
      Ok(segments) => Some(segments),
      Err(err) => {
        report.error(None, format!("cannot determine code segments: {}", err));
        return report;
      }
    }
  } else {
    None
  };

  let mut seen_names: HashSet<&str> = HashSet::new();
  let mut seen_starts: BTreeMap<SegOff, &str> = BTreeMap::new();

  for func in selected {
    if !seen_names.insert(func.name.as_str()) {
      report.error(Some(func.start), format!("duplicate function name '{}'", func.name));
    }
    if let Some(other) = seen_starts.insert(func.start, func.name.as_str()) {
      report.error(Some(func.start), format!(
        "functions '{}' and '{}' share start address {}", other, func.name, func.start));
    }

    validate_function(cfg, binary, mz_segments.as_ref(), func, &mut report);
  }

  report
}

fn validate_code_segments(cfg: &Config, report: &mut Report) {
  let mut names = HashSet::new();
  let mut segs = HashSet::new();
  for code_seg in &cfg.code_segs {
    if !names.insert(code_seg.name.as_str()) {
      report.error(None, format!("duplicate code segment name '{}'", code_seg.name));
    }
    if !segs.insert(code_seg.seg) {
      report.error(None, format!("duplicate code segment entry for segment {}", code_seg.seg));
    }
  }
}

fn validate_text_sections(cfg: &Config, binary: &Binary, report: &mut Report) {
  let mut names = HashSet::new();
  for text in &cfg.text_section {
    if !names.insert(text.name.as_str()) {
      report.error(Some(text.start), format!("duplicate text section name '{}'", text.name));
    }
    if text.start.seg != text.end.seg {
      report.error(Some(text.start), format!(
        "text section '{}' crosses segments: {}..{}", text.name, text.start, text.end));
      continue;
    }
    if text.end <= text.start {
      report.error(Some(text.start), format!(
        "text section '{}' has an invalid range: {}..{}", text.name, text.start, text.end));
      continue;
    }
    if !binary.contains_region(text.start, text.end) {
      report.error(Some(text.start), format!(
        "text section '{}' range {}..{} is outside the loaded image", text.name, text.start, text.end));
    }
    if let Some(access) = text.access {
      if !binary.contains_addr(access) {
        report.error(Some(access), format!(
          "text section '{}' access address {} is outside the loaded image", text.name, access));
      }
    }
  }
}

fn validate_indirects(cfg: &Config, binary: &Binary, report: &mut Report) {
  let mut seen = HashSet::new();
  for indirect in &cfg.indirects {
    if !seen.insert(indirect.addr) {
      report.error(Some(indirect.addr), format!(
        "duplicate indirect call entry at {}", indirect.addr));
    }
    if !binary.contains_addr(indirect.addr) {
      report.error(Some(indirect.addr), format!(
        "indirect call location {} is outside the loaded image", indirect.addr));
    }
  }
}

fn validate_function(
  cfg: &Config,
  binary: &Binary,
  mz_segments: Option<&CodeSegments>,
  func: &Func,
  report: &mut Report,
) {
  let Some(end) = func.end else {
    report.error(Some(func.start), format!(
      "function '{}' has no end address; batch decompilation has no range", func.name));
    return;
  };
  let start = func.start;
  if start.seg != end.seg {
    report.error(Some(start), format!(
      "function '{}' range crosses segments: {}..{}", func.name, start, end));
    return;
  }
  if end <= start {
    report.error(Some(start), format!(
      "function '{}' has an invalid range: {}..{}", func.name, start, end));
    return;
  }

  if cfg.code_seg_lookup(start.seg).is_none() {
    report.error(Some(func.start), format!(
      "function '{}' is in segment {}, which is not declared in dis86.code_segments", func.name, start.seg));
  }

  let code_seg = match mz_segments {
    Some(segments) => match segments.find_by_segment(start.seg) {
      Some(code_seg) => (*code_seg).clone(),
      None => {
        report.error(Some(func.start), format!(
          "function '{}' is in segment {}, which is not a code segment in the binary", func.name, start.seg));
        return;
      }
    },
    None => match raw_code_segment(binary, start.seg) {
      Ok(code_seg) => code_seg,
      Err(err) => {
        report.error(Some(func.start), format!(
          "function '{}' cannot be analyzed: {}", func.name, err));
        return;
      }
    },
  };

  if !binary.contains_region(start, end) {
    let mut message = format!(
      "function '{}' range {}..{} is outside the loaded image", func.name, start, end);
    if let Some(hint) = file_offset_hint(binary, end) {
      message.push_str("; ");
      message.push_str(&hint);
    }
    report.error(Some(start), message);
    return;
  }
  if !code_seg.contains(start) {
    report.error(Some(start), format!(
      "function '{}' starts outside its code segment [{}, {})",
      func.name, code_seg.start(), code_seg.end()));
    return;
  }
  if end > code_seg.end() {
    report.error(Some(start), format!(
      "function '{}' end {} is past the code segment end {} (segment range [{}, {}))",
      func.name, end, code_seg.end(), code_seg.start(), code_seg.end()));
    return;
  }

  if let Some(entry) = func.entry {
    if entry.seg != start.seg || entry < start || entry >= end {
      report.error(Some(entry), format!(
        "function '{}' entry {} is outside its configured range", func.name, entry));
    }
  }

  for text in &cfg.text_section {
    if text.start.seg == start.seg && ranges_overlap(start, end, text.start, text.end) {
      report.error(Some(text.start), format!(
        "text section '{}' [{}..{}) overlaps function '{}' range [{}, {})",
        text.name, text.start, text.end, func.name, start, end));
    }
  }

  if let Some(gap) = first_undecodable_addr(binary, start, end) {
    report.error(Some(gap), format!(
      "function '{}' linear decode of [{}, {}) fails at {}; the range spans non-code bytes (split the range or declare the data in text_section)",
      func.name, start, end, gap));
    return;
  }

  validate_branch_targets(func, start, end, binary, report);

  let details = match FuncDetails::build(start, Some(end), &code_seg, binary) {
    Ok(details) => details,
    Err(err) => {
      report.error(Some(start), format!(
        "function '{}' analysis failed for [{}, {}): {}", func.name, start, end, err));
      return;
    }
  };

  if details.return_kind.is_some() || !details.ignored_return_kinds.is_empty() {
    validate_return_mode(func, &details.return_kind, &details.ignored_return_kinds, report);
  } else {
    report.warning(Some(func.start), format!(
      "function '{}' has no observed return; call mode is not validated from its body", func.name));
  }
  validate_direct_calls(cfg, func, &details.direct_call_kinds, report);

  if details.indirect_calls != 0 {
    report.warning(Some(func.start), format!(
      "function '{}' has {} indirect call(s); their targets and modes cannot be validated",
      func.name, details.indirect_calls));
  }
}

fn validate_return_mode(
  func: &Func,
  return_kind: &Option<ReturnKind>,
  ignored_return_kinds: &[ReturnKind],
  report: &mut Report,
) {
  match return_kind {
    Some(ReturnKind::Near) if func.mode == CallMode::Far => {
      report.error(Some(func.start), format!(
        "function '{}' is configured far but returns near; set mode near", func.name));
    }
    Some(ReturnKind::Far) if func.mode == CallMode::Near => {
      report.error(Some(func.start), format!(
        "function '{}' is configured near but returns far; set mode far", func.name));
    }
    Some(ReturnKind::Interrupt) => {
      report.warning(Some(func.start), format!(
        "function '{}' ends in an interrupt return; near/far call mode is not validated", func.name));
    }
    None => {
      report.warning(Some(func.start), format!(
        "function '{}' has no observed return; call mode is not validated from its body", func.name));
    }
    _ => {}
  }

  if !ignored_return_kinds.is_empty() {
    let kinds: Vec<String> = ignored_return_kinds.iter().map(|kind| kind.to_string()).collect();
    report.warning(Some(func.start), format!(
      "function '{}' has mixed return kinds ({}); the configured mode may not fit every exit path",
      func.name, kinds.join(", ")));
  }
}

fn validate_direct_calls(
  cfg: &Config,
  func: &Func,
  direct_call_kinds: &BTreeMap<SegOff, BTreeSet<CallMode>>,
  report: &mut Report,
) {
  for (target, modes) in direct_call_kinds {
    if modes.contains(&CallMode::Near) && modes.contains(&CallMode::Far) {
      report.error(Some(func.start), format!(
        "function '{}' calls {} as both near and far", func.name, target));
      continue;
    }
    for mode in modes {
      match cfg.func_lookup(*target) {
        Some(target_func) => {
          if target_func.mode != *mode {
            report.error(Some(func.start), format!(
              "function '{}' makes a {} call to '{}' ({}), but '{}' is configured {}",
              func.name, mode_str(*mode), target_func.name, target, target_func.name, mode_str(target_func.mode)));
          }
        }
        None => {
          report.warning(Some(func.start), format!(
            "function '{}' makes a {} call to unconfigured target {}; its mode cannot be validated",
            func.name, mode_str(*mode), target));
        }
      }
    }
  }
}

fn validate_branch_targets(
  func: &Func,
  start: SegOff,
  end: SegOff,
  binary: &Binary,
  report: &mut Report,
) {
  let in_range = |target: SegOff| {
    target.seg == start.seg && target >= start && target < end
  };
  let mut decoder = Decoder::new(binary.region_iter(start, end));
  loop {
    let instr = match decoder.try_next() {
      Ok(Some((instr, _raw))) => instr,
      Ok(None) => break,
      Err(_) => break,
    };
    let Some(target) = branch::branch_target(&instr) else { continue };
    match target {
      BranchTarget::Relative { target, .. } => {
        if !in_range(target) {
          report.warning(Some(instr.addr), format!(
            "function '{}' branch at {} targets {}, outside its configured range",
            func.name, instr.addr, target));
        }
      }
      BranchTarget::Unresolved(UnresolvedTransfer::FarDirect { seg, off }) => {
        let target = SegOff::new(seg, off);
        if !in_range(target) {
          report.warning(Some(instr.addr), format!(
            "function '{}' far branch at {} targets {}, outside its configured range",
            func.name, instr.addr, target));
        }
      }
      BranchTarget::Unresolved(_) => {}
    }
  }
}

fn raw_code_segment(binary: &Binary, seg: Seg) -> Result<CodeSegment, String> {
  let Seg::Normal(_) = seg else {
    return Err("a raw binary has no overlay segments".to_string());
  };
  let base = seg.abs_normal();
  let image_len = binary.image_len();
  if base > image_len {
    return Err(format!(
      "segment {} starts at image byte {:#x}, past the {}-byte raw image", seg, base, image_len));
  }
  let size = image_len - base;
  if size > u16::MAX as usize {
    return Err(format!(
      "raw image segment {} is {} bytes; segment offsets are u16 and cannot cover it", seg, size));
  }
  Ok(CodeSegment {
    primary: Region { seg, skip_off: 0, size: size as u32 },
    stub: None,
  })
}

fn file_offset_hint(binary: &Binary, end: SegOff) -> Option<String> {
  let exe = binary.exe()?;
  let Seg::Normal(_) = end.seg else { return None };
  let end_abs = end.abs_normal();
  let image_len = binary.image_len();
  let header_len = exe.exe_start as usize;
  if end_abs > image_len && end_abs - image_len == header_len {
    return Some(format!(
      "{} looks like a file offset; dis86 configuration addresses are load-relative and the image ends at {:#x}, so subtract the {:#x}-byte MZ header",
      end, image_len, header_len));
  }
  None
}

fn ranges_overlap(a_start: SegOff, a_end: SegOff, b_start: SegOff, b_end: SegOff) -> bool {
  a_start.seg == b_start.seg && a_start < b_end && b_start < a_end
}

fn mode_str(mode: CallMode) -> &'static str {
  match mode {
    CallMode::Near => "near",
    CallMode::Far => "far",
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::binfmt::mz;
  use crate::config::{CodeSeg, Indirect, TextSectionRegion};
  use crate::segoff::{Off, Seg};
  use crate::types::{Type, TypeDatabase};
  use std::rc::Rc;

  fn segoff(off: u16) -> SegOff {
    SegOff { seg: Seg::Normal(0), off: Off(off) }
  }

  fn func(name: &str, start: u16, end: u16, mode: CallMode) -> Func {
    Func {
      name: name.to_string(),
      start: segoff(start),
      end: Some(segoff(end)),
      entry: None,
      mode,
      ret: None,
      args: Some(0),
      regargs: None,
      dont_pop_args: false,
    }
  }

  fn config(
    funcs: Vec<Func>,
    text_section: Vec<TextSectionRegion>,
    indirects: Vec<Indirect>,
  ) -> Config {
    Config {
      types: Rc::new(TypeDatabase::new()),
      structs: vec![],
      code_segs: vec![CodeSeg { seg: Seg::Normal(0), name: "main".to_string() }],
      funcs,
      indirects,
      globals: vec![],
      text_section,
    }
  }

  fn synthetic_mz(image_len: usize) -> Vec<u8> {
    // A 512-byte MZ header (cparhdr=32) plus a load image of image_len bytes.
    let exe_end = 512 + image_len as u32;
    let (cp, cblp) = if exe_end % 512 == 0 {
      (exe_end / 512, 0)
    } else {
      (exe_end / 512 + 1, exe_end % 512)
    };
    let mut data = vec![0u8; exe_end as usize];
    data[0..2].copy_from_slice(b"MZ");
    data[2..4].copy_from_slice(&(cblp as u16).to_le_bytes());
    data[4..6].copy_from_slice(&(cp as u16).to_le_bytes());
    data[8..10].copy_from_slice(&32u16.to_le_bytes());
    data[24..26].copy_from_slice(&0x40u16.to_le_bytes());
    data
  }

  #[test]
  fn valid_raw_function_passes() {
    let binary = Binary::from_raw(&[0xC3], None);
    let cfg = config(vec![func("f", 0, 1, CallMode::Near)], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert_eq!(report.errors(), 0);
    assert_eq!(report.warnings(), 0);
  }

  #[test]
  fn out_of_image_range_is_an_error() {
    let binary = Binary::from_raw(&[0xC3], None);
    let cfg = config(vec![func("f", 0, 2, CallMode::Near)], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.message.contains("outside the loaded image")));
  }

  #[test]
  fn file_offset_end_gets_an_actionable_hint() {
    let exe = mz::Exe::decode(&synthetic_mz(1)).expect("valid synthetic MZ");
    let binary = Binary::from_exe(&exe, None);
    let cfg = config(vec![func("f", 0, 0x201, CallMode::Near)], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.message.contains("looks like a file offset")));
  }

  #[test]
  fn cross_segment_range_is_an_error() {
    let binary = Binary::from_raw(&[0xC3, 0xC3], None);
    let mut f = func("f", 0, 0, CallMode::Near);
    f.end = Some(SegOff { seg: Seg::Normal(1), off: Off(1) });
    let cfg = config(vec![f], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.message.contains("crosses segments")));
  }

  #[test]
  fn missing_end_is_an_error() {
    let binary = Binary::from_raw(&[0xC3], None);
    let mut f = func("f", 0, 1, CallMode::Near);
    f.end = None;
    let cfg = config(vec![f], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.message.contains("no end address")));
  }

  #[test]
  fn return_mode_mismatch_is_an_error() {
    let binary = Binary::from_raw(&[0xCB], None);
    let cfg = config(vec![func("f", 0, 1, CallMode::Near)], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.message.contains("returns far")));
  }

  #[test]
  fn direct_call_mode_mismatch_is_an_error() {
    let binary = Binary::from_raw(&[0xE8, 0x00, 0x00, 0xCB], None);
    let cfg = config(vec![
      func("caller", 0, 3, CallMode::Near),
      func("callee", 3, 4, CallMode::Far),
    ], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.message.contains("makes a near call")));
  }

  #[test]
  fn undecodable_range_is_an_error() {
    let binary = Binary::from_raw(&[0xE9, 0x03, 0x00, 0x63, 0x63, 0x63, 0xC3], None);
    let cfg = config(vec![func("f", 0, 7, CallMode::Near)], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.message.contains("linear decode")));
  }

  #[test]
  fn text_section_overlap_is_an_error() {
    let binary = Binary::from_raw(&[0xC3], None);
    let text = TextSectionRegion {
      name: "string".to_string(),
      start: segoff(0),
      end: segoff(1),
      typ: Type::U16,
      access: None,
    };
    let cfg = config(vec![func("f", 0, 1, CallMode::Near)], vec![text], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.message.contains("overlaps function")));
  }

  #[test]
  fn unresolved_target_is_a_warning_and_strict_fails() {
    let binary = Binary::from_raw(&[0xE8, 0x01, 0x00, 0xC3], None);
    let cfg = config(vec![func("f", 0, 3, CallMode::Near)], vec![], vec![]);
    let report = validate_config(&cfg, &binary, &ValidationScope::All);
    assert_eq!(report.errors(), 0);
    assert!(report.warnings() > 0);
    assert!(!report.should_fail(false));
    assert!(report.should_fail(true));
  }
}
