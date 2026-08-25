; launch.asm - real-mode loader stub for the Phase 7 Item C test.
;
; Purpose: get TESTPROG.EXE loaded by DOS as a real child process and let
; the host take control at its relocated entry with no executed image byte
; and no JIT history.
;
; Why not dosemu2's own bpload/DBGload machinery: stock fdpp never
; publishes the INT21 AH=4B01 results (initial SS:SP / entry CS:IP, RBIL
; EXEC-paramblock offsets 0Eh/12h) back into guest memory - neither into
; the caller's parameter block nor into dosemu's DBGload block in the BIOS
; segment - so dosemu's stub jumps to 0000:0000 and kills dosemu with
; SIGILL, and a launcher-side TF handoff cannot stop the debugger either
; (trapcmd is only armed by bpload's INT3 or 't' commands; otherwise INT1
; reflects through the guest IVT). Verified against dosemu2 2.0pre9 +
; libfdpp 1.11 (2026-08). This launcher therefore performs the standard
; RBIL sequence itself and hands the published child PSP to the host,
; which constructs and validates the entry state (see test_driver_exe.c /
; host_driver.c).
;
; Protocol with the host (BDA scratch bytes 0040:00F4-00F9):
;   1. comcom32 EXECs us normally during autoexec (nothing is armed).
;   2. mark 0040:00F5 = A5 ("resident"), spin until 0040:00F4 != 0 ("go").
;   3. shrink own allocation (a .COM owns all free RAM; the child load
;      fails with error 8 otherwise), then INT21 AH=4B01 (load-don't-
;      execute) TESTPROG.EXE. On failure: 0040:00F6 = 7E, error code in
;      0040:00F7, INT 20h. On resize failure: 0040:00F6 = 5E, INT 20h.
;   4. on success: fetch the child PSP (AH=62h; DOS made the child the
;      current process), publish it as a word at 0040:00F8, mark
;      0040:00F6 = 5A ("loaded") and spin forever. The host reads the PSP
;      over shared memory, verifies the loaded image, parks the machine
;      and pushes the child entry register state itself.

BITS 16
        org 100h

start:
        push cs
        pop ds

        ; announce presence
        push bx
        push ax
        mov bx, 0x40
        mov ds, bx
        mov byte [0xF5], 0xA5
        pop ax
        pop bx
        push cs
        pop ds

        ; fill runtime pointer fields of the EXEC parameter block
        mov [parm+4], ds        ; cmdtail segment
        mov [parm+8], ds        ; fcb1 segment (offset PSP:5Ch)
        mov [parm+0xC], ds      ; fcb2 segment (offset PSP:6Ch)

wait_go:
        push ds
        mov ax, 0x40
        mov ds, ax
        mov al, [0xF4]
        pop ds
        test al, al
        jz wait_go

        ; shrink own allocation: a .COM inherits all free memory, leaving
        ; nothing for the child (EXEC fails with error 8 otherwise).
        push cs
        pop es                          ; for a .COM, CS == PSP
        mov bx, ((prog_end - start + 0x100 + 15) / 16) + 6
        mov ah, 0x4A
        int 21h
        jnc resized_ok
        mov ax, 0x40
        mov ds, ax
        mov byte [0xF6], 0x5E           ; "resize failed"
        int 20h
resized_ok:
        push cs
        pop ds

        ; DS:DX = ASCIZ path, ES:BX = parameter block, AH=4B01 (load only)
        mov dx, pathstr
        push ds
        pop es
        mov bx, parm
        mov ax, 0x4B01
        int 21h
        jnc load_ok

load_failed:
        push ax
        mov ax, 0x40
        mov ds, ax
        mov byte [0xF6], 0x7E           ; "load failed"
        mov [0xF7], al                  ; DOS error code
        pop ax
        int 20h

load_ok:
        ; publish the child PSP and idle; the host takes it from here
        mov ah, 0x62            ; current PSP == loaded child's PSP
        int 21h                 ; BX = child PSP
        push bx
        mov ax, 0x40
        mov ds, ax
        mov [0xF8], bx                  ; word: child PSP segment
        mov byte [0xF6], 0x5A           ; "loaded"
        pop bx
idle:
        jmp idle

pathstr: db 'TESTPROG.EXE', 0

parm:   dw  0                   ; +0 : environment segment (0 = inherit)
        dw  tail, 0             ; +2 : command tail far pointer (seg @+4)
        dw  0x5C, 0             ; +6 : FCB1 far pointer (seg @+8)
        dw  0x6C, 0             ; +A : FCB2 far pointer (seg @+C)
        dw  0, 0                ; +E : (4B01) SS:SP save pointer
        dw  0, 0                ; +12: (4B01) entry CS:IP
tail:   db  0, 0Dh              ; empty command line: count=0, CR
prog_end:
