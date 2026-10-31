use crate::analyze::analyze::Analyze;
use crate::binary::Binary;
use crate::config::{Config, Func};
use crate::decompile::build_ir_with_diagnostics;
use crate::decompile::control_flow::ControlFlow;
use crate::diagnostics::{Cause, Diagnostic};
use crate::report::TranslationLedger;
use crate::spec;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub fn run(cfg: &Config, exe_path: &str, pin_all: bool) -> i32 {
  let a = match Analyze::new(cfg, exe_path) {
    Ok(a) => a,
    Err(e) => {
      eprintln!("Error: {}.", e);
      return 1;
    }
  };
  a.scan_for_all_functions(true);
  a.report_coverage();

  let ledger = translation_summary(a.cfg(), a.binary(), pin_all);
  eprint!("{}", ledger.render(a.cfg()));

  0
}

fn translation_summary(cfg: &Config, binary: &Binary, pin_all: bool) -> TranslationLedger {
  let mut ledger = TranslationLedger::default();
  let mut configured = BTreeMap::new();
  for func in &cfg.funcs {
    *configured.entry(func.start.seg).or_insert(0) += 1;
  }
  for (seg, count) in &configured {
    ledger.set_configured(*seg, *count);
  }

  for func in &cfg.funcs {
    let seg = func.start.seg;
    ledger.attempt(seg);
    let mut diagnostics = vec![];
    let outcome = catch_unwind(AssertUnwindSafe(|| {
      summarize_function(cfg, binary, func, pin_all, &mut diagnostics)
    }));
    for diagnostic in &diagnostics {
      ledger.blocker(seg, diagnostic);
    }
    match outcome {
      Ok(Ok(())) => ledger.translated(seg),
      Ok(Err(diagnostic)) => ledger.failure(seg, diagnostic),
      Err(_) => ledger.failure(seg, Diagnostic::new(
        Cause::InternalPanic, Some(func.start), None,
        format!("panicked while summarizing {}", func.name))),
    }
  }

  ledger
}

fn summarize_function(cfg: &Config, binary: &Binary, func: &Func, pin_all: bool,
                      diagnostics: &mut Vec<Diagnostic>) -> Result<(), Diagnostic> {
  let spec = crate::app::infer_spec(binary, func)
    .map_err(|failed| Diagnostic::new(
      Cause::ConfigError, Some(failed.start), None, failed.note))?;
  let (instrs, _raw) = spec::decode_regions(binary, cfg, &spec)
    .map_err(|failure| Diagnostic::new(
      failure.cause, Some(failure.addr), None, failure.message))?;

  let (ir, markers) = build_ir_with_diagnostics(
    &instrs, cfg, &spec, binary, pin_all, diagnostics);
  diagnostics.extend(markers.diagnostics(&ir));

  let ctrlflow = ControlFlow::from_ir(&ir);
  crate::decompile::build_ast(cfg, &spec, &ir, &ctrlflow)
    .map_err(|err| Diagnostic::new(
      Cause::CodegenFailure, Some(spec.start), None,
      format!("failed to build AST: {}", err)))?;

  Ok(())
}
