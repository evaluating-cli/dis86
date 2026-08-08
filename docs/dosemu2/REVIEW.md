# Investigation review findings

The original migration direction is credible, but several source-level claims need to be treated as hypotheses rather than settled design decisions.

## Critical findings

### 1. A pure plugin is unlikely to be sufficient

Hydra requires interception at arbitrary guest instruction boundaries and at arbitrary original function addresses. Current `simx86` builds translated instruction sequences and executes them as blocks. A normal plugin callback therefore cannot simply run between every guest instruction while retaining normal block-JIT behavior.

Likely options are:

- a deliberate single-step / one-instruction-block mode; or
- instrumentation generated into translated code so each architectural instruction can call the Hydra boundary hook.

Either approach implies some `simx86` core integration.

### 2. Lockstep validation and JIT throughput are different workloads

Strict differential validation is effectively:

1. execute one guest instruction;
2. expose/register state;
3. synchronize with the peer emulator;
4. compare state;
5. repeat.

That execution pattern removes much of the benefit of normal multi-instruction JIT blocks. Performance claims for normal Hydra execution must therefore be separated from performance claims for lockstep validation.

The proposed 10–50x validator speedup should not be treated as established until benchmarked.

### 3. Reuse dosemu2's low-memory backing before replacing allocation

Current dosemu2 already maintains a shared low-memory image (`lowmem_base`) for the simulator. The first memory experiment should determine whether its existing backing object can be exported to the external validator.

This is preferable to adding an unrelated second `/dev/shm/dosemu_mem` allocation.

Also, `MEM_BASE32(addr)` and `lowmem_base + addr` should not be assumed to have identical semantics. The former represents the logical DOS memory mapping, while the latter is used as the simulator's low-memory image.

### 4. REP handling depends on the stepping contract

The primary question is not whether REP MOVS/STOS needs a special callback placement. The architectural contract must first define whether a REP-prefixed instruction is observed as one guest instruction or whether individual iterations are externally visible.

Once single-step semantics are specified, REP behavior should be tested against that contract.

### 5. coopth / HLT facilities do not replace arbitrary CS:IP interception

Dosemu2 cooperative threading and HLT handlers may be useful for control transfers or asynchronous services, but they do not inherently provide a hook when existing guest execution reaches an arbitrary original function address.

They should therefore be considered supporting facilities, not substitutes for the CPU hook.

### 6. Register updates must account for translated simulator state

Assigning `_AX`, `_IP`, `_CS`, etc. is not by itself proof that execution can safely continue after Hydra redirects control flow. `simx86` also maintains translated execution state and segment-derived state.

After Hydra modifies control flow, the integration may need to:

- leave the current translated block;
- avoid or invalidate stale translated state;
- recompute segment bases/state through the normal synchronization path;
- re-enter execution cleanly.

This is one of the first behaviors the prototype must prove.

## What remains sound

The high-level Hydra model remains a good fit for the migration:

- native/decompiled functions are selected by original x86-16 address;
- native code can inspect and modify guest registers and memory;
- native code can call back into x86-16 execution;
- differential validation still benefits from a separately observable emulator state.

The dosemu2 `mmap_min_addr`/non-zero-base model also reinforces the requirement to use dosemu2's memory abstraction instead of assuming identity-mapped low memory.

## Revised first milestone

Before implementing the full shared-memory protocol or Hydra ABI, build a minimal simulator experiment that proves all of the following:

1. exactly one guest instruction can be executed;
2. complete architectural state can be read at that boundary;
3. `CS:IP` can be modified externally;
4. execution resumes at the new target without stale translated state;
5. CALL, RET, RETF, interrupts, prefixes and REP behave deterministically.

If this milestone succeeds, the rest of the port becomes a tractable integration exercise. If it fails, the required simulator patch surface will be clearer before significant validator/plugin code is written.
