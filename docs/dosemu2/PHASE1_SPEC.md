# Phase 1 Specification: `simx86` Core Hook Integration & Lockstep Synchronization

**Target Milestone:** Phase 1 — `simx86` Core Hook Integration  
**Target Projects:** `dosemu2` (`src/base/emu-i386/simx86/`, `src/base/lib/mapping/`), `dis86` (`dis86/src/emu86/validator/`)  
**Status:** Source-verified implementation contract

---

## 1. Executive Summary

Phase 1 establishes deterministic validator stepping and bidirectional shared-memory synchronization between `dosemu2`'s `simx86` CPU simulator and `dis86`'s `emu86_validator`.

The source-verified contract is:

1. Instrument the persistent `FindExecCode()` dispatch loop, with separate pre-execution request/apply and post-execution publish/ack phases.
2. Reassert `MSSTP` after `FindExecCode()` clears `MSSTP|MTRAP`; do **not** use `TNode::seqlen` as an instruction count.
3. Treat the local `PC` in `FindExecCode()` as authoritative. External `CS:IP` mutations recompute `PC` before node lookup, and post-step `IP` is published as `PC - LONG_CS`.
4. Do **not** use `EXCP_EMULEAVE` for normal validator control redirection. In current dosemu2 it leaves instruction-simulation mode via `instr_sim_leave()`.
5. Preserve high halves of 32-bit registers and EFLAGS when importing the 16-bit validator ABI.
6. Keep Phase 1 strictly real-mode. Segment mutation while `PROTMODE()` is true is a validation error; it is not silently ignored.
7. Normalize `simx86` node boundaries to the validator's decoded-instruction comparison boundary. `MSSTP` exposes REP iterations separately, while `STI`, `MOV SS`, and `POP SS` may include the following instruction in the same generated node.
8. Engage lockstep only for the DOS process created for the requested executable: capture that process's PSP in the DOS exec path, then require its exact MZ entry coordinates.
9. Export low memory through a dedicated named POSIX-SHM backing object for the `MAPPING_LOWMEM` allocation only. The generic mapping-object allocator remains unchanged.

Phase 1 is deliberately scoped to **16-bit real-mode MZ executables** and a single validator/dosemu2 instance using the fixed Phase 0 paths `/dev/shm/hydra_remote` and `/dev/shm/dosemu_mem`.

---

## 2. `simx86` Dispatch Integration

### 2.1 Hook placement

`Interp86()` computes the initial linear address and calls `FindExecCode()`. `FindExecCode()` owns the persistent dispatch loop and local `PC`, so the synchronization surface belongs there:

```text
FindExecCode(PC):
  while (1):
    validator pre-step:
      gate on exact target entry until initialized
      wait for req or end
      apply register mutations
      PC = LONG_CS + TheCPU.eip

    normal mode reset
    reassert MSSTP when validator is active

    lookup/generate node
    PC = DoExec(G)

    validator post-step:
      publish state using returned PC
      ack = req
```

The pre-step phase runs **before** node lookup. This prevents an externally rewritten `CS:IP` from executing a node selected for the old address.

### 2.2 `MSSTP` means one outer `InterpOne()` call, not universally one decoded instruction

Current `codegen.h` defines:

```c
#define MSSTP 0x02000000 /* generate only one instruction */
```

`FindExecCode()` clears `MSSTP|MTRAP` each iteration, so validator mode must reassert it after the normal reset:

```c
TheCPU.mode &= ~(MSSTP | MTRAP);
if (EFLAGS & TF)
    TheCPU.mode |= MSSTP | MTRAP;
if (dosemu_hydra_active())
    TheCPU.mode |= MSSTP;
```

`_Interp86()` then stops after one outer `InterpOne()` call. Two qualifications are essential:

- `TNode::seqlen` is **guest byte length**, not instruction count, and can be greater than one.
- `InterpOne()` has architectural special cases. With `MSSTP`, REP string instructions are converted to loop-style execution so a node can represent one REP iteration. Conversely, `POP SS`, `MOV SS`, and non-trap `STI` can recursively compile the following decoded instruction into the same node to preserve the interrupt-shadow semantics.

