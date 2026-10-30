use pico_args;
use crate::binary::{self, Binary};
use crate::segoff::SegOff;
use crate::asm::intel_syntax;
use crate::config::Config;
use crate::decompile::ir;
use crate::decompile::gen;
use crate::decompile::ast;
use crate::decompile::ir_build;
use crate::decompile::sym;
use crate::decompile::opt;
use crate::decompile::control_flow;
use crate::spec::{self, Spec};
use crate::app_infer::FailedFunc;
use crate::diagnostics::{Cause, Diagnostic};
use crate::report::TranslationLedger;
use crate::validate::{self, ValidationScope};
use std::fs::File;
use std::io::Write;

fn print_help() {
  let appname = std::env::args().next().unwrap();
  println!("usage: {} OPTIONS", appname);
  println!("");
  println!("REQUIRED OPTIONS:");
  println!("  --config          path to binary configuration file (required)");
  println!("  --binary-exe      path to MZ format exe on the filesystem (exactly 1 --binary-* flag required)");
  println!("  --binary-raw      path to raw binary on the filesystem (exactly 1 --binary-* flag required)");
  println!("");
  println!("MODE: ANALYZE");
  println!("  --analyze         analyze the binary using the configuration annotations");
  println!("");
  println!("MODE: INFER ABI");
  println!("  --infer-abi       report inferred args, register inputs, and return width for the");
  println!("                    selected functions (uses --name, --codeseg-name, or a range)");
  println!("                    register inputs are iterated to a fixpoint (at most 8 passes)");
  println!("                    call-site columns need callers in the selection; --name builds");
  println!("                    only the named function");
  println!("");
  println!("MODE: VALIDATE CONFIG");
  println!("  --validate-config validate config ranges, decode, and call modes; then exit");
  println!("                    (also runs automatically before --name and --codeseg-name)");
  println!("  --strict          make validation warnings fatal (with --validate-config or auto preflight)");
  println!("");
  println!("MODE: TRACE");
  println!("  --trace-analyze   analyze a recorded Hydra trace (issue #97)");
  println!("  --trace PATH      the trace file to analyze (required with --trace-analyze)");
  println!("");
  println!("MODE: ADDRESS RANGE");
  println!("  --start-addr      start seg:off address (maybe required)");
  println!("  --end-addr        end seg:off address (maybe required)");
  println!("");
  println!("MODE: FUNCTION NAME");
  println!("  --name            lookup address range by name in config (maybe required)");
  println!("");
  println!("MODE: CODE SEGMENT NAME");
  println!("  --codeseg-name    decompile all config defined functions in the segment (maybe required)");
  println!("");
  println!("EMIT MODES:");
  println!("  --emit-dis        path to emit disassembly (optional)");
  println!("  --emit-ir-initial path to emit initial unoptimized SSA IR (optional)");
  println!("  --emit-ir-presym  path to emit pre-symbolized SSA IR (optional)");
  println!("  --emit-ir-sym     path to emit symbolized SSA IR (optional)");
  println!("  --emit-ir-fwd     path to emit memory forwarding SSA IR (optional)");
  println!("  --emit-ir-opt     path to emit optimized SSA IR (optional)");
  println!("  --emit-ir-final   path to emit final SSA IR before control-flow analysis (optional)");
  println!("  --emit-graph      path to emit a control-flow-graph dot file (optional)");
  println!("  --emit-ctrlflow   path to emit the inferred control-flow structure (optional)");
  println!("  --emit-ast        path to emit the constructed AST (optional)");
  println!("  --emit-code       path to emit c code (optional)");
  println!("");
  println!("REPORT:");
  println!("  --report          path to emit a deterministic translation blocker ledger (optional)");
  println!("");
  println!("IR BUILD FLAGS:");
  println!("  --build-pin-all   keep every value alive as a versioned local (a DCE");
  println!("                    diagnostic; NOT a correctness knob: pinning a value does");
  println!("                    not write it to the guest register, so the emulator can");
  println!("                    still observe stale register state)");
  println!("");
  println!("CODEGEN FLAGS:");
  println!("  --codegen-hydra   emit code that integrates well with the hydra runtime (optional)");
  println!("  --relocatable-seg HEX[,HEX...]");
  println!("                    absolute link-time segment values to emit relocated as");
  println!("                    (CODE_START_SEG + seg) instead of baked (optional, only");
  println!("                    with --codegen-hydra; comma- or whitespace-separated)");
}

