# Phase 1 Implementation Guide: dosemu2 `simx86` Validator

**Pinned dosemu2 base:** `604ce0cdd1a71f657e2a2df623d216d5ab289313`  
**Scope:** 16-bit real-mode MZ executables; one concurrent validator instance  
**Status:** emu86 prerequisites are merged; the dosemu2 carrier exists in draft #17; the Rust adapter and runtime proof remain incomplete.

This guide maps the Phase 1 architecture to the implementation that now exists. It replaces older speculative sketches where #12/#14/#17 have established a concrete contract.

## 1. Implementation status

### 1.1 emu86/dis86 prerequisites

Merged in #12:

- CMPS byte/word execution and REP/REPE/REPNE termination;
- configurable PSP placement through the runtime load configuration;
- PSP/load-segment rebasing, relocation, entry, and stack handling;
- a pure initial-state normalization helper.

Merged in #14:

- SHL/SHR/SAR semantics aligned with the pinned dosemu2 `simx86` interpreter.

Do not reimplement these as Phase 1 TODOs. The Rust validator adapter should consume them.

### 1.2 dosemu2 carrier #17

The current carrier consists of four ordered patches:

1. `0001-simx86-add-validator-control-abi.patch`
   - page-sized `/hydra_remote`;
   - versioned 80-byte control ABI;
   - target MZ/path gate;
   - request/apply + publish/ack hook around `DoExec(G)`;
   - `MSSTP` control;
   - register serialization and authoritative PC recomputation;
   - `decoded_instructions = TNode.seqnum`;
   - raw `step_flags`;
   - pre-node `end` acknowledgement.
2. `0002-mapping-export-validator-lowmem.patch`
   - requires the `mapshm` driver;
   - exports the live `MAPPING_LOWMEM` backing as `/dosemu_mem`;
   - checks size/provenance and address-zero aliasing;
   - verifies writes in both directions through a second `MAP_SHARED` view.
3. `0003-simx86-harden-validator-target-lifecycle.patch`
   - requires entry PSP == dosemu current PSP;
   - uses `sda_cur_psp()` and `struct PSP::parent_psp`;
   - lets descendants run without consuming target requests;
   - latches target exit and prevents stale PSP reactivation;
   - publishes `DIIS_STEP_TARGET_EXIT`.
4. `0004-simx86-publish-validator-step-flags-atomically.patch`
   - release-publishes `step_flags` so asynchronous `END_ACK`/`TARGET_EXIT` events have their own synchronization point.

The current #17 head has a green ordinary `test` workflow and a failing `dosemu2 patch series` workflow. The source/apply/compile/link proof must therefore be re-established before it is described as current green evidence.

### 1.3 Rust adapter

The main-branch `DosemuProcess` is still Phase-0-era code. It still launches with DOSBox-X `-hydra` arguments, consumes the old shared layout, performs only one raw req/ack per `step()`, and kills without an explicit reap in `Drop`.

That adapter is the main remaining implementation surface in `dis86`.

## 2. Launch contract

The adapter must launch the patched dosemu2 through the execution path that actually reaches `simx86`:

```rust
cmd.args(["-dumb", "-quiet", "-K", test_dir, "-E", test_exe]);
cmd.args(["-I", "$_cpu_vm = \"emulated\""]);
cmd.args(["-I", "$_cpuemu = (1)"]);
cmd.args(["-I", "$_mapping = \"mapshm\""]);

cmd.env("DIIS_DOSEMU_VALIDATOR", "1");
cmd.env("DIIS_DOSEMU_MZ_CS", mz_cs.to_string());
cmd.env("DIIS_DOSEMU_MZ_IP", mz_ip.to_string());
cmd.env("DIIS_DOSEMU_TARGET_DOS_PATH", canonical_dos_path);
```

Remove:

```text
-hydra <libhydraremote.so>
-hydra-conf normal
```

Those are DOSBox-X integration arguments and are not the dosemu2 Phase 1 mechanism.

`DIIS_DOSEMU_TARGET_DOS_PATH` must identify the path visible to DOS. The dosemu2 gate normalizes slash/case when comparing it with the program name stored in the PSP environment trailer.

## 3. Shared ABI

Rust must model the dosemu2 layout exactly rather than reusing the old 64-byte Hydra structure:

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
offset 48  u16 ax
           ... bx,cx,dx,si,di,bp,sp,ip,cs,ds,es,ss,flags ...
