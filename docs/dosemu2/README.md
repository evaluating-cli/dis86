# dosemu2 hosting

Hydra hosts on the **stock, unmodified** dosemu2 binary. No rebuild, no patching, no
fork. Hydra runs as an external process that drives dosemu2 through its built-in
debugger protocol (dosdebug FIFOs) and maps guest memory via `/proc/<pid>/fd/`. The
design is in [`OPTION_D_DESIGN.md`](OPTION_D_DESIGN.md).

SST (SingleStepTests hardware captures) is the validation authority for emu86. dosemu2's
role is Hydra hosting only — not validation.

## How it works

1. **Launch stock dosemu2** with simx86 (`$_cpu_vm = "emulated"`, `$_cpuemu = (1)`) and
   the target DOS program.
2. **Connect via dosdebug** — two FIFOs in `$XDG_RUNTIME_DIR/dosemu2/` (`dosemu.dbgin.<pid>`,
   `dosemu.dbgout.<pid>`), created automatically by the stock binary.
3. **Map guest memory** — open dosemu2's lowmem memfd via `/proc/<dosemu_pid>/fd/<fd>`
   and mmap it `MAP_SHARED`. Provides the raw pointer Hydra needs (`mem_hostaddr`).
4. **Set breakpoints** at function entry points (`bp ADDR`). Continue (`g`).
5. **On hit** — parse register dump, execute native decompiled function, write return
   registers (`r REG val`), continue (`g`).

Function-level hooking is the right granularity for a decompilation tool. Instruction-
level hooking is not needed.

## Reference: retired validator transport

<<<<<<< ours
The frozen transport layer (`patches/dosemu2/0001`, `0002`) and the ABI-v1 shared-memory
contract were the retired differential validator's mechanism. They are kept as reference
for the boundary-hook and low-memory-backing semantics — not applied, not active. The
ABI is briefly described in [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md).
=======
Development patches 0001–0010 are closed historical development. The active carrier is the two-patch squashed frozen series, governed by [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md).

Historical patch 0010 implemented deferred acknowledgement for standalone nonterminating host services. Eligible unchanged DOS/BIOS vectors are normalized at the exact saved return `CS:IP`. Application-installed handlers are a different contract: they remain visible and controller-stepped in lockstep.

## Evidence snapshot

**Pinned-runtime proven:** the frozen carrier applies/builds/links; pinned FDPP and comcom32 provisioning; ABI initialization; basic request/step acknowledgement; live `/dosemu_mem` bidirectional aliasing; end barrier and clean shutdown; terminating MZ execution; target-exit and fault publication; an unprefixed `INT 21h/AH=30h` acknowledgement at the post-service target boundary with DOS-returned state; and (historically) the validator corpus mode (`emu86_validator --corpus`) running a small declarative fixture set with per-boundary register and memory-window comparison, proven before the validator binary and fixture corpus were archived.

**Host-only proven (emu86, unit level):** the host-side fixture corpus (REP string-op matrix and seeded register/segment mutation) was deleted when the differential validator was archived. On a separate axis, emu86's instruction behavior is *hardware*-anchored against the SingleStepTests 80286 captures via the SST harness (`docs/emu86/sst.md`); that axis is unrelated to — and does not speak to — twin equivalence with dosemu2 simx86.

**Not integration-tested:** prefixed service calls; application-installed handlers outside the target MCB; broader BIOS coverage; differential REP semantics (whole-REP-per-step in emu86 vs per-iteration SAME_PC in dosemu2). SST Track 3 R1 validates emu86 REP behavior against hardware, but it does not establish emu86/dosemu2 boundary equivalence; that differential mismatch remains unverified and is simply no longer a validation gate. Also unverified are interrupt shadow and shadow composition; child/helper exclusion; helper/lifecycle transitions; differential register/segment mutation (notably segment-override memory and stack effects); broad state/control redirection; and the representative per-boundary differential corpus.

On a separate axis, emu86's instruction behavior is *hardware*-anchored against the SingleStepTests 80286 captures via the SST harness (`docs/emu86/sst.md`); that axis is unrelated to — and does not speak to — twin equivalence with dosemu2 simx86.

“Implemented,” “host-only tested,” and “pinned-runtime tested” are distinct claims. Focused runtime proofs must not be reported as completion of the expanded corpus. Correctness smoke tests also establish no performance claim.
>>>>>>> theirs

## Document map

| Document | Content |
| --- | --- |
| [`OPTION_D_DESIGN.md`](OPTION_D_DESIGN.md) | Current design: external Hydra client via dosdebug + `/proc/pid/fd` mmap. |
| [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md) | Reference-only: the retired validator's 88-byte ABI-v1 contract. |
| [`PHASE1_SPEC.md`](PHASE1_SPEC.md) | Reference-only: normative execution/ABI/ownership/interrupt/shutdown contract. |
| [`PHASE1_IMPLEMENTATION.md`](PHASE1_IMPLEMENTATION.md) | Reference-only: implementation map for the frozen transport. |
| [`TESTING.md`](TESTING.md) | Reference-only: evidence levels and proven paths for the frozen transport. |
| [`REVIEW.md`](REVIEW.md) | Reference-only: design decisions and review cautions. |
| [`SOURCES.md`](SOURCES.md) | Code and upstream source coordinates. |
| [`patches/dosemu2/README.md`](../../patches/dosemu2/README.md) | Reference-only: patch provenance and carrier mechanics. |
