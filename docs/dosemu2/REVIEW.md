# dosemu2 validator design review

> **Note (2026-08-17):** SST (SingleStepTests hardware captures) is the validation authority for emu86; the dosemu2 differential validator is no longer the validation method. This design review covers the dosemu2 validator transport and is kept as reference for the transport design that Track 4 Option D (Hydra hosting via in-process plugin) research builds on.

This document records the non-obvious design conclusions. Current status and test coverage live in [`README.md`](README.md) and [`TESTING.md`](TESTING.md); the normative requirements live in [`PHASE1_SPEC.md`](PHASE1_SPEC.md).

## Core instrumentation, not a pure plugin

Arbitrary imported `CS:IP` and validator-bounded execution require a small `simx86` core hook around `FindExecCode()`/`DoExec(G)`. Patch 0001 applies state before node lookup, recomputes the authoritative PC, reasserts `MSSTP`, and publishes the returned node boundary. A plugin alone does not provide this control point.

## Low memory follows the live backing

Patch 0002 exports the existing `MAPPING_LOWMEM` `mapshm` allocation as `/dosemu_mem`; it does not allocate a second DOS-memory buffer. The backing can be larger than the visible `LOWMEM_SIZE + HMASIZE` window, so provenance, containment, address-zero resolution, and bidirectional aliasing are the invariants—not exact-size equality.

## ABI-v1 is deliberately real mode

Imported state is a 16-bit real-mode contract. General registers and FLAGS preserve their high halves, segment changes use the normal real-mode cache path, and protected-mode import fails before mutation. The 88-byte ABI remains append-only after the legacy 64-byte Hydra prefix.

## Node boundaries require metadata

`TNode.seqlen` is byte length, not instruction count. The carrier publishes `TNode.seqnum` and raw flags so the Rust adapter does not infer consumption from opcode length. This plumbing is implemented, but REP, interrupt-shadow behavior, and their composition remain not integration-tested.

## Host services and application handlers are different contracts

Patch 0010 normalizes a standalone software interrupt only when its eligible vector still matches the activation-time host vector. It keeps the request outstanding across the host service and publishes at the exact saved return `CS:IP`. The unprefixed `INT 21h/AH=30h` pinned-runtime proof validates this path and its DOS-returned state.

If the application changes the vector, emu86 exposes handler entry as a controlled boundary. dosemu2 therefore keeps that application-installed handler in controller lockstep, including code outside the target MCB, rather than normalizing it away. Prefixed calls, application-installed handlers, broader BIOS services, and shadow-composed interrupts still require runtime fixtures.

## Ownership and lifecycle fail closed

Activation combines executable path, MZ entry, PSP/MCB/environment identity, and dosemu2’s current-PSP accessor. Descendant PSPs bypass request consumption; leaving the captured ancestry latches target exit and prevents stale-PSP reactivation. The launcher accepts its own PID or a verified descendant PID. Representative helper execution and helper/lifecycle transitions remain not integration-tested even though target-exit and fault publication have focused runtime proofs.

## Approval boundary

The focused carrier build/runtime job supports the implemented claims listed in [`TESTING.md`](TESTING.md). It does not approve prefixed/application-handler cases, broader BIOS coverage, REP, interrupt shadow, helper exclusion, lifecycle transitions, broad mutation, or full differential state/memory comparison.

Strict lockstep correctness also makes no performance claim. Hydra native-function interception, overlays, and hybrid-performance benchmarking remain separate work.
