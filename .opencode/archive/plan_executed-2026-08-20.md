# Plan: Hydra Hosting on dosemu2

Archived: `.opencode/archive/plan_executed-2026-08-17.md` (Tracks 1-3 complete; SST is
the validation authority, dosemu2 differential validator retired).

## Conventions

- One approved task at a time; subagents execute/test; supervisor verifies; interview when <95% certain.
- Gates: cargo test --locked --all-targets fully green, emu86_sst audit --probe = 0 PROBE-MISMATCH, docs/ledger/manifest consistency. No push.
- dosemu2 patches are kept as reference only (not applied).

## Current state (2026-08-20)

- **SST is the validation authority.** Full V1 sweep: PASS=1,050,652 FAIL=0 DECODE_ERR=0 PANIC=0 across 268 forms. cargo test --locked --all-targets 277 passed, 0 failed. audit --probe 0.
- **Hydra hosting on dosemu2: COMPLETE.** External Hydra client on the **stock, unmodified** dosemu2 binary: dosdebug FIFO protocol for control (breakpoints, register sync, single-step) + `/proc/<pid>/fd/` mmap for raw guest memory pointer. No rebuild, no patching, no fork, no plugin. Design doc: `docs/dosemu2/OPTION_D_DESIGN.md`.
- **Phase 4/5 integration test PASSED:** 5 hook dispatches × 5 raw-code runs (CLI, STI, INT, INB, OUTB) = 25 raw-code executions, all returned via single-step trace (RET/RETF lands at the return address; the `stub_hits` stat name is historical — no stub breakpoint is planted). Guest observed hook result (0xBE00). Commits `079d1fc`, review fixes `a8af5f2`.
- **Review cleanup (2026-08-20, `a8af5f2`):** `host_run` clears all breakpoints it planted at exit (was leaking dosdebug bp indices); test no longer pre-plants (driver owns the lifecycle); `hydra_impl_raw_code_reset()` resets the 64KB raw-code slot region per hook boundary (was aborting after 512 executions); stale stub-breakpoint comments updated. Low-severity items noted, not triggered: `host_run_install_hook_breakpoints` never returns -1 (doc mismatch); CALL_NEAR 6-step trace limit would fail for real guest far/near function calls.

## Approach

Hydra needs function-level hooking (breakpoints at function entry points), register
access at hook boundaries, and a raw pointer to guest memory. The stock dosemu2 binary
provides all three:

- **dosdebug protocol** (compiled in by default): `bp ADDR` (INT3 breakpoints), `r`/`r REG val` (register read/write), `g`/`stop` (execution control), `d`/`m` (memory read/write). Transport: two FIFOs in `$XDG_RUNTIME_DIR/dosemu2/`.
- **`/proc/<pid>/fd/` mmap**: dosemu2's lowmem backing (memfd, ~1MB+64KB) is accessible from another process via procfs. Verified empirically: `open("/proc/<pid>/fd/<fd>")` + `mmap(MAP_SHARED)` gives bidirectional access. Provides `mem_hostaddr` for all Hydra macros (`PTR_*`, `ARG_*`, `LOCAL_*`).
- **Guest opcode execution**: I/O is handled by Hydra writing real `IN`/`OUT` opcodes into guest code (`hydra_impl_raw_code`), then continuing dosemu2 — no dosdebug I/O port access needed.

Key insight: instruction-level hooking is not needed for a decompilation tool. Function-
level hooking via breakpoints is the right granularity.

## Phased implementation

