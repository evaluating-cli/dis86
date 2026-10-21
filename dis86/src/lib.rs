extern crate static_assertions as sa;

// Helper libraries
mod util;
mod bsl;

// Core support libraries
pub mod binfmt;
pub mod binary;
pub mod region;
pub mod segoff;
pub mod spec;
pub mod config;
pub mod types;
pub mod access;

// Subsystems
pub mod asm;
pub mod analyze;
pub mod decompile;

// Emulator: frozen offline tooling (no development; not part of product
// validation). See the development direction in README.md.
#[cfg(feature = "emu86")]
pub mod emu86;

// Main application glue
pub mod app;
pub mod app_analyze;
pub mod app_trace;

// The Hydra trace format (issue #97) and what can be learned from it.
pub mod trace;
