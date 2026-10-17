#!/usr/bin/env python3
"""Build the synthetic inputs for the round-3 verification run.

Three groups, all raw binaries run with `--name test --codegen-hydra`:

  * #85 — the decoded-but-unlowered opcode list (panic survey) plus a
    two-function batch that mixes a good function with a panicking one.
  * #86 — the function-wide `df_may_be_set` guard (STD/POPF/IRET poisoning
    string ops) with CLD-only controls.
  * #69 — the closure claim: every instruction class the issue listed, which
    PRs #75/#76/#79-#84 say is now modeled or loud-but-non-fatal.

Everything lands in /tmp/repro.
"""
import pathlib

D = pathlib.Path("/tmp/repro")
D.mkdir(exist_ok=True)


def cfg(path, funcs, seg="CODE"):
    out = f"dis86 {{\n  code_segments {{ _0000 {{ seg 0 name {seg} }} }}\n  functions {{\n"
    for name, (start, end) in funcs.items():
        out += f"    {name} {{ start 0000:{start:04x} end 0000:{end:04x} mode near ret None args None }}\n"
    out += "  }\n  structures {}\n  globals {}\n  text_section {}\n}\n"
    pathlib.Path(path).write_text(out)


def raw(name: str, data: bytes, end: int = None, start: int = 0, fn="test"):
    (D / f"{name}.bin").write_bytes(data)
    cfg(D / f"{name}.bsl", {fn: (start, end if end is not None else len(data))})
    print(f"{name}: {len(data)} bytes -> {name}.bin + {name}.bsl")


# ---------------------------------------------------------------- #77 -------
# Case 1: mov ax,0x0607; mov ah,0x4A; ret   (high-byte leak)
raw("t77a", bytes([0xB8, 0x07, 0x06, 0xB4, 0x4A, 0xC3]), 0x6)
# Case 2: mov ax,0x1234; mov al,0x56; ret   (high-byte drop)
raw("t77b", bytes([0xB8, 0x34, 0x12, 0xB0, 0x56, 0xC3]), 0x6)
# Extra: mov ax,0x1234; mov [0x100],ah; ret  (fragment READ after a full write)
raw("t77c", bytes([0xB8, 0x34, 0x12, 0x88, 0x26, 0x00, 0x01, 0xC3]), 0x8)

# ---------------------------------------------------------------- #78 -------
# Reported: mov bx,ax; mov ah,6; int 21h; mov ax,bx; ret
raw("t78a", bytes([0x89, 0xC3, 0xB4, 0x06, 0xCD, 0x21, 0x89, 0xD8, 0xC3, 0x90, 0x90, 0x90]),
    0xC)
