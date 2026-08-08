# Phase 1 Specification: `simx86` Core Hook Integration & Lockstep Synchronization

**Target Milestone:** Phase 1 — `simx86` Core Hook Integration  
**Target Projects:** `dosemu2` (`src/base/emu-i386/simx86/`), `dis86` (`dis86/src/emu86/validator/`)  
**Status:** Implementation Blueprint & Architecture Specification  

---

## 1. Executive Summary & Objective

The objective of **Phase 1** is to establish deterministic instruction-level stepping and bidirectional shared-memory synchronization between `dosemu2`'s `simx86` CPU simulator and `dis86`'s `emu86_validator` differential testing engine.

This integration replaces the intrusive `core_normal_286.cpp` patches from DosBox-X with a small, explicit `simx86` instrumentation surface that:
1. Forces one decoded guest instruction per generated node by reasserting `MSSTP` inside `FindExecCode()` after its normal mode reset.
2. Extracts complete 14-register architectural state (`AX..FLAGS`, `CS:IP`, segments) at instruction boundaries.
3. Synchronizes with the existing Phase 0 `DosemuProcess` ABI over `/dev/shm/hydra_remote` using acquire/release atomic ordering.
4. Applies external register mutations before execution, recomputes real-mode segment caches with `SetSegReal()`, and restarts dispatch from a recomputed linear `PC` when `CS:IP` changes.
5. Validates lockstep execution against `emu86` on a dedicated x86-16 test corpus.

Phase 1 is deliberately scoped to **16-bit real-mode execution**. Protected-mode segment mutation is out of scope for this milestone and must not be implemented by calling `SetSegReal()` while `PROTMODE()` is active.

---

## 2. `simx86` Execution Loop & Hook Injection Point

### 2.1 Control Flow in `interp.c`
`Interp86()` computes the initial linear `PC` and delegates execution to `FindExecCode()` in `src/base/emu-i386/simx86/interp.c`. The persistent dispatch loop is in `FindExecCode()`, not `Interp86()` itself.

The Phase 1 hook therefore belongs around the existing node lookup / execution sequence in `FindExecCode()`:

```
                       ┌─────────────────────────┐
                       │      Interp86(void)     │
                       │ PC = LONG_CS + EIP      │
                       └───────────┬─────────────┘
                                   │
                                   ▼
                       ┌─────────────────────────┐
                       │    FindExecCode(PC)     │◄────────────────────┐
                       │       while (1)         │                     │
                       └───────────┬─────────────┘                     │
                                   │                                   │
                      Hydra wait for req / end                         │
                      apply register mutations                         │
                      recompute PC if CS:IP changed                    │
                                   │                                   │
                                   ▼                                   │
                      reset normal mode bits                           │
                      reassert MSSTP if Hydra active                   │
                                   │                                   │
                                   ▼                                   │
                       ┌─────────────────────────┐                     │
                       │ FindTree(PC) or         │                     │
                       │ _Interp86(..., MSSTP)   │                     │
                       │ -> one guest instruction│                     │
                       └───────────┬─────────────┘                     │
                                   │                                   │
                                   ▼                                   │
                       ┌─────────────────────────┐                     │
                       │       PC = DoExec(G)    │                     │
                       └───────────┬─────────────┘                     │
                                   │                                   │
                      publish post-step state                           │
                      ack = req (release)                              │
                                   │                                   │
                                   └───────────────────────────────────┘
```

This avoids executing an already-selected stale node after an external `CS:IP` rewrite: mutations are consumed before node lookup, and a changed code address is converted back to the local linear `PC` used by the dispatcher.

### 2.2 Enforcing Single-Instruction Mode (`MSSTP`)
`MSSTP` is defined in `codegen.h`:

```c
#define MSSTP 0x02000000 /* generate only one instruction */
```

`FindExecCode()` currently clears `MSSTP|MTRAP` at the top of every dispatch iteration and re-enables them only when guest `TF` is set. Therefore setting `TheCPU.mode |= MSSTP` once before entering `FindExecCode()` is insufficient.

Hydra mode must reassert `MSSTP` after the existing reset/`TF` handling:

