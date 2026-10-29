use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
  let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
  let dir = std::env::temp_dir().join(format!(
    "dis86-infer-{}-{}", std::process::id(), id));
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

fn row<'a>(stdout: &'a str, name: &str) -> &'a str {
  stdout.lines().find(|line| line.starts_with(&format!("{}\t", name)))
    .unwrap_or_else(|| panic!("no {} row: {}", name, stdout))
}

fn field(row: &str, index: usize) -> &str {
  row.split('\t').nth(index)
    .unwrap_or_else(|| panic!("short row: {}", row))
}

const ONE_FUNC: &str = "f { start 0000:0000 end 0000:0001 mode near ret u16 args 0 }";

#[test]
fn infer_abi_runs_in_range_mode() {
  let dir = temp_dir();
  let out = run(&dir, &[0xC3], ONE_FUNC, &[
    "--infer-abi", "--start-addr", "0000:0000", "--end-addr", "0000:0001",
  ]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stdout.contains("name\tstart\tend\targs\tdont_pop_args\tret\tregargs\tnote"),
          "stdout: {}", stdout);
  // The range path has no Func annotation, so args/regargs are unknown.
  assert!(stdout.contains("func_"), "stdout: {}", stdout);
  assert!(stdout.contains("\t?\tfalse\tu16\t-"), "stdout: {}", stdout);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_rejects_conflicting_modes() {
  for (args, needle) in [
    (vec!["--infer-abi", "--analyze"], "cannot be combined with --analyze"),
    (vec!["--infer-abi", "--validate-config"], "cannot be combined with --validate-config"),
    (vec!["--infer-abi", "--emit-code", "-"], "cannot be combined with an --emit-* mode"),
    (vec!["--infer-abi", "--build-pin-all"], "cannot be combined with --build-pin-all"),
  ] {
    let dir = temp_dir();
    let out = run(&dir, &[0xC3], ONE_FUNC, &args);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected failure for {:?}", args);
    assert!(stderr.contains(needle), "args {:?}, stderr: {}", args, stderr);
    assert!(!stderr.contains("panicked"), "args {:?}, stderr: {}", args, stderr);
    let _ = fs::remove_dir_all(dir);
  }
}

#[test]
fn infer_abi_preflight_is_non_fatal_and_reports_decodable_functions() {
  let dir = temp_dir();
  let functions = "\
    good { start 0000:0000 end 0000:0001 mode near ret u16 args 0 } \
    bad { start 0000:0001 end 0000:0003 mode near ret u16 args 0 }";
  let out = run(&dir, &[0xC3, 0x0F, 0xFF], functions,
                &["--infer-abi", "--codeseg-name", "main"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  // A validation/decode failure must not suppress the report.
  assert!(!out.status.success(), "the failed function must still fail the run");
  assert!(stdout.contains("good"), "stdout: {}", stdout);
  assert!(stdout.contains("bad"), "stdout: {}", stdout);
  assert!(stdout.contains("decode failed at 0000:0001"), "stdout: {}", stdout);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_skips_structurally_invalid_functions() {
  let dir = temp_dir();
  let functions = "\
    good { start 0000:0000 end 0000:0001 mode near ret u16 args 0 } \
    bad { start 0000:0010 end 0000:0020 mode near ret u16 args 0 }";
  let out = run(&dir, &[0xC3], functions, &["--infer-abi", "--codeseg-name", "main"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success(), "the invalid function must fail the run");
  assert!(stdout.contains("good"), "stdout: {}", stdout);
  assert!(stdout.contains("bad"), "stdout: {}", stdout);
  assert!(stdout.contains("outside the loaded image"), "stdout: {}", stdout);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_rejects_a_function_without_end_without_panicking() {
  let dir = temp_dir();
  let functions = "bad { start 0000:0000 mode near ret u16 args 0 }";
  let out = run(&dir, &[0xC3], functions, &["--infer-abi", "--name", "bad"]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success(), "the invalid function must fail the run");
  assert!(stderr.contains("No function 'end' property"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_marks_caller_observed_u32_return() {
  let dir = temp_dir();
  let code = [
    0xE8, 0x0D, 0x00, // call callee at 0x0010; next IP is 0x0003
    0x89, 0xD0,       // mov ax,dx
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xBA, 0x07, 0x00, // callee: mov dx,7
    0xB8, 0x03, 0x00, // callee: mov ax,3
    0xC3,             // callee: ret
  ];
  let functions = "\
    caller { start 0000:0000 end 0000:0006 mode near ret u16 args 0 } \
    callee { start 0000:0010 end 0000:0017 mode near ret u32 args 0 }";
  let out = run(&dir, &code, functions, &["--infer-abi", "--codeseg-name", "main"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  let callee_line = stdout.lines().find(|l| l.starts_with("callee\t"))
    .unwrap_or_else(|| panic!("no callee row: {}", stdout));
  assert!(callee_line.contains("\tu32\t"), "callee row: {}", callee_line);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_keeps_validation_errors_in_the_exit_code() {
  let dir = temp_dir();
  // A near function containing `retf` is a validation error but still
  // decodes, so inference reports it and must exit nonzero.
  let functions = "f { start 0000:0000 end 0000:0001 mode near ret u16 args 0 }";
  let out = run(&dir, &[0xCB], functions, &["--infer-abi", "--name", "f"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stdout.contains("name\tstart\tend"), "stdout: {}", stdout);
  assert!(stdout.contains("f\t"), "stdout: {}", stdout);
  assert!(!out.status.success(), "validation errors must fail the run");
  assert!(stderr.contains("returns far"), "stderr: {}", stderr);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_reports_an_out_of_image_name_without_panicking() {
  let dir = temp_dir();
  let functions = "f { start 0000:0010 end 0000:0020 mode near ret u16 args 0 }";
  let out = run(&dir, &[0xC3], functions, &["--infer-abi", "--name", "f"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success(), "the invalid range must fail the run");
  assert!(stdout.contains("f\t"), "stdout: {}", stdout);
  assert!(stdout.contains("outside the loaded image"), "stdout: {}", stdout);
  assert!(!stderr.contains("panicked"), "stderr: {}", stderr);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_keeps_annotated_ret_when_the_range_truncates() {
  for (ret, expected) in [
    ("u16", "u16 (annotated; ret outside the reachable range)"),
    ("u32", "u32 (annotated; ret outside the reachable range)"),
  ] {
    let dir = temp_dir();
    let functions = format!(
      "f {{ start 0000:0000 end 0000:0001 mode near ret {} args 0 }}", ret);
    // The range ends before the ret at 0x0001.
    let out = run(&dir, &[0x90, 0xC3], &functions, &["--infer-abi", "--name", "f"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", stderr);
    assert!(stdout.contains(expected), "ret {}: stdout: {}", ret, stdout);
    assert!(!stdout.contains("\tvoid\t"), "ret {}: stdout: {}", ret, stdout);
    let _ = fs::remove_dir_all(dir);
  }
}

#[test]
fn infer_abi_does_not_mistake_frame_teardown_for_arguments() {
  let dir = temp_dir();
  let code = [
    0x83, 0xEC, 0x08, // sub sp,8
    0xE8, 0x0A, 0x00, // call callee at 0x0010; next IP is 0x0006
    0x83, 0xC4, 0x08, // add sp,8 (frame teardown, not argument cleanup)
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xC3,             // callee: ret
  ];
  let functions = "\
    caller { start 0000:0000 end 0000:000A mode near ret u16 args 0 } \
    callee { start 0000:0010 end 0000:0011 mode near ret u16 args 0 }";
  let out = run(&dir, &code, functions, &["--infer-abi", "--codeseg-name", "main"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  let callee = row(&stdout, "callee");
  assert_eq!(field(callee, 3), "?", "callee row: {}", callee);
  assert!(field(callee, 7).contains("no call-site evidence"),
          "callee row: {}", callee);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_uses_pushed_arguments_to_corroborate_cleanup() {
  let dir = temp_dir();
  let code = [
    0x50,             // push ax
    0xE8, 0x0C, 0x00, // call callee at 0x0010; next IP is 0x0004
    0x83, 0xC4, 0x02, // add sp,2
    0xC3,             // ret
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xC3,             // callee: ret
  ];
  let functions = "\
    caller { start 0000:0000 end 0000:0008 mode near ret u16 args 0 } \
    callee { start 0000:0010 end 0000:0011 mode near ret u16 args 0 }";
  let out = run(&dir, &code, functions, &["--infer-abi", "--codeseg-name", "main"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  let callee = row(&stdout, "callee");
  assert_eq!(field(callee, 3), "1", "callee row: {}", callee);
  assert_eq!(field(callee, 4), "false", "callee row: {}", callee);
  assert!(field(callee, 7).contains("caller cleans 2 byte(s)"),
          "callee row: {}", callee);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_ignores_cleanup_from_an_unreachable_call() {
  let dir = temp_dir();
  let code = [
    0xC3,             // ret: the following bytes are unreachable
    0x50,             // push ax
    0xE8, 0x0B, 0x00, // call callee at 0x0010; next IP is 0x0005
    0x83, 0xC4, 0x02, // add sp,2
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // padding to 0x0010
    0xC3,             // callee: ret
  ];
  let functions = "\
    caller { start 0000:0000 end 0000:0008 mode near ret u16 args 0 } \
    callee { start 0000:0010 end 0000:0011 mode near ret u16 args 0 }";
  let out = run(&dir, &code, functions, &["--infer-abi", "--codeseg-name", "main"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  let callee = row(&stdout, "callee");
  assert_eq!(field(callee, 3), "?", "callee row: {}", callee);
  assert!(field(callee, 7).contains("no call-site evidence"),
          "callee row: {}", callee);
  let _ = fs::remove_dir_all(dir);
}

#[test]
fn infer_abi_marks_u32_return_from_a_high_byte_read() {
  for read in [[0x88, 0xF0], [0x88, 0xD0]] { // mov al,dh / mov al,dl
    let dir = temp_dir();
    let mut code = vec![0xE8, 0x0D, 0x00, read[0], read[1], 0xC3];
    code.extend([0x90; 10]);
    code.extend([0xBA, 0x07, 0x00, 0xB8, 0x03, 0x00, 0xC3]);
    let functions = "\
      caller { start 0000:0000 end 0000:0006 mode near ret u16 args 0 } \
      callee { start 0000:0010 end 0000:0017 mode near ret u16 args 0 }";
    let out = run(&dir, &code, functions, &["--infer-abi", "--codeseg-name", "main"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "read {:02X?}, stderr: {}", read, stderr);
    let callee = row(&stdout, "callee");
    assert_eq!(field(callee, 5), "u32", "read {:02X?}, callee row: {}", read, callee);
    let _ = fs::remove_dir_all(dir);
  }
}

#[test]
fn infer_abi_decodes_configured_regions_without_touching_gaps() {
  let dir = temp_dir();
  // `jmp 0x0008`; the gap contains invalid opcode bytes; `ret` at 8.
  let code = [0xE9, 0x05, 0x00, 0x63, 0x63, 0x63, 0x63, 0x63, 0xC3];
  let functions = "f { start 0000:0000 end \"\" mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0003 } _1 { start 0000:0008 end 0000:0009 } } }";
  let out = run(&dir, &code, functions, &["--infer-abi", "--name", "f"]);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stdout.contains("f\t"), "stdout: {}", stdout);
  assert!(!stdout.contains("decode failed"), "stdout: {}", stdout);
  assert!(!stdout.contains("outside the loaded image"), "stdout: {}", stdout);
  let _ = fs::remove_dir_all(dir);
}
