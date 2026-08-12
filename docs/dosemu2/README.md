# Porting dis86 / Hydra to dosemu2

This directory records both the validator contract and the evidence for the dosemu2 migration. PR #12 added configurable executable loading and `CMPS` support; PR #17 established the pinned dosemu2 patch carrier; PRs #18–#20 completed the current Rust-side node-outcome/shutdown path and added a terminating runtime fixture.

## Authoritative implementation coordinates

- **Current dosemu2 implementation carrier:** `patches/dosemu2/`
- **Pinned dosemu2 commit:** `604ce0cdd1a71f657e2a2df623d216d5ab289313`
- **Shared-memory ABI:** version **1**, 88-byte append-only structure preserving the legacy 64-byte Hydra prefix
- **Current carrier size:** nine ordered patches

The carrier is a `git am` patch series because there is not yet a writable dosemu2 fork. It contains the executable-scoped `simx86` hook and exports the live `mapshm` allocation backing `lowmem_base` as `/dosemu_mem`. These are implemented behavior, not hypothetical approaches.

## Status vocabulary

| Label | Scope |
| --- | --- |
| **Specified** | Required behavior documented in `PHASE1_SPEC.md`. |
| **Implemented** | Code exists in the dosemu2 patch carrier and/or dis86 adapter. |
| **Unit tested** | Host-independent tests cover local ABI, state, and comparison logic. |
| **Pinned-runtime tested** | CI applies the patches to the pinned commit and boots that runtime for focused proofs. |
| **Still-unverified E2E** | No passing expanded pinned-runtime differential corpus yet. |

## Evidence summary

**Specified and implemented:** ABI-v1 initialization; request/apply/publish/ack synchronization for ordinary target-owned nodes; validator-bounded `MSSTP` execution; decoded-instruction/outcome metadata; real-mode segment updates and protected-mode rejection; live low-memory export; executable/PSP ownership gating; descendant-helper bypass; pre-execution termination publication; dynamic target-drive identity; target-exit/end-barrier publication; Rust-side launcher/descendant process-ownership validation; and Rust-side outcome handling/cooperative shutdown. The DOS-handler PC filter exists, but acknowledgement deferral to a post-service target-owned boundary is not implemented.

**Unit tested:** Rust-side ABI access, initial-state comparison, and handling of multi-instruction, same-PC, target-exit, end-acknowledgement, and fault outcomes, plus the reference CPU suite. Unit tests do not prove dosemu2 runtime semantics.

**Pinned-runtime tested:** complete patch-series application/build/link, pinned FDPP/comcom32 provisioning, ABI initialization, a basic request/step acknowledgement, bidirectional `/dosemu_mem` alias visibility, the end barrier/clean shutdown, and one small terminating MZ fixture driven through the dosemu2 backend.

**Still unverified E2E:** REP behavior, interrupt-shadow composition, DOS/BIOS and child/helper exclusion under realistic execution, target lifecycle transitions, and the full instruction-by-instruction differential comparison. Do not describe these as integration-tested until the expanded pinned-runtime corpus passes.

Performance claims and Hydra native-function interception remain separate work and must be evaluated independently from validator lockstep correctness.

## Documents

- `PHASE1_SPEC.md` — normative execution/ABI contract and evidence status.
- `PHASE1_IMPLEMENTATION.md` — implementation map, adapter behavior, and remaining work.
- `PR_PHASE1_IMPLEMENTATION.md` — PR #10 consolidation/status notes.
- `REVIEW.md` — technical review and evidence boundaries.
- `TESTING.md` — unit, pinned-runtime smoke, and expanded E2E gates.
- `PR_DESCRIPTION.md` — concise migration/status summary.
- `patches/dosemu2/README.md` — patch application and provenance.
