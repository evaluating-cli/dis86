# Implementation source coordinates

> **Note (2026-08-17):** SST (SingleStepTests hardware captures) is the validation authority for emu86; the dosemu2 differential validator is no longer the validation method. These source coordinates reference the frozen transport/patch carrier kept for Track 4 Option D (Hydra hosting via in-process plugin) research.

The implementation is pinned to dosemu2 commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`. Line numbers in upstream sources may change; the carrier patches are the reviewable record of the exact changes.

## Authoritative repository paths

- `patches/dosemu2/series` — two-patch squashed frozen carrier.
- `docs/dosemu2/FREEZE_ABI_V1.md` — ABI-v1 freeze and dosemu2-side change gate.
- `patches/dosemu2/*.patch` — exact dosemu2 changes and commit messages.
- `.github/workflows/dosemu2-patches.yml` — pinned apply/build/link and runtime probes, including FDPP/comcom32 provenance.
- `scripts/dosemu2-runtime-probe.rs` — shared-memory and runtime behavior probe.
- `scripts/make-dosemu2-runtime-probe.py` — MZ fixtures used by the runtime job.
- `dis86/src/emu86/validator/dosemu_process.rs` — dosemu2 launch, ownership, stepping, diagnostics, and shutdown.
- `dis86/src/emu86/validator/shmdata.rs` — ABI-v1 shared-memory mapping and accessors.

## Pinned dosemu2 areas modified or relied upon

- `src/base/emu-i386/simx86/interp.c` — translated-node boundary and validator hook.
- `src/base/lib/mapping/` and mapping headers — live `mapshm` low-memory export.
- PSP, MCB, SDA/current-PSP, and DOS redirector definitions — target identity and ancestry.
- simx86 decode metadata — `TNode.seqnum`, interrupt decoding, and step classification.

## Related projects

- `xorvoid/dis86` — reference x86-16 interpreter; former differential-validator host, now frozen transport reference.
- `xorvoid/hydra` — consumer of the legacy shared-memory ABI prefix.
- `xorvoid/dosbox-x` — historical patched DOSBox-X backend, removed from this tree; its CLI is not the dosemu2 launch contract. Retained here for provenance only.
- `dosemu2/dosemu2` — target DOS runtime.

Performance for normal Hydra hybrid execution and strict validator lockstep must be measured separately. Neither the source review nor focused correctness probes establish a speedup. (Emu86 validation itself is handled by SST, not the frozen dosemu2 transport.)
