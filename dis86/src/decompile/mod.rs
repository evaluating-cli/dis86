pub mod ir;
pub mod ir_build;
pub mod abi;
pub mod sym;
pub mod opt;
pub mod fuse;
pub mod ast;
pub mod control_flow;
pub mod gen;

use crate::config::Config;
use crate::decompile::control_flow::ControlFlow;
use crate::decompile::ir::IR;
use crate::spec::Spec;

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