- [x] Phase 0 — empirical verification: launch stock dosemu2 headless with simx86; connect dosdebug FIFO; find and mmap lowmem via `/proc/pid/fd`; set a breakpoint and verify it fires.
- [x] Phase 1 — dosdebug client: implement a dosdebug protocol client (FIFO connection, PID discovery, command encoding, response parsing, register read/write, breakpoints, continue/stop). Commit `5a575ec`.
- [x] Phase 2 — lowmem mmap bridge: find and mmap dosemu2's lowmem backing via `/proc/<pid>/fd/`; provide `mem_hostaddr`, `mem_read8/16`, `mem_write8/16`. Commit `5a575ec`.
- [x] Phase 3 — Hydra bridge: wire the full `hydra_machine_hardware_t` vtable (`mem_hostaddr` from mmap, `update_registers`/`state_save`/`state_restore` from dosdebug, `hydra_machine_init`/`exec`, I/O via guest opcode execution). Commit `c188fb7`.
- [x] Phase 4 — function-level hooking: breakpoint management, hook dispatch (on hit: read regs → execute native → write regs → continue), `hydra_impl_raw_code` integration. Trace-based raw-code execution via dosdebug single-step (`t`). Commit `079d1fc`.
- [x] Phase 5 — integration testing: end-to-end with a target DOS program. 5 hooks, 25 raw-code runs, all returned. Commit `079d1fc`.

## Verification

- Per-track closeout gates: cargo test --locked --all-targets fully green, emu86_sst audit --probe = 0 PROBE-MISMATCH, docs/ledger/manifest consistency.

## Reference: prior in-process plugin approach (superseded)

A prior design phase investigated an in-process `src/plugin/hydra/` plugin with a
minimal simx86 core callback (function-pointer registration slot in `FindExecCode`).
That approach required a dosemu2 fork and a ~17-line core edit. The current external
client approach supersedes it — no rebuild is needed. The prior research findings
(dosemu2 plugin architecture, debugger hook trace, simx86 internals, bridge mapping)
are preserved in `docs/dosemu2/OPTION_D_DESIGN.md` §9 (appendix).

## Changelog

- 2026-08-20: **PROJECT COMPLETE — plan archived.** Code-review fixes in `a8af5f2`;
  docs updated (`docs/dosemu2/`): `OPTION_D_DESIGN.md`, `README.md`, `TESTING.md`
  reflect the shipped trace-based implementation; PHASE1_*/REVIEW/SOURCES banners
  mark the superseded plugin approach. Plan moved to
  `.opencode/archive/plan_executed-2026-08-20.md`.
- 2026-08-18: **Phase 4/5 COMPLETE.** Function-level hooking via dosdebug trace.
  External driver loop: plant INT3 at hook addresses, dispatch hooks through Hydra
  exec engine, trace raw-code slots via single-step (`t`) to detect returns. No
  breakpoint planting at ROM addresses; no JIT cache staleness (monotonically
  increasing raw-code slots). Integration test: 5 hooks, 25 raw-code runs, 25 stub
  returns — ALL PASSED. Commit `079d1fc`.
- 2026-08-17: **Hydra hosting approach finalized.** External Hydra client on stock
  dosemu2: dosdebug FIFO protocol + `/proc/<pid>/fd/` mmap. No rebuild, no patching,
  no fork. Function-level hooking (breakpoints) replaces instruction-level hooking
  (not needed for a decompilation tool). Prior in-process plugin approach superseded.
  Doc cleanup: `OPTION_D_DESIGN.md` rewritten, `FREEZE_ABI_V1.md` trimmed to brief
  reference (freeze gate removed), `README.md` reframed. `/proc/pid/fd` mmap verified
  empirically. Commits: see git log.
- 2026-08-17: Track 4 Option D research phase complete (prior approach). Design doc
  `docs/dosemu2/OPTION_D_DESIGN.md`: crux gate decision (zero core edits = NO),
  plugin-architecture survey, boundary-hook design, hydra bridge mapping, local-fork
  logistics, Phases 0-5 sketch. Research via 4 parallel subagents over a shallow
  dosemu2 clone. cargo test 277/0.
- 2026-08-17: Reassessment — SST is the validation authority; dosemu2 = hosting only.
  Tracks 1-3 complete; Track 3 R2/R3 + Option C dropped. Commits: 8bbc166, d7f1afe,
  4bdb7de, f568371.