# Control (dest materialized first): mov ax,bx; mov bh,1; int 21h; ret
raw("t78b", bytes([0x89, 0xD8, 0xB7, 0x01, 0xCD, 0x21, 0xC3]), 0x7)
# Variant (dest materialized last): mov si,cx; mov ch,1; int 21h; ret
raw("t78c", bytes([0x89, 0xCE, 0xB5, 0x01, 0xCD, 0x21, 0xC3]), 0x7)
# Same hazard via a call's regargs materialization:
#   mov bx,ax; mov ah,6; call f (regargs AX,BX); mov ax,bx; ret
(D / "t78d.bin").write_bytes(bytes([
    0x89, 0xC3,             # 0: mov bx,ax
    0xB4, 0x06,             # 2: mov ah,6
    0xE8, 0x03, 0x00,       # 4: call f (0x000a)
    0x89, 0xD8,             # 7: mov ax,bx
    0xC3,                   # 9: ret
    0xC3,                   # a: f: ret
]))
(D / "t78d.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    test { start 0000:0000 end 0000:000a mode near ret None args None }\n"
    "    f { start 0000:000a end 0000:000b mode near ret None args None regargs \"AX,BX\" }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
print("t78d: 11 bytes -> t78d.bin + t78d.bsl (call-site regargs AX,BX)")

# ------------------------------------------------------------------ #85 -----
# Each is `opcode; ret` (ret so the function range ends cleanly).
PANIC_CASES = {
    "t85-xlat":  ([0xD7, 0xC3], "xlat; ret"),
    "t85-lahf":  ([0x9F, 0xC3], "lahf; ret"),
    "t85-sahf":  ([0x9E, 0xC3], "sahf; ret"),
    "t85-pushf": ([0x9C, 0xC3], "pushf; ret"),
    "t85-idiv":  ([0xF7, 0xF9, 0xC3], "idiv cx; ret"),
    "t85-enter": ([0xC8, 0x00, 0x00, 0x00, 0xC3], "enter 0,0; ret"),
    "t85-aaa":   ([0x37, 0xC3], "aaa; ret"),
    "t85-aas":   ([0x3F, 0xC3], "aas; ret"),
    "t85-daa":   ([0x27, 0xC3], "daa; ret"),
    "t85-das":   ([0x2F, 0xC3], "das; ret"),
    "t85-hlt":   ([0xF4, 0xC3], "hlt; ret"),
    "t85-into":  ([0xCE, 0xC3], "into; ret"),
    "t85-seto":  ([0x0F, 0x90, 0xC0, 0xC3], "seto al; ret"),
    "t85-setp":  ([0x0F, 0x9A, 0xC0, 0xC3], "setp al; ret"),
}
CONTROL_CASES = {           # decodable AND already lowered -> must not panic
    "t85-popf-ctl":  ([0x9D, 0xC3], "popf; ret"),
    "t85-leave-ctl": ([0xC9, 0xC3], "leave; ret"),
    "t85-cwd-ctl":   ([0x99, 0xC3], "cwd; ret"),
    "t85-ret-ctl":   ([0xB8, 0x01, 0x00, 0xC3], "mov ax,1; ret"),
}
(pathlib.Path("/tmp/repro") / "85-cases.txt").write_text(
    "".join(f"{n}: {d}\n" for n, (_, d) in {**PANIC_CASES, **CONTROL_CASES}.items()))
for name, (data, _desc) in {**PANIC_CASES, **CONTROL_CASES}.items():
    raw(name, bytes(data))

# Batch: one good function and one that panics, in the same code segment.
# Asks the question the issue does not: does the good function still get
# emitted when a sibling function hits the panic?
raw("t85batch", bytes([
    0xB8, 0x01, 0x00, 0xC3,   # 0000 good: mov ax,1; ret
    0xD7, 0xC3,               # 0004 bad:  xlat; ret
]))
(D / "t85batch.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    good { start 0000:0000 end 0000:0004 mode near ret None args None }\n"
    "    bad { start 0000:0004 end 0000:0006 mode near ret None args None }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
print("t85batch: good(0000:0000-0004) + bad(0000:0004-0006, xlat)")

# ------------------------------------------------------------------ #86 -----
DF_CASES = {
    # name: (bytes, description)
    "d1-cld-movsb":     ([0xFC, 0xA4, 0xC3], "cld; movsb; ret            (control: modeled)"),
    "d2-std-cld-movsb": ([0xFD, 0xFC, 0xA4, 0xC3], "std; cld; movsb; ret  (issue's example)"),
    "d3-std-movsb":     ([0xFD, 0xA4, 0xC3], "std; movsb; ret"),
    "d4-popf-movsb":    ([0x9D, 0xA4, 0xC3], "popf; movsb; ret"),
    "d5-iret-movsb":    ([0xCF, 0xA4, 0xC3], "iret; movsb; ret"),
    "d6-cld-repmovsb":  ([0xFC, 0xF3, 0xA4, 0xC3], "cld; rep movsb; ret   (control)"),
    "d7-std-cld-repmovsb": ([0xFD, 0xFC, 0xF3, 0xA4, 0xC3], "std; cld; rep movsb; ret"),
    "d8-cld-lodsb":     ([0xFC, 0xAC, 0xC3], "cld; lodsb; ret        (control)"),
    "d9-std-cld-lodsb": ([0xFD, 0xFC, 0xAC, 0xC3], "std; cld; lodsb; ret"),
    "d10-cld-outsb":    ([0xFC, 0x6E, 0xC3], "cld; outsb; ret        (control)"),
    "d11-std-cld-outsb": ([0xFD, 0xFC, 0x6E, 0xC3], "std; cld; outsb; ret"),
    "d12-cld-scasb":    ([0xFC, 0xAE, 0xC3], "cld; scasb; ret        (control)"),
    "d13-std-cld-scasb": ([0xFD, 0xFC, 0xAE, 0xC3], "std; cld; scasb; ret"),
    "d14-cld-stosb":    ([0xFC, 0xAA, 0xC3], "cld; stosb; ret        (control)"),
    "d15-std-cld-stosb": ([0xFD, 0xFC, 0xAA, 0xC3], "std; cld; stosb; ret"),
    # A std that lives on an unrelated path (never falls through to the movsb).
    "d16-std-on-side-path": ([
        0xEB, 0x01,       # 0000: jmp 0003
        0xFD,             # 0002: std      (skipped)
        0xFC, 0xA4, 0xC3  # 0003: cld; movsb; ret
    ], "jmp over a std; cld; movsb"),
}
(D / "86-cases.txt").write_text(
    "".join(f"{n}: {d}\n" for n, (_, d) in DF_CASES.items()))
for name, (data, _d) in DF_CASES.items():
    raw(name, bytes(data))

# ------------------------------------------------------------------ #69 -----
# The closure claim: every class the issue listed now has a real or partial
# lowering. Controls are ordered (opcode, description).
ISSUE69_CASES = {
    "r-rol":        ([0xD1, 0xC0, 0xC3], "rol ax,1"),
    "r-ror":        ([0xD1, 0xC8, 0xC3], "ror ax,1"),
    "r-rcl-stc":    ([0xF9, 0xD1, 0xD0, 0xC3], "stc; rcl ax,1      (carry known)"),
    "r-rcl-nocf":   ([0xD1, 0xD0, 0xC3], "rcl ax,1           (carry unknown)"),
    "r-rcr-stc":    ([0xF9, 0xD1, 0xD8, 0xC3], "stc; rcr ax,1      (carry known)"),
    "r-rcr-nocf":   ([0xD1, 0xD8, 0xC3], "rcr ax,1           (carry unknown)"),
    "r-adc8-clc":   ([0xF8, 0xB0, 0x01, 0x14, 0x02, 0xC3], "clc; mov al,1; adc al,2"),
    "r-adc8-stc":   ([0xF9, 0xB0, 0x01, 0x14, 0x02, 0xC3], "stc; mov al,1; adc al,2"),
    "r-sbb8-clc":   ([0xF8, 0xB0, 0x05, 0x1C, 0x02, 0xC3], "clc; mov al,5; sbb al,2"),
    "r-adc16-stc":  ([0xF9, 0xB8, 0x01, 0x00, 0x15, 0x02, 0x00, 0xC3], "stc; mov ax,1; adc ax,2"),
    "r-sbb16-clc":  ([0xF8, 0xB8, 0x05, 0x00, 0x1D, 0x02, 0x00, 0xC3], "clc; mov ax,5; sbb ax,2"),
    # Cross-block carry (#84): the producer (stc) is in block 0, the consumer
    # (adc) in block 1.
    "r-xblock-carry": ([
        0xF9,                   # 0000: stc
        0x74, 0x03,             # 0001: jz 0006   (two blocks)
        0xB8, 0x01, 0x00,       # 0003: mov ax,1
        0x15, 0x02, 0x00,       # 0006: adc ax,2   <- block 1, carry from block 0
        0xC3,                   # 0009: ret
    ], "stc; jz +3; mov ax,1; adc ax,2 (cross-block carry)"),
    "r-repmovsb":   ([0xF3, 0xA4, 0xC3], "rep movsb"),
    "r-repmovsw":   ([0xF3, 0xA5, 0xC3], "rep movsw"),
    "r-repstosb":   ([0xF3, 0xAA, 0xC3], "rep stosb"),
    "r-replodsb":   ([0xF3, 0xAC, 0xC3], "rep lodsb"),
    "r-repcmpsb":   ([0xF3, 0xA6, 0xC3], "rep cmpsb"),
    "r-repscasb":   ([0xF3, 0xAE, 0xC3], "rep scasb"),
    "r-insb":       ([0x6C, 0xC3], "insb"),
    "r-outsb":      ([0x6E, 0xC3], "outsb"),
    "r-in-al-imm":  ([0xE4, 0x21, 0xC3], "in al,21h"),
    "r-out-imm-al": ([0xE6, 0x21, 0xC3], "out 21h,al"),
    "r-sti":        ([0xFB, 0xC3], "sti"),
    "r-cli":        ([0xFA, 0xC3], "cli"),
    "r-cld":        ([0xFC, 0xC3], "cld"),
    "r-std":        ([0xFD, 0xC3], "std"),
    "r-stc":        ([0xF9, 0xC3], "stc"),
    "r-clc":        ([0xF8, 0xC3], "clc"),
    "r-cmc":        ([0xF5, 0xC3], "cmc"),
    "r-cbw":        ([0x98, 0xC3], "cbw"),
    # The "Related (optional)" item in #86: shift carry-out feeding an ADC.
    "r-shl-adc":    ([0xB8, 0xFF, 0x00, 0xD1, 0xE0, 0x15, 0x00, 0x00, 0xC3],
                     "mov ax,0x00ff; shl ax,1; adc ax,0"),
}
(D / "69-cases.txt").write_text(
    "".join(f"{n}: {d}\n" for n, (_, d) in ISSUE69_CASES.items()))
for name, (data, _d) in ISSUE69_CASES.items():
    raw(name, bytes(data))

print(f"\n{len(PANIC_CASES) + len(CONTROL_CASES)} #85 cases, {len(DF_CASES)} #86 cases, "
      f"{len(ISSUE69_CASES)} #69 cases")

# ----------------------------------------------------------- round 3b -------
# Follow-up probes: (a) are REP SCAS/CMPS/LODS/STOS lowered at all, or was the
# empty output just DCE? (make DI/SI live); (b) do the remaining SETcc forms
# panic? (c) does an STD *after* the string op poison it too? (d) is the DF
# guard per function?
PROBE_CASES = {
    "p-scasb-di":    ([0xF3, 0xAE, 0x89, 0xF8, 0xC3], "rep scasb; mov ax,di; ret"),
    "p-cmpsb-di":    ([0xF3, 0xA6, 0x89, 0xF8, 0xC3], "rep cmpsb; mov ax,di; ret"),
    "p-lodsb-si":    ([0xF3, 0xAC, 0x89, 0xF0, 0xC3], "rep lodsb; mov ax,si; ret"),
    "p-stosb-di":    ([0xF3, 0xAA, 0x89, 0xF8, 0xC3], "rep stosb; mov ax,di; ret"),
    "p-cmpsb-si":    ([0xF3, 0xA6, 0x89, 0xF0, 0xC3], "rep cmpsb; mov ax,si; ret"),
    "p-sets":        ([0x0F, 0x98, 0xC0, 0xC3], "sets al; ret"),
    "p-setns":       ([0x0F, 0x99, 0xC0, 0xC3], "setns al; ret"),
    "p-setno":       ([0x0F, 0x91, 0xC0, 0xC3], "setno al; ret"),
    "p-setnp":       ([0x0F, 0x9B, 0xC0, 0xC3], "setnp al; ret"),
    "p-seta":        ([0x0F, 0x97, 0xC0, 0xC3], "seta al; ret   (lowered control)"),
    "p-movsb-then-std": ([0xA4, 0xFD, 0xC3], "movsb; std; ret  (STD AFTER the op)"),
    "p-std-cld-repstosb": ([0xFD, 0xFC, 0xF3, 0xAA, 0xC3], "std; cld; rep stosb; ret"),
    "p-std-cld-repcmpsb": ([0xFD, 0xFC, 0xF3, 0xA6, 0xC3], "std; cld; rep cmpsb; ret"),
    "p-std-cld-replodsb": ([0xFD, 0xFC, 0xF3, 0xAC, 0xC3], "std; cld; rep lodsb; ret"),
}
(D / "3b-cases.txt").write_text(
    "".join(f"{n}: {d}\n" for n, (_, d) in PROBE_CASES.items()))
for name, (data, _d) in PROBE_CASES.items():
    raw(name, bytes(data))

# Per-function blast radius: f1 has no STD, f2 does. Codeseg-wide run asks
# whether f2's STD poisons f1.
raw("p-batch-df", bytes([
    0xFC, 0xA4, 0xC3,   # 0000 f1: cld; movsb; ret
    0xFD, 0xC3,         # 0003 f2: std; ret
]))
(D / "p-batch-df.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    f1 { start 0000:0000 end 0000:0003 mode near ret None args None }\n"
    "    f2 { start 0000:0003 end 0000:0005 mode near ret None args None }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
print("p-batch-df: f1(cld; movsb) + f2(std) — per-function blast-radius probe")

# ------------------------------------------------------------------ r5 ------
# Round-5 probes for the three round-3/4 residuals:
#   A. app.rs: is the "discard the whole run on one failure" path still
#      reachable, and does it really throw away the good functions?
#   B. #86: does `df_clear` reset to unknown at block boundaries (CLD in one
#      block, string op in the next)?
#   C. #78: `append_regargs` writes in annotation order; is there a matching
#      hazard on the return path (`retn AX DX`)?

# --- A. app.rs failure handling -------------------------------------------
# f_good decodes and lowers; f_inval's range covers 0x63 bytes, which decode to
# OP_INVAL -> the decoder errors (non-panicking) -> decompile_spec returns 1.
raw("a1-mixed", bytes([
    0xB8, 0x01, 0x00, 0xC3,   # 0000 good: mov ax,1; ret
    0x63, 0x63, 0x63,         # 0004 inval: OP_INVAL x3 (decode error)
]))
(D / "a1-mixed.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    good { start 0000:0000 end 0000:0004 mode near ret None args None }\n"
    "    inval { start 0000:0004 end 0000:0007 mode near ret None args None }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
# Same shape, but the bad function panics instead of returning 1
# (`jmp bx`: indirect jump with no resolved targets).
raw("a2-mixed-panic", bytes([
    0xB8, 0x01, 0x00, 0xC3,   # 0000 good: mov ax,1; ret
    0xFF, 0xE3, 0xC3,         # 0004 indirect: jmp bx; ret
]))
(D / "a2-mixed-panic.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    good { start 0000:0000 end 0000:0004 mode near ret None args None }\n"
    "    indirect { start 0000:0004 end 0000:0007 mode near ret None args None }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
print("a1-mixed / a2-mixed-panic: good + one failing function")

# --- B. DF across block boundaries ----------------------------------------
DF_BLOCK_CASES = {
    # CLD in block 0, movsb in block 1 (separated by an explicit jmp).
    "b1-cld-blk-movsb":  ([0xFC, 0xEB, 0x01, 0x90, 0xA4, 0xC3],
                          "cld; jmp 0004; nop; movsb; ret"),
    # movsb in block 0, STD in block 1 (writer comes after, other block).
    "b2-movsb-blk-std":  ([0xA4, 0xEB, 0x01, 0x90, 0xFD, 0xC3],
                          "movsb; jmp 0004; nop; std; ret"),
    # CLD + movsb in block 0, STD later in block 1.
    "b3-cld-movsb-blk-std": ([0xFC, 0xA4, 0xEB, 0x01, 0x90, 0xFD, 0xC3],
                          "cld; movsb; jmp 0005; nop; std; ret"),
    # The realistic shape: CLD once, then a movsb loop body.
    "b4-cld-movsb-loop": ([0xFC, 0x90, 0xA4, 0xE2, 0xFD, 0xC3],
                          "cld; nop; movsb; loop 0002; ret"),
    # movsb strictly before any writer (already known poisoned in r4).
    "b5-movsb-then-std": ([0xA4, 0xFD, 0xC3], "movsb; std; ret"),
    # STD in block 0, CLD + movsb in block 1.
    "b6-std-blk-cld-movsb": ([0xFD, 0xEB, 0x01, 0x90, 0xFC, 0xA4, 0xC3],
                          "std; jmp 0004; nop; cld; movsb; ret"),
    # Two paths: one STD, one CLD, then a joined movsb (must stay loud).
    "b7-two-paths-join": ([0x74, 0x03, 0xFD, 0xEB, 0x02, 0xFC, 0x90, 0xA4, 0xC3],
                          "jz 0005; std; jmp 0007; cld; nop; movsb; ret"),
    # Control: no DF writer anywhere.
    "b8-ctl-cld-movsb":  ([0xFC, 0xA4, 0xC3], "cld; movsb; ret"),
}
(D / "r5-df-cases.txt").write_text(
    "".join(f"{n}: {d}\n" for n, (_, d) in DF_BLOCK_CASES.items()))
for name, (data, _d) in DF_BLOCK_CASES.items():
    raw(name, bytes(data))

# --- C. materialization order ---------------------------------------------
# Caller: mov bx,ax; mov ah,6; call f; ret   (callee returns BX -> the wrong
# guest BX is what f observes).
(D / "c2-call-regargs-use.bin").write_bytes(bytes([
    0x89, 0xC3,             # 0000: mov bx,ax
    0xB4, 0x06,             # 0002: mov ah,6
    0xE8, 0x02, 0x00,       # 0004: call f (0009)
    0xC3,                   # 0007: ret
    0x90,                   # 0008: pad
    0x89, 0xD8,             # 0009: f: mov ax,bx
    0xC3,                   # 000b: ret
]))
(D / "c2-call-regargs-use.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    test { start 0000:0000 end 0000:0008 mode near ret u16 args None }\n"
    "    f { start 0000:0009 end 0000:000c mode near ret u16 args None regargs \"AX,BX\" }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
print("c2-call-regargs-use: callee reads BX (regargs AX,BX)")

# Annotation order reversed: does the emitted order follow the annotation?
(D / "c5-regargs-reversed.bin").write_bytes(bytes([
    0x89, 0xC3,             # mov bx,ax
    0xB4, 0x06,             # mov ah,6
    0xE8, 0x03, 0x00,       # call f (000a)
    0x89, 0xD8,             # mov ax,bx
    0xC3,                   # ret
    0xC3,                   # f: ret
]))
(D / "c5-regargs-reversed.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    test { start 0000:0000 end 0000:000a mode near ret None args None }\n"
    "    f { start 0000:000a end 0000:000b mode near ret None args None regargs \"BX,AX\" }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
print("c5-regargs-reversed: regargs BX,AX")

# Return path: mov dx,ax; mov ax,1; ret  (ret u32 -> vals are [AX, DX]).
raw("c3-ret-dxax", bytes([0x89, 0xC2, 0xB8, 0x01, 0x00, 0xC3]),
    fn="test")
(D / "c3-ret-dxax.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    test { start 0000:0000 end 0000:0006 mode near ret u32 args None }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
raw("c4-ret-dxax-ctl", bytes([0x89, 0xC2, 0xC3]), fn="test")
(D / "c4-ret-dxax-ctl.bsl").write_text(
    "dis86 {\n"
    "  code_segments { _0000 { seg 0 name CODE } }\n"
    "  functions {\n"
    "    test { start 0000:0000 end 0000:0003 mode near ret u32 args None }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
print("c3/c4: return-path DX:AX ordering (ret u32)")
