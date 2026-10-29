use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
  let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
  let dir = std::env::temp_dir().join(format!(
    "dis86-regions-{}-{}", std::process::id(), id));
  fs::create_dir_all(&dir).expect("create temp dir");
  dir
}

fn write_case(dir: &PathBuf, raw: &[u8], function: &str) -> (PathBuf, PathBuf) {
  let raw_path = dir.join("raw.bin");
  let bsl_path = dir.join("config.bsl");
  fs::write(&raw_path, raw).expect("write raw");
  fs::write(&bsl_path, format!(r#"
dis86 {{
  code_segments {{
    _0000 {{ seg 0 name main }}
  }}
  functions {{
    {}
  }}
  structures {{
  }}
  globals {{
  }}
  text_section {{
  }}
}}
"#, function)).expect("write bsl");
  (raw_path, bsl_path)
}

fn run(dir: &PathBuf, raw: &[u8], function: &str, extra: &[&str]) -> std::process::Output {
  let (raw_path, bsl_path) = write_case(dir, raw, function);
  let mut cmd = Command::new(env!("CARGO_BIN_EXE_dis86"));
  cmd.arg("--config").arg(&bsl_path)
    .arg("--binary-raw").arg(&raw_path);
  for arg in extra {
    cmd.arg(arg);
  }
  cmd.output().expect("run dis86")
}

#[test]
fn regions_parse_validate_and_derive_a_missing_end() {
  let dir = temp_dir();
  let out = run(
    &dir,
    &[0x90, 0x90, 0x63, 0x63, 0xC3],
    r#"f { start 0000:0000 mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0002 } _1 { start 0000:0004 end 0000:0005 } } }"#,
    &["--validate-config"],
  );
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stderr.contains("0 error(s)"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn decompilation_skips_bytes_outside_regions() {
  let dir = temp_dir();
  let out = run(
    &dir,
    &[0x90, 0x90, 0xB8, 0xEF, 0xBE, 0xC3],
    r#"f { start 0000:0000 end 0000:0006 mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0002 } _1 { start 0000:0005 end 0000:0006 } } }"#,
    &["--name", "f", "--emit-code", "-"],
  );
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stdout.contains("u16 f(void)"), "stdout: {}", stdout);
  assert!(!stdout.contains("0xbeef"), "gap bytes were decoded: {}", stdout);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn auto_preflight_and_decompilation_follow_a_cross_region_branch() {
  let dir = temp_dir();
  // `jmp 0x0008`; offsets 3..8 are a data gap; `ret` at 8.
  let out = run(
    &dir,
    &[0xE9, 0x05, 0x00, 0x63, 0x63, 0x63, 0x63, 0x63, 0xC3],
    r#"f { start 0000:0000 end "" mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0003 } _1 { start 0000:0008 end 0000:0009 } } }"#,
    &["--name", "f", "--emit-code", "-"],
  );
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stdout.contains("u16 f(void)"), "stdout: {}", stdout);
  assert!(!stderr.contains("linear decode"), "stderr: {}", stderr);
  assert!(!stderr.contains("outside function"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn shared_epilogue_before_entry_decodes_entry_first() {
  let dir = temp_dir();
  let mut raw = vec![0xC3];
  raw.resize(0x10, 0x90);
  raw.extend_from_slice(&[0xEB, 0xEE]); // jmp 0x0000 from 0x0010
  let out = run(
    &dir,
    &raw,
    r#"f { start 0000:0010 end 0000:0012 mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0001 } _1 { start 0000:0010 end 0000:0012 } } }"#,
    &["--name", "f", "--emit-dis", "-"],
  );
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  let entry = stdout.find("0000:0010").unwrap_or_else(|| panic!("no entry line: {}", stdout));
  let epilogue = stdout.find("0000:0000").unwrap_or_else(|| panic!("no epilogue line: {}", stdout));
  assert!(entry < epilogue, "entry must decode before the shared epilogue: {}", stdout);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn branch_into_a_gap_stays_a_validation_warning() {
  let dir = temp_dir();
  let mut raw = vec![0xE9, 0x1D, 0x00]; // jmp 0x0020
  raw.resize(0x20, 0x63);              // gap bytes are not code
  raw.push(0xC3);
  let out = run(
    &dir,
    &raw,
    r#"f { start 0000:0000 end 0000:0021 mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0003 } } }"#,
    &["--validate-config"],
  );
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stderr.contains("outside its configured range"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}
