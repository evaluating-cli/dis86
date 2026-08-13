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

## Frozen feature series

The active carrier is regenerated from the final tree as exactly two feature
commits. Patch boundaries follow subsystem ownership rather than the historical
development sequence:

1. `0001-simx86-add-executable-scoped-validator-control.patch`
   (`simx86: add executable-scoped validator control`)
   - owns the simx86 build integration, interpreter hook, ABI, activation gate,
     state exchange, and executable-scoped stepping implementation;
   - preserves ABI version 1 and the 88-byte shared control layout;
   - includes all lifecycle hardening, atomic flag publication, protected-mode
     rejection, DOS-handler exclusion, termination publication, dynamic drive
     identity, architectural-fault classification, and interrupt-service
     acknowledgement deferral from the former patches 0001 and 0003-0010.

2. `0002-mapping-expose-live-low-memory-backing.patch`
   (`mapping: expose live low-memory backing for external validation`)
   - adds a generic mapping-backing query describing backing kind, fd, POSIX
     name, size, and base-address provenance;
   - uses the generic named-backing path to retain the live `mapshm`
     low-memory allocation at `/dosemu_mem`, without DIIS-specific policy in
     `mapfile.c`;
   - includes the validator-side provenance call so `init` is not published
     until the selected mapping is proven to be that live backing.

These patches are frozen against the pinned base and preserve the same observable
validator behavior as the former ten-patch carrier. The original ten commits remain
available in this repository's Git history; they are intentionally no longer
kept as active patch files or listed in `series`.

## Validator launch contract

The validator launcher must force CPU emulation, select the deterministic
simx86 C interpreter, and select the shared-memory mapping driver explicitly:

```text
$_cpu_vm = "emulated"
$_cpuemu = (1)
$_mapping = "mapshm"
```

`$_cpu_vm` defaults to `"auto"`; setting only `$_cpuemu = (1)` chooses the
interpreter **if CPU emulation is used**, but does not itself prevent KVM or
vm86 from being selected. The validator hook lives in simx86, so
`$_cpu_vm = "emulated"` is part of the mandatory launch contract.

The simx86 feature patch fails closed if it is reached without the simx86 interpreter selected.
The mapping feature patch fails closed unless the live low-memory backing is the verified
`mapshm` export at `/dosemu_mem`.

The exact target gate expects:

```text
DIIS_DOSEMU_VALIDATOR=1
DIIS_DOSEMU_MZ_CS=<u16, decimal or 0x-prefixed>
DIIS_DOSEMU_MZ_IP=<u16, decimal or 0x-prefixed>
DIIS_DOSEMU_TARGET_DOS_PATH=<canonical DOS path, e.g. C:\\TEST.EXE or ?:\\TEST.EXE>
```

`?` is accepted only as the first-character drive-letter wildcard. This is
useful with dosemu2's `-K` mount, whose drive is selected from the redirects
available in the active DOS boot stack.

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

The pinned source establishes the lifecycle abstraction used by the simx86 feature patch:

1. dosemu's redirector initialization selects SDA offsets by supported DOS /
   redirector version (`REDVER_PC30`, `REDVER_PC31`, `REDVER_PC40`, and the
   Compaq 3.0 variant), including `sda_cur_psp_off`;
2. unsupported redirector versions fail initialization rather than silently
   using one fixed SDA layout;
3. `dos2linux.h` exposes `sda_cur_psp(sda_t)` and dosemu itself uses that
   accessor when tracking the current DOS program;
4. the same header defines `struct PSP`, including its `parent_psp` field;
5. the simx86 feature patch therefore reads the current PSP through dosemu's abstraction and
   uses dosemu's PSP layout only to walk the process ancestry.

This lets a child EXEC temporarily replace the current PSP without being
mistaken for target termination, while returning to the target PSP resumes the
pending validator request. Once DOS returns to a PSP outside the target's
ancestry, the target is latched finished and cannot reactivate.

## Low-memory provenance

The pinned source establishes the chain used by the mapping feature patch:

