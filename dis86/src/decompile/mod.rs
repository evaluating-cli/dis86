pub mod ir;
pub mod ir_build;
pub mod abi;
pub mod sym;
pub mod opt;
pub mod fuse;
pub mod ast;
pub mod control_flow;
pub mod gen;

use crate::decompile::ir::IR;

/// Run the IR passes that turn the post-`mem_symbol_to_ref` form into the IR
/// the decompiler emits. Both code generation and ABI inference must use this
/// so the analysis sees the same operands the caller will.
pub fn finalize_for_emission(ir: &mut IR) {
  fuse::fuse_mem(ir);
  opt::optimize(ir);
  ir::fin::finalize(ir);
}
