pub mod ir;
pub mod ir_build;
pub mod abi;
pub mod sym;
pub mod opt;
pub mod fuse;
pub mod ast;
pub mod control_flow;
pub mod gen;

use crate::asm::instr;
use crate::binary::Binary;
use crate::config::Config;
use crate::decompile::control_flow::ControlFlow;
use crate::decompile::ir::IR;
use crate::decompile::ir_build::FlagMarkers;
use crate::diagnostics::Diagnostic;
use crate::spec::Spec;

/// Build the analysis IR for decoded instructions. ABI inference and the
/// non-emitting `--analyze` summary both use this so their pass sequence
/// cannot diverge from each other. Build diagnostics are appended before the
/// passes run so a caller that catches a panic can still report them.
pub fn build_ir_with_diagnostics(
  instrs: &[instr::Instr], cfg: &Config, spec: &Spec<'_>, binary: &Binary, pin_all: bool,
  diagnostics: &mut Vec<Diagnostic>,
) -> (IR, FlagMarkers) {
  let overlay = spec.start.is_overlay_addr();
  let (mut ir, build_diagnostics, markers) = ir_build::build_from_instrs_with_diagnostics(
    instrs, cfg, spec, binary, overlay, pin_all);
  diagnostics.extend(build_diagnostics);
  opt::optimize(&mut ir);
  sym::symbolize(&mut ir, cfg);
  opt::forward_store_to_load(&mut ir);
  opt::optimize(&mut ir);
  opt::mem_symbol_to_ref(&mut ir);
  opt::optimize(&mut ir);
  finalize_for_emission(&mut ir);
  (ir, markers)
}

/// Run the IR passes that turn the post-`mem_symbol_to_ref` form into the IR
/// the decompiler emits. Both code generation and ABI inference must use this
/// so the analysis sees the same operands the caller will.
pub fn finalize_for_emission(ir: &mut IR) {
  fuse::fuse_mem(ir);
  opt::optimize(ir);
  ir::fin::finalize(ir);
}

/// Build the annotated C AST for a translated function. Shared by code
/// generation and the non-emitting `--analyze` summary so both see the same
/// AST-level failures.
pub fn build_ast(cfg: &Config, spec: &Spec<'_>, ir: &IR, ctrlflow: &ControlFlow)
  -> Result<ast::Function, String>
{
  let ret = spec.func.map(|f| f.return_type_defaulted());
  ast::Function::from_ir(cfg, &spec.name, ret, ir, ctrlflow)
}
