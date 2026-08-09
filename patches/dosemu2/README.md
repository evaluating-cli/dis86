# dosemu2 validator patch series

This directory carries the small dosemu2 core patch series required by the
`dis86` validator while we do not have a writable dosemu2 fork.

The patches are ordinary `git am`-applicable mbox patches against one exact
upstream revision. They are not a vendored dosemu2 source copy and should not
be treated as an upstream PR by themselves.

## Upstream base

- Upstream: `https://github.com/dosemu2/dosemu2`
- Base branch: `devel`
- Base commit: `604ce0cdd1a71f657e2a2df623d216d5ab289313`
- Verified date: 2026-08-06
- Intended eventual dosemu2/fork PR title:
  `simx86: add executable-scoped validator stepping hook`

The base commit is intentionally pinned. Rebase/regeneration onto a newer
upstream revision should be explicit and reviewed as a separate change.

## Series

1. `0001-simx86-add-validator-control-abi.patch`
   - page-sized `/hydra_remote` POSIX shared-memory object
     (`/dev/shm/hydra_remote` on Linux)
   - ABI version and structure-size fields
   - `init`, `end`, `pid`, `runtime_psp`, `req`, `ack`
   - register request/apply and publish/ack synchronization
   - exact MZ-entry activation gate using PSP/MCB/environment program identity
   - validator-only `MSSTP` mode
   - translated-node `decoded_instructions` from `TNode.seqnum`
   - raw node-boundary `step_flags`
   - final end acknowledgement before execution stops

2. `0002-mapping-export-validator-lowmem.patch`
   - requires the POSIX `mapshm` mapping driver in validator mode
   - keeps the actual `MAPPING_LOWMEM` POSIX SHM object named `/dosemu_mem`
   - uses the same fd that backs `lowmem_base`; no second DOS-memory copy
   - creates a temporary second `MAP_SHARED` view before guest boot and proves
     writes are visible in both directions, restoring the original bytes
   - verifies before validator `init` publication that the selected driver is
     `mapshm`, the exported object is the live `lowmem_base` allocation, and
     conventional address zero resolves through that backing
   - unlinks `/dosemu_mem` when the backing mapping is freed

3. `0003-simx86-harden-validator-target-lifecycle.patch`
   - requires the exact entry-gate PSP to equal dosemu's current PSP
   - uses dosemu's version-adjusted `sda_cur_psp()` accessor rather than a
     literal SDA offset
   - treats PSPs descending from the captured target PSP as child/helper
     processes and lets those nodes run without consuming validator requests
   - checks `end` before child/helper bypass, preserving the global stop barrier
   - permanently marks the target finished once the current PSP leaves the
     target ancestry
   - clears `runtime_psp`, publishes `DIIS_STEP_TARGET_EXIT`, and release-stores
     `ack` for any pending request
   - never allows the exact target gate to reactivate after target exit, which
     prevents stale PSP reuse from resuming validation

The PSP ancestry walk is bounded and derives the parent field with
`offsetof(struct PSP, parent_psp)` from dosemu's own PSP definition rather
than embedding a numeric DOS offset.

## Validator launch contract

The validator launcher must select the deterministic simx86 C interpreter and
the shared-memory mapping driver explicitly:

```text
$_cpuemu = (1)
$_mapping = "mapshm"
```

Patch 0001 fails closed if the simx86 interpreter is not selected. Patch 0002
fails closed unless the live low-memory backing is the verified `mapshm`
export at `/dosemu_mem`.

The exact target gate expects:

```text
DIIS_DOSEMU_VALIDATOR=1
DIIS_DOSEMU_MZ_CS=<u16, decimal or 0x-prefixed>
DIIS_DOSEMU_MZ_IP=<u16, decimal or 0x-prefixed>
DIIS_DOSEMU_TARGET_DOS_PATH=<canonical DOS path, e.g. C:\\TEST.EXE>
```

Activation occurs only when all of the following are true at a simx86 boundary:

- real mode;
- `DS == ES`;
- `DS` points at a PSP beginning with `CD 20`;
- the PSP-owned MCB owner field equals the candidate PSP;
- the PSP environment block's executable path matches
  `DIIS_DOSEMU_TARGET_DOS_PATH` case-insensitively with slash normalization;
- `CS == PSP + 0x10 + MZ_CS`;
- `IP == MZ_IP`;
- dosemu's version-aware SDA current-PSP accessor reports the same PSP.

Only after that match is `runtime_psp` published and `init` release-stored.
This prevents startup helpers or stale PSP-shaped memory from activating the
hook merely because they execute under simx86.

