//! Hermetic micro-corpus lane (plan.md phase P4).
//!
//! Runs the checked-in real-data corpus at `data/sst/micro/` (resolved relative
//! to `CARGO_MANIFEST_DIR`, never the process cwd) through the REAL runner and
//! asserts the recorded outcomes. The lane is hermetic: it reads only files
//! committed in the repo and makes no network access and no use of the
//! gitignored 612MB `data/sst/full/` fetch.
//!
//! Corpus classes:
//! - **PASS files** (`<form>.MOO`, one per instruction family): the lane asserts
//!   every executed test PASSes and that nothing lands in FAIL / DECODE_ERR /
//!   PANIC / FILTERED / SKIP_EXCEPTION. Entries are hand-picked clean captures
//!   (no leading prefix byte, no exception key), so FILTERED and SKIP_EXCEPTION
//!   must be 0.
//! - **FAILREPRO files**: the lane asserts each entry diverges EXACTLY as
//!   recorded in the expectations table below (mirrored in
//!   `data/sst/micro/FAILREPRO.txt`), keyed by the test's SHA1. These pin the
//!   KNOWN emu86-bug behavior classified in docs/emu86/sst.md, guarding both
//!   the harness and emu86: when a future emu86 fix flips an entry to PASS the
//!   lane fails, prompting a corpus + registry update (documented contract in
//!   docs/emu86/sst.md).
//!
//! Regeneration (from the pinned full fetch) is out-of-band via
//! `emu86_sst micro-extract data/sst/micro/spec.txt`; spec.txt is the single
//! source of truth for the file list below.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use moo::test_file::MooTestFile;

use super::policy::policy_for_file;
use super::runner::{run_file, FileSummary, RunOpts, DEFAULT_FLAGS_UMASK};

/// Stems of the PASS micro files (one per family), mirroring `spec.txt`.
const PASS_STEMS: &[&str] = &[
  "04", "0C", "14", "1C", "24", "2D", "35", "3D", "40", "4F", "50", "58", "74",
  "89", "8B", "91", "98", "99", "9C", "A2", "B8", "C3", "D1.4", "E2", "E3",
  "EB", "F6.0", "F6.2", "F7.3", "F8", "FC",
];

/// Expected divergence of one FAILREPRO entry, keyed by filename and SHA1.
struct FailExpect {
  /// Micro file stem (also the policy-table key for the flag umask).
  file: &'static str,
  /// docs/emu86/sst.md cluster this pins.
  cluster: &'static str,
  /// First 20 hex chars of the test's upstream SHA1.
  hash_prefix: &'static str,
  /// Expected bucket tag ("FAIL" or "PANIC").
  bucket: &'static str,
  /// Exact `sample_detail` string the runner must produce.
  detail: &'static str,
}

const FAILREPRO_EXPECT: &[FailExpect] = &[
  FailExpect {
    file: "D1.0",
    cluster: "SST-D-003",
    hash_prefix: "f6b69bc8bdfed7bd1dd7",
    bucket: "FAIL",
    detail: "flags exp=0x0842 act=0x0843 umask=0x0FD7",
  },
  FailExpect {
    file: "F7.5",
    cluster: "SST-D-004",
    hash_prefix: "6b0be02aa172b5cd3b83",
    bucket: "FAIL",
    detail: "flags exp=0x0096 act=0x0883 umask=0x0801",
  },
  FailExpect {
    file: "F7.7",
    cluster: "SST-D-005",
    hash_prefix: "b08f0b9dd03d3b7d2646",
    bucket: "FAIL",
    detail: "AX exp=0xE5AA act=0x133E; DX exp=0x5DB7 act=0x2FB3",
  },
  FailExpect {
    file: "C1.4",
    cluster: "SST-D-009",
    hash_prefix: "aa19e33e8c0c96929dba",
    bucket: "PANIC",
    detail: "panic: assertion failed: val as u8 as u16 == val",
  },
  FailExpect {
    file: "87",
    cluster: "SST-D-006",
    hash_prefix: "eb65e3ae7e2c04e11592",
    bucket: "FAIL",
    detail: "[0xB4356] exp=0xA6 act=0x1E; [0xB4357] exp=0x8A act=0xA9",
  },
  FailExpect {
    file: "FF.5",
    cluster: "SST-D-007",
    hash_prefix: "0921a4aa520839742a7c",
    bucket: "FAIL",
    detail: "CS exp=0x848F act=0x0000",
  },
];

fn failrepro_stems() -> Vec<&'static str> {
  FAILREPRO_EXPECT.iter().map(|e| e.file).collect()
}

/// Resolve the checked-in micro corpus directory, anchored at the crate root
/// (`CARGO_MANIFEST_DIR` == dis86/) so the lane is hermetic and cwd-independent.
fn micro_dir() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("data/sst/micro")
}

fn load_file(name: &str) -> MooTestFile {
  let path = micro_dir().join(name);
  let bytes =
    std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
  let mut cursor = Cursor::new(&bytes[..]);
  MooTestFile::read(&mut cursor)
    .unwrap_or_else(|e| panic!("failed to parse {}: {}", path.display(), e))
}