fn validation_scope(args: &Args) -> Result<ValidationScope, String> {
  match (&args.name, &args.codeseg_name) {
    (Some(_), Some(_)) => Err("choose only one of --name or --codeseg-name".to_string()),
    (Some(name), None) => Ok(ValidationScope::Function(name.clone())),
    (None, Some(name)) => Ok(ValidationScope::CodeSegment(name.clone())),
    (None, None) => Ok(ValidationScope::All),
  }
}

fn has_emit_flags(args: &Args) -> bool {
  args.emit_dis.is_some() ||
    args.emit_ir_initial.is_some() ||
    args.emit_ir_presym.is_some() ||
    args.emit_ir_sym.is_some() ||
    args.emit_ir_fwd.is_some() ||
    args.emit_ir_opt.is_some() ||
    args.emit_ir_final.is_some() ||
    args.emit_graph.is_some() ||
    args.emit_ctrlflow.is_some() ||
    args.emit_ast.is_some() ||
    args.emit_code.is_some()
}

fn write_to_path(path: &str, data: &str) {
  if path.starts_with('-') {
    println!("{}", data);
  } else {
    let mut w = File::create(&path).unwrap();
    writeln!(&mut w, "{}", data).unwrap();
  }
}

#[derive(Debug)]
struct Args {
  binary: Option<binary::Fmt>,
  config: Option<String>,

  start_addr: Option<SegOff>,
  end_addr: Option<SegOff>,
  name: Option<String>,
  codeseg_name: Option<String>,

  analyze: bool,
  infer_abi: bool,
  validate_config: bool,
  strict: bool,
  trace_analyze: bool,
  trace: Option<String>,

  emit_dis: Option<String>,
  emit_ir_initial: Option<String>,
  emit_ir_presym: Option<String>,
  emit_ir_sym: Option<String>,
  emit_ir_fwd: Option<String>,
  emit_ir_opt: Option<String>,
  emit_ir_final: Option<String>,
  emit_graph: Option<String>,
  emit_ctrlflow: Option<String>,
  emit_ast: Option<String>,
  emit_code: Option<String>,
  report: Option<String>,

  build_pin_all: bool,
  codegen_hydra: bool,
  // Explicit relocation table: comma-separated absolute link-time
  // segment values, hex with optional 0x prefix. Only meaningful with
  // --codegen-hydra.
  relocatable_seg: Option<String>,
}

fn match_flag(args: &mut Vec<std::ffi::OsString>, flag: &str) -> bool {
  for i in 0..args.len() {
    if args[i] == flag {
      args.remove(i);
      return true;
    }
  }
  false
}

// Parse an explicit relocation table: comma- or whitespace-separated
// absolute link-time segment values, hex with an optional 0x prefix.
// Sorted, deduped; error text (not panic) on bad input, matching the
// --analyze error convention.
fn parse_relocatable_segs(list: &str) -> Result<Vec<u16>, String> {
  use std::collections::BTreeSet;
  let mut set = BTreeSet::new();
  for tok in list.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
    let hex = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")).unwrap_or(tok);
    match u16::from_str_radix(hex, 16) {
      Ok(v) => { set.insert(v); }
      Err(_) => return Err(format!("Expected hex u16 list for --relocatable-seg, got '{}'", tok)),
    }
  }
  if set.is_empty() {
    return Err("Expected at least one segment value for --relocatable-seg".to_string());
  }
  Ok(set.into_iter().collect())
}

