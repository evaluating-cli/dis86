; testprog_ovl.asm - guest program for the Phase 7 Item E (overlay support)
; Hydra-on-dosemu2 integration test.
;
; A synthetic Borland-VROOMM-style overlay setup in a single .COM segment:
;
;   - hooked_fn   mov ax,0x1111; ret      -> regular Hydra hook (near ret)
;   - STUB        db 0CDh,03Fh / dw BODY_OFF / nops
;                 The VROOMM page-in stub: "int 3F" until first called; the
;                 fake pager (INT 3F handler) then patches it into
;                 "jmp far OVSEG:BODY_OFF" (0EAh ...).
;   - PAGER       INT 3F handler = fake VROOMM loader. Copies OVLBODY to
;                 OVSEG:0000, patches the stub in place and rewinds the
;                 stacked return IP so the IRET lands ON the patched stub.
;   - OVLBODY     mov ax,0x0A77; retf (+nop pad) - the NATIVE paged body.
;                 The Hydra OVERLAY hook returns 0xBEEF instead once the
;                 core takes over, so native page-in vs decompiled path is
;                 distinguishable in guest-visible results.
;
; The regular hook runs once before the overlay loop. There is deliberately
; NO regular-hook stop and NO self-write of STUB between overlay calls: the
; host must observe page-in itself and must not rely on a guest store to flush
; simx86 translations before its debugger breakpoint can work.
;
; Memory map note (linear): image lives at com_seg<<4 (well below 64K),
; raw-code region at 0xF0000, overlay body at OVSEG<<4 = 0x30000. The three
; regions never overlap.
;
; Layout (org 100h; offsets mirrored in test_driver_ovl.c):
;   0x100  start:    jmp short waitloop
;   0x102  goflag    db  0        <- host writes 1 here to start
;   0x103  hookres   dw  0        <- AX of hooked_fn (hook or native 0x1111)
;   0x105  ovlcnt    dw  0        <- overlay-call counter
;   0x107  ovlres    dw  0 x8     <- AX after each far call (result ring)
;   0x117  stubptr   dw  STUB, 0  <- far pointer; seg filled with cs at init
;
; The loop uses conditional/relative branches only (never a direct self-jump),
; so dosemu2's simx86 "forever loop" exit (F_SLFJ, codegen.c) never triggers.

BITS 16
        org 100h

OVSEG    equ 0x3000
BODY_OFF equ 0
BODY_LEN equ 8

start:
        jmp short waitloop

goflag:  db 0
hookres: dw 0
ovlcnt:  dw 0
ovlres:  dw 0,0,0,0,0,0,0,0
stubptr: dw STUB, 0

waitloop:
        mov al, [goflag]
        test al, al
        jz waitloop

        ; Install the INT 3F handler (fake VROOMM pager).
        xor ax, ax
        mov ds, ax
        mov word [0x00FC], PAGER
        mov ax, cs
        mov [0x00FE], ax
        mov ax, cs
        mov ds, ax
        mov [stubptr+2], ax

        ; One ordinary hook proves static hooks still coexist with the overlay
        ; machinery, but it is intentionally outside the repeated overlay loop.
        call hooked_fn
        mov [hookres], ax

mainloop:
        inc word [ovlcnt]
        mov bx, [ovlcnt]
        dec bx
        and bx, 7
        shl bx, 1
        call far [stubptr]
        mov [ovlres+bx], ax
        jmp mainloop

hooked_fn:
        mov ax, 0x1111
        ret

; VROOMM-style page-in stub. Initially "int 3F"; the pager rewrites it in
; place to "jmp far OVSEG:BODY_OFF" (0xEA off_off seg_seg).
STUB:
        db 0xCD, 0x3F
        dw BODY_OFF
        nop
        nop
        nop

; Fake VROOMM pager. Entry stack frame (pushed by the int):
;   [sp+0]=IP  [sp+2]=CS  [sp+4]=FL  (IP points AFTER the int: stub+2).
; After the 8 register pushes below (16 bytes), with bp := sp:
;   [bp+0..14]=saved regs, [bp+16]=IP, [bp+18]=CS, [bp+20]=FL.
; All GP/segment registers preserved; IRET restores flags.
PAGER:
        push ax
        push bx
        push cx
        push si
        push di
        push bp
        push es
        push ds
        mov bp, sp
        mov bx, [bp+16]
        sub bx, 2

        ; copy the body: cs:OVLBODY -> OVSEG:BODY_OFF
        mov di, OVSEG
        mov es, di
        mov di, BODY_OFF
        mov ax, cs
        mov ds, ax
        mov si, OVLBODY
        mov cx, BODY_LEN
        cld
        rep movsb

        ; patch the stub in place: EA <off> <seg>
        mov word [bx], 0x00EA
        mov word [bx+1], BODY_OFF
        mov word [bx+3], OVSEG

        mov [bp+16], bx
        pop ds
        pop es
        pop bp
        pop di
        pop si
        pop cx
        pop bx
        pop ax
        iret

; The NATIVE paged-in body: AX=0x0A77, far-return through the original
; call-far frame. Padded to BODY_LEN with NOPs.
OVLBODY:
        mov ax, 0x0A77
        retf
        times BODY_LEN-4 db 0x90
