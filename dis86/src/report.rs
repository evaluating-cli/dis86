use crate::asm::instr;
use crate::config::Config;
use crate::diagnostics::{Cause, Diagnostic};
use crate::segoff::Seg;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Aggregated translation results for a batch run. Aggregation keys are
/// ordered maps so the text report is deterministic across runs.
#[derive(Debug, Default)]
pub struct TranslationLedger {
  segments: BTreeMap<Seg, SegmentReport>,
  global_failures: Vec<Diagnostic>,
}

#[derive(Debug, Default)]
struct SegmentReport {
  configured: usize,
  attempted: usize,
  translated: usize,
  failed: usize,
  failures_by_cause: BTreeMap<Cause, usize>,
  blockers: BTreeMap<Cause, CauseReport>,
}

#[derive(Debug, Default)]
struct CauseReport {
  count: usize,
  first: Option<Diagnostic>,
  opcodes: BTreeMap<String, OpcodeReport>,
}

#[derive(Debug, Default)]
struct OpcodeReport {
  count: usize,
  first: Option<Diagnostic>,
}

impl TranslationLedger {
  pub fn set_configured(&mut self, seg: Seg, count: usize) {
    self.segment(seg).configured = count;
  }

  pub fn attempt(&mut self, seg: Seg) {
    self.segment(seg).attempted += 1;
  }

  pub fn translated(&mut self, seg: Seg) {
    self.segment(seg).translated += 1;
  }

  pub fn blocker(&mut self, seg: Seg, diagnostic: &Diagnostic) {
    if !diagnostic.cause.is_blocker() { return; }
    let report = self.segment(seg).blockers.entry(diagnostic.cause).or_default();
    report.count += 1;
    if report.first.is_none() {
      report.first = Some(diagnostic.clone());
    }
    if let Some(opcode) = diagnostic.opcode {
      let entry = report.opcodes.entry(opcode_name(opcode)).or_default();
      entry.count += 1;
      if entry.first.is_none() {
        entry.first = Some(diagnostic.clone());
      }
    }
  }

  pub fn failure(&mut self, seg: Seg, diagnostic: Diagnostic) {
    {
      let report = self.segment(seg);
      report.failed += 1;
      *report.failures_by_cause.entry(diagnostic.cause).or_insert(0) += 1;
    }
    if diagnostic.cause.is_blocker() {
      self.blocker(seg, &diagnostic);
    }
  }

  pub fn global_failure(&mut self, diagnostic: Diagnostic) {
    self.global_failures.push(diagnostic);
  }

  fn segment(&mut self, seg: Seg) -> &mut SegmentReport {
    self.segments.entry(seg).or_default()
  }

  pub fn render(&self, cfg: &Config) -> String {
    let mut out = String::new();
    for (seg, report) in &self.segments {
      let name = cfg.code_seg_lookup(*seg)
        .map(|code_seg| code_seg.name.as_str())
        .unwrap_or("?");
      writeln!(out, "segment {} ({})", seg, name).unwrap();
      writeln!(out, "  functions configured {} attempted {} translated {} failed {}",
               report.configured, report.attempted, report.translated, report.failed).unwrap();
      writeln!(out, "  failed by cause: {}", render_cause_counts(&report.failures_by_cause)).unwrap();
      if report.blockers.is_empty() {
        writeln!(out, "  blockers: none").unwrap();
      } else {
        writeln!(out, "  blockers:").unwrap();
        for (cause, blockers) in &report.blockers {
          writeln!(out, "    {} {} {}", cause.label(), blockers.count,
                   render_first(blockers.first.as_ref())).unwrap();
          let mut opcodes: Vec<_> = blockers.opcodes.iter().collect();
          opcodes.sort_by(|(a_name, a), (b_name, b)| {
            b.count.cmp(&a.count).then_with(|| a_name.cmp(b_name))
          });
          for (opcode, entry) in opcodes.into_iter().take(3) {
            writeln!(out, "      {} {} {}", opcode, entry.count,
                     render_first_addr(entry.first.as_ref())).unwrap();
          }
        }
      }
    }
    for diagnostic in &self.global_failures {
      writeln!(out, "global failure: {}: {}", diagnostic.cause.label(),
               diagnostic.message).unwrap();
    }
    out
  }
}

fn opcode_name(opcode: instr::Opcode) -> String {
  format!("{:?}", opcode)
}