## Lifecycle provenance

The pinned source establishes the lifecycle abstraction used by patch 0003:

1. dosemu's redirector initialization selects SDA offsets by supported DOS /
   redirector version (`REDVER_PC30`, `REDVER_PC31`, `REDVER_PC40`, and the
   Compaq 3.0 variant), including `sda_cur_psp_off`;
2. unsupported redirector versions fail initialization rather than silently
   using one fixed SDA layout;
3. `dos2linux.h` exposes `sda_cur_psp(sda_t)` and dosemu itself uses that
   accessor when tracking the current DOS program;
4. the same header defines `struct PSP`, including its `parent_psp` field;
5. patch 0003 therefore reads the current PSP through dosemu's abstraction and
   uses dosemu's PSP layout only to walk the process ancestry.

This lets a child EXEC temporarily replace the current PSP without being
mistaken for target termination, while returning to the target PSP resumes the
pending validator request. Once DOS returns to a PSP outside the target's
ancestry, the target is latched finished and cannot reactivate.

## Low-memory provenance

The pinned source establishes the chain used by patch 0002:

1. `mapping.c::do_alloc_mapping()` assigns `lowmem_base` from an allocation
   only when the allocation capability contains `MAPPING_LOWMEM`.
2. `mapshm` uses the `mapfile.c` POSIX-SHM backend.
3. `alloc_mapping_file()` `ftruncate`s one fd and maps it with `MAP_SHARED`.
4. `alias_mapping_file()` creates aliases from the fd stored for that same
   allocation.
5. In validator mode, patch 0002 names that low-memory fd `/dosemu_mem`, proves
   a second shared view has bidirectional visibility, and records that exact
   allocation as the export.
6. At the first simx86 validator boundary, `mapping.c` confirms the selected
   driver is `mapshm`, the recorded export equals `lowmem_base` with the full
   `LOWMEM_SIZE + HMASIZE` size, and address zero resolves through that backing.
7. Only after this proof can patch 0001 publish validator `init`.

This is deliberately different from allocating another shared-memory buffer
and copying low memory into it.

## Node-boundary metadata

`TNode.seqlen` is byte length and is **not** an instruction count. Patch 0001
publishes `TNode.seqnum`, which is populated from the number of decoded
`IMeta` entries used to construct the translated node.

Current `step_flags` are deliberately raw observable categories rather than
opcode guesses:

- `DIIS_STEP_MULTI_INSN`: translated node consumed more than one decoded instruction;
- `DIIS_STEP_SAME_PC`: the node returned to its starting PC;
- `DIIS_STEP_FAULT`: simx86 reported a CPU error/exception after the node;
- `DIIS_STEP_END_ACK`: final publication after observing `end` and before stopping;
- `DIIS_STEP_TARGET_EXIT`: DOS current-PSP ancestry has left the captured target.

This gives the Rust side actual node consumption without assuming that
`STI`, `MOV SS`, or `POP SS` necessarily consumed two instructions. More
specific classifications (for example interrupt-shadow versus partial REP)
should be added only where the source gives us a reliable discriminator.

## End barrier

At every active validator boundary, `end` is acquire-loaded before a translated
node is selected or before child/helper bypass is allowed. If set, the hook
publishes the current CPU state with `DIIS_STEP_END_ACK`, release-stores `ack`,
and asks the simx86 loop to leave dosemu without beginning another guest node.

A dedicated runtime proof test is still required before the series is
considered ready for an upstream/fork PR.

## Applying

From the `dis86` checkout:

```sh
bash scripts/apply-dosemu2-patches.sh /path/to/dosemu2
```

The script refuses to apply to a dosemu2 checkout whose `HEAD` is not the
pinned base commit or whose worktree/index is dirty.

## CI verification

The carrier workflow fetches exactly the pinned dosemu2 commit and applies
every entry in `patches/dosemu2/series` with `git am`, then runs
`git diff --check`.

All three current patches have passed this gate together against the exact
pinned upstream revision. This proves patch syntax/context and whitespace, but
it is **not** a compile or runtime proof.

Still required:

- compile the patched dosemu2 tree;
- prove the `end` barrier at runtime;
- exercise target -> child -> target and target -> parent lifecycle transitions;
- exercise `/dosemu_mem` bidirectional visibility from the external Rust side.

## Upstreaming later

Once a writable dosemu2 fork exists, replay this exact series with `git am`
onto the pinned base, push those commits to the fork, and make the fork PR the
authoritative implementation. At that point this local carrier can either be
regenerated from the fork commits or removed once the integration fetches the
fork directly.
