# Hardened SST full-corpus validation gate

Status: **OPEN — do not promote the historical zero-failure aggregate to a hardened-runner result until this gate is closed.**

This file is repository-tracked because GitHub Issues are disabled for this repository. It is the persistent follow-up for PR #30's review-hardening boundary.

## Why this is a merge-quality gate

PR #30 hardened the SST comparison path in two areas that can create false confidence if implemented incorrectly:

1. shift/rotate FLAGS masks are now refined from the effective per-test count, so defined count-1 OF and preserved ROL flags cannot be hidden by a form-wide mask;
2. undeclared final memory changes are checked by default using address-level write tracking.

Unit tests and the checked-in micro-corpus exercise these mechanics, but the hardened runner has not yet been run stride-1 over the full pinned Harris 80C286 corpus. The historical 268-form aggregate predates these changes and is not evidence that the stricter runner also produces zero failures.

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

cargo run --locked --bin emu86_sst -- audit --probe

cargo run --locked --release --bin emu86_sst -- run \
  --stride 1 \
  --revocations dis86/data/sst/full/revocation_list.txt \
  $(awk '!/^#/ && NF >= 2 { print "dis86/data/sst/full/" $1 ".MOO" }' \
      dis86/data/sst/manifest.txt)
```

The runner is report-mode: a `FAIL` bucket does not make the process exit non-zero. The printed aggregate must therefore be inspected and recorded explicitly.

## Acceptance criteria

- [ ] `audit --probe` agrees with the intended 268-form V1 scope.
- [ ] The full run uses PR #30's final hardened head (record exact commit SHA below).
- [ ] All 268 V1 files are included at `--stride 1`.
- [ ] `DECODE_ERR = 0` for executed V1 tests.
- [ ] `PANIC = 0` for executed V1 tests.
- [ ] Every `FAIL` is investigated; no failure is removed by broadening a mask without architecture/hardware justification.
- [ ] Variable/immediate shift/rotate failures are checked specifically for count-sensitive OF/AF/preserved-flag handling.
- [ ] Memory-writing failures are checked specifically for undeclared final writes.
- [ ] The authoritative aggregate is copied into `docs/emu86/sst.md` with the exact runner/head SHA and date.
- [ ] Only after the above is complete may `docs/emu86/sst.md` describe the hardened runner as full-corpus zero-failure, if the observed result actually supports that claim.

## Result

Hardened runner/head SHA: **pending**

Run date: **pending**

Aggregate: **pending**

Investigation notes: **pending**
