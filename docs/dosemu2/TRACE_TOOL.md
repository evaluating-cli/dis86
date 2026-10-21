# The instrumented-execution trace tool

Issue [#97](https://github.com/evaluating-cli/dis86/issues/97) proposes a
small instrumented-execution tool: run a real DOS binary under
dosemu2 + Hydra and extract structured insight for reverse-engineering work.
This document is the tool's contract: what it is, what it observes, what it
deliberately does not observe, and how its correctness is anchored.

The tool is `hydra_trace` (`hydra/src/dosemu_host/trace_tool.c`). The record
it produces is the Hydra trace sink (`hydra/src/trace.c`, documented in
[`../../hydra/README.md`](../../hydra/README.md)), and the reader for that
record is `dis86 --trace-analyze`.

## What it is

`hydra_trace` drives the same host loop the integration tests drive
(`host_run()` in `hydra/src/dosemu_host/host_driver.c`) with the trace sink
open. It adds no host machinery of its own: no new transport, no new CPU or
DOS model, no second validation path.

```
hydra_trace --pid <dosemu2 pid> --program <file.com> --trace <out>
            [--lib <user.so>] [--step] [--stop-after <n>]
            [--poke-image <off>:<val>] [--max-steps <n>]
            [--timeout-ms <n>] [--verbose]
```

| Option | Meaning |
| --- | --- |
| `--pid` | the dosemu2 process to attach to (the host is the stock binary; Hydra is an external client) |
| `--program` | the guest image to trace. `.COM` only in this version |
| `--trace` | the trace file to write |
| `--lib` | the user library: the Hydra metadata library, and - when it exports `hydra_user_register_hooks()` - the native bodies of the hooked functions |
| `--step` | record the individual guest instructions of the regions the host single-steps |
| `--stop-after` | stop the run after this many hook dispatches (a guest that loops forever would otherwise never let go) |
| `--poke-image` | write a byte into the guest image before the guest is released (image offset, not a guest address) |
| `--max-steps`, `--timeout-ms`, `--verbose` | the run limits, as in the integration driver |

The guest image must carry the raw-code reservation marker
(`HYDRA_RAW_SLOT!!`, the convention the integration fixture uses): the host
refuses to run without a guest-owned raw-code region, and the tool does not
commandeer a low-memory address of its own.

The guest must be **freshly booted**: the tool waits for the image to appear
in guest memory and then takes the segment the guest runs in. A guest that
has already left its load phase is not a guest the tool can attach to.

## What it observes

The guest CPU runs free between the function-level breakpoints. That is the
single most important fact about this tool, and it bounds everything below.

| The trace holds | The trace does not hold |
| --- | --- |
| the function-level hook boundaries (a dispatch and its return) | the guest code between two breakpoints |
| the guest call/return boundaries the callstack model sees at those boundaries | a call to a function that is neither hooked nor reached through a call seam |
| the native-to-guest call seam: the callee, the arguments it was handed, and the value it returned | the arguments of a call the decompiler did not lower through a call stub |
| - with `--step` - the individual guest instructions of the regions the host single-steps | port I/O, and any device state (see the limits below) |

So the discovery unit is the **function**, not the instruction. A traced call
records the calling function's *entry* address, because the decompiler does
not model intra-function instruction addresses.

## The trace format

Line oriented text, one event per line:

```
# hydra trace v1
# code_load=0080
dispatch enter cs=0080 ip=0013 name=F_myfunc
dispatch leave cs=0080 ip=0013 result=resume
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
- `name=` appears only when the metadata resolves the address.
- `nargs=` counts 16-bit argument words, followed by `arg0=`, `arg1=`, ... The
  words are read back from the guest stack, so they are what the callee will
  read, not the values the caller had in C.
- `dx=`/`ax=` are the registers the callee left behind. **They are not a
  return type**: a callee that returns 16 bits in AX leaves whatever was in
  DX, so a non-zero `dx` does not mean the call returned 32 bits.

The format is pinned by `hydra/src/dosemu_host/test_trace.c`, which the
reader in `dis86 --trace-analyze` matches.

## Reading a trace

```
dis86 --trace-analyze --trace <trace file> [--config <config.bsl>]
```

The reader turns the record into the function-level picture: the boundaries
the run touched, the calls between them, the argument counts observed at the
call seams, and the values the callees left behind. It prints the call graph
and - for the functions the configuration does not know yet - suggestion
lines in the same shape `--analyze` prints, so a trace and a static analysis
can be pasted into the same annotation file.

Two things the reader deliberately does not infer:

- the **end** of a discovered function. Tracing sees where a function was
  entered, not where it stops.
- the **return type** of a call. The trace records the registers the callee
  left behind, and a callee that returns 16 bits in AX leaves whatever was
  in DX.

The **argument count** is exact: it is the number of words the call stub
pushed, which is what the configuration's `args` field states.

## Correctness: the parity anchor

The tool's own correctness rides on the [#96](https://github.com/evaluating-cli/dis86/issues/96)
parity fixture rather than on a validation path of its own. `parity/build.sh`
runs the fixture with the trace sink open (Gate 3) and checks the
observations against the fixture's known layout: the entry function was
dispatched, the call into the leaf was recorded with its argument count, and
the returned value was recorded at that boundary. The expected addresses are
read from the generated configuration, so they move with the annotations
instead of being a second copy of the layout.

The lane also carries a **mutation check**: a tampered copy of the trace must
fail the same check. A green run means "the tool saw what the fixture says",
not merely "the tool ran".

## Limits

These are deliberate, and they are the same discipline the parity anchor
uses:

- **No port I/O claims.** Raw IN/OUT reaches real device state (PIT, RTC,
  VGA) and is nondeterministic; the parity anchor excludes it for the same
  reason.
- **No snapshot/replay.** `state_save`/`state_restore` do not carry the
  device state (the PIT and keyboard gap), so a replay-based analysis would
  silently lose it.
- **No whole-program single-stepping.** The guest runs free between the
  breakpoints; `--step` records the instructions of the regions the host
  already single-steps (the native-to-guest callthrough trace), at the cost
  of a debugger round trip per instruction.
- **`.COM` guests only** in this version. MZ guests need the launcher
  handshake the integration driver implements; the follow-up reuses it.

## See also

- [`../../hydra/README.md`](../../hydra/README.md) - the trace sink in the
  core, its events, and its counters.
- [`../../parity/README.md`](../../parity/README.md) - the parity fixture and
  the trace gate that anchors this tool.
- [`README.md`](README.md) - the dosemu2 hosting in general.
