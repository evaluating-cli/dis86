# Hydra hosting on dosemu2: external client via dosdebug + /proc/pid/fd

> **Status (2026-08-25): implementation hardened and runtime verified for the supported static-hook scope; Phase 7 real-target enablement (Items A–D) layered on top.**
> The external stock-dosemu2 host implements function-level hooks, independently
> verified lowmem mapping, exact guest-visible IF restoration, persistent
> register+lowmem snapshots, explicit guest-owned raw-code scratch, and
> capture/restore special-mode breakpoints. Overlay hooks are **not supported by
> default** in this backend and are rejected before guest execution instead of being
> silently skipped.
>
> Final behavioral candidate `3beac7989b1a982f1645ee597192d0c8a40d8080`
> passed both host-independent CI and the stock-dosemu2 runtime workflow on
> dosemu2 2.0pre9 / Revision 7076. See `TESTING.md` for the recorded commands,
> package identity, counters, FLAGS samples, and PASS evidence.
>
> Phase 7 additions (this PR): diff-write register pushes (2.8× faster), user
> metadata via `lib=` dlopen, MZ/.exe loading with dynamic load-segment discovery,
> HYDSNAP full-state capture/restore across instances. VROOMM-style overlay
> page-in (Phase 7 Item E) is **deferred to an explicit opt-in follow-up**: the
> default rejection contract above is unchanged, and any future opt-in design
> must preserve it. Integration suite stages in this PR: .com / .exe / capture /
> restore — see `TESTING.md`.
> Implementation: `hydra/src/dosemu_host/`. Where this doc and the shipped code
> differ (raw-code execution is single-step-trace-based, not return-stub-based),
> §6/§8 note the as-built behavior.

**Scope:** design doc for hosting Hydra on the **stock, unmodified** dosemu2 binary.
No rebuild, no patching, no fork, no plugin. Hydra runs as an external process that
drives dosemu2 through its built-in debugger protocol and maps guest memory via procfs.

**Validation authority:** SST (SingleStepTests hardware captures) validates emu86;
dosemu2's role is Hydra hosting only.

## 1. Approach

Hydra needs function-level hooking (intercept execution at specific CS:IP addresses),
register access at hook boundaries, and a raw pointer to guest memory. The stock dosemu2
binary provides the required primitives without modification:

| Hydra need | Mechanism | Stock binary? |
|---|---|---|
| Function-level interception | dosdebug `bp ADDR` (software INT3 breakpoint) | yes — debugger compiled in by default |
| Continue / stop execution | dosdebug `g` / `stop` | yes |
| Register read (on breakpoint hit) | dosdebug `r` / `r0` | yes |
| Register write (return values/state restore) | dosdebug `r REG val` (per-register) | yes |
| Memory read/write (fallback/provenance probes) | dosdebug `d ADDR SIZE` / `m ADDR val` | yes |
| Raw pointer to guest memory (`mem_hostaddr`) | mmap verified dosemu2 lowmem backing via `/proc/<pid>/fd/<fd>` | yes — memfd/shm accessible via procfs |
| I/O port access | Guest opcode execution from an explicitly guest-owned raw-code reservation | yes |

**Key insight:** instruction-level interception is not required for ordinary Hydra
function hooks. dosdebug breakpoints provide the function boundary, while dosdebug
single-step is used only to execute injected raw opcodes/native→guest callthroughs and
to complete those calls.

## 2. Architecture

```
  ┌──────────────────────┐         ┌──────────────────────────┐
  │   Hydra (host proc)  │         │   dosemu2 (stock binary)  │
  │                      │         │                          │
  │  hydra_machine_init  │ FIFOs   │  mhpdbg (debugger plugin) │
  │  ├── dosdebug client │◄───────►│  ├── dbgin.<pid> (cmds)  │
  │  ├── lowmem mmap     │ procfs  │  ├── dbgout.<pid> (resp) │
  │  │   /proc/pid/fd/N  │◄───────►│  └── lowmem backing      │
  │  └── hook dispatch   │         │                          │
  │                      │         │  simx86 (CPU emulator)    │
  │  bp ADDR → hook fire │         │  ├── INT3 trap → mhp_debug│
  │  r/r0 → read regs    │         │  └── stopped → dump regs  │
  │  r REG val → write   │         │                          │
  │  g/t → run/trace     │         │  DOS (FDPP/FreeDOS)      │
  └──────────────────────┘         └──────────────────────────┘
```

