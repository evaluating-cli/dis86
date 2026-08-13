# dosemu2 validator ABI-v1 freeze

**Freeze version:** 1  
**Status:** Frozen  
**Pinned upstream:** `dosemu2/dosemu2` commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`  
**Active carrier:** the two patches listed by `patches/dosemu2/series`

This document is the change-control authority for the dosemu2 side of the validator. ABI-v1 and the execution, lifecycle, interrupt-service, and low-memory contracts below are frozen. A later contract must use a new freeze version; it must not silently reinterpret ABI-v1.

## ABI layout and atomic ordering

The control object is 88 bytes, aligned to 8 bytes, in a page-sized POSIX SHM object. Offsets 0--63 are the unchanged Hydra prefix; the dosemu2 extension is append-only.

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

`init`, `end`, `req`, and `ack` are atomic control fields. The producer writes the applicable non-atomic payload before a release store to its control field; the consumer performs an acquire load of that field before reading or changing the payload. Initialization payload precedes release `init`, request payload precedes release `req`, and result payload precedes release `ack`. The hook acquire-loads `req` before importing requested state and acquire-loads `end` at every active boundary. No concurrent payload access is permitted outside those hand-offs. ABI version, size, and PID ownership are validated after acquiring `init`; undersized storage is rejected before mapping.

Every ABI-v1 step flag is frozen at its current bit:

```text
DIIS_STEP_MULTI_INSN  = 1 << 0
DIIS_STEP_SAME_PC     = 1 << 1
DIIS_STEP_FAULT       = 1 << 2
DIIS_STEP_END_ACK     = 1 << 3
DIIS_STEP_TARGET_EXIT = 1 << 4
```

Unknown future bits must not change these meanings. Expected validator single-step and simx86 internal return reasons are not architectural faults. `decoded_instructions` is `TNode.seqnum`, not byte length `seqlen`.

## Execution and supported scope

Activation publishes initial state before `init`. For each accepted request, the hook rejects protected-mode import before mutation, preserves the high 16 bits of general registers and FLAGS, updates real-mode segment caches, applies requested `CS:IP`, recomputes linear PC before node lookup, and reasserts `MSSTP`. It executes the selected translated node and publishes state, decoded count, flags, and acknowledgement. Rust normalization advances emu86 from reported node consumption; REP same-PC iterations and interrupt-shadow composition must not be double-consumed.

The supported scope is **only one validator instance controlling a 16-bit real-mode MZ application through simx86**. Protected-mode/DPMI execution, 32-bit applications, multiple controllers, and other CPU backends are outside ABI-v1 and must fail closed.

## Target activation and termination

Activation requires real mode; `DS == ES`; a PSP beginning with `CD 20`; a PSP-owned MCB; a valid environment identity whose normalized executable path matches the configured DOS path; the configured MZ entry `CS:IP`; and agreement with dosemu2's version-aware current-PSP accessor. The path wildcard `?` is allowed only in the drive-letter position.

After activation, target code consumes requests. Descendant children and host helpers do not. A child may temporarily replace the current PSP and returning to the target resumes pending work. Leaving the target's PSP ancestry publishes `TARGET_EXIT`, clears `runtime_psp`, acknowledges pending work, permanently latches termination, and forbids stale-PSP reactivation. Architectural faults publish `FAULT`; DOS termination is published before another target node runs.

## End barrier

`end` means zero more controlled nodes. The hook acquire-checks it at every active boundary, before node selection **and before every helper, descendant, handler, or other bypass path**. Once observed, it publishes final state with `END_ACK`, release-stores `ack` to the current request value, and begins no later controlled node. The parent attempts bounded cooperative shutdown and reaps the process; forced termination is fallback only.

## Interrupt-service boundary

At activation, vectors `10h`, `1Ah`, `21h`, and `33h` are snapshotted. A standalone target software interrupt (`INT3`, `INT imm8`, or `INTO`, after accepted `26h`, `2Eh`, `36h`, or `3Eh` segment prefixes) is a host service only when its eligible vector remains unchanged. Its request stays pending across DOS/BIOS handler nodes and callbacks; handler entry is neither published nor acknowledged. Publication occurs only at the exact return `CS:IP` saved from the handler-entry stack, with host-returned state.

If the application changes the vector, it owns the handler. Handler entry is an emu86-visible boundary and the handler remains controller-stepped, including outside the target MCB, until its recorded interrupt return boundary. Host-service normalization must never suppress an application handler. Fault, target-exit, and end-barrier paths terminate pending service work under their normal rules. Terminating `INT 21h/AH=4Ch` remains a distinct terminal pre-execution case.

## Low-memory alias

Validator mode requires `mapshm`. `/dosemu_mem` must be the live POSIX-SHM backing that contains `lowmem_base`, maps conventional address zero, and covers at least `LOWMEM_SIZE + HMASIZE`; it may be larger than that visible window. A copied mirror or second guest-memory allocation is forbidden. Before `init`, dosemu2 verifies backing kind, name, fd/base provenance, coverage, address zero, and bidirectional alias visibility through the generic mapping-backing interface.

## Mandatory change gate

Every future dosemu2-side change, including a proposed contract correction, must include all four items in its review:

1. a checked-in fixture that fails on the pinned runtime before the change;
2. an explicit defect classification: **dosemu2**, **Rust normalization**, or **emu86**, with cross-layer symptoms separated rather than left ambiguous;
3. ABI compatibility analysis covering layout, offsets, sizes, atomic ordering, flags, and observable boundary behavior; and
4. a documented attempt to solve the defect through the generic hook/module interface before changing dosemu2 core code, including why that attempt is insufficient if a core edit remains necessary.

A change lacking any item does not pass the ABI-v1 freeze gate. New behavior that cannot remain compatible requires a new ABI/freeze version.

## Carrier history

Development patches **0001--0010 are closed historical development**. They remain available only through Git history and are not an active or supported series. The active series is the squashed frozen carrier:

1. `0001-simx86-add-executable-scoped-validator-control.patch`;
2. `0002-mapping-expose-live-low-memory-backing.patch`.

Together, and only against the pinned upstream commit, those two patches carry the frozen behavior described here.