Therefore Phase 1 defines a **normalized comparison boundary**, not a false one-node-equals-one-instruction invariant.

### 2.3 No `EXCP_EMULEAVE` for control mutation

Normal validator redirection is handled entirely before node lookup:

```c
PC = dosemu_hydra_apply_state(); /* returns LONG_CS + TheCPU.eip */
```

Do not set `TheCPU.err = EXCP_EMULEAVE`. Current `cpu-emu.c` handles that exception by calling `instr_sim_leave()`, which leaves instruction simulation rather than merely restarting `FindExecCode()`.

### 2.4 Shutdown is a stop barrier

An observed `shm->end` must prevent any further guest instruction from executing. The pre-step helper returns a stop status and `FindExecCode()` immediately returns its current `PC`.

`end` is not the sole process-lifetime mechanism. `DosemuProcess::Drop` already owns process teardown and may terminate the child after publishing `end`. This keeps shutdown synchronization separate from dosemu2's global exit machinery.

---

## 3. Register and Segment State

### 3.1 Publish from authoritative `PC`

At the post-node boundary:

```c
static void publish_cpu(shmdata_t *shm, unsigned int pc)
{
    shm->ax    = (uint16_t)TheCPU.eax;
    shm->bx    = (uint16_t)TheCPU.ebx;
    shm->cx    = (uint16_t)TheCPU.ecx;
    shm->dx    = (uint16_t)TheCPU.edx;
    shm->si    = (uint16_t)TheCPU.esi;
    shm->di    = (uint16_t)TheCPU.edi;
    shm->bp    = (uint16_t)TheCPU.ebp;
    shm->sp    = (uint16_t)TheCPU.esp;
    shm->ip    = (uint16_t)(pc - LONG_CS);
    shm->cs    = TheCPU.cs;
    shm->ds    = TheCPU.ds;
    shm->es    = TheCPU.es;
    shm->ss    = TheCPU.ss;
    shm->flags = (uint16_t)TheCPU.eflags;
}
```

Inside `FindExecCode()`, `PC = DoExec(G)` is authoritative. `TheCPU.eip` is synchronized by `Interp86()` when `FindExecCode()` returns, so publishing `TheCPU.eip` after every node is stale.

### 3.2 Import validator mutations

```c
static unsigned int apply_cpu(const shmdata_t *shm)
{
    assert(!PROTMODE());

    TheCPU.eax = (TheCPU.eax & 0xffff0000u) | shm->ax;
    TheCPU.ebx = (TheCPU.ebx & 0xffff0000u) | shm->bx;
    TheCPU.ecx = (TheCPU.ecx & 0xffff0000u) | shm->cx;
    TheCPU.edx = (TheCPU.edx & 0xffff0000u) | shm->dx;
    TheCPU.esi = (TheCPU.esi & 0xffff0000u) | shm->si;
    TheCPU.edi = (TheCPU.edi & 0xffff0000u) | shm->di;
    TheCPU.ebp = (TheCPU.ebp & 0xffff0000u) | shm->bp;
    TheCPU.esp = (TheCPU.esp & 0xffff0000u) | shm->sp;
    TheCPU.eip = shm->ip;
    TheCPU.eflags = (TheCPU.eflags & 0xffff0000u) | shm->flags;

    if (TheCPU.cs != shm->cs) SetSegReal(shm->cs, Ofs_CS);
    if (TheCPU.ds != shm->ds) SetSegReal(shm->ds, Ofs_DS);
    if (TheCPU.es != shm->es) SetSegReal(shm->es, Ofs_ES);
    if (TheCPU.ss != shm->ss) SetSegReal(shm->ss, Ofs_SS);

    return LONG_CS + TheCPU.eip;
}
```

Protected-mode segment mutation is out of scope and must fail loudly. A guard that merely skips `SetSegReal()` while still accepting the new selector would leave the selector and cached descriptor inconsistent.

---

## 4. Exact Program-Entry Gate

