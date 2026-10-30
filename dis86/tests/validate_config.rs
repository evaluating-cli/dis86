use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
  let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
  let dir = std::env::temp_dir().join(format!(
    "dis86-validate-{}-{}", std::process::id(), id));
  fs::create_dir_all(&dir).expect("create temp dir");
  dir
}

fn config_with_annotations(end: &str, annotations: &str) -> String {
  format!(r#"
dis86 {{
  code_segments {{
    _0000 {{ seg 0 name main }}
  }}
  functions {{
    f {{ start 0000:0000 end {} mode near ret u16 args 0 {} }}
  }}
  structures {{
  }}
  globals {{
  }}
  text_section {{
  }}
}}
"#, end, annotations)
}

fn config(end: &str) -> String {
  config_with_annotations(end, "")
}

fn run_validate_config(dir: &PathBuf, bsl_text: &str, extra: &[&str]) -> std::process::Output {
  let raw = dir.join("raw.bin");
  let bsl = dir.join("config.bsl");
  fs::write(&raw, [0xC3]).expect("write raw");
  fs::write(&bsl, bsl_text).expect("write bsl");
  let mut cmd = Command::new(env!("CARGO_BIN_EXE_dis86"));
  cmd.arg("--config").arg(&bsl)
    .arg("--binary-raw").arg(&raw);
  for arg in extra {
    cmd.arg(arg);
  }
  cmd.output().expect("run dis86")
}

fn run_validate(dir: &PathBuf, end: &str, extra: &[&str]) -> std::process::Output {
  run_validate_config(dir, &config(end), extra)
}

#[test]
fn validate_config_accepts_a_clean_raw_function() {
  let dir = temp_dir();
  let out = run_validate(&dir, "0000:0001", &["--validate-config"]);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("0 error(s)"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn validate_config_rejects_out_of_image_range() {
  let dir = temp_dir();
  let out = run_validate(&dir, "0000:0002", &["--validate-config"]);
  assert!(!out.status.success(), "unexpected success");
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("outside the loaded image"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn name_mode_preflight_blocks_bad_ranges_without_panicking() {
  let dir = temp_dir();
  let out = run_validate(&dir, "0000:0002", &["--name", "f", "--emit-code", "-"]);
  assert!(!out.status.success(), "unexpected success");
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("outside the loaded image"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn validate_config_accepts_regouts_and_clobbers() {
  let dir = temp_dir();
  let bsl = config_with_annotations(
    "0000:0001",
    "regargs AX,DS regouts DS,ES clobbers BX,SS",
  );
  let out = run_validate_config(&dir, &bsl, &["--validate-config"]);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn validate_config_rejects_an_unknown_contract_register() {
  let dir = temp_dir();
  let bsl = config_with_annotations("0000:0001", "regouts FOO");
  let out = run_validate_config(&dir, &bsl, &["--validate-config"]);
  assert!(!out.status.success(), "unexpected success");
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("regouts"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn validate_config_rejects_a_frame_register_as_a_contract_register() {
  let dir = temp_dir();
  let bsl = config_with_annotations("0000:0001", "clobbers SP");
  let out = run_validate_config(&dir, &bsl, &["--validate-config"]);
  assert!(!out.status.success(), "unexpected success");
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("Unsupported register"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn validate_config_rejects_ax_as_a_contract_register() {
  let dir = temp_dir();
  let bsl = config_with_annotations("0000:0001", "regouts AX");
  let out = run_validate_config(&dir, &bsl, &["--validate-config"]);
  assert!(!out.status.success(), "unexpected success");
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("Unsupported register"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}
