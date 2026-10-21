# Hydra

Hydra is a runtime for reverse-engineering x86-16 MS-DOS binaries. It supports
hybrid computation where some functions have been decompiled to ordinary C
code and others remain x86-16 machine code.

## Goal

Hydra provides the runtime side of integrating decompiled code back into a
running x86-16 program without forcing that code back into the original DOS
address space. Decompiled functions execute as native host code while Hydra
keeps the guest register, memory, call/return, overlay, and callstack model
needed to transfer control between native and x86-16 execution.

## Emulator host interface

The Hydra core is emulator-independent. `src/hydra_machine.h` defines the host
bridge for guest memory, I/O, register synchronization, state save/restore,
audio, execution notification, and step hooks. An emulator integration supplies
those callbacks and calls Hydra's exported `hydra_machine_*` entrypoints.

The supported host connects to the stock dosemu2 process through its dosdebug
FIFOs and maps guest memory only after verifying the shared backing. It
implements function hooks, native-to-guest calls, guest register and memory
access, and capture/restore. The implementation and runtime evidence are
documented in [`../docs/dosemu2/`](../docs/dosemu2/README.md).

A second host lives in `src/native_host/`: the standalone substrate for
executable ports, with no emulator process behind it. It implements the same
bridge over a linear 1 MiB guest-memory array and layers a device HAL on top —
a text-mode terminal backend, a selective INT dispatch table, port routing for
the keyboard and PIT, and mirroring of direct guest writes into the text window
(`B8000h` for the CGA-compatible text modes, `B0000h` for MDA). Graphics
backends are not built; [`../docs/native_host/README.md`](../docs/native_host/README.md)
covers what renders and why SDL2 is a new dependency rather than a reuse of
dis86's optional crate.

SST (SingleStepTests hardware captures) validates emu86. The dosemu2 host
provides the execution environment for Hydra and is a separate integration
track.

## Function hooks

A native function can be registered at an x86-16 address:

```
HYDRA_FUNC(H_my_function)
{
  FRAME_ENTER(2);
  u16 arg = ARG_16(0x6);
  u16 result = F_some_other_function(m, arg);
  AX = result > 1 ? 4 : 5;
  FRAME_LEAVE();
  RETURN_FAR();
}
```

When an emulator host transfers execution to that hook, native code can modify
guest registers and memory, call x86-16 or other hooked functions, perform
near/far returns, trigger interrupts, and use guest I/O through the machine
bridge.

## Guest termination

`INT 21h AH=4Ch` ends the guest. The exit code is `AL`.

Termination is **a result the exec loop reports**, not a process exit. The
core's `hydra_impl_int` asks the host's `hydra_user_int` hook what happened;
a terminating interrupt comes back as `HYDRA_RESULT_TERMINATE`, and the
decompiled hook returns at the interrupt — `INT 21h AH=4Ch` does not return,
so the statements the decompiler emitted after it are code the guest never
meant to reach. `hydra_machine_exec` then returns `HYDRA_EXEC_TERMINATED`,
and the exit code is available from `hydra_machine_exit_code()`.

The process-wide decision stays with the host: `machine.c` compiles into every
host, so the core reports that the guest is done and never ends the process
itself. A host driver finishes its own reporting — write the outbox, present
the last frame, save the state — and then exits with the guest's code.

Two things an embedder has to design around:

- **The driver's `main()` regains control**, so its epilogue runs in the
  order it wants. The backend's teardown still happens through the `atexit`
  handler registered at init (atexit is LIFO, so handlers the driver
  registers later run first).
- **The guest's exit code is the driver's to use.** A port that wants the DOS
  convention exits with it; a harness that reports a verdict (the parity
  lane) prints it and keeps its own exit status.
- **The machine is spent after a termination.** A guest that terminates
  inside a function reached through a callstub leaves the caller's execution
  context waiting for a return that never comes, so the machine cannot be
  reused: a second guest needs a second machine. Running more than one guest
  in a process (issue #95 Tier 2) needs an explicit teardown for the
  contexts a terminating guest leaves behind.

Only `AH=4Ch` terminates. `INT 20h` and `INT 21h AH=00h` still fail loud, as
do COM programs that end by far-returning to their PSP. `INT 20h` in
particular is the common COM ending, so this is not yet complete coverage of
how DOS programs actually finish.

## Retained runtime mechanisms

Hydra retains function hooks and native execution contexts, generated-code
register/memory/stack macros, native-to-guest call stubs and return semantics,
overlay handling, data-section integration, callstack tracking/backtraces, and
annotation metadata.

## The trace sink

The core can record what the machine did, as structured text (issue #97). The
sink is **off by default**: every emission helper returns immediately when no
sink is open, so an ordinary run neither changes behaviour nor pays for the
record. A host opens one explicitly:

```c
hydra_trace_open("/tmp/trace.out");   /* 0 on success */
...
hydra_trace_close();
```

The file is line oriented, one event per line:

```
# hydra trace v1
# code_load=0080
dispatch enter cs=0080 ip=0013 name=F_myfunc
dispatch leave cs=0080 ip=0013 name=F_myfunc result=resume
call from=0080:0010 to=0080:0185 name=F_parity_leaf
ret from=0080:0189 to=0080:0016
native_call from=0080:0012 to=0080:0185 name=F_parity_leaf nargs=0
native_ret from=0080:0012 to=0080:0185 dx=0000 ax=0001
step cs=0080 ip=0134
int num=21 ah=4c
```

- Addresses are **absolute guest `seg:off`**. `# code_load=SSSS` states the
  load segment in effect from that point on; it is re-emitted when the load
  segment changes (the MZ path moves the code after the image is loaded), so
  a reader applies the most recent preceding line. Subtracting the code load
  turns a guest address into the image-relative space a configuration table
  uses.
- `name=` carries the name of the function the event is about, when there is
  one: for a dispatch it is the hook's registered name, for the other events
  it is the metadata's name for that address. A hook registered by address
  has no name, and an address no metadata knows has none either.
- The `from=` of a `call` is an address **inside the caller**, never a
  function entry: the return address for a call the decompiler lowered
  through a call stub (there is no guest call instruction to point at), the
  call instruction for a call the guest executed.
- `nargs=` counts 16-bit argument words, followed by `arg0=`, `arg1=`, ... The
  words are read back from the guest stack, so they are what the callee will
  read, not the values the caller had in C.
- `dx=`/`ax=` carry the value returned at a call boundary.

The events come from the places that already know them:

| Event | Emitted at |
| --- | --- |
| `dispatch` | a hook was dispatched / returned (`exec.c`) |
| `call`, `ret` | the callstack model's guest call/return boundaries (`callstack.c`) |
| `native_call`, `native_ret` | the native-to-guest call seam: the arguments and the returned DX:AX (`callstubs.h`) |
| `step` | one guest instruction, emitted by a host that advances the guest one instruction at a time |
| `int` | an interrupt that was serviced (`machine.c`) |

`hydra_machine_stats()` reports the same counts without recording anything,
so a host can report what the machine did with the trace switched off. The
counters are process-wide; a host that needs a per-run figure takes the
difference, the way the dosemu2 host driver does with the hook dispatch count.
The dispatch counter counts dispatch *attempts*: an overlay hook that is not
paged in yet is an attempt whose hook never ran (the host lets the guest page
it in instead), so it has no `dispatch` event.

**What the trace can and cannot see.** On the dosemu2 host the guest CPU runs
free between function-level breakpoints, so the free-running guest is not in
the trace: what is there is the function-level hook boundaries, the guest
call/return boundaries the callstack model sees at those boundaries, the
native-to-guest call seam, and the individual guest instructions of the
regions the host single-steps (the native-to-guest callthrough trace). The
discovery unit is therefore the function, not the instruction, and a traced
call records the calling function's entry address rather than an intra-function
call site.

Two more things the free-running guest keeps to itself: the interrupts it
raises (they never reach the core, so they are neither traced nor counted -
only the interrupts decompiled code raises are), and the calls it makes into
code that is neither hooked nor reached through a call seam.

## Building

No git submodule is required for the Hydra core:

```
just build
```

`just test` builds the local runtime and runs its Meson tests. A running
dosemu2 instance is required to execute a hybrid DOS program through the host.

## Cross-host constraints

These apply to every host (native, dosemu2) and are easy to violate by accident.

- **`hydra_user_*` hooks are `dlsym` symbols that hosts define as functions.** A
  pointer in core code must not share the hook's name — that is a link-time
  collision. `hydra_user_step_hook` escapes this only by being `static` in
  `api_impl.c`, which is unavailable when another translation unit must see it.
  The pointer is named differently already: the `dlsym` symbol is
  `hydra_user_int`, the pointer core code calls through is
  `hydra_user_int_hook` (`hydra/src/internal.h`). Keep the `dlsym` string
  clean.
- **`machine.c` and `exec.c` are shared with the dosemu2 host.** Native-host
  behaviour belongs behind the `hydra_raw_code_available()` guard so dosemu2
  keeps its unchanged path; the guard is `static` to `machine.c`, so it scopes
  that file, and `exec.c` carries the same constraint. Never make the core
  link a backend.
- **Read registers live from the core** (`m->registers`), not the host's mirror.
  The mirror is synced by `update_registers` and can lag executing code.
- **`struct hydra_machine` embeds `hardware[1]` and `registers[1]`** — arrays,
  not pointers. Copying one copies by value, and the copy's `registers` points
  into the copy's own storage.
- **`hydra_impl_*` primitives reach the machine via
  `execution_context_get(NULL)` → `thread_active`,** which is NULL outside a
  running hook. Tests must install a context with `execution_context_set()`. The
  accessor fails loud on NULL, so a missing context is a clean `SIGABRT` — not a
  `SIGSEGV` to be mistaken for a guest fault.
- **Hooks run on their own pthreads** (`run_begin` spawns one per hook). Never
  lazily initialise shared state in a dispatch path — that is a data race. Do it
  in `hydra_user_init()` (once, single-threaded) and let dispatch only *read*.

### Three explicit file lists — add files in all of them

`just check` globs `hydra/src/*.c`, `hydra/src/dosemu_host/*.c` and
`hydra/src/native_host/*.c`, but the meson build and `parity/build.sh` use
**explicit file lists**. A file missing from those passes `just check` and is
then silently not linked — latent until something references it. When adding a
source file, update:

1. `hydra/src/meson.build` — the core sources (`HYDRA_SRC`). This is the one
   that bites silently: a new core file is globbed by `just check` *and* by
   `parity/build.sh`, but without this entry it is absent from `libhydra`.
2. `hydra/src/native_host/meson.build` — `libhydra_native` and the native-host
   test target.
3. `hydra/src/dosemu_host/meson.build` — the dosemu2 host driver and its
   tests.

`hydra/meson.build` itself lists no sources: it holds the compiler flags and
`subdir('src')`.

The meson build uses `-Wall -Werror -Wextra` plus `-Wno-*` disables. The
`dosemu-host-runtime.yml` lane enforces them — it builds with meson/ninja —
while the host-independent `just check` lane compiles with bare
`cc -fsyntax-only` and no warning flags.