// The binary is required by every mode but --trace-analyze: a trace
// carries the addresses it saw, so there is nothing to load.
fn parse_binary_fmt(pargs: &mut pico_args::Arguments, required: bool)
  -> Result<Option<binary::Fmt>, pico_args::Error>
{
  let binary_exe = pargs.opt_value_from_str("--binary-exe")?;
  let binary_raw = pargs.opt_value_from_str("--binary-raw")?;
  if 1 != binary_exe.is_some() as i32 + binary_raw.is_some() as i32 {
    if required {
      panic!("Exactly one of --binary-exe or --binary-raw must be set");
    }
    return Ok(None);
  }
  if let Some(path) = binary_exe {
    return Ok(Some(binary::Fmt::Exe(path)));
  }
  if let Some(path) = binary_raw {
    return Ok(Some(binary::Fmt::Raw(path)));
  }
  unreachable!();
}

fn parse_args() -> Result<Args, pico_args::Error> {
  let mut pargs = pico_args::Arguments::from_env();

  // Help has a higher priority and should be handled separately.
  if pargs.contains(["-h", "--help"]) {
    print_help();
    std::process::exit(0);
  }

  // The trace mode is known before the rest is parsed: it is the one mode
  // that needs neither a configuration nor a binary.
  let trace_analyze = pargs.contains("--trace-analyze");

  // Parse out all args
  let mut args = Args {
    config:          pargs.opt_value_from_str("--config")?,
    binary:          parse_binary_fmt(&mut pargs, !trace_analyze)?,
    analyze:         false,
    infer_abi:       false,
    validate_config: false,
    strict:          false,
    trace_analyze:   trace_analyze,
    trace:           pargs.opt_value_from_str("--trace")?,
    start_addr:      pargs.opt_value_from_str("--start-addr")?,
    end_addr:        pargs.opt_value_from_str("--end-addr")?,
    name:            pargs.opt_value_from_str("--name")?,
    codeseg_name:    pargs.opt_value_from_str("--codeseg-name")?,
    emit_dis:        pargs.opt_value_from_str("--emit-dis")?,
    emit_ir_initial: pargs.opt_value_from_str("--emit-ir-initial")?,
    emit_ir_opt:     pargs.opt_value_from_str("--emit-ir-opt")?,
    emit_ir_presym:  pargs.opt_value_from_str("--emit-ir-presym")?,
    emit_ir_sym:     pargs.opt_value_from_str("--emit-ir-sym")?,
    emit_ir_fwd:     pargs.opt_value_from_str("--emit-ir-fwd")?,
    emit_ir_final:   pargs.opt_value_from_str("--emit-ir-final")?,
    emit_graph:      pargs.opt_value_from_str("--emit-graph")?,
    emit_ctrlflow:   pargs.opt_value_from_str("--emit-ctrlflow")?,
    emit_ast:        pargs.opt_value_from_str("--emit-ast")?,
    emit_code:       pargs.opt_value_from_str("--emit-code")?,
    report:          pargs.opt_value_from_str("--report")?,
    build_pin_all:   false,
    codegen_hydra:   false,
    relocatable_seg: pargs.opt_value_from_str("--relocatable-seg")?,
  };

  let mut remaining = pargs.finish();
  args.analyze         = match_flag(&mut remaining, "--analyze");
  args.infer_abi       = match_flag(&mut remaining, "--infer-abi");
  args.validate_config = match_flag(&mut remaining, "--validate-config");
  args.strict          = match_flag(&mut remaining, "--strict");
  args.build_pin_all   = match_flag(&mut remaining, "--build-pin-all");
  args.codegen_hydra   = match_flag(&mut remaining, "--codegen-hydra");

  // It's up to the caller what to do with the remaining arguments.
  if remaining.len() != 0 {
    eprintln!("Error: unused arguments left: {:?}.", remaining);
    std::process::exit(1);
  }

  Ok(args)
}