The validator must not engage during BIOS, DOS, or command-shell startup.

`emu86` already models MZ loading with:

- `PSP_SEGMENT` as the process segment;
- image load segment `PSP + 0x10`;
- initial `CS = PSP + 0x10 + mz_header.cs`;
- initial `IP = mz_header.ip`;
- initial `DS = ES = PSP`.

`DosemuProcess` supplies both the canonical target path (or another unambiguous exec token) and the target executable's **relative** MZ `CS` and `IP`. The patched DOS exec path compares the executable being opened with that identity and records the PSP assigned to that successful exec in `g_target_psp`. A valid PSP with matching entry coordinates is insufficient: an AUTOEXEC/helper MZ can have the same, very common, relative `0:0` entry.

A source-compatible gate is:

```c
static bool at_target_entry(unsigned int pc)
{
    uint16_t psp;
    uint16_t ip;
    uint16_t expected_cs;

    if (PROTMODE())
        return false;
    if (TheCPU.ds != TheCPU.es)
        return false;

    psp = g_target_psp;
    if (!g_target_exec_seen || TheCPU.ds != psp || TheCPU.es != psp)
        return false;
    if (READ_WORD((dosaddr_t)psp << 4) != 0x20cd) /* PSP starts CD 20 */
        return false;

    ip = (uint16_t)(pc - LONG_CS);
    expected_cs = (uint16_t)(psp + 0x10u + g_target_mz_cs);

    return TheCPU.cs == expected_cs && ip == g_target_mz_ip;
}
```

On the first match, publish the complete initial state and release-store `init = 1`. Before the target exec callback has recorded its PSP, or before this match, no request wait occurs and dosemu2 continues normal startup execution. Clear the captured identity when that DOS process terminates so a later process cannot reuse a stale PSP.

The validator must then construct/rebase emu86 with this runtime PSP (including its PSP, image load at `PSP + 0x10`, relocation fixups, entry segments, and stack) and apply the same initial-register normalization used by the existing Hydra backend before issuing request 1. AX through BP and FLAGS must be copied/normalized explicitly; comparing an emu86 instance loaded at fixed `0x813` against a different dosemu load segment is invalid. The initial shared snapshot is a handshake, not an executed instruction.

This gate is intentionally scoped to the MZ executable path already used by `Emulator::new()`; COM support is a separate extension.

---

## 5. Normalized Validator Step Contract

### 5.1 Raw dosemu2 step

A raw request/ack advances one `simx86` generated node in validator mode.

### 5.2 REP normalization

Under `MSSTP`, `simx86` makes REP string operations visible one iteration at a time, while `emu86` currently completes the REP loop inside one `Machine::step()`.

`DosemuProcess::step()` therefore coalesces raw dosemu2 requests when the starting decoded instruction is a REP string instruction:

1. Record starting `CS:IP`.
2. Issue raw request/ack steps.
3. Continue while the published `CS:IP` remains the starting address.
4. Return one normalized step when `CS:IP` advances.

This covers count exhaustion and REP/REPNE condition termination without pretending each raw node is a decoded instruction.

### 5.3 Interrupt-shadow normalization

Current `simx86` may execute the instruction following `STI`, `MOV SS`, or `POP SS` in the same node, but it does not always do so (notably trap-mode `STI`). The hook or adapter derives the actual consumed boundary from generated-node metadata and the authoritative post-node PC; it must not infer a fixed span from the first opcode. The result is one when no following instruction was combined and two when it was.

If the combined second instruction is REP-prefixed and the node exposes only its first iteration, the adapter continues raw requests while `CS:IP` remains at that REP instruction before comparing. Normalization is boundary-driven: account for the instructions actually consumed, then coalesce unfinished REP micro-iterations at the tail.

The backend interface should expose the number of decoded instructions consumed by the last normalized step, with a default of one for `Emulator` and `HydraProcess`. `DosemuProcess` reports the measured decoded-instruction count, not a guessed opcode-based count.

This keeps dosemu2's existing interrupt-shadow implementation intact and moves backend-boundary normalization into the validator, where the semantic mismatch belongs.

