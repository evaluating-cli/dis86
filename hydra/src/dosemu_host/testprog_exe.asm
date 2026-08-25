; testprog_exe.asm - MZ (.exe) variant of the Phase 4/5/6 guest program for
; the Hydra-on-dosemu2 integration test (Phase 7 Item C).
;
; Same body as testprog.asm (myfunc/func2/callthru + helper/helper2), but
; wrapped in a hand-rolled MZ header so NASM alone produces a loadable .exe
; (no external linker). DOS loads it via INT21 AH=4B01; the host validates
; the machine state against e_cs/e_ip emitted below.
;
; Addressing model (important): the file is assembled with org 0, i.e. ALL
; labels carry FILE offsets while the runtime segment covers only the load
; MODULE (file bytes 0x20..). Self-references therefore subtract HDR_SIZE
; explicitly ([result-HDR_SIZE] etc.) so that DS=CS=load segment - which is
; what the host's dispatch cycle restores around every hook - sees
; module-relative offsets. (Do NOT add org to the labels: with org 0x20 the
; subtraction would only cancel the org and yield FILE offsets again, which
; then overwrote code at runtime.) Hook breakpoints are planted by the host
; at PHYSICAL module-relative addresses and are unaffected.
;
; Layout (offsets are FILE offsets; subtract 0x20 for module offsets):
;   0x020  start:     set DS; jmp short mainloop   <- entry (e_ip = 0)
;   0x024  result     dw  0        <- guest stores hook1's AX here
;   0x026  hookcnt    dw  0        <- hook1 increments via guest memory
;   0x028  result2    dw  0        <- guest stores hook2's AX here
;   0x02a  res3       dw  0        <- guest stores hook3's AX here
;   0x02c  res4       dw  0        <- hook3 stores helper2's AX via mem_write16
;   0x02e  flagres    dw  0        <- pushf after hook3 (flags preservation)
;   0x030  mainloop / hooked fns / helpers (identical to testprog.asm)
;   0x240  stack: 512 bytes inside the module (e_ss/e_sp point here)
;
; EXE vs COM notes:
;   - DS is set explicitly (DOS gives EXEs DS=ES=PSP, not the image).
;   - No goflag/waitloop: the host pushes the entry register state and holds
;     the machine stopped until the hook breakpoints are planted, so the
;     guest cannot race ahead.
;   - All segment-register-dependent addressing is label-based within one
;     image segment (e_cs = 0), keeping hooks image-relative by design.
;
; The loop uses conditional/relative branches only (never a direct self-jump),
; so dosemu2's simx86 "forever loop" exit (F_SLFJ, codegen.c) never triggers.

BITS 16
        org 0                   ; labels == FILE offsets (header included)

HDR_SIZE    equ 0x20            ; header bytes (this many before the module)
STACK_PARA  equ 0x20            ; stack segment, in module paragraphs
STACK_SIZE  equ 512

; ---------------- MZ header (emitted first; DOS strips these 32 bytes) ----
; Canonical RBIL Table 01403 field offsets - real DOS (fdpp included) parses
; THIS layout: reloc count @06, header paragraphs @08, ss/sp @0E/@10,
; ip/cs @14/@16, reloc table offset @18. Address fields are MODULE-relative;
; file-length fields count header + module. NOTE: NASM cannot fold forward
; labels into arithmetic here, so the file-length/stack fields are literals;
; if the module below ever grows past them, bump these numbers.
mz_header:
        db 'M', 'Z'                             ; e_magic
        dw 0x0020                               ; 02: e_cblp (file = 1056 B)
        dw 3                                    ; 04: e_cp (three pages)
        dw 0                                    ; 06: e_crlc: NO relocations
        dw HDR_SIZE / 16                        ; 08: e_cparhdr
        dw 0                                    ; 0A: e_minalloc (stack in module)
        dw 0xFFFF                               ; 0C: e_maxalloc
        dw STACK_PARA                           ; 0E: e_ss (module paras)
        dw STACK_SIZE                           ; 10: e_sp
        dw 0                                    ; 12: e_csum (unchecked)
        dw 0                                    ; 14: e_ip (module offset)
        dw 0                                    ; 16: e_cs (module para)
        dw 0x1E                                 ; 18: e_lfarlc: empty reloc table
        dw 0                                    ; 1A: e_ovno: no overlay
times HDR_SIZE-($-$$) db 0                       ; pad header to 2 paragraphs

; ---------------- load module ----------------

start:                          ; module 0x00, exactly 4 bytes: the host
        push cs                 ; driver expects the data words that follow
        pop ds                  ; at fixed module offsets 0x04..0x0E
        jmp short over_data

result:  dw 0                   ; module 0x04
hookcnt: dw 0                   ; module 0x06
result2: dw 0                   ; module 0x08
res3:    dw 0                   ; module 0x0A
res4:    dw 0                   ; module 0x0C
flagres: dw 0                   ; module 0x0E

over_data:                      ; module 0x10
mainloop:
        call myfunc
        mov [result-HDR_SIZE], ax
        call func2
        mov [result2-HDR_SIZE], ax
        stc                             ; CF=1 must survive the hook roundtrip
        call callthru
        mov [res3-HDR_SIZE], ax
        pushf
        pop bx
        mov [flagres-HDR_SIZE], bx
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

; resident stack (e_ss/e_sp): DOS does not fix up an invalid SP. The pad
; pins the stack to EXACTLY module paragraph STACK_PARA so the literal in
; the header cannot drift; if the code above ever grows past it, NASM fails
; on the negative times. (Labeled offsets are FILE offsets here: module
; paragraph STACK_PARA == FILE offset (STACK_PARA<<4) + HDR_SIZE.)
times (STACK_PARA << 4) + HDR_SIZE - ($ - $$) db 0
stack_base:
        times STACK_SIZE db 0
stack_top:
img_end:
