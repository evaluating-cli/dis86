# Dosemu2 port: revised technical stance and Phase 0 gate

The migration remains technically credible, but several source-level conclusions should be treated as explicit hypotheses until the simulator behavior is proven experimentally.

## Revised technical hypotheses

| Area | Current stance |
| --- | --- |
| Core integration | A pure out-of-band plugin is insufficient; the likely design is hybrid integration using a core callback, single-instruction block mode, generated instrumentation, or another `simx86` mechanism established in Phase 0. |
| Workload performance | JIT block execution may substantially improve normal Hydra runs, but strict lockstep validation is a separate workload and must be benchmarked independently. |
| Memory subsystem | Investigate reuse/export of dosemu2's existing `lowmem_base` backing before introducing a second shared-memory allocation; verify that its mirror semantics match validator requirements. |
| REP contract | Phase 0 must define and verify whether a REP-prefixed instruction is one observable architectural step or whether sub-iterations must be externally visible. |
| Control redirection | Hydra must escape stale translated execution and resynchronize CPU/segment-derived state after control-flow changes; the exact `simx86` mechanism remains a Phase 0 question. |

## 1. Integration model: likely hybrid integration

A pure out-of-band plugin is insufficient because `simx86` translates and executes multi-instruction blocks. However, the minimal integration mechanism is not yet established.

Candidate approaches include:

- a direct core callback at an architectural instruction boundary;
- forcing a single-instruction / one-block stepping mode;
- generating Hydra instrumentation into translated blocks; or
- another existing `simx86` facility discovered during prototype work.

Phase 0 must determine the smallest patch surface that provides the required semantics. The project should not commit to a particular `interp.c` hook or instrumentation strategy before that experiment succeeds.

## 2. Workload separation and performance qualification

Two workloads must be measured independently.

### Normal Hydra hybrid execution

`simx86` may retain translated block execution between native/decompiled and emulated boundaries, so it may provide substantial speedups over interpreted execution. This must be benchmarked on representative DOS binaries rather than inferred from raw simulator throughput.

### `emu86_validator` differential testing

Strict differential validation is effectively:

1. execute one guest instruction;
2. expose architectural state;
3. synchronize with the peer emulator;
4. compare state;
5. repeat.

This pattern fragments normal JIT block execution and introduces synchronization overhead. The original 10–50x validator-speedup estimate is therefore not established and should not be used as a planning assumption.

## 3. Low-memory backing: investigate `lowmem_base` first

Before introducing an independent `/dev/shm/dosemu_mem` allocation, investigate whether dosemu2's existing `lowmem_base` backing can be exported to the external validator.

The memory investigation must determine whether the validator needs:

- the simulator's low-memory image exposed through `lowmem_base`;
- logical DOS memory semantics through `MEM_BASE32(addr)` or related accessors; or
- a deliberately defined combination of the two.

Do not assume `MEM_BASE32(addr)` and `lowmem_base + addr` are interchangeable. Their semantics can differ around logical mappings, holes, video memory, and protection behavior.

This memory validation follows the CPU-control architectural gate and should be treated as its own experiment rather than silently folded into Phase 0.

## 4. REP stepping contract

The primary issue is the validator's architectural stepping contract, not callback placement for string operations in isolation.

Phase 0 must formally define and test whether a REP-prefixed instruction such as `REP MOVS` or `REP STOS` is observed as:

- one guest instruction whose internal repetitions complete before the next architectural boundary; or
- a sequence in which individual repetitions are externally observable.

The intended contract must then be checked against both `emu86` and `simx86`. Until that source-level verification is recorded, REP atomicity should remain a tested hypothesis rather than a settled compatibility claim.

## 5. coopth and HLT do not replace arbitrary `CS:IP` interception

Dosemu2 cooperative threading and HLT handlers may be useful supporting facilities for control transfers, asynchronous services, or implementation structure. They do not by themselves provide a callback when existing guest execution reaches an arbitrary original function address.

Hydra therefore still requires an execution-address interception mechanism at the CPU boundary.

## 6. Control redirection and translated simulator state

Assigning `_AX`, `_IP`, `_CS`, or other register macros is not by itself proof that execution can safely continue after Hydra redirects control flow with operations such as `RETURN_FAR` or `RETURN_JUMP`.

The architectural requirement is to:

- leave or terminate the currently active translated execution path when necessary;
- avoid or invalidate translated state that is no longer valid;
- resynchronize segment-derived and CPU state through the appropriate simulator path; and
- resume execution at the redirected architectural state without stale execution artifacts.

Phase 0 must establish the exact `simx86` mechanism and API. It should not assume that a specific helper such as `leave_block()` or global translation-cache invalidation is required until demonstrated.

## Phase 0: architectural gate

Before implementing the full Hydra ABI, validator transport, or memory-sharing design, prove the following with a minimal `simx86` experiment:

1. Step exactly one guest instruction.
2. Extract complete architectural state: general registers, segment registers, `CS:IP`, and FLAGS.
3. Mutate `CS:IP` and register state externally.
4. Escape any active translated execution state that is no longer valid and resynchronize CPU/segment state cleanly.
5. Resume execution and compare the resulting architectural state against a known-good execution path, proving that state manipulation does not violate x86 architectural semantics.
6. Verify deterministic behavior for CALL, RET, RETF, INT, instruction prefixes, and REP-prefixed string operations.

Passing this gate establishes that Hydra-style interception is technically sound on `simx86` and reveals the minimal core patch surface. Failing it still produces a useful result: the simulator integration requirements become concrete before substantial validator or plugin code is written.

## Follow-on sequence

After Phase 0 succeeds:

1. define the dosemu2-side register snapshot ABI;
2. investigate/export the appropriate low-memory backing and validate its semantics;
3. implement `/dev/shm/hydra_remote` synchronization;
4. adapt `emu86_validator` to launch and drive dosemu2;
5. validate a small differential corpus;
6. implement Hydra address interception and native-function execution;
7. validate overlays and other edge cases;
8. benchmark normal Hydra execution and lockstep validation separately;
9. decide the final boundary between the isolated `simx86` patch and the surrounding Hydra module.

The migration should proceed from verified CPU semantics rather than early implementation assumptions.
