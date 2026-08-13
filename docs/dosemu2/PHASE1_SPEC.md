# Phase 1 specification: dosemu2 validator hook and lockstep transport

**Scope:** 16-bit real-mode `simx86` and `emu86_validator`  
**Pinned dosemu2:** `604ce0cdd1a71f657e2a2df623d216d5ab289313`  
**ABI:** version 1

**Implementation:** dosemu2 carrier in `patches/dosemu2/`, with prerequisites from PR #12, Rust integration from PRs #18–#23, and service-boundary patch 0010 from PR #25.

## 1. Status and interpretation

This is the normative behavior specification. The CPU hook, low-memory export, ABI, current Rust adapter path, and focused runtime smoke tests are implemented. “Implemented” does not mean every semantic edge has passed an end-to-end differential corpus.

| Evidence level | Current claim |
| --- | --- |
| Specified | All requirements below. |
| Implemented | Ten-patch dosemu2 carrier plus current dis86 adapter/outcome/shutdown path. |
| Unit tested | Host-independent ABI/state/comparison/reference-CPU paths. |
| Pinned-runtime tested | Build/link, ABI init, basic step, live low-memory alias, end barrier/clean exit, nonterminating `INT 21h/AH=30h` post-service acknowledgement, target-exit/fault publication, terminating MZ smoke fixture. |
| Remaining implementation | No known gap in the tested standalone `INT 21h` host-service acknowledgement path; expanded corpus may still expose REP/shadow/helper/lifecycle or interrupt-classification fixes. |
| Still-unverified E2E | Prefixed service calls, application-installed interrupt handlers outside the target MCB, REP, interrupt shadow, broader handler/helper exclusion, lifecycle transitions, full differential corpus. |

## 2. Execution-boundary contract

When validator mode is enabled and the executable entry gate is reached, dosemu2 shall:

1. publish ABI version, structure size, PID, runtime PSP, and initial register state before release-storing `init`;
2. acquire-load a new request before reading the non-atomic register payload;
3. reject protected-mode state import before mutation;
4. apply requested real-mode register/segment state before node lookup and recompute linear PC from `CS:IP`;
5. reassert `MSSTP` after normal mode-bit reset;
6. select/execute the translated node, publish state and node metadata, then release-store acknowledgement; and
7. acquire-check `end` before selecting another target node or bypassing a helper, publish `END_ACK`, and execute no subsequent controlled node.

The hook is around `FindExecCode()` node selection/execution. A changed `CS:IP` therefore selects from the recomputed PC instead of returning through a path that could overwrite the requested IP with a stale local PC.

`TNode.seqnum`, not byte-length `seqlen`, supplies `decoded_instructions`.

## 3. ABI-v1 contract

The shared structure is 88 bytes and preserves the established 64-byte Hydra controller/register prefix. The dosemu2 extension is append-only and begins at offset 64. The POSIX-SHM backing remains page-sized.

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

Current raw flags:

```text
DIIS_STEP_MULTI_INSN   = 1 << 0
DIIS_STEP_SAME_PC      = 1 << 1
DIIS_STEP_FAULT        = 1 << 2
DIIS_STEP_END_ACK      = 1 << 3
DIIS_STEP_TARGET_EXIT  = 1 << 4
```

Control fields use acquire/release ordering; non-atomic payload writes are ordered by those control operations.

`ShmData::attach()` rejects an undersized backing before mapping it. After `init`, the Rust adapter validates ABI version, structure size, and published process ownership before trusting the payload. The published PID may be the launcher returned by `Command::spawn()` or an emulator descendant whose parent ancestry leads back to that launcher; it need not equal `Child::id()`.

General-register and FLAGS imports preserve their high 16 bits. Real-mode segment changes use dosemu2's segment-cache update path. Protected-mode import fails closed with `DIIS_STEP_FAULT` before mutation.

Expected validator single-step/internal return reasons are not architectural faults.

## 4. Launch contract

The adapter shall force the path containing the hook and the shared-memory mapping driver:

```text
-I "cpu_vm emulated"
-I "cpuemu 1"
-I "cpu_vm_dpmi emulated"
-I "mappingdriver mapshm"
```

with:

```text
DIIS_DOSEMU_VALIDATOR=1
DIIS_DOSEMU_MZ_CS=<u16>
DIIS_DOSEMU_MZ_IP=<u16>
DIIS_DOSEMU_TARGET_DOS_PATH=?:\<exe>
```

The `?` is permitted only in the configured path's drive-letter position because `-K` may receive a different redirected DOS drive depending on boot-stack state.

DOSBox-X `-hydra` / `-hydra-conf` arguments are not part of this contract.

## 5. Low-memory contract

Validator mode shall require `mapshm` and expose as `/dosemu_mem` the same POSIX shared-memory backing that contains `lowmem_base`; it shall not allocate a second guest-memory buffer and copy into it.

Before publishing `init`, dosemu2 shall verify driver/backing provenance, that conventional address zero resolves through that backing, and bidirectional alias visibility. The backing may be larger than the visible `LOWMEM_SIZE + HMASIZE` window; exact-size equality is not required.

