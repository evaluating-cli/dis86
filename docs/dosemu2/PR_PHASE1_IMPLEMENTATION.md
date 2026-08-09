# Draft Pull Request: Phase 1 `simx86` Validator Architecture

> **Draft — architecture/documentation.** PR #10 defines the contract for the dosemu2-backed differential validator. Concrete implementation is split across merged emu86 prerequisites, draft dosemu2 carrier #17, and a still-pending Rust adapter.

## Current status

- **#12 merged:** CMPS/REP behavior, configurable runtime PSP loading, and initial-state normalization.
- **#14 merged:** SHL/SHR/SAR semantics aligned with the pinned dosemu2 `simx86` interpreter.
- **#17 draft:** four-patch dosemu2 carrier implementing the control ABI/hook, live low-memory export, target lifecycle policy, and atomic lifecycle flags.
- **Rust adapter pending:** corrected dosemu2 launch, extended ABI handling, runtime-PSP handshake, boundary-driven normalized stepping, lifecycle handling, and explicit child reaping remain to be implemented.

At this consolidation point, #17's ordinary `test` workflow passes but its current `dosemu2 patch series` workflow fails. PR #10 must therefore not claim that the current #17 head has a green apply/compile/link proof.

## Consolidated architecture contract

PR #10 now requires:

- hook placement around the persistent `FindExecCode()` node boundary;
- authoritative post-node state from the returned local `PC`;
- local `PC` recomputation before lookup after imported `CS:IP`;
- no `EXCP_EMULEAVE` for normal validator state redirection;
- explicit release-build protected-mode rejection **before** any state import;
- the versioned 80-byte control ABI from #17, including `runtime_psp`, `decoded_instructions`, and `step_flags`;
- page-sized `/hydra_remote` backing;
- forced `$_cpu_vm = "emulated"`, `$_cpuemu = (1)`, and `$_mapping = "mapshm"` launch configuration;
- target identity from the requested DOS path plus validated PSP/MCB/environment/current-PSP state;
- target/descendant/exit lifecycle tracking through `sda_cur_psp()` and dosemu's PSP parent field;
- actual translated-node consumption from `TNode.seqnum`, not `seqlen` or opcode guesses;
- REP and interrupt-shadow normalization that never double-consumes an already-executed first REP iteration;
- `/dosemu_mem` as the verified live `MAPPING_LOWMEM` backing, not a copy;
- `END_ACK`/`TARGET_EXIT` lifecycle publication through release-ordered `step_flags`;
- parent-owned process cleanup that terminates **and explicitly reaps** dosemu2.

## Superseded material removed

The consolidated docs no longer treat any of the following as the Phase 1 contract:

- the old 64-byte Phase 0 shared-memory layout;
- `DIIS_DOSEMU_EXE` as the current target-path variable;
- a DOS-exec callback/`g_target_exec_seen` mechanism as required implementation;
- `assert(!PROTMODE())` as acceptable protected-mode handling;
- `$_mapping = "mapshm"` alone as sufficient to reach the simx86 hook;
- CMPS/configurable PSP loading as unimplemented future work;
- a fixed emu86 PSP of `0x0813` as a validator constraint;
- an inferred fixed comparison span for STI/MOV SS/POP SS;
- “three patches” as the current #17 carrier series;
- a current claim that #17's patch-series CI is green.

## Known implementation gaps

1. **Protected-mode import in #17:** the current carrier `apply_cpu()` still needs an explicit real-mode guard/error-stop path before any register or segment mutation.
2. **#17 patch-series CI:** current head is red in the dedicated dosemu2 patch-series workflow; the build-gate evidence must be restored.
3. **Rust adapter:** main still uses the Phase-0/DOSBox-X launch and old req/ack-only shared contract.
4. **Shutdown:** Rust `Drop` still needs an explicit `wait()`/equivalent reap after termination.
5. **Runtime proof:** end barrier, target child/exit lifecycle, external low-memory visibility, normalized REP/shadow boundaries, and DOS/INT 21h boundary visibility remain unproven end-to-end.

## Draft exit criteria

### Landed prerequisites

- [x] #12 CMPS/configurable PSP/initial normalization
- [x] #14 shift-semantic alignment

### dosemu2 carrier

- [x] concrete four-patch source implementation exists in #17
- [x] versioned ABI and node metadata are represented
- [x] live low-memory export/provenance checks are represented
- [x] target lifecycle and atomic lifecycle flags are represented
- [ ] protected-mode import is explicitly fail-closed
- [ ] current dedicated patch-series CI is green

### Rust integration

- [ ] corrected launch contract implemented
- [ ] 80-byte ABI/version validation implemented
- [ ] runtime PSP initialization handshake implemented
- [ ] metadata-driven normalized stepping implemented
- [ ] target-exit/fault/end lifecycle events handled
- [ ] dosemu2 child explicitly reaped

### Runtime proof

- [ ] zero additional controlled guest nodes after end barrier
- [ ] target -> child -> target lifecycle verified
- [ ] target -> parent / stale-PSP lifecycle verified
- [ ] external `/dosemu_mem` bidirectional runtime visibility verified
- [ ] REP and interrupt-shadow composition verified
- [ ] DOS/INT 21h boundary visibility characterized

Keep PR #10 as draft until the remaining implementation and runtime-proof work is linked and reviewable.

## Files

- `docs/dosemu2/PHASE1_SPEC.md` — normative architecture contract.
- `docs/dosemu2/PHASE1_IMPLEMENTATION.md` — implementation map and remaining work.
- `docs/dosemu2/PR_PHASE1_IMPLEMENTATION.md` — PR status and review summary.

## Source basis

The architecture remains pinned to dosemu2 `604ce0cdd1a71f657e2a2df623d216d5ab289313`. #17 is the concrete carrier used to refine the contract: its current four patches establish the ABI, `TNode.seqnum` metadata, mapshm low-memory provenance, current-PSP/ancestry lifecycle handling, and release-ordered lifecycle flags.
