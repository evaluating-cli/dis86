#!/usr/bin/env python3
"""Second-round repros: how register values reach the guest in emitted C, and
what happens to flags after an INT. Writes to $OUT (default: /tmp/verify2)."""
import os
import struct

OUT = os.environ.get("OUT", "/tmp/verify2")
os.makedirs(OUT, exist_ok=True)


def mz(load: bytes, path: str) -> None:
    filesize = 0x40 + len(load)
    hdr = bytearray(0x40)
    struct.pack_into('<2sHHHHHHHHHHHHH', hdr, 0,
                     b'MZ', filesize % 512 or 512, (filesize + 511) // 512,
                     0, 4, 0, 0xFFFF, 0, 0xFFFE, 0, 0, 0, 0x40, 0)
    with open(path, 'wb') as f:
        f.write(bytes(hdr) + load)
    print(f"wrote {path}: {filesize} bytes")


# A: caller sets AX then calls a callee annotated `regargs ax`; the callee
#    returns immediately. Shows how an annotated register argument is
#    materialized in the emitted C.
binary_a = bytes([
    0xB8, 0x34, 0x12,     # 0000: mov ax,0x1234
    0xE8, 0x01, 0x00,     # 0003: call 0007
    0xC3,                 # 0006: ret
    0xC3,                 # 0007: ret (callee)
])
mz(binary_a, os.path.join(OUT, "regargs.exe"))

cfg_a = """dis86 {
  code_segments { _0000 { seg 0 name CODE } }
  functions {
    caller { start 0000:0000 end 0000:0007 mode near ret None args None }
    callee { start 0000:0007 end 0000:0008 mode near ret None args None regargs AX }
  }
  structures {}
  globals {}
  text_section {}
}
"""

# B: plain register write then ret. Does the callee-saved write reach the
#    guest registers in the emitted C (pin_registers at ret)?
binary_b = bytes([
    0xBB, 0x05, 0x00,     # mov bx,5
    0xC3,                 # ret
])
mz(binary_b, os.path.join(OUT, "movbx.exe"))

cfg_b = """dis86 {
  code_segments { _0000 { seg 0 name CODE } }
  functions { main { start 0000:0000 end 0000:0004 mode near ret None args None } }
  structures {}
  globals {}
  text_section {}
}
"""

# C: flags producer, then INT (which clobbers flags at runtime), then JZ.
#    Does the emitted branch still fold against the pre-INT flags producer?
binary_c = bytes([
    0x29, 0xC0,           # 0000: sub ax,ax   (ZF=1)
    0xCD, 0x21,           # 0002: int 21h     (flags clobbered at runtime)
    0x74, 0x01,           # 0004: jz 0007
    0xC3,                 # 0006: ret
    0xC3,                 # 0007: ret
])
mz(binary_c, os.path.join(OUT, "flags.exe"))

cfg_c = """dis86 {
  code_segments { _0000 { seg 0 name CODE } }
  functions { main { start 0000:0000 end 0000:0008 mode near ret None args None } }
  structures {}
  globals {}
  text_section {}
}
"""

for name, body in [("a", cfg_a), ("b", cfg_b), ("c", cfg_c)]:
    path = os.path.join(OUT, f"cfg{name}.bsl")
    with open(path, "w") as f:
        f.write(body)
    print(f"wrote {path}")
