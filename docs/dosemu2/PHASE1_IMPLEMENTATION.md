# Phase 1 Implementation Guide: dosemu2 `simx86` Validator

**Pinned dosemu2 base:** `604ce0cdd1a71f657e2a2df623d216d5ab289313`  
**Scope:** 16-bit real-mode MZ executables; one concurrent validator instance  
**ABI:** version 1, 88-byte structure preserving the legacy 64-byte Hydra prefix in page-sized `/hydra_remote` backing  
**Status:** dosemu2 carrier and current Rust adapter path are implemented; expanded semantic runtime coverage remains incomplete.

## 1. Landed implementation map

### emu86/dis86 prerequisites

- **#12 merged:** CMPS byte/word + REP termination, configurable PSP/MZ loading, and initial-state normalization.
- **#14 merged:** SHL/SHR/SAR behavior aligned with the pinned simx86 interpreter.

### dosemu2 carrier

`patches/dosemu2/series` contains two squashed frozen feature patches. The former development patches 0001–0010 are closed historical development; see the authoritative [`FREEZE_ABI_V1.md`](FREEZE_ABI_V1.md).

1. `0001-simx86-add-executable-scoped-validator-control.patch` carries the simx86 ABI, activation, execution, lifecycle, atomic publication, protected-mode rejection, handler boundary, termination, fault, and interrupt-service behavior developed in historical patches 0001 and 0003–0010.
2. `0002-mapping-expose-live-low-memory-backing.patch` carries the generic mapping-backing query and live `/dosemu_mem` alias work developed in historical patch 0002.

### Rust validator path

- **#18 merged:** `StepOutcome`, decoded-node metadata consumption, initial-state comparison, terminal/fault outcome handling, and emu86 advancement from reported decoded counts.
- **#19 merged:** dosemu2 stderr diagnostics, cooperative `END_ACK` shutdown, bounded fallback termination, and explicit reap.
- **#20 merged:** pinned-runtime terminating MZ fixture driven through the dosemu2 backend/validator path.
- **#21 merged:** pinned-runtime target-exit and architectural-fault publication evidence.
- **#22 merged:** digest-pinned comcom32 artifact provisioning for reproducibility.
- **#23 merged:** host-only exact-command, MZ-identity, canonical-path, mapping-size, and launcher/descendant PID-ownership coverage.
- **#25 merged:** patch 0010 deferred acknowledgement across standalone nonterminating host services.

The adapter now launches dosemu2 with:

```text
-dumb -quiet -K <dir> -E <exe>
-I "cpu_vm emulated"
-I "cpuemu 1"
-I "cpu_vm_dpmi emulated"
-I "mappingdriver mapshm"
```

and supplies:

```text
DIIS_DOSEMU_VALIDATOR=1
DIIS_DOSEMU_MZ_CS=<u16>
DIIS_DOSEMU_MZ_IP=<u16>
DIIS_DOSEMU_TARGET_DOS_PATH=?:\<exe>
```

The `?` wildcard is accepted only for the drive-letter position by patch 0008.

## 2. Shared ABI and ordering

The dosemu2 extension is append-only: the first 64 bytes retain the established Hydra controller/register ABI, and dosemu2 metadata begins at offset 64.

```text
offset  0  u32 init
offset  4  u32 end
offset  8  i32 pid
offset 12  u32 reserved0
offset 16  u64 req
offset 24  u64 ack
offset 32  u16 ax, bx, cx, dx, si, di, bp, sp, ip, cs, ds, es, ss, flags
offset 60  u32 legacy_reserved1
offset 64  u32 abi_version
offset 68  u32 struct_size
offset 72  u16 runtime_psp
offset 74  u16 reserved1
offset 76  u32 decoded_instructions
offset 80  u32 step_flags
size       88 bytes (8-byte aligned)
```

Step flags:

```text
DIIS_STEP_MULTI_INSN   = 1 << 0
DIIS_STEP_SAME_PC      = 1 << 1
DIIS_STEP_FAULT        = 1 << 2
DIIS_STEP_END_ACK      = 1 << 3
DIIS_STEP_TARGET_EXIT  = 1 << 4
```

Ordering:

```text
controller writes request payload
store_release(req)
        |
        v
hook acquire-loads req
applies state
executes validator-bounded node
writes CPU + metadata
store_release(step_flags)
store_release(ack)
        |
        v
controller acquire-loads ack
reads ordinary result
```

Asynchronous `END_ACK`/`TARGET_EXIT` publication is synchronized through release/acquire access to `step_flags` even when `req`/`ack` do not change.

## 3. Initialization

The adapter:

1. removes stale fixed shared-memory names;
2. parses the target MZ header;
3. launches the verified simx86/mapshm path;
4. attaches `/hydra_remote` and `/dosemu_mem`;
5. waits for `init` with process-exit/timeout handling;
6. validates ABI version, structure size, and published process ownership before trusting the payload;
7. captures the published initial CPU snapshot and `runtime_psp`;
8. constructs/rebases emu86 using the configurable PSP load path;
9. applies the shared initial-state normalization policy before comparison.

`ShmData::attach()` rejects an undersized shared-memory backing before mapping it. After `init`, `DosemuProcess` accepts the published PID when it is either the launcher returned by `Command::spawn()` or an emulator descendant whose parent ancestry leads back to that launcher; equality with `Child::id()` is not required.

The load segment is therefore discovered from runtime state rather than guessed from a fixed PSP.

## 4. CPU state import/export

### Export

Post-node IP is derived from the authoritative `next_PC` returned by `DoExec(G)`, not a potentially stale EIP snapshot while still inside the persistent dispatch loop.

### Import

For real mode, the hook preserves high halves of GPRs/EFLAGS, applies the 16-bit payload, updates changed real-mode segment caches with the normal segment path, and recomputes the local linear PC from imported `CS:IP` before node lookup.

Protected mode is rejected before mutation and published as `DIIS_STEP_FAULT`.

Normal external `CS:IP` redirection does not use `EXCP_EMULEAVE`.

## 5. Raw and normalized stepping

One dosemu2 request/ack exchange produces a raw `StepOutcome` containing at least `decoded_instructions` and `step_flags`. The Rust validator validates the metadata and advances emu86 according to the reported decoded count rather than assuming every node consumes one instruction.

Two semantic cases still need focused runtime proof:

### REP

A REP instruction may remain at the same `CS:IP` across raw micro-iterations. The intended normalized comparison boundary is the completed semantic REP operation expected by emu86. `SAME_PC`/published PC and decoded metadata provide the raw facts; the expanded corpus must prove that coalescing/advancement does not double-consume iterations.

### Interrupt shadow

`STI`, `MOV SS`, and `POP SS` may compose with a following decoded instruction in one node. `TNode.seqnum` reports the actual span. The particularly important case is shadow + first REP iteration: the first REP iteration must never be executed or compared twice.

The current plumbing exists; these semantics remain **unverified E2E**.

## 6. Target ownership and lifecycle

Activation requires MZ entry identity plus validated PSP/MCB/environment/current-PSP state.

After activation:

- target-owned PC + target PSP may consume requests;
- DOS/BIOS code outside the target-owned MCB does not consume a new target request;
- descendant/helper PSPs may run without consuming a target request;
- global `end` remains effective before bypass;
- leaving target ancestry permanently publishes `TARGET_EXIT`, clears `runtime_psp`, acknowledges pending work, and prevents stale-PSP reactivation.

Patch 0010 distinguishes host-service normalization from application-handler lockstep. For a standalone software interrupt through an eligible unchanged activation-time vector, it records the exact saved return `CS:IP`, leaves the request pending across the host service and callbacks, and publishes only on that return. If the application changes the vector, handler entry remains an emu86-visible boundary and the handler stays controller-stepped through interrupt return, even outside the target MCB.

The pinned nonterminating `INT 21h/AH=30h` probe passes: acknowledgement occurs back at the post-interrupt target instruction with DOS-returned register state, and the following target instruction consumes it. This does not integration-test prefixed calls, application-installed handlers, broader BIOS coverage, shadow-composed interrupts, helpers, or lifecycle transitions.

## 7. Low-memory export

`/dosemu_mem` is the live `mapshm` backing containing `lowmem_base`, not a copy. The backing may be larger than `LOWMEM_SIZE + HMASIZE`; the implementation verifies the relevant backing relationship rather than exact-size equality.

The pinned-runtime workflow proves external bidirectional alias visibility. Broader per-instruction memory equivalence remains part of the differential corpus.

## 8. Shutdown

The dosemu2 side implements a pre-node zero-more-controlled-nodes end barrier and publishes `DIIS_STEP_END_ACK`. The Rust adapter cooperatively requests shutdown, waits boundedly for acknowledgement/exit, uses kill only as fallback, and explicitly reaps the child.

This path has focused pinned-runtime coverage.

## 9. Verification ownership

The evidence matrix is maintained in [`TESTING.md`](TESTING.md) rather than duplicated here. In summary, the standalone unprefixed `INT 21h/AH=30h` host-service path, target-exit/fault publication, transport, low-memory alias, and shutdown have focused pinned-runtime proofs. Prefixed/application-handler interrupt cases, broader BIOS coverage, REP, interrupt shadow, helper/lifecycle transitions, broad mutation, and the full differential corpus do not.

Hydra native-function interception, overlays, and performance benchmarking remain separate follow-on work.
