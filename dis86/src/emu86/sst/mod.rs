//! SST 80286 harness: execute hardware-captured SingleStepTests MOO test cases
//! against emu86 and bucket the results (see plan.md, phase P1).

pub mod runner;
pub mod policy;
#[cfg(test)]
pub mod micro;
