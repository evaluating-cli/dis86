//! The Hydra trace format, and what can be learned from it (issue #97).
//!
//! The trace is a line-oriented text file: a version header, the load
//! segment in a comment line, and one event per line with `key=value`
//! fields. This module parses it and turns it into what a reverse engineer
//! wants out of a run: the function boundaries the run touched, the calls
//! between them, and the arguments and results observed at the call
//! boundaries.
//!
//! Two things the trace does not say are reported as unknown here rather
//! than guessed:
//!
//!   - the *end* of a discovered function. Tracing sees where a function
//!     was entered, not where it stops; the end offset still has to come
//!     from the operator or from the static analysis.
//!   - the *return type* of a call. The trace records the registers the
//!     callee left behind, and a callee that returns 16 bits in AX leaves
//!     whatever was in DX, so a non-zero DX does not mean a 32-bit return.
//!
//! The argument count, on the other hand, is exact: it is the number of
//! words the call stub pushed, which is what the configuration's `args`
//! field states.

use std::collections::{BTreeMap, BTreeSet};

use crate::segoff::SegOff;

/// The version line every trace starts with.
pub const TRACE_HEADER: &str = "# hydra trace v1";

/// A parsed event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceEvent {
  /// A hook was dispatched (`enter`) or returned (`leave`). The address is
  /// the image-relative entry of the hook.
  Dispatch { enter: bool, addr: Option<SegOff>, name: Option<String>, result: Option<String> },
  /// A guest call boundary seen by the callstack model: the guest reached
  /// the callee's entry (`to`), coming from the address it will return to
  /// (`from`, a return address in the caller, not a function entry).
  Call { from: Option<SegOff>, to: Option<SegOff>, name: Option<String> },
  /// A guest return boundary seen by the callstack model: the guest left
  /// the function at `from` and continued at `to` (the return address in
  /// the caller).
  Ret { from: Option<SegOff>, to: Option<SegOff> },
  /// The native-to-guest call seam: the callee, and the arguments it was
  /// handed (read back from the guest stack).
  NativeCall { from: Option<SegOff>, to: Option<SegOff>, name: Option<String>, nargs: u16, args: Vec<u16> },
  /// The registers the callee left behind at that seam.
  NativeRet { from: Option<SegOff>, to: Option<SegOff>, dx: u16, ax: u16 },
  /// One guest instruction.
  Step { addr: Option<SegOff> },
  /// An interrupt that was serviced.
  Int { num: u8, ah: u8 },
}

