# dosemu2 hosting testing

This document covers the **Hydra-on-dosemu2 hosting tests** and their exact-head
evidence. The retired differential validator and its frozen transport are
summarized in [`HISTORY.md`](HISTORY.md).

## Current: Hydra-on-dosemu2 hosting (external client)

### Merged exact-head verification

PR [#39](https://github.com/evaluating-cli/dis86/pull/39) was merged as
`5eb5567` on 2026-10-05. Its exact PR head, `b7ee70d`, passed both checks before
merge:

- [Stock dosemu2 host integration](https://github.com/evaluating-cli/dis86/actions/runs/37386457601): **PASS** — the Phase 7 acceptance matrix, including the generated-metadata MZ/FBOV overlay smoke and COM overlay page-in/capture/restore cases.
- [Rust and shared-memory checks](https://github.com/evaluating-cli/dis86/actions/runs/37386457675): **PASS**.

This exact-head result supersedes the earlier local-worktree-only status. The
historical static-hook candidate evidence below remains useful context but is
not the current implementation result.

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

Future behavior changes must pass the repository's normal CI and stock-dosemu
runtime workflow. The checks above record the successful exact-head gate for the
currently merged implementation.

### Build

```sh
meson setup /home/p/dis86/hydra/build /home/p/dis86/hydra   # once
ninja -C /home/p/dis86/hydra/build
```

The dosemu-dependent executables are built under
`hydra/build/src/dosemu_host/`.

### Reference dosemu2 configuration
Integration suite (`run_driver_test.sh`, `TESTPROG_FLAVOR=com|exe|cap|ovl|mz-ovl|both`,
default = all) launches fresh headless dosemu2 instances per stage:

| Stage | What it proves | Guest / driver |
|---|---|---|
| `.com` | hook dispatch, raw-code traces, nested hooks, flags preservation (13 stops / 17 dispatches / 41 raw / 58 redirects, flagres=3203) | `testprog.com` / `test_driver` |
| `.exe` | MZ loading: launcher handshake, entry validation (PSP/MCB/CD 20/entry CS:IP), dynamic `code_load_offset=PSP+0x10`; hook dispatch | `testprog.exe` + `launch.com` / `test_driver_exe` |
| capture | HYDSNAP full-state snapshot (regs + 1MB+HMA lowmem + CRC32) at a deterministic mid-run point | `testprog.exe` / `test_driver_cap cap` |
| restore | same boot on a FRESH instance, snapshot restored byte-exact (memory before registers), execution continues from the capture point and counters advance exactly | `test_driver_cap restore` |
| overlay | lazy `CD 3F` page-in, native first call, dynamic hook dispatch, plus paged/unpaged HYDSNAP restore | `testprog_ovl.com` / `test_driver_ovl` |
| `mz-ovl` | MZ/FBOV fixture metadata generated from annotations, loaded through `lib=`, native first page-in then generated-metadata Hydra overlay dispatch | `testprog_mz_ovl.exe` + `launch.com` / `test_driver_mz_ovl` |

Takes several minutes for all stages:

```sh
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

`run_driver_test.sh` launches a fresh dosemu2 instance per stage, copies the relevant
fixture into its working directory, waits for that newly started `dosemu2.bin`, then
runs its driver. Cleanup is scoped to that instance; unrelated dosemu processes are
left alone.

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

### Overlay behavior

With `overlays=armed`, overlay hooks are lazily armed after the guest pager changes a
stub from `CD 3F` to `EA`. The `ovl` matrix covers first-call native execution,
subsequent Hydra hook dispatch, malformed-stub rejection, and clean paged/unpaged
HYDSNAP capture and restore. Without the opt-in, `test_overlay_reject` requires the
host to refuse execution before the guest runs.

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

emu86/SST are frozen offline tooling with no role in product validation and are
not built by the default lane (enable with `--features emu86`); see
[`docs/emu86/sst.md`](../emu86/sst.md).

## CI job profile and cost

Measured on PR #116: job `Stock dosemu2 host integration` in
`dosemu-host-runtime.yml`, on `ubuntu-24.04`. Total **6m39s**.

| Step | Time |
|---|---|
| Run Phase 7 acceptance matrix | **5m38s** (85%) |
| Install stock dosemu2 runtime | 31s |
| Run host state/transport test | 24s |
| Build Hydra host tests | 3s |
| everything else | ~3s |

Phase 7 dominates. With `TESTPROG_FLAVOR=both`, `run_driver_test.sh` boots
dosemu2 **12 times, serially** (~28s each): mz-ovl 1, exe 1, com 2, cap 3,
ovl 5.

The per-launch `sleep` calls are *not* the cost — they total only ~25–40s across
the whole job. The dominant cost is booting the guest under full CPU emulation
(`$_cpu_vm = "emulated"`, `$_cpuemu = (1)`, set both in `run_driver_test.sh` and
in the workflow's generated dosemu config).

The repository is public, so `ubuntu-24.04` provides 4 vCPU / 16 GB.

### Reducing Phase 7 wall-clock

**Split the flavours into a matrix** (~5m38s → ~3m). `run_driver_test.sh`
already accepts `TESTPROG_FLAVOR=com|exe|cap|ovl|mz-ovl|both`, so no script
changes are needed:

```yaml
strategy:
  fail-fast: false
  matrix:
    flavor: [mz-ovl, exe, com, cap, ovl]
# ...then: TESTPROG_FLAVOR=${{ matrix.flavor }} bash hydra/src/dosemu_host/run_driver_test.sh
```

Each flavour gets its own VM, so there are **no shared-state races** — the real
advantage over parallelising inside the script, where all flavours share
`/tmp/opencode` and `$XDG_RUNTIME_DIR` and each would need its own working
directory. The critical path becomes `ovl` (5 boots, ~140s) plus ~35s setup.
Intra-flavour ordering is preserved: `com` still runs twice inside its job, and
`cap`/`ovl` keep capture-before-restore.

Trade-off: roughly 5× the runner-minutes. Free for a public repository, but more
resource overall.

**Do not touch `$_cpuemu = (1)`.** It is the single largest per-boot cost, but it
is *assumed* to be required for deterministic breakpoint and state inspection.
This has not been tested — treat changing it as a correctness risk, not a
performance knob.

**Do not drop the repeated runs.** The `.com` flavour running twice, and `cap`'s
two restores, are deliberate idempotency and determinism checks.

### Test baselines

The native-host unit test (`build/test_native_host`) counts assertions. Quote the
commit you measured — the count moves as tests are added.

| Commit | PASS | Note |
|---|---|---|
| `e9339ba` | 60 | before the function-hook wiring |
| `fe70157` | 71 | +11 wiring tests |
| `6a1a850` | 72 | +1 (guest termination: two assertions replace one) |

A run is green when it exits 0 with 0 failures.

---

## Retired validator (historical)

The frozen ABI-v1 differential validator (emu86 vs dosemu2) and its pinned-runtime
transport were retired; [`HISTORY.md`](HISTORY.md) keeps the one-page record.
