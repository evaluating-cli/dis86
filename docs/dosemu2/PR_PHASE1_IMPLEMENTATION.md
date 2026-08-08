# Pull Request: Harden Phase 1 `simx86` Core Hook & Low-Memory Export Blueprint

## Summary

This PR replaces the initial Phase 1 draft with a source-verified implementation contract for integrating dosemu2 `simx86` with the `dis86` differential validator.

The revision resolves the original review findings and the follow-up source investigation:

- removes the false `G->seqlen == 1` invariant;
- splits request/apply and publish/ack around `DoExec(G)`;
- publishes post-step `IP` from the authoritative local `PC`;
- recomputes local `PC` before lookup after external `CS:IP` mutation;
- removes `EXCP_EMULEAVE` from normal validator redirection;
- preserves upper EFLAGS/register halves on 16-bit ABI import;
- rejects protected-mode segment mutation instead of silently leaving stale descriptor caches;
- replaces the unsafe global `do_open_pshm()` rename with a `MAPPING_LOWMEM`-only named POSIX-SHM backing path;
- explicitly selects `mapshm` in validator mode because full-sim `softmmu` low memory is anonymous;
- makes `end` an execution stop barrier so no further guest instruction executes;
- replaces hard-coded/inequality entry gating with an exact MZ entry predicate derived from runtime PSP + MZ-relative `CS:IP`;
- defines validator normalization for REP micro-iterations and the `STI` / `MOV SS` / `POP SS` interrupt-shadow node cases;
- removes inherited DOSBox-X `-hydra` / `-hydra-conf` launch arguments, which upstream dosemu2 does not implement.

## Files

- `docs/dosemu2/PHASE1_SPEC.md` — corrected architecture/stepping contract.
- `docs/dosemu2/PHASE1_IMPLEMENTATION.md` — concrete source-verified implementation blueprint and verification checklist.
- `docs/dosemu2/PR_PHASE1_IMPLEMENTATION.md` — this PR description.

## Source verification

The revised blueprint was checked against current dosemu2 `devel` at commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`, including `interp.c`, `codegen.h`, `cpu-emu.c`, `protmode.c`, `mapping.c`, `mapfile.c`, `mapping.h`, and `etc/global.conf`, plus the current dis86 MZ loader, validator adapter, REP implementation, and shared-memory ABI.

This remains a documentation/architecture PR: it does not claim that the downstream dosemu2 implementation has been compiled or integration-tested yet. The implementation checklist now states the exact behaviors that the subsequent code patch must prove.
