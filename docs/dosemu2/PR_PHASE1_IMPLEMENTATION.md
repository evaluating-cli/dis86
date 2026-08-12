# Draft Pull Request: Phase 1 `simx86` Validator Architecture

> **Draft — architecture/evidence consolidation.** PR #10 now documents the implementation that has landed across #12, #14, #17, #18, #19, and #20, and separates implemented/smoke-tested behavior from semantic E2E work that is still open.

## Current status

- **#12 merged:** CMPS/REP behavior, configurable runtime PSP loading, initial-state normalization.
- **#14 merged:** SHL/SHR/SAR semantics aligned with the pinned simx86 interpreter.
- **#17 merged:** initial five-patch dosemu2 carrier with hook/ABI, live low-memory export, target lifecycle, atomic lifecycle flags, and protected-mode rejection.
- **Current carrier on main:** nine patches; adds DOS-handler exclusion, pre-execution termination publication, dynamic target-drive identity, and validator single-step fault classification.
- **#18 merged:** Rust consumes node-boundary outcomes and advances/computes comparison state from published metadata.
- **#19 merged:** cooperative shutdown, diagnostics, kill fallback, explicit reap.
- **#20 merged:** pinned-runtime terminating MZ fixture through the dosemu2 backend/validator path.

## Reconciled architecture contract

PR #10 requires and now records:

- request/apply and publish/ack synchronization around the persistent `FindExecCode()` node boundary;
- authoritative post-node PC and fresh node lookup after imported `CS:IP`;
- real-mode segment-cache updates and fail-closed protected-mode import;
- ABI-v1 80-byte control structure in page-sized `/hydra_remote`;
- `TNode.seqnum` as decoded-instruction count;
- forced `cpu_vm emulated`, `cpuemu 1`, `cpu_vm_dpmi emulated`, `mappingdriver mapshm` launch configuration;
- executable-scoped target identity with explicit drive-letter wildcard support only where required by `-K`;
- live `/dosemu_mem` export from the actual `MAPPING_LOWMEM` backing, with no exact backing-size equality assumption;
- target-owned-PC filtering so DOS/BIOS handler code does not consume a target request;
- PSP-ancestry descendant bypass and permanent target-exit latch;
- release-published lifecycle flags;
- metadata-driven Rust outcome handling;
- zero-more-controlled-nodes end barrier and parent-owned cooperative shutdown/reaping.

## Evidence now established

### Source/build/runtime smoke

- [x] complete current nine-patch series exists on `main`
- [x] pinned dosemu2 patch application/build/link gate
- [x] pinned FDPP + comcom32 runtime provisioning
- [x] ABI initialization
- [x] basic request/step acknowledgement
- [x] external `/dosemu_mem` bidirectional alias proof
- [x] end-barrier proof
- [x] clean/cooperative shutdown path
- [x] terminating MZ fixture through dosemu2 backend

### Implemented but still requiring expanded E2E proof

- [ ] REP MOVS/STOS/CMPS/SCAS normalization
- [ ] STI/MOV SS/POP SS multi-instruction behavior
- [ ] shadow + REP composition without double-consuming the first REP iteration
- [ ] representative DOS/BIOS handler exclusion
- [ ] representative child/helper exclusion
- [ ] target -> child -> target lifecycle
- [ ] target -> parent / stale-PSP lifecycle
- [ ] broad external state/control redirection coverage
- [ ] complete per-boundary state + relevant-memory differential corpus

## Reconciliation with the supplied status diff

The supplied diff correctly reflects a newer repository state than the previous PR #10 text: the hook/low-memory export are implemented rather than hypothetical, the Rust adapter is no longer merely pending, and focused pinned-runtime tests now exist.

Two wording constraints are retained here:

1. **Implemented is not the same as integration-tested.** DOS-handler exclusion, descendant/lifecycle paths, REP, and interrupt-shadow handling have source implementations but still need representative expanded runtime fixtures.
2. **Smoke success is not performance evidence.** No validator-speedup claim follows from these correctness tests; normal Hydra hybrid performance remains a separate benchmark workload.

## Files

- `PHASE1_SPEC.md` — normative architecture/ABI contract.
- `PHASE1_IMPLEMENTATION.md` — concrete implementation map and remaining semantic proof.
- `PR_PHASE1_IMPLEMENTATION.md` — this PR consolidation note.
- `README.md`, `REVIEW.md`, `TESTING.md`, `PR_DESCRIPTION.md` — reconciled status/evidence vocabulary across the wider dosemu2 docs.

Keep PR #10 as draft until the expanded semantic runtime corpus is either landed or explicitly split into the next reviewable milestone.