---

## 6. Shared-Memory Ordering

The Phase 0 ABI remains:

```text
validator writes payload
store_release(req)
        |
        v
load_acquire(req)
dosemu2 applies payload
executes normalized raw node(s)
publishes payload
store_release(ack)
        |
        v
load_acquire(ack)
validator reads payload
```

`req`/`ack` are 64-bit atomics. Register payload fields remain ordinary shared-memory fields ordered by those control atomics.

---

## 7. Low-Memory Export Contract

The generic `do_open_pshm()` must remain unchanged. It services generic file-backed mappings and currently creates PID-specific objects which are immediately unlinked.

Full-simulation dosemu2 normally selects the `softmmu` mapping driver first; its allocator uses anonymous memory (`fd = -1`). Such memory has no pathname or fd that `ShmMem::attach()` can reopen.

Phase 1 therefore makes validator mode explicit:

1. `DosemuProcess` selects the existing POSIX-SHM backend with dosemu2's verified command-line configuration interface: `-I '$_mapping = "mapshm"'`. Do not rely on an inferred `dosemu__mapping` environment-variable spelling.
2. `DosemuProcess` also sets a validator-only marker such as `DIIS_DOSEMU_VALIDATOR=1`.
3. In `alloc_mapping_file()`, only when that marker is active **and** `cap & MAPPING_LOWMEM`, create `/dosemu_mem` as the backing object instead of calling generic `mfops->open()`.
4. All EMS/XMS/DPMI/VGA/other allocations continue through the normal PID-unique/unlinked object path.
5. Use `O_EXCL` after stale-path cleanup so a second live validator cannot silently truncate the first process's memory.
6. Keep the named object until dosemu2 mapping shutdown; `ShmMem` maps the same fd-backed pages with `MAP_SHARED`, providing true zero-copy visibility.

The fixed `/dosemu_mem` and `/hydra_remote` names deliberately imply a single concurrent Phase 1 validator instance. PID-scoped names require an ABI change and are deferred.

---

## 8. Launch Contract

The patched dosemu2 binary contains the core hook; it is not a DOSBox-X Hydra plugin.

Therefore `DosemuProcess` must remove the inherited DOSBox-X-only arguments:

```text
-hydra ...
-hydra-conf normal
```

Upstream dosemu2 has no such options.

The Phase 1 launch path is:

```text
DOSEMU_BIN=<patched dosemu2> \
DIIS_DOSEMU_VALIDATOR=1 \
DIIS_DOSEMU_MZ_CS=<relative header CS> \
DIIS_DOSEMU_MZ_IP=<header IP> \
  dosemu -I '$_mapping = "mapshm"' -dumb -quiet -K <test_dir> -E <test_exe>
```

Exact environment-variable names may be changed in the implementation patch, but the data flow and absence of DOSBox-X plugin flags are mandatory.

---

## 9. Verification Gate

Phase 1 is complete only when tests demonstrate:

- ordinary ALU, stack, near/far control-flow, and flag instructions compare at every normalized boundary;
- REP MOVS/STOS/SCAS/CMPS cases cross the raw-node/decoded-instruction boundary correctly;
- `STI`, `MOV SS`, and `POP SS` comparison spans are normalized correctly;
- external `CS:IP` mutation performs fresh node lookup at the requested address;
- DS/ES/SS/CS mutations refresh real-mode descriptor caches;
- upper EFLAGS bits survive 16-bit ABI imports;
- protected-mode mutation is rejected;
- `end` executes no further guest instruction;
- `/dosemu_mem` aliases the same pages dosemu2 uses for `lowmem_base`;
- generic mapping allocations remain independent and are never redirected to `/dosemu_mem`;
- lockstep engages only at the exact target MZ entry point;
- the DOS exec path associates that entry with the requested executable and rejects a helper MZ with identical relative entry coordinates;
- initial emu86 registers and its PSP/load-segment-dependent memory layout are aligned before request 1;
- emu86 implements CMPS (including REP/REPE/REPNE termination) before CMPS lockstep cases are enabled.