Hydra is an external process. It connects to dosemu2's debugger via two FIFOs, maps
guest memory via `/proc/<pid>/fd/`, and drives execution through breakpoint/continue
commands. All Hydra macros (`PTR_*`, `ARG_*`, `LOCAL_*`) work through the mmap'd
pointer; register sync happens at hook/special-mode boundaries via the dosdebug
protocol.

## 3. dosdebug protocol

**Transport:** two POSIX named pipes in `$XDG_RUNTIME_DIR/dosemu2/`, per dosemu2 PID:
`dosemu.dbgin.<pid>` (client writes commands) and `dosemu.dbgout.<pid>` (server writes
responses). Created automatically by the stock binary on every startup
(`mhpdbg.c:167-177`).

**Format:** text/line-oriented. Commands are plain-ASCII lines; responses are raw text.
No framing, no length prefixes, no structured replies. Buffer limit 8192 bytes
(`mhpdbg.h:77`).

**Debugger is available in the stock build used by this work:** `USE_MHPDBG` is
defined by the default debugger plugin configuration and the installed binary used for
the integration work exports the debugger symbols.

**Key commands** (handlers in `src/plugin/debugger/mhpdbgc.c`):

| Command | Effect |
|---|---|
| `r` | dump all registers |
| `r0` | debugger register/state dump used for stop notification/read-back |
| `r REG val` | write one register (AX/BX/CX/DX/SI/DI/BP/SP/IP/CS/DS/ES/SS/FL) |
| `d ADDR SIZE` | read guest memory |
| `m ADDR val...` | write guest memory |
| `bp ADDR` | set software (INT3) breakpoint; debugger table max is 64 |
| `bc n` | clear breakpoint by index |
| `g` | go/continue |
| `stop` | stop if running |
| `t` / `ti` | debugger single-step variants |

**Protocol/host limitations:**
- No interrupt injection command; Hydra executes `INT xx` as guest code in its raw-code reservation.
- No direct I/O-port command; Hydra executes `IN`/`OUT` as guest code.
- Register writes are not atomic; the host keeps the CPU stopped, writes them one at a time, then performs read-back verification before `g`.
- **Command bursts are unreliable** on the tested runtime. The client uses paced per-command round-trips.
- **Response stream can desynchronize by one block** when a step produces extra unsolicited text; `dosdebug_drain()` is used to re-synchronize around tracing.
- **`r0` exposes raw vm86 EFLAGS, not directly Hydra's architectural 16-bit FLAGS.** Physical IF remains forced by dosemu while guest-visible IF is represented in VIF. The client reconstructs bit 9 from VIF before exposing the register set to Hydra.
- **`r FL value` can apply the requested guest IF yet emit a misleading textual failure.** Stock dosemu's immediate command verifier compares against raw/normalized flags and may print `failed to set register 'FL'` when guest IF=1. The client tolerates only that known FL textual false verdict. FIFO/transport failures remain fatal, and `host_set_regs()` verifies the resulting guest-visible FLAGS by architectural readback; IOPL and reserved bit 1 remain host-managed.
- **Traced callees must not do blocking DOS I/O**: a single `t` operation that enters a blocking service can stall. The driver bounds each step (`HOST_STEP_TIMEOUT_MS`, 10 s) and reports failure.

## 4. Memory access: /proc/pid/fd mmap

Hydra's `mem_hostaddr(ctx, addr)→uint8_t*` is structurally required — the `PTR_8/16/32`
macros produce dereferenceable lvalues used throughout decompiled code. A debugger-only
read/write API cannot replace that pointer contract.

The stock dosemu2 `mapmshm` mapping backend can create **multiple** memfds with the same
`dosemu_<pid>` name. Therefore the name and size are not sufficient provenance and the
host must never accept the first matching `/proc/<pid>/fd` entry.

The hardened selection procedure is:

1. Enumerate `/proc/<dosemu_pid>/fd/` entries whose readlink identifies a
   `/memfd:dosemu_<pid>` object.
2. Deduplicate duplicate descriptors that refer to the same underlying inode and reject
   candidates smaller than the required lowmem+HMA window (`0x110000`).
3. Independently read stable guest bytes through dosdebug (currently 16-byte probes at
   physical `0x0000`/IVT and `0x0400`/BDA).
4. Map each candidate read-only for selection and compare those offsets with the
   independent dosdebug probes.
5. Require **exactly one** candidate to match; fail closed on zero or multiple matches.
6. Re-open/map the selected backing read/write `MAP_SHARED` for Hydra memory access.

