# dosemu2 migration testing environment

The default repository check is intentionally host-independent. It validates
the Rust decompiler and 8086 reference CPU as well as the C half of the Hydra
shared-memory ABI without installing or building DosBox-X or SDL:

```sh
just check
```

The command requires a stable Rust toolchain, a C11 compiler, and `just`. The
GitHub Actions workflow provisions those tools on Ubuntu 24.04
and runs the same command for pull requests and pushes to `main`.

The recipe names four existing shift/overflow-flag tests explicitly as skips.
They remain visible as known emu86 correctness work rather than hiding failures
elsewhere in the suite; remove each skip when its CPU behavior is corrected.

The interactive `emu86` window is a separate, opt-in developer tool. To build
it on a workstation with SDL2 development files installed, use:

```sh
cargo build --manifest-path dis86/Cargo.toml --features sdl --bin emu86
```

## Future dosemu2 integration tests

End-to-end validator tests should be added as a separate CI job once the
dosemu2 transport exists. That job should invoke a checked-in test corpus in
headless dosemu2 (`-dumb`) and must not make the host-independent job depend on
an emulator checkout, a graphical display, or SDL. Keeping the jobs separate
ensures ordinary Rust and shared-memory protocol regressions remain quick to
diagnose while the dosemu2 integration evolves.