pub fn run() -> i32 {
  let args = match parse_args() {
    Ok(v) => v,
    Err(e) => {
      eprintln!("Error: {}.", e);
      return 1;
    }
  };

  if args.validate_config {
    if args.analyze {
      eprintln!("Error: --validate-config cannot be combined with --analyze.");
      return 1;
    }
    if args.trace_analyze {
      eprintln!("Error: --validate-config cannot be combined with --trace-analyze.");
      return 1;
    }
    if has_emit_flags(&args) {
      eprintln!("Error: --validate-config cannot be combined with an --emit-* mode.");
      return 1;
    }
    if args.start_addr.is_some() || args.end_addr.is_some() {
      eprintln!("Error: --validate-config takes --name or --codeseg-name, not --start-addr/--end-addr.");
      return 1;
    }
  }

  if args.infer_abi {
    if args.analyze {
      eprintln!("Error: --infer-abi cannot be combined with --analyze.");
      return 1;
    }
    if args.validate_config {
      eprintln!("Error: --infer-abi cannot be combined with --validate-config.");
      return 1;
    }
    if has_emit_flags(&args) {
      eprintln!("Error: --infer-abi cannot be combined with an --emit-* mode.");
      return 1;
    }
    if args.build_pin_all {
      eprintln!("Error: --infer-abi cannot be combined with --build-pin-all.");
      return 1;
    }
  }

  if args.report.is_some() {
    if args.analyze {
      eprintln!("Error: --report cannot be combined with --analyze.");
      return 1;
    }
    if args.validate_config {
      eprintln!("Error: --report cannot be combined with --validate-config.");
      return 1;
    }
    if args.infer_abi {
      eprintln!("Error: --report cannot be combined with --infer-abi.");
      return 1;
    }
    if args.trace_analyze {
      eprintln!("Error: --report cannot be combined with --trace-analyze.");
      return 1;
    }
  }

  // The trace mode needs neither a configuration nor a binary: a trace
  // carries the addresses it saw. The configuration is still useful (it
  // names the functions the run did not discover), so it is read when it
  // is there.
  if args.trace_analyze {
    let Some(trace_path) = &args.trace else {
      eprintln!("Error: --trace-analyze needs --trace <file>.");
      return 1;
    };
    let cfg = match &args.config {
      Some(path) => match Config::from_path(path) {
        Ok(cfg) => Some(cfg),
        Err(err) => {
          eprintln!("Error: {}", err);
          return 1;
        }
      },
      None => None,
    };
    return crate::app_trace::run(cfg.as_ref(), trace_path);
  }

  let Some(config_path) = &args.config else {
    eprintln!("Error: --config is required.");
    return 1;
  };
  let cfg = match Config::from_path(config_path) {
    Ok(cfg) => cfg,
    Err(err) => {
      eprintln!("Error: {}", err);
      return 1;
    }
  };

  // Parse the relocation table once per run (not per function): a bad
  // value is a single upfront error, even without --codegen-hydra, and
  // the audit line below prints once.
  let relocatable_segs: Vec<u16> = match args.relocatable_seg.as_deref() {
    Some(list) => {
      let segs = match parse_relocatable_segs(list) {
        Ok(segs) => segs,
        Err(err) => {
          eprintln!("Error: {}", err);
          return 1;
        }
      };
      if args.codegen_hydra {
        segs
      } else {
        eprintln!("Warning: --relocatable-seg has no effect without --codegen-hydra; ignoring.");
        vec![]
      }
    }
    None => vec![],
  };
  if args.emit_code.is_some() && args.codegen_hydra {
    let table: Vec<String> = relocatable_segs.iter().map(|s| format!("0x{:x}", s)).collect();
    eprintln!("relocatable segments: [{}]", table.join(", "));
  }

  if args.analyze {
    let Some(binary::Fmt::Exe(path)) = &args.binary else {
      eprintln!("Error: --analyze requires --binary-exe.");
      return 1;
    };
    return crate::app_analyze::run(&cfg, path);
  }

  let Some(binary_fmt) = &args.binary else {
    eprintln!("Error: a binary is required.");
    return 1;
  };
  let binary = match Binary::from_fmt(binary_fmt, Some(&cfg)) {
    Ok(binary) => binary,
    Err(err) => {
      eprintln!("Error: {}", err);
      return 1;
    }
  };

  let scope = match validation_scope(&args) {
    Ok(scope) => scope,
    Err(err) => {
      eprintln!("Error: {}", err);
      return 1;
    }
  };
  if args.validate_config {
    let report = validate::validate_config(&cfg, &binary, &scope);
    report.print();
    return if report.should_fail(args.strict) { 1 } else { 0 };
  }

  // Batch modes get an automatic preflight: errors abort, warnings are
  // reported but do not block unless --strict is set. Inference and --report
  // are read-only/best-effort reports: a broken annotation must not suppress
  // findings for the functions that do decode, so they warn and continue. The
  // validation verdict still decides the exit code below.
  let report_mode = args.report.is_some();
  let mut ledger = TranslationLedger::default();
  let mut preflight_failed = false;
  if args.name.is_some() || args.codeseg_name.is_some() {
    let validation = validate::validate_config(&cfg, &binary, &scope);
    if validation.should_fail(args.strict) {
      validation.print();
      preflight_failed = true;
      if report_mode {
        for diagnostic in &validation.diagnostics {
          if diagnostic.severity == validate::Severity::Error || args.strict {
            ledger.global_failure(Diagnostic::new(
              Cause::ConfigError, diagnostic.addr, None, diagnostic.message.clone()));
          }
        }
      }
      if !args.infer_abi && !report_mode {
        return 1;
      }
    } else if validation.warnings() != 0 {
      validation.print();
    }
  }

  if report_mode {
    configure_report_segments(&cfg, &args, &mut ledger);
  }

  let (specs, infer_failures) = if args.infer_abi || report_mode {
    match infer_specs(&cfg, &binary, &args) {
      Ok(selections) => selections,
      Err(()) => {
        if report_mode {
          ledger.global_failure(Diagnostic::new(
            Cause::ConfigError, None, None,
            "function selection failed; see stderr for details"));
          if let Err(err) = write_report(&args, &cfg, &ledger) {
            eprintln!("Error: {}", err);
          }
        }
        return 1;
      }
    }
  } else {
    let specs =
      if let Some(name) = &args.name {
        match spec::Spec::from_config_name(&cfg, name) {
          Some(spec) => vec![spec],
          None => {
            eprintln!("Error: Failed to lookup function named: {}", name);
            return 1;
          }
        }
      } else if let Some(name) = &args.codeseg_name {
        spec::specs_from_codeseg_name(&cfg, name)
      } else {
        match (args.start_addr, args.end_addr) {
          (Some(start), Some(end)) => {
            if !binary.contains_region(start, end) {
              eprintln!("Error: address range {}..{} is invalid or outside the loaded image", start, end);
              return 1;
            }
            vec![spec::Spec::from_start_and_end(Some(start), Some(end))]
          }
          (None, None) => {
            eprintln!("Error: one of --name, --codeseg-name, or --start-addr/--end-addr is required.");
            return 1;
          }
          _ => {
            eprintln!("Error: --start-addr and --end-addr must be provided together.");
            return 1;
          }
        }
      };
    (specs, Vec::new())
  };

  if args.infer_abi {
    let rc = crate::app_infer::run(&cfg, &binary, specs, infer_failures);
    return if preflight_failed { 1 } else { rc };
  }

  if report_mode {
    for failed in &infer_failures {
      ledger.attempt(failed.start.seg);
      ledger.failure(failed.start.seg, Diagnostic::new(
        Cause::ConfigError, Some(failed.start), None, failed.note.clone()));
    }
  }

  let mut all_code = String::new();
  let mut failures = 0;
  for spec in specs {
    let name = spec.name.clone();
    let seg = spec.start.seg;
    let start = spec.start;
    // Contain panics per function: ControlFlow inference and AST building
    // still panic on degenerate input (goto cycles, unimplemented opcodes,
    // unsupported types). Without this, one such function unwinds past the
    // loop and aborts the whole batch. Truncate any partial output the
    // panicking spec appended before counting it as failed.
    let code_len = all_code.len();
    if report_mode {
      ledger.attempt(seg);
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      decompile_spec(&args, &cfg, &binary, spec, &mut all_code, &relocatable_segs, &mut ledger)
    }));
    match outcome {
      Ok(Ok(())) => {
        if report_mode {
          ledger.translated(seg);
        }
      }
      Ok(Err(diagnostic)) => {
        // Best-effort batch decompile: one malformed function must not abort
        // the rest of the codeseg (the specific error was already reported by
        // decompile_spec above). Count it and continue.
        eprintln!("Error: Failed to decompile {}; continuing with the rest.", name);
        failures += 1;
        if report_mode {
          ledger.failure(seg, diagnostic);
        }
      }
      Err(_) => {
        all_code.truncate(code_len);
        eprintln!("Error: Panicked while decompiling {}; continuing with the rest.", name);
        failures += 1;
        if report_mode {
          ledger.failure(seg, Diagnostic::new(
            Cause::InternalPanic, Some(start), None,
            format!("panicked while decompiling {}", name)));
        }
      }
    }
  }

  let mut report_failed = false;
  if report_mode {
    if let Err(err) = write_report(&args, &cfg, &ledger) {
      eprintln!("Error: {}", err);
      report_failed = true;
    }
  }
  if failures != 0 {
    eprintln!("Error: Failed to decompile {} function(s).", failures);
    return 1;
  }
  if report_failed || preflight_failed {
    return 1;
  }

  if let Some(path) = args.emit_code.as_ref() {
    write_to_path(path, &all_code);
  }

  0
}

