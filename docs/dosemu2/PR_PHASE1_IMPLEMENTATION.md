# Historical PR #10 phase 1 consolidation

> **Archived:** This was the PR #10 implementation checklist. It is no longer a live status matrix.

PR #10’s architecture was subsequently implemented and extended across PRs #12, #14, #17–#23, and #25. The authoritative current documents are:

- [`PHASE1_SPEC.md`](PHASE1_SPEC.md) — normative contract;
- [`PHASE1_IMPLEMENTATION.md`](PHASE1_IMPLEMENTATION.md) — ten-patch carrier and Rust implementation map; and
- [`TESTING.md`](TESTING.md) — evidence and remaining integration gate.

Important final reconciliation: PR #25/patch 0010 implements deferred acknowledgement across standalone nonterminating host services, and the unprefixed `INT 21h/AH=30h` pinned-runtime proof passes. Host-service normalization is not application-handler suppression: application-installed handlers remain controller-stepped. Prefixed/application-handler cases, broader BIOS coverage, REP, interrupt shadow, helpers/lifecycle, and the broad differential corpus remain not integration-tested.