/// Parse a trace. The version header is required, an unknown event is an
/// error, and a malformed field is an error: a trace that does not parse
/// is a trace nobody can trust.
pub fn parse_trace(text: &str) -> Result<Vec<TraceEvent>, String> {
  let mut events = Vec::new();
  let mut code_load: Option<u16> = None;
  let mut seen_header = false;

  for (idx, raw) in text.lines().enumerate() {
    let lineno = idx + 1;
    let line = raw.trim();
    if line.is_empty() { continue; }

    if let Some(rest) = line.strip_prefix('#') {
      let rest = rest.trim();
      if rest == "hydra trace v1" {
        if seen_header { return Err(format!("line {}: duplicate version header", lineno)); }
        if lineno != 1 { return Err(format!("line {}: the version header must be the first line", lineno)); }
        seen_header = true;
        continue;
      }
      if let Some(value) = rest.strip_prefix("code_load=") {
        code_load = Some(parse_hex_u16(value)
          .map_err(|e| format!("line {}: bad code load: {}", lineno, e))?);
        continue;
      }
      // Any other comment is a comment.
      continue;
    }

    if !seen_header {
      return Err(format!("line {}: the trace does not start with '{}'", lineno, TRACE_HEADER));
    }

    let mut tokens = line.split_whitespace();
    let kind = tokens.next().unwrap_or("");
    let rest: Vec<&str> = tokens.collect();

    let event = match kind {
      "dispatch" => {
        if rest.is_empty() { return Err(format!("line {}: dispatch without a phase", lineno)); }
        let phase = rest[0];
        let enter = match phase {
          "enter" => true,
          "leave" => false,
          other => return Err(format!("line {}: unknown dispatch phase '{}'", lineno, other)),
        };
        let fields = parse_fields(&rest[1..])
          .map_err(|e| format!("line {}: {}", lineno, e))?;
        let (cs, ip) = need_addr(&fields, "cs", "ip", lineno)?;
        TraceEvent::Dispatch {
          enter,
          addr: resolve(cs, ip, code_load),
          name: opt_str(&fields, "name"),
          result: opt_str(&fields, "result"),
        }
      }
      "call" | "ret" => {
        let fields = parse_fields(&rest).map_err(|e| format!("line {}: {}", lineno, e))?;
        let (fs, fo) = need_segoff(&fields, "from", lineno)?;
        let (ts, to) = need_segoff(&fields, "to", lineno)?;
        let from = resolve(fs, fo, code_load);
        let to = resolve(ts, to, code_load);
        if kind == "call" {
          TraceEvent::Call { from, to, name: opt_str(&fields, "name") }
        } else {
          TraceEvent::Ret { from, to }
        }
      }
      "native_call" => {
        let fields = parse_fields(&rest).map_err(|e| format!("line {}: {}", lineno, e))?;
        let (fs, fo) = need_segoff(&fields, "from", lineno)?;
        let (ts, to) = need_segoff(&fields, "to", lineno)?;
        let nargs = need_u16(&fields, "nargs", lineno)?;
        let mut args = Vec::new();
        for i in 0..nargs {
          args.push(need_u16(&fields, &format!("arg{}", i), lineno)?);
        }
        TraceEvent::NativeCall {
          from: resolve(fs, fo, code_load),
          to: resolve(ts, to, code_load),
          name: opt_str(&fields, "name"),
          nargs,
          args,
        }
      }
      "native_ret" => {
        let fields = parse_fields(&rest).map_err(|e| format!("line {}: {}", lineno, e))?;
        let (fs, fo) = need_segoff(&fields, "from", lineno)?;
        let (ts, to) = need_segoff(&fields, "to", lineno)?;
        TraceEvent::NativeRet {
          from: resolve(fs, fo, code_load),
          to: resolve(ts, to, code_load),
          dx: need_u16(&fields, "dx", lineno)?,
          ax: need_u16(&fields, "ax", lineno)?,
        }
      }
      "step" => {
        let fields = parse_fields(&rest).map_err(|e| format!("line {}: {}", lineno, e))?;
        let (cs, ip) = need_addr(&fields, "cs", "ip", lineno)?;
        TraceEvent::Step { addr: resolve(cs, ip, code_load) }
      }
      "int" => {
        let fields = parse_fields(&rest).map_err(|e| format!("line {}: {}", lineno, e))?;
        TraceEvent::Int {
          num: need_u16(&fields, "num", lineno)? as u8,
          ah: need_u16(&fields, "ah", lineno)? as u8,
        }
      }
      other => return Err(format!("line {}: unknown event '{}'", lineno, other)),
    };

    events.push(event);
  }

  if !seen_header {
    return Err(format!("the trace does not start with '{}'", TRACE_HEADER));
  }

  Ok(events)
}

/// What a run touched.
#[derive(Debug, Default, Clone)]
pub struct FunctionInfo {
  /// The name the metadata gave the address, when it knew one.
  pub name: Option<String>,
  /// The functions this one was observed to call.
  pub callees: BTreeSet<SegOff>,
  /// The functions this one was observed to be called from.
  pub callers: BTreeSet<SegOff>,
  /// The addresses the calls into this function were seen at. These are
  /// *return addresses* in the caller, not function entries: the guest ran
  /// free between the breakpoints, so the caller's own entry is not known
  /// from them.
  pub call_sites: BTreeSet<SegOff>,
  /// The argument count observed at the call seam, when there was one.
  pub observed_args: Option<u16>,
  /// Set when two call seams disagreed about the argument count.
  pub args_conflict: bool,
  /// How often the function was dispatched.
  pub dispatches: u64,
  /// How often the function returned.
  pub returns: u64,
  /// The registers the callee left behind at the call seam.
  pub results: Vec<(u16, u16)>,
}

#[derive(Debug, Default)]
pub struct TraceAnalysis {
  pub functions: BTreeMap<SegOff, FunctionInfo>,
  pub events: u64,
  pub dispatches: u64,
  pub calls: u64,
  pub returns: u64,
  pub native_calls: u64,
  pub native_returns: u64,
  pub steps: u64,
  pub interrupts: u64,
}

impl TraceAnalysis {
  fn entry(&mut self, addr: SegOff) -> &mut FunctionInfo {
    self.functions.entry(addr).or_default()
  }
}

