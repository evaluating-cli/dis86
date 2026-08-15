use dis86::emu86::sst::policy::{decode_check, policy_for_file, v1_form_files, Capability, FormPolicy, Scope, FORM_POLICIES};
use dis86::emu86::sst::runner::{is_prefix_filtered, run_file, run_test, sample_detail, FileSummary, Outcome, RunOpts, DEFAULT_FLAGS_UMASK};
use moo::prelude::*;
use moo::test_file::MooTestFile;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: emu86_sst <COMMAND> [ARGS]

COMMANDS:
  dump <FILE.MOO> [n]   Print the first n tests (default 2) of a MOO test file
  run <FILE.MOO> ...    Execute tests against emu86 and print bucket summaries.
                        REPORT MODE: a FAIL bucket never fails the process; the
                        exit code is non-zero only on IO/parse/usage errors.
                        Options (after the files):
                          --sample N   stride = ceil(total/N) per file
                          --stride K   every K-th test (default 1 = all)
                          --all        also run prefix-prefixed tests (default:
                                       conservative filter skips them)
                          --check-writes  snapshot memory and report writes not
                                       declared in final.ram (slower)
                          --umask 0xXXXX   override the per-form FLAGS umask
                                       (default: policy umask, else 0x0FD7)
                          --revocations <path>  SHA1 revocation list (default:
                                       <file-dir>/revocation_list.txt)
  audit [--probe]       Print the classified capability inventory of all
                        upstream v1_real_mode forms (grouped V1 first), plus
                        the conservative-v1 fetch list. With --probe, also run
                        the real emu86 decoder against each form's canonical
                        probe bytes and flag any mismatch as !! PROBE-MISMATCH.
  micro-extract SPEC    Rebuild the checked-in hermetic micro corpus
                        (dis86/data/sst/micro) from the pinned full data
                        (dis86/data/sst/full) using the vendored MOO writer.
                        SPEC lines: '<out>.MOO <pass|failrepro> <src> <idx>'.
                        Options (after SPEC):
                          --sourcedir <DIR>   where <src>.MOO live (default
                                              dis86/data/sst/full)
                          --outdir <DIR>      where <out>.MOO are written
                                              (default dis86/data/sst/micro)
                          --verify            run each selected test through the
                                              real runner instead of writing,
                                              and report the outcome (pass
                                              entries must PASS; failrepro
                                              entries must diverge).
";

fn main() -> ExitCode {
  let mut args: Vec<String> = std::env::args().skip(1).collect();
  if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
    println!("{}", USAGE);
    return ExitCode::SUCCESS;
  }

  let command = args.remove(0);
  let result = match command.as_str() {
    "dump" => cmd_dump(&args),
    "run" => cmd_run(&args),
    "audit" => cmd_audit(&args),
    "micro-extract" => cmd_micro_extract(&args),
    other => {
      eprintln!("Error: unknown command '{}'", other);
      println!("{}", USAGE);
      return ExitCode::FAILURE;
    }
  };
  match result {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("Error: {}", e);
      ExitCode::FAILURE
    }
  }
}

fn cmd_dump(args: &[String]) -> Result<(), String> {
  if args.is_empty() {
    return Err("dump requires a FILE.MOO path".to_string());
  }

  let path = &args[0];
  let n = if args.len() > 1 {
    args[1]
      .parse::<usize>()
      .map_err(|_| format!("invalid count '{}'", args[1]))?
  }
  else {
    2
  };

  let bytes = std::fs::read(path).map_err(|e| format!("failed to read {}: {}", path, e))?;
  let mut cursor = Cursor::new(&bytes[..]);
  let mut file = MooTestFile::read(&mut cursor).map_err(|e| format!("failed to parse {}: {}", path, e))?;

  // Brief file-header summary.
  println!("== MOO file header ==");
  let (maj, min) = file.version();
  println!("version: {}.{}", maj, min);
  println!("arch:    {}", file.arch());
  println!("cpu:     {:?}", file.cpu_type());
  println!("tests:   {}", file.test_ct());
  if let Some(mask) = file.register_mask() {
    println!("regmask: {:?}", mask);
  }
  println!("");

  let stats = file.calc_stats(0);
  println!("stats:   exceptions_seen={:?} registers_modified={:?}", stats.exceptions_seen, stats.registers_modified);
  println!("");

  let tests = file.tests();
  let ct = tests.len().min(n);
  for (i, test) in tests.iter().take(ct).enumerate() {
    print_test(i, test);
  }

  Ok(())
}

