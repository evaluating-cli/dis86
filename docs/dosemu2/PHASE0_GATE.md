# Phase 0 Gate: Minimal Simulator Control Specification

**Status:** Proposed Architectural Gate  
**Target:** `dosemu2` `simx86` simulator integration  

---

## 1. Objective

Before writing full shared-memory protocols or building the complete Hydra hybrid ABI, `dosemu2`'s CPU simulator (`simx86`) must demonstrate that it can satisfy the fundamental instruction-control and state-manipulation requirements demanded by Hydra and `emu86_validator`.

---

## 2. Gate Criteria & Test Vectors

The Phase 0 milestone requires a minimal standalone test harness to pass the following 6 gates:

### Gate 1: Precise Single-Instruction Stepping
* **Requirement:** The simulator must execute exactly **one** guest architectural instruction and halt at the instruction boundary.
* **Verification:** Advance through a test sequence (`MOV AX, 0x1234`, `ADD AX, 0x0001`, `PUSH AX`) one step at a time. After step 1, `AX` must equal `0x1234` and `IP` must point exactly to the `ADD` opcode.

### Gate 2: Full Architectural State Extraction
* **Requirement:** At each instruction boundary, complete architectural state must be readable without side effects:
  * General registers: `AX`, `BX`, `CX`, `DX`
  * Index / Pointer registers: `SI`, `DI`, `BP`, `SP`, `IP`
  * Segment registers: `CS`, `DS`, `ES`, `SS`
  * Flags: `FLAGS` (`CF`, `PF`, `AF`, `ZF`, `SF`, `TF`, `IF`, `DF`, `OF`)
* **Verification:** Read state from `vm86s.regs` and compare against reference values for known operations.

### Gate 3: External State & `CS:IP` Mutation
* **Requirement:** An external caller must be able to mutate general registers, flags, and `CS:IP`.
* **Verification:** Inject arbitrary register changes (`AX = 0x55AA`) and advance `IP` past an instruction; verify the next instruction operates on the modified register value.

### Gate 4: Clean Block Exit & Stale Translation Invalidation
* **Requirement:** When `CS:IP` or segment registers are redirected externally (simulating a Hydra `RETURN_FAR`, `CALL_FAR`, or `RETURN_JUMP`), `simx86` must cleanly exit the current translated block and invalidate any cached translation state.
* **Verification:** Redirect `CS:IP` to a target address in a different code segment. Verify that the simulator does not execute residual instructions from the previous translated block and recomputes segment bases correctly.

### Gate 5: Control Flow & Calling Convention Determinism
* **Requirement:** Control flow instructions must behave identically to reference 8086 semantics:
  * `CALL NEAR` / `CALL FAR`: Correctly pushes return address (`IP` / `CS:IP`) onto `SS:SP`.
  * `RET` / `RETF`: Correctly pops return address and adjusts `SP`.
  * `INT <n>` / `IRET`: Correctly pushes/pops flags and segment registers.
* **Verification:** Compare post-instruction stack frames (`SS:SP`) and register state against `dis86/emu86`.

### Gate 6: REP String Operation Stepping Contract
* **Requirement:** Establish and verify the stepping contract for `REP`-prefixed instructions (`REP MOVSB/W`, `REP STOSB/W`, `REP SCASB/W`, `REP CMPSB/W`).
* **Verification:** Determine whether the stepping contract treats `REP` as a single atomic step (`CX -> 0`, `SI`/`DI` advanced) or exposes individual iterations. Verify that stepping does not desynchronize the validator.

---

## 3. Known-Good Path Validation Protocol

To prove that state manipulation does not violate x86 architectural semantics:

```
                      ┌────────────────────────────────┐
                      │    Input Machine Code Stream   │
                      └───────┬────────────────┬───────┘
                              │                │
                              ▼                ▼
                     ┌─────────────────┐ ┌───────────────┐
                     │ dosemu2 simx86  │ │  dis86 emu86  │
                     │  (Phase 0 Test) │ │  (Reference)  │
                     └────────┬────────┘ └───────┬───────┘
                              │ Step 1           │ Step 1
                              ▼                  ▼
                     ┌─────────────────┐ ┌───────────────┐
                     │ Snapshot State  │ │ State Check   │
                     └────────┬────────┘ └───────┬───────┘
                              │                  │
                              ▼                  ▼
                     ┌───────────────────────────────────┐
                     │    Assert(simx86 == Reference)    │
                     └───────────────────────────────────┘
                              │
                              ▼
                     ┌───────────────────────────────────┐
                     │ External Mutation (e.g. CS:IP/AX) │
                     └────────┬──────────────────────────┘
                              │
                              ▼
                     ┌───────────────────────────────────┐
                     │  Verify Clean Resume & Next Step  │
                     └───────────────────────────────────┘
```

Every state mutation in the test harness must be verified against a reference execution in `dis86/emu86` to ensure complete semantic parity before proceeding to Phase 1.
