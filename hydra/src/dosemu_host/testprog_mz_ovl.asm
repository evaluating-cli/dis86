; Minimal MZ/FBOV overlay fixture. The loaded image has a 32-byte
; CodeOverlaySeg header at module offset 0, then one five-byte stub at 0:0020.
; The same 4-byte native body is also appended after the FBOV record as the
; parser-visible overlay payload. The guest pager copies the resident copy.

BITS 16
org 0

HDR_SIZE equ 0x20
OVSEG    equ 0x3000
BODY_OFF equ 0
BODY_LEN equ 4

mz_header:
    db 'M','Z'
    dw (exe_end - $$) % 512
    dw ((exe_end - $$ + 511) / 512)
    dw 0                       ; relocations
    dw HDR_SIZE / 16
    dw 0                       ; minalloc
    dw 0xffff                  ; maxalloc
    dw 0x10                    ; SS (module paragraph 16)
    dw 0x0200                  ; SP
    dw 0                       ; checksum
    dw start - HDR_SIZE        ; IP (module-relative)
    dw 0                       ; CS
    dw 0x1e                    ; relocation table offset
    dw 0                       ; overlay number
times HDR_SIZE-($-$$) db 0

; Overlay segment header, exactly the format consumed by Dis86's FBOV parser.
overlay_seg_header:
    db 0xcd, 0x3f, 0, 0
    dd 0                       ; data offset from overlay payload start
    dw BODY_LEN
    dw 0, 0
    times 18 db 0

; Single VROOMM-style stub: INT 3F, destination offset, zero pad.
overlay_stub:
    db 0xcd, 0x3f
    dw BODY_OFF
    db 0

; Publicly fixed guest result slots keep the host smoke assertion simple.
call_count:
    dw 0
last_result:
    dw 0
first_result:
    dw 0
stub_ptr:
    dw 0, 0

start:
    push cs
    pop ds
    xor ax, ax
    mov ds, ax
    mov word [0x00fc], pager - HDR_SIZE
    mov ax, cs
    mov [0x00fe], ax
    mov ds, ax
    mov word [stub_ptr - HDR_SIZE], overlay_stub - HDR_SIZE
    mov word [stub_ptr - HDR_SIZE + 2], ax

run_loop:
    inc word [call_count - HDR_SIZE]
    call far [stub_ptr - HDR_SIZE]
    mov [last_result - HDR_SIZE], ax
    cmp word [call_count - HDR_SIZE], 1
    jne run_loop
    mov [first_result - HDR_SIZE], ax
    jmp run_loop

pager:
    push ax
    push bx
    push cx
    push si
    push di
    push bp
    push es
    push ds
    mov bp, sp
    mov bx, [bp+16]             ; stacked IP, just after INT 3F
    sub bx, 2                   ; start of the stub

    ; Simulated overlay loader: copy the body into its destination segment.
    mov ax, cs
    mov ds, ax
    mov si, native_body - HDR_SIZE
    mov ax, OVSEG
    mov es, ax
    xor di, di
    mov cx, BODY_LEN
    cld
    rep movsb

    ; Patch CS:stub into JMP FAR OVSEG:BODY_OFF and rewind IRET to it.
    mov ax, cs
    mov es, ax
    mov word [es:bx], 0x00ea
    mov word [es:bx+1], BODY_OFF
    mov word [es:bx+3], OVSEG
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

native_body:
    mov ax, 0x0a77
    retf

; Put the stack at module offset 0x100 with e_ss=0x10.
times HDR_SIZE + 0x100 - ($-$$) db 0
stack_area:
    times 512 db 0
stack_end:

seginfo:
    dw 0                       ; overlay stub segment
    dw 37                      ; 32-byte header + one 5-byte stub
    dw 3                       ; SegInfoType::STUB
    dw 0

exe_end:
fbov:
    db 'F','B','O','V'
    dd BODY_LEN
    dd seginfo - $$             ; absolute file offset of SegInfo
    dd 1                        ; one SegInfo record
overlay_payload:
    db 0xb8, 0x77, 0x0a, 0xcb  ; mov ax,0x0a77; retf
