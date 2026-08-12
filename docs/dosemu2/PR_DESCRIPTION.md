# dosemu2 validator migration: implementation and verification status

## Relationship to landed work

PR #12 added configurable executable loading and `CMPS` support. PR #17 added the pinned dosemu2 validator implementation carrier. PR #18 taught the Rust validator to consume dosemu2 node-boundary outcomes, PR #19 added cooperative shutdown/diagnostics/reaping, and PR #20 added a terminating dosemu2-backed validator fixture.

Until the dosemu2-side changes are replayed onto a writable dosemu2 fork, **`patches/dosemu2/` is the implementation carrier**. It is an ordered patch series, not merely an architectural proposal.

The series applies to dosemu2 commit `604ce0cdd1a71f657e2a2df623d216d5ab289313` and exposes validator ABI version **1**. The current series contains nine patches.

## Evidence vocabulary

| Status | Meaning |
| --- | --- |
| Specified | Required contract is documented; this alone is not implementation evidence. |
| Implemented | Code is present in the dosemu2 patch carrier and/or dis86 adapter. |
| Unit tested | Host-independent tests exercise local data layout, comparison, or state-handling logic without booting dosemu2. |
| Pinned-runtime tested | CI applies the series to the exact pinned commit, builds/boots the pinned runtime stack, and exercises the stated focused path. |
| Still-unverified E2E | Behavior has not passed the expanded pinned-runtime differential corpus and must not be described as integration-tested. |

## Current status

### Specified and implemented

The current carrier implements the executable-scoped `simx86` request/execute/ack hook, ABI-v1 register exchange, node metadata, live low-memory export, protected-mode rejection, target/child lifecycle policy, DOS-handler exclusion, pre-execution termination publication, dynamic target-drive identity, and validator single-step fault classification.

The Rust side consumes decoded-node outcomes, performs normalized initial-state comparison, handles terminal/fault categories, and shuts the dosemu2 child down cooperatively with explicit reaping.

### Unit tested

Host-independent tests cover validator outcome interpretation (including multi-instruction nodes, same-PC nodes, target exit, end acknowledgement, and faults), initial-state comparison, ABI access/layout logic, and the reference CPU behavior. These prove local code paths, not dosemu2 execution semantics.

### Pinned-runtime tested

The carrier workflow applies the complete patch series to the pinned dosemu2 commit, checks the resulting diff, builds and links the touched runtime, and provisions pinned FDPP/comcom32. Focused runtime tests prove:

- ABI-v1 initialization;
- a basic request/step acknowledgement;
- external `/dosemu_mem` bidirectional alias visibility;
- the zero-more-controlled-nodes end barrier;
- clean/cooperative shutdown; and
- one small terminating MZ fixture through the dosemu2 backend/validator path.

### Still unverified end to end

The current smoke coverage is not the expanded instruction corpus. In particular, **REP semantics, interrupt-shadow composition, representative DOS/BIOS and child/helper exclusion, target lifecycle transitions, and full differential state/memory comparison remain unverified E2E**.

Performance and Hydra native-function interception are outside the current validator proof.

## Next gate

Add the expanded pinned-runtime corpus with focused fixtures for REP string operations, shadow instructions, DOS/BIOS and child/helper execution, target-to-child-to-target and target-to-parent transitions, external register/segment/memory mutation, and complete per-boundary state/memory comparison against emu86.
