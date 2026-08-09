# Draft Pull Request: Correct Phase 1 `simx86` Architecture Blueprint

> **Draft — documentation only.** This PR corrects the Phase 1 contract. It does not implement the downstream dosemu2 hook or the complete `dis86` adapter, and it must not be presented as fixing the runtime integration.

## Summary

This PR replaces the initial Phase 1 draft with a reviewed architecture contract for integrating dosemu2 `simx86` with the `dis86` differential validator.

The revision resolves the original review findings and the follow-up source investigation:

- removes the false `G->seqlen == 1` invariant;
- splits request/apply and publish/ack around `DoExec(G)`;
- publishes post-step `IP` from the authoritative local `PC`;
- recomputes local `PC` before lookup after external `CS:IP` mutation;
- removes `EXCP_EMULEAVE` from normal validator redirection;
- preserves upper EFLAGS/register halves on 16-bit ABI import;
- rejects protected-mode segment mutation instead of silently leaving stale descriptor caches;
- replaces the unsafe global `do_open_pshm()` rename with a `MAPPING_LOWMEM`-only named POSIX-SHM backing path;
- explicitly selects `mapshm` through dosemu2's verified `$_mapping` configuration interface because full-sim `softmmu` low memory is anonymous;
- makes `end` an execution stop barrier so no further guest instruction executes;
- replaces hard-coded/inequality entry gating with an exact MZ entry predicate derived from runtime PSP + MZ-relative `CS:IP`;
- defines validator normalization for REP micro-iterations and the `STI` / `MOV SS` / `POP SS` interrupt-shadow node cases;
- removes inherited DOSBox-X `-hydra` / `-hydra-conf` launch arguments, which upstream dosemu2 does not implement.

## Non-goals

This PR does not:

- patch or build dosemu2;
- change `DosemuProcess::spawn()`;
- implement configurable runtime-PSP loading in emu86;
- implement CMPS;
- implement boundary-driven normalized step outcomes; or
- claim successful runtime or integration testing.

Those changes belong to the implementation branches below.

## Draft exit criteria

Keep this PR open as a draft until both implementation branches exist and are linked here:

- [ ] **emu86/dis86 branch:** CMPS, configurable PSP loading, corrected dosemu2 launch, initial-state normalization, and boundary-driven step normalization.
- [ ] **dosemu2 branch:** page-sized shared-control mapping, the `simx86` request/ack hook, executable-bound PSP capture, named low-memory export, and node-boundary reporting.

The branches do not need to be merged before this architecture PR, but reviewers must be able to validate the contract against concrete implementation work before marking it ready for review.

## Files

- `docs/dosemu2/PHASE1_SPEC.md` — corrected architecture/stepping contract.
- `docs/dosemu2/PHASE1_IMPLEMENTATION.md` — concrete source-verified implementation blueprint and verification checklist.
- `docs/dosemu2/PR_PHASE1_IMPLEMENTATION.md` — this PR description.

## Source verification

The revised blueprint was checked against current dosemu2 `devel` at commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`, including `interp.c`, `codegen.h`, `cpu-emu.c`, `protmode.c`, `mapping.c`, `mapfile.c`, `mapping.h`, and `etc/global.conf`, plus the current dis86 MZ loader, validator adapter, REP implementation, and shared-memory ABI.

This remains a documentation/architecture PR. It does not claim that the downstream dosemu2 implementation or the required `dis86` changes have been written, compiled, or integration-tested. The implementation checklist states the behaviors the two follow-up branches must prove.
