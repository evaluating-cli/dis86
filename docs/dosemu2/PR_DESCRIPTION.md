# Historical PR description: dosemu2 validator migration

> **Archived:** This file was the evolving migration PR summary. It is retained for provenance and is not a current status source.

Use the maintained documents instead:

- [`README.md`](README.md) for the current ten-patch carrier and evidence snapshot;
- [`PHASE1_SPEC.md`](PHASE1_SPEC.md) for the normative contract;
- [`PHASE1_IMPLEMENTATION.md`](PHASE1_IMPLEMENTATION.md) for the implementation map; and
- [`TESTING.md`](TESTING.md) for verified and still-unverified behavior.

The final milestone recorded by this historical PR series is PR #25/patch 0010: deferred acknowledgement for standalone nonterminating host services. The passing pinned-runtime proof uses unprefixed `INT 21h/AH=30h` and observes DOS-returned state at the post-service target `CS:IP`. This does not prove prefixed services, application-installed handler lockstep, broader BIOS coverage, REP, interrupt shadow, helper/lifecycle transitions, or the broad differential corpus.
