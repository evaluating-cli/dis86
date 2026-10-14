<!-- arena-triage-verify-r3 -->
Reproduced on `main` (`bae36b0`). The report's example is exact, and three further probes show the guard is coarser than "a `std` that is later balanced by `cld`".

## The report's case, confirmed

```c
// std; cld; movsb; ret
STD();
CLD();
*PTR_8(ES, DI) = *PTR_8(DS, SI);
UNIMPL() /* UNIMPL op */;
```

versus the control (`cld; movsb; ret`), which lowers cleanly. The IR shows what was lost — the control has

```text
si.2 = u16 add SI #1
di.2 = u16 add DI #1
```

while the `std` case has `t4 = void unimpl` and then `pin SI` / `pin DI`. That last part matters beyond the abort: SI/DI are left referring to their **pre-op** values, so any path that does not reach the `UNIMPL` silently runs with stale pointers. `UNIMPL()` is fatal (`hydra/src/header.h:23` — `abort()`), so today you get an abort first, but the emitted C is wrong-and-quiet, not just loud.

## Coarser than reported

1. **REP forms lose the whole operation**, not just the pointer advance. `std; cld; rep movsb; ret` compiles to an IR body of `t2 = u16 unimpl` — no `REP_MOVSB`, no memory side effect, CX/SI/DI unchanged. The control lowers to `REP_MOVSB(DS, SI, ES, DI, CX); tmp_0 = CX * 1;`. Same for `rep stosb`, `rep cmpsb`, `rep lodsb` (`tmp_0 = UNIMPL()`), whose controls lower exactly, e.g. `ax_2 = DI + ((CX - REPE_SCASB(ES, DI, CX, AL)) * 1);`.

2. **Flow-insensitive in both directions.** A `std` *after* the string op poisons it too: `movsb; std; ret` emits `*PTR_8(ES, DI) = *PTR_8(DS, SI); UNIMPL(); STD();`.

3. **Even a `std` on a path that is never taken poisons.** With `jmp +1; std; cld; movsb; ret`, the IR retains only `b0` and `b2` (the `std` block is dead-code-eliminated) — yet the `movsb` still degrades, because `df_may_be_set` is computed from the raw decoded instruction list at `ir_build.rs:2102`, before any optimization.

Blast radius *is* per function, at least: a code segment with `f1` = `cld; movsb; ret` and `f2` = `std; ret` decompiles `f1` fully (exit 0, both functions emitted).

## Note on `POPF`/`IRET`

Both are in `opcode_may_set_df` (`ir_build.rs:24`), but neither has a lowering arm, so each panics at #85's catch-all (`Unimpl opcode: OP_POPF` / `OP_IRET`, exit 1, no output). The `POPF`/`IRET` half of this issue therefore cannot be observed until #85's catch-all is fixed — the two are best fixed together.

## The "Related" item confirmed

`mov ax,0x00ff; shl ax,1; adc ax,0` emits `AX = UNIMPL(255 << 1, 0);` — shifts publish no carry-out, so the following `ADC` aborts rather than lowering.

## Suggested shape

- A per-block DF lattice (`STD`/`CLD` as basic-block facts, merged to "may be set", defaulting to clear per the DOS ABI) would fix items 1-3 at once.
- Degrade only genuinely-unknown cases to `UNIMPL_FLAGS` (warn + return 0, `header.h:27`) rather than fatal `UNIMPL()`: for these ops the *data* movement is known and only the direction is not.
- When the direction is unknown, pin SI/DI to an unknown value rather than to their previous ones, so the stale-pointer class above is visible even if the `UNIMPL` is never reached.

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); the repro scripts and captured evidence for this round are on branch `arena/737d7972-dis86`._
