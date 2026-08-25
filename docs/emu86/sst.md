# SST 80286 hardware validation — current status

This document is the current, review-hardened status of the SingleStepTests (SST)
80286 validation work for `emu86`. The detailed chronological divergence audit
from the original investigation is preserved verbatim in
[`sst-historical.md`](sst-historical.md).

## Hardware anchor

- Corpus: SingleStepTests/80286 `v1_real_mode`, suite v1.1.0.
- Pinned upstream commit: `37c73caf53dcd22d3dd369ff09305d13d117a4fe`.
- Capture target: Harris 80C286 hardware.
- Full corpus is fetched out-of-band into gitignored `dis86/data/sst/full/` from
  the SHA-256 manifest. CI uses the checked-in hermetic micro corpus instead.

## Scope

The current V1 policy contains **268 forms**: the original 258 conservative
forms plus 10 string forms (`A4-A7`, `AA-AF`) lifted in Track 3 R1.

Covered families include the step-implemented arithmetic, logical, move,
stack, control-flow, shift/rotate, multiply/divide, and flag operations, plus
the lifted MOVS/CMPS/STOS/LODS/SCAS string family. For the string family, the
hardware investigation also exercised and fixed REP handling, segment override
semantics, and the observed LOCK-prefixed cases. LOCK acceptance in the decoder
is intentionally scoped to these validated string operations; this work does
**not** claim general LOCK semantics.

Still deferred are INS/OUTS, non-string LOCK/prefix semantics not covered by the
V1 policy, port I/O, INT/INTO/IRET/HLT, exception-expected execution (emu86 has
no architectural exception machinery), and decode-only/not-implemented forms
such as DAA/DAS/AAA/AAS, PUSHA/POPA, BOUND, WAIT, LAHF/SAHF, ROR/RCL/RCR,
8-bit MUL/DIV/IMUL, CMC, AAM/AAD, ESC, LES/LDS, and ENTER/LEAVE.

## Authoritative hardened full-corpus result (2026-08-25)

The review-hardened runner was rerun at stride 1 over every manifest-pinned V1
file with count-sensitive FLAGS refinement and undeclared-final-write checking
left enabled at their defaults.

