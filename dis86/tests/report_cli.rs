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

fn mz_with_image(image: &[u8]) -> Vec<u8> {
  let exe_end = 32 + image.len() as u32;
  let (cp, cblp) = if exe_end % 512 == 0 {
    (exe_end / 512, 0)
  } else {
    (exe_end / 512 + 1, exe_end % 512)
  };
  let mut data = vec![0u8; exe_end as usize];
  data[0..2].copy_from_slice(b"MZ");
  data[2..4].copy_from_slice(&(cblp as u16).to_le_bytes());
  data[4..6].copy_from_slice(&(cp as u16).to_le_bytes());
  data[8..10].copy_from_slice(&2u16.to_le_bytes());
  data[24..26].copy_from_slice(&28u16.to_le_bytes());
  data[32..].copy_from_slice(image);
  data
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

fn run_exe(dir: &PathBuf, image: &[u8], functions: &str, extra: &[&str])
  -> std::process::Output
{
  let (_, bsl_path) = write_case(dir, &[], functions);
  let exe_path = dir.join("program.exe");
  fs::write(&exe_path, mz_with_image(image)).expect("write exe");
  let mut cmd = Command::new(env!("CARGO_BIN_EXE_dis86"));
  cmd.arg("--config").arg(&bsl_path)
    .arg("--binary-exe").arg(&exe_path);
  for arg in extra {
    cmd.arg(arg);
  }
  cmd.output().expect("run dis86")
}

const ONE_FUNC: &str = "f { start 0000:0000 end 0000:0002 mode near ret u16 args 0 }";
const ONE_BYTE_FUNC: &str = "f { start 0000:0000 end 0000:0001 mode near ret u16 args 0 }";

#[test]
fn report_includes_raw_byte_coverage() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let out = run(
    &dir,
    &[0xC3, 0x90, 0x90, 0x90],
    ONE_BYTE_FUNC,
    &["--codeseg-name", "main", "--emit-code", "-",
      "--report", report_path.to_str().unwrap()],
  );
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  assert!(report.contains(
    "  bytes size 0x0004 code 0x0001 data 0x0000 unknown 0x0003 annotated 0x0001 / 0x0004 (25.00%)"),
    "report: {}", report);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn report_includes_exe_byte_coverage() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let out = run_exe(
    &dir,
    &[0xC3, 0x90, 0x90, 0x90],
    ONE_BYTE_FUNC,
    &["--codeseg-name", "main", "--emit-code", "-",
      "--report", report_path.to_str().unwrap()],
  );
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  assert!(report.contains(
    "  bytes size 0x0004 code 0x0001 data 0x0000 unknown 0x0003 annotated 0x0001 / 0x0004 (25.00%)"),
    "report: {}", report);
  let _ = fs::remove_dir_all(dir);
}

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
fn report_records_an_ast_failure() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let functions = "f { start 0000:0000 end 0000:0006 mode near ret u16 args 0 }";
  let out = run(
    &dir,
    &[0x89, 0xE5, 0x8B, 0x46, 0xFB, 0xC3],
    functions,
    &["--codeseg-name", "main", "--emit-code", "-",
      "--report", report_path.to_str().unwrap()],
  );
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(!out.status.success(), "an AST failure must fail the run");
  assert!(report.contains("functions configured 1 attempted 1 translated 0 failed 1"),
    "report: {}", report);
  assert!(report.contains("failed by cause: CODEGEN_FAILURE 1"),
    "report: {}", report);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn report_counts_an_unimpl_flags_marker() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let functions = "f { start 0000:0000 end 0000:0005 mode near ret u16 args 0 }";
  let out = run(
    &dir,
    &[0x0F, 0x90, 0xC0, 0x50, 0xC3],
    functions,
    &["--codeseg-name", "main", "--emit-code", "-",
      "--report", report_path.to_str().unwrap()],
  );
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  assert!(report.contains("UNIMPL_FLAGS 1 first 0000:0000 (OP_SETO)"),
    "report: {}", report);
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
fn report_counts_a_range_run_as_one_unit() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let out = run(
    &dir,
    &[0xC3, 0x90],
    ONE_FUNC,
    &["--start-addr", "0000:0000", "--end-addr", "0000:0002",
      "--emit-code", "-", "--report", report_path.to_str().unwrap()],
  );
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  assert!(report.contains("functions configured 1 attempted 1 translated 1 failed 0"),
          "report: {}", report);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn report_records_preflight_failures_as_global_failures() {
  let dir = temp_dir();
  let report_path = dir.join("report.txt");
  let functions = "f { start 0000:0000 end 0000:0002 entry 0000:0003 mode near ret u16 args 0 }";
  let out = run(
    &dir,
    &[0xC3, 0x90],
    functions,
    &["--codeseg-name", "main", "--emit-code", "-",
      "--report", report_path.to_str().unwrap()],
  );
  let report = fs::read_to_string(&report_path).expect("read report");
  assert!(!out.status.success(), "the preflight error must fail the run");
  assert!(report.contains("functions configured 1 attempted 1 translated 1 failed 0"),
          "report: {}", report);
  assert!(report.contains("global failure: CONFIG_ERROR:"), "report: {}", report);
  assert!(report.contains("outside its configured range"), "report: {}", report);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn report_write_failure_is_reported_without_panicking() {
  let dir = temp_dir();
  let report_path = dir.join("missing").join("report.txt");
  let out = run(
    &dir,
    &[0xC3, 0x90],
    ONE_FUNC,
    &["--codeseg-name", "main", "--emit-code", "-",
      "--report", report_path.to_str().unwrap()],
  );
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success(), "an unwritable report must fail the run");
  assert!(stderr.contains("failed to write report"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
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
    (vec!["--report", "-", "--trace-analyze"], "cannot be combined with --trace-analyze"),
  ] {
    let dir = temp_dir();
    let out = run(&dir, &[0xC3], ONE_FUNC, &args);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected failure for {:?}", args);
    assert!(stderr.contains(needle), "args {:?}, stderr: {}", args, stderr);
    let _ = fs::remove_dir_all(dir);
  }
}
