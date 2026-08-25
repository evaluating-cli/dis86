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
; Mainloop: call hooked_fn; inc ovlcnt; call FAR cs:[stubptr] (4-byte frame);
; store AX into an 8-word result ring indexed by ovlcnt; repeat.
;
; Memory map note (linear): image lives at com_seg<<4 (well below 64K),
; raw-code region at 0x1C000 (driver conf raw_code=0x1c00), overlay body at
; OVSEG<<4 = 0x30000. The three regions never overlap.
;
; Layout (org 100h; offsets mirrored in test_driver_ovl.c):
;   0x100  start:    jmp short waitloop
;   0x102  goflag    db  0        <- host writes 1 here to start
;   0x103  hookres   dw  0        <- AX of hooked_fn (hook or native 0x1111)
;   0x105  ovlcnt    dw  0        <- overlay-call counter
;   0x107  ovlres    dw  0 x8     <- AX after each far call (result ring)
;   0x117  stubptr   dw  STUB, 0  <- far pointer; seg filled with cs at init
;   0x11B  waitloop / mainloop / functions / stub / pager / body
;
; The loop uses conditional/relative branches only (never a direct self-jump),
; so dosemu2's simx86 "forever loop" exit (F_SLFJ, codegen.c) never triggers.

BITS 16
        org 100h

OVSEG    equ 0x3000              ; overlay segment (linear 0x30000)
BODY_OFF equ 0                   ; body offset inside the overlay segment
BODY_LEN equ 8                   ; bytes copied by the pager

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

        ; Install the INT 3F handler (fake VROOMM pager). Vector for INT 3F
        ; lives at 0000:003F*4 = 0000:00FC. Interrupts are still enabled at
        ; this point; that is fine (two-word IVT store).
        xor ax, ax
        mov ds, ax
        mov word [0x00FC], PAGER
        mov ax, cs
        mov [0x00FE], ax
        mov ax, cs
        mov ds, ax               ; restore DS = CS (.COM convention)
        mov [stubptr+2], ax      ; far pointer segment := cs

mainloop:
        call hooked_fn           ; near call, hooked by Hydra (RETURN_NEAR)
        mov [hookres], ax
        inc word [ovlcnt]
        mov bx, [ovlcnt]
        dec bx                   ; ring slot = (ovlcnt-1) & 7
        and bx, 7
        shl bx, 1
        mov ax, [STUB]           ; read-modify-write the stub's first word:
        mov [STUB], ax           ; drops simx86 translations for it, so a
        call far [stubptr]       ; debugger-planted CC is ALWAYS seen (and
        mov [ovlres+bx], ax      ; always traps). Guest stores are the only
        jmp mainloop             ; writes whose cache effects are visible.

hooked_fn:
        mov ax, 0x1111
        ret

; VROOMM-style page-in stub. Initially "int 3F"; the pager rewrites it in
; place to "jmp far OVSEG:BODY_OFF" (0xEA off_off seg_seg). Keep the trailing
; NOPs: they make the stub byte pattern unique for the host signature scan.
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
        mov bx, [bp+16]          ; IP after the int == stub_base + 2
        sub bx, 2                ; bx = stub offset within cs

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
        mov word [bx], 0x00EA        ; opcode + low byte of dest offset
        mov word [bx+1], BODY_OFF    ; dest offset word
        mov word [bx+3], OVSEG       ; dest segment word

        mov [bp+16], bx         ; IRET lands ON the patched stub (executes EA)
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
