# Porting dis86 / Hydra to dosemu2

This directory tracks the investigation and implementation plan for replacing the patched DosBox-X execution backend used by Hydra and the `emu86` differential validator with `dosemu2`.

## Current conclusion

The migration appears technically viable, but the initial idea of implementing it as a pure `dosemu2` plugin is too optimistic. Hydra needs control at guest instruction boundaries and arbitrary `CS:IP` interception. Current `simx86` executes translated multi-instruction blocks, so the likely design is:

1. a small, isolated `simx86` instrumentation patch that provides deterministic instruction-boundary control;
2. a Hydra integration module around that CPU hook;
3. reuse/export of dosemu2's existing low-memory shared backing where possible;
4. adaptation of `dis86`'s validator/process tooling to the new backend.

## Recommended implementation order

### Phase 0 — prove CPU control semantics

Before designing the full integration:

- stop execution after exactly one guest instruction;
- extract `CS:IP`, FLAGS and all general/segment registers;
- alter `CS:IP` externally and resume safely;
- verify translated-state invalidation/re-entry behavior;
- test CALL/RET/RETF, interrupts, prefixes and REP string operations.

This is the architectural gate. A plugin API is not useful unless the CPU backend can provide the required execution semantics.

### Phase 1 — validator transport

- define the dosemu2-side register snapshot ABI;
- implement `/dev/shm/hydra_remote` synchronization;
- adapt `src/emu86/validator/hydra_process.rs` to launch dosemu2;
- validate a small instruction corpus before optimizing anything.

### Phase 2 — memory sharing

Current dosemu2 already maintains a shared low-memory image (`lowmem_base`) for simx86. Investigate exporting that existing backing object to the validator rather than introducing a second independent mapping.

Do not assume `MEM_BASE32(addr)` and `lowmem_base + addr` are interchangeable; their semantics differ around logical DOS mappings and protected/video-memory regions.

### Phase 3 — Hydra execution hooks

Once precise instruction control works:

- intercept registered Hydra function addresses;
- transfer guest register state into Hydra;
- run the native/decompiled function;
- handle Hydra result types (near/far return, call, jump, resume);
- force a safe exit from the current translated block whenever Hydra changes control flow.

### Phase 4 — overlays and edge cases

- validate INT 3Fh overlay behavior;
- define whether REP is one architectural step or exposes iterations to the validator;
- calibrate FLAGS comparison masks against simx86 behavior;
- verify dynamic code-load offsets.

### Phase 5 — benchmark and CI

Benchmark two workloads separately:

- normal Hydra hybrid execution, where JIT execution can provide a substantial benefit;
- strict instruction-lockstep validation, where synchronization and forced stepping may dominate runtime.

Do not assume the original 10–50x validation speedup until measured.

## Expected repository split

This fork should contain the `dis86`-side validator/tooling changes. Any required simx86 instrumentation belongs in a coordinated `dosemu2` fork rather than being vendored into this repository.
