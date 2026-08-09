# Draft Pull Request: Correct Phase 1 `simx86` Architecture Blueprint

> **Draft — documentation/architecture.** PR #10 defines the Phase 1 contract. It does not by itself complete the downstream dosemu2 hook or the `dis86` adapter, and it must not be presented as fixing the runtime integration.

## What is happening right now

PR #10 is the architecture contract against which the implementation work is reviewed.

Concrete implementation work now exists:

- **emu86/dis86:** #12 landed CMPS, configurable runtime-PSP loading, and initial-state normalization. #14 subsequently aligned SHL/SHR/SAR semantics with the pinned dosemu2 `simx86` interpreter.
- **dosemu2:** #17 is the draft carrier PR for the pinned dosemu2 patch series. The patches apply, compile, and link in CI, but the remaining runtime proof items are still open.
- **Rust validator adapter:** the final dosemu2 launch contract and boundary-driven step integration still need to be implemented against the dosemu2-reported node metadata.

Keep PR #10 as a draft until the remaining runtime proof items in #17 are resolved and the Rust adapter implementation is linked here.

## Summary

This PR replaces the initial Phase 1 draft with a reviewed architecture contract for integrating dosemu2 `simx86` with the `dis86` differential validator.

The contract:

- removes the false `G->seqlen == 1` invariant;
- splits request/apply and publish/ack around `DoExec(G)`;
- publishes post-step `IP` from the authoritative local `PC`;
- recomputes local `PC` before lookup after external `CS:IP` mutation;
- removes `EXCP_EMULEAVE` from normal validator redirection;
- preserves upper EFLAGS/register halves on 16-bit ABI import;
- requires explicit runtime rejection of protected-mode state import before any CPU mutation;
- replaces the unsafe global `do_open_pshm()` rename with a `MAPPING_LOWMEM`-only named POSIX-SHM backing path;
- selects `mapshm` through dosemu2's verified `$_mapping` configuration interface because full-sim `softmmu` low memory is anonymous;
- treats `shm->end` as an execution stop barrier, separate from parent-owned process termination and explicit child reaping;
- binds the MZ entry gate to the requested executable and its captured runtime PSP, not merely a generic PSP signature plus relative entry coordinates;
- derives interrupt-shadow comparison spans from actual translated-node consumption plus the authoritative post-node PC rather than opcode guesses;
- coalesces REP micro-iterations without executing or comparing an already-consumed first REP iteration twice; and
- removes inherited DOSBox-X `-hydra` / `-hydra-conf` launch arguments, which upstream dosemu2 does not implement.

## Patch review notes

The follow-up patches are directionally correct, with these required clarifications:

1. **Requested-executable entry gate:** accept. The target identity must be captured by the DOS exec/load path and cleared/lifecycle-managed with the target process; the PSP signature is only a sanity check, not executable identity.
2. **REP + interrupt-shadow normalization:** accept the two-case model. Use actual node-consumption metadata reported by the dosemu2 hook (the implementation work in #17 reports decoded-node consumption) together with the authoritative returned PC. A combined shadow-plus-REP node must account for the first REP iteration already consumed before issuing more raw requests.
3. **Shutdown:** the stop-barrier wording is correct but incomplete unless `DosemuProcess::Drop` explicitly reaps the child. After termination, call `wait()` or an equivalent reap operation; do not use `EXCP_EMULEAVE` or dosemu2 global-exit machinery as the normal simx86 hook path.
4. **Protected mode:** an `assert(!PROTMODE())` is insufficient. The implementation must take an explicit runtime error/stop path before changing any register or segment state.

## Non-goals

This PR itself does not:

- patch or build dosemu2;
- complete `DosemuProcess::spawn()` / normalized stepping;
- claim successful end-to-end runtime or integration testing; or
- replace the implementation work tracked in #12, #14, and #17.

## Draft exit criteria

- [x] **emu86/dis86 prerequisites exist:** CMPS, configurable PSP loading, and initial-state normalization landed in #12; shift semantics were aligned in #14.
- [ ] **dosemu2 runtime proof:** #17's patch series passes its source/apply/compile/link gates, but its stated runtime proof items remain open.
- [ ] **Rust adapter implementation:** corrected dosemu2 launch, initialization handshake, and boundary-driven normalized stepping are implemented and linked here.

The architecture PR can become ready for review once reviewers can validate the contract against those concrete implementation paths. Completing the runtime verification checklist remains the Phase 1 completion criterion.

## Files

- `docs/dosemu2/PHASE1_SPEC.md` — architecture and normalized-stepping contract.
- `docs/dosemu2/PHASE1_IMPLEMENTATION.md` — source-verified implementation blueprint and verification checklist.
- `docs/dosemu2/PR_PHASE1_IMPLEMENTATION.md` — current PR status and review notes.

## Source verification

The blueprint is pinned to dosemu2 `devel` commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`. The follow-up implementation work in #17 additionally source-verifies translated-node instruction consumption and the dosemu2 process/low-memory mechanisms used by the carrier patch series.

This remains an architecture PR. It does not claim that the remaining runtime behaviors have been proven until the corresponding implementation branches demonstrate them.
