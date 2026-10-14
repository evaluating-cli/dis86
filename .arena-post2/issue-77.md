<!-- arena-triage-verify-r2 -->
Reproduced on current `main` (`ca4981b`) — both cases match the report exactly, and the same wrong fragments leak into byte **reads** too.

Case 1 (`mov ax,0x0607; mov ah,0x4A; ret`): emitted `AX = (74 << 8) | (u16)0x607;` = **0x4E07** (expected 0x4A07).

Case 2 (`mov ax,0x1234; mov al,0x56; ret`): emitted `AX = ((u16)(0x1234 >> 16) << 8) | 86;` = **0x0056** (expected 0x1256).

`--emit-ir-final` for case 1 shows the mis-typed fragment directly:

```text
al.2     = u16      lower16    #0x607                 <- 16-bit "low byte"
t0       = u16      shl        #0x4a         #8
ax.3     = u16      or         t0            al.2     <- 0x0607 leaks into AH
```

Root cause confirmed as described: `append_asm_dst_reg` splits a full 16-bit write with `append_upper_lower_split`, and `Upper16`/`Lower16` (`v >> 16` / `v & 0xffff`) only make sense on the 32-bit `Make32` values at the MUL/DIV/CWD/LES/LDS sites. `Lower16` of a 16-bit value is the whole value and `Upper16` is 0, so `Or(Shl(imm,8), lo)` ORs the leaked high byte into `AH`, and `Or(Shl(0,8), imm)` drops the preserved one.

## Extra: fragment **reads** are wrong too, not just recomposes

`mov ax,0x1234; mov [0x100],ah; ret`:

```text
ah.2     = u16      upper16    #0x1234
         void     store8     DS           #0x100       ah.2
```

```c
*PTR_8(DS, 0x100) = (u16)(0x1234 >> 16);   /* stores 0x00, expected 0x12 */
```

So every `mov r/m8, ah|bh|ch|dh` after a full write stores 0 — a larger blast radius than the recompose framing. The low-fragment read (`mov [0x100],al`) survives only because C truncates the 16-bit value to its low byte; any other byte consumer of a fragment (`cmp al,imm`, …) sees a `u16`, so masking only inside the recompose would leave that class broken. That argues for fixing the split (or the fragment read), not the recompose.

## Test gap confirmed (and one test pins the bug)

`partial_writes_recompose_full_register` asserts the opcode *shape*, including `assert_eq!(..AL.., Lower16)` / `assert_eq!(..AH.., Upper16)`, and never the numeric composition — a fix will have to update that assertion as well as add value-asserting cases. `int_register_setup_survives_dce` only covers `mov ah,imm` as the *first* write to the pair (sibling `Init(AL)`), where the recompose reads a real byte alias and happens to be correct.

## Suggested shape

Make the split produce genuine bytes — `AL = And(v, 0xFF)`, `AH = And(UShr(v,8), 0xFF)` — and keep `Upper16`/`Lower16` for `Make32` only. Then the recompose needs no extra mask and fragment reads are correct by construction. Regression tests should assert values (`0x4A07`, `0x1256`, the `store8` case), not shape.

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
