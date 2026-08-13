# dosemu2 validator migration

This directory documents the 16-bit real-mode `simx86` backend used by the dis86 differential validator.

## Current coordinates

- **Implementation carrier:** `patches/dosemu2/series` (two squashed frozen `git am` patches)
- **Pinned dosemu2 base:** `604ce0cdd1a71f657e2a2df623d216d5ab289313`
- **Shared-memory ABI:** version 1; 88-byte append-only structure preserving the 64-byte Hydra prefix
- **Scope:** one validator instance controlling a 16-bit real-mode MZ executable

Development patches 0001–0010 are closed historical development. The active carrier is the two-patch squashed frozen series, governed by [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md).

Historical patch 0010 implemented deferred acknowledgement for standalone nonterminating host services. Eligible unchanged DOS/BIOS vectors are normalized at the exact saved return `CS:IP`. Application-installed handlers are a different contract: they remain visible and controller-stepped in lockstep.

## Evidence snapshot

**Pinned-runtime proven:** the frozen carrier applies/builds/links; pinned FDPP and comcom32 provisioning; ABI initialization; basic request/step acknowledgement; live `/dosemu_mem` bidirectional aliasing; end barrier and clean shutdown; terminating MZ execution; target-exit and fault publication; and an unprefixed `INT 21h/AH=30h` acknowledgement at the post-service target boundary with DOS-returned state.

**Not integration-tested:** prefixed service calls; application-installed handlers outside the target MCB; broader BIOS coverage; REP; interrupt shadow and shadow composition; child/helper exclusion; helper/lifecycle transitions; broad state/control redirection; and the full per-boundary differential corpus.

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

`PHASE0_GATE.md`, `PR_PHASE0_HARNESS.md`, `PR_PHASE1_IMPLEMENTATION.md`, and `PR_DESCRIPTION.md` are historical records. They are retained for provenance, not as current status or implementation guidance.