fn configure_report_segments(cfg: &Config, args: &Args, ledger: &mut TranslationLedger) {
  if let Some(name) = &args.name {
    if let Some(func) = cfg.func_lookup_by_name(name) {
      ledger.set_configured(func.start.seg, 1);
    }
  } else if let Some(name) = &args.codeseg_name {
    if let Some(code_seg) = cfg.code_seg_lookup_by_name(name) {
      ledger.set_configured(code_seg.seg, cfg.func_lookup_by_seg(code_seg.seg).len());
    }
  } else if let Some(start) = args.start_addr {
    ledger.set_configured(start.seg, 1);
  }
}

fn write_report(args: &Args, cfg: &Config, ledger: &TranslationLedger) -> Result<(), String> {
  let Some(path) = args.report.as_ref() else { return Ok(()); };
  let text = ledger.render(cfg);
  let text = text.trim_end();
  if path.starts_with('-') {
    println!("{}", text);
    return Ok(());
  }
  std::fs::write(path, format!("{}\n", text))
    .map_err(|err| format!("failed to write report '{}': {}", path, err))
}

/// Build a selection without the panicking `Spec::from_func` path, so a
/// missing end or an out-of-image range becomes a report row instead of
/// aborting the whole run. Used by inference and `--report`.
fn infer_specs<'a>(cfg: &'a Config, binary: &Binary, args: &Args)
  -> Result<(Vec<Spec<'a>>, Vec<FailedFunc>), ()> {
  if let Some(name) = &args.name {
    let Some(func) = cfg.func_lookup_by_name(name) else {
      eprintln!("Error: Failed to lookup function named: {}", name);
      return Err(());
    };
    let (specs, failed) = match infer_spec(binary, func) {
      Ok(spec) => (vec![spec], vec![]),
      Err(f) => (vec![], vec![f]),
    };
    return Ok((specs, failed));
  }
  if let Some(name) = &args.codeseg_name {
    let Some(codeseg) = cfg.code_seg_lookup_by_name(name) else {
      eprintln!("Error: Failed to find code segment with name '{}'", name);
      return Err(());
    };
    let mut specs = vec![];
    let mut failed = vec![];
    for func in cfg.func_lookup_by_seg(codeseg.seg) {
      match infer_spec(binary, func) {
        Ok(spec) => specs.push(spec),
        Err(f) => failed.push(f),
      }
    }
    return Ok((specs, failed));
  }
  match (args.start_addr, args.end_addr) {
    (Some(start), Some(end)) => {
      if !binary.contains_region(start, end) {
        eprintln!("Error: address range {}..{} is invalid or outside the loaded image", start, end);
        return Err(());
      }
      Ok((vec![Spec::from_start_and_end(Some(start), Some(end))], vec![]))
    }
    (None, None) => {
      eprintln!("Error: one of --name, --codeseg-name, or --start-addr/--end-addr is required.");
      Err(())
    }
    _ => {
      eprintln!("Error: --start-addr and --end-addr must be provided together.");
      Err(())
    }
  }
}

