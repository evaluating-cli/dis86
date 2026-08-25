# dosemu2 hosting

Hydra hosts on the **stock, unmodified** dosemu2 binary. No rebuild, no patching, no
fork. Hydra runs as an external process that drives dosemu2 through its built-in
debugger protocol (dosdebug FIFOs) and maps guest memory via `/proc/<pid>/fd/`. The
design is in [`OPTION_D_DESIGN.md`](OPTION_D_DESIGN.md).

> **Status (2026-08-25): VERIFIED for the supported static-hook scope.** Phases 0–6
> are complete. The final behavioral candidate `3beac798...` passed host-independent
> CI and the stock-dosemu2 runtime workflow on dosemu2 2.0pre9 / Revision 7076,
> including verified lowmem selection, guest-visible IF=0 restoration, persistent
> register+lowmem snapshots, three-hook/nested-callthrough integration, guest-owned
> raw-code scratch, and default-mode overlay rejection. Code lives in
> `hydra/src/dosemu_host/`; see [`TESTING.md`](TESTING.md) for the recorded evidence.
>
> Overlay hooks remain **unsupported by default** in PR #34 and fail closed before
> guest execution. Any future overlay implementation is separate follow-up work and
> must preserve this default contract unless explicitly opted in.

SST (SingleStepTests hardware captures) is the validation authority for emu86. dosemu2's
role is Hydra hosting only — not validation.

## How it works

1. **Launch stock dosemu2** with simx86 (`$_cpu_vm = "emulated"`, `$_cpuemu = (1)`),
   `$_mapping = "mapmshm"` (memfd lowmem), `$_hdimage = "+1"` (FreeDOS boot), and
   the target DOS program.
2. **Connect via dosdebug** — two FIFOs in `$XDG_RUNTIME_DIR/dosemu2/` (`dosemu.dbgin.<pid>`,
   `dosemu.dbgout.<pid>`), created automatically by the stock binary.
3. **Map guest memory** — enumerate the candidate dosemu2 lowmem memfds under
   `/proc/<dosemu_pid>/fd/`, cross-check them against independent dosdebug reads, and
   require exactly one matching backing before mapping it `MAP_SHARED`.
4. **Set breakpoints** at supported static function entry points (`bp ADDR`). Continue (`g`).
5. **On hit** — parse the register dump (including VIF-aware reconstruction of guest
   IF), dispatch the hook through the Hydra exec engine, write return registers, verify
   guest-visible FLAGS, and continue. Guest-opcode/raw-code requests are single-stepped
   until their magic return and use fresh, never-reused addresses inside an explicitly
   guest-owned reservation.
6. **Capture/restore** — clear tracked software breakpoints before memory imaging,
   persist registers plus the `0x110000` lowmem+HMA window, restore memory then CPU
   state, and re-arm breakpoints before guest execution resumes.

Function-level hooking is the right granularity for the supported decompilation path.
Instruction-level interception is used only for bounded raw-code/native→guest tracing.

## Reference: retired validator transport

Development patches 0001–0010 are closed historical development. The active carrier is the two-patch squashed frozen series, governed by [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md).

Historical patch 0010 implemented deferred acknowledgement for standalone nonterminating host services. Eligible unchanged DOS/BIOS vectors are normalized at the exact saved return `CS:IP`. Application-installed handlers are a different contract: they remain visible and controller-stepped in lockstep.

## Evidence snapshot

**Current Hydra host:** exact behavioral candidate `3beac798...` passed both normal CI
and the stock-dosemu runtime workflow. The runtime suite includes the default overlay
fail-closed fixture, `test_host`, and the full function-hook driver with 21 hook
dispatches, 45 raw/native→guest runs, 45 returns, 66 redirects, CF preservation, IF=0
preservation, byte-for-byte raw scratch restoration, and a live dosemu2 process at
completion.

**Pinned-runtime proven (historical validator):** the frozen carrier applies/builds/links; pinned FDPP and comcom32 provisioning; ABI initialization; basic request/step acknowledgement; live `/dosemu_mem` bidirectional aliasing; end barrier and clean shutdown; terminating MZ execution; target-exit and fault publication; an unprefixed `INT 21h/AH=30h` acknowledgement at the post-service target boundary with DOS-returned state; and (historically) the validator corpus mode (`emu86_validator --corpus`) running a small declarative fixture set with per-boundary register and memory-window comparison, proven before the validator binary and fixture corpus were archived.

**Host-only proven (emu86, unit level):** the host-side fixture corpus (REP string-op matrix and seeded register/segment mutation) was deleted when the differential validator was archived. On a separate axis, emu86's instruction behavior is *hardware*-anchored against the SingleStepTests 80286 captures via the SST harness (`docs/emu86/sst.md`); that axis is unrelated to — and does not speak to — twin equivalence with dosemu2 simx86.

**Not integration-tested on the retired differential path:** prefixed service calls; application-installed handlers outside the target MCB; broader BIOS coverage; differential REP semantics (whole-REP-per-step in emu86 vs per-iteration SAME_PC in dosemu2). SST Track 3 R1 validates emu86 REP behavior against hardware, but it does not establish emu86/dosemu2 boundary equivalence; that differential mismatch remains unverified and is simply no longer a validation gate. Also unverified are interrupt shadow and shadow composition; child/helper exclusion; helper/lifecycle transitions; differential register/segment mutation (notably segment-override memory and stack effects); broad state/control redirection; and the representative per-boundary differential corpus.

On a separate axis, emu86's instruction behavior is *hardware*-anchored against the SingleStepTests 80286 captures via the SST harness (`docs/emu86/sst.md`); that axis is unrelated to — and does not speak to — twin equivalence with dosemu2 simx86.

“Implemented,” “host-only tested,” “pinned-runtime tested,” and “exact-head runtime tested” are distinct claims. Correctness smoke tests establish no performance claim.

## Document map

| Document | Content |
| --- | --- |
| [`OPTION_D_DESIGN.md`](OPTION_D_DESIGN.md) | Current design: external Hydra client via dosdebug + verified `/proc/pid/fd` mmap. |
| [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md) | Reference-only: the retired validator's 88-byte ABI-v1 contract. |
| [`PHASE1_SPEC.md`](PHASE1_SPEC.md) | Reference-only: normative execution/ABI/ownership/interrupt/shutdown contract. |
| [`PHASE1_IMPLEMENTATION.md`](PHASE1_IMPLEMENTATION.md) | Reference-only: implementation map for the frozen transport. |
| [`TESTING.md`](TESTING.md) | Current hosting tests and exact-head evidence + reference-only frozen-transport evidence. |
| [`REVIEW.md`](REVIEW.md) | Reference-only: design decisions and review cautions. |
| [`SOURCES.md`](SOURCES.md) | Code and upstream source coordinates. |
| [`patches/dosemu2/README.md`](../../patches/dosemu2/README.md) | Reference-only: patch provenance and carrier mechanics. |
