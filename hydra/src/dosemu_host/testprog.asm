; testprog.asm - guest program for the Phase 4/5/6 Hydra-on-dosemu2
; integration test.
;
; Layout (loaded at PSP:0100, org 100h):
;   0x100  start:    jmp short waitloop
;   0x102  goflag    db  0         <- host writes 1 here to start
;   0x103  result    dw  0         <- guest stores hook1's AX here
;   0x105  hookcnt   dw  0         <- hook1 increments via guest memory
;   0x107  result2   dw  0         <- guest stores hook2's AX here
;   0x109  res3      dw  0         <- guest stores hook3's AX here
;   0x10b  res4      dw  0         <- hook3 stores helper2's AX via mem_write16
;   0x10d  flagres   dw  0         <- pushf after hook3 (flags preservation)
;   0x10f  waitloop: spins until goflag != 0
;   mainloop: call myfunc / func2 / (stc) callthru; jmp mainloop
;
; Hooked functions (hooked by the host via signature scan):
;   myfunc   mov ax,0x1111; ret   (B8 11 11 C3)  hook1: raw codes, AX=0xBE00|t
;   func2    mov ax,0x2222; ret   (B8 22 22 C3)  hook2: NOP raw code, AX=0xCAFE
;   callthru mov ax,0x3333; ret   (B8 33 33 C3)  hook3: CALL_FAR helper2+helper
;
; Unhooked guest functions (called natively by hook3 through CALL_FAR):
;   helper2  11 instructions, AX=0x7ACE, retf     (B8 CE 7A ...) plain callthrough
;   helper   calls func2 (nested hooked call inside a trace), AX=0xCBFE
;            if the nested breakpoint fires during the trace, 0x2322 if not
;            (B8 44 44 ...) nested callthrough
;
; NOTE: keep each hooked function's mov ax,0xNNNN body unique so the host's
; signature scan is unambiguous.
;
; The loop uses conditional/relative branches only (never a direct self-jump),
; so dosemu2's simx86 "forever loop" exit (F_SLFJ, codegen.c) never triggers.

BITS 16
        org 100h

start:
        jmp short waitloop

goflag:  db  0
result:  dw  0
hookcnt: dw  0
result2: dw  0
res3:    dw  0
res4:    dw  0
flagres: dw  0

waitloop:
        mov al, [goflag]
        test al, al
        jz waitloop

mainloop:
        call myfunc
        mov [result], ax
        call func2
        mov [result2], ax
        stc                             ; CF=1 must survive the hook roundtrip
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

; helper2: plain native->guest callthrough, >6 instructions (the old 6-step
; trace limit would have failed here). AX=0x7ACE. All filler instructions are
; flag-neutral (mov/xchg/nop/push/pop only) so the pushf flags-preservation
; check in mainloop is not contaminated. 13 instructions total.
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

; helper: native->guest callthrough whose body near-calls the HOOKED func2.
; If the breakpoint fires during the trace, hook2 runs (AX=0xCAFE); if not,
; func2's native body runs (AX=0x2222). Everything after the call is
; flag-neutral (mov/xchg/push/pop only) so the pushf flags-preservation
; check in mainloop is not contaminated by helper itself.
helper:
        mov ax, 0x4444
        call func2              ; AX=CAFE (nested hook fired) / 2222 (native)
        mov dx, ax
        xchg ah, al             ; flag-neutral byte swap exercise
        mov ax, dx              ; restore CAFE / 2222
        push bx
        mov bx, ax
        pop bx
        retf
