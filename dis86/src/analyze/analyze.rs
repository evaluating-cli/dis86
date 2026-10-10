use crate::binary::{Binary, Fmt};
use crate::config::{CallMode, Config};
use crate::segoff::{Seg, SegOff};
use crate::util::range_set::RangeSet;

use super::workqueue::WorkQueue;
use super::code_segment::{CodeSegments};
use super::func_details::{FuncDetails, ReturnKind};

use std::collections::{BTreeMap, HashSet};

pub struct Analyze {
  cfg: Config,
  binary: Binary,
  pub code_segments: CodeSegments,
}

impl Analyze {
  pub fn new(cfg: &Config, exe_path: &str) -> Result<Self, String> {
    let fmt = Fmt::Exe(exe_path.to_string());
    let binary = Binary::from_fmt(&fmt, Some(cfg))?;
    let code_segments = CodeSegments::from_binary(&binary)?;

    Ok(Self {
      cfg: cfg.clone(),
      binary,
      code_segments,
    })
  }

  pub fn dump_info(&self) {
    self.binary.exe().unwrap().print();
  }

  pub fn analyze_code_segment(&self, seg: Seg) -> (u32, u32) {
    let code_seg = self.code_segments.find_by_segment(seg).unwrap();

    let mut r = RangeSet::new();

    // Add all ranges implied by function config
    for f in &self.cfg.funcs {
      if f.start.seg != code_seg.primary.seg { continue };
      let Some(end) = &f.end else {
        println!("Unknown end address for {}", f.name);
        continue;
      };
      r.insert(f.start.off.0 as u32, end.off.0 as u32);
    }

    // Add all ranges implied by the text section data
    for t in self.cfg.text_regions_matching_segment(code_seg.primary.seg) {
      r.insert(t.start.off.0 as u32, t.end.off.0 as u32);
    }

    let seg_start = code_seg.primary.skip_off;
    let seg_end = seg_start + code_seg.primary.size;

    if let Some(span_end) = r.span_end() {
      if span_end > seg_end {
        println!("WARN: Function ranges exceed the segment! (expected: {}, got: {})", seg_end, span_end);
      }
    }

    let gaps = r.gaps_within(seg_start, seg_end);
    let mut total_gap = 0;
    for gap in &gaps {
      total_gap += gap.end - gap.start;
    }

    let total_size = code_seg.primary.size;
    let perc = if total_size > 0 {
      100.0 * (1.0 - (total_gap as f64) / (total_size as f64))
    } else {
      100.0
    };
    println!("Percent annotated: {:.2}", perc);

    if gaps.len() > 0 {
      println!("Gaps:");
      for gap in &gaps {
        println!("   [ 0x{:04x}, 0x{:04x} )   size: {}", gap.start, gap.end, gap.end - gap.start);
      }
    }

    (total_gap, total_size)
  }

  pub fn analyze_code_segments_and_report(&self) {
    //self.code_segments.dump();
    let mut total_gap = 0;
    let mut total_size = 0;
    for c in &self.code_segments.0 {
      let seg = c.primary.seg;
      println!("Segment {}", seg);
      println!("===============================");
      let (gap, size) = self.analyze_code_segment(seg);
      total_gap += gap;
      total_size += size;
      println!("");
    }

    let perc = 100.0 * (1.0 - (total_gap as f64) / (total_size as f64));
    println!("Total completion: {} / {} = {:.2} %", total_size - total_gap, total_size, perc);

  }

  pub fn analyze_function(&self, name: &str) -> FuncDetails {
    let func = self.cfg.func_lookup_by_name(name).unwrap(); // FIXME
    let code_seg = self.code_segments.find_for_function(func).unwrap(); // FIXME
    assert!(func.start >= code_seg.start());
    FuncDetails::build(func.start, func.end, code_seg, &self.binary).unwrap() // HAX FIXME
  }

  pub fn analyze_function_by_start(&self, start: SegOff) -> Result<FuncDetails, String> {
    let Some(code_seg) = self.code_segments.find_by_segment(start.seg) else {
      return Err(format!("Failed to find code segement"));
    };
    assert!(start >= code_seg.start());
    let end = self.cfg.func_lookup(start).and_then(|f| f.end);
    FuncDetails::build(start, end, code_seg, &self.binary)
  }

  // Scan known functions to find new functions, then scan those, return a big list of all found functions
  pub fn scan_for_all_functions(&self, emit_annotation_format: bool) {
    let mut workqueue = WorkQueue::new();

    // init work queue with known config functions
    for f in &self.cfg.funcs {
      workqueue.insert(f.start);
    }

    let mut functions = BTreeMap::new();
    while let Some(addr) = workqueue.pop() {
      let result = self.analyze_function_by_start(addr);

      // Add all new calls to the work queue
      if let Ok(details) = &result {
        for call in &details.direct_calls {
          workqueue.insert(*call);
        }
      }

      functions.insert(addr, result);
    }

    if emit_annotation_format {
      // Synthesize annotations
      generate_annotations(&functions, &self.cfg);
    } else {
      // Print out a report
      dump_functions(&functions, &self.cfg);
    }
  }
}