fn load_moo(path: &str) -> Result<MooTestFile, String> {
  let bytes = std::fs::read(path).map_err(|e| format!("failed to read {}: {}", path, e))?;
  let mut cursor = Cursor::new(&bytes[..]);
  MooTestFile::read(&mut cursor).map_err(|e| format!("failed to parse {}: {}", path, e))
}

fn cmd_run(args: &[String]) -> Result<(), String> {
  let mut files: Vec<&str> = Vec::new();
  let mut sample: Option<usize> = None;
  let mut stride: Option<usize> = None;
  let mut all = false;
  let mut check_writes = false;
  let mut cli_umask: Option<u16> = None;
  let mut revocations: Option<String> = None;

  let mut i = 0;
  while i < args.len() {
    match args[i].as_str() {
      "--sample" => {
        i += 1;
        let v = args.get(i).ok_or("--sample requires a count")?;
        sample = Some(v.parse::<usize>().map_err(|_| format!("invalid --sample '{}'", v))?);
      }
      "--stride" => {
        i += 1;
        let v = args.get(i).ok_or("--stride requires a count")?;
        stride = Some(v.parse::<usize>().map_err(|_| format!("invalid --stride '{}'", v))?);
      }
      "--umask" => {
        i += 1;
        let v = args.get(i).ok_or("--umask requires a hex value like 0x0FD7")?;
        let v = v.trim_start_matches("0x").trim_start_matches("0X");
        cli_umask = Some(u16::from_str_radix(v, 16).map_err(|_| format!("invalid --umask '{}'", v))?);
      }
      "--revocations" => {
        i += 1;
        let v = args.get(i).ok_or("--revocations requires a path")?;
        revocations = Some(v.to_string());
      }
      "--all" => all = true,
      "--check-writes" => check_writes = true,
      f => files.push(f),
    }
    i += 1;
  }

  if files.is_empty() {
    return Err("run requires at least one FILE.MOO path".to_string());
  }
  if sample.is_some() && stride.is_some() {
    return Err("--sample and --stride are mutually exclusive".to_string());
  }

  // Revocation set: explicit --revocations path, else <first-file-dir>/revocation_list.txt.
  let fallback_dir = Path::new(files[0])
    .parent()
    .filter(|p| !p.as_os_str().is_empty())
    .map(|p| p.to_string_lossy().into_owned())
    .unwrap_or_else(|| ".".to_string());
  let skip_hashes = load_revocation_set(revocations.as_deref(), &fallback_dir);

  let mut aggregate = FileSummary::default();

  for path in &files {
    let file = load_moo(path)?;
    let total = file.tests().len();
    let stride_eff = match (sample, stride) {
      (Some(n), _) if n > 0 => total.div_ceil(n).max(1),
      (_, Some(k)) => k,
      _ => 1,
    };

    let policy = file_stem(path).and_then(|s| policy_for_file(&s));
    let effective_umask = cli_umask
      .or_else(|| policy.and_then(|p| p.flags_umask))
      .unwrap_or(DEFAULT_FLAGS_UMASK);
    let opts = RunOpts { flags_umask: effective_umask, check_extra_writes: check_writes };

    let summary = run_file(file.tests(), &opts, stride_eff, !all, Some(&skip_hashes), |_, _| ());

    println!("== {} ==", path);
    print_summary(&summary, total, stride_eff, effective_umask);

    aggregate.visited += summary.visited;
    aggregate.executed += summary.executed;
    aggregate.pass += summary.pass;
    aggregate.fail += summary.fail;
    aggregate.decode_err += summary.decode_err;
    aggregate.panic += summary.panic;
    aggregate.skip_exception += summary.skip_exception;
    aggregate.skip_32bit += summary.skip_32bit;
    aggregate.filtered += summary.filtered;
    aggregate.revoked += summary.revoked;
    aggregate.total += total;
    aggregate.samples.extend(summary.samples.iter().take(3).cloned().map(|mut s| {
      s.name = format!("{}: {}", path, s.name);
      s
    }));
  }

  if files.len() > 1 {
    println!("== AGGREGATE ==",);
    print_summary(&aggregate, aggregate.total, 1, cli_umask.unwrap_or(DEFAULT_FLAGS_UMASK));
  }

  Ok(())
}

