<!-- arena-triage-verify -->
Verified on `54d2d41` with the 79-byte `synth_a.exe` from the report. Emitted code matches the report exactly:

```c
u32 main(void)
{
  u16 SP0 = SP;

  INT(0x21);
  INT(0x21);
}
```

and `--emit-ir-final` shows the setup is gone from the IR itself, not just the C:

```text
b0: () entry
0   |   t0       = void     int        #0x21
0   |   t1       = void     int        #0x21
```

Mechanism confirmed: `OP_INT` carries only the interrupt number (`ir_build.rs:1131-1134`); `Opcode::Int` is a DCE root because it has side effects (`ir/opcode.rs:293`), but liveness follows operands (`opt.rs:415-437`), and the interrupt references no registers. `mov ah,9` / `mov dx,0xb` / `mov ax,0x4c00` die as reported.

Three things that may be useful for the fix:

1. **`--build-pin-all` is not a workaround.** It keeps the values alive, but they materialize as versioned *locals*, not guest-register writes:

```c
  u16 ah_2, dx_2, ax_3;

  ah_2 = 9;
  dx_2 = 11;
  INT(0x21);
  ax_3 = 0x4c00;
  INT(0x21);
```

   In the Hydra model the guest registers are the `AX`/`AH` macros over `m->registers` (`hydra/src/machine.h:14-30`), and `Opcode::Pin` is transparent in C (`ast.rs:505-508`) and skipped entirely at statement level (`ast.rs:1036`). The emulator still sees stale registers, so the flag is a DCE diagnostic, not a correctness knob — worth documenting where `--build-pin-all` is described.

2. **The machinery the suggested fix needs already exists.** `append_regargs` (`ir_build.rs:671-678`) materializes an annotated register argument by appending a pinned `WriteVar16` against the `sym::Table::Register` symbol, which renders as a real guest write. Verified with a callee annotated `regargs AX`:

```c
  AX = 0x1234;
  tmp_0 = callee();
```

   So "model `INT` like a call" concretely becomes: for each register the interrupt consumes, append (pinned `WriteVar16` register-symbol, current SSA value) → `Int(num)` → refresh the clobbered views the way `save_return_value` (`ir_build.rs:599-629`) does for calls. Note one caveat on the literal reading of the suggestion: `Opcode::Int`'s emitter only renders `operands[0]` (`ast.rs:1127-1130`), so merely appending register operands to `Int` would keep them alive but drop them at codegen. A per-interrupt register/clobber contract also needs a home — the config, mirroring `regargs`, is the natural one.

3. **Flags after `int` are unmodeled too** — same root cause, quieter failure mode. `int` neither refreshes nor invalidates the flags SSA value, so a following conditional branch folds against the pre-interrupt producer. Verified with `sub ax,ax; int 21h; jz +1`:

```text
1   |   t1       = u16      eq         AX                   AX
```
```c
  ax_2 = AX - AX;
  INT(0x21);
  if (AX == AX) {
    goto addr_0007;
  }
```

   The guest's real flags after `int 21h` belong to the handler (and `AX` may have been clobbered), so this is silently wrong rather than marked. If per-interrupt flag semantics aren't in scope yet, an "flags unknown after `int`" marker would at least fail loudly. Calls have the same property (`save_return_value` refreshes AX/CX/DX but not FLAGS), so it may be worth deciding the convention for both at once.

---
_Posted from CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); verification evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
