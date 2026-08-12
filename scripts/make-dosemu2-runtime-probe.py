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

# Short, deterministic program used by the end-to-end validator check.  Keep
# this instruction stream deliberately conservative: both emu86 and dosemu2's
# interpreter execute the register moves, arithmetic, and DOS exit interrupt.
# Unlike CODE, it reaches a normal target-exit boundary without relying on the
# validator's shutdown barrier to escape a safety loop.
TERMINATING_CODE = bytes.fromhex(
    "b8 34 12 "       # mov ax,1234h
    "bb ff 00 "       # mov bx,00ffh
    "01 d8 "          # add ax,bx
    "31 d2 "          # xor dx,dx
    "b8 00 4c "       # mov ax,4c00h
    "eb 00 "          # force a translated-node boundary before termination
    "cd 21"           # int 21h (DOS terminate process)
)

# Runtime lifecycle probe: one ordinary instruction followed by the DOS
# terminate-process service.  Keeping the interrupt at its own node boundary
# lets the hook publish TARGET_EXIT before DOS takes control.
TARGET_EXIT_CODE = bytes.fromhex(
    "b8 00 4c "       # mov ax,4c00h
    "cd 21"           # int 21h (DOS terminate process)
)

# Runtime fault probe: establish an unmistakable register snapshot and then
# execute unsigned division by zero.  DIV raises architectural exception 0 in
# simx86, which the validator hook must publish as DIIS_STEP_FAULT.
FAULT_CODE = bytes.fromhex(
    "b8 34 12 "       # mov ax,1234h
    "31 d2 "          # xor dx,dx
    "31 db "          # xor bx,bx
    "f7 f3"           # div bx
)

assert len(CODE) == 18
assert CODE[16:18] == b"\x11\x11"


def build_mz(code: bytes = CODE) -> bytes:
    header_paragraphs = 2
    header_size = header_paragraphs * 16
    total_size = header_size + len(code)
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
    # emu86's current MZ loader maps the complete page range declared by e_cp,
    # so keep the compact fixture internally consistent with that loader as
    # well as with DOS.  Retain e_cblp's logical image length while supplying
    # harmless trailing bytes through the end of the declared page.
    return (header + code).ljust(pages * 512, b"\x00")


def main() -> int:
    args = sys.argv[1:]
    mode = "end-barrier"
    modes = {
        "--terminating": "terminating",
        "--target-exit": "target-exit",
        "--fault": "fault",
    }
    if args[:1] and args[0] in modes:
        mode = modes[args[0]]
        args = args[1:]
    if len(args) != 1:
        print(
            f"usage: {sys.argv[0]} [--terminating|--target-exit|--fault] OUTPUT.EXE",
            file=sys.stderr,
        )
        return 2
    out = pathlib.Path(args[0])
    out.parent.mkdir(parents=True, exist_ok=True)
    code = {
        "end-barrier": CODE,
        "terminating": TERMINATING_CODE,
        "target-exit": TARGET_EXIT_CODE,
        "fault": FAULT_CODE,
    }[mode]
    data = build_mz(code)
    out.write_bytes(data)
    kind = f"{mode} fixture"
    print(f"wrote {out} ({len(data)} bytes, entry CS:IP=0000:0000, {kind})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
