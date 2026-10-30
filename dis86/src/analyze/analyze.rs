use crate::binary::{Binary, Fmt};
use crate::config::{CallMode, Config, Func, FuncRegion};
use crate::segoff::{Off, SegOff};
use crate::coverage::{self, Coverage};

use super::workqueue::WorkQueue;
use super::code_segment::{CodeSegments};
use super::func_details::{FuncDetails, ReturnKind, first_undecodable_addr_in_regions};

use crate::asm::decode::Decoder;
use crate::spec::direct_target;

use std::collections::{BTreeMap, BTreeSet, HashSet};

pub struct Analyze {
  cfg: Config,
  binary: Binary,
  pub code_segments: CodeSegments,
}

/// Regions for analyzing the function resolved at `start`. A normal-segment
/// overlay stub resolves to the overlay function's entry, but the stub itself
/// is not part of the overlay body: analyze it with the legacy range path.
fn analysis_regions_for(func: &Func, start: SegOff) -> Vec<FuncRegion> {
  if func.start == start { func.analysis_regions() } else { vec![] }
}

/// Resolve a workqueue address for analysis. Calls target `Func::entry`, but
/// overlay workqueues start at the body address; fall back to `start` so the
/// body keeps its name and regions instead of becoming an unknown function.
fn analysis_func_lookup<'a>(cfg: &'a Config, addr: SegOff) -> Option<&'a Func> {
  cfg.func_lookup(addr)
    .or_else(|| cfg.funcs.iter().find(|func| func.start == addr))
}

