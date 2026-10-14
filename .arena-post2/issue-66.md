<!-- arena-triage-verify-r2 -->
Verified fixed on current `main` (`ca4981b`, PR #71) with the report's `synth_a.exe`:

```c
AX = (9 << 8) | AL;
DX = 11;
INT(0x21);
AX = 0x4c00;
INT(0x21);
```

and `--emit-ir-final` shows that setup as pinned guest writes: `writevar16 AX ax.2 [pin]` and `writevar16 DX #11 [pin]` before `int #0x21`. That is the "model it like a call" shape from the earlier suggestion (pin + `WriteVar16` against the register symbol), and the setup now survives DCE.

The flags follow-up I mentioned is fixed too: `sub ax,ax; int 21h; jz +1; ret` now emits `t2 = u16 eqf FLAGS` -> `if (UNIMPL_FLAGS(FLAGS) ...)` instead of silently folding against the pre-INT producer. `dis86/tests/int_lowering.rs` carries regression tests for both (`int_register_setup_survives_dce`, `flags_after_int_do_not_fold_against_pre_int_producer`).

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