The focused pinned-runtime probe proves external bidirectional alias visibility. It does not prove every logical DOS mapping or instruction memory effect matches emu86.

## 6. Activation, exclusion, and lifecycle contract

Activation requires real mode, a valid PSP/MCB/environment identity, matching configured executable path and MZ entry `CS:IP`, and agreement with dosemu2's version-aware current-PSP accessor.

Once active:

- target requests are consumed only for target-owned code, except while executing an application-installed interrupt handler that emu86 exposes as part of the controlled instruction stream;
- when a target-owned host-normalized DOS/BIOS service interrupt transfers control outside the target-owned MCB, dosemu2 shall keep that request pending and shall not publish or acknowledge the handler-entry state;
- while a host-service request remains pending, intervening DOS/BIOS handler nodes and callbacks shall execute without consuming another request; publication and acknowledgement occur only at the exact interrupt return `CS:IP` recorded from the handler-entry stack;
- software-interrupt recognition skips the segment-override prefixes accepted by emu86 (`26h`, `2Eh`, `36h`, `3Eh`) before decoding `INT3`, `INT imm8`, or `INTO`;
- unchanged activation-time vectors `10h`, `1Ah`, `21h`, and `33h` are eligible for host-service normalization; if the target changes a vector, the interrupt retains emu86's application-handler boundary instead of being normalized as a host service;
- an application-installed handler is published at handler entry and remains controller-stepped even when its code resides outside the target MCB, until its recorded interrupt return boundary is reached;
- descendant child/helper processes bypass target request consumption;
- the global end barrier remains effective before all bypass paths;
- leaving target ancestry publishes `TARGET_EXIT`, clears `runtime_psp`, acknowledges any pending request, and permanently prevents stale-PSP reactivation.

Patch 0010 implements this classification and acknowledgement behavior for standalone controlled software-interrupt nodes. At activation it snapshots the IVT, then uses the current vector value plus the decoded interrupt number to distinguish host-normalized services from application-installed handlers. Host-service requests remain outstanding until the exact saved return `CS:IP`; intermediate target callbacks are not mistaken for service completion. Application-installed handlers instead expose their handler-entry boundary and remain in lockstep through their interrupt return. Architectural faults remain immediate, and target exit or the global end barrier terminates pending work through the existing lifecycle paths.

The pinned runtime probe exercises a nonterminating unprefixed `INT 21h/AH=30h` service and requires the acknowledgement snapshot to be back at the target instruction after the interrupt with the DOS-returned register state. The subsequent target instruction consumes that returned state. The terminating `INT 21h/AH=4Ch` pre-execution path remains a separate terminal special case.

The current pinned runtime proof does not separately exercise a prefixed service encoding or an application-installed interrupt handler outside the target MCB. It also does not cover a software interrupt composed as the following instruction of an interrupt-shadow multi-instruction node, REP/shadow composition, a broader DOS/BIOS handler set, descendant helpers, or target lifecycle transitions. Those remain part of the expanded corpus rather than being inferred from the standalone service proof.

## 7. Normalized comparison contract

A raw step is one dosemu2 request/ack result. The result publishes `decoded_instructions` and `step_flags`. The Rust validator advances the emu86 candidate from reported node consumption rather than assuming one decoded instruction per node.

Two composition rules remain critical:

- **REP:** same-PC raw micro-iterations must reach the same final semantic comparison boundary as emu86 without double-consuming an iteration.
- **Interrupt shadow:** STI/MOV SS/POP SS may compose with the following decoded instruction in one node; if that following instruction is REP, an already-consumed first REP iteration must not be executed or compared twice.

The metadata/outcome plumbing is implemented. These semantics are not integration-tested until the expanded corpus passes.

## 8. Shutdown contract

`end` is a zero-more-controlled-nodes barrier. When observed before a controlled node begins, dosemu2 publishes final state with `DIIS_STEP_END_ACK`, acknowledges the current request value, and starts no subsequent controlled node.

The parent performs cooperative bounded shutdown and explicitly reaps the child; kill is fallback only. Focused pinned-runtime coverage exists for this path.

## 9. Required expanded corpus

The remaining gate shall compare architectural state and relevant memory after each normalized boundary and include:

- arithmetic/logic and flag-producing instructions;
- stack, near/far call/return, branches, and interrupts;
- prefixed DOS/BIOS service calls and application-installed interrupt handlers, including handlers outside the target MCB;
- REP MOVS/STOS/CMPS/SCAS, including termination conditions;
- STI/MOV SS/POP SS multi-instruction nodes and shadow + REP composition;
- external `CS:IP`, GPR, segment, and memory mutation;
- DOS/BIOS handler and descendant child/helper execution;
- target -> child -> target and target -> parent lifecycle transitions; and
- normal termination, fault, target-exit, and controller end-barrier paths.

Until this corpus passes, prefixed/application-handler interrupt cases, REP, shadow composition, representative helper exclusion, lifecycle transitions, and full differential comparison are **specified/implemented where noted, but not integration-tested**.
