<!-- arena-triage-verify-r6 -->
Confirmed on `main` (`f5b563c`), and the inconsistency is visible **within the same instruction family** — which I think settles it in favour of the non-fatal reader.

Same source shape, three opcodes:

```c
/* seto al / setno al / setp al / setnp al */
AX = (AH << 8) | (UNIMPL() /* UNIMPL op */ & 255);

/* sets al */
AX = (AH << 8) | (UNIMPL_FLAGS(FLAGS) /* UNIMPL flags */ & 255);

/* setns al */
AX = (AH << 8) | ((UNIMPL_FLAGS(FLAGS) /* UNIMPL flags */ == 0) & 255);
```

`SETA`/`SETAE`/`SETB`/`SETBE`/`SETE`/`SETNE`/`SETG`/`SETGE`/`SETL`/`SETLE` also use `UNIMPL_FLAGS` (they fold from the CF/ZF/SF readers). So `SETO`/`SETNO`/`SETP`/`SETNP` are the only four of the sixteen that abort, purely because OF and PF have no reader opcode yet — exactly the "missing input, not missing operation" case your marker taxonomy draws the line on.

## One consequence worth weighing against #96

Because `UNIMPL()` aborts (`hydra/src/header.h:23`), **any decompiled function containing a reachable `seto`/`setp` cannot run to completion** — it cannot warn-and-continue the way a `UNIMPL_FLAGS` site can. That matters for the standing parity anchor: a fixture whose semantics list grows to include overflow/parity-driven branches would abort on the native side instead of producing an outbox. (#96's v1 fixture excludes port I/O for the same determinism reason; this is the analogous trap for OF/PF.)

I agree with the counterpoint you already note — `UNIMPL_FLAGS` returns 0, so `seto` would silently yield false. But the alternative today is an abort, and the emitted C carries the marker either way, so the failure is still visible at the source level rather than only at runtime.

Evidence: branch `arena/737d7972-dis86`, `.arena-verify4/out/3b-p-{seto,setno,setp,setnp,sets,setns,seta}.txt` and `.arena-verify4/out/85-t85-seto.txt` (which also carries the build-time `Unimpl opcode OP_SETO at 0000:0000 — emitting UNIMPL marker` warning).

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence is on branch `arena/737d7972-dis86`._