/// Basename of the file sans the `.MOO` extension (the policy-table key).
fn file_stem(path: &str) -> Option<String> {
  let name = Path::new(path).file_name()?.to_string_lossy().into_owned();
  Some(name.strip_suffix(".MOO").unwrap_or(&name).to_string())
}

/// Load the pinned upstream SHA1 revocation list: an explicit `--revocations`
/// path, else `<fallback_dir>/revocation_list.txt` when present. Lines that are
/// blank or start with '#' are ignored. Missing files yield an empty set.
fn load_revocation_set(explicit: Option<&str>, fallback_dir: &str) -> HashSet<String> {
  let candidate = match explicit {
    Some(p) => Some(p.to_string()),
    None => {
      let p = Path::new(fallback_dir).join("revocation_list.txt");
      if p.is_file() { Some(p.to_string_lossy().into_owned()) } else { None }
    }
  };
  let mut set = HashSet::new();
  if let Some(candidate) = candidate {
    if let Ok(contents) = std::fs::read_to_string(&candidate) {
      for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
          continue;
        }
        set.insert(line.to_string());
      }
    }
  }
  set
}

fn print_summary(s: &FileSummary, total: usize, stride: usize, flags_umask: u16) {
  println!(
    "total={} visited={} (stride {}) executed={} flags_umask=0x{:04X}",
    total, s.visited, stride, s.executed, flags_umask
  );
  println!(
    "  PASS={} FAIL={} DECODE_ERR={} PANIC={} SKIP_EXCEPTION={} SKIP_32BIT={} FILTERED={} REVOKED={}",
    s.pass, s.fail, s.decode_err, s.panic, s.skip_exception, s.skip_32bit, s.filtered, s.revoked
  );
  for (i, sample) in s.samples.iter().take(3).enumerate() {
    println!("  sample[{}]: idx={} name={} hash={}", i, sample.idx, sample.name, sample.hash);
    println!("    {}", sample.detail);
  }
}

fn cmd_audit(args: &[String]) -> Result<(), String> {
  let probe = args.iter().any(|a| a == "--probe");
  if args.iter().any(|a| a != "--probe") {
    return Err(format!("audit takes only optional --probe, got: {}", args.join(" ")));
  }

  let mut v1: Vec<&FormPolicy> = FORM_POLICIES.iter().filter(|p| p.scope == Scope::V1).collect();
  let mut deferred: Vec<&FormPolicy> = FORM_POLICIES.iter().filter(|p| p.scope != Scope::V1).collect();
  v1.sort_by_key(|p| p.file);
  deferred.sort_by_key(|p| p.file);

  println!("== V1 (conservative subset, {}) ==", v1.len());
  print_audit_table(&v1, probe);
  println!();
  println!("== DEFERRED ({} total) ==", deferred.len());
  print_audit_table(&deferred, probe);

  // Totals.
  let mut impl_ct = 0;
  let mut donly_ct = 0;
  let mut undec_ct = 0;
  for p in FORM_POLICIES {
    match p.cap {
      Capability::Implemented => impl_ct += 1,
      Capability::DecodeOnly => donly_ct += 1,
      Capability::Undecodable => undec_ct += 1,
    }
  }
  println!();
  println!(
    "totals: Implemented={} DecodeOnly={} Undecodable={} | V1={} Deferred={} (entries={})",
    impl_ct,
    donly_ct,
    undec_ct,
    v1.len(),
    deferred.len(),
    FORM_POLICIES.len()
  );

  // Pinned revocation snapshot line count (best effort from the repo layout).
  let revoke_path = Path::new("dis86/data/sst/full/revocation_list.txt");
  match std::fs::read_to_string(revoke_path) {
    Ok(contents) => {
      let hashes = contents
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count();
      println!("revocation_list.txt: {} sha1 hashes ({})", hashes, revoke_path.display());
    }
    Err(_) => println!("revocation_list.txt: not found at {}", revoke_path.display()),
  }

  // Conservative v1 fetch list.
  let list = v1_form_files();
  println!();
  println!("files to fetch: {}", list.len());
  println!("./scripts/sst_fetch.sh dis86/data/sst/full {}", list.join(" "));

  Ok(())
}

fn print_audit_table(rows: &[&FormPolicy], probe: bool) {
  println!(
    "{:<13} {:<12} {:<20} {:<8} {}",
    "file", "cap", "scope", "umask", "note"
  );
  for p in rows {
    let umask = match p.flags_umask {
      Some(m) => format!("0x{:04X}", m),
      None => "default".to_string(),
    };
    let mut line = format!(
      "{:<13} {:<12} {:<20} {:<8} {}",
      p.file,
      p.cap.name(),
      p.scope.name(),
      umask,
      p.note
    );
    if probe {
      let ok = decode_check(p);
      let expected = p.cap != Capability::Undecodable;
      if ok != expected {
        line.push_str("  !! PROBE-MISMATCH");
      }
    }
    println!("{}", line);
  }
}

/// One spec line: '<out>.MOO <kind> <src> <idx>' — select one real
/// hardware-captured test from the pinned full data (src) and place it in a
/// micro MOO file (out), whose lane treats it as kind == "pass" or "failrepro".
struct MicroEntry {
  out: String,
  kind: String,
  src: String,
  idx: usize,
}

fn load_spec(path: &str) -> Result<Vec<MicroEntry>, String> {
  let contents =
    std::fs::read_to_string(path).map_err(|e| format!("failed to read {}: {}", path, e))?;
  let mut entries = Vec::new();
  for (ln, line) in contents.lines().enumerate() {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 4 {
      return Err(format!(
        "{}:{}: expected '<out>.MOO <pass|failrepro> <src> <idx>', got: {}",
        path,
        ln + 1,
        line
      ));
    }
    let idx = parts[3]
      .parse::<usize>()
      .map_err(|_| format!("{}:{}: bad idx '{}'", path, ln + 1, parts[3]))?;
    entries.push(MicroEntry {
      out: parts[0].to_string(),
      kind: parts[1].to_string(),
      src: parts[2].to_string(),
      idx,
    });
  }
  Ok(entries)
}

/// Rebuild a `MooTest` from its parts so it can be re-serialized by the
/// vendored writer (`MooTest` is not Clone). Everything — name, generator
/// metadata, bytes, both states, cycles, exception, and the upstream SHA1 —
/// is preserved verbatim.
fn reconstruct_test(test: &MooTest) -> MooTest {
  let hash = match test.hash_string().as_str() {
    "##NOHASH##" => None,
    hex => {
      let mut bytes = [0u8; 20];
      for i in 0..20 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("sha1 hex");
      }
      Some(bytes)
    }
  };
  MooTest::new(
    test.name().to_string(),
    test.gen_metadata().cloned(),
    test.bytes(),
    test.initial_state().clone(),
    test.final_state().clone(),
    test.cycles(),
    test.exception().cloned(),
    hash,
  )
}

fn micro_umask(stem: &str) -> u16 {
  policy_for_file(stem)
    .and_then(|p| p.flags_umask)
    .unwrap_or(DEFAULT_FLAGS_UMASK)
}

fn read_source_file(sourcedir: &str, src: &str) -> Result<MooTestFile, String> {
  let path = Path::new(sourcedir).join(format!("{}.MOO", src));
  let bytes = std::fs::read(&path).map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
  let mut cursor = Cursor::new(&bytes[..]);
  MooTestFile::read(&mut cursor).map_err(|e| format!("failed to parse {}: {}", path.display(), e))
}

