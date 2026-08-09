mod value;

mod mem;
mod cpu;
#[allow(dead_code)]
mod cpu_flags;
mod cpu_scas;
mod cpu_cmps;
mod cpu_stos;
mod cpu_movs;

pub mod alu;

mod machine;
mod loader;
mod interrupt;
mod step;

mod dos;
#[allow(dead_code)]
mod dos_ivt;
mod dos_structs;
mod dos_filesystem;
mod dos_mouse;

mod video;
mod adlib;
mod io;

mod opl;

mod mzhdr;

#[cfg(feature = "sdl")]
mod sdl;
pub mod emu;

pub use emu::run;
pub use emu::{Emulator, LoadConfig};
pub use cpu::{InitialStatePolicy, normalize_initial_state};
pub mod validator;

// Tests
#[cfg(test)] mod alu_test;
