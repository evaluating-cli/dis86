#!/usr/bin/env python3
import pathlib
import struct
import sys

# Probe image, loaded at MZ CS:IP 0000:0000.
#
#   mov ax, cs:[0010]       ; external -> guest visibility
#   inc ax
#   mov cs:[0010], ax       ; guest -> external visibility
#   inc word cs:[0010]      ; must NOT execute after END_ACK request
#   jmp $                   ; safety loop if the barrier is broken later
#   dw 1111h                ; shared sentinel at image offset 0010h
CODE = bytes.fromhex(
    "2e a1 10 00 "
    "40 "
    "2e a3 10 00 "
    "2e ff 06 10 00 "
    "eb fe "
    "11 11"
)

assert len(CODE) == 18
assert CODE[16:18] == b"\x11\x11"


def build_mz() -> bytes:
    header_paragraphs = 2
    header_size = header_paragraphs * 16
    total_size = header_size + len(CODE)
    pages = (total_size + 511) // 512
    last_page = total_size % 512

    # DOS MZ header through e_ovno (14 little-endian u16 words / 28 bytes).
    words = (
        0x5A4D,             # e_magic
        last_page,          # e_cblp
        pages,              # e_cp
        0,                  # e_crlc
        header_paragraphs,  # e_cparhdr
        0x0020,             # e_minalloc (512 bytes)
        0xFFFF,             # e_maxalloc
        0x0000,             # e_ss relative to load module
        0x0200,             # e_sp
        0x0000,             # e_csum
        0x0000,             # e_ip
        0x0000,             # e_cs
        0x001C,             # e_lfarlc
        0x0000,             # e_ovno
    )
    header = struct.pack("<14H", *words)
    header += b"\x00" * (header_size - len(header))
    assert len(header) == 32
    return header + CODE


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} OUTPUT.EXE", file=sys.stderr)
        return 2
    out = pathlib.Path(sys.argv[1])
    out.parent.mkdir(parents=True, exist_ok=True)
    data = build_mz()
    out.write_bytes(data)
    print(f"wrote {out} ({len(data)} bytes, entry CS:IP=0000:0000, sentinel=0010h)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
