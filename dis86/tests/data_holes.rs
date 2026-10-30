use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
  let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
  let dir = std::env::temp_dir().join(format!(
    "dis86-data-holes-{}-{}", std::process::id(), id));
  fs::create_dir_all(&dir).expect("create temp dir");
  dir
}

fn write_case(dir: &PathBuf, raw: &[u8], functions: &str, text_section: &str) -> (PathBuf, PathBuf) {
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
    {}
  }}
}}
"#, functions, text_section)).expect("write bsl");
  (raw_path, bsl_path)
}

fn run(dir: &PathBuf, raw: &[u8], functions: &str, text_section: &str,
       extra: &[&str]) -> std::process::Output {
  let (raw_path, bsl_path) = write_case(dir, raw, functions, text_section);
  let mut cmd = Command::new(env!("CARGO_BIN_EXE_dis86"));
  cmd.arg("--config").arg(&bsl_path)
    .arg("--binary-raw").arg(&raw_path);
  for arg in extra {
    cmd.arg(arg);
  }
  cmd.output().expect("run dis86")
}

#[test]
fn jump_over_a_declared_hole_skips_the_data() {
  let dir = temp_dir();
  // jmp 0x0006; three declared data bytes; ret at 0x0006.
  let raw = [0xE9, 0x03, 0x00, 0x63, 0x63, 0x63, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0007 mode near ret u16 args 0 }";
  let text = "banner { start 0000:0003 end 0000:0006 }";

  let out = run(&dir, &raw, functions, text, &["--validate-config"]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stderr.contains("0 error(s)"), "stderr: {}", stderr);

  for selector in [&["--name", "f"][..], &["--codeseg-name", "main"][..]] {
    let mut args = selector.to_vec();
    args.extend_from_slice(&["--emit-dis", "-"]);
    let out = run(&dir, &raw, functions, text, &args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", stderr);
    assert!(stdout.contains("0000:0006"), "stdout: {}", stdout);
    assert!(!stdout.contains("0000:0003"), "data bytes were decoded: {}", stdout);
    assert!(!stdout.contains("0000:0004"), "data bytes were decoded: {}", stdout);
  }
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn fallthrough_into_a_hole_is_an_error() {
  let dir = temp_dir();
  let raw = [0x90, 0x63, 0x63, 0x63, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0005 mode near ret u16 args 0 }";
  let text = "banner { start 0000:0001 end 0000:0004 }";
  let out = run(&dir, &raw, functions, text, &["--validate-config"]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success());
  assert!(stderr.contains("falls through into declared data 'banner'"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn branch_target_into_a_hole_is_an_error() {
  let dir = temp_dir();
  let raw = [0xE9, 0x00, 0x00, 0x63, 0x63, 0x63, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0007 mode near ret u16 args 0 }";
  let text = "banner { start 0000:0003 end 0000:0006 }";

  let out = run(&dir, &raw, functions, text, &["--validate-config"]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success());
  assert!(stderr.contains("targets declared data 'banner'"), "stderr: {}", stderr);

  // Range mode has no preflight, so decode itself must reject the target.
  let out = run(&dir, &raw, functions, text, &[
    "--start-addr", "0000:0000", "--end-addr", "0000:0007", "--emit-code", "-",
  ]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success());
  assert!(stderr.contains("targets declared data 'banner'"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn unreachable_call_into_a_hole_fails_validation_and_decode() {
  let dir = temp_dir();
  // ret; unreachable call 0000:0005; ret; hole [5,7); ret at 7.
  let raw = [0xC3, 0xE8, 0x01, 0x00, 0xC3, 0x63, 0x63, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0008 mode near ret u16 args 0 }";
  let text = "hole { start 0000:0005 end 0000:0007 }";

  let out = run(&dir, &raw, functions, text, &["--validate-config"]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success());
  assert!(stderr.contains("call at 0000:0001 targets declared data 'hole'"), "stderr: {}", stderr);

  let out = run(&dir, &raw, functions, text, &[
    "--start-addr", "0000:0000", "--end-addr", "0000:0008", "--emit-code", "-",
  ]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success());
  assert!(stderr.contains("targets declared data 'hole'"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn data_fully_covering_a_function_region_is_an_error() {
  let dir = temp_dir();
  let raw = [0xC3];
  let functions = "f { start 0000:0000 end 0000:0001 mode near ret u16 args 0 }";
  let text = "banner { start 0000:0000 end 0000:0001 }";
  let out = run(&dir, &raw, functions, text, &["--validate-config"]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success());
  assert!(stderr.contains("fully covered"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn typed_jump_table_inside_a_function_is_decoded_as_data() {
  let dir = temp_dir();
  // jmp cs:[bx+5]; table entries 0009,0009; ret at 0009.
  let raw = [0x2E, 0xFF, 0xA7, 0x05, 0x00, 0x09, 0x00, 0x09, 0x00, 0xC3];
  let functions = "f { start 0000:0000 end 0000:000a mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0005 } _1 { start 0000:0009 end 0000:000a } } }";
  let text = "jt { start 0000:0005 end 0000:0009 type u16[2] }";
  let out = run(&dir, &raw, functions, text, &["--name", "f", "--emit-code", "-"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stdout.contains("switch (BX >> 1)"), "stdout: {}", stdout);
  assert!(!stdout.contains("UNIMPL()"), "stdout: {}", stdout);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn untyped_jump_table_shape_is_loud_without_panicking() {
  let dir = temp_dir();
  let raw = [0x2E, 0xFF, 0xA7, 0x05, 0x00, 0x09, 0x00, 0x09, 0x00, 0xC3];
  let functions = "f { start 0000:0000 end 0000:000a mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0005 } _1 { start 0000:0009 end 0000:000a } } }";
  let text = "hole { start 0000:0005 end 0000:0009 }";
  let out = run(&dir, &raw, functions, text, &["--name", "f", "--emit-code", "-"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stderr.contains("declared data 'hole'"), "stderr: {}", stderr);
  assert!(stderr.contains("not a jump table"), "stderr: {}", stderr);
  assert!(stdout.contains("UNIMPL()"), "stdout: {}", stdout);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn untyped_text_section_rejects_access() {
  let dir = temp_dir();
  let out = run(
    &dir,
    &[0xC3],
    "f { start 0000:0000 end 0000:0001 mode near ret u16 args 0 }",
    "hole { start 0000:0000 end 0000:0001 access 0000:0000 }",
    &["--validate-config"],
  );
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success());
  assert!(stderr.contains("access without a type"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}
