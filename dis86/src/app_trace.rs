//! The `--trace-analyze` mode: what a recorded run says about the program
//! it ran (issue #97).
//!
//! The output has three parts: the summary of what the run did, the
//! function-level call graph, and the suggestion lines for the functions
//! the run discovered that the configuration does not know yet. The
//! suggestion lines are the same shape `--analyze` prints, so a trace and a
//! static analysis can be pasted into the same annotation file.

use std::collections::BTreeMap;

use crate::config::Config;
use crate::segoff::SegOff;
use crate::trace::{analyze, parse_trace, TraceAnalysis, FunctionInfo};

pub fn run(cfg: Option<&Config>, trace_path: &str) -> i32 {
  let text = match std::fs::read_to_string(trace_path) {
    Ok(text) => text,
    Err(err) => {
      eprintln!("Error: cannot read the trace '{}': {}.", trace_path, err);
      return 1;
    }
  };

  let events = match parse_trace(&text) {
    Ok(events) => events,
    Err(err) => {
      eprintln!("Error: {} is not a readable trace: {}.", trace_path, err);
      return 1;
    }
  };

  let analysis = analyze(&events);

  println!("=== trace analysis: {} ===", trace_path);
  println!("events: {}", analysis.events);
  println!("  hook dispatches:  {}", analysis.dispatches);
  println!("  calls:            {}", analysis.calls);
  println!("  returns:          {}", analysis.returns);
  println!("  native calls:     {}", analysis.native_calls);
  println!("  native rets:      {}", analysis.native_returns);
  println!("  instructions:     {}", analysis.steps);
  println!("  interrupts:       {}", analysis.interrupts);
  println!("functions seen: {}", analysis.functions.len());

  print_call_graph(cfg, &analysis);

  let discovered = print_suggestions(cfg, &analysis);
  if discovered == 0 {
    println!("no function the configuration does not know was discovered");
  }

  0
}

/// The call graph, one function per block.
fn print_call_graph(cfg: Option<&Config>, analysis: &TraceAnalysis) {
  if analysis.functions.is_empty() { return; }

  println!();
  println!("call graph:");
  for (addr, info) in &analysis.functions {
    let name = function_name(cfg, *addr, info);
    println!("  {} {}", addr, name);
    for callee in &info.callees {
      let callee_info = analysis.functions.get(callee);
      let callee_name = match callee_info {
        Some(i) => function_name(cfg, *callee, i),
        None => String::new(),
      };
      if callee_name.is_empty() {
        println!("    -> {}", callee);
      } else {
        println!("    -> {} {}", callee, callee_name);
      }
    }
    // The callers are the other end of the same edges: the call seam knows
    // both, so the graph can be read either way.
    for caller in &info.callers {
      let caller_info = analysis.functions.get(caller);
      let caller_name = match caller_info {
        Some(i) => function_name(cfg, *caller, i),
        None => String::new(),
      };
      if caller_name.is_empty() {
        println!("    <- {}", caller);
      } else {
        println!("    <- {} {}", caller, caller_name);
      }
    }
  }
}

/// The suggestion lines for the functions the run discovered.
///
/// The end offset is left empty: tracing sees where a function was
/// entered, not where it stops. The return type is left unknown too: the
/// trace records the registers the callee left behind, and a callee that
/// returns 16 bits in AX leaves whatever was in DX. The argument count is
/// exact - it is the number of words the call stub pushed.
fn print_suggestions(cfg: Option<&Config>, analysis: &TraceAnalysis) -> usize {
  let mut names: BTreeMap<u16, usize> = BTreeMap::new();
  let mut count = 0;

  for (addr, info) in &analysis.functions {
    if let Some(cfg) = cfg {
      if cfg.func_lookup(*addr).is_some() { continue; }
    }

    let seg_name = segment_name(cfg, *addr);
    let n = {
      let counter = names.entry(addr.seg.unwrap_normal()).or_insert(0);
      *counter += 1;
      *counter
    };
    let name = format!("F_{}_unknown_{}", seg_name, n);

    let quoted_name = format!("\"{}\",", name);
    let ret_str = "None,".to_string();
    let args_str = match info.observed_args {
      Some(args) => format!("{},", args),
      None => "None,".to_string(),
    };
    let start = format!("\"{}\",", addr);
    let end = "\"\"".to_string();

    println!();
    println!("    # DISCOVERED BY TRACE | {} | name={} | args={} | the end offset is not inferred by tracing; the return type is not inferred either (the trace records the registers the callee left behind)",
             addr,
             info.name.clone().unwrap_or_else(|| "(unnamed)".to_string()),
             match info.observed_args { Some(a) => a.to_string(), None => "unknown".to_string() });
    if info.args_conflict {
      println!("    # ARGS DISAGREE | {} | two call seams reported different argument counts; the highest one is shown", addr);
    }
    println!("    F( {:<30} {:<7} {:<12} {} {}{} ),", quoted_name, ret_str, args_str, start, end, "");

    count += 1;
  }

  count
}

/// The name to show for a function: the configuration's, when it has one,
/// otherwise the name the trace carried.
fn function_name(cfg: Option<&Config>, addr: SegOff, info: &FunctionInfo) -> String {
  if let Some(cfg) = cfg {
    if let Some(func) = cfg.func_lookup(addr) {
      return func.name.clone();
    }
  }
  match &info.name {
    Some(name) => name.clone(),
    None => "(unnamed)".to_string(),
  }
}

/// The segment name the suggestion lines use: the configuration's code
/// segment name when there is one, the segment number otherwise.
fn segment_name(cfg: Option<&Config>, addr: SegOff) -> String {
  if let Some(cfg) = cfg {
    if let Some(seg) = cfg.code_seg_lookup(addr.seg) {
      return seg.name.clone();
    }
  }
  format!("{:04x}", addr.seg.unwrap_normal())
}
