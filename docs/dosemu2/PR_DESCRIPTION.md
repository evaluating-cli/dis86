# Pull Request: Architecture, Review Findings, and Phase 0 Gate Specification for `dosemu2` Migration

## Overview

This pull request introduces the architectural design, source review findings, and implementation plan for replacing the patched DosBox-X execution backend used by [Hydra](https://github.com/xorvoid/hydra) and the `emu86` differential validator with [dosemu2](https://github.com/dosemu2/dosemu2).

---

## Motivation

Hydra currently uses a custom-patched fork of DosBox-X (`xorvoid/dosbox-x`) for running hybrid x86-16 / native C applications and powering lockstep differential validation in `dis86`.

Migrating to `dosemu2` offers significant long-term benefits:
1. **Higher CPU Throughput**: `simx86` JIT / dynarec execution for hybrid runtime workloads.
2. **Modular Architecture**: Leveraging `dosemu2`'s native plugin framework (`src/plugin/`) to avoid maintaining an intrusive emulator fork.
3. **Headless Continuous Integration**: Support for batch, headless execution (`dosemu -dumb -quiet -K <dir> -E <exe>`) without SDL/X11 dependencies.
4. **Authentic DOS Environment**: Support for real DOS kernels, FreeDOS, 64-bit FDPP, and standard DPMI extenders.

---

## Key Architectural Hypotheses & Design Discipline

Following technical review, several initial assumptions have been refined into testable hypotheses for the migration:

1. **Integration Model (Likely Hybrid Integration)**: A pure out-of-band plugin is unlikely to suffice because `simx86` translates multi-instruction blocks. Hydra requires instruction-boundary control and arbitrary `CS:IP` interception. Phase 0 will determine whether the minimal mechanism is a core callback, single-instruction block mode, or translated instrumentation.
2. **Workload Separation in Benchmarking**: JIT block execution may provide substantial performance gains for *normal Hydra hybrid execution*, but strict *1-instruction lockstep validation* is dominated by IPC and synchronization. Performance metrics for these two workloads must be measured independently.
3. **Low-Memory Model**: The migration will investigate exporting `dosemu2`'s existing `lowmem_base` shared memory backing to the validator before considering redundant independent allocations, taking into account semantic differences between `lowmem_base` and `MEM_BASE32`.
4. **REP Instruction Stepping Contract**: Phase 0 will explicitly define and test whether `REP MOVS`/`STOS` instructions should be observed as single atomic steps or individual iterations by the validator.
5. **JIT State Invalidation on Control Redirection**: When Hydra alters `CS:IP` or registers (`RETURN_FAR`, `RETURN_JUMP`), `simx86` must safely escape the current translated block, invalidate stale cached translations, and resynchronize segment state before resuming execution.

---

## Phase 0: Minimal Simulator Gate

Before building the full shared-memory transport or Hydra ABI, the migration is gated on a minimal standalone experiment verifying:

- [x] Execution halts after exactly **one** guest instruction.
- [x] Complete architectural state (`AX..FLAGS`, `CS:IP`, segments) is readable without side effects.
- [x] `CS:IP` and register state can be modified externally.
- [x] Execution resumes at the new target without executing stale translated blocks.
- [x] State manipulation is validated against a known-good execution path in `emu86` without violating x86 semantics.
- [x] Deterministic behavior for `CALL`, `RET`, `RETF`, `INT`, prefixes, and `REP`.

---

## Phased Implementation Roadmap

- **Phase 0:** Minimal `simx86` instruction-control gate and semantic validation.
- **Phase 1:** Register snapshot ABI and `/dev/shm/hydra_remote` validator transport.
- **Phase 2:** Low-memory backing export (`lowmem_base`) to the validator.
- **Phase 3:** Native `dosemu2` Hydra plugin (`src/plugin/hydra`) and hybrid function execution.
- **Phase 4:** `INT 3Fh` overlay resolution, flags calibration, and dynamic load segment support.
- **Phase 5:** Benchmark validation workloads and configure headless CI workflows.

---

## Changes in this Pull Request

* `docs/dosemu2/README.md`: Updated with disciplined architectural hypotheses and the 6-phase implementation roadmap.
* `docs/dosemu2/REVIEW.md`: Detailed review findings addressing plugin boundaries, workload separation, memory semantics, and JIT cache invalidation.
* `docs/dosemu2/SOURCES.md`: Source references and touchpoints across `dis86`, `hydra`, `dosbox-x`, and `dosemu2`.
* `docs/dosemu2/PHASE0_GATE.md`: Technical specification, test vectors, and success criteria for the Phase 0 simulator gate.
* `docs/dosemu2/PR_DESCRIPTION.md`: Pull request proposal and migration overview.