offset 76  u32 reserved1
size       80 bytes
```

ABI version is currently `1`.

Step flags:

```rust
const DIIS_STEP_MULTI_INSN:  u32 = 1 << 0;
const DIIS_STEP_SAME_PC:     u32 = 1 << 1;
const DIIS_STEP_FAULT:       u32 = 1 << 2;
const DIIS_STEP_END_ACK:     u32 = 1 << 3;
const DIIS_STEP_TARGET_EXIT: u32 = 1 << 4;
```

The backing file remains page-sized. `ShmData::attach()` should reject incompatible `abi_version`, `struct_size`, undersized backing mappings, or a PID that does not match the spawned child.

### Ordering

Use the control fields as synchronization points:

```text
controller payload writes
store_release(req)
        |
        v
load_acquire(req)
dosemu2 applies state
executes one controlled translated node
publishes CPU + metadata
store_release(step_flags)
store_release(ack)
        |
        v
load_acquire(ack)
controller reads ordinary step result
```

For asynchronous lifecycle events that need not change `req`/`ack`, acquire-load `step_flags` and then read the preceding payload.

## 4. Target initialization

The adapter does **not** guess dosemu's load segment.

Startup sequence:

1. remove stale fixed shared-memory paths before spawning;
2. parse the same target MZ header used by emu86;
3. launch dosemu2 with the exact contract in section 2;
4. attach `/hydra_remote` and validate ABI/PID;
5. attach `/dosemu_mem`;
6. wait for release-published `init` or child failure/timeout;
7. read `runtime_psp` and initial CPU state;
8. instantiate emu86 with #12's configurable PSP equal to `runtime_psp`;
9. apply #12's initial-state normalization policy;
10. verify compared registers and load-dependent memory before request 1.

The dosemu2 side activates only when the requested program identity matches its PSP/MCB/environment state and that PSP is also dosemu's current PSP.

The old `g_target_exec_seen`/DOS-exec-callback sketch is not the current #17 implementation and should not be reintroduced unless a later source review demonstrates a need for it.

## 5. CPU state import/export

### Export

Post-node `IP` must come from the authoritative `next_PC` returned by `DoExec(G)`:

```c
shm->ip = (uint16_t)(next_pc - LONG_CS);
```

The 16-bit register payload remains the compared ABI, while dosemu2 preserves upper halves of 32-bit registers/EFLAGS on import.

### Import and protected mode

State import must reject protected mode **before any mutation**:

```text
if PROTMODE():
    publish/return an explicit validator failure or stop
    execute no controlled node
    mutate no register or segment cache
else:
    apply 16-bit register payload
    SetSegReal() for changed CS/DS/ES/SS
    recompute local PC from imported CS:IP
```

Do not use `assert(!PROTMODE())`.

The current #17 `apply_cpu()` still lacks this explicit guard. This is an open carrier implementation issue and should remain visible in review/exit criteria.

Normal `CS:IP` mutation must not set `EXCP_EMULEAVE`. The explicit shutdown path is a separate lifecycle operation.

## 6. Raw step primitive

Refactor `DosemuProcess` so one raw request/ack is an internal primitive:

```text
raw_step():
  fail if TARGET_EXIT already latched
  write requested CPU payload if changed
  next_req = ack + 1
  store_release(req = next_req)
  wait until:
    acquire ack == next_req, or
    acquire step_flags reports terminal async lifecycle, or
    child exits / timeout
  read CPU + decoded_instructions + step_flags
  reject FAULT as required by validator policy
  return RawStep
