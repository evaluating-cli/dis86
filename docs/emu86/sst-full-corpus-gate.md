# Hardened SST full-corpus validation gate

Status: **CLOSED — the hardened runner was validated stride-1 over the full pinned 268-form V1 corpus on 2026-08-25.**

This file remains repository-tracked as the persistent checklist for PR #30's review-hardening boundary; GitHub Issue #31 is the durable issue tracker for the same gate.

## Why this was a merge-quality gate

PR #30 hardened the SST comparison path in two areas that can create false confidence if implemented incorrectly:

1. shift/rotate FLAGS masks are refined from the effective per-test count, so defined count-1 OF and preserved ROL flags cannot be hidden by a form-wide mask;
2. undeclared final memory changes are checked by default using address-level write tracking.

Unit tests and the checked-in micro-corpus exercise these mechanics. The full pinned Harris 80C286 corpus has now also been rerun with the hardened defaults enabled.

## Pinned evidence source

- repository: `SingleStepTests/80286`
- commit: `37c73caf53dcd22d3dd369ff09305d13d117a4fe`
- suite: `v1_real_mode`, v1.1.0
- policy scope: 268 V1 forms
- stride: 1
- count-sensitive FLAGS refinement: enabled (default)
- undeclared-write checking: enabled (default)
- revocation list: fetched from the same pinned upstream commit

`scripts/sst_fetch.sh` fetches the manifest-pinned `.MOO` files plus the pinned root-level `revocation_list.txt` and `CHANGELOG.md` into the gitignored full-corpus directory.

## Reproduce

From the repository root:

```sh
./scripts/sst_fetch.sh dis86/data/sst/full

cargo run --locked --manifest-path dis86/Cargo.toml --bin emu86_sst -- audit --probe

cargo run --locked --release --manifest-path dis86/Cargo.toml --bin emu86_sst -- run \
  --stride 1 \
  --revocations dis86/data/sst/full/revocation_list.txt \
  $(awk '!/^#/ && NF >= 2 { print "dis86/data/sst/full/" $1 ".MOO" }' \
      dis86/data/sst/manifest.txt)
```

The runner is report-mode: a `FAIL` bucket does not make the process exit non-zero. The printed aggregate must therefore be inspected and recorded explicitly.

## Acceptance criteria

- [x] `audit --probe` agrees with the intended 268-form V1 scope.
- [x] The full run uses PR #30's final hardened head (exact commit recorded below).
- [x] All 268 V1 files are included at `--stride 1`.
- [x] `DECODE_ERR = 0` for executed V1 tests.
- [x] `PANIC = 0` for executed V1 tests.
- [x] Every `FAIL` is investigated; no failure is removed by broadening a mask without architecture/hardware justification. The authoritative run produced zero FAILs.
- [x] Variable/immediate shift/rotate failures are checked specifically for count-sensitive OF/AF/preserved-flag handling. The 30 in-scope C0/C1/D0/D1/D2/D3 forms produced 0 FAIL / 0 DECODE_ERR / 0 PANIC.
- [x] Memory-writing failures are checked specifically for undeclared final writes. Undeclared-write checking remained enabled by default for the entire run and produced no failures.
- [x] The authoritative aggregate is copied into `docs/emu86/sst.md` with the exact runner/head SHA and date.
- [x] `docs/emu86/sst.md` may now describe the hardened runner as full-corpus zero-failure within the documented V1 scope.

## Result

Hardened runner/head SHA: **`d7ab2bd137cfe94a652ec0c69c947ec0483d9baa`**

Run date: **2026-08-25 (UTC)**

Files: **268 / 268 at stride 1**

Aggregate:

```text
== AGGREGATE ==
total=1212000 visited=1212000 (stride 1) executed=1064157 flags_umask=0x0FD7
  PASS=1050652 FAIL=0 DECODE_ERR=0 PANIC=0 SKIP_EXCEPTION=13505 SKIP_32BIT=0 FILTERED=147841 REVOKED=2
```

Investigation notes:

- The first repository-side run used the default pull-request checkout and therefore tested GitHub's synthetic merge SHA. Its aggregate was also zero-failure, but it was not accepted for this gate because the requested PR #30 head identity was not literal.
- The authoritative rerun explicitly checked out `d7ab2bd137cfe94a652ec0c69c947ec0483d9baa` before fetching or running the corpus.
- The documented root-level reproducer previously omitted `--manifest-path dis86/Cargo.toml`; because the repository has no root `Cargo.toml`, this gate and `docs/emu86/sst.md` now carry the corrected commands.