fn infer_spec<'a>(binary: &Binary, func: &'a crate::config::Func)
  -> Result<Spec<'a>, FailedFunc> {
  if func.outer_end().is_none() {
    return Err(FailedFunc::new(func.name.clone(), func.start, func.start,
                               "missing end address".to_string()));
  }
  for region in func.effective_regions() {
    if !binary.contains_region(region.start, region.end) {
      return Err(FailedFunc::new(func.name.clone(), region.start, region.end,
                                 "region outside the loaded image".to_string()));
    }
  }
  Ok(Spec::from_func(func))
}

fn decompile_spec(args: &Args, cfg: &Config, binary: &Binary, spec: Spec<'_>,
                  all_code: &mut String, relocatable_segs: &[u16],
                  ledger: &mut TranslationLedger) -> Result<(), Diagnostic> {
  let (instr_list, raw_list) = match spec::decode_regions(binary, &spec) {
    Ok(decoded) => decoded,
    Err(e) => {
      eprintln!("Error: Failed to decode instruction: {}", e);
      return Err(Diagnostic::new(Cause::DecodeError, Some(e.addr), None, e.message));
    }
  };
  // FIXME: SIMPLIFY!!
  if let Some(path) = args.emit_dis.as_ref() {
    let mut buf = String::new();
    for i in 0..instr_list.len() {
      let instr = &instr_list[i];
      let raw = &raw_list[i];
      buf += &intel_syntax::format(instr.addr, Some(&instr), raw, true).unwrap();
      buf += "\n";
    }
    write_to_path(path, &buf);
    return Ok(());
  }

  let overlay = spec.start.is_overlay_addr();

  let (mut ir, diagnostics) = ir_build::build_from_instrs_with_diagnostics(
    &instr_list, &cfg, &spec, &binary, overlay, args.build_pin_all);
  for diagnostic in diagnostics {
    eprintln!("{}", diagnostic.message);
    ledger.blocker(spec.start.seg, &diagnostic);
  }
  if let Some(path) = args.emit_ir_initial.as_ref() {
    write_to_path(path, &format!("{}", ir));
    return Ok(());
  }

  opt::optimize(&mut ir);
  if let Some(path) = args.emit_ir_presym.as_ref() {
    write_to_path(path, &format!("{}", ir));
    return Ok(());
  }

  sym::symbolize(&mut ir, &cfg);
  if let Some(path) = args.emit_ir_sym.as_ref() {
    write_to_path(path, &format!("{}", ir));
    return Ok(());
  }

  opt::forward_store_to_load(&mut ir);
  opt::optimize(&mut ir);
  if let Some(path) = args.emit_ir_fwd.as_ref() {
    write_to_path(path, &format!("{}", ir));
    return Ok(());
  }

  opt::mem_symbol_to_ref(&mut ir);
  opt::optimize(&mut ir);
  if let Some(path) = args.emit_ir_opt.as_ref() {
    let text = ir::display::display_ir_with_uses(&ir).unwrap();
    write_to_path(path, &format!("{}", &text));
    return Ok(());
  }

  crate::decompile::finalize_for_emission(&mut ir);
  if let Some(path) = args.emit_ir_final.as_ref() {
    let text = ir::display::display_ir_with_uses(&ir).unwrap();
    write_to_path(path, &format!("{}", &text));
    return Ok(());
  }

  if let Some(path) = args.emit_graph.as_ref() {
    let dot = ir::util::gen_graphviz_dotfile(&ir).unwrap();
    write_to_path(path, &dot);
    return Ok(());
  }

  let ctrlflow = control_flow::ControlFlow::from_ir(&ir);
  if let Some(path) = args.emit_ctrlflow.as_ref() {
    let text = control_flow::format(&ctrlflow).unwrap();
    write_to_path(path, &text);
    return Ok(());
  }

  let ret = spec.func.map(|f| f.return_type_defaulted());
  let ast = match ast::Function::from_ir(&cfg, &spec.name, ret, &ir, &ctrlflow) {
    Ok(ast) => ast,
    Err(err) => {
      eprintln!("Error while building annotated accesses for {}: {}", spec.name, err);
      return Err(Diagnostic::new(
        Cause::CodegenFailure, Some(spec.start), None,
        format!("failed to build AST: {}", err)));
    }
  };
  if let Some(path) = args.emit_ast.as_ref() {
    let text = format!("{:#?}", ast);
    write_to_path(path, &text);
    return Ok(());
  }

  if args.emit_code.is_some() {
    // Segment relocation: Hydra runs the recompiled code relocated, so
    // baked absolute link-time segment constants listed in the explicit
    // --relocatable-seg table (parsed once in run()) emit as
    // (CODE_START_SEG + seg) instead of baked. The table is
    // operator-audited by construction (the operator typed it); there
    // is no derived default. Standard keeps raw values.
    let flavor = if args.codegen_hydra {
      gen::Flavor::Hydra { relocatable_segs: relocatable_segs.to_vec() }
    } else {
      gen::Flavor::Standard
    };
    let code = match gen::generate(&ast, flavor) {
      Ok(code) => code,
      Err(err) => {
        eprintln!("Error while generating code for {}: {}", spec.name, err);
        return Err(Diagnostic::new(
          Cause::CodegenFailure, Some(spec.start), None,
          format!("code generation failed: {}", err)));
      }
    };
    all_code.push_str(&code);
    all_code.push('\n');
    return Ok(());
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::parse_relocatable_segs;
  use crate::spec::decode_regions;
  use crate::binary::Binary;
  use crate::config::FuncRegion;
  use crate::segoff::{Off, Seg, SegOff};
  use crate::spec::Spec;

  fn so(off: u16) -> SegOff {
    SegOff { seg: Seg::Normal(0), off: Off(off) }
  }

  #[test]
  fn region_decode_skips_gap_bytes() {
    let binary = Binary::from_raw(&[0x90, 0x90, 0x63, 0x63, 0x90], None);
    let spec = Spec {
      func: None,
      name: "split".to_string(),
      start: so(0),
      end: so(5),
      regions: vec![
        FuncRegion { start: so(0), end: so(2) },
        FuncRegion { start: so(4), end: so(5) },
      ],
    };
    let (instrs, raws) = decode_regions(&binary, &spec).unwrap();
    let addrs: Vec<u16> = instrs.iter().map(|instr| instr.addr.off.0).collect();
    assert_eq!(addrs, vec![0, 1, 4]);
    assert_eq!(raws.len(), 3);
  }

  #[test]
  fn entry_region_decodes_before_shared_epilogue() {
    let binary = Binary::from_raw(&[0x90, 0x90, 0x90], None);
    let spec = Spec {
      func: None,
      name: "shared_tail".to_string(),
      start: so(2),
      end: so(3),
      regions: vec![
        FuncRegion { start: so(0), end: so(1) },
        FuncRegion { start: so(2), end: so(3) },
      ],
    };
    let (instrs, _) = decode_regions(&binary, &spec).unwrap();
    let addrs: Vec<u16> = instrs.iter().map(|instr| instr.addr.off.0).collect();
    assert_eq!(addrs, vec![2, 0]);
  }

  #[test]
  fn relocatable_seg_list_parses_hex_with_optional_prefix() {
    // Comma list, optional 0x/0X prefixes, sorted and deduped.
    assert_eq!(parse_relocatable_segs("0x456,1789,0x456").unwrap(), vec![0x456, 0x1789]);
    assert_eq!(parse_relocatable_segs("0X1 2").unwrap(), vec![0x1, 0x2]);
    assert_eq!(parse_relocatable_segs("ffff").unwrap(), vec![0xffff]);
  }

  #[test]
  fn relocatable_seg_list_rejects_empty_and_bad_tokens() {
    // Error text (not panic): empty list, non-hex token, out-of-range.
    assert!(parse_relocatable_segs("").is_err());
    assert!(parse_relocatable_segs("  , ").is_err());
    assert!(parse_relocatable_segs("0x456,banana").is_err());
    assert!(parse_relocatable_segs("0x10000").is_err());
  }
}
