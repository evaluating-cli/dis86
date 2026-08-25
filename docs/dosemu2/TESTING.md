# dosemu2 hosting testing

SST (SingleStepTests hardware captures) is the validation authority for emu86; this document covers the **current Hydra-on-dosemu2 hosting tests** (first section) and the reference-only frozen dosemu2 transport evidence (remaining sections, historical).

## Current: Hydra-on-dosemu2 hosting (external client)

### Merge gate: exact-head real-dosemu verification

**Status: PASS for the final behavioral candidate.**

The hardened host was verified on commit
`3beac7989b1a982f1645ee597192d0c8a40d8080` in GitHub Actions runtime run #10
(`32875595772`) on Ubuntu 24.04 using the stock dosemu2 PPA package:

```text
dosemu2  2.0~pre9-10260-31cd1289e+202608231532~ubuntu24.04.1
dosemu   dosemu2-2.0pre9, Revision 7076
fdpp     1.10-10002-1671-9282d8c+202605091902~ubuntu24.04.1
```

The same commit also passed the host-independent `test` workflow (run #193). Runtime
results on `3beac798...` were:

- `test_overlay_reject`: **PASS** — a real overlay-typed hook is rejected before guest execution in the default backend mode;
- `test_host`: **PASS** — verified lowmem provenance, complete register transport, guest-visible IF=0 round-trip, persistent register+memory snapshot restore, and breakpoint smoke;
- `run_driver_test.sh`: **PASS** — three static hooks, nested native→guest callthrough, CF preservation, IF=0 preservation, and byte-for-byte raw scratch restoration;
- driver counters: `hook_dispatches=21`, `raw_code_runs=45`, `raw_code_returns=45`, `redirects=66`.

Observed architectural regression samples included `FLAGS=3203` for the CF-preservation
check and `FLAGS=3003` for the IF=0 round-trip check. The guest-owned 8 KiB raw-code
reservation was restored byte-for-byte and dosemu2 remained alive at test completion.

Any commits after `3beac798...` that only update this verification record or PR text do
not change executable behavior. They must still clear the repository's normal CI and
stock-dosemu runtime workflow before merge; the PR discussion records that final-head
check so the merge itself remains exact-head gated.

### Build

```sh
meson setup /home/p/dis86/hydra/build /home/p/dis86/hydra   # once
ninja -C /home/p/dis86/hydra/build
```

The dosemu-dependent executables are built under
`hydra/build/src/dosemu_host/`.

### Reference dosemu2 configuration
Integration suite (`run_driver_test.sh`, `TESTPROG_FLAVOR=com|exe|cap|both`,
default = all) launches fresh headless dosemu2 instances per stage:

| Stage | What it proves | Guest / driver |
|---|---|---|
| `.com` | hook dispatch, raw-code traces, nested hooks, flags preservation (13 stops / 17 dispatches / 41 raw / 58 redirects, flagres=3203) | `testprog.com` / `test_driver` |
| `.exe` | MZ loading: launcher handshake, entry validation (PSP/MCB/CD 20/entry CS:IP), dynamic `code_load_offset=PSP+0x10`; identical hook counts | `testprog.exe` + `launch.com` / `test_driver_exe` |
| capture | HYDSNAP full-state snapshot (regs + 1MB+HMA lowmem + CRC32) at a deterministic mid-run point | `testprog.exe` / `test_driver_cap cap` |
| restore | same boot on a FRESH instance, snapshot restored byte-exact (memory before registers), execution continues from the capture point and counters advance exactly | `test_driver_cap restore` |

Takes ~4 minutes for all stages:

```sh
pkill -9 -x dosemu2.bin 2>/dev/null; sleep 1
cd /home/p/dis86/hydra/src/dosemu_host
timeout 600 bash run_driver_test.sh
```

`run_driver_test.sh` uses `/tmp/opencode/dosemu_mshm.conf` and creates it if absent:

```text
$_cpu_vm = "emulated"
$_cpuemu = (1)
$_sound = (off)
$_layout = "us"
$_vbios_post = (off)
$_console = (0)
$_video = "vga"
$_hdimage = "+1"
$_mapping = "mapmshm"
```

Required properties for this host are `$_mapping = "mapmshm"` (procfs-visible memfd
backings) and a boot/runtime setup that launches the test COM. The test script uses
`XDG_RUNTIME_DIR=/tmp/opencode/runtime` so the dosdebug FIFOs are predictable.

### Test 1: host/transport state checks

`test_host` exercises the host against a running dosemu2 process. It verifies:

- connection to stock dosemu2 and initial register read;
- lowmem mapping selected by the independent dosdebug-probe provenance check;
- complete register read/write;
- guest-visible IF=0 write/read round-trip;
- process-persistent snapshot file containing registers plus the complete
  `0x110000` lowmem+HMA window;
- restoration of both a mutated register and mutated guest memory;
- debugger breakpoint plant/continue/clear smoke behavior.

Launch dosemu2 with the configuration above, obtain the `dosemu2.bin` PID, then run:

```sh
/home/p/dis86/hydra/build/src/dosemu_host/test_host "$DOPID"
```

A passing run ends with:

```text
ALL TESTS PASSED
```

### Test 2: current function-hook driver fixture

Run:

```sh
pkill -9 -x dosemu2.bin 2>/dev/null; sleep 1
cd /home/p/dis86/hydra/src/dosemu_host
timeout 300 bash run_driver_test.sh
```

`run_driver_test.sh` launches a fresh dosemu2, copies the built COM fixture into its
working directory, waits for `dosemu2.bin`, then runs `test_driver` against that PID.

The current fixture is intentionally larger than the original ~95-byte guest because it
contains an aligned **8 KiB guest-owned raw-code reservation**. There is no implicit
`0x1c00` scratch address. `test_driver` locates the reservation marker in the loaded COM
and calls `host_reserve_raw_code()` only after verifying that the entire reservation is
inside the guest image and paragraph aligned.

The five-iteration fixture covers:

- three simultaneous static hooks;
- CLI/STI/INT/INB/OUTB guest-opcode execution;
- native→guest `CALL_FAR` callthrough to unhooked guest code;
- a nested hooked call encountered while tracing that guest callthrough;
- CF preservation across hook return;
- **IF=0 preservation across a complete hook round-trip**;
- every raw/native→guest call reaching its magic return;
- exact redirect accounting; and
- byte-for-byte restoration of the full 8 KiB raw-code reservation after all snippets.

For `target=5`, the current test code requires:

```text
hook_dispatches = 21
raw_code_runs = 45
raw_code_returns = 45
redirects = 66
```

It also requires the driver to stop via its callback, the expected guest results to be
observed, the nested hook to return `0xCAFE`, CF to remain set where expected, IF to
remain clear in the IF regression sample, and dosemu2 to still be alive at the end.
A passing run ends with:

```text
=== TEST PASSED ===
```

### Unsupported overlay behavior

The external dosdebug backend does **not** claim overlay-hook support. Registering a
`HYDRA_HOOK_FLAGS_OVERLAY` hook causes breakpoint installation to fail and `host_run()`
to refuse execution. This replaces the old silent overlay skip. The negative
`test_overlay_reject` fixture is a permanent default-mode contract: a future overlay
implementation must preserve this behavior unless an explicit opt-in mode is enabled.
No `.ovl` success path is claimed by PR #34.

### dosdebug protocol lessons retained by the current host

- Send register writes paced, with per-command round-trips; bursts were observed to drop
  commands on the tested runtime.
- Read back the register state after a complete push.
- Drain stale debugger output around trace operations to avoid one-block response-stream
  desynchronization.
- `r0` reports raw EFLAGS with physical IF forced by dosemu's vm86 machinery. The client
  reconstructs guest-visible IF from VIF before exposing 16-bit FLAGS to Hydra.
- A direct `r FL value` can apply the requested guest IF correctly but still emit the
  debugger text `failed to set register 'FL'` because its immediate verifier compares
  against raw/normalized flags. The client tolerates only that known FL textual false
  verdict; FIFO/transport failures remain fatal, and `host_set_regs()` verifies the
  resulting guest-visible FLAGS by architectural readback.
- A traced callee that blocks inside DOS I/O can stall a step; the driver bounds each
  individual step to 10 seconds.
- `DOSDEBUG_STREAM_LOG=<path>` records the raw protocol stream for diagnostics.

### Host-independent checks

```sh
just check
```

These checks are useful regression coverage but are not the real-dosemu merge gate.
The CI recipe compiles the `hydra/src/dosemu_host/*.c` implementation/test sources in
addition to the top-level Hydra sources so a green workflow cannot miss host-driver
compile errors.

emu86 validation authority remains independent:

```sh
cargo test --locked --all-targets
# emu86_sst audit --probe           # hardware-corpus authority where applicable
```

---

## Reference-only: frozen transport evidence (historical)

Everything below describes the retired differential validator's pinned-runtime
transport. Kept for provenance; not a validation gate and not the current
approach.

## Test levels

Testing claims use these levels; passing a lower level must not be reported as passing a higher one.

1. **Specified:** behavior is required by `PHASE1_SPEC.md`.
2. **Implemented:** code exists in `patches/dosemu2/` and/or the dis86 adapter.
3. **Unit tested:** host-independent tests exercise code without booting dosemu2.
4. **Pinned-runtime tested:** the patches are applied to dosemu2 commit `604ce0cdd1a71f657e2a2df623d216d5ab289313` and exercised with the pinned runtime stack. The shared-memory contract is ABI version **1**.
5. **Still-unverified E2E:** the expanded pinned-runtime differential corpus has not yet proved the behavior.

## Host-independent checks (historical transport)

The old frozen-transport checks covered the retired validator's host command, target identity, shared-memory ABI, lifecycle, and comparison plumbing. They are retained as transport provenance, not as evidence for the current external Hydra host.

### Hardware-anchored emu86 coverage (validation authority)

SST is the replacement validation authority for the dosemu2 differential validator:
emu86 is validated against real 80C286 hardware captures, and the dosemu2 differential
corpus is no longer a validation gate.

The authoritative hardened full-corpus run recorded elsewhere in this repository
executed 1,064,157 tests across all 268 V1 forms with 1,050,652 PASS, 0 FAIL,
0 DECODE_ERR, and 0 PANIC; see `docs/emu86/sst.md` for aggregate, filtering/revocation
counts, and pinned runner/corpus SHAs. This is the hardware-validation axis, not a
Hydra-on-dosemu2 hosting result.

The optional SDL frontend remains separate:

```sh
cargo build --manifest-path dis86/Cargo.toml --features sdl --bin emu86
```

## Pinned-runtime coverage

The `dosemu2 frozen feature patches` workflow uses `patches/dosemu2/` as the historical
implementation carrier. It applied the frozen patch series to the pinned dosemu2
commit and exercised the validator transport. Focused historical evidence included:

- patch-series apply/compile/link success;
- ABI-v1 initialization;
- request/step acknowledgement;
- bidirectional visibility of the live low-memory backing;
- end-barrier and shutdown behavior;
- a small terminating MZ fixture through the retired backend;
- target-exit and architectural-fault publication;
- host-service normalization cases; and
- the archived differential fixture corpus before that validator path was removed.

These results are not substitutes for the current Hydra host's real-dosemu tests above.

## Expanded pinned-runtime differential corpus

**SUPERSEDED:** this differential corpus is no longer a validation gate. SST validates
emu86 against real hardware captures. The old dosemu2 boundary-alignment work remains
reference-only, including historical open questions around REP boundary mapping,
interrupt shadows, application-installed handlers, lifecycle transitions, broad
register/control-flow mutation, and representative per-boundary memory comparison.

Keep historical runtime evidence conceptually separate from the current external Hydra
host so an old validator pass is never reported as a current hosting pass.
