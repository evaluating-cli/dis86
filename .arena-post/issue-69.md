<!-- arena-triage-verify -->
Confirmed on `54d2d41`. The lowering table is accurate and the line numbers have not drifted — they are still exactly: `REP` `ir_build.rs:817`, `SBB` `:847`, `RCL`/`RCR`/`ROL`/`ROR` `:860`, `ADC` `:870`, `INS` `:1210`, `OUTS` `:1215`, `STD` `:1218`, `STC`/`CLC`/`CMC` `:1222`, `IN` `:1239`, `OUT` `:1242`. The runtime claim holds: `UNIMPL(...)` maps to `hydra_unimpl_trap` → `abort()` (`hydra/src/header.h:14-32`).

Reproduced the class on a 9-byte function (`adc bx,ax; sbb ax,ax; rol al,1; rep movsb; stc; ret`) — every site becomes a marker, and the first one aborts the recompiled program:

```c
  u16 bx_2, al_3, tmp_0;

  bx_2 = UNIMPL(BX, AX) /* UNIMPL op */;
  al_3 = UNIMPL((u16)UNIMPL(AX, AX) /* UNIMPL op */, 1) /* UNIMPL op */;
  tmp_0 = UNIMPL() /* UNIMPL op */;
  UNIMPL() /* UNIMPL op */;
```

Notes for deciding direction:

- **One listed class is already partly recoverable, asymmetrically.** Plain `ROL`/`ROR` do *not* consume CF (only `RCL`/`RCR` do), and the 16-bit result is expressible with existing ops (`Shl`/`UShr`/`Or`). So those two could be lowered value-correctly today, leaving only *consumers of the resulting carry* to degrade to `UNIMPL_FLAGS` (warn-and-return-0, non-fatal). `ADC`/`SBB` are different: they need a carry *input*, and the IR carries one opaque `UpdateFlags` value with no per-flag SSA value, so a value-correct lowering needs an IR change.
- **Adjacent silent case, same family.** Non-`REP` string ops *are* lowered, but without the SI/DI auto-update: `OP_STOS`/`OP_MOVS`/`OP_LODS` (`ir_build.rs:1184-1192`, `:1229`) move the element and note in-code that the pointer update is not modeled, while `OP_CMPS`/`OP_SCAS` (`:1194-1209`) only produce the comparison flags. A hand-written `movsb` loop therefore decompiles to C that copies the same byte forever — no abort, just wrong output. If the `REP` forms are out of reach for now, giving the single-step forms the same loud treatment (or modeling SI/DI, which are in the register SSA) would remove a silent-wrongness case sitting right next to the loud ones.
- **The gap is measurable.** A per-function count of `UNIMPL`/`UNIMPL_FLAGS` sites in the emitter would let a recompile run rank functions by divergence, turning this into a first-class metric for `--codeseg-name` runs instead of a manual grep (the 126/235-across-45/168 numbers in the report presumably came from bespoke tooling).
- Directionally, +1 on keeping unknown ops fatal by default — that is the design stated in the header comment (differential testing pinpoints the first unimplemented operation) — with non-fatal degradation scoped to cases where the *result value* is still correct (as with `ROL`/`ROR` above).

---
_Posted from CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); verification evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
