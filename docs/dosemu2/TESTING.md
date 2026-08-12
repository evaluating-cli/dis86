# dosemu2 migration testing

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

This covers Rust/reference CPU tests and local ABI/state/comparison logic without making normal repository checks depend on a dosemu2 checkout or graphical stack. Passing it is unit-test evidence, not dosemu2 integration evidence.

The optional SDL frontend remains separate:

```sh
cargo build --manifest-path dis86/Cargo.toml --features sdl --bin emu86
```

## Pinned-runtime coverage

The `dosemu2 patch series` workflow uses `patches/dosemu2/` as the implementation carrier. It applies the complete nine-patch series to the exact pinned commit, checks the resulting diff, builds/links the runtime, and provisions pinned FDPP/comcom32 components.

Current focused runtime evidence includes:

- patch-series apply/compile/link success;
- ABI-v1 initialization;
- a basic request/step acknowledgement;
- external bidirectional visibility of the live `/dosemu_mem` backing;
- the zero-more-controlled-nodes end barrier;
- cooperative/clean shutdown behavior; and
- one small terminating MZ fixture driven through the dosemu2 backend/validator path.

These prove that the hook, transport, live low-memory alias, and basic adapter path execute on the pinned runtime. They are not a substitute for a representative differential corpus.

## Expanded pinned-runtime corpus still required

Do **not** mark the following integration-tested until checked-in fixtures exercise them against the pinned runtime:

- REP MOVS/STOS/CMPS/SCAS stepping, termination, state, and memory effects;
- interrupt-shadow behavior for STI, MOV SS, and POP SS, including shadow + REP composition;
- DOS/BIOS handler exclusion;
- descendant child/helper exclusion;
- target -> child -> target and target -> parent / stale-PSP lifecycle transitions;
- external register/segment/control-flow mutation across a broad instruction corpus; and
- full per-boundary architectural-state and relevant-memory comparison against emu86.

Keep the runtime job separate from `just check` so emulator/toolchain failures do not obscure host-independent regressions.
