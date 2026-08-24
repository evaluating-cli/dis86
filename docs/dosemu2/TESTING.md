# dosemu2 hosting testing

SST (SingleStepTests hardware captures) is the validation authority for emu86; this document covers the **current Hydra-on-dosemu2 hosting tests** (first section) and the reference-only frozen dosemu2 transport evidence (remaining sections, historical).

## Current: Hydra-on-dosemu2 hosting (external client, shipped 2026-08-20)

Build:

```sh
meson setup /home/p/dis86/hydra/build /home/p/dis86/hydra   # once
ninja -C /home/p/dis86/hydra/build
```

Integration test (launches a fresh headless dosemu2, runs the guest COM, hooks
three functions, drives 5 loop iterations — 17 hook dispatches, 41 traced
raw-code/guest-call executions — and verifies dispatch counts, native→guest
callthrough, last-iteration partial accounting, and flag preservation).
Takes ~2.5 minutes:

```sh
pkill -9 -x dosemu2.bin 2>/dev/null; sleep 1
cd /home/p/dis86/hydra/src/dosemu_host
timeout 300 bash run_driver_test.sh
```

Reference dosemu2 config (`/tmp/opencode/dosemu_mshm.conf` — create if absent):

```
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

Required keys: `$_mapping = "mapmshm"` (memfd lowmem backing, so
`/proc/<pid>/fd/` mmap works), `$_hdimage = "+1"` (FreeDOS boot off the working
directory, which must contain `testprog.com`). The test script sets
`XDG_RUNTIME_DIR=/tmp/opencode/runtime` so the dosdebug FIFOs land predictably;
it kills dosemu2 on exit. Expected output ends with:

```
stops=13 hook_dispatches=17 raw_code_runs=41 raw_code_returns=41 redirects=58
...
=== TEST PASSED ===
```

The test guest (`testprog.asm`, ~95 bytes) exercises:
- 3 simultaneous hooks (raw-code opcodes, bare NOP hook, native→guest calls),
- native→guest `CALL_FAR` callthroughs into unhooked guest functions
  (helper2: 13 instructions — beyond the original 6-step trace limit),
- a hooked function called from inside such a guest call — the breakpoint
  fires mid-trace and dispatches as a nested hook,
- guest flags (CF) surviving a full hook dispatch.

dosdebug protocol hard-won lessons (raw stream logs verified these):
- Send register writes PACED (per-command round-trips). Bursts of set-register
  commands get some commands silently dropped by dosemu2's debugger, and at
  high cadence the CPU occasionally executed while the debugger believed it
  stopped. The writes are followed by an `r0` read-back verify that fails
  hard on mismatch.
- The response stream can desynchronize by one block when a step produces
  extra unsolicited text; `dosdebug_drain()` resynchronizes before/after steps.
- `r FL` reports "failed to set register 'FL'" even on success (dosemu
  forces IF/IOPL/bit-1 then verifies the full EFLAGS); the low 16 bits do
  land — always verify flags by read-back (`| 0x3202`).
- `DOSDEBUG_STREAM_LOG=<path>` records the raw protocol stream for debugging.

emu86 validation authority (unchanged, host-independent):

```sh
cargo test --locked --all-targets   # 277 passed
# emu86_sst audit --probe           # 0 PROBE-MISMATCH
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

## Host-independent checks

```sh
just check
```

This covers Rust/reference CPU tests and local ABI/state/comparison logic. PR #23 specifically covers the exact host command, MZ-derived identity, canonical target path, page-sized mapping with its 88-byte ABI prefix, and launcher/descendant PID ownership without making normal repository checks depend on a dosemu2 checkout or graphical stack. Passing it is unit-test evidence, not dosemu2 integration evidence.

The emu86-only fixture corpora (`dis86/src/emu86/validator/fixture.rs` — the REP matrix, the seeded register/segment mutation corpus, and the fixture builder plus host-side memory-window comparison logic) were archived when the differential validator was deleted and are no longer part of `just check` coverage.

### Hardware-anchored emu86 coverage (validation authority)

SST is the replacement validation authority for the dosemu2 differential validator:
emu86 is validated against real 80C286 hardware captures, and the dosemu2
differential corpus is no longer a validation gate.

