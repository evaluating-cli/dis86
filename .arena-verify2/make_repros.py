#!/usr/bin/env python3
"""Build every synthetic input used by the re-verification run.

Re-checks the closed issues #65/#66/#67 (same inputs as the issue reports) and
verifies the two new issues #77/#78 (raw binaries + configs copied verbatim
from the reports).  Everything lands in /tmp/repro.
"""
import pathlib
import struct

D = pathlib.Path("/tmp/repro")
D.mkdir(exist_ok=True)


def mz(load: bytes) -> bytes:
    hdrbytes = 0x40
    filesize = hdrbytes + len(load)
    cblp = filesize % 512 or 512
    cp = (filesize + 511) // 512
    hdr = bytearray(hdrbytes)
    # MZ, last-page bytes, pages, relocs=0, hdr paragraphs=4, min/max alloc,
    # ss, sp, csum, ip, cs, lfarlc, ovno
    struct.pack_into('<2sHHHHHHHHHHHHH', hdr, 0,
                     b'MZ', cblp, cp, 0, 4, 0, 0xFFFF, 0, 0xFFFE, 0, 0, 0, 0x40, 0)
    return bytes(hdr) + load


def cfg(path, funcs, seg="CODE"):
    out = f"dis86 {{\n  code_segments {{ _0000 {{ seg 0 name {seg} }} }}\n  functions {{\n"
    for name, (start, end) in funcs.items():
        out += f"    {name} {{ start 0000:{start:04x} end 0000:{end:04x} mode near ret None args None }}\n"
    out += "  }\n  structures {}\n  globals {}\n  text_section {}\n}\n"
    pathlib.Path(path).write_text(out)


def raw(name: str, data: bytes, funcs, seg="CODE"):
    (D / f"{name}.bin").write_bytes(data)
    cfg(D / f"{name}.bsl", funcs, seg)
    print(f"{name}: {len(data)} bytes -> {name}.bin + {name}.bsl")


# ---------------------------------------------------------------- #65 -------
load65 = bytes([
    0xE9, 0x03, 0x00,     # 0000: jmp 0006   (skip 0003..0005)
    0x63, 0x63, 0x63,     # 0003: 3 data bytes (0x63 decodes to OP_INVAL)
    0xB8, 0x00, 0x4C,     # 0006: mov ax,4c00
    0xCD, 0x21,           # 0009: int 21h
])
(D / "synth_65.exe").write_bytes(mz(load65))
cfg(D / "r65.bsl", {"main": (0x0, 0xB)})
cfg(D / "r65_fixed.bsl", {"main": (0x6, 0xB)})
print(f"synth_65.exe: {0x40 + len(load65)} bytes")

# ---------------------------------------------------------------- #66 -------
load66 = bytes([
    0xB4, 0x09,             # mov ah,9
    0xBA, 0x0B, 0x00,       # mov dx,0x000b
    0xCD, 0x21,             # int 21h   (print "Hi$")
    0xB8, 0x00, 0x4C,       # mov ax,0x4c00
    0xCD, 0x21,             # int 21h   (terminate, exit code 0)
    0x48, 0x69, 0x24,       # "Hi$"
])
(D / "synth_a.exe").write_bytes(mz(load66))
cfg(D / "r66.bsl", {"main": (0x0, 0xC)})
print(f"synth_a.exe: {0x40 + len(load66)} bytes")

# #66 follow-up from the posted comment: flags after int must not fold silently.
raw("t66b", bytes([0x29, 0xC0, 0xCD, 0x21, 0x74, 0x01, 0xC3]),  # sub ax,ax; int 21h; jz +1; ret
    {"test": (0x0, 0x7)})

# ---------------------------------------------------------------- #67 -------
load67 = bytes([
    0xC7, 0x06, 0x00, 0x01, 0x01, 0x00,  # 0: A: mov word [0x100],1
    0xE9, 0x01, 0x00,                    # 6: jmp 0x000a  (into B, outside A's range)
    0x90,                                # 9: nop
    0xBB, 0x02, 0x00,                    # a: B: mov bx,2
    0xC3,                                # d: ret
    0x90,                                # e: pad
])
(D / "synth_b.exe").write_bytes(mz(load67))
cfg(D / "r67.bsl", {"A": (0x0, 0x9), "B": (0xA, 0xE)})
print(f"synth_b.exe: {0x40 + len(load67)} bytes")

# ---------------------------------------------------------------- #77 -------
# Case 1: mov ax,0x0607; mov ah,0x4A; ret   (high-byte leak)
raw("t77a", bytes([0xB8, 0x07, 0x06, 0xB4, 0x4A, 0xC3]), {"test": (0x0, 0x6)})
# Case 2: mov ax,0x1234; mov al,0x56; ret   (high-byte drop)
raw("t77b", bytes([0xB8, 0x34, 0x12, 0xB0, 0x56, 0xC3]), {"test": (0x0, 0x6)})
# Extra: mov ax,0x1234; mov [0x100],ah; ret  (fragment READ after a full write)
raw("t77c", bytes([0xB8, 0x34, 0x12, 0x88, 0x26, 0x00, 0x01, 0xC3]), {"test": (0x0, 0x8)})

# ---------------------------------------------------------------- #78 -------
# Reported: mov bx,ax; mov ah,6; int 21h; mov ax,bx; ret
raw("t78a", bytes([0x89, 0xC3, 0xB4, 0x06, 0xCD, 0x21, 0x89, 0xD8, 0xC3, 0x90, 0x90, 0x90]),
    {"test": (0x0, 0xC)})
# Control (dest materialized first): mov ax,bx; mov bh,1; int 21h; ret
raw("t78b", bytes([0x89, 0xD8, 0xB7, 0x01, 0xCD, 0x21, 0xC3]), {"test": (0x0, 0x7)})
# Variant (dest materialized last): mov si,cx; mov ch,1; int 21h; ret
raw("t78c", bytes([0x89, 0xCE, 0xB5, 0x01, 0xCD, 0x21, 0xC3]), {"test": (0x0, 0x7)})
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
    "    main { start 0000:0000 end 0000:000a mode near ret None args None }\n"
    "    f { start 0000:000a end 0000:000b mode near ret None args None regargs \"AX,BX\" }\n"
    "  }\n"
    "  structures {}\n"
    "  globals {}\n"
    "  text_section {}\n"
    "}\n")
print("t78d: 11 bytes -> t78d.bin + t78d.bsl (call-site regargs AX,BX)")
