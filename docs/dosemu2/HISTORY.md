# HISTORY — the retired ABI-v1 differential validator

> One-page record of a retired mechanism, kept for provenance. It is **not** part
> of the current host or validation story; see [`README.md`](README.md) and the
> development direction in [`../../README.md`](../../README.md).

## What it was

An instruction-boundary differential validator: it ran the same MZ executable on
two backends simultaneously and compared register/memory state at each step.

- **Backend A:** `emu86`, the in-repo 8086/286 interpreter (upstream's
  `Emulator` implementing the `Emu` trait).
- **Backend B:** Hydra's hybrid execution driven through a real DOS emulator —
  originally **dosbox-x** (upstream `hydra_process.rs`, pinned as a
  `xorvoid/dosbox-x` submodule), later **stock dosemu2**
  (`dis86/src/emu86/validator/dosemu_process.rs`).
- **Transport (ABI v1):** an 88-byte shared-memory control block split across
  `/dev/shm/hydra_remote` (request/ack + registers) and `/dev/shm/dosemu_mem`
  (low memory), stepped through dosemu2's debugger. Dosemu2-side carrier patches
  were pinned to commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`.
- The comparison also had to reconcile boundary-shape differences, e.g. emu86
  executes a whole `REP` as one step while dosemu2 reports per-iteration
  `SAME_PC` boundaries.

## Why it was retired

1. **The host changed substrate.** Hydra moved from dosbox-x to the external
   stock-dosemu2 host (dosdebug FIFOs + verified `/proc/pid/fd` lowmem mapping,
   merged as PR [#39](https://github.com/evaluating-cli/dis86/pull/39),
   `5eb5567`). The pinned patch carrier that provided the ABI-v1 hooks was
   removed.
2. **Validation narrowed to what the product uses.** Per the 2026-10 development
   direction, dosemu2 (+simx86, FDPP) is a trusted third-party component — we do
   not validate, patch, or fork it — so emu86↔dosemu2 twin equivalence is a
   non-goal. emu86 and the SingleStepTests harness were frozen as offline
   tooling.
3. **The leftovers were dead weight.** After retirement the validator code, the
   frozen ABI docs, and the runtime-probe scripts remained in-tree as
   "reference-only" material with no callers. They were deleted in the 2026-10
   backpedal cleanup; this page is the only record kept in the repository.

## What replaced it

- **Host correctness today:** the Hydra host integration tests and runtime CI
  (see [`TESTING.md`](TESTING.md)).
- **Product correctness:** decompiler unit/integration tests plus end-to-end
  observable parity in the native-recompile workflow.
- **emu86:** frozen offline tooling under the `emu86` cargo feature; see
  [`docs/emu86/sst.md`](../emu86/sst.md).

## Recovering details

Everything described here is in git history: the validator code
(`dis86/src/emu86/validator/`), its frozen ABI and phase specs
(`FREEZE_ABI_V1.md`, `PHASE1_SPEC.md`, `PHASE1_IMPLEMENTATION.md`,
`REVIEW.md`, `SOURCES.md`), and the probe scripts were all present before the
cleanup commit that introduced this file.