fn dump_functions(functions: &BTreeMap<SegOff, Result<FuncDetails, String>>, cfg: &Config) {
  let mut current_seg = None;
  for (addr, result) in functions {
    let seg = addr.seg;
    if Some(seg) != current_seg {
      println!("");
      println!("Segment {}", seg);
      println!("--------------------------------------------------------------------------------------------------------------------------------------------------------");
      current_seg = Some(seg);
    }

    let name = match cfg.func_lookup(*addr) {
      Some(func) => func.name.clone(),
      None => "UNKNOWN".to_string(),
    };

    print!("Function: {:<35} |  addr: {}  | ", name, addr);
    match result {
      Ok(details) => {
        println!("start: {}  end: {}  indirect_calls: {}",
                 details.start_addr, details.end_addr_inferred, details.indirect_calls);
      }
      Err(err) => {
        println!("error: '{}'", err);
      }
    }
  }
}

struct FunctionNames {
  used: HashSet<String>,
}

impl FunctionNames {
  fn from_cfg(cfg: &Config) -> FunctionNames {
    let mut used = HashSet::new();
    for func in &cfg.funcs {
      if used.get(&func.name).is_some() {
        panic!("Duplicate function name in the config: {}", func.name);
      }
      used.insert(func.name.clone());
    }
    FunctionNames { used }
  }

  fn compute_unique(&mut self, base: &str) -> String {
    // Try a bunch of names until we find a unique one
    // NOTE: THIS IS VERY INEFFICENT... Falls apart to O(n^2) over many calls
    let mut n = 1;
    loop {
      let name = format!("F_{}_unknown_{}", base, n);
      if self.used.get(&name).is_none() {
        self.used.insert(name.clone());
        return name;
      }
      n += 1;
    }
  }
}

/// Suggestion flags for the annotations format, from the analysis' return
/// kind and any already-configured mode. Observed kinds are authoritative.
/// An unknown kind never downgrades an operator-resolved mode, but the
/// unresolved state must still persist: plain RET_UNKNOWN reaches only
/// newly-discovered functions, while RET_UNKNOWN_CONFIG_* preserve the
/// configured mode *and* carry the unresolved state, so confgen keeps
/// emitting the marker until the operator resolves the flag itself.
/// (See RET_UNKNOWN_MODES in confgen/hydra/annotations.py for the
/// cross-language contract: BSL mode + greppable marker + C TODO comment.)
fn ret_kind_flags(kind: Option<ReturnKind>, configured: Option<CallMode>) -> &'static str {
  match kind {
    Some(ReturnKind::Near) => ", flags = \"NEAR\"",
    Some(ReturnKind::Far) => "",
    // An interrupt handler returns via IRET, not RET/RETF, so it has no
    // near/far call mode by construction: never inherit a configured mode.
    // (Produced by OP_IRET since #50; previously unreachable dead code.)
    Some(ReturnKind::Interrupt) => ", flags = \"RET_UNKNOWN\"",
    None => match configured {
      Some(CallMode::Near) => ", flags = \"RET_UNKNOWN_CONFIG_NEAR\"",
      Some(CallMode::Far) => ", flags = \"RET_UNKNOWN_CONFIG_FAR\"",
      None => ", flags = \"RET_UNKNOWN\"",
    },
  }
}

