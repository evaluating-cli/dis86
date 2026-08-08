# Pull Request: Add DosemuProcess Validator Adapter and Configurable Emulator Backend Support

## Summary

This pull request introduces the `DosemuProcess` process adapter and adds configurable emulator backend selection (`--backend <dosbox-x|dosemu2>` or `EMULATOR_BACKEND=dosemu2`) to the `dis86` differential validator (`emu86_validator`).

---

## Changes

1. **`dis86/src/emu86/validator/dosemu_process.rs`**:
   - Implements `DosemuProcess` adhering to the `Emu` trait.
   - Spawns `dosemu2` in headless batch mode (`-dumb -quiet -K <dir> -E <exe> -hydra <libhydraremote.so>`).
   - Integrates with the shared-memory transport (`/dev/shm/hydra_remote` and `/dev/shm/dosemu_mem`).
   - Implements atomic acquire/release synchronization, spin-wait with timeout detection, and full 14-register snapshot extraction.

2. **`dis86/src/emu86/validator/run.rs`**:
   - Defines `EmulatorBackend` enum (`DosboxX` and `Dosemu2`).
   - Adds `Validator::new_with_backend()` and `run_with_backend()`.
   - Supports selecting the backend via `EMULATOR_BACKEND` environment variable.

3. **`dis86/src/bin/emu86_validator.rs`**:
   - Adds optional `--backend <dosbox-x|dosemu2>` CLI parameter.

4. **Validation**:
   - Verified with `just check` (158 passing unit and reference tests).
