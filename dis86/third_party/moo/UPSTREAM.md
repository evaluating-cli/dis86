# Upstream: dbalsom/moo (vendored moo-rs)

This directory contains a vendored copy of the `moo-rs` crate, the Rust MOO
(Machine Opcode Operation) file parser/writer from the upstream `dbalsom/moo`
repository.

## Identity

- **Upstream URL**: https://github.com/dbalsom/moo
- **Pinned commit SHA**: `c438962d2b30856817d8e59bd5f0f4c628596ca8` (branch `main`)
- **Pinned date**: 2026-08-15 (commit resolved on this date)
- **Crate package name**: `moo-rs` (NOT the unrelated `moo` crate on crates.io)
- **Crate lib name**: `moo`
- **Crate version**: 0.3.0
- **Edition**: 2021
- **License**: MIT (see `LICENSE` in this directory)

## Rationale for vendoring

The MOO file format is used to store CPU emulator validation tests (SST /
Single Step Tests) for the 80286 target. We vendor the crate to:

- Keep builds fully **offline** and **reproducible** (pinned against crates.io
  is impossible for this crate anyway, since the crates.io `moo` name is an
  unrelated crate).
- **Pin** the parser to a specific upstream revision so format behavior cannot
  drift beneath us.
- **Control format evolution** as the upstream format spec (currently v1.1)
  changes over time; a vendored copy lets us audit and stage upgrades
  deliberately.

## What was copied / adjusted

Copied verbatim from upstream `crates/moo/`:

- `src/**` (parser/writer logic — NOT modified)
- `Cargo.toml` (workspace-inherited fields/deps concretized; see below)
- `LICENSE` (from the upstream repo root)

Additional:

- `doc/moo_format_v1.md` — the format spec doc, copied so that `src/lib.rs`'s
  `include_str!` docs attribute resolves under the path-dependency layout.

### Strictly-required adjustments only (no parser logic changes)

1. `Cargo.toml`: the upstream crate uses a cargo **workspace** and inherits
   `version`, `edition`, `authors`, `license`, `repository`, and dependency
   versions from the workspace root. Since this is vendored as a standalone
   (non-workspace) path dependency, those inherited values were inlined as
   concrete values, and the workspace dependency references (`binrw`,
   `sha1`, `env_logger`, `serde`, `log`, `thiserror`, `flate2`,
   `document-features`) were concretized to the versions the upstream
   workspace pins. The default features (`use_serde`, `gzip`) are preserved,
   so `gzip` support for reading `.MOO.gz` test files remains available.
2. `src/lib.rs`: the `#![doc = include_str!(...)]` attribute path was
   changed from `"../../../doc/moo_format_v1.md"` (workspace-relative) to
   `"../doc/moo_format_v1.md"` (path-dependency-relative). This is required
   for compilation.

No parser, writer, type, or format logic was modified.
