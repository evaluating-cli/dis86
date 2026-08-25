# dosemu2 validator ABI-v1 (reference)

> **Note (2026-08-17):** SST (SingleStepTests hardware captures) is the validation authority for emu86; the dosemu2 differential validator is no longer the validation method. This ABI-v1 freeze is the change-control authority for the dosemu2 transport and remains the frozen transport/contract reference for Track 4 Option D (Hydra hosting via in-process plugin) research — not the validation authority.

**Freeze version:** 1  
**Status:** Frozen  
**Pinned upstream:** `dosemu2/dosemu2` commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`  
**Active carrier:** the two patches listed by `patches/dosemu2/series`

> The two frozen patches (`patches/dosemu2/0001`, `0002`) are the reference implementation of this contract; the boundary-hook and low-memory-backing semantics informed the current Hydra hosting approach (see [`OPTION_D_DESIGN.md`](OPTION_D_DESIGN.md)).

**Pinned upstream:** `dosemu2/dosemu2` commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`
**Carrier:** the two patches listed by `patches/dosemu2/series`

## ABI layout

The control object is 88 bytes, aligned to 8 bytes, in a page-sized POSIX SHM object.
Offsets 0--63 are the Hydra prefix; the dosemu2 extension is append-only.

```text
offset  0  u32 init
offset  4  u32 end
offset  8  i32 pid
offset 12  u32 reserved0
offset 16  u64 req
offset 24  u64 ack
offset 32  u16 ax, bx, cx, dx, si, di, bp, sp, ip, cs, ds, es, ss, flags
offset 60  u32 legacy_reserved1
offset 64  u32 abi_version (= 1)
offset 68  u32 struct_size (= 88)
offset 72  u16 runtime_psp
offset 74  u16 reserved1
offset 76  u32 decoded_instructions
offset 80  u32 step_flags
size       88 bytes
```

`init`, `end`, `req`, and `ack` are atomic control fields. The producer writes the
non-atomic payload before a release store to its control field; the consumer performs an
acquire load before reading or changing the payload.

Step flags:

```text
DIIS_STEP_MULTI_INSN  = 1 << 0
DIIS_STEP_SAME_PC     = 1 << 1
DIIS_STEP_FAULT       = 1 << 2
DIIS_STEP_END_ACK     = 1 << 3
DIIS_STEP_TARGET_EXIT = 1 << 4
```

`decoded_instructions` is `TNode.seqnum` (instruction count), not byte length `seqlen`.

## Scope

The ABI supported **only one validator instance controlling a 16-bit real-mode MZ
application through simx86**. Protected-mode/DPMI, 32-bit applications, multiple
controllers, and other CPU backends were outside scope and failed closed.

## Carrier

1. `0001-simx86-add-executable-scoped-validator-control.patch` — simx86 boundary hook,
   activation gate, state exchange, stepping, lifecycle, interrupt-service deferral.
2. `0002-mapping-expose-live-low-memory-backing.patch` — generic mapping-backing query
   exposing `lowmem_base` as the live POSIX-SHM backing at `/dosemu_mem`.

Both patches apply only against the pinned upstream commit. The former ten-patch
development series (0001--0010) is closed historical development, available only in Git
history.
