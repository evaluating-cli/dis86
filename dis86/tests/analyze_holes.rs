use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
  let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
  let dir = std::env::temp_dir().join(format!(
    "dis86-analyze-holes-{}-{}", std::process::id(), id));
  fs::create_dir_all(&dir).expect("create temp dir");
  dir
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

fn run_analyze(image: &[u8], functions: &str, text_section: &str) -> std::process::Output {
  run_analyze_with_args(image, functions, text_section, &[])
}

fn run_analyze_with_args(image: &[u8], functions: &str, text_section: &str,
                         extra: &[&str]) -> std::process::Output {
  let dir = temp_dir();
  let exe_path = dir.join("program.exe");
  let bsl_path = dir.join("config.bsl");
  fs::write(&exe_path, mz_with_image(image)).expect("write exe");
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
  let mut cmd = Command::new(env!("CARGO_BIN_EXE_dis86"));
  cmd.arg("--config").arg(&bsl_path)
    .arg("--binary-exe").arg(&exe_path)
    .arg("--analyze");
  for arg in extra {
    cmd.arg(arg);
  }
  let out = cmd.output().expect("run dis86 --analyze");
  let _ = fs::remove_dir_all(dir);
  out
}

#[test]
fn analyze_skips_and_reports_calls_into_data() {
  // call 0006; ret; nops; hole [6,8); ret at 8.
  let image = [0xE8, 0x03, 0x00, 0xC3, 0x90, 0x90, 0x63, 0x63, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0009 mode near ret u16 args 0 }";
  let text = "hole { start 0000:0006 end 0000:0008 }";
  let out = run_analyze(&image, functions, text);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);

  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stdout.contains("# CALL TARGET IN DATA | f | 0000:0006 | hole [0000:0006, 0000:0008)"),
    "stdout: {}", stdout);
  assert!(!stdout.contains("# UNDECODABLE GAP"), "stdout: {}", stdout);
  assert!(!stdout.contains("F_hole"), "the data target must not become a function: {}", stdout);
  assert!(stderr.contains("WARN: f: code at 0000:0008 follows declared data 'hole'"),
    "stderr: {}", stderr);
  assert!(stderr.contains("Coverage 0000 (main): code 7 (77.78%), data 2 (22.22%), unknown 0 (0.00%)"),
    "stderr: {}", stderr);
}

#[test]
fn analyze_reports_translation_blockers() {
  let image = [0x0F, 0x90, 0xC0, 0x50, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0005 mode near ret u16 args 0 }";
  let out = run_analyze(&image, functions, "");
  let stderr = String::from_utf8_lossy(&out.stderr);

  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stderr.contains("segment 0000 (main)"), "stderr: {}", stderr);
  assert!(stderr.contains("functions configured 1 attempted 1 translated 1 failed 0"),
    "stderr: {}", stderr);
  assert!(stderr.contains("UNIMPL_FLAGS 1 first 0000:0000 (OP_SETO)"),
    "stderr: {}", stderr);
}

#[test]
fn analyze_reports_ast_failures() {
  // mov bp, sp; mov ax, [bp-5]; ret: the local ends at -3, below the
  // return-address slot, so AST construction fails.
  let image = [0x89, 0xE5, 0x8B, 0x46, 0xFB, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0006 mode near ret u16 args 0 }";
  let out = run_analyze(&image, functions, "");
  let stderr = String::from_utf8_lossy(&out.stderr);

  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stderr.contains("functions configured 1 attempted 1 translated 0 failed 1"),
    "stderr: {}", stderr);
  assert!(stderr.contains("failed by cause: CODEGEN_FAILURE 1"),
    "stderr: {}", stderr);
}

#[test]
fn analyze_retains_blockers_when_ast_build_fails() {
  // mov bp, sp; seto al; mov [bp-5], al; ret: the marker survives long enough
  // to report, and the local below the return slot still fails AST construction.
  let image = [0x89, 0xE5, 0x0F, 0x90, 0xC0, 0x88, 0x46, 0xFB, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0009 mode near ret u16 args 0 }";
  let out = run_analyze(&image, functions, "");
  let stderr = String::from_utf8_lossy(&out.stderr);

  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stderr.contains("functions configured 1 attempted 1 translated 0 failed 1"),
    "stderr: {}", stderr);
  assert!(stderr.contains("failed by cause: CODEGEN_FAILURE 1"),
    "stderr: {}", stderr);
  assert!(stderr.contains("UNIMPL_FLAGS 1 first 0000:0002 (OP_SETO)"),
    "stderr: {}", stderr);
}

#[test]
fn analyze_honors_build_pin_all() {
  // seto al; mov al, 0; ret: the unused seto is removed normally but kept as a
  // pinned versioned local, so its marker must appear only with the flag.
  let image = [0x0F, 0x90, 0xC0, 0xB0, 0x00, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0006 mode near ret u16 args 0 }";
  let plain = run_analyze(&image, functions, "");
  let pinned = run_analyze_with_args(&image, functions, "", &["--build-pin-all"]);
  let plain_stderr = String::from_utf8_lossy(&plain.stderr);
  let pinned_stderr = String::from_utf8_lossy(&pinned.stderr);

  assert!(plain.status.success(), "stderr: {}", plain_stderr);
  assert!(pinned.status.success(), "stderr: {}", pinned_stderr);
  assert!(!plain_stderr.contains("UNIMPL_FLAGS"), "stderr: {}", plain_stderr);
  assert!(pinned_stderr.contains("UNIMPL_FLAGS 1 first 0000:0000 (OP_SETO)"),
    "stderr: {}", pinned_stderr);
}

#[test]
fn branch_target_after_a_hole_is_not_warned_as_unreachable() {
  // jmp 0008; hole [6,8); ret at 8, which is an explicit branch target.
  let image = [0xEB, 0x06, 0x90, 0x90, 0x90, 0x90, 0x63, 0x63, 0xC3];
  let functions = "f { start 0000:0000 end 0000:0009 mode near ret u16 args 0 }";
  let text = "hole { start 0000:0006 end 0000:0008 }";
  let out = run_analyze(&image, functions, text);
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);

  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(!stdout.contains("# UNDECODABLE GAP"), "stdout: {}", stdout);
  assert!(!stderr.contains("follows declared data"), "stderr: {}", stderr);
  assert!(stderr.contains("Coverage 0000 (main): code 7 (77.78%), data 2 (22.22%), unknown 0 (0.00%)"),
    "stderr: {}", stderr);
}

#[test]
fn gap_filling_hole_warns_for_a_non_target_region_start() {
  // Explicit regions [0,4) and [8,12), with the gap [4,8) declared as a
  // hole. The second region start is not a branch target or entry.
  let image = [0xC3, 0x90, 0x90, 0x90, 0x63, 0x63, 0x63, 0x63, 0xC3, 0x90, 0x90, 0x90];
  let functions = "f { start 0000:0000 end 0000:000c mode near ret u16 args 0 regions { _0 { start 0000:0000 end 0000:0004 } _1 { start 0000:0008 end 0000:000c } } }";
  let text = "hole { start 0000:0004 end 0000:0008 }";
  let out = run_analyze(&image, functions, text);
  let stderr = String::from_utf8_lossy(&out.stderr);

  assert!(out.status.success(), "stderr: {}", stderr);
  assert!(stderr.contains("WARN: f: code at 0000:0008 follows declared data 'hole'"),
    "stderr: {}", stderr);
}