fn cmd_micro_extract(args: &[String]) -> Result<(), String> {
  let mut spec_path: Option<String> = None;
  let mut outdir = "dis86/data/sst/micro".to_string();
  let mut sourcedir = "dis86/data/sst/full".to_string();
  let mut verify = false;

  let mut i = 0;
  while i < args.len() {
    match args[i].as_str() {
      "--outdir" => {
        i += 1;
        outdir = args.get(i).ok_or("--outdir requires a path")?.clone();
      }
      "--sourcedir" => {
        i += 1;
        sourcedir = args.get(i).ok_or("--sourcedir requires a path")?.clone();
      }
      "--verify" => verify = true,
      a if spec_path.is_none() => spec_path = Some(a.to_string()),
      a => return Err(format!("unexpected argument '{}'", a)),
    }
    i += 1;
  }
  let spec_path = spec_path.ok_or("micro-extract requires a SPEC path")?;
  let entries = load_spec(&spec_path)?;

  let mut sources: HashMap<String, MooTestFile> = HashMap::new();
  let mut extracted_for = |e: &MicroEntry| -> Result<MooTest, String> {
    let file = match sources.entry(e.src.clone()) {
      std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
      std::collections::hash_map::Entry::Vacant(v) => {
        v.insert(read_source_file(&sourcedir, &e.src)?)
      }
    };
    let test = file
      .tests()
      .get(e.idx)
      .ok_or_else(|| format!("{}: idx {} out of range ({} tests)", e.src, e.idx, file.test_ct()))?;
    let rebuilt = reconstruct_test(test);
    if rebuilt.hash_string() != test.hash_string() {
      return Err(format!(
        "{}: hash changed on reconstruction: {} -> {}",
        e.src,
        test.hash_string(),
        rebuilt.hash_string()
      ));
    }
    Ok(rebuilt)
  };

  if verify {
    let mut unexpected = 0usize;
    for e in &entries {
      let test = extracted_for(e)?;
      let outcome = run_test(&test, &RunOpts { flags_umask: micro_umask(&e.src), ..RunOpts::default() });
      // "pass" entries must PASS and must not be conservative-filtered (no
      // leading prefix byte) or exception-expected: the lane asserts
      // FILTERED == 0 and SKIP_EXCEPTION == 0 for every PASS micro file.
      let pass_ok = e.kind == "pass"
        && matches!(outcome, Outcome::Pass)
        && !is_prefix_filtered(test.bytes())
        && test.exception().is_none();
      let fail_ok = e.kind == "failrepro" && !matches!(outcome, Outcome::Pass);
      let mark = if pass_ok || fail_ok { "ok " } else { "!! " };
      if !pass_ok && !fail_ok {
        unexpected += 1;
      }
      let mut extra = String::new();
      if e.kind == "pass" && is_prefix_filtered(test.bytes()) {
        extra.push_str("  [prefix-filtered!]");
      }
      if e.kind == "pass" && test.exception().is_some() {
        extra.push_str("  [exception-expected!]");
      }
      println!(
        "{} {:12} src={}[{:<5}] name={:44} -> {}{}",
        mark,
        e.out,
        e.src,
        e.idx,
        test.name(),
        sample_detail(&test, &outcome),
        extra
      );
    }
    if unexpected > 0 {
      return Err(format!("micro-extract --verify: {} entry/entries diverged from their declared kind", unexpected));
    }
    println!("micro-extract --verify: all {} entries match their declared kind", entries.len());
    return Ok(());
  }

  // Write mode: group by target micro file (one MOO per family) and serialize.
  let mut grouped: Vec<(String, Vec<MooTest>)> = Vec::new();
  for e in &entries {
    let test = extracted_for(e)?;
    match grouped.iter_mut().find(|(o, _)| *o == e.out) {
      Some((_, v)) => v.push(test),
      None => grouped.push((e.out.clone(), vec![test])),
    }
  }
  std::fs::create_dir_all(&outdir).map_err(|e| format!("failed to create {}: {}", outdir, e))?;
  for (out, tests) in &grouped {
    let mut file = MooTestFile::new(1, 1, MooCpuType::Intel80286, tests.len());
    for t in tests {
      file.add_test(reconstruct_test(t));
    }
    let path = Path::new(&outdir).join(out);
    let mut cursor = Cursor::new(Vec::new());
    file.write(&mut cursor, true).map_err(|e| format!("failed to write {}: {}", path.display(), e))?;
    std::fs::write(&path, cursor.into_inner())
      .map_err(|e| format!("failed to write {}: {}", path.display(), e))?;
    println!("wrote {} ({} test(s))", path.display(), tests.len());
  }
  Ok(())
}

