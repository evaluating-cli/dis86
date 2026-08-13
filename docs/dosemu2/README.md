# dosemu2 validator migration

This directory documents the 16-bit real-mode `simx86` backend used by the dis86 differential validator.

## Current coordinates

- **Implementation carrier:** `patches/dosemu2/series` (ten ordered `git am` patches)
- **Pinned dosemu2 base:** `604ce0cdd1a71f657e2a2df623d216d5ab289313`
- **Shared-memory ABI:** version 1; 88-byte append-only structure preserving the 64-byte Hydra prefix
- **Scope:** one validator instance controlling a 16-bit real-mode MZ executable

PR #17 established the carrier; PRs #18–#20 completed the Rust outcome and shutdown path and added a terminating runtime fixture. PR #21 added pinned-runtime target-exit and fault evidence. PR #22 pinned and digest-verifies the comcom32 artifact. PR #23 added host-only coverage for the exact command, MZ identity, canonical path, mapping size, and launcher/descendant PID ownership. PR #25 added patch 0010.

Patch 0010 implements deferred acknowledgement for standalone nonterminating host services. Eligible unchanged DOS/BIOS vectors are normalized at the exact saved return `CS:IP`. Application-installed handlers are a different contract: they remain visible and controller-stepped in lockstep.

## Evidence snapshot

**Pinned-runtime proven:** the ten-patch series applies/builds/links; pinned FDPP and comcom32 provisioning; ABI initialization; basic request/step acknowledgement; live `/dosemu_mem` bidirectional aliasing; end barrier and clean shutdown; terminating MZ execution; target-exit and fault publication; and an unprefixed `INT 21h/AH=30h` acknowledgement at the post-service target boundary with DOS-returned state.

**Not integration-tested:** prefixed service calls; application-installed handlers outside the target MCB; broader BIOS coverage; REP; interrupt shadow and shadow composition; child/helper exclusion; helper/lifecycle transitions; broad state/control redirection; and the full per-boundary differential corpus.

“Implemented,” “host-only tested,” and “pinned-runtime tested” are distinct claims. Focused runtime proofs must not be reported as completion of the expanded corpus. Correctness smoke tests also establish no performance claim.

## Document map

| Document | Authority |
| --- | --- |
| [`PHASE1_SPEC.md`](PHASE1_SPEC.md) | Normative execution, ABI, ownership, interrupt, and shutdown contract. |
| [`PHASE1_IMPLEMENTATION.md`](PHASE1_IMPLEMENTATION.md) | Current implementation map and design details. |
| [`TESTING.md`](TESTING.md) | Evidence levels, commands, proven paths, and remaining integration gate. |
| [`REVIEW.md`](REVIEW.md) | Design decisions and review cautions not repeated by the spec. |
| [`SOURCES.md`](SOURCES.md) | Code and upstream source coordinates. |
| [`patches/dosemu2/README.md`](../../patches/dosemu2/README.md) | Patch application, provenance, and carrier mechanics. |

`PHASE0_GATE.md`, `PR_PHASE0_HARNESS.md`, `PR_PHASE1_IMPLEMENTATION.md`, and `PR_DESCRIPTION.md` are historical records. They are retained for provenance, not as current status or implementation guidance.
