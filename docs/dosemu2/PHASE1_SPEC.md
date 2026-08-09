# Phase 1 Specification: `simx86` Validator Integration

**Target milestone:** Phase 1 — deterministic dosemu2/emu86 differential stepping  
**Pinned dosemu2 source:** `dosemu2/dosemu2@604ce0cdd1a71f657e2a2df623d216d5ab289313`  
**Scope:** 16-bit real-mode MZ executables; one concurrent validator instance  
**Status:** Architecture contract with concrete implementation work in #12, #14, and draft carrier #17. The Rust dosemu2 adapter and runtime proof remain incomplete.

> PR #10 is the architecture contract. It must describe the implementation that now exists rather than preserve superseded Phase 0 sketches.

## 1. Current implementation map

- **#12 — merged:** CMPS/REP semantics, configurable runtime PSP loading, and initial-state normalization for emu86.
- **#14 — merged:** SHL/SHR/SAR behavior aligned with the pinned dosemu2 `simx86` interpreter.
- **#17 — draft:** four-patch dosemu2 carrier series implementing the validator control hook, verified low-memory export, target lifecycle handling, and atomic lifecycle flags.
- **Rust `DosemuProcess` adapter — pending:** the current main-branch adapter still uses the inherited DOSBox-X launch path and the old shared-memory contract.

At this consolidation point, #17's ordinary `test` workflow is green but its `dosemu2 patch series` workflow is red on the current head. Therefore source/apply/compile/link gates must not be described as currently green until that workflow is restored.

## 2. Non-negotiable architecture

### 2.1 Hook the persistent `FindExecCode()` boundary

`FindExecCode()` owns the authoritative local `PC`. Validator synchronization is split around `DoExec(G)`:

```text
pre-node:
  reject/stop unsupported state before mutation
  observe end/target lifecycle
  wait for request when the target owns the node
  apply requested register state
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

- an externally supplied `CS:IP` must change local `PC` **before** node lookup;
- post-node `IP` is `next_PC - LONG_CS`, not a possibly stale `TheCPU.eip`;
- `TNode::seqlen` is byte length and is never an instruction count;
- normal validator redirection must not use `EXCP_EMULEAVE`.

`MSSTP` is required but does not imply one translated node always equals one semantic emu86 step. REP micro-iterations and interrupt-shadow nodes are normalized at the adapter boundary.

### 2.2 Force the execution path that contains the hook

Validator launch must force CPU emulation, the C `simx86` interpreter, and the POSIX shared-memory mapping driver:

```text
$_cpu_vm = "emulated"
$_cpuemu = (1)
$_mapping = "mapshm"
```

`$_cpuemu = (1)` is not sufficient by itself: if `$_cpu_vm` remains `"auto"`, KVM/vm86 may bypass `simx86` and therefore bypass the validator hook.

The launcher also supplies:

```text
DIIS_DOSEMU_VALIDATOR=1
DIIS_DOSEMU_MZ_CS=<relative MZ header CS>
DIIS_DOSEMU_MZ_IP=<MZ header IP>
DIIS_DOSEMU_TARGET_DOS_PATH=<canonical DOS path>
```

The old `DIIS_DOSEMU_EXE` spelling and the inherited DOSBox-X `-hydra` / `-hydra-conf` arguments are not part of this contract.

### 2.3 Real-mode import must fail closed

Phase 1 state import is real-mode only. Before applying **any** externally supplied CPU or segment state, the hook must explicitly reject unsupported protected-mode state through a runtime error/stop path.

`assert(!PROTMODE())` is not an implementation of this contract: it disappears under `NDEBUG` and otherwise aborts the process. The check must execute in release builds and must occur before register or descriptor-cache mutation.

The current #17 carrier still needs this explicit pre-import guard; it remains an implementation gap, not a solved item.

## 3. Shared control ABI

The dosemu2 implementation in #17 extends the old Phase 0 structure. The architecture contract is the versioned 80-byte ABI:

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

Required layout checks:

```c
_Static_assert(offsetof(struct diis_validator_shm, req) == 24, "req offset");
_Static_assert(offsetof(struct diis_validator_shm, ack) == 32, "ack offset");
_Static_assert(offsetof(struct diis_validator_shm, decoded_instructions) == 40,
               "metadata offset");
