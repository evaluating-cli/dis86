; testprog.asm - guest program for Hydra-on-dosemu2 integration testing.
;
; Besides function hooking/callthrough, this fixture reserves an 8 KiB block
; inside its own COM image for Hydra raw-code slots. The host locates the
; 16-byte marker and explicitly registers the following bytes as scratch; no
; arbitrary DOS low-memory address is commandeered.

BITS 16
        org 100h

start:
        jmp short waitloop

goflag:    db  0
result:    dw  0
hookcnt:   dw  0
result2:   dw  0
res3:      dw  0
res4:      dw  0
flagres:   dw  0
ifflagres: dw  0

waitloop:
        mov al, [goflag]
        test al, al
        jz waitloop

mainloop:
        call myfunc
        mov [result], ax

        call func2
        mov [result2], ax

        ; Exercise a complete hook round-trip while guest IF is clear. The
        ; host must preserve guest-visible IF=0 even though dosemu keeps its
        ; physical vm86 IF enabled internally.
        cli
        call func2
        pushf
        pop bx
        mov [ifflagres], bx
        sti

        stc
        call callthru
        mov [res3], ax
        pushf
        pop bx
        mov [flagres], bx
        jmp mainloop

myfunc:
        mov ax, 0x1111
        ret

func2:
        mov ax, 0x2222
        ret

callthru:
        mov ax, 0x3333
        ret

; helper2: plain native->guest callthrough, 13 instructions, AX=0x7ACE.
helper2:
        mov ax, 0x7ACE
        push bx
        mov bx, cx
        xchg bx, dx
        push dx
        pop dx
        xchg dx, bx
        mov cx, bx
        nop
        nop
        nop
        nop
        pop bx
        retf

; helper near-calls hooked func2 so the host must dispatch a nested hook while
; tracing a native->guest callthrough.
helper:
        mov ax, 0x4444
        call func2
        mov dx, ax
        xchg ah, al
        mov ax, dx
        push bx
        mov bx, ax
        pop bx
        retf

; Explicit guest-owned raw-code reservation. Keep the marker exactly 16 bytes
; so the following scratch block remains paragraph aligned.
        align 16, db 0
raw_marker:
        db 'HYDRA_RAW_SLOT!!'
raw_scratch:
        times 8192 db 0
