# Hydra hosting on dosemu2: external client via dosdebug + /proc/pid/fd

> **Status (2026-08-20): SHIPPED + hardened (Phase 6).** Phases 0–5 complete;
> Phase 6 (real-workload hardening) complete: native→guest call completion
> (trace until magic return, no step cap), nested hook dispatch mid-trace,
> loud breakpoint-install failures, run-until-stop_fn mode, SS:SP/flags
> strictness with r0 read-back verify, paced command writes. Integration test
> passes: 17 hook dispatches (incl. nested), 41 traced executions, flags
> preserved — see `TESTING.md`.
> Commits: `5a575ec` (dosdebug client + lowmem mmap), `c188fb7` (Hydra vtable
> bridge), `079d1fc` (Phase 4/5 hooking), `a8af5f2` (review fixes), Phase 6
> commit (see git log).
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
binary provides all three without modification:

| Hydra need | Mechanism | Stock binary? |
|---|---|---|
| Function-level interception | dosdebug `bp ADDR` (software INT3 breakpoint) | yes — debugger compiled in by default |
| Continue / stop execution | dosdebug `g` / `stop` | yes |
| Register read (on breakpoint hit) | dosdebug `r` (full register dump) | yes |
| Register write (return values) | dosdebug `r REG val` (per-register) | yes |
| Memory read/write (fallback) | dosdebug `d ADDR SIZE` / `m ADDR val` | yes |
| Raw pointer to guest memory (`mem_hostaddr`) | mmap dosemu2's lowmem backing via `/proc/<pid>/fd/<fd>` | yes — memfd/shm accessible via procfs |
| I/O port access | Guest opcode execution (Hydra writes `IN`/`OUT` into a raw-code slot, driver single-steps `t` until RET) | yes |

**Key insight:** instruction-level hooking is not needed for a decompilation tool.
Function-level hooking via breakpoints is the right granularity, and the dosdebug
protocol provides it on the stock binary.

## 2. Architecture

```
  ┌──────────────────────┐         ┌──────────────────────────┐
  │   Hydra (host proc)  │         │   dosemu2 (stock binary)  │
  │                      │         │                          │
  │  hydra_machine_init  │ FIFOs   │  mhpdbg (debugger plugin) │
  │  ├── dosdebug client │◄───────►│  ├── dbgin.<pid> (cmds)  │
  │  ├── lowmem mmap     │ procfs  │  ├── dbgout.<pid> (resp) │
  │  │   /proc/pid/fd/N  │◄───────►│  └── lowmem_base (memfd) │
  │  └── hook dispatch   │         │                          │
  │                      │         │  simx86 (CPU emulator)    │
  │  bp ADDR → hook fire │         │  ├── INT3 trap → mhp_debug│
  │  r → read regs       │         │  └── stopped → dump regs  │
  │  r REG val → write   │         │                          │
  │  g → continue        │         │  DOS (FDPP/FreeDOS)      │
  └──────────────────────┘         └──────────────────────────┘
```

Hydra is an external process. It connects to dosemu2's debugger via two FIFOs, maps
guest memory via `/proc/<pid>/fd/`, and drives execution through breakpoint/continue
commands. All Hydra macros (`PTR_*`, `ARG_*`, `LOCAL_*`) work through the mmap'd
pointer; register sync happens at hook boundaries via the dosdebug protocol.

## 3. dosdebug protocol

**Transport:** two POSIX named pipes in `$XDG_RUNTIME_DIR/dosemu2/`, per dosemu2 PID:
`dosemu.dbgin.<pid>` (client writes commands) and `dosemu.dbgout.<pid>` (server writes
responses). Created automatically by the stock binary on every startup
(`mhpdbg.c:167-177`).

**Format:** text/line-oriented. Commands are plain-ASCII lines; responses are raw text.
No framing, no length prefixes, no structured replies. Buffer limit 8192 bytes
(`mhpdbg.h:77`).

**Debugger is always on:** `USE_MHPDBG` is defined by default
(`src/plugin/debugger/config/plugin_config.h:1`), `debugger` is in the default
`plugin_list` (`plugin_list:23`). Verified on the installed stock binary: `nm -D
/usr/lib/dosemu/libdosemu2.so` exports all `mhp_*` symbols.

**Key commands** (handlers in `src/plugin/debugger/mhpdbgc.c`):

| Command | Effect | Handler |
|---|---|---|
| `r` | dump all registers | `mhpdbgc.c:2297` |
| `r0` | internal: reg dump + state (stop notification) | `mhpdbgc.c:2389` |
| `r REG val` | write one register (AX/BX/CX/DX/SI/DI/BP/SP/IP/CS/DS/ES/SS/FL) | `mhpdbgc.c:2303` |
| `d ADDR SIZE` | hexdump memory (max 256 bytes) | `mhpdbgc.c:903` |
| `m ADDR val...` | write bytes/words/dwords to memory | `mhpdbgc.c:1792` |
| `bp ADDR` | set software (INT3) breakpoint (max 64) | `mhpdbgc.c:2022` |
| `bc n` | clear breakpoint by index | `mhpdbgc.c:2089` |
| `bpint xx` | break on INT xx | `mhpdbgc.c:2111` |
| `g` | go/continue | `mhpdbgc.c:736` |
| `stop` | stop (if running) | `mhpdbgc.c:758` |
| `t` / `ti` | single-step (over / into interrupt) | `mhpdbgc.c:813` |

**Limitations:**
- No interrupt injection command (must write `CD xx` into memory + step).
- No I/O port access (not needed — Hydra uses guest opcode execution).
- No atomic state snapshot (registers read/written one at a time).
- No structured response format (client must parse text).
- One FIFO round-trip per breakpoint hit or single-step.
- **Command bursts are unreliable** (Phase 6, verified via raw stream logs):
  bursts of set-register commands get some commands silently dropped, and at
  high cadence the CPU occasionally executes while the debugger reports
  "stopped". Pace commands (per-command round-trips) and verify with `r0`
  read-back.
- **Response stream can desync by one block** when a step produces extra
  unsolicited text; drain before/after steps.
- **`r FL` false-fails** even on success (full-EFLAGS verify after forcing
  IF/IOPL/bit-1); always verify flags via read-back.

## 4. Memory access: /proc/pid/fd mmap

Hydra's `mem_hostaddr(ctx, addr)→uint8_t*` is structurally required — the `PTR_8/16/32`
macros (`machine.h:97-99`) produce dereferenceable lvalues used throughout decompiled
code (`ARG_16(off) = x;`). A read/write-only host cannot work.

The stock dosemu2 binary creates a memfd (or POSIX SHM) for low-memory backing but does
not expose it as a named SHM object:
- Default driver `mapmshm`: `memfd_create("dosemu_<pid>")` (`mapfile.c:184`) — anonymous.
- `mapshm` driver: `shm_open("/dosemu_<pid>")` then immediate `shm_unlink`
  (`mapfile.c:146-151`) — name removed after creation.

**Linux procfs workaround:** another process can open the memfd via
`/proc/<dosemu_pid>/fd/<fd>` and mmap it `MAP_SHARED`. Verified empirically: a child
process can `open("/proc/<ppid>/fd/<fd>")`, `mmap(MAP_SHARED)`, and both processes see
bidirectional changes. This provides `mem_hostaddr` = `mmap'd_base + guest_addr` for
`addr < LOWMEM_SIZE + HMASIZE` (1MB + 64KB).

**Finding the fd:** scan `/proc/<dosemu_pid>/fd/` and identify the lowmem backing by
size (`fstat` → `st_size >= LOWMEM_SIZE + HMASIZE`), or parse `/proc/<dosemu_pid>/maps`
for the low-memory mapping and trace its backing fd. The memfd appears as
`/memfd:dosemu_<pid> (deleted)` in `/proc/<pid>/fd/` readlinks.

## 5. Register sync

At a breakpoint hit, dosemu2 stops and the dosdebug server pushes a register dump
(via `r0`, `mhpdbgc.c:2389`). Hydra parses this to populate
`hydra_machine_registers_t` (ax/bx/cx/dx/si/di/bp/sp/ip/cs/ds/es/ss/flags, all uint16).

After executing a native hook function, Hydra writes return registers via individual
`r REG val` commands (`mhpdbgc.c:2303`), then sends `g` to continue. Up to 14
register writes per hook return (one per register); all safe because the CPU only
resumes on `g`.

## 6. I/O and raw-code execution (as shipped)

Hydra implements I/O by writing real 8086 `IN`/`OUT` opcodes into a guest code slot
(`hydra_impl_raw_code`, `machine.c`). With the mmap'd lowmem pointer, the slot
write/restore works directly — no dosdebug I/O port access needed.

