; fixture.asm - the standing parity fixture (#96): one self-contained entry
; function plus a one-instruction leaf reached by a near call, whose only
; observable surface is a byte-exact guest-memory "outbox". Both sides of
; the parity lane execute the SAME image:
;
;   golden : dosemu2 headless (-E fixture.exe -K <dir>, mapmshm conf),
;            parked forever on the sentinel; the lane captures the outbox
;            from live guest lowmem and kills the process.
;   native : dis86 --codegen-hydra output run through hydra + native_host;
;            the runner pre-releases the sentinel and stops on RESUME.
;
; Design rules (issue #96):
;   - Self-contained: sets its own ds/es/ss/sp, zero-fills its own outbox;
;     nothing dosemu or fdpp seeded is ever read, so environment skew
;     cannot leak into the diff.
;   - Zero INTs: `INT()` decompiles to hydra_impl_int, which executes via
;     raw code on the emulated CPU (dosemu-host-only seam). v1 has no INT
;     dispatch at all; the "exit-AL" analog is the outbox checksum lane
;     via a plain store (v3: lanes 0..38 arithmetic, lane 39 checksum),
;     and AH=4Ch/file-channel moves to a later version's lanes.
;   - Deterministic ops only, chosen to stay exactly-lowered (no UNIMPL on
;     either side): adc/sbb chains with explicit carry seeds, immediate
;     shifts/rotates, REP MOVSB/STOSW/CMPSB with a cld-ed DF, byte/word
;     MUL/IMUL/DIV/IDIV result lanes (no flag use after), byte/word
;     aliasing, one near call.
;   - The outbox lives inside the image and the reader scans for a
;     magic+version+count header (parity/outbox.h), so no absolute guest
;     address is hardcoded anywhere; load-segment skew between dosemu and
;     the native host cannot touch the diff.
;
; Addressing model (the hydra/src/dosemu_host/testprog_exe.asm lesson): the
; file is assembled with org 0 so labels carry FILE offsets, while the
; runtime segment covers only the load module (file bytes 0x20..). Every
; runtime memory reference uses the module-relative *_M numbers defined
; below; module offsets are always file offset - HDR_SIZE. Labels are used
; only by the assembler for ranges (never addressed from code).

BITS 16
        org 0

HDR_SIZE    equ 0x20            ; header bytes before the module
MODULE_SIZE equ 0x400           ; module bytes (pinned below via `times`)

; Layout constants (module-relative byte offsets). Mirror of
; parity/outbox.h - build.sh asserts PARK_M and OUTBOX_VERSION agree with
; its C defines.
OUTBOX_M    equ 0x0200          ; 16-byte outbox header
LANE_M      equ 0x0210          ; lanes (41 used of 64 reserved)
LANE_N      equ 41              ; lanes actually written
LANE_RESERVE equ 64             ; outbox.g cap (the scanner's expectation)
PARK_M      equ 0x0260          ; park sentinel byte
END_STALL_M equ 0x01CE          ; raw `jmp $` landing (outside both functions)
SRC_A_M     equ 0x0270          ; REP MOVSB source (8 bytes)
SRC_B_M     equ 0x0278          ; REP MOVSB dest  (8 bytes)
SRC_C_M     equ 0x0280          ; REP CMPSB run 2 (8 bytes)
FILL_M      equ 0x0288          ; REP STOSW fill (4 bytes)
ALIAS_M     equ 0x0290          ; byte/word aliasing cells
STACK_M     equ 0x0400          ; one past the image stack (grows down)
OUTBOX_VERSION equ 3

; MZ header (canonical RBIL Table 01403 layout, hand-rolled like testprog_
; exe.asm: reloc count @06, header paragraphs @08, ss/sp @0E/@10, ip/cs
; @14/@16, reloc table offset @18). File = header 0x20 + module 0x400 =
; 0x420 bytes -> e_cblp 0x20, e_cp 3. NASM cannot fold file-length
; arithmetic here; the pads below pin those values.
mz_header:
        db 'M', 'Z'                             ; e_magic
        dw 0x0020                               ; 02: e_cblp
        dw 3                                    ; 04: e_cp
        dw 0                                    ; 06: e_crlc: no relocations
        dw HDR_SIZE / 16                        ; 08: e_cparhdr
        dw 0                                    ; 0A: e_minalloc
        dw 0xFFFF                               ; 0C: e_maxalloc
        dw 0                                    ; 0E: e_ss (set by entry code)
        dw 0                                    ; 10: e_sp (set by entry code)
        dw 0                                    ; 12: e_csum (unchecked)
        dw 0                                    ; 14: e_ip (module 0)
        dw 0                                    ; 16: e_cs (module 0)
        dw 0x1E                                 ; 18: e_lfarlc: empty reloc table
        dw 0                                    ; 1A: e_ovno: no overlay
times HDR_SIZE-($-$$) db 0                       ; pad header to 2 paragraphs

; ---------------- module ----------------

parity_main:
        push cs
        pop ds
        push cs
        pop es
        cld
        push cs
        pop ss
        mov sp, STACK_M

        ; Zero the outbox header + lanes: the world we hand the reader is
        ; fully ours; nothing preloaded is trusted or read.
        mov di, OUTBOX_M
        mov cx, LANE_RESERVE + 16
        mov al, 0
        rep stosb

        ; ---- LANES 0..31 (each result is one outbox byte) ---------------
        ; L0/L1: byte ADC with both explicit carry-in seeds.
        clc
        mov al, 0x80
        adc al, 0x40            ; 0x80+0x40+0 = 0xC0
        mov [LANE_M+0], al
        stc
        mov al, 0x80
        adc al, 0x40            ; 0x80+0x40+1 = 0xC1
        mov [LANE_M+1], al

        ; L2/L3: byte SBB with both carry-in seeds.
        stc
        mov al, 0x10
        sbb al, 0x0F            ; 0x10-0x0F-1 = 0x00
        mov [LANE_M+2], al
        clc
        mov al, 0x10
        sbb al, 0x0F            ; 0x10-0x0F-0 = 0x01
        mov [LANE_M+3], al

        ; L4/L5: word ADC that must carry across the byte boundary.
        clc
        mov ax, 0xFF80
        adc ax, 0x0090          ; 0xFF80+0x90 = 0x0100+0x10... exact: 0x0010
        mov [LANE_M+4], al      ; 0x10
        mov [LANE_M+5], ah      ; 0x00

        ; L6: byte SHL imm-1 value lane.
        mov al, 0x81
        shl al, 1               ; 0x02
        mov [LANE_M+6], al

        ; L7: RCL consumes an explicitly seeded carry (STC, count 1).
        stc
        mov al, 0x40
        rcl al, 1               ; (0x40<<1)|1 = 0x81
        mov [LANE_M+7], al

        ; L8: RCR consumes an explicitly seeded carry-in bit.
        stc
        mov al, 0x03
        rcr al, 1               ; (3>>1)|(1<<7) = 0x81
        mov [LANE_M+8], al

        ; L9..L11: ROL/ROR value-exact; SHL imm-mask exact.
        mov al, 0xF0
        shl al, 3               ; (0xF0<<3)&255 = 0x80
        mov [LANE_M+9], al
        mov al, 0x61
        rol al, 3               ; 0110_0001 rol3 = 0000_1011 = 0x0B
        mov [LANE_M+10], al
        mov al, 0x61
        ror al, 3               ; 0110_0001 ror3 = 0010_1100? see lane check
        mov [LANE_M+11], al

        ; L12/L13: fill SRC_A from immediates; REP MOVSB SRC_A -> SRC_B.
        mov byte [SRC_A_M+0], 0xA0
        mov byte [SRC_A_M+1], 0xA1
        mov byte [SRC_A_M+2], 0xA2
        mov byte [SRC_A_M+3], 0xA3
        mov byte [SRC_A_M+4], 0xA4
        mov byte [SRC_A_M+5], 0xA5
        mov byte [SRC_A_M+6], 0xA6
        mov byte [SRC_A_M+7], 0xA7
        mov si, SRC_A_M
        mov di, SRC_B_M
        mov cx, 8
        rep movsb
        mov ax, si              ; SRC_A_M+8 = 0x0278
        mov [LANE_M+12], al     ; 0x78
        mov [LANE_M+13], ah     ; 0x02

        ; L14/L15: REP STOSW fills FILL_M (di advances by 4).
        mov ax, 0xBEEF
        mov di, FILL_M
        mov cx, 2
        rep stosw
        mov ax, di              ; FILL_M+4 = 0x028C
        mov [LANE_M+14], al     ; 0x8C
        mov [LANE_M+15], ah     ; 0x02

        ; L16..L19: byte/word aliasing over the same cells.
        mov word [ALIAS_M], 0x1234
        mov al, [ALIAS_M]       ; 0x34
        mov [LANE_M+16], al
        mov ax, [ALIAS_M]       ; 0x1234
        mov [LANE_M+17], al     ; 0x34 again via a word load
        mov ah, [ALIAS_M+1]     ; 0x12 into AH
        mov [LANE_M+18], ah
        mov dl, [ALIAS_M]       ; alias read lands in DL
        mov [LANE_M+19], dl

        ; L20/L21: WORD MUL result lane (DX:AX = AX*r/m; no flag use after).
        mov ax, 0x0060
        mov cx, 3
        mul cx                  ; DX:AX = 0x0001:0x0120
        mov [LANE_M+20], al     ; 0x20
        mov [LANE_M+21], ah     ; 0x01

        ; L22..L25: register-movement conventions that IR lowers exactly
        ; (native xchg reg-reg plus a push/pop round-trip through the image
        ; stack).
        mov ax, 0x1234
        mov dx, 0x5678
        xchg ax, dx             ; AX=0x5678, DX=0x1234
        mov [LANE_M+22], al     ; 0x78
        push dx
        pop cx                  ; CX = 0x1234
        mov [LANE_M+23], ch     ; 0x12
        mov [LANE_M+24], cl     ; 0x34
        mov [LANE_M+25], dl     ; 0x34

        ; L26: near-call convention (leaf returns AX).
        call leaf
        mov [LANE_M+26], al     ; 0x7A
        mov al, ah
        mov [LANE_M+27], al     ; 0x11

        ; L28/L29: REP CMPSB over two equal 8-byte runs (cx exhausts to 0).
        mov si, SRC_A_M
        mov di, SRC_C_M
        mov cx, 8
        rep movsb               ; copy SRC_A -> SRC_C for equal runs
        mov si, SRC_B_M
        mov di, SRC_C_M
        mov cx, 8
        repe cmpsb
        mov ax, si              ; SRC_B_M+8 = 0x0280
        mov [LANE_M+28], al
        mov ax, di              ; SRC_C_M+8 = 0x0288
        mov [LANE_M+29], al

        ; L30: byte SAR (the previously-filed sign-loss gap): hardware
        ; widens 0xF0 as -16, so sar by 1 yields 0xF8 - not (0x78, the
        ; masked-source value the old lowering shifted).
        mov al, 0xF0
        sar al, 1               ; signed 240 -> -8 = 0xF8
        mov [LANE_M+30], al

        ; L31/L32: byte MUL - the previously-panicked form: the product is
        ; AL*r/m8, 16 bits, and every bit lands in AX (0x81 * 8 = 0x0408).
        mov al, 0x81
        mov bl, 8
        mul bl
        mov [LANE_M+31], al     ; 0x08
        mov [LANE_M+32], ah     ; 0x04

        ; L33/L34: byte IMUL - signed: 0xF0 reads as -16, * 2 = -32 =
        ; 0xFFE0 (the unsigned product would put 0x01 in AH instead of FF).
        mov al, 0xF0
        mov bl, 2
        imul bl
        mov [LANE_M+33], al     ; 0xE0
        mov [LANE_M+34], ah     ; 0xFF

        ; L35/L36: word DIV - quotient in AX, exact remainder in DX
        ; (0x123456 / 0x1000 = 291 r 1110).
        mov dx, 0x0012
        mov ax, 0x3456
        mov cx, 0x1000
        div cx
        mov [LANE_M+35], al     ; 0x23 (AX = 0x0123)
        mov [LANE_M+36], dl     ; 0x56 (DX = 0x0456)

        ; L37/L38: word IDIV - signed: DX:AX = 0xFFFFFF66 = -154, /3 =
        ; -51 r -1 (truncation toward zero; the remainder keeps the
        ; dividend's sign - a floor division would answer -52 r 2).
        mov dx, 0xFFFF
        mov ax, 0xFF66
        mov cx, 3
        idiv cx
        mov [LANE_M+37], al     ; 0xCD (AX = 0xFFCD)
        mov [LANE_M+38], dl     ; 0xFF (DX = 0xFFFF)

        ; L39: checksum ("exit-AL" analog) = sum of lanes 0..38 mod 256.
        mov si, LANE_M
        mov cx, 39
        mov ax, 0
.sum:
        add al, [si]
        inc si
        dec cx
        jnz .sum
        mov [LANE_M+39], al

        ; Write the outbox header LAST: the reader scans for it, so its
        ; presence alone proves a complete world.
        mov byte [OUTBOX_M+0], 'P'
        mov byte [OUTBOX_M+1], 'H'
        mov byte [OUTBOX_M+2], 'Y'
        mov byte [OUTBOX_M+3], 'P'
        mov byte [OUTBOX_M+4], 'A'
        mov byte [OUTBOX_M+5], 'R'
        mov byte [OUTBOX_M+6], '1'
        mov byte [OUTBOX_M+7], '6'
        mov byte [OUTBOX_M+8], OUTBOX_VERSION
        mov byte [OUTBOX_M+9], LANE_RESERVE

        ; Park: the golden side spins here until the lane kills the process
        ; after capture; the native side falls through immediately.
.park:
        cmp byte [PARK_M], 0
        je .park

        ; Deterministic landing: return to a raw spin parked OUTSIDE both
        ; annotated functions. Golden never reaches here (it parks above);
        ; if the sentinel is ever released there, the spin leaves the
        ; outbox final. The native side pops its return ip from the seeded
        ; image word instead (see parity/native_runner.c) and stops on the
        ; hook miss at end_stall (hydra_exec_run -> RESUME -> runner loop
        ; ends) without ever executing the raw bytes.
        mov ax, END_STALL_M
        push ax
        ret

leaf:
        mov ax, 0x117A
        ret

end_stall:
        jmp $

; ---------------- trace data (all outside the annotated range) -----------
; The stack sentinel word pins the near-ret landing at a known image word.
; The layout puts it at module 0x03FE, exactly one below STACK_M.

; Pad the file to HDR_SIZE + MODULE_SIZE exactly (0x420): the trailing
; times includes the module's zero tail, of which bytes [0x3FE..0x400] are
; the ret-popped sentinel word (zeros exactly as both engines see them).
times HDR_SIZE + MODULE_SIZE - ($ - $$) db 0
img_end:
