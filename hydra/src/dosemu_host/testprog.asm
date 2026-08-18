; testprog.asm - guest program for the Phase 4/5 Hydra-on-dosemu2 integration test.
;
; Layout (loaded at PSP:0100, org 100h):
;   0x100  start:    jmp short waitloop
;   0x102  goflag    db  0         <- host writes 1 here to start
;   0x103  result    dw  0         <- guest stores hook's AX here
;   0x105  hookcnt   dw  0         <- hook increments via guest memory
;   0x107  waitloop: spins until goflag != 0
;   0x10f  mainloop: call myfunc; mov [result],ax; jmp mainloop
;   0x118  myfunc:   mov ax,0x1111; ret    <- replaced by the Hydra hook
;
; NOTE: the host registers the hook at (CS-CODE_START_SEG):0x118 and locates
; this function by scanning for the B8 11 11 C3 signature. Keep myfunc's body
; unique so the scan is unambiguous.
;
; The loop uses conditional/relative branches only (never a direct self-jump),
; so dosemu2's simx86 "forever loop" exit (F_SLFJ, codegen.c) never triggers.

BITS 16
        org 100h

start:
        jmp short waitloop

goflag: db  0
result: dw  0
hookcnt: dw 0

waitloop:
        mov al, [goflag]
        test al, al
        jz waitloop

mainloop:
        call myfunc
        mov [result], ax
        jmp mainloop

myfunc:
        mov ax, 0x1111
        ret