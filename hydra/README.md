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

`INT 21h AH=4Ch` ends the process. The exit code is `AL`.

Termination is handled **by the host, not the core**: the core's
`hydra_impl_int` fails loud on any non-zero return from the `hydra_user_int`
hook and never exits the process itself. `machine.c` compiles into every host,
so a process-wide policy there would be one the core cannot take back; hosts
that need to observe termination instead of ending would have no seam. The
native host performs the exit inside its own hook
(`hydra/src/native_host/backend_host.c`), which is also the only place that
knows the backend teardown that must run first.

Two consequences an embedder has to design around:

- **The process ends, so a host's epilogue only runs if it is an `atexit`
  handler.** The hook runs on a worker thread, so the host's `main()` never
  regains control. Handlers registered after `hydra_machine_init` run before
  the backend's (atexit is LIFO), which is the order you want.
- **The host cannot observe that the guest terminated.** It only sees the
  process stop. Making termination a result the exec loop can return — so the
  host can finish its reporting and then exit with the guest's code — is the
  target design for issue #95 Tier 2, which covers save/exit behaviour and
  running more than one guest in a process.

Only `AH=4Ch` terminates. `INT 20h` and `INT 21h AH=00h` still fail loud, as
do COM programs that end by far-returning to their PSP. `INT 20h` in
particular is the common COM ending, so this is not yet complete coverage of
how DOS programs actually finish.

## Retained runtime mechanisms

Hydra retains function hooks and native execution contexts, generated-code
register/memory/stack macros, native-to-guest call stubs and return semantics,
overlay handling, data-section integration, callstack tracking/backtraces, and
annotation metadata.

## Building

No git submodule is required for the Hydra core:

```
just build
```

`just test` builds the local runtime and runs its Meson tests. A running
dosemu2 instance is required to execute a hybrid DOS program through the host.