1. `mapping.c::do_alloc_mapping()` assigns `lowmem_base` from an allocation
   only when the allocation capability contains `MAPPING_LOWMEM`.
2. `mapshm` uses the `mapfile.c` POSIX-SHM backend.
3. `alloc_mapping_file()` `ftruncate`s one fd and maps it with `MAP_SHARED`.
4. `alias_mapping_file()` creates aliases from the fd stored for that same
   allocation.
5. The feature patch adds a generic query for that allocation's backing kind,
   fd, optional POSIX name, mapped size, and base address.
6. In validator mode, `mapping.c` requests `/dosemu_mem` through the generic
   named-backing path; `mapfile.c` contains no DIIS-specific name or policy.
7. At the first simx86 validator boundary, the generic query proves that the
   export is POSIX SHM, is named `/dosemu_mem`, equals `lowmem_base`, covers
   `LOWMEM_SIZE + HMASIZE`, and resolves conventional address zero.
8. Only after this proof can the simx86 feature publish validator `init`.

This is deliberately different from allocating another shared-memory buffer
and copying low memory into it.

## Node-boundary metadata

`TNode.seqlen` is byte length and is **not** an instruction count. The simx86 feature patch
publishes `TNode.seqnum`, which is populated from the number of decoded
`IMeta` entries used to construct the translated node.

Current `step_flags` are deliberately raw observable categories rather than
opcode guesses:

- `DIIS_STEP_MULTI_INSN`: translated node consumed more than one decoded instruction;
- `DIIS_STEP_SAME_PC`: the node returned to its starting PC;
- `DIIS_STEP_FAULT`: simx86 reported a CPU error/exception after the node;
- `DIIS_STEP_END_ACK`: final publication after observing `end` and before stopping;
- `DIIS_STEP_TARGET_EXIT`: DOS current-PSP ancestry has left the captured target.

The interpreter's `EXCP01_SSTP` result is the expected boundary produced by
the hook's `MSSTP` request and is therefore not classified as
`DIIS_STEP_FAULT`. The `EXCP_GOBACK` and higher values are simx86-internal
return reasons rather than architectural CPU exceptions and are not faults.

This gives the Rust side actual node consumption without assuming that
`STI`, `MOV SS`, or `POP SS` necessarily consumed two instructions. More
specific classifications (for example interrupt-shadow versus partial REP)
should be added only where the source gives us a reliable discriminator.

## End barrier

At every active validator boundary, `end` is acquire-loaded before a translated
node is selected or before child/helper bypass is allowed. If set, the hook
publishes the current CPU state with `DIIS_STEP_END_ACK`, release-stores `ack`,
and asks the simx86 loop to leave dosemu without beginning another guest node.

The pinned runtime job proves this barrier and cooperative clean shutdown.

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

Both frozen feature patches pass that gate together against the exact pinned
upstream revision, and CI asserts their count and commit subjects.

The workflow then configures an interpreter-only build, generates the standard
`version.hh` and `plugin_config.hh` prerequisites, and directly builds the two
libraries touched by this series:

- `src/base/lib/mapping`
- `src/base/emu-i386/simx86`

Both patched libraries compile successfully. The workflow also performs a
minimal whole-runtime link with the `charsets`, `msdos`, and `term` plugins,
which supply the core charset, DOS, and headless-terminal hooks without pulling
in SDL/audio/network plugins.

Still required for expanded integration coverage:

- prefixed host services, application-installed handlers, and broader BIOS services;
- REP and interrupt-shadow composition;
- target -> child -> target and target -> parent lifecycle transitions;
- representative helper execution and broad state/memory differential comparison.

The external `/dosemu_mem` alias, end barrier, target-exit/fault paths, and
unprefixed `INT 21h/AH=30h` post-service acknowledgement already have focused
pinned-runtime proofs.

## Upstreaming later

Once a writable dosemu2 fork exists, replay these two frozen feature commits with `git am`
onto the pinned base, push those commits to the fork, and make the fork PR the
authoritative implementation. At that point this local carrier can either be
regenerated from the fork commits or removed once the integration fetches the
fork directly.
