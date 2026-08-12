# dosemu2 validator technical review

## Evidence model

The original migration review correctly rejected a pure-plugin assumption, unsupported validator speedup claims, an unrelated second low-memory mapping, and unproven REP semantics. Those questions now have partial implementation evidence and should no longer be described uniformly as hypotheses.

PR #12 supplied configurable validator executable loading and `CMPS`; PR #17 established the dosemu2 carrier; PRs #18–#20 added Rust-side node-outcome handling, cooperative shutdown, and a terminating runtime fixture. The current dosemu2 implementation is carried in `patches/dosemu2/` against pinned upstream commit `604ce0cdd1a71f657e2a2df623d216d5ab289313`, using ABI version **1**.

| Area | Specified behavior | Implementation / evidence | Remaining E2E proof |
| --- | --- | --- | --- |
| CPU boundary | Consume a request, execute a validator-bounded translated node, publish state/metadata, acknowledge. | Implemented around `FindExecCode()` with `MSSTP`; pinned runtime performs a basic step. | Expanded instruction-by-instruction differential corpus. |
| Register ABI | Exchange 14 x86-16 registers with acquire/release ordering and reject incompatible layouts. | ABI v1 implemented; protected-mode import fails closed. | Broad mutation coverage across the expanded corpus. |
| Low memory | Share the live simulator low-memory backing, not a copied second buffer. | `mapshm`/`lowmem_base` backing exported as `/dosemu_mem`; external bidirectional alias proof passes. | Differential memory effects across the expanded corpus. |
| REP | Compare at the same semantic boundary as emu86 without double-consuming an iteration. | `decoded_instructions`, `SAME_PC`, CMPS support, and Rust outcome plumbing exist. | **Not integration-tested** with REP MOVS/STOS/CMPS/SCAS fixtures. |
| Shadow instructions | Account for nodes that legitimately consume multiple decoded instructions. | `TNode.seqnum`, `MULTI_INSN`, and Rust multi-instruction handling exist. | **Not integration-tested** with STI, MOV SS, and POP SS fixtures. |
| DOS/BIOS exclusion | Do not consume target requests while executing outside the target's owned MCB. | Patch 0006 implements a target-owned PC range check. | **Not integration-tested** against representative DOS/BIOS handler execution. |
| Child/helper exclusion | Descendants may run without consuming target requests while global end remains effective. | PSP ancestry policy implemented. | **Not integration-tested** with actual child/helper execution. |
| Lifecycle | Publish target exit once and prevent stale PSP reactivation. | Implemented; end barrier and terminating smoke path are runtime-tested. | Target -> child -> target and target -> parent transitions remain unverified. |
| Target identity | Bind activation to executable path, MZ entry, PSP/MCB/environment/current PSP. | Implemented; patch 0008 allows an explicit wildcard only in the drive-letter position for `-K` drive variability. | More boot-stack/path coverage. |
| Differential comparison | Advance emu86 from reported node outcomes and compare normalized state. | PR #18 implements `StepOutcome` handling and initial-state comparison; PR #20 runs one terminating fixture. | **Full differential corpus remains unverified E2E.** |
| Shutdown | End barrier, bounded cooperative exit, termination fallback, reap. | dosemu2 end barrier runtime proof plus PR #19 cooperative shutdown/reap path. | Stress/error-path runtime coverage. |

## Review conclusions

### Core hook and low-memory export are concrete

The `FindExecCode()` hook, `MSSTP` reassertion, pre-node state application, authoritative PC recomputation, post-node publication, and live low-memory export are implemented. They should no longer be described as proposed or merely likely.

This also confirms the original review's central architectural correction: the validator needs a small `simx86` core instrumentation surface. The plugin architecture alone is not the mechanism for arbitrary existing `CS:IP` instruction-boundary control.

### The low-memory design follows the existing backing

The implementation exports the existing `MAPPING_LOWMEM` backing rather than allocating and copying a second DOS memory buffer. The real backing may be larger than the visible `LOWMEM_SIZE + HMASIZE` window; provenance/containment and address-zero aliasing are the relevant invariants, not exact-size equality.

### Workloads remain separate

Strict validator lockstep pays synchronization cost after each observable node. Its correctness smoke tests establish no 10–50x speedup. Normal Hydra hybrid execution may have different performance characteristics and must be benchmarked separately.

### Real-mode scope remains deliberate

ABI-v1 state import is a 16-bit real-mode contract. Protected mode is rejected before external register or segment mutation rather than applying `SetSegReal()` under unsupported conditions.

### REP and shadow composition remain the semantic risk

`MSSTP` does not imply every observable node corresponds to exactly one emu86 semantic step. Interrupt-shadow handling may produce multi-instruction nodes; REP may remain at the same PC across raw micro-iterations. The adapter therefore relies on published metadata rather than byte length or opcode guesses.

The plumbing is implemented, but the combined REP/shadow behavior is not proven until focused pinned-runtime fixtures pass.

### Handler/child exclusion is implemented but not fully proven

The current patch series contains explicit DOS-handler PC exclusion and PSP-ancestry child/helper handling. That resolves the earlier design ambiguity at source level. It does **not** justify claiming representative DOS/BIOS/child execution is integration-tested yet.

## Approval gate

The patch/build/runtime smoke checks support continued development, but not a claim of complete validator integration. Approval of REP semantics, interrupt-shadow composition, helper exclusion, lifecycle transitions, or full differential comparison as integration-tested requires the expanded pinned-runtime corpus to pass.

Hydra native-function interception, overlays, and performance remain follow-on work outside the current validator proof.