fn seg_end_of(code_seg: &crate::analyze::code_segment::CodeSegment) -> u32 {
  code_seg.primary.skip_off + code_seg.primary.size
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

  /// Configured coverage for every analyzed code segment, written to stderr
  /// so stdout stays a pasteable annotation fragment.
  pub fn report_coverage(&self) {
    for func in &self.cfg.funcs {
      if func.effective_regions().is_empty() {
        eprintln!("WARN: {}: unknown end address; not counted in coverage", func.name);
      }
    }
    let mut total = Coverage::default();
    for code_seg in &self.code_segments.0 {
      let seg = code_seg.primary.seg;
      let seg_name = self.cfg.code_seg_lookup(seg)
        .map(|s| s.name.as_str())
        .unwrap_or("?");
      let seg_start = code_seg.primary.skip_off;
      let seg_end = seg_start + code_seg.primary.size;
      let cov = coverage::for_span(&self.cfg, seg, seg_start, seg_end);
      let mut max_end = seg_end_of(code_seg);
      for func in &self.cfg.funcs {
        for region in func.effective_regions() {
          if region.start.seg == seg && region.end.seg == seg {
            max_end = max_end.max(region.end.off.0 as u32);
          }
        }
      }
      for region in self.cfg.text_regions_matching_segment(seg) {
        if region.end.seg == seg {
          max_end = max_end.max(region.end.off.0 as u32);
        }
      }
      if max_end > seg_end_of(code_seg) {
        eprintln!("WARN: configured ranges exceed segment {} (expected end {}, got {})",
          seg, seg_end_of(code_seg), max_end);
      }
      eprintln!("Coverage {} ({}): code {} ({:.2}%), data {} ({:.2}%), unknown {} ({:.2}%), annotated {} / {} ({:.2}%)",
        seg, seg_name,
        cov.code, cov.percent(cov.code),
        cov.data, cov.percent(cov.data),
        cov.unknown, cov.percent(cov.unknown),
        cov.annotated(), cov.total, cov.percent(cov.annotated()));
      total.code += cov.code;
      total.data += cov.data;
      total.unknown += cov.unknown;
      total.total += cov.total;
    }
    eprintln!("Coverage total: code {} ({:.2}%), data {} ({:.2}%), unknown {} ({:.2}%), annotated {} / {} ({:.2}%)",
      total.code, total.percent(total.code),
      total.data, total.percent(total.data),
      total.unknown, total.percent(total.unknown),
      total.annotated(), total.total, total.percent(total.annotated()));
  }

  pub fn analyze_function(&self, name: &str) -> FuncDetails {
    let func = self.cfg.func_lookup_by_name(name).unwrap(); // FIXME
    let code_seg = self.code_segments.find_for_function(func).unwrap(); // FIXME
    assert!(func.start >= code_seg.start());
    let regions = func.analysis_regions();
    if regions.is_empty() {
      FuncDetails::build_with_regions_and_data(
        func.start, func.end, &[], code_seg, &self.binary, &self.cfg.text_section,
      ).unwrap()
    } else {
      FuncDetails::build_with_regions_and_data(
        func.start, func.outer_end(), &regions, code_seg, &self.binary, &self.cfg.text_section,
      ).unwrap()
    } // HAX FIXME
  }

  pub fn analyze_function_by_start(&self, start: SegOff) -> Result<FuncDetails, String> {
    let Some(code_seg) = self.code_segments.find_by_segment(start.seg) else {
      return Err(format!("Failed to find code segement"));
    };
    assert!(start >= code_seg.start());
    match analysis_func_lookup(&self.cfg, start) {
      Some(func) => {
        let regions = analysis_regions_for(func, start);
        if regions.is_empty() {
          FuncDetails::build_with_regions_and_data(
            start, func.end, &[], code_seg, &self.binary, &self.cfg.text_section,
          )
        } else {
          FuncDetails::build_with_regions_and_data(
            start, func.outer_end(), &regions, code_seg, &self.binary, &self.cfg.text_section,
          )
        }
      }
      None => FuncDetails::build_with_regions_and_data(
        start, None, &[], code_seg, &self.binary, &self.cfg.text_section,
      ),
    }
  }

  fn direct_targets_in_regions(&self, regions: &[FuncRegion]) -> BTreeSet<SegOff> {
    let mut targets = BTreeSet::new();
    for region in self.cfg.code_regions(regions) {
      let mut decoder = Decoder::new(self.binary.region_iter(region.start, region.end));
      while let Ok(Some((instr, _raw))) = decoder.try_next() {
        if let Some(target) = direct_target(&instr) {
          targets.insert(target);
        }
      }
    }
    targets
  }

  /// Warn when a code run after an untyped hole is neither a direct-transfer
  /// target nor the function entry. The code may still be reached indirectly,
  /// so this is a warning, not an error.
  fn warn_unreachable_after_holes(&self, func: &Func) {
    let regions = func.effective_regions();
    if regions.is_empty() { return; }
    let span_start = regions.first().map(|region| region.start).unwrap();
    let span_end = regions.last().map(|region| region.end).unwrap();
    let has_hole = self.cfg.text_section.iter().any(|data|
      data.is_data_hole()
        && data.start.seg == span_start.seg
        && data.end.seg == span_start.seg
        && data.start < span_end
        && span_start < data.end);
    if !has_hole { return; }
    let code_regions = self.cfg.code_regions(&regions);
    let targets = self.direct_targets_in_regions(&regions);
    for region in code_regions {
      if region.start.off.0 == 0 { continue; }
      let prev = SegOff { seg: region.start.seg, off: Off(region.start.off.0 - 1) };
      let Some(data) = self.cfg.data_region_containing(prev) else { continue; };
      if !data.is_data_hole() { continue; }
      if region.start == func.start || Some(region.start) == func.entry { continue; }
      if targets.contains(&region.start) { continue; }
      eprintln!("WARN: {}: code at {} follows declared data '{}' [{}, {}) but is not a branch target or entry in this function; it may be reached from another function or indirectly",
        func.name, region.start, data.name, data.start, data.end);
    }
  }

  // Scan known functions to find new functions, then scan those, return a big list of all found functions
  pub fn scan_for_all_functions(&self, emit_annotation_format: bool) {
    let mut workqueue = WorkQueue::new();

    // init work queue with known config functions
    for f in &self.cfg.funcs {
      workqueue.insert(f.start);
    }

    let mut functions = BTreeMap::new();
    let mut call_targets_in_data: BTreeMap<SegOff, BTreeSet<SegOff>> = BTreeMap::new();
    while let Some(addr) = workqueue.pop() {
      let result = self.analyze_function_by_start(addr);

      // Add all new calls to the work queue, except calls that point at
      // declared data. Those are reported in the annotation output instead
      // of being analyzed as bogus functions.
      if let Ok(details) = &result {
        for call in &details.direct_calls {
          if self.cfg.data_region_containing(*call).is_some() {
            call_targets_in_data.entry(addr).or_default().insert(*call);
          } else {
            workqueue.insert(*call);
          }
        }
        if let Some(func) = analysis_func_lookup(&self.cfg, addr) {
          if func.start == addr {
            self.warn_unreachable_after_holes(func);
          }
        }
      }

      functions.insert(addr, result);
    }

    if emit_annotation_format {
      // Synthesize annotations
      generate_annotations(&functions, &self.cfg, &self.binary, &call_targets_in_data);
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

    let name = match analysis_func_lookup(cfg, *addr) {
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
    // An interrupt handler returns via IRET, not RET/RETF, so the IRET
    // itself implies no near/far call mode: never *guess* from the kind.
    // An operator-configured mode is still inherited (with the unresolved
    // marker), so resolving a handler's mode survives regeneration exactly
    // like a plain unknown (see the None arm below).
    Some(ReturnKind::Interrupt) => match configured {
      Some(CallMode::Near) => ", flags = \"RET_UNKNOWN_CONFIG_NEAR\"",
      Some(CallMode::Far) => ", flags = \"RET_UNKNOWN_CONFIG_FAR\"",
      None => ", flags = \"RET_UNKNOWN\"",
    },
    None => match configured {
      Some(CallMode::Near) => ", flags = \"RET_UNKNOWN_CONFIG_NEAR\"",
      Some(CallMode::Far) => ", flags = \"RET_UNKNOWN_CONFIG_FAR\"",
      None => ", flags = \"RET_UNKNOWN\"",
    },
  }
}

/// Renders one analyzed function's suggestion output as up to three
/// ordered lines: the `# RET KIND UNKNOWN` diagnostic (for unanalyzable
/// or interrupt kinds), the `# MIXED RETURN KINDS` diagnostic (when the
/// function has exits of differing kinds and the classification came
/// from the first exit in ascending block order only), then the F(...)
/// entry. Pure for testability — the caller only prints.
/// Incomplete-inventory caveat: indirect-call and error branches
/// (rendered by render_ignored_indirect/render_ignored_error below) emit
/// no F(...) line at all, so grepping the output is not a complete
/// inventory of unresolved call modes.
fn suggestion_end(inferred: SegOff, configured: Option<SegOff>) -> SegOff {
  configured.unwrap_or(inferred)
}

fn render_region_arg(regions: &[FuncRegion]) -> String {
  let items = regions.iter()
    .map(|region| format!("(\"{}\", \"{}\")", region.start, region.end))
    .collect::<Vec<_>>()
    .join(", ");
  format!(", regions = [{}]", items)
}

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
  ignored: &[ReturnKind],
  configured_regions: Option<&[FuncRegion]>,
) -> Vec<String> {
  let mut lines = vec![];
  if matches!(kind, None | Some(ReturnKind::Interrupt)) {
    // The diagnostic always prints: it is the operator signal, even when a
    // configured mode is preserved below.
    let reason = match kind {
      Some(ReturnKind::Interrupt) => "interrupt-return terminator; no near/far call mode applies",
      _ => "no return instruction observed (noreturn helper or tail-jump exit)",
    };
    // The "keeping configured" note applies to plain unknown and to
    // interrupt handlers alike: both preserve the operator's mode while
    // carrying the unresolved marker.
    let kept = match (kind, configured) {
      (None | Some(ReturnKind::Interrupt), Some(CallMode::Near)) => " (keeping configured near)",
      (None | Some(ReturnKind::Interrupt), Some(CallMode::Far)) => " (keeping configured far)",
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
  if !ignored.is_empty() {
    // A mixed-kind function: the classification `kind` came from the
    // first exit in ascending block order only (block-pop order, not
    // necessarily the lowest-address instruction: overlapping blocks
    // from mid-instruction jump targets are possible), so the annotated
    // call mode may not fit every exit path. `kind` is always Some here
    // (an ignored kind is only recorded after a first kind was observed);
    // the `if let` keeps that invariant structural for future callers.
    if let Some(kind) = kind {
      let others: Vec<String> = ignored.iter().map(|k| k.to_string()).collect();
      lines.push(format!("    # MIXED RETURN KINDS | {} | {} | classified {} by the first exit in ascending block order; other exits: {}; the annotated call mode may not fit every exit path",
        bare_name, addr, kind, others.join(", ")));
    }
  }
  let region_arg = configured_regions
    .filter(|regions| !regions.is_empty())
    .map(render_region_arg)
    .unwrap_or_default();
  lines.push(format!("    F( {:<30} {:<7} {:<12} {} {}{}{} ),", quoted_name, ret_str, args_str, start, end, flags, region_arg));
  lines
}

fn render_ignored_indirect(name: &str, addr: SegOff, start: SegOff, end: SegOff, indirect_calls: usize) -> String {
  format!("    # IGNORED INDIRECT CALLS | {} | {} | start: {}  end: {}  indirect_calls: {}",
    name, addr, start, end, indirect_calls)
}

fn render_ignored_error(name: &str, addr: SegOff, err: &str) -> String {
  format!("    # IGNORED ERROR | {} | {} | error: '{}'", name, addr, err)
}

/// Renders the #65 diagnostic: the suggested `[start, end)` spans bytes
/// that fail a linear decode (embedded data jumped over), so a config
/// built verbatim from the suggestion aborts `--codeseg-name` with
/// "Unsupported or invalid instruction" at `gap`. Pure for testability.
fn render_undecodable_gap(name: &str, addr: SegOff, start: SegOff, end: SegOff, gap: SegOff) -> String {
  format!("    # UNDECODABLE GAP | {} | {} | linear decode of [{}, {}) fails at {}; the suggested range spans non-code bytes - audit/shorten the range before use",
    name, addr, start, end, gap)
}

fn render_call_target_in_data(name: &str, target: SegOff, data: &crate::config::TextSectionRegion) -> String {
  format!("    # CALL TARGET IN DATA | {} | {} | {} [{}, {})",
    name, target, data.name, data.start, data.end)
}

fn generate_annotations(
  functions: &BTreeMap<SegOff, Result<FuncDetails, String>>,
  cfg: &Config,
  binary: &Binary,
  call_targets_in_data: &BTreeMap<SegOff, BTreeSet<SegOff>>,
) {
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


    let (name, ret, args, configured_mode, suggestion_regions, configured_end) =
      match analysis_func_lookup(cfg, *addr) {
        Some(func) => {
          let has_regions = func.start == *addr && !func.regions.is_empty();
          (
            func.name.clone(),
            func.ret.clone(),
            func.args,
            Some(func.mode),
            if has_regions { Some(func.regions.clone()) } else { None },
            if has_regions { func.outer_end() } else { None },
          )
        }
        None => (function_names.compute_unique(&seg_name), None, None, None, None, None),
      };

    match result {
      Ok(details) => {
        // #65: analysis follows control flow, so the inferred end can
        // span bytes that are not code; decompilation decodes the range
        // linearly and aborts there. Probe the net code runs so declared
        // data holes are not reported as false gaps.
        let configured_regions = analysis_func_lookup(cfg, *addr)
          .map(|func| analysis_regions_for(func, *addr))
          .filter(|regions| !regions.is_empty());
        let probe_regions = match &configured_regions {
          Some(regions) => cfg.code_regions(regions),
          None if details.end_addr_inferred > details.start_addr => cfg.code_regions(&[
            FuncRegion { start: details.start_addr, end: details.end_addr_inferred },
          ]),
          None => vec![],
        };
        if let Some((region, gap)) = first_undecodable_addr_in_regions(binary, &probe_regions) {
          println!("{}", render_undecodable_gap(
            &name, *addr, region.start, region.end, gap));
        }
        if let Some(targets) = call_targets_in_data.get(addr) {
          for target in targets {
            if let Some(data) = cfg.data_region_containing(*target) {
              println!("{}", render_call_target_in_data(&name, *target, data));
            }
          }
        }
        if details.indirect_calls > 0 {
          println!("{}", render_ignored_indirect(&name, *addr, details.start_addr, details.end_addr_inferred, details.indirect_calls));
        } else {
          // Capture the bare name before the rebind below: the guidance
          // line follows the sibling `# IGNORED ...` comment styles, which
          // print the bare name, not the quoted padded form.
          let bare_name = name.as_str();
          let name     = format!("\"{}\",", name);
          let start    = format!("\"{}\",", details.start_addr);
          let end      = format!("\"{}\"", suggestion_end(details.end_addr_inferred, configured_end));
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
            &details.ignored_return_kinds, suggestion_regions.as_deref(),
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
  use crate::segoff::Seg;

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
    // Interrupt handlers: the IRET itself implies no mode (never guess),
    // but an operator-configured mode is inherited with the marker, so a
    // manual resolution survives regeneration like a plain unknown.
    assert_eq!(ret_kind_flags(Some(ReturnKind::Interrupt), None), ", flags = \"RET_UNKNOWN\"");
    assert_eq!(ret_kind_flags(Some(ReturnKind::Interrupt), Some(CallMode::Near)), ", flags = \"RET_UNKNOWN_CONFIG_NEAR\"");
    assert_eq!(ret_kind_flags(Some(ReturnKind::Interrupt), Some(CallMode::Far)), ", flags = \"RET_UNKNOWN_CONFIG_FAR\"");
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
      None, ", flags = \"RET_UNKNOWN\"", None, &[], None,
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
      None, ", flags = \"RET_UNKNOWN_CONFIG_FAR\"", Some(CallMode::Far), &[], None,
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
      Some(ReturnKind::Near), ", flags = \"NEAR\"", Some(CallMode::Near), &[], None,
    );
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("NEAR"), "unexpected entry: {}", lines[0]);
  }

  #[test]
  fn overlay_stub_entry_uses_the_legacy_analysis_path() {
    use crate::segoff::Off;
    let body_start = SegOff { seg: Seg::Overlay(0), off: Off(0) };
    let body_end = SegOff { seg: Seg::Overlay(0), off: Off(4) };
    let stub = segoff(0x20);
    let func = Func {
      name: "ovl".to_string(),
      start: body_start,
      end: Some(body_end),
      entry: Some(stub),
      mode: CallMode::Far,
      ret: None,
      args: Some(0),
      regargs: None,
      regouts: None,
      clobbers: None,
      dont_pop_args: false,
      regions: vec![FuncRegion { start: body_start, end: body_end }],
    };
    assert!(analysis_regions_for(&func, stub).is_empty(),
      "the stub address must not inherit the overlay body regions");
    assert_eq!(analysis_regions_for(&func, body_start),
      vec![FuncRegion { start: body_start, end: body_end }]);

    let cfg = Config {
      types: std::rc::Rc::new(crate::types::TypeDatabase::new()),
      structs: vec![],
      code_segs: vec![],
      funcs: vec![func],
      indirects: vec![],
      globals: vec![],
      text_section: vec![],
    };
    assert_eq!(analysis_func_lookup(&cfg, stub).map(|f| f.name.as_str()), Some("ovl"));
    let body = analysis_func_lookup(&cfg, body_start).expect("body start resolves");
    assert_eq!(analysis_regions_for(body, body_start),
      vec![FuncRegion { start: body_start, end: body_end }]);
  }

  #[test]
  fn suggestion_end_keeps_the_configured_region_bound() {
    assert_eq!(suggestion_end(segoff(0x10), None), segoff(0x10));
    assert_eq!(suggestion_end(segoff(0x10), Some(segoff(0x20))), segoff(0x20),
      "a trailing region past the inferred end must not make the suggestion invalid");
  }

  #[test]
  fn suggestion_preserves_explicit_regions() {
    let regions = [
      FuncRegion { start: segoff(0x800), end: segoff(0x810) },
      FuncRegion { start: segoff(0x880), end: segoff(0x890) },
    ];
    let lines = render_suggestion(
      "F_regions", segoff(0x800),
      "\"F_regions\",", "None,", "None,", "\"0000:0800\",", "\"0000:0890\"",
      Some(ReturnKind::Near), ", flags = \"NEAR\"", Some(CallMode::Near), &[],
      Some(&regions),
    );
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains(
      "regions = [(\"0000:0800\", \"0000:0810\"), (\"0000:0880\", \"0000:0890\")]"),
      "unexpected entry: {}", lines[0]);
  }

  #[test]
  fn suggestion_gives_interrupt_specific_guidance() {
    // Interrupt handlers are CPU-dispatched via the IVT, never called, so
    // the diagnostic must not advise resolving from call sites. A
    // configured mode is still inherited (with the marker) so a manual
    // resolution survives regeneration; unconfigured stays plain unknown.
    let lines = render_suggestion(
      "F_irq", segoff(0x400),
      "\"F_irq\",", "None,", "None,", "\"0000:0400\",", "\"0000:0401\"",
      Some(ReturnKind::Interrupt), ", flags = \"RET_UNKNOWN_CONFIG_FAR\"", Some(CallMode::Far), &[], None,
    );
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("interrupt-return terminator"), "unexpected diagnostic: {}", lines[0]);
    assert!(lines[0].contains("CPU-dispatched via the IVT"), "unexpected diagnostic: {}", lines[0]);
    assert!(lines[0].contains("(keeping configured far)"), "unexpected diagnostic: {}", lines[0]);
    assert!(lines[1].contains("RET_UNKNOWN_CONFIG_FAR"), "unexpected entry: {}", lines[1]);

    let unconfigured = render_suggestion(
      "F_irq2", segoff(0x500),
      "\"F_irq2\",", "None,", "None,", "\"0000:0500\",", "\"0000:0501\"",
      Some(ReturnKind::Interrupt), ", flags = \"RET_UNKNOWN\"", None, &[], None,
    );
    assert_eq!(unconfigured.len(), 2);
    assert!(!unconfigured[0].contains("keeping configured"), "unexpected diagnostic: {}", unconfigured[0]);
    assert!(unconfigured[1].contains("\"RET_UNKNOWN\""), "unexpected entry: {}", unconfigured[1]);
  }

  #[test]
  fn suggestion_flags_mixed_return_kinds() {
    // A function whose exits differ in kind keeps the first-in-block-order
    // classification but gains a `# MIXED RETURN KINDS` line naming the
    // dropped exits, so the operator can verify the annotated call
    // mode fits every exit path. Homogeneous functions emit no such line.
    let ignored = [ReturnKind::Interrupt, ReturnKind::Far];
    let lines = render_suggestion(
      "F_mix", segoff(0x600),
      "\"F_mix\",", "None,", "None,", "\"0000:0600\",", "\"0000:0610\"",
      Some(ReturnKind::Near), ", flags = \"NEAR\"", None, &ignored, None,
    );
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("    # MIXED RETURN KINDS | F_mix | 0000:0600 | classified near by the first exit in ascending block order; other exits: interrupt, far"),
      "unexpected diagnostic: {}", lines[0]);
    assert!(lines[1].contains("NEAR"), "unexpected entry: {}", lines[1]);
  }

  #[test]
  fn suggestion_mixed_needs_a_classified_kind() {
    // Structural pin for the mixed diagnostic: with no classified kind
    // (None) there is nothing to attribute the dropped exits to, so no
    // MIXED line is emitted — and, critically, no panic. Only reachable
    // by future callers; FuncDetails never produces this combination.
    let lines = render_suggestion(
      "F_none", segoff(0x700),
      "\"F_none\",", "None,", "None,", "\"0000:0700\",", "\"0000:0710\"",
      None, ", flags = \"RET_UNKNOWN\"", None, &[ReturnKind::Interrupt], None,
    );
    assert!(lines.iter().all(|l| !l.contains("MIXED RETURN KINDS")),
      "unexpected mixed diagnostic: {:?}", lines);
    assert_eq!(lines.len(), 2);
  }
  #[test]
  fn suggestion_flags_undecodable_gap() {
    // #65: the diagnostic names the function, its start, the suggested
    // range, and the failing address, and tells the operator to
    // audit/shorten the range before use.
    let line = render_undecodable_gap("F_gap", segoff(0), segoff(0), segoff(0x0b), segoff(3));
    assert!(line.starts_with("    # UNDECODABLE GAP | F_gap | 0000:0000 |"),
      "unexpected diagnostic: {}", line);
    assert!(line.contains("[0000:0000, 0000:000b)"), "unexpected diagnostic: {}", line);
    assert!(line.contains("fails at 0000:0003"), "unexpected diagnostic: {}", line);
    assert!(line.contains("audit/shorten"), "unexpected diagnostic: {}", line);
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
