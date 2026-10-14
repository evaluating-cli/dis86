<!-- arena-triage-verify-r3 -->
Reproduced on `main` (`bae36b0`): the whole table is live, and I found two things the report does not list — two more panicking opcodes, and a blast radius that reaches the entire run rather than just the offending function.

## Table confirmed

Every opcode listed panics at the catch-all and produces no output:

```text
$ dis86 --config t85-xlat.bsl --binary-raw t85-xlat.bin --name test --emit-code t85-xlat.c
thread 'main' panicked at src/decompile/ir_build.rs:1987:12:
Unimpl opcode: OP_XLAT
Error: Panicked while decompiling test; continuing with the rest.
exit=1
OUTPUT FILE: MISSING (nothing written)
```

All 14 tested (`xlat`, `lahf`, `sahf`, `pushf`, `idiv cx`, `enter`, `aaa`, `aas`, `daa`, `das`, `hlt`, `into`, `seto`, `setp`) behave identically — exit 1, no output file. The four remaining `SETcc` forms from the table (`sets`, `setns`, `setno`, `setnp`) panic too.

I also checked the table for completeness by enumerating every opcode that has an `InstrFmt` entry (`asm/instr_fmt.rs`) but no lowering arm in `ir_build.rs`: the set is exactly the 19 in the report, so nothing is missing there (and nothing extra on that metric).

For contrast, `SETA` — already lowered at `ir_build.rs:1777` — shows the target shape: exit 0, `AX = (AH << 8) | (UNIMPL_FLAGS(FLAGS) & 255);`, i.e. loud about the imprecision but recoverable.

## Addition 1: `POPF` and `IRET` belong in the table

Both are decodable, and their only mention in `ir_build.rs` is inside `opcode_may_set_df` (`ir_build.rs:24`) plus that helper's unit test — there is no lowering arm, so both hit the same catch-all:

```text
Unimpl opcode: OP_POPF      (9D C3)   exit=1, no output
Unimpl opcode: OP_IRET      (CF C3)   exit=1, no output
```

Worth noting for #86 in particular: the direction-flag guard tracks `POPF`/`IRET`, but a function containing either one cannot produce output at all today, so that half of #86 is currently unreachable.

## Addition 2: the panic discards the whole run, not just the function

`app.rs:271` (commit `85661fb`) wraps each spec in `catch_unwind`, which keeps the process alive — but `failures != 0` returns 1 *before* `write_to_path` (`app.rs:290-296`), so nothing is written. With a code segment holding one good function and one panicking one:

```text
$ dis86 --config t85batch.bsl --binary-raw t85batch.bin --codeseg-name CODE --emit-code t85batch.c
thread 'main' panicked at src/decompile/ir_build.rs:1987:12: Unimpl opcode: OP_XLAT
Error: Panicked while decompiling bad; continuing with the rest.
Error: Failed to decompile 1 function(s).
exit=1
OUTPUT FILE: MISSING — the good function was discarded too
```

So `continuing with the rest` is currently misleading: the good function's output is thrown away with the bad one's.

## Suggested shape

1. Replace the catch-all with a loud, non-fatal marker. The precedent already exists: `UpdateFlags` and the flag-test opcodes render as `UNIMPL_FLAGS` (`ast.rs:732`, `:741`), which warns and returns 0 (`hydra/src/header.h:27`), whereas `UNIMPL()` aborts (`header.h:23`). Emitting `Unimpl` plus a `WARN` at the panic site would match the #69 direction.
2. Write the partial output — or at least keep the functions that succeeded — instead of returning early, so one stray opcode cannot discard an entire code segment.

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); the repro scripts and captured evidence for this round are on branch `arena/737d7972-dis86`._