fn print_test(i: usize, test: &MooTest) {
  println!("== Test #{} ==", i);
  println!("name:       {}", test.name());
  println!("bytes:      {}", hex_bytes(test.bytes()));
  println!("hash:       {}", test.hash_string());

  let init = test.initial_state();
  let fin = test.final_state();

  println!("-- initial --");
  print_regs("regs", init.regs());
  print_ram("ram", init.ram(), 16);

  println!("-- final (sparse: only listed registers; absent means unchanged) --");
  print_regs_sparse("regs", fin.regs());
  print_ram("ram", fin.ram(), 16);

  let ram_deltas = ram_deltas(init.ram(), fin.ram());
  if ram_deltas.is_empty() {
    println!("ram deltas: (none)");
  }
  else {
    println!("ram deltas:");
    for (addr, from, to) in &ram_deltas {
      println!("  [0x{:05X}]: 0x{:02X} -> 0x{:02X}", addr, from, to);
    }
  }

  match test.exception() {
    Some(ex) => println!("exception:  #{} (flag addr 0x{:08X})", ex.exception_num, ex.flag_address),
    None => println!("exception:  (none)"),
  }
  println!("");
}

fn print_regs(label: &str, regs: &moo::registers::MooRegisters) {
  match regs {
    moo::registers::MooRegisters::Sixteen(r) => {
      println!(
        "{}: ax={:04X} bx={:04X} cx={:04X} dx={:04X} si={:04X} di={:04X} bp={:04X} sp={:04X}",
        label,
        r.ax().unwrap_or(0),
        r.bx().unwrap_or(0),
        r.cx().unwrap_or(0),
        r.dx().unwrap_or(0),
        r.si().unwrap_or(0),
        r.di().unwrap_or(0),
        r.bp().unwrap_or(0),
        r.sp().unwrap_or(0)
      );
      println!(
        "{}: ip={:04X} cs={:04X} ds={:04X} es={:04X} ss={:04X} flags={:04X}",
        label,
        r.ip().unwrap_or(0),
        r.cs().unwrap_or(0),
        r.ds().unwrap_or(0),
        r.es().unwrap_or(0),
        r.ss().unwrap_or(0),
        r.flags().unwrap_or(0)
      );
    }
    moo::registers::MooRegisters::ThirtyTwo(r) => {
      println!(
        "{}: eax={:08X} ebx={:08X} ecx={:08X} edx={:08X} esi={:08X} edi={:08X} ebp={:08X} esp={:08X}",
        label,
        r.eax().unwrap_or(0),
        r.ebx().unwrap_or(0),
        r.ecx().unwrap_or(0),
        r.edx().unwrap_or(0),
        r.esi().unwrap_or(0),
        r.edi().unwrap_or(0),
        r.ebp().unwrap_or(0),
        r.esp().unwrap_or(0)
      );
      println!(
        "{}: eip={:08X} cs={:04X} ds={:04X} es={:04X} ss={:04X} eflags={:08X}",
        label,
        r.eip().unwrap_or(0),
        r.cs().unwrap_or(0),
        r.ds().unwrap_or(0),
        r.es().unwrap_or(0),
        r.ss().unwrap_or(0),
        r.eflags().unwrap_or(0)
      );
    }
  }
}

/// Prints only the registers that are present (`Some`) in a (final) state —
/// the MOO final state is sparse and absent registers mean "unchanged".
fn print_regs_sparse(label: &str, regs: &moo::registers::MooRegisters) {
  let pairs: Vec<(&str, u32)> = match regs {
    moo::registers::MooRegisters::Sixteen(r) => [
      ("ax", r.ax()), ("bx", r.bx()), ("cx", r.cx()), ("dx", r.dx()),
      ("si", r.si()), ("di", r.di()), ("bp", r.bp()), ("sp", r.sp()),
      ("ip", r.ip()), ("cs", r.cs()), ("ds", r.ds()), ("es", r.es()),
      ("ss", r.ss()), ("flags", r.flags()),
    ]
    .iter()
    .filter_map(|(n, v)| v.map(|v| (*n, v as u32)))
    .collect(),
    moo::registers::MooRegisters::ThirtyTwo(r) => [
      ("eax", r.eax()), ("ebx", r.ebx()), ("ecx", r.ecx()), ("edx", r.edx()),
      ("esi", r.esi()), ("edi", r.edi()), ("ebp", r.ebp()), ("esp", r.esp()),
      ("eip", r.eip()), ("cs", r.cs().map(u32::from)), ("ds", r.ds().map(u32::from)),
      ("es", r.es().map(u32::from)), ("ss", r.ss().map(u32::from)),
      ("eflags", r.eflags()),
    ]
    .iter()
    .filter_map(|(n, v)| v.map(|v| (*n, v)))
    .collect(),
  };
  if pairs.is_empty() {
    println!("{}: (none listed)", label);
    return;
  }
  let body: Vec<String> = pairs.iter().map(|(n, v)| format!("{}={:04X}", n, v)).collect();
  println!("{}: {}", label, body.join(" "));
}

