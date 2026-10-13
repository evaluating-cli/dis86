#!/usr/bin/env python3
"""Generate the synthetic MZ repros used by the issue-verification workflow.

These reproduce the exact programs published in issues #65, #66, #67 and a
small #69 sample. Nothing here depends on the repository; the files are
written to $OUT (default: the current directory).
"""
import os
import struct

OUT = os.environ.get("OUT", ".")


def mz(load: bytes, path: str) -> None:
    """Assemble a minimal MZ around `load` (entry 0000:0000, 4-paragraph header)."""
    filesize = 0x40 + len(load)
    hdr = bytearray(0x40)
    # MZ, last-page bytes, pages, relocs=0, hdr paragraphs=4, min/max alloc,
    # ss, sp, csum, ip, cs, lfarlc, ovno
    struct.pack_into('<2sHHHHHHHHHHHHH', hdr, 0,
                     b'MZ', filesize % 512 or 512, (filesize + 511) // 512,
                     0, 4, 0, 0xFFFF, 0, 0xFFFE, 0, 0, 0, 0x40, 0)
    with open(path, 'wb') as f:
        f.write(bytes(hdr) + load)
    print(f"wrote {path}: {filesize} bytes (load {len(load)})")


# ---------------------------------------------------------------- issue #65
# Entry jumps over 3 embedded data bytes; --analyze infers a range that spans
# them, and the linear decoder then aborts at 0000:0003.
issue65 = bytes([
    0xE9, 0x03, 0x00,     # 0000: jmp 0006   (skip 0003..0005)
    0x63, 0x63, 0x63,     # 0003: 3 data bytes (0x63 decodes to OP_INVAL)
    0xB8, 0x00, 0x4C,     # 0006: mov ax,4c00
    0xCD, 0x21,           # 0009: int 21h
])
mz(issue65, os.path.join(OUT, "synth_65.exe"))

issue65_cfg = """dis86 {
  code_segments { _0000 { seg 0 name CODE } }
  functions { main { start 0000:0000 end 0000:000b mode near ret None args None } }
  structures {}
  globals {}
  text_section {}
}
"""

# Range corrected to the real code entry (issue #65 workaround).
issue65_fixed_cfg = """dis86 {
  code_segments { _0000 { seg 0 name CODE } }
  functions { main { start 0000:0006 end 0000:000b mode near ret None args None } }
  structures {}
  globals {}
  text_section {}
}
"""

# ---------------------------------------------------------------- issue #66
# mov ah,9; mov dx,0x000b; int 21h; mov ax,0x4c00; int 21h; "Hi$"
issue66 = bytes([
    0xB4, 0x09,             # mov ah,9
    0xBA, 0x0B, 0x00,       # mov dx,0x000b
    0xCD, 0x21,             # int 21h   (print "Hi$")
    0xB8, 0x00, 0x4C,       # mov ax,0x4c00
    0xCD, 0x21,             # int 21h   (terminate, exit code 0)
    0x48, 0x69, 0x24,       # "Hi$"
])
mz(issue66, os.path.join(OUT, "synth_66.exe"))

issue66_cfg = """dis86 {
  code_segments { _0000 { seg 0 name CODE } }
  functions { main { start 0000:0000 end 0000:000c mode near ret None args None } }
  structures {}
  globals {}
  text_section {}
}
"""

# ---------------------------------------------------------------- issue #67
# A tail-jumps into B; A's declared range ends before the target.
issue67 = bytes([
    0xC7, 0x06, 0x00, 0x01, 0x01, 0x00,  # 0: A: mov word [0x100],1
    0xE9, 0x01, 0x00,                    # 6: jmp 0x000a (outside A's range)
    0x90,                                # 9: nop
    0xBB, 0x02, 0x00,                    # a: B: mov bx,2
    0xC3,                                # d: ret
    0x90,                                # e: pad
])
mz(issue67, os.path.join(OUT, "synth_67.exe"))

issue67_cfg = """dis86 {
  code_segments { _0000 { seg 0 name CODE } }
  functions {
    A { start 0000:0000 end 0000:0009 mode near ret None args None }
    B { start 0000:000a end 0000:000e mode near ret None args None }
  }
  structures {}
  globals {}
  text_section {}
}
"""

# ---------------------------------------------------------------- issue #69
# adc/sbb/rol/rep movsb/stc: the UNIMPL lowering table.
issue69 = bytes([
    0x11, 0xC3,             # adc ax, bx
    0x1B, 0xC0,             # sbb ax, ax
    0xD0, 0xC0,             # rol al, 1
    0xF3, 0xA4,             # rep movsb
    0xF9,                   # stc
    0xC3,                   # ret
])
mz(issue69, os.path.join(OUT, "synth_69.exe"))

issue69_cfg = """dis86 {
  code_segments { _0000 { seg 0 name CODE } }
  functions { main { start 0000:0000 end 0000:0009 mode near ret None args None } }
  structures {}
  globals {}
  text_section {}
}
"""

for name, body in [("65", issue65_cfg), ("65_fixed", issue65_fixed_cfg),
                   ("66", issue66_cfg), ("67", issue67_cfg), ("69", issue69_cfg)]:
    path = os.path.join(OUT, f"repro{name}.bsl")
    with open(path, "w") as f:
        f.write(body)
    print(f"wrote {path}")