/// Run one micro file through the real runner exactly like the CLI: stride 1,
/// conservative filter ON, per-form policy flag umask.
fn summary_for(stem: &str) -> FileSummary {
  let file = load_file(&format!("{}.MOO", stem));
  let umask = policy_for_file(stem)
    .and_then(|p| p.flags_umask)
    .unwrap_or(DEFAULT_FLAGS_UMASK);
  run_file(
    file.tests(),
    &RunOpts { flags_umask: umask, ..RunOpts::default() },
    1,
    true,
    None,
    |_, _| {},
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashSet;

  /// The micro/ directory must contain exactly the PASS stems plus the
  /// FAILREPRO stems (and no stragglers), and every PASS stem must have a
  /// matching file. This pins `micro.rs` to `data/sst/micro/spec.txt`.
  #[test]
  fn micro_corpus_files_match_registry() {
    let mut expected: HashSet<String> = PASS_STEMS
      .iter()
      .chain(failrepro_stems().iter())
      .map(|s| format!("{}.MOO", s))
      .collect();
    assert_eq!(expected.len(), PASS_STEMS.len() + FAILREPRO_EXPECT.len());

    let mut actual = HashSet::new();
    for entry in std::fs::read_dir(micro_dir()).expect("micro/ dir must exist") {
      let entry = entry.expect("read_dir entry");
      let name = entry.file_name().to_string_lossy().into_owned();
      if name == "spec.txt" || name == "FAILREPRO.txt" || name == "ATTRIBUTION.md" {
        continue;
      }
      assert!(name.ends_with(".MOO"), "unexpected non-MOO file: {}", name);
      assert!(
        expected.remove(&name),
        "micro file {} is not declared in the PASS or FAILREPRO registry",
        name
      );
      actual.insert(name);
    }
    assert!(
      expected.is_empty(),
      "registry-declared micro files missing from disk: {:?}",
      expected
    );
    assert_eq!(actual.len(), PASS_STEMS.len() + FAILREPRO_EXPECT.len());
  }

  /// Every PASS micro file must run 100% PASS: zero FAIL/DECODE_ERR/PANIC and
  /// zero FILTERED/SKIP_EXCEPTION (entries are chosen clean: no leading prefix
  /// byte, no exception key). REGRESSION: `emu86_sst micro-extract --verify`
  /// enforces the same properties at regeneration time.
  #[test]
  fn pass_micro_files_all_pass_cleanly() {
    let mut executed = 0usize;
    let mut passed = 0usize;
    for stem in PASS_STEMS {
      let s = summary_for(stem);
      assert_eq!(
        s.total, 1,
        "{}: micro files are single-test by construction, total={}",
        stem, s.total
      );
      assert_eq!(s.visited, 1, "{}: stride-1 must visit the only test", stem);
      assert_eq!(s.executed, 1, "{}: entry must be executed, not filtered/revoked", stem);
      assert_eq!(s.filtered, 0, "{}: entry must have no leading prefix byte", stem);
      assert_eq!(s.revoked, 0, "{}: entry must not be on the revocation list", stem);
      assert_eq!(s.skip_exception, 0, "{}: entry must not be exception-expected", stem);
      assert_eq!(s.skip_32bit, 0, "{}: entry must be 16-bit real-mode", stem);
      assert_eq!(s.pass, 1, "{}: entry must PASS, summary={:?}", stem, s);
      assert_eq!(s.fail, 0, "{}: unexpected FAIL", stem);
      assert_eq!(s.decode_err, 0, "{}: unexpected DECODE_ERR", stem);
      assert_eq!(s.panic, 0, "{}: unexpected PANIC", stem);
      executed += s.executed;
      passed += s.pass;
    }
    assert_eq!(executed, PASS_STEMS.len());
    assert_eq!(passed, PASS_STEMS.len());
  }

  /// Every FAILREPRO entry must diverge EXACTLY as recorded: the bucket tag and
  /// the full sample detail must match the expectations table, and the sampled
  /// test's SHA1 must be the pinned upstream one. A future emu86 fix that
  /// flips any of these to PASS will fail this test (contract in docs/emu86/sst.md).
  #[test]
  fn failrepro_micro_files_match_registry() {
    for expect in FAILREPRO_EXPECT {
      let s = summary_for(expect.file);
      assert_eq!(s.executed, 1, "{}: must be executed", expect.file);
      match expect.bucket {
        "FAIL" => {
          assert_eq!(s.fail, 1, "{}: expected FAIL bucket, summary={:?}", expect.file, s);
          assert_eq!(s.panic, 0, "{}: expected FAIL not PANIC", expect.file);
        }
        "PANIC" => {
          assert_eq!(s.panic, 1, "{}: expected PANIC bucket, summary={:?}", expect.file, s);
          assert_eq!(s.fail, 0, "{}: expected PANIC not FAIL", expect.file);
        }
        other => panic!("bad bucket '{}' in expectations table", other),
      }
      assert_eq!(s.decode_err, 0, "{}: no DECODE_ERR expected", expect.file);
      assert_eq!(s.samples.len(), 1, "{}: exactly one sample expected", expect.file);
      let sample = &s.samples[0];
      assert!(
        sample.hash.starts_with(expect.hash_prefix),
        "{}: SHA1 {} does not match pinned prefix {}",
        expect.file,
        sample.hash,
        expect.hash_prefix
      );
      assert_eq!(
        sample.detail, expect.detail,
        "{}: divergence detail changed (cluster {})",
        expect.file, expect.cluster
      );
    }
  }
}
