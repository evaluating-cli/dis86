use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
  let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
  let dir = std::env::temp_dir().join(format!(
    "dis86-report-{}-{}", std::process::id(), id));
  fs::create_dir_all(&dir).expect("create temp dir");
  dir
}

fn write_case(dir: &PathBuf, raw: &[u8], functions: &str) -> (PathBuf, PathBuf) {
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
"#, functions)).expect("write bsl");
  (raw_path, bsl_path)
}

fn run(dir: &PathBuf, raw: &[u8], functions: &str, extra: &[&str]) -> std::process::Output {
  let (raw_path, bsl_path) = write_case(dir, raw, functions);
  let mut cmd = Command::new(env!("CARGO_BIN_EXE_dis86"));
  cmd.arg("--config").arg(&bsl_path)
    .arg("--binary-raw").arg(&raw_path);
  for arg in extra {
    cmd.arg(arg);
  }
  cmd.output().expect("run dis86")
}

const ONE_FUNC: &str = "f { start 0000:0000 end 0000:0002 mode near ret u16 args 0 }";

#[test]
fn report_counts_an_unimpl_blocker() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let out = run(
    &dir,
    &[0xF4, 0xC3],
    ONE_FUNC,
    &["--codeseg-name", "main", "--emit-code", "-",
      "--report", report_path.to_str().unwrap()],
  );
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stdout.contains("UNIMPL()"), "stdout: {}", stdout);
  assert!(report.contains("segment 0000 (main)"), "report: {}", report);
  assert!(report.contains("functions configured 1 attempted 1 translated 1 failed 0"),
          "report: {}", report);
  assert!(report.contains("failed by cause: none"), "report: {}", report);
  assert!(report.contains("UNIMPL 1 first 0000:0000 (OP_HLT)"), "report: {}", report);
  assert!(report.contains("OP_HLT 1 first 0000:0000"), "report: {}", report);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn report_continues_after_a_decode_failure() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let functions = "\
    good { start 0000:0000 end 0000:0001 mode near ret u16 args 0 } \
    bad { start 0000:0001 end 0000:0003 mode near ret u16 args 0 }";
  let out = run(
    &dir,
    &[0xC3, 0x0F, 0xFF],
    functions,
    &["--codeseg-name", "main", "--emit-code", "-",
      "--report", report_path.to_str().unwrap()],
  );
  let stderr = String::from_utf8_lossy(&out.stderr);
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(!out.status.success(), "a decode failure must fail the run");
  assert!(stderr.contains("linear decode"), "stderr: {}", stderr);
  assert!(report.contains("functions configured 2 attempted 2 translated 1 failed 1"),
          "report: {}", report);
  assert!(report.contains("failed by cause: DECODE_ERROR 1"), "report: {}", report);
  assert!(report.contains("DECODE_ERROR 1 first 0000:0001"), "report: {}", report);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn report_text_is_deterministic() {
  let dir = temp_dir();
  let first = dir.join("first.txt");
  let second = dir.join("second.txt");
  let out = run(&dir, &[0xF4, 0xC3], ONE_FUNC,
    &["--codeseg-name", "main", "--emit-code", "-", "--report", first.to_str().unwrap()]);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let out = run(&dir, &[0xF4, 0xC3], ONE_FUNC,
    &["--codeseg-name", "main", "--emit-code", "-", "--report", second.to_str().unwrap()]);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  assert_eq!(
    fs::read_to_string(&first).expect("read first"),
    fs::read_to_string(&second).expect("read second"));
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn report_is_written_when_selection_fails() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let out = run(
    &dir,
    &[0xC3],
    ONE_FUNC,
    &["--name", "missing", "--report", report_path.to_str().unwrap()],
  );
  let stderr = String::from_utf8_lossy(&out.stderr);
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(!out.status.success(), "missing function must fail");
  assert!(stderr.contains("Failed to lookup function named: missing"), "stderr: {}", stderr);
  assert!(report.contains("global failure: CONFIG_ERROR: function selection failed"),
          "report: {}", report);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn report_rejects_incompatible_modes() {
  for (args, needle) in [
    (vec!["--report", "-", "--analyze"], "cannot be combined with --analyze"),
    (vec!["--report", "-", "--validate-config"], "cannot be combined with --validate-config"),
    (vec!["--report", "-", "--infer-abi"], "cannot be combined with --infer-abi"),
  ] {
    let dir = temp_dir();
    let out = run(&dir, &[0xC3], ONE_FUNC, &args);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected failure for {:?}", args);
    assert!(stderr.contains(needle), "args {:?}, stderr: {}", args, stderr);
    let _ = fs::remove_dir_all(dir);
  }
}