```c
TheCPU.mode &= ~(MSSTP | MTRAP);
if (EFLAGS & TF)
    TheCPU.mode |= MSSTP | MTRAP;

#if USE_HYDRA
if (hydra_active)
    TheCPU.mode |= MSSTP;
#endif
```

With `MSSTP`, `_Interp86()` breaks after one `InterpOne()` call, so each generated node represents one decoded guest instruction. **`TNode::seqlen` is the number of guest code bytes spanned by the node, not an instruction count, and may be greater than 1.**

Because `GoodNode()` compares the cached node's `mode` with `TheCPU.mode`, nodes generated without `MSSTP` are not reused for Hydra single-step execution.

### 2.3 Request / Execute / Acknowledge Placement
The hook is intentionally split into pre-execution and post-execution phases:

```c
/* Sketch inside FindExecCode(); exact helper names are illustrative. */
while (1) {
#if USE_HYDRA
    uint64_t hydra_req = 0;
    if (hydra_active) {
        if (!dosemu_hydra_wait_request(&hydra_req))
            return PC; /* end/shutdown path */

        /* Applies register mutations and returns LONG_CS + TheCPU.eip. */
        PC = dosemu_hydra_apply_state(PC);
    }
#endif

    TheCPU.mode &= ~(MSSTP | MTRAP);
    if (EFLAGS & TF)
        TheCPU.mode |= MSSTP | MTRAP;
#if USE_HYDRA
    if (hydra_active)
        TheCPU.mode |= MSSTP;
#endif

    /* Existing FindTree() / _Interp86() logic selects one-instruction node. */

    PC = DoExec(G);

#if USE_HYDRA
    if (hydra_active) {
        /*
         * PC is the authoritative post-node linear address here.
         * TheCPU.eip is normally synchronized by Interp86() only when
         * FindExecCode() returns, so publish IP as PC - LONG_CS.
         */
        dosemu_hydra_publish_and_ack(hydra_req, PC);
    }
#endif

    if (TheCPU.err)
        return PC;
}
```

The first implementation should not use `EXCP_EMULEAVE` merely to handle a pre-execution `CS:IP` mutation. `Interp86()` writes `TheCPU.eip = PC - LONG_CS` when `FindExecCode()` returns; returning an old local `PC` after changing `CS` can therefore overwrite the externally requested `IP`. Restarting dispatch with `PC = LONG_CS + TheCPU.eip` avoids that ambiguity.

---

## 3. Register Synchronization & Segment Descriptor Management

### 3.1 State Serialization (`TheCPU` -> `shmdata`)
The Phase 0 `shmdata` ABI already contains the 14 x86-16 register fields required by the validator. At a post-execution boundary, use the `PC` returned by `DoExec()` for `IP`, because `TheCPU.eip` is synchronized by `Interp86()` only after `FindExecCode()` returns:

```c
void dosemu_update_shmdata_from_cpu(shmdata_t *shm, unsigned int pc) {
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

For the initial published state, pass `LONG_CS + TheCPU.eip` as `pc`.

### 3.2 State Deserialization (`shmdata` -> `TheCPU`)
When the validator mutates registers, preserve the high halves of the 32-bit general registers and EFLAGS, update real-mode segment caches through `SetSegReal()`, then return the linear address that must be used for the next node lookup:

```c
unsigned int dosemu_update_cpu_from_shmdata(const shmdata_t *shm) {
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

    /* Phase 1 is real-mode only. */
    assert(!PROTMODE());

    if (TheCPU.cs != shm->cs) SetSegReal(shm->cs, Ofs_CS);
    if (TheCPU.ds != shm->ds) SetSegReal(shm->ds, Ofs_DS);
    if (TheCPU.es != shm->es) SetSegReal(shm->es, Ofs_ES);
    if (TheCPU.ss != shm->ss) SetSegReal(shm->ss, Ofs_SS);

    return LONG_CS + TheCPU.eip;
}
```

### 3.3 Segment Recomputation Guarantee
In current `simx86`, `SetSegReal(sel, ofs)`:
1. Writes the segment selector with `CPUWORD(ofs) = sel`.
2. Recomputes the cached real-mode bounds as `sel << 4` through `+ 0xffff`.
3. Makes `LONG_CS + TheCPU.eip` valid for the next real-mode decode.

This is sufficient for Phase 1's real-mode contract. Protected-mode selector updates require the protected-mode validation/cache path and are explicitly deferred.

---

## 4. Shared-Memory IPC Handshake Protocol

The Phase 0 Rust side already uses 64-bit `req`/`ack` counters and acquire/release ordering. Phase 1 implements the matching dosemu2 side. Register payload fields remain non-atomic and are ordered by the control atomics:

```
   dosemu2 (simx86)                            dis86 (DosemuProcess)
  ─────────────────                            ──────────────────────
  remote_init():
    shm->init = 0
    shm->pid = getpid()
    shm->req = 0
    shm->ack = 0

  [Reaches Entry CS:IP]:
    publish registers using current PC
    atomic_store_release(shm->init, 1) ───────► wait_for_init():
                                                  load_acquire(init) == 1
                                                  verify pid == child.id()

  ┌► wait_for_request():                       register mutation:
  │    if load_acquire(end) -> shutdown          reg_write()/flag_write()
  │    req = load_acquire(shm->req)              update payload fields
  │    if req == ack: keep waiting
  │                                              step():
  │    acquire(req) orders payload reads ◄────── store_release(req, ack + 1)
  │    apply payload to TheCPU
  │    recompute PC = LONG_CS + EIP
  │
  │  [Execute exactly 1 guest instruction]
  │    PC = DoExec(G)
  │
  │    publish registers using returned PC
  │    store_release(shm->ack, req) ───────────► spin_wait():
  │                                              load_acquire(ack) == req
  │                                              read_cpu_state()
  └──────────────────────────────────────────────compare_with_emu86()
```

The release store to `req` publishes any register mutations written by `DosemuProcess` before the step. The acquire load of `req` on the dosemu2 side must occur before reading payload fields. In the opposite direction, dosemu2 writes the register snapshot before its release store to `ack`, and `DosemuProcess` reads the snapshot only after an acquire load observes that acknowledgement.

---

## 5. Test Corpus & Known-Good Validation Plan

To satisfy the Phase 0 and Phase 1 gate requirements, a dedicated x86-16 test corpus exercises:

| Test Group | Instructions Tested | Validation Target |
| :--- | :--- | :--- |
| **Arithmetic & Logic** | `MOV`, `ADD`, `ADC`, `SUB`, `SBB`, `AND`, `OR`, `XOR`, `SHL`, `SHR`, `IMUL` | Register values and `FLAGS` parity across every step. |
| **Stack Operations** | `PUSH reg/imm`, `POP reg`, `PUSHA`, `POPA`, `PUSHF`, `POPF` | Stack pointer alignment (`SP -= 2/4`) and memory integrity. |
| **Control Flow** | `JMP short/near`, `JZ`, `JNZ`, `CALL NEAR`, `RET`, `CALL FAR`, `RETF` | Target `CS:IP` landing and return address stack frame layout. |
| **String Operations** | `REP MOVSB`, `REP MOVSW`, `REP STOSB`, `REP STOSW` | Single-step atomicity contract vs iteration visibility. |
| **External Redirection** | `CS:IP` rewrite + segment change | Recompute real-mode segment cache and restart dispatch without executing the stale node. |

---

## 6. Implementation Deliverables

1. **`dosemu2` core hook patch**:
   * `src/base/emu-i386/simx86/interp.c`: Request/execute/ack hook integration and `MSSTP` reassertion.
   * A small Hydra remote helper (location to be chosen during implementation) for shared-memory lifecycle and register serialization.
   * `cpu-emu.c` changes only if required by the final entry/exit integration; do not add them solely for register copying.
2. **`dis86` validator test runner**:
   * Reuse the Phase 0 `dis86/src/emu86/validator/dosemu_process.rs` adapter and complete any behavior exposed by end-to-end testing.
   * CLI test harness: `emu86_validator --backend dosemu2 --exe <test_binary>`.
3. **CI automated check**:
   * Integration test in GitHub Actions running headless `dosemu2` against reference binaries.
