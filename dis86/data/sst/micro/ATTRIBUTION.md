# Hermetic micro-corpus — source attribution

The MOO test files in `dis86/data/sst/micro/` are a small, hand-picked subset
of the **SingleStepTests / 80286** real-mode hardware test suite, extracted
from the pinned fetch in `dis86/data/sst/full/` (which is gitignored) by
`emu86_sst micro-extract` (see `spec.txt`).

- **Suite:** SingleStepTests / 80286
- **Version:** v1.1.0 (`v1_real_mode` sub-directory)
- **Repository:** https://github.com/SingleStepTests/80286
- **Pinned commit:** `37c73caf53dcd22d3dd369ff09305d13d117a4fe` (refs/heads/main),
  also pinned with per-file SHA-256 hashes in `dis86/data/sst/manifest.txt`.
- **Origin of the data:** the tests are real hardware captures produced on a
  Harris N80C286-12 (80286 real mode). Each test records an initial CPU state,
  one instruction (plus a terminating HALT), and the resulting final state as
  observed on that hardware.
- **License:** the upstream suite carries an MIT license; these extracted
  sub-files are the same MIT-licensed content, redistributed in trimmed form.
- **Selection / provenance:** one MOO file per instruction family, each holding
  a single test selected for deterministic PASS (or, for `FAILREPRO.*`, a
  deterministic recorded divergence). See `spec.txt` for the exact
  source-file + index of every entry and the lane contract in
  `docs/emu86/sst.md` ("Hermetic micro-corpus (checked-in)").

The MOO **reader/writer** used to re-serialize these files is vendored from
https://github.com/dbalsom/moo (`crates/moo`, MOO-rs) under MIT license; see the
vendored crate's license at `dis86/third_party/moo/` (and its `UPSTREAM.md`,
which pins the upstream commit). The extracted test payloads are unchanged
upstream test data; only the containing file's header was re-written by the
vendored writer.