This converts the memfd path from a naming heuristic into a cross-checked guest-memory
identity test. `/proc/<pid>/fd` remains the transport mechanism; dosdebug supplies the
independent provenance oracle.

## 5. Register sync and persistent state

At a breakpoint hit, dosemu2 is stopped and Hydra reads a full register dump into
`hydra_machine_registers_t`. `update_registers` is a **pull** from dosemu2 into the
Hydra machine state.

After a native hook changes state, the driver pushes the complete register set through
paced `r REG val` commands and verifies the result. `host_set_regs()` then re-applies
exact guest `FL` so IF=0 survives a complete hook round-trip.

`state_save`/`state_restore` are process-persistent, not an in-process register table:

- snapshot file magic/version identify the format;
- the file stores the complete register set plus the `0x110000` real-mode lowmem+HMA
  window;
- capture writes a sibling temporary file, `fsync`s it, then renames atomically;
- restore validates the header, restores memory first, then restores registers;
- the vtable callbacks fail hard if snapshot read/write or register restoration fails, because their interface has no error return and silent continuation would create a false-success capture/restore;
- software INT3 breakpoint patches are cleared before capture/restore and every clear must succeed, so breakpoint bytes are neither serialized nor overwritten underneath dosdebug's breakpoint table;
- restore pulls the restored CPU state back into Hydra and then re-arms static hook
  breakpoints against the restored memory image before guest execution resumes.

Capture and restore are special instruction-boundary modes in the Hydra core. Because
those addresses are not necessarily ordinary registered hooks, the external driver
plants a dedicated capture breakpoint at `capture_addr`, or a restore-entry breakpoint
at `hydra_hook_entry_addr()`, so the special-mode code is reachable.

## 6. I/O and raw-code execution (as built)

Hydra implements I/O and several machine helpers by writing real 8086 instructions into
a guest-owned scratch reservation and executing them through the same CALL/CALL_NEAR
trace machinery used for native→guest callthrough.

**There is no implicit scratch address.** The host initializes
`raw_code_offset=0/raw_code_size=0`; after the target is loaded, the guest/launcher must
identify memory it owns and call `host_reserve_raw_code(ctx, addr, size)`. The
reservation must be at least 128 bytes, paragraph aligned, entirely within mapped guest
memory, and addressable relative to the configured code load segment. `host_run()`
refuses to release the guest unless a reservation is present. The integration fixture
reserves an aligned 8 KiB region embedded in its own COM image and checks it is restored
byte-for-byte.

## Implementation phases (as built)

- **Phase 0 — empirical verification:** launch stock dosemu2 headless with simx86
  (`$_cpu_vm = "emulated"`, `$_cpuemu = (1)`); connect dosdebug FIFO; find and mmap
  lowmem via `/proc/pid/fd`; set a breakpoint and verify it fires. **Done** — also
  established that `$_mapping = "mapmshm"` (memfd lowmem) and `$_hdimage = "+1"`
  (FreeDOS boot off a bare working dir) are required.
- **Phase 1 — dosdebug client:** C client in `hydra/src/dosemu_host/dosdebug.c`:
  FIFO connection, PID discovery, command encoding, response parsing, register
  read/write, breakpoints, go/stop, single-step. **Commit `5a575ec`.**
- **Phase 2 — lowmem mmap bridge:** `lowmem.c` finds and mmaps the lowmem
  backing via `/proc/<pid>/fd/`. Provides `mem_hostaddr`, `mem_read8/16`,
  `mem_write8/16`. **Commit `5a575ec`.**
- **Phase 3 — Hydra bridge:** full `hydra_machine_hardware_t` vtable (14
  callbacks) in `host.c`: `mem_hostaddr` from mmap, `update_registers` /
  `state_save` / `state_restore` via dosdebug, emulated `DOS_REAL` hardware
  context for test mode, I/O via guest opcode execution. **Commit `c188fb7`.**
- **Phase 4 — function-level hooking:** `host_driver.c` run loop — plant INT3 at
  every registered hook (cleared on exit), dispatch through
  `hydra_machine_exec` + `hydra_machine_notify`, trace-based raw-code execution
  per §6. **Commit `079d1fc`** (+ review fixes `a8af5f2`).
- **Phase 5 — integration testing:** `test_driver.c` + `run_driver_test.sh`:
  26-byte NASM COM guest loops 5× calling a hooked function
  (CLI/STI/INT/INB/OUTB/ret); hook returns 0xBE00 in AX. Result: 5 hook
  dispatches, 25 raw-code runs, all returned via trace, guest observed the
  result — **PASSED. Commit `079d1fc`.**