_Static_assert(sizeof(struct diis_validator_shm) == 80, "ABI size");
```

`abi_version` is currently `1`. `/hydra_remote` is backed by a page-sized POSIX-SHM object even though the structure is 80 bytes.

Current step flags are:

```text
DIIS_STEP_MULTI_INSN   = 1 << 0
DIIS_STEP_SAME_PC      = 1 << 1
DIIS_STEP_FAULT        = 1 << 2
DIIS_STEP_END_ACK      = 1 << 3
DIIS_STEP_TARGET_EXIT  = 1 << 4
```

Ordering contract:

- controller writes request payload, then release-stores `req`;
- dosemu2 acquire-loads `req` before reading the payload;
- ordinary node publication writes CPU/metadata, release-publishes `step_flags`, then release-stores `ack`;
- controller acquire-loads `ack` before reading an ordinary step result;
- asynchronous lifecycle events such as `END_ACK` and `TARGET_EXIT` are release-published through `step_flags`, so the controller can acquire-load the flags even when `req`/`ack` do not change.

The Rust side must validate at least ABI version, structure size, mapping size, and child PID before trusting the payload.

## 4. Target identity and lifecycle

Target identity is not inferred from a PSP signature or relative MZ entry coordinates alone.

The #17 design binds activation to the requested canonical DOS path and validates the candidate process through DOS-visible state:

1. real-mode entry state has the expected relative MZ `CS:IP`;
2. the candidate PSP has the standard PSP signature;
3. its owning MCB is consistent with that PSP;
4. its environment block contains the requested program path;
5. dosemu's version-adjusted `sda_cur_psp()` reports that same PSP as the current process.

On activation, dosemu2 publishes `runtime_psp` and the initial CPU snapshot, then release-publishes `init`.

After activation, process ownership is lifecycle-aware:

- while current PSP equals the captured target PSP, validator requests control target nodes;
- descendant PSPs discovered through dosemu's `struct PSP::parent_psp` ancestry may run as child/helper processes without consuming target requests;
- `end` remains global and is checked before child/helper bypass;
- once current PSP leaves the target ancestry, target exit is latched permanently;
- `runtime_psp` is cleared and `DIIS_STEP_TARGET_EXIT` is published;
- any pending request is acknowledged;
- a later reuse of the old PSP cannot reactivate the target gate.

This replaces the earlier proposed DOS-exec callback/`g_target_exec_seen` contract. The architecture requirement is executable-scoped identity plus lifecycle correctness; the concrete #17 mechanism above is the current source-verified implementation path.

## 5. Initialization handshake

The initial shared snapshot is a setup handshake, not a completed instruction.

After `init`:

1. validate the ABI and dosemu child PID;
2. read `runtime_psp`;
3. construct/rebase emu86 with #12's configurable PSP load path;
4. load the same MZ at `PSP + 0x10`, including relocations and entry/stack segments;
5. apply the shared initial-state normalization policy from #12;
6. verify the compared initial register set and load-segment-dependent memory before request 1.

No Phase 1 code should claim emu86 is permanently fixed at PSP `0x0813`; that is only the default load configuration retained for compatibility.

## 6. Normalized stepping contract

A **raw step** is exactly one request/ack exchange with the dosemu2 hook. A **normalized step** advances dosemu2 to the same comparison boundary as one or more emu86 decoded instructions.

The dosemu2 hook reports actual translated-node consumption from `TNode.seqnum` as `decoded_instructions`. It also reports raw boundary facts through `step_flags`. The adapter must use those measurements and the authoritative pre/post `CS:IP`; it must not guess a comparison span from opcode class alone.

### 6.1 Direct REP case

Under `MSSTP`, a REP string instruction may execute one iteration per raw node while remaining at the same `CS:IP`.

For a normalized REP step, issue raw requests until the REP instruction reaches its semantic exit boundary. `DIIS_STEP_SAME_PC` and the published `CS:IP` provide the raw-node boundary signal. emu86 then performs its one REP-aware `Machine::step()` for comparison.

CMPS support is already a merged prerequisite in #12 and must not be described as future work.

### 6.2 Interrupt-shadow case

`STI`, `MOV SS`, and `POP SS` may place the following decoded instruction in the same translated node. The actual `decoded_instructions` value determines whether that happened.

If a combined shadow node has already executed the first iteration of a following REP instruction, that iteration is already consumed. The adapter must account for it, coalesce only the remaining raw REP iterations, then compare at the final shared semantic boundary. It must never execute or compare the first REP iteration twice.

`DIIS_STEP_FAULT` is a reported raw-node condition and must be propagated as a validator error/boundary condition rather than silently ignored.

## 7. Low-memory export

`/dosemu_mem` must alias the **same backing object** used for dosemu2's `MAPPING_LOWMEM` allocation. A copied snapshot or unrelated shared-memory allocation is invalid.

Validator mode therefore:

- requires `mapshm`;
- special-cases only `MAPPING_LOWMEM`;
- retains the same fd/object used by `alloc_mapping_file()`;
- exposes that live object as `/dosemu_mem`;
- verifies bidirectional visibility through a temporary second `MAP_SHARED` mapping and restores probe bytes;
- verifies the selected mapping driver, allocation size/provenance, and that conventional address zero resolves through `lowmem_base`;
- unlinks the named object when the backing allocation is freed.

All unrelated EMS/XMS/DPMI/VGA/generic mappings retain their normal allocation path.

The fixed `/hydra_remote` and `/dosemu_mem` names intentionally constrain Phase 1 to one concurrent validator instance.

## 8. Shutdown contract

`end` is a **zero-more-controlled-guest-nodes barrier**.

When dosemu2 observes `end` before a controlled node begins, it must publish final state with `DIIS_STEP_END_ACK`, acknowledge the current request value, and execute no subsequent controlled guest node. The current #17 implementation then leaves dosemu execution through its explicit end path; this is distinct from normal `CS:IP` redirection and does not make `EXCP_EMULEAVE` a valid redirection primitive.

The parent remains responsible for process lifetime. `DosemuProcess::Drop` must:

1. release-store `end`;
2. allow/observe the final acknowledgement when practical;
3. terminate the child if it has not exited;
4. explicitly reap it with `wait()` or an equivalent operation.

A `kill()` without `wait()` is incomplete because it can leave a zombie in a long-lived validator process.

## 9. Completion gates

### Landed prerequisites

- [x] CMPS and REP-aware CMPS behavior in emu86 (#12)
- [x] configurable runtime PSP loading (#12)
- [x] shared initial-state normalization helper (#12)
- [x] shift semantics aligned with pinned simx86 (#14)

### dosemu2 carrier contract

- [x] page-sized versioned control mapping represented in #17
- [x] request/apply and publish/ack hook represented in #17
- [x] `TNode.seqnum`/step metadata represented in #17
- [x] executable-scoped target identity and lifecycle represented in #17
- [x] live `MAPPING_LOWMEM` export and provenance checks represented in #17
- [x] atomic lifecycle flag publication represented by #17 patch 0004
- [ ] explicit protected-mode rejection before state import
- [ ] current `dosemu2 patch series` CI restored to green

### Rust adapter

- [ ] launch forces `$_cpu_vm = "emulated"`, `$_cpuemu = (1)`, and `$_mapping = "mapshm"`
- [ ] old DOSBox-X Hydra CLI arguments removed
- [ ] 80-byte ABI/version/size/PID validation implemented
- [ ] runtime PSP initialization handshake implemented with #12 load configuration
- [ ] boundary-driven raw/normalized stepping implemented from reported metadata
- [ ] `TARGET_EXIT`, `END_ACK`, and faults handled explicitly
- [ ] `Drop` terminates **and reaps** the child

### Runtime proof

- [ ] no controlled guest node begins after `end` is observed
- [ ] target -> child -> target pauses and resumes target request consumption correctly
- [ ] target -> parent publishes terminal exit and stale PSP reuse cannot reactivate
- [ ] external `/dosemu_mem` mapping observes bidirectional live writes during a real run
- [ ] REP and interrupt-shadow boundaries match emu86, including shadow + REP composition
- [ ] DOS/INT 21h handler execution is tested for unwanted validator-visible boundaries while current PSP remains the target

PR #10 should remain draft while these implementation and runtime-proof items are open.