fn print_ram(label: &str, ram: &[moo::types::MooRamEntry], limit: usize) {
  let shown = ram.iter().take(limit).collect::<Vec<_>>();
  let mut line = format!("{}: ", label);
  for e in &shown {
    line.push_str(&format!("[0x{:05X}]=0x{:02X} ", e.address, e.value));
  }
  if ram.len() > limit {
    line.push_str(&format!("... ({} total)", ram.len()));
  }
  else if ram.is_empty() {
    line.push_str("(none)");
  }
  println!("{}", line.trim_end());
}

fn ram_deltas(
  init: &[moo::types::MooRamEntry],
  fin: &[moo::types::MooRamEntry],
) -> Vec<(u32, u8, u8)> {
  use std::collections::HashMap;
  let mut fin_map: HashMap<u32, u8> = HashMap::new();
  for e in fin {
    fin_map.insert(e.address, e.value);
  }
  let mut deltas = Vec::new();
  for e in init {
    if let Some(fv) = fin_map.get(&e.address) {
      if *fv != e.value {
        deltas.push((e.address, e.value, *fv));
      }
    }
  }
  for (addr, v) in fin_map {
    if !init.iter().any(|e| e.address == addr) {
      deltas.push((addr, 0, v));
    }
  }
  deltas.sort_by_key(|d| d.0);
  deltas
}

fn hex_bytes(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
  use super::*;
  use moo::types::{MooStateType, MooTestState};

  #[test]
  fn writer_reader_roundtrip_preserves_register() {
    // Build a minimal MOO file with one test in memory (writer -> reader roundtrip).
    let init = MooRegisters16Init {
      ax:    0x1234,
      bx:    0x0000,
      cx:    0x0000,
      dx:    0x0000,
      cs:    0xF000,
      ss:    0x0000,
      ds:    0x0000,
      es:    0x0000,
      sp:    0x0000,
      bp:    0x0000,
      si:    0x0000,
      di:    0x0000,
      ip:    0x0000,
      flags: 0x0002,
    };
    let fin = MooRegisters16Init {
      ax:    0x1235,
      ..init.clone()
    };

    let init_state = MooTestState::new(MooStateType::Initial, &MooRegistersInit::Sixteen(init.clone()), None, None, vec![], vec![]);
    let fin_state = MooTestState::new(
      MooStateType::Final,
      &MooRegistersInit::Sixteen(init),
      Some(&MooRegistersInit::Sixteen(fin)),
      None,
      vec![],
      vec![],
    );

    let test = MooTest::new(
      "roundtrip".to_string(),
      None,
      &[0x04, 0x01], // ADD AL, imm8
      init_state,
      fin_state,
      &[],
      None,
      None,
    );

    let mut file = MooTestFile::new(1, 1, MooCpuType::Intel80286, 1);
    file.add_test(test);

    let mut buf = Cursor::new(Vec::new());
    file.write(&mut buf, true).expect("write failed");
    buf.set_position(0);

    let read = MooTestFile::read(&mut buf).expect("read failed");
    assert_eq!(read.test_ct(), 1);

    let test = &read.tests()[0];
    let final_regs = match test.final_state().regs() {
      MooRegisters::Sixteen(r) => r,
      _ => panic!("expected 16-bit regs"),
    };
    assert_eq!(final_regs.ax(), Some(0x1235));
  }
}
