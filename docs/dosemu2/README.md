# dosemu2 port

**Supersession note:** SST (SingleStepTests hardware captures) is the validation authority. The dosemu2 differential validator is no longer the validation method. This directory documents the frozen transport/hosting layer for Hydra-on-dosemu2 (Track 4 Option D).

This directory documents the frozen dosemu2 port layer: the 16-bit real-mode `simx86` boundary-hook transport that served as the dis86 differential validator's stepping host. dosemu2's current role is Hydra hosting, not validation: the `simx86` backend is the transport for the in-process Hydra hosting plugin (Track 4 Option D), not the validation lockstep. The frozen transport and its ABI-v1 shared-memory contract are kept as reference for that hosting work (see [`hydra/README.md`](../../hydra/README.md)).

## Roles and glossary

- **emu86** — the upstream-authored Rust 8086/286 interpreter in `dis86/src/emu86/`. It is the project's semantic **reference CPU model**: the readable, authoritative statement of expected instruction, flag, and DOS behavior that this project maintains.
- **dosemu2 simx86** — the frozen `simx86` boundary-hook transport, patched by the frozen carrier below (reference for Option D plugin research). It steps one translated node per request and publishes state.
- **`reference`/`candidate` (validator code)** — internal naming on the *stepping* axis only: `reference` is the stepped host (dosemu2) whose decoded-node counts drive the loop, `candidate` is emu86 replaying them. It says nothing about semantic priority; semantic authority remains with emu86 as the reference CPU model.
- **differential validator (`emu86_validator`)** — archived: the differential validation layers have been deleted, and only the transport layer remains as reference. It was the harness that ran both engines in lockstep and halted on the first state divergence.
- **ABI v1** — the frozen 88-byte shared-memory control contract between the Rust validator and patched dosemu2 (see [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md)).
- **carrier** — the two squash-frozen `git am` patches in `patches/dosemu2/` that implement the dosemu2 side of the contract.

## Current coordinates

- **Implementation carrier:** `patches/dosemu2/series` (two squashed frozen `git am` patches)
- **Pinned dosemu2 base:** `604ce0cdd1a71f657e2a2df623d216d5ab289313`
- **Shared-memory ABI:** version 1; 88-byte append-only structure preserving the 64-byte Hydra prefix
- **Scope:** one validator instance controlling a 16-bit real-mode MZ executable

Development patches 0001–0010 are closed historical development. The active carrier is the two-patch squashed frozen series, governed by [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md).

Historical patch 0010 implemented deferred acknowledgement for standalone nonterminating host services. Eligible unchanged DOS/BIOS vectors are normalized at the exact saved return `CS:IP`. Application-installed handlers are a different contract: they remain visible and controller-stepped in lockstep.

## Evidence snapshot

**Pinned-runtime proven:** the frozen carrier applies/builds/links; pinned FDPP and comcom32 provisioning; ABI initialization; basic request/step acknowledgement; live `/dosemu_mem` bidirectional aliasing; end barrier and clean shutdown; terminating MZ execution; target-exit and fault publication; an unprefixed `INT 21h/AH=30h` acknowledgement at the post-service target boundary with DOS-returned state; and (historically) the validator corpus mode (`emu86_validator --corpus`) running a small declarative fixture set with per-boundary register and memory-window comparison, proven before the validator binary and fixture corpus were archived.

**Host-only proven (emu86, unit level):** the host-side fixture corpus (REP string-op matrix and seeded register/segment mutation) was deleted when the differential validator was archived. On a separate axis, emu86's instruction behavior is *hardware*-anchored against the SingleStepTests 80286 captures via the SST harness (`docs/emu86/sst.md`); that axis is unrelated to — and does not speak to — twin equivalence with dosemu2 simx86.

**Not integration-tested:** prefixed service calls; application-installed handlers outside the target MCB; broader BIOS coverage; differential REP semantics (whole-REP-per-step in emu86 vs per-iteration SAME_PC in dosemu2). SST Track 3 R1 validates emu86 REP behavior against hardware, but it does not establish emu86/dosemu2 boundary equivalence; that differential mismatch remains unverified and is simply no longer a validation gate. Also unverified are interrupt shadow and shadow composition; child/helper exclusion; helper/lifecycle transitions; differential register/segment mutation (notably segment-override memory and stack effects); broad state/control redirection; and the representative per-boundary differential corpus.

On a separate axis, emu86's instruction behavior is *hardware*-anchored against the SingleStepTests 80286 captures via the SST harness (`docs/emu86/sst.md`); that axis is unrelated to — and does not speak to — twin equivalence with dosemu2 simx86.

“Implemented,” “host-only tested,” and “pinned-runtime tested” are distinct claims. Focused runtime proofs must not be reported as completion of the expanded corpus. Correctness smoke tests also establish no performance claim.

## Document map

| Document | Authority |
| --- | --- |
| [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md) | Versioned ABI-v1 freeze and mandatory change gate. |
| [`PHASE1_SPEC.md`](PHASE1_SPEC.md) | Normative execution, ABI, ownership, interrupt, and shutdown contract. |
| [`PHASE1_IMPLEMENTATION.md`](PHASE1_IMPLEMENTATION.md) | Current implementation map and design details. |
| [`TESTING.md`](TESTING.md) | Evidence levels, commands, proven paths, and remaining integration gate. |
| [`REVIEW.md`](REVIEW.md) | Design decisions and review cautions not repeated by the spec. |
| [`SOURCES.md`](SOURCES.md) | Code and upstream source coordinates. |
| [`patches/dosemu2/README.md`](../../patches/dosemu2/README.md) | Patch application, provenance, and carrier mechanics. |

Earlier phase-gate and PR-description records were removed from this directory; they remain available in git history for provenance and are not current status or implementation guidance.