- Runner/head SHA: `d7ab2bd137cfe94a652ec0c69c947ec0483d9baa` (PR #30 hardened head).
- Pinned SST commit: `37c73caf53dcd22d3dd369ff09305d13d117a4fe`.
- V1 audit scope: 268 forms.
- Files run: 268 at `--stride 1`.

| metric | count |
|---|---:|
| files | 268 |
| tests visited | 1,212,000 |
| tests executed | 1,064,157 |
| PASS | 1,050,652 |
| FAIL | 0 |
| DECODE_ERR | 0 |
| PANIC | 0 |
| SKIP_EXCEPTION | 13,505 |
| FILTERED | 147,841 |
| REVOKED | 2 |

The aggregate independently reproduces the numerical zero-failure result of the
pre-review sweep, now under the stricter comparison mechanics. No FAIL bucket
was produced to investigate or justify away. In particular, all 30 in-scope
C0/C1/D0/D1/D2/D3 shift/rotate forms completed with 0 FAIL, 0 DECODE_ERR, and
0 PANIC, so the hardened count-sensitive OF/AF/preserved-flag path introduced no
full-corpus regression. Undeclared-final-write checking was enabled for the
entire run and produced no failure anywhere in the 268-form scope.

The complete per-form history, divergence samples, and resolution notes for
SST-D-001 through SST-D-015 are in `sst-historical.md`.

## Review hardening (2026-08-25)

The PR review found two ways the harness could overstate evidence and one
unrelated decompiler regression. The branch now addresses them as follows.

### Count-sensitive FLAGS comparison

Per-form masks alone cannot express the 80286 shift/rotate rule that definedness
depends on the effective count. The runner now decodes each test and refines the
policy mask for SHL/SHR/SAR/ROL using the effective count (`count & 0x1f`):

- count 0: flags are compared as preserved;
- count 1: OF is compared because it is defined;
- count >1: OF is ignored where architecturally undefined;
- SHL/SHR/SAR mask AF for non-zero counts;
- ROL preserves SF/ZF/PF/AF, so those preserved bits remain compared.

An explicit CLI `--umask` is treated as an exact diagnostic override and
therefore disables the automatic count-sensitive refinement.

Unit tests pin the important cases: a count-1 SHL OF mismatch must fail, a
count>1 OF-only difference is ignored, and an ROL preserved-AF mismatch must
fail.

### Undeclared memory writes are checked by default

The old `--check-writes` implementation cloned the complete emulated memory
image for each test, making full-sweep use impractical. `Memory` now supports
lightweight address-level write tracking: after SST initial state setup, it
records the original value only for addresses actually written by the
instruction. The runner compares those touched addresses with the final image
and reports final changes absent from `final.ram`.

Write tracking is enabled by default for SST runs. `--no-check-writes` exists
only for targeted diagnostics. Write-then-restore sequences are not reported as
unexpected because their final memory value equals the recorded original.

### Decompiler scope

The emulator's signed IDIV fix remains in this PR, but the attempted new IDIV
IR lowering was removed during review because it incorrectly split the quotient
into DX:AX instead of producing AX=quotient and DX=remainder, and the generic
signed AST path would have narrowed the 32-bit dividend. The decompiler is not
taught new IDIV semantics by this PR; OP_IDIV remains unsupported there rather
than being lowered incorrectly.

### LOCK scope

The decoder no longer treats LOCK as a globally discardable prefix. It accepts
LOCK only when the decoded operation is one of the hardware-validated string
operations (MOVS/CMPS/STOS/LODS/SCAS). Non-string LOCK-prefixed instructions
remain unsupported, preventing this SST work from silently broadening emulator
or decompiler semantics beyond the evidence.

## CPU divergences resolved by the fix series

The historical investigation identified and fixed these concrete emu86 defects:

- NEG overflow trap at signed minimum;
- stack/RET/XLAT wrapping arithmetic;
- sign-extended C1 shift-count handling;
- ROL CF/OF updates;
- XCHG memory effective-address recomputation after a register write;
- signed IMUL overflow detection;
- segment-offset wrapping for multi-byte memory accesses at 64 KiB;
- signed IDIV execution;
- runner terminating-HALT IP normalization;
- string destination segment override handling;
- REP LODS;
- the validated LOCK-prefixed string decode path.

Architecturally undefined flag residuals for logical operations, multiply, and
shifts are masked only where the architecture permits. The hardened runner's
count-sensitive refinement prevents form-wide masks from hiding defined
count-1 shift/rotate behavior.

## Reproduce

From the repository root, fetch the pinned corpus:

```sh
./scripts/sst_fetch.sh dis86/data/sst/full
```

Audit policy/decode coverage:

```sh
cargo run --locked --manifest-path dis86/Cargo.toml --bin emu86_sst -- audit --probe
```

Run all 268 forms with the hardened defaults (count-sensitive flags and
undeclared write checking enabled):

```sh
cargo run --locked --release --manifest-path dis86/Cargo.toml --bin emu86_sst -- run \
  --stride 1 \
  --revocations dis86/data/sst/full/revocation_list.txt \
  $(awk '!/^#/ && NF >= 2 { print "dis86/data/sst/full/" $1 ".MOO" }' \
      dis86/data/sst/manifest.txt)
```

The runner is report-mode, so inspect the printed aggregate rather than relying
on process exit status alone.

## Hermetic CI evidence

`dis86/data/sst/micro/` contains checked-in tests extracted from the same pinned
hardware corpus. The cargo-test lane executes them through the real SST runner
without network access. It is regression evidence for the harness and selected
CPU semantics, not a substitute for the full 268-form corpus.

The historical investigation and all detailed divergence accounting remain
available in `sst-historical.md`.