/// Turn the events into the function-level picture.
pub fn analyze(events: &[TraceEvent]) -> TraceAnalysis {
  let mut a = TraceAnalysis { events: events.len() as u64, ..Default::default() };

  for event in events {
    match event {
      TraceEvent::Dispatch { enter, addr, name, .. } => {
        a.dispatches += 1;
        let Some(addr) = addr else { continue };
        let f = a.entry(*addr);
        if *enter { f.dispatches += 1; } else { f.returns += 1; }
        if f.name.is_none() { f.name = name.clone(); }
      }
      TraceEvent::Call { from, to, name } => {
        a.calls += 1;
        // The guest reached the callee's entry; the address it came from is
        // the return address in the caller, not a function entry.
        if let Some(to) = to {
          {
            let f = a.entry(*to);
            if f.name.is_none() { f.name = name.clone(); }
          }
          if let Some(from) = from {
            a.entry(*to).call_sites.insert(*from);
          }
        }
      }
      TraceEvent::Ret { from, to } => {
        a.returns += 1;
        // The guest left the function at `from` and continued at `to`, which
        // is the return address in the caller.
        if let Some(from) = from {
          if let Some(to) = to {
            a.entry(*from).call_sites.insert(*to);
          }
        }
      }
      TraceEvent::NativeCall { from, to, name, nargs, .. } => {
        a.native_calls += 1;
        if let Some(to) = to {
          {
            let f = a.entry(*to);
            if f.name.is_none() { f.name = name.clone(); }
            match f.observed_args {
              None => f.observed_args = Some(*nargs),
              Some(seen) if seen == *nargs => {}
              Some(_) => f.args_conflict = true,
            }
          }
          if let Some(from) = from {
            a.entry(*from).callees.insert(*to);
            a.entry(*to).callers.insert(*from);
          }
        }
      }
      TraceEvent::NativeRet { to, dx, ax, .. } => {
        a.native_returns += 1;
        if let Some(to) = to {
          a.entry(*to).results.push((*dx, *ax));
        }
      }
      TraceEvent::Step { .. } => a.steps += 1,
      TraceEvent::Int { .. } => a.interrupts += 1,
    }
  }

  a
}

// -----------------------------------------------------------------------
// parsing helpers
// -----------------------------------------------------------------------

fn parse_fields<'a>(tokens: &'a [&'a str]) -> Result<BTreeMap<&'a str, &'a str>, String> {
  let mut out = BTreeMap::new();
  for token in tokens {
    let Some((key, value)) = token.split_once('=') else {
      return Err(format!("expected a key=value field, got '{}'", token));
    };
    if out.insert(key, value).is_some() {
      return Err(format!("duplicate field '{}'", key));
    }
  }
  Ok(out)
}

fn opt_str(fields: &BTreeMap<&str, &str>, key: &str) -> Option<String> {
  fields.get(key).map(|v| v.to_string())
}

fn need_u16(fields: &BTreeMap<&str, &str>, key: &str, lineno: usize) -> Result<u16, String> {
  let value = fields.get(key)
    .ok_or_else(|| format!("line {}: missing field '{}'", lineno, key))?;
  parse_hex_u16(value).map_err(|e| format!("line {}: bad field '{}': {}", lineno, key, e))
}

fn need_addr(fields: &BTreeMap<&str, &str>, seg_key: &str, off_key: &str, lineno: usize)
  -> Result<(u16, u16), String>
{
  let seg = need_u16(fields, seg_key, lineno)?;
  let off = need_u16(fields, off_key, lineno)?;
  Ok((seg, off))
}

fn need_segoff(fields: &BTreeMap<&str, &str>, key: &str, lineno: usize) -> Result<(u16, u16), String> {
  let value = fields.get(key)
    .ok_or_else(|| format!("line {}: missing field '{}'", lineno, key))?;
  let Some((seg, off)) = value.split_once(':') else {
    return Err(format!("line {}: field '{}' is not a seg:off address", lineno, key));
  };
  let seg = parse_hex_u16(seg).map_err(|e| format!("line {}: bad field '{}': {}", lineno, key, e))?;
  let off = parse_hex_u16(off).map_err(|e| format!("line {}: bad field '{}': {}", lineno, key, e))?;
  Ok((seg, off))
}

fn parse_hex_u16(s: &str) -> Result<u16, String> {
  u16::from_str_radix(s, 16).map_err(|_| format!("'{}' is not a 16-bit hex number", s))
}

/// Turn an absolute guest address into the image-relative address a
/// configuration table uses.
///
/// Three addresses are not addresses of code: an address below the code
/// load (DOS or ROM space), the far magic return address (`0xffff:exec_id`)
/// and the near magic return window (`cs:0xff00+exec_id`). The core uses
/// those two windows to resume a suspended hook, and the trace records them
/// because they are where the guest was - not because a function lives
/// there.
fn resolve(seg: u16, off: u16, code_load: Option<u16>) -> Option<SegOff> {
  let load = code_load?;
  if seg < load { return None; }
  if seg == 0xffff { return None; }
  if off >= 0xff00 { return None; }
  Some(SegOff::new(seg - load, off))
}