fn render_cause_counts(counts: &BTreeMap<Cause, usize>) -> String {
  if counts.is_empty() { return "none".to_string(); }
  counts.iter()
    .map(|(cause, count)| format!("{} {}", cause.label(), count))
    .collect::<Vec<_>>()
    .join(", ")
}

fn render_first(first: Option<&Diagnostic>) -> String {
  let Some(first) = first else { return "first unknown".to_string(); };
  match (first.addr, first.opcode) {
    (Some(addr), Some(opcode)) => format!("first {} ({})", addr, opcode_name(opcode)),
    (Some(addr), None) => format!("first {}", addr),
    (None, Some(opcode)) => format!("first unknown ({})", opcode_name(opcode)),
    (None, None) => "first unknown".to_string(),
  }
}

fn render_first_addr(first: Option<&Diagnostic>) -> String {
  let Some(first) = first else { return "first unknown".to_string(); };
  match first.addr {
    Some(addr) => format!("first {}", addr),
    None => "first unknown".to_string(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::segoff::{Off, SegOff};
  use crate::types::TypeDatabase;
  use std::rc::Rc;

  fn so(off: u16) -> SegOff {
    SegOff { seg: Seg::Normal(0), off: Off(off) }
  }

  fn diag(cause: Cause, off: u16, opcode: Option<instr::Opcode>) -> Diagnostic {
    Diagnostic::new(cause, Some(so(off)), opcode, "message")
  }

  fn cfg() -> Config {
    Config {
      types: Rc::new(TypeDatabase::new()),
      structs: vec![],
      code_segs: vec![crate::config::CodeSeg { seg: Seg::Normal(0), name: "main".to_string() }],
      funcs: vec![],
      indirects: vec![],
      globals: vec![],
      text_section: vec![],
    }
  }

  #[test]
  fn aggregates_blockers_by_cause_opcode_and_first_sighting() {
    let mut ledger = TranslationLedger::default();
    let seg = Seg::Normal(0);
    ledger.set_configured(seg, 2);
    ledger.attempt(seg);
    ledger.translated(seg);
    ledger.blocker(seg, &diag(Cause::Unimpl, 0x10, Some(instr::Opcode::OP_HLT)));
    ledger.blocker(seg, &diag(Cause::Unimpl, 0x20, Some(instr::Opcode::OP_INVAL)));
    ledger.blocker(seg, &diag(Cause::Unimpl, 0x30, Some(instr::Opcode::OP_HLT)));
    ledger.failure(seg, diag(Cause::DecodeError, 0x40, None));

    let text = ledger.render(&cfg());
    assert!(text.contains("functions configured 2 attempted 1 translated 1 failed 1"), "{text}");
    assert!(text.contains("failed by cause: DECODE_ERROR 1"), "{text}");
    assert!(text.contains("UNIMPL 3 first 0000:0010 (OP_HLT)"), "{text}");
    assert!(text.contains("OP_HLT 2 first 0000:0010"), "{text}");
    assert!(text.contains("OP_INVAL 1 first 0000:0020"), "{text}");
    assert!(text.contains("DECODE_ERROR 1 first 0000:0040"), "{text}");
  }

  #[test]
  fn advisories_are_not_blockers() {
    let mut ledger = TranslationLedger::default();
    let seg = Seg::Normal(0);
    ledger.set_configured(seg, 0);
    ledger.blocker(seg, &diag(Cause::AbiArgsInference, 0x10, None));
    ledger.blocker(seg, &diag(Cause::RegargsSetupDropped, 0x20, None));
    let text = ledger.render(&cfg());
    assert!(text.contains("blockers: none"), "{text}");
  }

  #[test]
  fn report_is_deterministic() {
    let mut ledger = TranslationLedger::default();
    let seg = Seg::Normal(0);
    ledger.blocker(seg, &diag(Cause::Unimpl, 0x20, Some(instr::Opcode::OP_INVAL)));
    ledger.blocker(seg, &diag(Cause::Unimpl, 0x10, Some(instr::Opcode::OP_HLT)));
    let first = ledger.render(&cfg());
    let second = ledger.render(&cfg());
    assert_eq!(first, second);
    let lines: Vec<_> = first.lines().collect();
    let hlt = lines.iter().position(|line| line.trim_start().starts_with("OP_HLT ")).unwrap();
    let inval = lines.iter().position(|line| line.trim_start().starts_with("OP_INVAL ")).unwrap();
    assert!(hlt < inval, "equal counts sort by opcode name: {first}");
  }
}
