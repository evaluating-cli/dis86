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

The repository no longer carries or builds the historical patched DOSBox-X
fork; dosemu2 has replaced it as the project's DOS runtime of choice. The
dosemu2-backed differential validator (see `../docs/dosemu2/`) is the first
delivered piece of that port, and is not, by itself, a Hydra native-function
host. Hydra-on-dosemu2 is the intended end state: porting Hydra's
arbitrary-address function interception and native/guest control transfer onto
the dosemu2 hosting target is the remaining integration step on the
roadmap.

Validation of emu86 is handled by SST (SingleStepTests hardware captures), not
the dosemu2 differential validator. dosemu2's role is Hydra hosting via an
in-process plugin (Track 4 Option D); emu86 is validated by SST. The frozen
simx86 transport/ABI is kept as reference for the plugin research.

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

`just test` builds the local runtime and runs its Meson tests. Supplying an
emulator host is required to execute a hybrid DOS program; the historical
DOSBox-X launch scripts and configuration are intentionally no longer part of
this repository.
