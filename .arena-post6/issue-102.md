<!-- arena-triage-verify-r6 -->
Confirmed on `main` (`f5b563c`). Two additions to the report: the `switch_blk` reset is not just a caveat for the fix — it is a **second, independently reachable mechanism** — and in the loop case the degradation is not merely loud, it emits a wrong program.

## `switch_blk` loses a `CLD` that demonstrably executed

`cld; jmp 0004; nop; movsb; std; ret` — the `CLD` is in block 0, the `movsb` in block 1:

```c
CLD();
*PTR_8(ES, DI) = *PTR_8(DS, SI);
UNIMPL() /* UNIMPL op */;      /* pointer advance lost */
STD();
```

The identical shape **without** any `std` in the function (`cld; jmp 0004; nop; movsb; ret`) is clean. So the two coexist: `df_clear` is genuinely per-point inside a block, but a block entry resets it to `!df_may_be_set`, discarding the `CLD` from the previous block. Your "Caveats" section is right that the entry seed alone does not fix this.

## The realistic loop case, and why "loud" understates it

`std; cld; movsb; loop 0002; ret` — i.e. the very common "backward copy somewhere in the function, forward copy loop elsewhere":

```c
STD();
CLD();
cx_2 = CX;
while (1) {
addr_0002:;
  *PTR_8(ES, DI) = *PTR_8(DS, SI);
  UNIMPL() /* UNIMPL op */;
  cx_3 = cx_2 - 1;
  ...
}
```

versus the same loop with no `std` anywhere in the function, which models the pointers in SSA:

```c
CLD();
cx_2 = CX; di_2 = DI; si_2 = SI;
while (1) {
addr_0001:;
  *PTR_8(ES, di_2) = *PTR_8(DS, si_2);
  si_3 = si_2 + 1;
  di_3 = di_2 + 1;
  ...
}
```

Note what the degraded form loses beyond the trap: there is **no pointer update at all**, so if the `UNIMPL()` were ever made non-fatal the loop would copy the same byte forever. That supports keeping the fatal marker for unknown direction (as you say at `:82-84`) but argues for fixing the reachability rather than only the marker class — right now any function mixing a `std` copy with a forward loop produces a loop that cannot run.

A forward loop plus a later backward copy (`cld; movsb; loop 0001; std; movsb; ret`) poisons **both** string ops.

## `POPF` behaves exactly like `STD` for the "writer later" case

| case | result |
| --- | --- |
| `movsb; popf; ret` | poisoned — the `popf` is *after* the op |
| `popf; movsb; ret` | poisoned — correctly (DF genuinely unknown) |
| `movsb; std; ret` | poisoned (the issue's case) |

So the entry-seed fix should treat the three writers uniformly, and `POPF` extends the blast radius to functions that never execute `std` at all.

For the requested test matrix, the three shapes that pin both mechanisms are: `movsb; std` (entry seed), `cld; jmp; movsb` **with** a `std` later in the function (block-boundary), and `std; cld; movsb; loop` (loop body). All three are in the evidence below.

Evidence: branch `arena/737d7972-dis86`, `.arena-verify5/out/B-{b1,b2,b3,b5,b6,b7}-*.txt`, `B2-{b9,b10,b11,b12,b13}-*.txt`, and `.arena-verify6/out/B3-{b15,b16}-*.txt`.

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence is on branch `arena/737d7972-dis86`._