- **Phase 6 — real-workload hardening:** trace-until-magic-return call
  completion, nested hooks mid-trace, loud bp failures, SS:SP strictness,
  per-step timeout clamp, FL non-forced-bit verify. **Commits `e6981bf`,
  `a2053a2`.**
- **Phase 7 — real-target enablement (all complete):**
  - *A: write-path performance* (`5547669`) — diff-writes skip registers
    unchanged since the last verified CPU dump; FL always written; r0 verify
    kept. Suite 2m24s → 51s.
  - *B: user metadata* (`a0c299e`) — conf key `lib=/path/user.so`; dlopen by
    handle (RTLD_DEFAULT would hit the host's own stubs); injection setters
    `hydra_function_metadata_set` / `hydra_callstack_metadata_set`; enables
    name-based registration on this host.
  - *C: MZ loading* (`a0c299e`) — launcher-driven load (`launch.com` EXECs the
    guest AH=4B01 and parks; dosemu's own bpload/DBGload cannot be used
    against stock fdpp because the shell boot consumes it), entry validation
    (MCB self-ownership, PSP `CD 20`, image-vs-file bytes, entry CS:IP),
    dynamic `code_load_offset = PSP+0x10`. Hand-rolled MZ header in NASM
    (no linker); assembled `org 0` so `[data-HDR_SIZE]` operands are module
    offsets.
  - *D: capture/restore* (`5ddc34e`) — HYDSNAP file (64-B header + full lowmem
    guest window + CRC32); restore into a fresh instance: blob memcpy BEFORE
    full paced register push, offsets adopted from snapshot;
    `restore|<path>|<seg:off>` un-hardcodes the restore entry (navigator
    literal remains fallback); dispatch_hook handles RESTORE-applied returns
    without clobbering restored regs. `lowmem_guest_size()` added — the memfd
    mapping is far larger than guest memory.
  - *E: overlays* (`66c7cc6`) — see §10.

Raw-code JIT discipline:

- each snippet uses a fresh 128-byte slot inside the reservation;
- external memfd writes do not invalidate simx86 translations, so a guest linear slot
  address is **never reused during the host process**;
- `hydra_impl_raw_code_reset()` remains only for source compatibility and is a no-op on
  this backend;
- exhaustion is a hard failure rather than wrapping to a potentially stale translation;
- the original 128 bytes are restored after the guest call returns.

Execution is single-step-trace-based:

- after Hydra redirects CS:IP to the call target, the driver pushes the redirect into
  dosemu2 and issues `t` until RET/RETF reaches Hydra's magic return address;
- native→guest calls use the same trace path with a default 10,000-step budget;
- a traced guest call that hits another registered static hook is dispatched recursively
  (nested-hook depth cap 8);
- redirect count is capped at 256 per dispatch;
- no return-stub breakpoint is planted at `ffff:0000`.

## 7. Hook/breakpoint policy

The stock debugger has a 64-breakpoint table. The external host therefore treats the
limit as a hard resource boundary: if the number of static hooks exceeds 64, or any
required breakpoint cannot be installed, `host_run()` fails before guest execution.
Capture/restore special-mode breakpoints also consume debugger entries when active.

**Overlay hooks are currently unsupported by the external dosdebug backend.** A logical
overlay hook cannot be translated to a stable physical breakpoint before the overlay
mapping exists. Silently omitting those hooks is semantically wrong, so
`HYDRA_HOOK_FLAGS_OVERLAY` causes breakpoint installation to fail and `host_run()`
refuses to run. `test_overlay_reject` makes that default-mode behavior a regression
contract. Dynamic/lazy overlay discovery and arming remains follow-up work and is not
claimed by PR #34; any future implementation must be explicitly opted in if it changes
this default behavior.

## 8. Hydra bridge mapping

| Hydra op | Hardened implementation |
|---|---|
| `mem_hostaddr` | unique verified lowmem memfd mapping + guest address |
| `mem_read8/16`, `mem_write8/16` | direct access through the verified shared mapping |
| `io_in8/16`, `io_out8/16` | guest opcode execution via explicit raw-code reservation + trace |
| `update_registers` | **pull** dosdebug register dump into Hydra machine registers |
| complete register push | paced dosdebug writes + VIF-aware read-back + exact guest-FL verification |
| `state_save` | atomic file containing registers + `0x110000` guest lowmem/HMA; failure is fatal |
| `state_restore` | restore memory, restore CPU registers, pull CPU state, re-arm breakpoints; failure is fatal |
| `hydra_machine_init` | connect dosdebug + independently verify/select lowmem backing; raw scratch remains unconfigured |
| hook execution | static INT3 breakpoints + `g`; trace only for raw/native→guest calls |
| capture/restore execution | explicit special-mode breakpoints with checked breakpoint cleanup |
| overlay hooks | unsupported by default; fail closed before guest execution |

## 9. Phased implementation and verification state

- **Phase 0 — empirical verification:** stock dosemu2 debugger + procfs memory transport established.
- **Phase 1 — dosdebug client:** FIFO connection, command pacing, parsing, register access, breakpoints, run/stop/step.
- **Phase 2 — lowmem mmap bridge:** candidate enumeration plus independent dosdebug probe verification; ambiguous backing selection fails closed.
- **Phase 3 — Hydra bridge:** vtable wiring, VIF-aware guest-visible register restoration, and persistent register+lowmem snapshot files.
- **Phase 4 — function-level hooking:** static INT3 hook breakpoints, trace-based raw/native→guest execution, nested dispatch, strict 64-breakpoint failure handling, and explicit rejection of overlays by default.
- **Phase 5 — integration fixture:** three static hooks plus native→guest callthrough/nested hook, CF and IF=0 checks, and an 8 KiB guest-owned raw-code reservation. The verified five-iteration fixture produced 21 hook dispatches, 45 raw/guest-call runs, and 45 returns.
- **Phase 6 — review hardening:** exact lowmem provenance, explicit scratch ownership/no slot reuse, persistent capture/restore reachability and breakpoint safety, fail-hard snapshot callbacks, checked breakpoint cleanup, and fail-closed unsupported/resource cases.

### Merge verification gate

**PASS for behavioral candidate `3beac7989b1a982f1645ee597192d0c8a40d8080`.**

The candidate was tested on stock dosemu2 2.0pre9 / Revision 7076 and passed all four
required gates:

1. `test_host` — verified-lowmem selection, guest IF=0 register round-trip, persistent snapshot register+memory restore, and breakpoint smoke.
2. `run_driver_test.sh` — three-hook fixture, nested callthrough, CF/IF=0 preservation, `hook_dispatches=21`, `raw_code_runs=45`, `raw_code_returns=45`, `redirects=66`, and byte-for-byte restoration of the guest-owned 8 KiB scratch reservation.
3. `test_overlay_reject` — a genuine overlay-typed hook fails before guest execution in default mode.
4. Host-independent CI — Rust tests plus syntax coverage for top-level Hydra and all `hydra/src/dosemu_host/*.c` implementation/test sources.

The exact commands, runtime package identity, workflow/run IDs, and observed FLAGS
samples are recorded in `TESTING.md`. Documentation-only commits after the behavioral
candidate still have to clear both workflows on their final PR head before merge; that
final-head result is recorded in the PR discussion.

## 10. Reference: prior in-process plugin research (appendix)

A prior design phase investigated an in-process `src/plugin/hydra/` plugin with a
minimal simx86 core callback (function-pointer registration slot in `FindExecCode`).
That approach required a dosemu2 fork and a small core edit. The current external client
approach supersedes it for the supported static-hook scope — no dosemu2 rebuild is
needed.

**simx86** is dosemu2's software CPU emulator (`src/base/emu-i386/simx86/`). It
translates x86 instructions into internal nodes and executes them. The debugger's
single-step path supplies the instruction-boundary behavior used only during Hydra's
trace operations; there is no generic stock simx86 observer registration API.

The dosdebug protocol was traced from `src/plugin/debugger/mhpdbg.c` (transport +
dispatch) and `mhpdbgc.c` (command handlers). The dosdebug client is `dosdebug.c`. All
three are compiled into the stock binary. Transport is FIFOs in
`$XDG_RUNTIME_DIR/dosemu2/`.

The dosemu2 low-memory backing is `uint8_t *lowmem_base` (`mapping.c:83`, declared
`memory.h:231`), covering `LOWMEM_SIZE` (1MB) + `HMASIZE` (64KB). Stock mapping drivers
(`mapfile.c`) create the backing as a `memfd_create` or `shm_open`+`shm_unlink` —
anonymous, but accessible via `/proc/<pid>/fd/`.

The frozen patches (`patches/dosemu2/0001`, `0002`) are the reference implementation of
the boundary hook and low-memory-backing query. They are not applied. The ABI-v1
contract they implement is described in `FREEZE_ABI_V1.md` (reference-only).
