# Hydra hosting on dosemu2: external client via dosdebug + /proc/pid/fd

> **Status (2026-08-25): implementation hardened; exact-head runtime verification pending.**
> The external stock-dosemu2 host implements function-level hooks, independently
> verified lowmem mapping, exact guest-visible IF restoration, persistent
> register+lowmem snapshots, explicit guest-owned raw-code scratch, and
> capture/restore special-mode breakpoints. Overlay hooks are **not supported** by
> this backend and are rejected before guest execution instead of being silently
> skipped.
>
> The branch records a real-dosemu integration verification before the final
> raw-slot and capture/restore hardening (`d4cf069`, "verify guest-owned scratch
> and guest IF in real host test"). Later commits changed raw-code slot lifetime
> and special-mode breakpoint handling, so that older pass is **not** accepted as
> proof for the current head. A fresh `run_driver_test.sh` plus host snapshot test
> on the exact merge candidate is a merge condition; see `TESTING.md`.
>
> Implementation: `hydra/src/dosemu_host/`.

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
- **`r FL` can report a misleading failure** because dosemu normalizes physical EFLAGS/IOPL. The low-level helper therefore cannot use the command verdict alone.
- **Guest IF is not a blind spot in the hardened host.** dosemu keeps physical vm86 IF managed by the emulator, but `set_FLAGS()` records the requested guest IF through VIF/`set_IF()`/`clear_IF()`, and `get_FLAGS()` exposes that guest-visible state. `host_set_regs()` re-applies the caller's unmodified `FL` value after the compatibility write and verifies all guest-visible bits; IOPL and reserved bit 1 remain host-managed. The real-host tests include an IF=0 round-trip.
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
- software INT3 breakpoint patches are cleared before capture/restore so they are not
  serialized or overwritten underneath dosdebug's breakpoint table;
- restore pulls the restored CPU state back into Hydra and then re-arms static hook
  breakpoints against the restored memory image.

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
refuses to run. Dynamic overlay discovery/re-arming is future work and is not claimed by
this PR.

## 8. Hydra bridge mapping

| Hydra op | Hardened implementation |
|---|---|
| `mem_hostaddr` | unique verified lowmem memfd mapping + guest address |
| `mem_read8/16`, `mem_write8/16` | direct access through the verified shared mapping |
| `io_in8/16`, `io_out8/16` | guest opcode execution via explicit raw-code reservation + trace |
| `update_registers` | **pull** dosdebug register dump into Hydra machine registers |
| complete register push | paced dosdebug writes + read-back + exact guest-FL reapply |
| `state_save` | atomic file containing registers + `0x110000` guest lowmem/HMA |
| `state_restore` | restore memory, restore CPU registers, pull CPU state, re-arm breakpoints |
| `hydra_machine_init` | connect dosdebug + independently verify/select lowmem backing; raw scratch remains unconfigured |
| hook execution | static INT3 breakpoints + `g`; trace only for raw/native→guest calls |
| capture/restore execution | explicit special-mode breakpoints |
| overlay hooks | unsupported; fail closed before guest execution |

## 9. Phased implementation and verification state

- **Phase 0 — empirical verification:** stock dosemu2 debugger + procfs memory transport established.
- **Phase 1 — dosdebug client:** FIFO connection, command pacing, parsing, register access, breakpoints, run/stop/step.
- **Phase 2 — lowmem mmap bridge:** candidate enumeration plus independent dosdebug probe verification; ambiguous backing selection fails closed.
- **Phase 3 — Hydra bridge:** vtable wiring, exact guest-visible register restoration, and persistent register+lowmem snapshot files.
- **Phase 4 — function-level hooking:** static INT3 hook breakpoints, trace-based raw/native→guest execution, nested dispatch, strict 64-breakpoint failure handling, and explicit rejection of overlays.
- **Phase 5 — integration fixture:** three static hooks plus native→guest callthrough/nested hook, CF and IF=0 checks, and an 8 KiB guest-owned raw-code reservation. The current test code expects 21 hook dispatches and 45 raw/guest-call executions for the five-iteration fixture.
- **Phase 6 — review hardening:** exact lowmem provenance, explicit scratch ownership/no slot reuse, persistent capture/restore reachability and breakpoint safety, and fail-closed unsupported/resource cases.

### Merge verification gate

The implementation is not considered merge-verified solely because an older branch
state passed the real-dosemu fixture. Before merge, run the real-dosemu tests on the
**exact candidate head** and record the commit SHA, commands, and complete result in
`TESTING.md` and the PR discussion. Required evidence:

1. `test_host` passes verified-lowmem selection, guest IF=0 register round-trip,
   persistent snapshot register+memory restore, and breakpoint smoke checks.
2. `run_driver_test.sh` passes the current three-hook fixture, including nested
   callthrough, IF=0 preservation, 21 expected hook dispatches, 45 raw/guest-call runs,
   and byte-for-byte restoration of the guest-owned 8 KiB scratch reservation.
3. A negative overlay fixture or host-independent test demonstrates that registering an
   overlay hook fails before guest execution; this backend must not claim overlay support.
4. Host-independent CI compiles the `hydra/src/dosemu_host/*.c` implementation, not only
   the top-level Hydra C files.

Until those exact-head results are recorded, the runtime verification gate is
**PENDING**.

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

The dosdebug protocol was traced from the stock debugger sources. The dosemu2
low-memory backing is created by its mapping layer as memfd/shm objects and exposed to
this external host only through procfs. The frozen patches under `patches/dosemu2/`
remain reference material for the retired validator and are not applied by Option D.
