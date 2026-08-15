# dosemu2 migration testing

This is the authoritative evidence ledger for the migration; other documents link here rather than maintaining parallel checklists.

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

It also covers the emu86-only fixture corpora in `dis86/src/emu86/validator/fixture.rs`:

- the REP matrix (MOVS/STOS/CMPS/SCAS × byte/word × direction × termination mode), verified as MZ fixtures stepped on emu86 alone;
- the seeded register/segment mutation corpus (ALU/MOV/XCHG/PUSH/POP/segment-override sequences), verified against an independent replay model of emu86's instruction effects; and
- the fixture builder itself plus host-side memory-window comparison logic.

These are unit-test evidence about emu86's own behavior; they are not differential evidence.

### Hardware-anchored emu86 coverage (separate evidence axis)

`just check` also runs a hermetic checked-in micro-corpus of real SingleStepTests
80286 hardware captures through the emu86 SST harness (V1 conservative family:
~1.01M hardware executions, 81.6% PASS; 11 classified emu86-bug clusters, see
`docs/emu86/sst.md`). This is a *hardware*-anchoring axis (emu86 vs a real
Harris 80C286), distinct from — and neither implied by nor implying — the
twin-equivalence claims above about emu86 vs dosemu2 simx86.

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
- the validator corpus mode (`emu86_validator --corpus`), which runs the Rust-side declarative fixture set on the pinned runtime with per-boundary register comparison plus a memory-window comparison over each fixture's deterministic region (`dis86/src/emu86/validator/`).

These prove that the hook, transport, live low-memory alias, basic adapter path, terminal/fault outcomes, the standalone unprefixed host-service normalization path, and the corpus/memory-comparison plumbing execute on the pinned runtime. Host-service normalization is distinct from application-handler lockstep: unchanged eligible vectors are deferred to their saved return, whereas application-installed handlers must remain controller-stepped. They are not a substitute for a representative differential corpus.

## Expanded pinned-runtime corpus still required

Do **not** mark the following integration-tested until checked-in fixtures exercise them against the pinned runtime:

- REP MOVS/STOS/CMPS/SCAS differential alignment. The matrix is captured host-side on emu86 (`fixture.rs` rep_fixtures), but the differential stepping models differ: emu86 completes a REP string op inside one `step()` (whole-REP-per-step) while dosemu2 publishes one SAME_PC node per REP iteration. Reconciling these (classification and possibly an expected-boundary-mapping convention) is required before REP fixtures join the differential corpus;
- differential (pinned-runtime) coverage of the register/segment mutation corpus. It is host-side model-tested on emu86 (`fixture.rs` mutation_fixtures); the differential variants — particularly segment-override memory writes and PUSH/POP stack effects inside a compared window — remain pending;
- interrupt-shadow behavior for STI, MOV SS, and POP SS, including shadow + REP composition;
- prefixed host-service encodings, application-installed handler lockstep (including outside the target MCB), and broader BIOS coverage;
- descendant child/helper exclusion;
- helper/lifecycle transitions, including target -> child -> target and target -> parent / stale-PSP;
- external register/segment/control-flow mutation across a broad instruction corpus; and
- a representative per-boundary memory corpus. The memory-window comparison harness is wired and runs on the pinned runtime with smoke fixtures; the compared windows so far are the fixtures' own deterministic regions, not broad relevant-memory coverage.

Keep the runtime job separate from `just check` so emulator/toolchain failures do not obscure host-independent regressions.
