<!-- arena-triage-verify-r2 -->
Reproduced on current `main` (`ca4981b`) with the report's binary and config. Emitted code and IR match the report:

```c
AX = (6 << 8) | AL;
BX = AX;          /* reads the recomposed AX */
INT(0x21);
DX = DX;
AX = BX;
```

```text
ax.2     = u16      or         t0           AL
         u16      writevar16 AX           ax.2   [pin]
         u16      writevar16 BX           AX     [pin]   <- live symbol read
         void     int        #0x21
```

Mechanism confirmed: `append_asm_src_reg` is a bare `get_var`, so `mov bx,ax` records `Ref::Init(AX)`; `Ref::Init(<reg>)` renders as the live register; `process_int` materialises in the fixed order `[AX,BX,CX,DX,SI,DI,DS,ES]` at the INT site, so any value that is `Ref::Init(src)` for a source materialised *earlier in that order* reads the new value.

## Three extra probes that bound the fix

- **Control (correct today)**: `mov ax,bx; mov bh,1; int 21h; ret` emits `AX = BX;` *before* `BX = (1 << 8) | BL;`, so AX captures the pre-write BX. This is order-dependent, not a blanket reordering of all copies: merely reversing the loop order would move the hazard onto this shape.
- **Mirror case (also wrong)**: `mov si,cx; mov ch,1; int 21h; ret` emits `CX = (1 << 8) | CL; SI = CX;` — CX is materialised before SI, so `SI` silently captures the recomposed CX. The invariant needed is "the snapshot is taken in program order", not "AX happens to be first".
- **Same defect at call sites**: with a callee annotated `regargs AX,BX`, `mov bx,ax; mov ah,6; call f; mov ax,bx; ret` emits `AX = (6 << 8) | AL; BX = AX; tmp_0 = f(m);`. `append_regargs` has the identical annotation-order loop with no snapshot, so a fix in `process_int` alone would leave the call path broken; both sites (and any future materialisation site) should share one snapshot helper.

## Suggested direction

Have a register-to-register copy materialise a real SSA value (e.g. a pinned `Ref`/temp) instead of aliasing `Ref::Init(src)`; alternatively snapshot the live values in program order (`tmp_X = X`) before the batched writes, or emit each register's write at its definition site. Any of these also makes the return-path materialisation (`return_vals` renders as `AX = ...; DX = ...;` at the ret) consistent.

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
