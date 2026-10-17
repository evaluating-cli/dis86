<!-- arena-triage-verify-r6 -->
Confirmed on `main` (`f5b563c`), including **the fragment-aliasing case you asked to be tested** — it reproduces, and the INT path on identical source is the ready-made expectation for the regression test.

## The requested fragment variant (`regargs "AX,CX"`)

`mov cl,al; mov ax,0x1234; call f; ret`:

```c
AX = 0x1234;
CX = (CH << 8) | (AL & 255);   /* CL captures the NEW AL (0x34), not the saved one */
tmp_0 = f(m);
```

Same aliasing through the INT path (`mov cl,al; mov ax,0x1234; int 21h; ret`), which does go through `order_guest_writes`:

```c
CX = (CH << 8) | (AL & 255);   /* before */
AX = 0x1234;
INT(0x21);
```

So the two materialization sites disagree on byte-for-byte identical source, and the INT ordering is the correct one. That pair is a tight regression test: same fixture, two call paths, one expected order.

## Two further facts that sharpen the fix

**1. Correctness currently depends on the annotation order, not program order.** Reversing the config to `regargs "BX,AX"` for the issue's own repro flips the output to the *correct* order:

```c
BX = AX;              /* reads AX before the recompose */
AX = (6 << 8) | AL;
```

So the bug is silent and luck-dependent: whether a given annotated callee decompiles correctly depends on how someone happened to list the regargs. That is worth saying in the test's comment — an annotation-order permutation test would catch a regression that a single fixed-order test cannot.

**2. The wrong value reaches the callee, not just the emitted C.** With a callee that actually consumes `BX` (`f: mov ax,bx; ret`, `ret u16`):

```c
AX = (6 << 8) | AL;
BX = AX;
ax_3 = f(m);          /* f observes the recomposed AX through the guest BX */
AX = ax_3;
```

**3. Scope check — the return path is *not* a third instance.** `return_vals` feeds `retn AX DX`, so I probed it: `mov dx,ax; mov ax,1; ret` with `ret u32` emits

```c
DX = AX;
AX = 1;
```

i.e. `DX` is assigned before `AX`, correctly capturing the old AX. So `append_regargs` is the only remaining unordered batch, and the one-class fix you describe is the whole fix.

Evidence: branch `arena/737d7972-dis86`, `.arena-verify5/out/{C1-call-regargs,C2-call-regargs-use,C5-regargs-reversed,C3-ret-dxax}.txt` and `.arena-verify6/out/{C7-frag-regargs,C8-frag-int}.txt` (+ `-ir` for each).

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence is on branch `arena/737d7972-dis86`._