**As shipped, execution is single-step-trace-based, not `g`-based:**

- Each raw-code snippet occupies a fresh 128-byte slot in a 64KB region below
  0x10000 (`raw_code_offset`, default 0x1c00), monotonically allocated within one
  hook dispatch. This dodges the simx86 JIT cache: the JIT never re-reads
  externally-written bytes, so re-running a slot with new bytes would execute a
  stale translation. The driver calls `hydra_impl_raw_code_reset()` at each hook
  boundary — by the next hook the JIT has run other guest code, so slots restart
  at 0 safely.
- After Hydra redirects CS:IP to the slot (CALL/CALL_NEAR result), the driver
  pushes the redirect into dosemu2 (`r cs` / `r ip`) and single-steps
  (`dosdebug_step()` → `t\n`; `t` steps over INTs, as dosemu2's tracer treats
  INT like CALL) until the terminating RETF/RET lands at the Hydra return
  address, then feeds the post-raw-code registers back into the exec engine.
- The same trace path completes **native→guest calls** (callstub CALL_FAR/CALL_NEAR
  into unhooked guest code): the step budget is `opts->max_trace_steps`
  (default 10000), and if the traced guest enters another hook's breakpoint,
  that hook is dispatched **recursively mid-trace** (depth cap 8).
- A hook may emit several sequential raw-code requests (e.g. CLI, STI, INT, INB,
  OUTB in one hook); the driver's inner loop handles each until the hook returns
  no redirect (cap 256 per dispatch).
- **No return-stub breakpoint is planted.** The designed one-shot stub at
  `ffff:0000` never fired (that address holds BIOS ROM content — boot vector +
  date string — in a running guest, and conflicts with dosemu2's ONE_STEP
  breakpointManager), which is why tracing replaced it.
- Calling `hydra_machine_notify(m)` after every `hydra_machine_exec(m, 0)` is
  mandatory — without it, the `hydra_callstack_trigger_enter` assertion
  (`call_event == CALL_EVENT_NONE`) fires on the second exec.

## 7. Hydra bridge mapping

| Hydra op | Implementation | Source |
|---|---|---|
| `mem_hostaddr` | `mmap'd_lowmem + addr` via `/proc/<pid>/fd/<fd>` | this doc §4 |
| `mem_read8/16`, `mem_write8/16` | direct dereference of mmap'd pointer (or `LOWMEM_READ/WRITE_*` equivalents) | this doc §4 |
| `io_in8/16`, `io_out8/16` | guest opcode execution via `hydra_impl_raw_code` + single-step trace | this doc §6 |
| `update_registers` | dosdebug `r REG val` per register | this doc §5 |
| `state_save` | dosdebug `r` → parse register dump | this doc §5 |
| `state_restore` | dosdebug `r REG val` per register | this doc §5 |
| `hydra_machine_init` | connect dosdebug FIFOs + mmap lowmem | this doc §3-4 |
| `hydra_machine_exec(m, interrupt_count)` | set breakpoints + `g` (run until next hook) | this doc §3 |
| `hydra_machine_notify` / `step_hook` | breakpoint hit handler | this doc §5 |

## 8. Phased implementation (all complete)

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

## 9. Reference: prior in-process plugin research (appendix)

A prior design phase investigated an in-process `src/plugin/hydra/` plugin with a
minimal simx86 core callback (function-pointer registration slot in `FindExecCode`).
That approach required a dosemu2 fork and a ~17-line core edit. The current external
client approach supersedes it — no rebuild is needed. The prior research findings
(dosemu2 plugin architecture, debugger hook trace, simx86 internals) remain useful
reference and are summarized below.

**simx86** is dosemu2's software CPU emulator (`src/base/emu-i386/simx86/`). It
translates x86 instructions into internal "nodes" (TNodes) and executes those. Under
`MSSTP` each node is one instruction. The main loop is `FindExecCode()`
(`interp.c:431-517`); `EXCP01_SSTP` (=2, `emu86.h:661`) is the single-step boundary
marker. The debugger's per-instruction hook uses TF → `MSSTP`/`MTRAP` → `EXCP01_SSTP`
→ `VM86_TRAP` dispatch → `mhp_debug(DBG_TRAP)` (`do_vm86.c:561-564`). No generic
observer/callback registration API exists in simx86.

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
