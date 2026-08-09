# dosemu2 validator patch series

This directory carries the small dosemu2 core patch series required by the
`dis86` validator while we do not have a writable dosemu2 fork.

The patches are intended to be ordinary `git format-patch`-style mbox patches
against one exact upstream revision. They are not a vendored dosemu2 source
copy and should not be treated as an upstream PR by themselves.

## Upstream base

- Upstream: `https://github.com/dosemu2/dosemu2`
- Base branch: `devel`
- Base commit: `604ce0cdd1a71f657e2a2df623d216d5ab289313`
- Verified date: 2026-08-06
- Intended eventual dosemu2/fork PR title:
  `simx86: add executable-scoped validator stepping hook`

The base commit is intentionally pinned. Rebase/regeneration onto a newer
upstream revision should be explicit and reviewed as a separate change.

## Series status

The series is being built in small source-auditable stages:

1. `0001-simx86-add-validator-control-abi.patch`
   - page-sized `/hydra_remote` POSIX shared-memory object
     (`/dev/shm/hydra_remote` on Linux)
   - ABI version and structure-size fields
   - `init`, `end`, `pid`, `runtime_psp`, `req`, `ack`
   - register request/apply and publish/ack synchronization
   - exact MZ-entry activation gate using the DOS PSP environment program path
   - validator-only `MSSTP` mode
   - translated-node `decoded_instructions` from `TNode.seqnum`
   - raw node-boundary `step_flags`
   - final end acknowledgement before execution stops

2. Planned: target lifecycle hardening
   - explicit target-termination invalidation/cleanup
   - child/helper process scoping proof
   - stale-runtime-PSP rejection

3. Planned: low-memory export
   - force/verify `mapshm`
   - export the actual `lowmem_base` backing as `/dosemu_mem`
   - prove bidirectional aliasing before publication
   - safe cleanup and one-validator-instance behavior

## Validator launch contract

The validator launcher must select the deterministic simx86 C interpreter and
the shared-memory mapping driver explicitly:

```text
$_cpuemu = (1)
$_mapping = "mapshm"
```

Patch 0001 also fails closed if the simx86 interpreter is not selected.

The exact target gate expects these environment variables:

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
- `IP == MZ_IP`.

Only after that match is `runtime_psp` published and `init` release-stored.
This prevents startup helper programs from activating the hook merely because
they happen to execute under simx86.

## Node-boundary metadata

`TNode.seqlen` is byte length and is **not** an instruction count. Patch 0001
publishes `TNode.seqnum`, which is populated from the number of decoded
`IMeta` entries used to construct the translated node.

The initial `step_flags` are deliberately raw observable categories rather
than opcode guesses:

- `DIIS_STEP_MULTI_INSN`: translated node consumed more than one decoded instruction;
- `DIIS_STEP_SAME_PC`: the node returned to its starting PC;
- `DIIS_STEP_FAULT`: simx86 reported a CPU error/exception after the node;
- `DIIS_STEP_END_ACK`: final publication after observing `end` and before stopping.

This gives the Rust side actual node consumption without assuming that
`STI`, `MOV SS`, or `POP SS` necessarily consumed two instructions. More
specific classifications (for example interrupt-shadow versus partial REP)
can be added only where the source gives us a reliable discriminator.

## End barrier

At every validator-controlled node boundary, `end` is acquire-loaded before
any translated node is selected or executed. If set, the hook publishes the
current CPU state with `DIIS_STEP_END_ACK`, release-stores `ack`, and asks the
simx86 loop to leave dosemu without beginning another guest node.

A dedicated proof test is still required before the series is considered
ready for an upstream/fork PR.

## Applying

From the `dis86` checkout:

```sh
scripts/apply-dosemu2-patches.sh /path/to/dosemu2
```

The script refuses to apply to a dosemu2 checkout whose `HEAD` is not the
pinned base commit.

## Upstreaming later

Once a writable dosemu2 fork exists, replay this exact series with `git am`
onto the pinned base, push those commits to the fork, and make the fork PR the
authoritative implementation. At that point this local carrier can either be
regenerated from the fork commits or removed once the integration fetches the
fork directly.
