# Phase 1 Implementation Guide: dosemu2 `simx86` Validator

**Pinned dosemu2 base:** `604ce0cdd1a71f657e2a2df623d216d5ab289313`  
**Scope:** 16-bit real-mode MZ executables; one concurrent validator instance  
**ABI:** version 1, 80-byte control structure in page-sized `/hydra_remote` backing  
**Status:** dosemu2 carrier and current Rust adapter path are implemented; expanded semantic runtime coverage remains incomplete.

## 1. Landed implementation map

### emu86/dis86 prerequisites

- **#12 merged:** CMPS byte/word + REP termination, configurable PSP/MZ loading, and initial-state normalization.
- **#14 merged:** SHL/SHR/SAR behavior aligned with the pinned simx86 interpreter.

### dosemu2 carrier

`patches/dosemu2/series` currently contains nine ordered patches:

1. `0001-simx86-add-validator-control-abi.patch`
   - page-sized `/hydra_remote`;
   - ABI version/size and 80-byte control layout;
   - executable-scoped target activation;
   - request/apply + publish/ack around `FindExecCode()` / `DoExec(G)`;
   - validator `MSSTP` mode;
   - real-mode state import, segment-cache update, PC recomputation;
   - `TNode.seqnum` decoded count and raw step flags;
   - pre-node end barrier.
2. `0002-mapping-export-validator-lowmem.patch`
   - requires `mapshm`;
   - exports the actual `MAPPING_LOWMEM` backing as `/dosemu_mem`;
   - accepts a backing larger than the visible low-memory window;
   - validates containment/address-zero provenance and bidirectional aliasing.
3. `0003-simx86-harden-validator-target-lifecycle.patch`
   - current-PSP and PSP-parent ancestry handling;
   - descendant bypass;
   - permanent target-exit latch and stale-PSP protection.
4. `0004-simx86-publish-validator-step-flags-atomically.patch`
   - release publication for asynchronous lifecycle flags.
5. `0005-simx86-reject-protected-mode-state-import.patch`
   - fail-closed protected-mode import with `DIIS_STEP_FAULT` before mutation.
6. `0006-simx86-exclude-dos-handlers-from-validator.patch`
   - consumes target requests only while PC lies inside the target-owned MCB range.
7. `0007-simx86-publish-dos-termination-before-execution.patch`
   - publishes termination before another target node executes.
8. `0008-simx86-allow-dynamic-target-drive-identity.patch`
   - supports an explicit wildcard only in the DOS drive-letter position for `-K` mount variability.
9. `0009-simx86-exclude-validator-single-step-from-faults.patch`
   - distinguishes expected validator single-step/internal return reasons from architectural faults.

### Rust validator path

- **#18 merged:** `StepOutcome`, decoded-node metadata consumption, initial-state comparison, terminal/fault outcome handling, and emu86 advancement from reported decoded counts.
- **#19 merged:** dosemu2 stderr diagnostics, cooperative `END_ACK` shutdown, bounded fallback termination, and explicit reap.
- **#20 merged:** pinned-runtime terminating MZ fixture driven through the dosemu2 backend/validator path.

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

Layout:

```text
offset  0  u32 abi_version
offset  4  u32 struct_size
offset  8  u32 init
offset 12  u32 end
offset 16  i32 pid
offset 20  u16 runtime_psp
offset 22  u16 reserved0
offset 24  u64 req
offset 32  u64 ack
offset 40  u32 decoded_instructions
offset 44  u32 step_flags
offset 48  u16 ax ... flags
offset 76  u32 reserved1
size       80 bytes
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
5. waits for `init` with child-exit/timeout handling;
6. validates ABI/version/structure and launcher process ancestry;
7. captures the published initial CPU snapshot and `runtime_psp`;
8. constructs/rebases emu86 using the configurable PSP load path;
9. applies the shared initial-state normalization policy before comparison.

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
- DOS/BIOS code outside the target-owned MCB does not consume a target request;
- descendant/helper PSPs may run without consuming a target request;
- global `end` remains effective before bypass;
- leaving target ancestry permanently publishes `TARGET_EXIT`, clears `runtime_psp`, acknowledges pending work, and prevents stale-PSP reactivation.

The source paths are implemented. Representative handler/helper execution and target lifecycle transitions remain runtime-proof items.

## 7. Low-memory export

`/dosemu_mem` is the live `mapshm` backing containing `lowmem_base`, not a copy. The backing may be larger than `LOWMEM_SIZE + HMASIZE`; the implementation verifies the relevant backing relationship rather than exact-size equality.

The pinned-runtime workflow proves external bidirectional alias visibility. Broader per-instruction memory equivalence remains part of the differential corpus.

## 8. Shutdown

The dosemu2 side implements a pre-node zero-more-controlled-nodes end barrier and publishes `DIIS_STEP_END_ACK`. The Rust adapter cooperatively requests shutdown, waits boundedly for acknowledgement/exit, uses kill only as fallback, and explicitly reaps the child.

This path has focused pinned-runtime coverage.

## 9. Verification matrix

### Implemented and pinned-runtime proven where stated

- [x] nine-patch series applies to the pinned dosemu2 revision
- [x] patched runtime compiles/links
- [x] pinned FDPP + comcom32 provisioning
- [x] ABI initialization
- [x] basic request/step acknowledgement
- [x] external `/dosemu_mem` bidirectional alias proof
- [x] end barrier
- [x] cooperative/clean shutdown path
- [x] terminating MZ fixture through dosemu2 backend

### Implemented, but expanded E2E proof still required

- [ ] REP MOVS/STOS/CMPS/SCAS comparison semantics
- [ ] STI/MOV SS/POP SS multi-instruction nodes
- [ ] shadow + REP composition
- [ ] representative DOS/BIOS handler exclusion
- [ ] representative descendant child/helper exclusion
- [ ] target -> child -> target lifecycle
- [ ] target -> parent / stale-PSP lifecycle
- [ ] broad external register/segment/control redirection coverage
- [ ] full per-boundary architectural-state and relevant-memory differential corpus

Hydra native-function interception, overlays, and performance benchmarking remain separate follow-on work.
