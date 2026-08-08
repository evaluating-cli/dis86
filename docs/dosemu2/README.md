# Porting dis86 / Hydra to dosemu2

This directory tracks the investigation, architectural design, and implementation plan for replacing the patched DosBox-X execution backend used by Hydra and the `emu86` differential validator with `dosemu2`.

---

## Architectural Hypotheses

The porting investigation identifies the following key architectural hypotheses that guide the implementation:

### 1. Integration Model: *Likely Hybrid Integration*
A pure out-of-band plugin is unlikely to be sufficient because `simx86` translates and executes multi-instruction blocks. Hydra requires control at guest instruction boundaries and arbitrary `CS:IP` interception. Whether the minimal solution is a core callback, a single-instruction block mode, or instrumentation generated into translated code is an open question that Phase 0 must determine.

### 2. Workload Separation & Performance Qualification
* **Normal Hydra Hybrid Execution:** `simx86` can retain block execution across native C and emulated boundaries and therefore **may** provide substantial speedups over interpreted execution; this must be benchmarked separately against concrete binary workloads.
* **`emu86_validator` Differential Testing:** Strict one-instruction lockstep validation (`[1 step -> state snapshot -> IPC sync -> wait for peer]`) removes the benefit of multi-instruction JIT blocks. Performance claims for lockstep validation must be measured independently rather than assuming JIT acceleration.

### 3. Memory Subsystem: *Investigate `lowmem_base` First*
`dosemu2` already maintains a shared low-memory image (`lowmem_base`) for the simulator. The initial memory investigation will evaluate whether its existing backing object can be exported directly to the external validator rather than adding an unrelated second `/dev/shm/dosemu_mem` allocation. Note that `MEM_BASE32(addr)` (logical DOS mapping) and `lowmem_base + addr` (simulator low-memory image) have distinct semantics.

### 4. REP Stepping Contract
The validator stepping contract for `REP`-prefixed string instructions (`REP MOVS`, `REP STOS`) must be explicitly defined and tested. Phase 0 must determine whether the validator contract should treat `REP` as a single atomic architectural step or expose individual loop iterations.

### 5. Escaping Stale Translated State on Control Redirection
When Hydra intercepts a function and alters control flow (`RETURN_FAR`, `RETURN_JUMP`), assigning `_AX`, `_IP`, `_CS` is not sufficient on its own. `simx86` maintains translated execution state and segment-derived state. The integration must escape the current translated block, avoid or invalidate stale cached translation, and recompute segment bases/state through the normal synchronization path before re-entering execution.

---

## Implementation Roadmap

```
┌────────────────────────────────────────────────────────────────────────────┐
│                    Phase 0: Minimal Simulator Gate                         │
├────────────────────────────────────────────────────────────────────────────┤
│ 1. Step exactly ONE guest instruction in simx86.                           │
│ 2. Extract full architectural state (AX..FLAGS, CS:IP, Segments).          │
│ 3. Mutate CS:IP / register state externally.                               │
│ 4. Escape active translation cache and resynchronize CPU/segment state.    │
│ 5. Resume execution and compare the resulting architectural state against  │
│    a known-good execution path to prove x86 semantic correctness.          │
│ 6. Verify deterministic behavior for CALL, RET, RETF, INT, prefixes, REP.  │
└────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                    Phase 1: Validator Transport & IPC                      │
├────────────────────────────────────────────────────────────────────────────┤
│ • Define the dosemu2-side register snapshot ABI.                           │
│ • Implement /dev/shm/hydra_remote synchronization protocol.                │
│ • Adapt src/emu86/validator/hydra_process.rs to spawn dosemu2.             │
│ • Validate lockstep execution against a small instruction corpus.          │
└────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                    Phase 2: Low-Memory Export & Sharing                    │
├────────────────────────────────────────────────────────────────────────────┤
│ • Investigate exporting lowmem_base backing directly to the validator.     │
│ • Validate memory read/write consistency against logical DOS mappings.     │
└────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                    Phase 3: Hydra Hybrid Execution Hooks                   │
├────────────────────────────────────────────────────────────────────────────┤
│ • Intercept registered Hydra function addresses at CS:IP dispatch.         │
│ • Transfer guest register state into Hydra C runtime.                      │
│ • Execute native/decompiled C functions and handle return types.           │
│ • Ensure clean exit from translated blocks on Hydra-directed jumps.        │
└────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                    Phase 4: Overlays & Edge Cases                          │
├────────────────────────────────────────────────────────────────────────────┤
│ • Validate INT 3Fh dynamic overlay segment remapping.                      │
│ • Calibrate FLAGS comparison masks against simx86 behavior.                │
│ • Verify dynamic code-load offsets (CODE_START_SEG).                       │
└────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                    Phase 5: Benchmark & CI Automation                      │
├────────────────────────────────────────────────────────────────────────────┤
│ • Benchmark normal Hydra hybrid execution vs. DosBox-X baseline.           │
│ • Benchmark strict lockstep validator throughput separately.               │
│ • Configure headless batch execution for Docker / CI pipelines.            │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## Repository Split

- **`dis86` repository (this fork)**: Contains `dis86`-side validator updates, `hydra_process.rs` process management, tooling scripts (`run.py`), and documentation.
- **`dosemu2` repository**: Contains any required `simx86` instrumentation patches, shared memory export, and the Hydra plugin integration.