```

`RawStep` should carry at least:

```text
start CS:IP
end CS:IP
decoded_instructions
step_flags
```

Do not derive the decoded count from `seqlen` or from the first opcode.

## 7. Normalized step primitive

`DosemuProcess::step()` should normalize raw dosemu nodes to the comparison boundary expected by emu86.

### 7.1 Ordinary node

For an ordinary node:

- issue one raw step;
- use `decoded_instructions` as the actual decoded span;
- advance emu86 by that many semantic decoded instructions before comparing.

The backend abstraction can expose the normalized comparison span rather than assuming one for every backend.

### 7.2 Direct REP

When the active emu86 instruction is REP-prefixed, dosemu2 may remain at the same REP `CS:IP` after one raw micro-iteration.

Coalesce raw steps until the REP instruction reaches its exit boundary. Use published `CS:IP` and `DIIS_STEP_SAME_PC`; do not treat each raw iteration as a separate emu86 comparison.

Then execute the one REP-aware emu86 step and compare once.

### 7.3 Interrupt shadow

`STI`, `MOV SS`, and `POP SS` may consume the following decoded instruction in the same translated node. `TNode.seqnum` reports what the node actually decoded, so use `decoded_instructions` rather than assuming a fixed span of two.

Special composition:

```text
shadow instruction + first iteration of following REP
```

If the raw node has already consumed that first REP iteration:

1. count the REP instruction as already entered/consumed by the node;
2. issue additional raw requests only for the remaining REP micro-iterations;
3. stop when the REP `CS:IP` advances to its final semantic boundary;
4. advance emu86 across the shadow instruction and its one full REP semantic step;
5. compare once at the shared final state.

Never execute or compare the first REP iteration twice.

## 8. Target process lifecycle

After initialization, the dosemu2 hook uses current-PSP ancestry to decide whether a node belongs to the controlled target:

- target PSP: consume validator requests;
- descendant/helper PSP: execute without consuming a target request;
- PSP outside target ancestry: permanently latch target exit.

The Rust side must treat `DIIS_STEP_TARGET_EXIT` as terminal for that validator instance. It must not continue waiting for a future request acknowledgement or attempt to rediscover the same PSP.

One unresolved runtime question remains important: DOS/INT 21h handler guest nodes may execute while current PSP still names the target. Runtime testing must establish whether those nodes become unwanted validator-visible boundaries. If they do, add an explicit normalization/filtering rule; do not infer a fix from PSP identity alone.

## 9. Low-memory attachment

`ShmMem::attach("/dev/shm/dosemu_mem")` is valid only after the patched dosemu2 verifies that the named object is the live low-memory backing.

The dosemu2 side already encodes these checks in #17:

- selected driver is `mapshm`;
- allocation is the `MAPPING_LOWMEM`/`lowmem_base` allocation;
- size is `LOWMEM_SIZE + HMASIZE`;
- conventional address zero resolves through that backing;
- a temporary second shared mapping sees writes in both directions;
- `/dosemu_mem` is unlinked when the backing allocation is freed.

The Rust adapter should treat missing/invalid low-memory export as startup failure, not fall back to copied memory.

## 10. Shutdown

Required parent behavior:

```rust
impl Drop for DosemuProcess {
    fn drop(&mut self) {
        self.data.store_end(1, Ordering::Release);

        // Optionally observe END_ACK / normal child exit for bounded cleanup.
        if self.dosemu.try_wait().ok().flatten().is_none() {
            let _ = self.dosemu.kill();
        }

        // Required: reap even after kill.
        let _ = self.dosemu.wait();
    }
}
```

Exact bounded-wait policy may vary, but the semantic requirements do not:

- release-publish `end`;
- dosemu2 checks it before starting another controlled node;
- dosemu2 release-publishes `DIIS_STEP_END_ACK` and acknowledges current `req`;
- parent terminates if necessary;
- parent reaps with `wait()` or equivalent.

The normal state-redirection path still must not use `EXCP_EMULEAVE`.

## 11. Verification matrix

### Already landed

- [x] CMPS including REP/REPE/REPNE emu86 behavior (#12)
- [x] configurable PSP/rebased MZ loading (#12)
- [x] initial-state normalization helper (#12)
- [x] dosemu2-compatible shift semantics (#14)

### Carrier/source implementation

- [x] versioned/page-sized control mapping represented in #17
- [x] authoritative PC hook split around `DoExec(G)` represented in #17
- [x] actual decoded-node count from `TNode.seqnum` represented in #17
- [x] target PSP/path/current-process lifecycle represented in #17
- [x] same-backing low-memory export/provenance checks represented in #17
- [x] atomic `step_flags` publication represented in patch 0004
- [ ] protected-mode state import rejected explicitly before mutation
- [ ] current `dosemu2 patch series` workflow green

### Rust adapter

- [ ] corrected dosemu2 launch options/env
- [ ] extended ABI model and validation
- [ ] runtime-PSP initialization handshake
- [ ] raw-step primitive
- [ ] decoded-count/flag-driven normalized stepping
- [ ] REP and shadow+REP coalescing
- [ ] terminal `TARGET_EXIT` handling
- [ ] `FAULT` handling
- [ ] `END_ACK`-aware shutdown
- [ ] explicit child reap

### Runtime proof

- [ ] exact target entry activates once and only for requested program identity
- [ ] external `CS:IP` mutation performs fresh node lookup
- [ ] segment mutation updates real-mode descriptor caches
- [ ] no controlled node begins after end acknowledgement path is entered
- [ ] target -> child -> target pauses/resumes target request consumption
- [ ] target -> parent terminates validation and stale PSP reuse cannot reactivate
- [ ] `/dosemu_mem` is bidirectionally live from an external process during a real run
- [ ] REP MOVS/STOS/SCAS/CMPS normalize correctly
- [ ] STI/MOV SS/POP SS measured spans normalize correctly
- [ ] shadow + REP composition does not double-consume the first REP iteration
- [ ] INT 21h/DOS-handler boundary behavior is characterized and normalized if necessary

PR #10 remains an architecture PR until the Rust adapter and runtime-proof gates above are satisfied.