`just check` also runs a hermetic checked-in micro-corpus of real SingleStepTests
80286 hardware captures through the emu86 SST harness. The authoritative hardened
full-corpus run executed 1,064,157 tests across all 268 V1 forms with 1,050,652
PASS, 0 FAIL, 0 DECODE_ERR, and 0 PANIC; see `docs/emu86/sst.md` for the complete
aggregate, filtering/revocation counts, and pinned runner/corpus SHAs. This is the
*hardware*-anchoring validation axis (emu86 vs a real Harris 80C286); the dosemu2
simx86 transport is kept only as reference for Hydra hosting (Option D), not as a
validation axis.

The optional SDL frontend remains separate:

```sh
cargo build --manifest-path dis86/Cargo.toml --features sdl --bin emu86
```

## Pinned-runtime coverage

The `dosemu2 frozen feature patches` workflow uses `patches/dosemu2/` as the implementation carrier. It applies the two-patch squashed frozen carrier to the exact pinned commit, checks the resulting diff, builds/links the runtime, and provisions pinned FDPP plus the exact digest-verified comcom32 artifact from PR #22.

Current focused runtime evidence includes:

- patch-series apply/compile/link success;
- ABI-v1 initialization;
- a basic request/step acknowledgement;
- external bidirectional visibility of the live `/dosemu_mem` backing;
- the zero-more-controlled-nodes end barrier;
- cooperative/clean shutdown behavior;
- one small terminating MZ fixture driven through the dosemu2 backend/validator path;
- PR #21 target-exit and architectural-fault publication;
- PR #25/patch 0010 nonterminating unprefixed `INT 21h/AH=30h`, acknowledged at the post-service target PC with DOS-returned state that the next target instruction consumes; and
- (archived) the validator corpus mode (`emu86_validator --corpus`), which ran the Rust-side declarative fixture set on the pinned runtime with per-boundary register comparison plus a memory-window comparison over each fixture's deterministic region (`dis86/src/emu86/validator/`) — proven before the validator binary and fixture corpus were deleted.

These proved that the hook, transport, live low-memory alias, basic adapter path, terminal/fault outcomes, the standalone unprefixed host-service normalization path, and the corpus/memory-comparison plumbing executed on the pinned runtime. Host-service normalization is distinct from application-handler lockstep: unchanged eligible vectors are deferred to their saved return, whereas application-installed handlers must remain controller-stepped. They are not a substitute for a representative differential corpus.

## Expanded pinned-runtime corpus still required

**SUPERSEDED:** this differential corpus is no longer a validation gate. SST validates REP and the other in-scope V1 instruction forms against real 80C286 hardware captures (`docs/emu86/sst.md`); the dosemu2 differential corpus below is retained as frozen transport evidence only, relevant for the Hydra-on-dosemu2 hosting work (Track 4 Option D).

Do **not** mark the following integration-tested until checked-in fixtures exercise them against the pinned runtime:

- REP MOVS/STOS/CMPS/SCAS differential alignment. The matrix was captured host-side on emu86, but the differential stepping models differ: emu86 completes a REP string op inside one `step()` (whole-REP-per-step) while dosemu2 publishes one SAME_PC node per REP iteration. Reconciling these (classification and possibly an expected-boundary-mapping convention) would still be required to establish differential equivalence; SST establishes emu86's REP behavior against hardware, not emu86/dosemu2 boundary equivalence. This differential mismatch is no longer a validation gate;
- differential (pinned-runtime) coverage of the register/segment mutation corpus. It was host-side model-tested on emu86; the differential variants — particularly segment-override memory writes and PUSH/POP stack effects inside a compared window — remain pending;
- interrupt-shadow behavior for STI, MOV SS, and POP SS, including shadow + REP composition;
- prefixed host-service encodings, application-installed handler lockstep (including outside the target MCB), and broader BIOS coverage;
- descendant child/helper exclusion;
- helper/lifecycle transitions, including target -> child -> target and target -> parent / stale-PSP;
- external register/segment/control-flow mutation across a broad instruction corpus; and
- a representative per-boundary memory corpus. The memory-window comparison harness is wired and runs on the pinned runtime with smoke fixtures; the compared windows so far are the fixtures' own deterministic regions, not broad relevant-memory coverage.

Keep the runtime job separate from `just check` so emulator/toolchain failures do not obscure host-independent regressions.
