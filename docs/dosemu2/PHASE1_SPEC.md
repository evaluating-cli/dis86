# Phase 1 Specification: `simx86` Validator Integration

**Target milestone:** Phase 1 — deterministic dosemu2/emu86 differential stepping  
**Pinned dosemu2 source:** `dosemu2/dosemu2@604ce0cdd1a71f657e2a2df623d216d5ab289313`  
**Scope:** 16-bit real-mode MZ executables; one concurrent validator instance  
**Status:** emu86 prerequisites and the dosemu2 carrier (#17) are merged. The Rust dosemu2 adapter and the remaining semantic runtime coverage are incomplete.

> PR #10 is the architecture contract. It describes the implementation that exists today and keeps unproven semantic behavior explicitly open.

## 1. Current implementation map

- **#12 — merged:** CMPS/REP semantics, configurable runtime PSP loading, and initial-state normalization for emu86.
- **#14 — merged:** SHL/SHR/SAR behavior aligned with the pinned dosemu2 `simx86` interpreter.
- **#17 — merged:** five-patch dosemu2 carrier implementing the validator control hook/ABI, verified live low-memory export, target lifecycle handling, atomic lifecycle flags, and fail-closed protected-mode import rejection.
- **Rust `DosemuProcess` adapter — pending:** main still uses the inherited Phase-0/DOSBox-X launch path and old shared-memory contract.

Merged #17 established green evidence for the complete five-patch `git am` / diff-check / compile / link gate, pinned FDPP + comcom32 runtime provisioning, external `/dosemu_mem` bidirectional aliasing, and the validator end barrier.

The following remain unproven end-to-end and are not completion claims: target -> child -> target, target -> parent / stale-PSP behavior, REP + interrupt-shadow normalization, and DOS/INT 21h boundary visibility.

## 2. Non-negotiable architecture

### 2.1 Hook the persistent `FindExecCode()` boundary

`FindExecCode()` owns the authoritative local `PC`. Validator synchronization is split around `DoExec(G)`:

```text
pre-node:
  reject unsupported protected-mode import before mutation
  observe end/target lifecycle
  wait for request when the target owns the node
  apply requested real-mode register state
  recompute PC from imported CS:IP
  reassert MSSTP

execute:
  lookup/generate node for the recomputed PC
  next_PC = DoExec(G)

post-node:
  publish CPU state using next_PC
  publish decoded-node metadata
  acknowledge the request
```

Consequences:

- externally supplied `CS:IP` changes local `PC` before node lookup;
- post-node `IP` is derived from authoritative `next_PC`;
- `TNode::seqnum` is the decoded-instruction count; `seqlen` is byte length;
- normal validator redirection does not use `EXCP_EMULEAVE`.

`MSSTP` is required but does not guarantee one translated node equals one semantic emu86 step. REP micro-iterations and interrupt-shadow composition are normalized at the adapter boundary.

### 2.2 Force the execution path containing the hook

The verified #17 runtime forces the interpreter path and shared-memory mapping driver with direct dosemu2 config commands passed through `-I`:

```text
cpu_vm emulated
cpuemu 1
cpu_vm_dpmi emulated
mappingdriver mapshm
```

The launcher also supplies:

```text
DIIS_DOSEMU_VALIDATOR=1
DIIS_DOSEMU_MZ_CS=<relative MZ header CS>
DIIS_DOSEMU_MZ_IP=<MZ header IP>
DIIS_DOSEMU_TARGET_DOS_PATH=<canonical DOS path>
```

The inherited DOSBox-X `-hydra` / `-hydra-conf` arguments are not part of this contract.

### 2.3 Real-mode import fails closed

Phase 1 state import is real-mode only. Merged #17 patch 0005 checks `PROTMODE()` before applying any externally supplied state. In protected mode it publishes the unmodified current CPU state with `DIIS_STEP_FAULT`, acknowledges the pending request, executes no controlled node, and mutates no selectors, segment caches, GPRs, FLAGS, or PC.

This is a runtime guard, not an assertion.

## 3. Shared control ABI

The contract is the versioned 80-byte ABI:

```c
struct diis_validator_shm {
    uint32_t abi_version;
    uint32_t struct_size;
    uint32_t init;
    uint32_t end;
    int32_t  pid;
    uint16_t runtime_psp;
    uint16_t reserved0;
    uint64_t req;
    uint64_t ack;
    uint32_t decoded_instructions;
    uint32_t step_flags;
    uint16_t ax, bx, cx, dx;
    uint16_t si, di, bp, sp;
    uint16_t ip, cs, ds, es, ss, flags;
    uint32_t reserved1;
};
```

Required layout checks include:

```c
_Static_assert(offsetof(struct diis_validator_shm, req) == 24, "req offset");
_Static_assert(offsetof(struct diis_validator_shm, ack) == 32, "ack offset");
_Static_assert(offsetof(struct diis_validator_shm, decoded_instructions) == 40,
               "metadata offset");
_Static_assert(sizeof(struct diis_validator_shm) == 80, "ABI size");
```

`abi_version` is currently `1`. `/hydra_remote` is page-sized even though the structure is 80 bytes.

Current step flags:

```text
DIIS_STEP_MULTI_INSN   = 1 << 0
DIIS_STEP_SAME_PC      = 1 << 1
DIIS_STEP_FAULT        = 1 << 2
DIIS_STEP_END_ACK      = 1 << 3
DIIS_STEP_TARGET_EXIT  = 1 << 4
```

Ordering contract:

- controller writes request payload, then release-stores `req`;
- dosemu2 acquire-loads `req` before reading payload;
- ordinary publication writes CPU/metadata, release-publishes `step_flags`, then release-stores `ack`;
- controller acquire-loads `ack` before reading an ordinary result;
- asynchronous lifecycle events (`END_ACK`, `TARGET_EXIT`) are release-published through `step_flags`, which the controller acquire-loads even if `req`/`ack` do not change.

Rust must validate ABI version, structure size, mapping size, and child PID before trusting the payload.

## 4. Target identity and lifecycle

Target activation is executable-scoped rather than inferred from a PSP signature or relative MZ entry alone. Merged #17 validates:

1. expected relative MZ `CS:IP`;
2. PSP signature;
3. owning MCB consistency;
4. environment block program path against `DIIS_DOSEMU_TARGET_DOS_PATH`;
5. current PSP through dosemu's version-adjusted `sda_cur_psp()`.

On activation, dosemu2 publishes `runtime_psp` and the initial CPU snapshot, then release-publishes `init`.

After activation:

- target PSP consumes validator requests;
- descendants discovered through dosemu's PSP parent field may run without consuming target requests;
- `end` is checked before descendant bypass;
- leaving target ancestry permanently latches target exit;
- `runtime_psp` is cleared and `DIIS_STEP_TARGET_EXIT` is published;
- any pending request is acknowledged;
- stale PSP reuse cannot reactivate the target gate.

The policy is implemented; target/child/exit runtime coverage is still required.

## 5. Initialization handshake

The initial shared snapshot is setup, not a completed instruction.

After `init`:

1. validate ABI and child PID;
2. read `runtime_psp`;
3. construct/rebase emu86 using #12's configurable PSP load path;
4. load the same MZ at `PSP + 0x10`, including relocations and entry/stack segments;
5. apply #12's initial-state normalization policy;
6. verify compared initial registers and load-dependent memory before request 1.

No Phase 1 code assumes a permanently fixed PSP such as `0x0813`.

## 6. Normalized stepping contract

A **raw step** is one request/ack exchange with the dosemu2 hook. A **normalized step** advances dosemu2 to the same comparison boundary as one or more emu86 decoded instructions.

The hook reports actual translated-node consumption from `TNode.seqnum` as `decoded_instructions` and boundary facts through `step_flags`. The adapter uses those measurements and authoritative pre/post `CS:IP`; it does not guess from opcode class alone.

### 6.1 Direct REP

Under `MSSTP`, a REP string instruction may execute one raw micro-iteration while remaining at the same `CS:IP`.

For one normalized REP step, coalesce raw requests until the REP reaches its semantic exit boundary, using published `CS:IP` and `DIIS_STEP_SAME_PC`. emu86 then performs its single REP-aware semantic step and compares once.

CMPS/REP support is already merged in #12.

### 6.2 Interrupt shadow

`STI`, `MOV SS`, and `POP SS` may place the following decoded instruction in the same translated node. `decoded_instructions` determines the actual span.

If a combined shadow node already executed the first iteration of a following REP instruction, that iteration is already consumed. The adapter must coalesce only the remaining raw REP iterations and compare at the final shared semantic boundary. It must never execute or compare the first REP iteration twice.

This semantic normalization remains a runtime-proof item.

## 7. Low-memory export

`/dosemu_mem` aliases the **same backing object** used for dosemu2's `MAPPING_LOWMEM` allocation. It is not a copied snapshot or unrelated mapping.

Merged #17 patch 0002:

- requires the POSIX `mapshm` driver;
- retains the same POSIX-SHM object/fd used for `MAPPING_LOWMEM`;
- accepts the real backing allocation even when it is larger than the `LOWMEM_SIZE + HMASIZE` visible low-memory window;
- verifies the export is the backing containing `lowmem_base` and conventional address zero resolves through that backing;
- proves bidirectional visibility through a temporary second `MAP_SHARED` view before guest boot;
- unlinks `/dosemu_mem` when the backing mapping is freed.

The external bidirectional alias proof is green in #17. Rust should still treat a missing/invalid export as startup failure.

## 8. Shutdown contract

`end` is a **zero-more-controlled-guest-nodes barrier**.

Merged #17 proves the end-barrier runtime path: when dosemu2 observes `end` before a controlled node begins, it publishes final state with `DIIS_STEP_END_ACK`, acknowledges the current request value, and starts no subsequent controlled guest node.

The parent remains responsible for process lifetime. `DosemuProcess::Drop` must:

1. release-store `end`;
2. observe final acknowledgement when practical;
3. terminate the child if needed;
4. explicitly reap it with `wait()` or equivalent.

A `kill()` without `wait()` is incomplete.

## 9. Completion gates

### Landed prerequisites

- [x] CMPS and REP-aware CMPS behavior in emu86 (#12)
- [x] configurable runtime PSP loading (#12)
- [x] shared initial-state normalization helper (#12)
- [x] shift semantics aligned with pinned simx86 (#14)

### dosemu2 carrier (#17, merged)

- [x] page-sized versioned control mapping
- [x] request/apply and publish/ack hook
- [x] `TNode.seqnum` metadata
- [x] executable-scoped target identity and lifecycle policy
- [x] live `MAPPING_LOWMEM` export and provenance checks
- [x] release-ordered lifecycle flag publication
- [x] protected-mode state import rejected before mutation
- [x] complete five-patch apply/compile/link gate green
- [x] pinned FDPP + comcom32 runtime provisioning green
- [x] external `/dosemu_mem` bidirectional alias proof green
- [x] validator end-barrier runtime proof green

### Rust adapter

- [ ] verified dosemu2 launch/config/env contract
- [ ] old DOSBox-X Hydra CLI arguments removed
- [ ] 80-byte ABI/version/size/PID validation
- [ ] runtime PSP initialization handshake with #12 load configuration
- [ ] raw-step primitive
- [ ] metadata-driven normalized stepping
- [ ] explicit `TARGET_EXIT`, `FAULT`, and `END_ACK` handling
- [ ] child termination and explicit reap

### Remaining semantic runtime proof

- [ ] target -> child -> target pauses/resumes request consumption correctly
- [ ] target -> parent publishes terminal exit and stale PSP reuse cannot reactivate
- [ ] REP and interrupt-shadow boundaries match emu86, including shadow + REP composition
- [ ] DOS/INT 21h handler execution is characterized for unwanted validator-visible boundaries

PR #10 should remain draft until the Rust adapter and remaining semantic runtime-proof items are linked and reviewable.