/// Renders one analyzed function's suggestion output as ordered lines: the
/// unknown-kind diagnostic first (when applicable), then the F(...) entry.
/// Pure for testability — the caller only prints. Documents the
/// incomplete-inventory caveat: indirect-call and error branches (rendered
/// by render_ignored_indirect/render_ignored_error below) emit no F(...)
/// line at all, so grepping the output is not a complete inventory of
/// unresolved call modes.
fn render_suggestion(
  bare_name: &str,
  addr: SegOff,
  quoted_name: &str,
  ret_str: &str,
  args_str: &str,
  start: &str,
  end: &str,
  kind: Option<ReturnKind>,
  flags: &str,
  configured: Option<CallMode>,
) -> Vec<String> {
  let mut lines = vec![];
  if matches!(kind, None | Some(ReturnKind::Interrupt)) {
    // The diagnostic always prints: it is the operator signal, even when a
    // configured mode is preserved below.
    let reason = match kind {
      Some(ReturnKind::Interrupt) => "interrupt-return terminator; no near/far call mode applies",
      _ => "no return instruction observed (noreturn helper or tail-jump exit)",
    };
    // The "keeping configured" note only applies to plain unknown:
    // interrupt handlers never inherit a configured mode (see above).
    let kept = match (kind, configured) {
      (None, Some(CallMode::Near)) => " (keeping configured near)",
      (None, Some(CallMode::Far)) => " (keeping configured far)",
      _ => "",
    };
    // Interrupt handlers are CPU-dispatched via the IVT, never called, so
    // "resolve from the call sites" would be wrong advice for them: the
    // operator must pick a mode manually if the handler also needs to be
    // analyzed as a called function.
    let guidance = match kind {
      Some(ReturnKind::Interrupt) => "CPU-dispatched via the IVT (not called); to analyze it as a called function, set the call mode manually: near calls need flags = \"NEAR\", far calls need no flag",
      _ => "resolve the call mode from the call sites: near calls need flags = \"NEAR\", far calls need no flag",
    };
    lines.push(format!("    # RET KIND UNKNOWN | {} | {} | {}{}; {}", bare_name, addr, reason, kept, guidance));
  }
  lines.push(format!("    F( {:<30} {:<7} {:<12} {} {}{} ),", quoted_name, ret_str, args_str, start, end, flags));
  lines
}

fn render_ignored_indirect(name: &str, addr: SegOff, start: SegOff, end: SegOff, indirect_calls: usize) -> String {
  format!("    # IGNORED INDIRECT CALLS | {} | {} | start: {}  end: {}  indirect_calls: {}",
    name, addr, start, end, indirect_calls)
}

fn render_ignored_error(name: &str, addr: SegOff, err: &str) -> String {
  format!("    # IGNORED ERROR | {} | {} | error: '{}'", name, addr, err)
}

fn generate_annotations(functions: &BTreeMap<SegOff, Result<FuncDetails, String>>, cfg: &Config) {
  let mut function_names = FunctionNames::from_cfg(cfg);

  let mut current_seg = None;
  for (addr, result) in functions {
    let seg = addr.seg;

    let seg_name = match cfg.code_seg_lookup(seg) {
      Some(cs) => cs.name.clone(),
      None => format!("_{}", seg),
    };

    if Some(seg) != current_seg {
      println!("");
      println!("    ##################################################################################################################");
      println!("    ## Section {}: {}", seg, seg_name);
      current_seg = Some(seg);
    }


    let (name, ret, args, configured_mode) = match cfg.func_lookup(*addr) {
      Some(func) => (func.name.clone(), func.ret.clone(), func.args, Some(func.mode)),
      None       => (function_names.compute_unique(&seg_name), None, None, None),
    };

    match result {
      Ok(details) => {
        if details.indirect_calls > 0 {
          println!("{}", render_ignored_indirect(&name, *addr, details.start_addr, details.end_addr_inferred, details.indirect_calls));
        } else {
          // Capture the bare name before the rebind below: the guidance
          // line follows the sibling `# IGNORED ...` comment styles, which
          // print the bare name, not the quoted padded form.
          let bare_name = name.as_str();
          let name     = format!("\"{}\",", name);
          let start    = format!("\"{}\",", details.start_addr);
          let end      = format!("\"{}\"", details.end_addr_inferred);
          let flags    = ret_kind_flags(details.return_kind, configured_mode);

          let ret_str  = match ret {
            Some(ret) => format!("\"{}\",", ret),
            None      => "None,".to_string(),
          };

          let args_str  = match args {
            Some(args) => format!("{},", args),
            None       => "None,".to_string(),
          };

          for line in render_suggestion(
            bare_name, *addr, &name, &ret_str, &args_str, &start, &end,
            details.return_kind, flags, configured_mode,
          ) {
            println!("{}", line);
          }
        }
      }
      Err(err) => {
        println!("{}", render_ignored_error(&name, *addr, err));
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn ret_kind_flags_observed_kinds_are_authoritative() {
    assert_eq!(ret_kind_flags(Some(ReturnKind::Near), None), ", flags = \"NEAR\"");
    assert_eq!(ret_kind_flags(Some(ReturnKind::Far), None), "");
    // Even against a conflicting configured mode: analysis evidence wins.
    assert_eq!(ret_kind_flags(Some(ReturnKind::Near), Some(CallMode::Far)), ", flags = \"NEAR\"");
    assert_eq!(ret_kind_flags(Some(ReturnKind::Far), Some(CallMode::Near)), "");
  }

  #[test]
  fn ret_kind_flags_unknown_defers_to_configured_mode() {
    // Unresolved and unconfigured: the plain RET_UNKNOWN marker confgen
    // branches on (interim near + visible marker until the operator
    // resolves it).
    assert_eq!(ret_kind_flags(None, None), ", flags = \"RET_UNKNOWN\"");
    // Unresolved but already configured: preserve the configured mode *and*
    // carry the unresolved state, so confgen keeps emitting the marker and
    // regeneration never downgrades the decision back to unresolved.
    assert_eq!(ret_kind_flags(None, Some(CallMode::Near)), ", flags = \"RET_UNKNOWN_CONFIG_NEAR\"");
    assert_eq!(ret_kind_flags(None, Some(CallMode::Far)), ", flags = \"RET_UNKNOWN_CONFIG_FAR\"");
    // Interrupt handlers have no call mode by construction: never inherit,
    // even when a mode happens to be configured.
    assert_eq!(ret_kind_flags(Some(ReturnKind::Interrupt), None), ", flags = \"RET_UNKNOWN\"");
    assert_eq!(ret_kind_flags(Some(ReturnKind::Interrupt), Some(CallMode::Near)), ", flags = \"RET_UNKNOWN\"");
    assert_eq!(ret_kind_flags(Some(ReturnKind::Interrupt), Some(CallMode::Far)), ", flags = \"RET_UNKNOWN\"");
  }

  fn segoff(off: u16) -> SegOff {
    use crate::segoff::Off;
    SegOff { seg: Seg::Normal(0), off: Off(off) }
  }

  #[test]
  fn suggestion_renders_diagnostic_before_entry_with_bare_name() {
    let lines = render_suggestion(
      "F_new", segoff(0x100),
      "\"F_new\",", "None,", "None,", "\"0000:0100\",", "\"0000:0110\"",
      None, ", flags = \"RET_UNKNOWN\"", None,
    );
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("    # RET KIND UNKNOWN | F_new | 0000:0100 | no return instruction observed"),
      "unexpected diagnostic: {}", lines[0]);
    assert!(lines[1].starts_with("    F( "), "unexpected entry: {}", lines[1]);
    assert!(lines[1].contains("RET_UNKNOWN"), "unexpected entry: {}", lines[1]);
  }

  #[test]
  fn suggestion_preserves_configured_mode_with_kept_note() {
    let lines = render_suggestion(
      "F_cfg", segoff(0x200),
      "\"F_cfg\",", "None,", "None,", "\"0000:0200\",", "\"0000:0210\"",
      None, ", flags = \"RET_UNKNOWN_CONFIG_FAR\"", Some(CallMode::Far),
    );
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("(keeping configured far)"), "unexpected diagnostic: {}", lines[0]);
    assert!(lines[1].contains("RET_UNKNOWN_CONFIG_FAR"), "unexpected entry: {}", lines[1]);
  }

  #[test]
  fn suggestion_omits_diagnostic_for_known_kinds() {
    let lines = render_suggestion(
      "F_known", segoff(0x300),
      "\"F_known\",", "None,", "None,", "\"0000:0300\",", "\"0000:0310\"",
      Some(ReturnKind::Near), ", flags = \"NEAR\"", Some(CallMode::Near),
    );
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("NEAR"), "unexpected entry: {}", lines[0]);
  }

  #[test]
  fn suggestion_gives_interrupt_specific_guidance() {
    // Interrupt handlers are CPU-dispatched via the IVT, never called, so
    // the diagnostic must not advise resolving from call sites — and must
    // not inherit a configured mode, even when one is present.
    let lines = render_suggestion(
      "F_irq", segoff(0x400),
      "\"F_irq\",", "None,", "None,", "\"0000:0400\",", "\"0000:0401\"",
      Some(ReturnKind::Interrupt), ", flags = \"RET_UNKNOWN\"", Some(CallMode::Far),
    );
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("interrupt-return terminator"), "unexpected diagnostic: {}", lines[0]);
    assert!(lines[0].contains("CPU-dispatched via the IVT"), "unexpected diagnostic: {}", lines[0]);
    assert!(!lines[0].contains("keeping configured"), "interrupt must not inherit: {}", lines[0]);
    assert!(lines[1].contains("RET_UNKNOWN"), "unexpected entry: {}", lines[1]);
  }

  #[test]
  fn ignored_branches_emit_no_suggestion_line() {
    // Documents the incomplete-inventory caveat: these branches emit only
    // their comment line, so grepping suggestion output for unresolved
    // modes misses them by construction.
    let indirect = render_ignored_indirect("F_ind", segoff(0), segoff(0), segoff(0x10), 2);
    assert!(indirect.contains("IGNORED INDIRECT CALLS"), "unexpected line: {}", indirect);
    assert!(!indirect.contains("F("), "unexpected suggestion line: {}", indirect);
    let err = render_ignored_error("F_err", segoff(0), "boom");
    assert!(err.contains("IGNORED ERROR"), "unexpected line: {}", err);
    assert!(!err.contains("F("), "unexpected suggestion line: {}", err);
  }
}
