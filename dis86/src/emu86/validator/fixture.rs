//! Declarative fixture builder for the differential corpus.
//!
//! A fixture describes a small real-mode MZ program (raw instruction bytes)
//! plus the linear memory ranges that must be byte-for-byte identical between
//! the two backends at every validation boundary. The builder assembles a
//! valid MZ image from the bytes, mirroring the layout produced by
//! `scripts/make-dosemu2-runtime-probe.py` (entry CS:IP = 0000:0000).
//!
//! The comparison window deliberately excludes the PSP and the initial stack:
//! emu86 only partially populates the PSP (`loader.rs` notes missing fields) and
//! stack contents are not a deterministic contract across backends. Instead a
//! fixture opts into comparing the loaded image region plus any scratch ranges
//! its code writes (e.g. a string-copy destination buffer). See the plan:
//! `.opencode/plan.md`, "Compared memory = defined covered range".

use crate::segoff::SegOff;

/// A byte-for-byte compared linear address range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRange {
  pub start: usize,
  pub end: usize,
}

/// The deterministic memory window a fixture opts into comparing at every
/// boundary. Ranges are absolute linear addresses in conventional memory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryWindow {
  pub ranges: Vec<MemoryRange>,
}

impl MemoryWindow {
  pub fn is_empty(&self) -> bool {
    self.ranges.is_empty()
  }
}

/// Builds an `Emu`-side absolute `SegOff` whose linear address equals `abs`,
/// so both `mem_slice` implementations (which slice by `abs_normal()`) agree
/// on the same physical byte. `abs` must be below the 1 MiB wrap: the 20-bit
/// linear space is the largest a 16-bit segment can cover and emu86's exposed
/// memory ends at 0x10fff0, so fixture windows never legitimately exceed it.
pub fn seg_off_from_abs(abs: usize) -> SegOff {
  assert!(abs < 0x100000, "fixture linear address 0x{:x} exceeds the 20-bit real-mode space", abs);
  SegOff::new((abs >> 4) as u16, (abs & 0xf) as u16)
}

/// A declarative corpus fixture.
#[derive(Debug, Clone)]
pub struct Fixture {
  /// Stable, filesystem-safe name used for the generated `.exe` and reporting.
  pub name: String,
  /// Raw instruction bytes loaded at MZ entry CS:IP = 0000:0000.
  pub code: Vec<u8>,
  /// Absolute scratch ranges (outside the image) the code writes, compared too.
  pub scratch: Vec<MemoryRange>,
}

impl Fixture {
  pub fn new(name: impl Into<String>, code: Vec<u8>) -> Self {
    Self { name: name.into(), code, scratch: Vec::new() }
  }

  pub fn with_scratch(mut self, ranges: Vec<MemoryRange>) -> Self {
    self.scratch = ranges;
    self
  }

  /// The length of the loaded image region in bytes, from the MZ `e_cp` pages
  /// minus the header paragraphs, matching `Machine::load_exe`.
  fn image_length(&self) -> usize {
    let header_paragraphs = 2usize;
    let header_size = header_paragraphs * 16;
    let total_size = header_size + self.code.len();
    let pages = (total_size + 511) / 512;
    pages * 512 - header_size
  }

  /// Assembles the MZ image bytes (entry CS:IP = 0000:0000), following the
  /// layout of `make-dosemu2-runtime-probe.py::build_mz`.
  pub fn build_mz(&self) -> Vec<u8> {
    let header_paragraphs = 2u16;
    let header_size = header_paragraphs as usize * 16;
    let total_size = header_size + self.code.len();
    let pages = (total_size + 511) / 512;
    let last_page = total_size % 512;

    let words: [u16; 14] = [
      0x5A4D,             // e_magic "MZ"
      last_page as u16,   // e_cblp
      pages as u16,       // e_cp
      0,                  // e_crlc
      header_paragraphs,  // e_cparhdr
      0x0020,             // e_minalloc
      0xFFFF,             // e_maxalloc
      0x0000,             // e_ss
      0x0200,             // e_sp
      0x0000,             // e_csum
      0x0000,             // e_ip
      0x0000,             // e_cs
      0x001C,             // e_lfarlc
      0x0000,             // e_ovno
    ];
    let mut header = Vec::with_capacity(header_size);
    for w in words {
      header.extend_from_slice(&w.to_le_bytes());
    }
    header.resize(header_size, 0u8);

    let mut image = header;
    image.extend_from_slice(&self.code);
    image.resize(pages * 512, 0u8);
    image
  }

  /// The comparison window resolved for a run: the loaded image region rooted
  /// at `code_seg` plus the declared scratch ranges.
  pub fn window(&self, code_seg: u16) -> MemoryWindow {
    let start = code_seg as usize * 16;
    let end = start + self.image_length();
    let mut ranges = vec![MemoryRange { start, end }];
    ranges.extend_from_slice(&self.scratch);
    MemoryWindow { ranges }
  }

  /// Writes the fixture's MZ image to `dir/<name>.exe`, returning the path.
  pub fn write_to(&self, dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let path = dir.join(format!("{}.exe", self.name));
    std::fs::write(&path, self.build_mz())?;
    Ok(path)
  }

  /// Loads the fixture into a fresh headless emu86 machine (no dosemu2) so a
  /// host-side unit test can step it deterministically.
  pub fn into_emulator(&self) -> Result<crate::emu86::emu::Emulator, String> {
    let dir = std::env::temp_dir().join("dis86-fixture").join(&self.name);
    std::fs::create_dir_all(&dir)
      .map_err(|e| format!("failed to create fixture workdir {}: {}", dir.display(), e))?;
    let path = self.write_to(&dir)
      .map_err(|e| format!("failed to materialize fixture '{}': {}", self.name, e))?;
    crate::emu86::emu::Emulator::new(path.to_str()
      .ok_or_else(|| format!("fixture path is not valid UTF-8: {}", path.display()))?)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::binfmt::mz;

  #[test]
  fn mz_layout_matches_script_and_loader() {
    let fx = Fixture::new("layout", vec![0xb8, 0x34, 0x12]);
    let mz = fx.build_mz();
    assert_eq!(&mz[0..2], b"MZ");
    let decoded = mz::Exe::decode(&mz).unwrap();
    let cs = decoded.hdr.cs;
    let ip = decoded.hdr.ip;
    assert_eq!(cs, 0);
    assert_eq!(ip, 0);
    // Image region length from e_cp pages minus header paragraphs.
    let header_paragraphs = decoded.hdr.cparhdr as usize * 16;
    let image_end = decoded.hdr.cp as usize * 512;
    assert_eq!(fx.image_length(), image_end - header_paragraphs);
    assert_eq!(&mz[header_paragraphs..header_paragraphs + 3], &[0xb8, 0x34, 0x12]);
  }

  #[test]
  fn image_length_tracks_pages() {
    // 3 code bytes in a 2-paragraph header => 32 + 3 = 35 bytes => 1 page (512).
    let fx = Fixture::new("len", vec![0x90, 0x90, 0x90]);
    assert_eq!(fx.image_length(), 512 - 32);
  }

  #[test]
  fn window_combines_image_region_and_scratch() {
    let fx = Fixture::new("win", vec![0x90])
      .with_scratch(vec![MemoryRange { start: 0x90000, end: 0x90020 }]);
    let window = fx.window(0x1000);
    assert_eq!(window.ranges, vec![
      MemoryRange { start: 0x10000, end: 0x10000 + (512 - 32) },
      MemoryRange { start: 0x90000, end: 0x90020 },
    ]);
  }

  #[test]
  fn seg_off_from_abs_roundtrips_linear_address() {
    for abs in [0usize, 0x20, 0x12345, 0x90000, 0x9ffff, 0xffff0, 0xfffff] {
      assert_eq!(seg_off_from_abs(abs).abs_normal(), abs);
    }
  }

  #[test]
  #[should_panic(expected = "exceeds the 20-bit real-mode space")]
  fn seg_off_from_abs_rejects_addresses_at_or_past_1mib() {
    seg_off_from_abs(0x100000);
  }
}

/// REP string-operation fixtures with host-side (emu86-only) tests.
///
/// These exercise the four real-mode string ops (`MOVS`, `STOS`, `CMPS`,
/// `SCAS`) under a REP/REPE/REPNE prefix across the direction-flag, element
/// size, and termination (CX-exhaust vs condition-exit) matrix. Each fixture
/// is a tiny real-mode MZ program whose instruction bytes are the only thing
/// loaded into the emulator; the source/destination buffers live in distinct
/// absolute scratch ranges in conventional memory that the test fills and then
/// byte-compares.
///
/// Non-obvious emu86 REP semantics exercised here: each string op executes the
/// whole REP inside a single `Machine::step()` (the CPU ops have an internal
/// `while count != 0` loop). MOVS/STOS always run to CX-exhaustion; CMPS/SCAS
/// break out early on their prefix's exit condition (REPE on ZF==0, REPNE on
/// ZF==1), and perform zero iterations when CX==0 at entry.
#[cfg(test)]
mod rep_fixtures {
  use super::*;
  use crate::emu86::emu::Emulator;
  use crate::emu86::machine::Machine;

  // Source and destination buffers as distinct absolute linear ranges in
  // conventional memory (both << 0xA0000, disjoint so a MOVS overwriting the
  // destination can never corrupt a not-yet-read source byte). Chosen with
  // zero segment offsets so SI/DI arithmetic stays within a single segment.
  const SRC_SEG: u16 = 0x5000;
  const DST_SEG: u16 = 0x4000;
  const SRC_ABS: usize = (SRC_SEG as usize) << 4;
  const DST_ABS: usize = (DST_SEG as usize) << 4;
  const COUNT: u16 = 8;

  fn mov_ax(imm: u16) -> [u8; 3] {
    [0xb8, imm as u8, (imm >> 8) as u8]
  }
  fn mov_si(imm: u16) -> [u8; 3] {
    [0xbe, imm as u8, (imm >> 8) as u8]
  }
  fn mov_di(imm: u16) -> [u8; 3] {
    [0xbf, imm as u8, (imm >> 8) as u8]
  }
  fn mov_cx(imm: u16) -> [u8; 3] {
    [0xb9, imm as u8, (imm >> 8) as u8]
  }
  fn dir_byte(df: bool) -> u8 {
    if df { 0xfd } else { 0xfc } // STD / CLD
  }

  fn write_bytes(m: &mut Machine, abs: usize, bytes: &[u8]) {
    for (i, &b) in bytes.iter().enumerate() {
      m.mem.write_u8(seg_off_from_abs(abs + i), b);
    }
  }

  fn fill(m: &mut Machine, abs: usize, len: usize, val: u8) {
    for i in 0..len {
      m.mem.write_u8(seg_off_from_abs(abs + i), val);
    }
  }

  fn read_bytes(m: &Machine, abs: usize, len: usize) -> Vec<u8> {
    (0..len).map(|i| m.mem.read_u8(seg_off_from_abs(abs + i))).collect()
  }

  fn step_emulator(emu: &mut Emulator, n: usize) {
    for _ in 0..n {
      emu.step().unwrap();
    }
  }

  // Build a REP MOVS program: load DS:SI=source, ES:DI=dest, CX=count, set
  // direction, then `rep movs{s|w}`. For DF=1 the pointers start at the end of
  // their buffers (the real-mode convention for downward copies) and decrement.
  fn movs_code(size: usize, df: bool, count: u16) -> Vec<u8> {
    let len = size * count as usize;
    // Backward pointers start at the last element (`len - size`) because emu86
    // writes at [SI]/[DI] before decrementing; starting at `len` would leave the
    // low element untouched and step one past the end.
    let (src_off, dst_off) = if df {
      (len.saturating_sub(size) as u16, len.saturating_sub(size) as u16)
    } else {
      (0, 0)
    };
    let mut c = Vec::new();
    c.extend_from_slice(&mov_ax(SRC_SEG));
    c.extend_from_slice(&[0x8e, 0xd8]); // mov ds, ax
    c.extend_from_slice(&mov_ax(DST_SEG));
    c.extend_from_slice(&[0x8e, 0xc0]); // mov es, ax
    c.extend_from_slice(&mov_si(src_off));
    c.extend_from_slice(&mov_di(dst_off));
    c.extend_from_slice(&mov_cx(count));
    c.push(dir_byte(df));
    c.push(0xf3); // rep
    c.push(if size == 1 { 0xa4 } else { 0xa5 }); // movsb / movsw
    c.push(0x90); // nop
    c
  }

  fn movs_steps() -> usize {
    10 // 7 movs + dir + rep op + nop
  }

  fn run_movs(size: usize, df: bool, count: u16, src: &[u8]) -> Machine {
    let len = size * count as usize;
    let fx = Fixture::new(format!("rep_movs_{}_{}_{}", size, df, count), movs_code(size, df, count))
      .with_scratch(vec![
        MemoryRange { start: SRC_ABS, end: SRC_ABS + len },
        MemoryRange { start: DST_ABS, end: DST_ABS + len },
      ]);
    let mut emu = fx.into_emulator().unwrap();
    write_bytes(&mut emu.machine, SRC_ABS, src);
    fill(&mut emu.machine, DST_ABS, len, 0x00);
    step_emulator(&mut emu, movs_steps());
    emu.machine
  }

  // Build a REP STOS program: load ES:DI=dest, CX=count, AL/AX fill value, set
  // direction, then `rep stos{s|w}`.
  fn stos_code(size: usize, df: bool, count: u16) -> Vec<u8> {
    let len = size * count as usize;
    let dst_off = if df { len.saturating_sub(size) as u16 } else { 0 };
    let mut c = Vec::new();
    c.extend_from_slice(&mov_ax(DST_SEG));
    c.extend_from_slice(&[0x8e, 0xc0]); // mov es, ax
    c.extend_from_slice(&mov_di(dst_off));
    c.extend_from_slice(&mov_cx(count));
    if size == 1 {
      c.extend_from_slice(&[0xb0, 0x5a]); // mov al, 0x5a
    } else {
      c.extend_from_slice(&mov_ax(0xabcd)); // mov ax, 0xabcd
    }
    c.push(dir_byte(df));
    c.push(0xf3); // rep
    c.push(if size == 1 { 0xaa } else { 0xab }); // stosb / stosw
    c.push(0x90); // nop
    c
  }

  fn stos_steps() -> usize {
    8 // 4 movs + fill + dir + rep op + nop
  }

  fn run_stos(size: usize, df: bool, count: u16) -> Machine {
    let len = size * count as usize;
    let fx = Fixture::new(format!("rep_stos_{}_{}_{}", size, df, count), stos_code(size, df, count))
      .with_scratch(vec![
        MemoryRange { start: DST_ABS, end: DST_ABS + len },
      ]);
    let mut emu = fx.into_emulator().unwrap();
    fill(&mut emu.machine, DST_ABS, len, 0x00);
    step_emulator(&mut emu, stos_steps());
    emu.machine
  }

  // Build a REP CMPS program: DS:SI=source, ES:DI=dest, CX=count, direction,
  // then `(rep|repne) cmps{s|w}`. `prefix` is 0xf3 (REPE) or 0xf2 (REPNE).
  fn cmps_code(size: usize, df: bool, count: u16, prefix: u8) -> Vec<u8> {
    let len = size * count as usize;
    let (src_off, dst_off) = if df {
      (len.saturating_sub(size) as u16, len.saturating_sub(size) as u16)
    } else {
      (0, 0)
    };
    let mut c = Vec::new();
    c.extend_from_slice(&mov_ax(SRC_SEG));
    c.extend_from_slice(&[0x8e, 0xd8]); // mov ds, ax
    c.extend_from_slice(&mov_ax(DST_SEG));
    c.extend_from_slice(&[0x8e, 0xc0]); // mov es, ax
    c.extend_from_slice(&mov_si(src_off));
    c.extend_from_slice(&mov_di(dst_off));
    c.extend_from_slice(&mov_cx(count));
    c.push(dir_byte(df));
    c.push(prefix);
    c.push(if size == 1 { 0xa6 } else { 0xa7 }); // cmpsb / cmpsw
    c.push(0x90); // nop
    c
  }

  fn cmps_steps() -> usize {
    10 // 7 movs + dir + rep op + nop
  }

  fn run_cmps(size: usize, df: bool, count: u16, prefix: u8, src: &[u8], dst: &[u8]) -> Machine {
    let len = size * count as usize;
    let fx = Fixture::new(
      format!("rep_cmps_{}_{}_{}_{}", size, df, count, prefix),
      cmps_code(size, df, count, prefix),
    ).with_scratch(vec![
      MemoryRange { start: SRC_ABS, end: SRC_ABS + len },
      MemoryRange { start: DST_ABS, end: DST_ABS + len },
    ]);
    let mut emu = fx.into_emulator().unwrap();
    write_bytes(&mut emu.machine, SRC_ABS, src);
    write_bytes(&mut emu.machine, DST_ABS, dst);
    step_emulator(&mut emu, cmps_steps());
    emu.machine
  }

  // Build a REP SCAS program: ES:DI=dest, CX=count, AL/AX search value,
  // direction, then `(rep|repne) scas{s|w}`.
  fn scas_code(size: usize, df: bool, count: u16, prefix: u8) -> Vec<u8> {
    let len = size * count as usize;
    let dst_off = if df { len.saturating_sub(size) as u16 } else { 0 };
    let mut c = Vec::new();
    c.extend_from_slice(&mov_ax(DST_SEG));
    c.extend_from_slice(&[0x8e, 0xc0]); // mov es, ax
    c.extend_from_slice(&mov_di(dst_off));
    c.extend_from_slice(&mov_cx(count));
    if size == 1 {
      c.extend_from_slice(&[0xb0, 0x42]); // mov al, 0x42
    } else {
      c.extend_from_slice(&mov_ax(0x1234)); // mov ax, 0x1234
    }
    c.push(dir_byte(df));
    c.push(prefix);
    c.push(if size == 1 { 0xae } else { 0xaf }); // scasb / scasw
    c.push(0x90); // nop
    c
  }

  fn scas_steps() -> usize {
    8 // 4 movs + fill + dir + rep op + nop
  }

  fn run_scas(size: usize, df: bool, count: u16, prefix: u8, data: &[u8]) -> Machine {
    let len = size * count as usize;
    let fx = Fixture::new(
      format!("rep_scas_{}_{}_{}_{}", size, df, count, prefix),
      scas_code(size, df, count, prefix),
    ).with_scratch(vec![
      MemoryRange { start: DST_ABS, end: DST_ABS + len },
    ]);
    let mut emu = fx.into_emulator().unwrap();
    write_bytes(&mut emu.machine, DST_ABS, data);
    step_emulator(&mut emu, scas_steps());
    emu.machine
  }

  // Assert SI/DI advanced by `size*count` in the direction of DF. Backward
  // pointers start at `len - size` and decrement past the low element, so a
  // full exhaust lands at `base_off - size` (wrapping to 0xFFFF / 0xFFFE).
  fn assert_ptr(base_off: u16, size: usize, df: bool, count: u16, si: u16, di: u16) {
    let len = size * count as usize;
    let expect = if df {
      base_off.wrapping_sub(size as u16)
    } else {
      base_off + len as u16
    };
    assert_eq!(si, expect, "unexpected SI");
    assert_eq!(di, expect, "unexpected DI");
  }

  //////////////////////////////////////////////////////////////////////
  // REP MOVS
  //////////////////////////////////////////////////////////////////////

  #[test]
  fn rep_movsb_fwd_exhaust() {
    let src: Vec<u8> = (0x10..0x10 + COUNT as usize).map(|x| x as u8).collect();
    let m = run_movs(1, false, COUNT, &src);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_ptr(0, 1, false, COUNT, m.reg_read_u16(crate::emu86::cpu::SI), m.reg_read_u16(crate::emu86::cpu::DI));
    assert_eq!(read_bytes(&m, DST_ABS, COUNT as usize), src);
  }

  #[test]
  fn rep_movsb_back_exhaust() {
    let src: Vec<u8> = (0x20..0x20 + COUNT as usize).map(|x| x as u8).collect();
    let m = run_movs(1, true, COUNT, &src);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_ptr(0, 1, true, COUNT, m.reg_read_u16(crate::emu86::cpu::SI), m.reg_read_u16(crate::emu86::cpu::DI));
    assert_eq!(read_bytes(&m, DST_ABS, COUNT as usize), src);
  }

  #[test]
  fn rep_movsw_fwd_exhaust() {
    let src: Vec<u8> = (0x30u16..0x30 + 2 * COUNT as u16).map(|x| x as u8).collect();
    let m = run_movs(2, false, COUNT, &src);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_ptr(0, 2, false, COUNT, m.reg_read_u16(crate::emu86::cpu::SI), m.reg_read_u16(crate::emu86::cpu::DI));
    assert_eq!(read_bytes(&m, DST_ABS, 2 * COUNT as usize), src);
  }

  #[test]
  fn rep_movsw_back_exhaust() {
    let src: Vec<u8> = (0x40u16..0x40 + 2 * COUNT as u16).map(|x| x as u8).collect();
    let m = run_movs(2, true, COUNT, &src);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_ptr(0, 2, true, COUNT, m.reg_read_u16(crate::emu86::cpu::SI), m.reg_read_u16(crate::emu86::cpu::DI));
    assert_eq!(read_bytes(&m, DST_ABS, 2 * COUNT as usize), src);
  }

  #[test]
  fn rep_movsb_cx_zero_no_iteration() {
    let src = vec![0x55; 4];
    let m = run_movs(1, false, 0, &src);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0);
    assert_eq!(read_bytes(&m, DST_ABS, 4), vec![0x00; 4]);
  }

  #[test]
  fn rep_movsw_cx_zero_no_iteration() {
    let src = vec![0x66; 8];
    let m = run_movs(2, false, 0, &src);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0);
    assert_eq!(read_bytes(&m, DST_ABS, 8), vec![0x00; 8]);
  }

  //////////////////////////////////////////////////////////////////////
  // REP STOS
  //////////////////////////////////////////////////////////////////////

  #[test]
  fn rep_stosb_fwd_exhaust() {
    let m = run_stos(1, false, COUNT);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), COUNT);
    assert_eq!(read_bytes(&m, DST_ABS, COUNT as usize), vec![0x5a; COUNT as usize]);
  }

  #[test]
  fn rep_stosb_back_exhaust() {
    let m = run_stos(1, true, COUNT);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xffff);
    assert_eq!(read_bytes(&m, DST_ABS, COUNT as usize), vec![0x5a; COUNT as usize]);
  }

  #[test]
  fn rep_stosw_fwd_exhaust() {
    let m = run_stos(2, false, COUNT);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 2 * COUNT);
    let expect: Vec<u8> = vec![0xcd, 0xab].repeat(COUNT as usize);
    assert_eq!(read_bytes(&m, DST_ABS, 2 * COUNT as usize), expect);
  }

  #[test]
  fn rep_stosw_back_exhaust() {
    let m = run_stos(2, true, COUNT);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xfffe);
    let expect: Vec<u8> = vec![0xcd, 0xab].repeat(COUNT as usize);
    assert_eq!(read_bytes(&m, DST_ABS, 2 * COUNT as usize), expect);
  }

  #[test]
  fn rep_stosb_cx_zero_no_iteration() {
    let m = run_stos(1, false, 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0);
    assert_eq!(read_bytes(&m, DST_ABS, 4), vec![0x00; 4]);
  }

  #[test]
  fn rep_stosw_cx_zero_no_iteration() {
    let m = run_stos(2, true, 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0);
    assert_eq!(read_bytes(&m, DST_ABS, 8), vec![0x00; 8]);
  }

  //////////////////////////////////////////////////////////////////////
  // REPE / REPNE CMPS
  //////////////////////////////////////////////////////////////////////

  #[test]
  fn repe_cmpsb_fwd_exhaust() {
    let src: Vec<u8> = vec![0x11; COUNT as usize];
    let dst: Vec<u8> = vec![0x11; COUNT as usize];
    let m = run_cmps(1, false, COUNT, 0xf3, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), COUNT);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), COUNT);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_cmpsb_fwd_cond_exit() {
    // Mismatch at index 4: REPE stops after comparing index 4 (5 consumed).
    let mut src = vec![0x11; COUNT as usize];
    let dst = vec![0x11; COUNT as usize];
    src[4] = 0x99;
    let m = run_cmps(1, false, COUNT, 0xf3, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 5);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 5);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 5);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_cmpsb_back_exhaust() {
    let src: Vec<u8> = vec![0x22; COUNT as usize];
    let dst: Vec<u8> = vec![0x22; COUNT as usize];
    let m = run_cmps(1, true, COUNT, 0xf3, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 0xffff);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xffff);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_cmpsb_back_cond_exit() {
    // Backward; mismatch at element index 4 from the end (abs index 3).
    let mut src = vec![0x22; COUNT as usize];
    let dst = vec![0x22; COUNT as usize];
    src[3] = 0x77;
    let m = run_cmps(1, true, COUNT, 0xf3, &src, &dst);
    // Elements compared: index 7,6,5,4, then 3 (mismatch) => 5 consumed; start off 7.
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 5);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 8 - 1 - 5);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 8 - 1 - 5);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_cmpsb_fwd_exhaust() {
    // All unequal: REPNE runs to exhaustion, ZF=0.
    let src: Vec<u8> = (0x30u16..0x30 + COUNT as u16).map(|x| x as u8).collect();
    let dst: Vec<u8> = vec![0xff; COUNT as usize];
    let m = run_cmps(1, false, COUNT, 0xf2, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), COUNT);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), COUNT);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_cmpsb_fwd_cond_exit() {
    // Equality at index 3: REPNE stops after comparing index 3 (4 consumed).
    let src = vec![0x40; COUNT as usize];
    let mut dst = vec![0x99; COUNT as usize];
    dst[3] = 0x40;
    let m = run_cmps(1, false, COUNT, 0xf2, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 4);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 4);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 4);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_cmpsb_back_exhaust() {
    let src: Vec<u8> = (0x50u16..0x50 + COUNT as u16).map(|x| x as u8).collect();
    let dst: Vec<u8> = vec![0xee; COUNT as usize];
    let m = run_cmps(1, true, COUNT, 0xf2, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 0xffff);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xffff);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_cmpsb_back_cond_exit() {
    let src = vec![0x60; COUNT as usize];
    let mut dst = vec![0xdd; COUNT as usize];
    dst[5] = 0x60; // equality at abs index 5
    let m = run_cmps(1, true, COUNT, 0xf2, &src, &dst);
    // Backward elements: 7,6,5(equal) => 3 consumed; start off 7.
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 3);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 8 - 1 - 3);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 8 - 1 - 3);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_cmpsw_fwd_exhaust() {
    let src: Vec<u8> = vec![0x12, 0x34].repeat(COUNT as usize);
    let dst: Vec<u8> = vec![0x12, 0x34].repeat(COUNT as usize);
    let m = run_cmps(2, false, COUNT, 0xf3, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 2 * COUNT);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 2 * COUNT);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_cmpsw_fwd_cond_exit() {
    let mut src = vec![0x12, 0x34].repeat(COUNT as usize);
    let dst = vec![0x12, 0x34].repeat(COUNT as usize);
    // Differ word 2 (bytes 4,5): src=0x99,0x99.
    src[4] = 0x99;
    src[5] = 0x99;
    let m = run_cmps(2, false, COUNT, 0xf3, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 3);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 6);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 6);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_cmpsw_back_exhaust() {
    let src: Vec<u8> = vec![0x56, 0x78].repeat(COUNT as usize);
    let dst: Vec<u8> = vec![0x56, 0x78].repeat(COUNT as usize);
    let m = run_cmps(2, true, COUNT, 0xf3, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 0xfffe);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xfffe);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_cmpsw_back_cond_exit() {
    let mut src = vec![0x9a, 0xbc].repeat(COUNT as usize);
    let dst = vec![0x9a, 0xbc].repeat(COUNT as usize);
    // Differ word at abs index 1 (bytes 2,3) from the low end => element 1.
    src[2] = 0x00;
    src[3] = 0x00;
    let m = run_cmps(2, true, COUNT, 0xf3, &src, &dst);
    // Backward elements: 7,6,5,4,3,2,1(mismatch) => 7 consumed; start off 14.
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 7);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 14 - 7 * 2);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 14 - 7 * 2);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_cmpsw_fwd_exhaust() {
    let src: Vec<u8> = vec![0x01, 0x02].repeat(COUNT as usize);
    let dst: Vec<u8> = vec![0xff, 0xff].repeat(COUNT as usize);
    let m = run_cmps(2, false, COUNT, 0xf2, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 2 * COUNT);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 2 * COUNT);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_cmpsw_fwd_cond_exit() {
    let src = vec![0x03, 0x04].repeat(COUNT as usize);
    let mut dst = vec![0xaa, 0xaa].repeat(COUNT as usize);
    // Equality in word 4 (bytes 8,9).
    dst[8] = 0x03;
    dst[9] = 0x04;
    let m = run_cmps(2, false, COUNT, 0xf2, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 5);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 10);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 10);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_cmpsw_back_exhaust() {
    let src: Vec<u8> = vec![0x11, 0x22].repeat(COUNT as usize);
    let dst: Vec<u8> = vec![0x88, 0x88].repeat(COUNT as usize);
    let m = run_cmps(2, true, COUNT, 0xf2, &src, &dst);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 0xfffe);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xfffe);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_cmpsw_back_cond_exit() {
    let src = vec![0x33, 0x44].repeat(COUNT as usize);
    let mut dst = vec![0x77, 0x77].repeat(COUNT as usize);
    // Equality at abs word index 6 (bytes 12,13).
    dst[12] = 0x33;
    dst[13] = 0x44;
    let m = run_cmps(2, true, COUNT, 0xf2, &src, &dst);
    // Backward elements: 7,6(equal) => 2 consumed; start off 14.
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 2);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::SI), 14 - 2 * 2);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 14 - 2 * 2);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  //////////////////////////////////////////////////////////////////////
  // REPE / REPNE SCAS
  //////////////////////////////////////////////////////////////////////

  #[test]
  fn repe_scasb_fwd_exhaust() {
    // Search value 0x42 in a buffer of all 0x42: CX-exhaust, ZF=1.
    let m = run_scas(1, false, COUNT, 0xf3, &vec![0x42; COUNT as usize]);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), COUNT);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_scasb_fwd_cond_exit() {
    // All 0x42 except index 5 (0x99): REPE stops at the mismatch (6 consumed).
    let mut data = vec![0x42; COUNT as usize];
    data[5] = 0x99;
    let m = run_scas(1, false, COUNT, 0xf3, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 6);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 6);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_scasb_back_exhaust() {
    let m = run_scas(1, true, COUNT, 0xf3, &vec![0x42; COUNT as usize]);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xffff);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_scasb_back_cond_exit() {
    // Backward; mismatch at abs index 2. Elements 7,6,5,4,3,2(mismatch)=6 consumed.
    let mut data = vec![0x42; COUNT as usize];
    data[2] = 0x99;
    let m = run_scas(1, true, COUNT, 0xf3, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 6);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 8 - 1 - 6);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_scasb_fwd_exhaust() {
    // Search 0x42 in a buffer with no 0x42: REPNE runs to exhaustion, ZF=0.
    let data: Vec<u8> = vec![0x00; COUNT as usize];
    let m = run_scas(1, false, COUNT, 0xf2, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), COUNT);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_scasb_fwd_cond_exit() {
    // 0x42 at index 3: stops after comparing index 3 (4 consumed).
    let mut data = vec![0x00; COUNT as usize];
    data[3] = 0x42;
    let m = run_scas(1, false, COUNT, 0xf2, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 4);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 4);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_scasb_back_exhaust() {
    let data: Vec<u8> = vec![0x01; COUNT as usize];
    let m = run_scas(1, true, COUNT, 0xf2, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xffff);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_scasb_back_cond_exit() {
    // Backward; 0x42 at abs index 6. Elements 7,6(equal)=2 consumed.
    let mut data = vec![0x01; COUNT as usize];
    data[6] = 0x42;
    let m = run_scas(1, true, COUNT, 0xf2, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 2);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 8 - 1 - 2);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_scasw_fwd_exhaust() {
    // Search value 0x1234 in a buffer of all 0x1234: exhaust, ZF=1.
    let data: Vec<u8> = vec![0x34, 0x12].repeat(COUNT as usize);
    let m = run_scas(2, false, COUNT, 0xf3, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 2 * COUNT);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_scasw_fwd_cond_exit() {
    // All 0x1234 except word 2 (bytes 4,5): stops at mismatch, 3 consumed.
    let mut data = vec![0x34, 0x12].repeat(COUNT as usize);
    data[4] = 0x00;
    data[5] = 0x00;
    let m = run_scas(2, false, COUNT, 0xf3, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 3);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 6);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_scasw_back_exhaust() {
    let data: Vec<u8> = vec![0x34, 0x12].repeat(COUNT as usize);
    let m = run_scas(2, true, COUNT, 0xf3, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xfffe);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repe_scasw_back_cond_exit() {
    // Backward; mismatch at abs word index 1 (bytes 2,3).
    let mut data = vec![0x34, 0x12].repeat(COUNT as usize);
    data[2] = 0x00;
    data[3] = 0x00;
    let m = run_scas(2, true, COUNT, 0xf3, &data);
    // Backward elements: 7,6,5,4,3,2,1(mismatch)=7 consumed; start off 14.
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 7);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 14 - 7 * 2);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_scasw_fwd_exhaust() {
    // Search 0x1234 in a buffer with no 0x1234: exhaust, ZF=0.
    let data: Vec<u8> = vec![0x00, 0x00].repeat(COUNT as usize);
    let m = run_scas(2, false, COUNT, 0xf2, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 2 * COUNT);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_scasw_fwd_cond_exit() {
    // 0x1234 at word 5 (bytes 10,11): stops after comparing it, 6 consumed.
    let mut data = vec![0x00, 0x00].repeat(COUNT as usize);
    data[10] = 0x34;
    data[11] = 0x12;
    let m = run_scas(2, false, COUNT, 0xf2, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 6);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 12);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_scasw_back_exhaust() {
    let data: Vec<u8> = vec![0xaa, 0xaa].repeat(COUNT as usize);
    let m = run_scas(2, true, COUNT, 0xf2, &data);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), 0);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 0xfffe);
    assert!(!m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }

  #[test]
  fn repne_scasw_back_cond_exit() {
    // Backward; 0x1234 at abs word index 6 (bytes 12,13).
    let mut data = vec![0xaa, 0xaa].repeat(COUNT as usize);
    data[12] = 0x34;
    data[13] = 0x12;
    let m = run_scas(2, true, COUNT, 0xf2, &data);
    // Backward elements: 7,6(equal)=2 consumed; start off 14.
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::CX), COUNT - 2);
    assert_eq!(m.reg_read_u16(crate::emu86::cpu::DI), 14 - 2 * 2);
    assert!(m.flag_read(crate::emu86::cpu_flags::FLAG_ZF));
  }
}

/// Register/segment mutation corpus with host-side (emu86-only) tests.
///
/// Each fixture is a real-mode MZ program built from a deterministic, seeded
/// pseudo-random sequence of register/segment mutation instructions: the
/// general ALU (ADD/SUB/AND/OR/XOR/CMP/INC/DEC), register MOV, XCHG (both the
/// ModRM and the short `xchg ax,r16` forms), PUSH/POP, sreg MOV (segment
/// mutation), and a segment-override memory store/load through `ES:[bx]`. Every
/// instruction is a single `Machine::step()` boundary, so an individual fixture
/// contributes a few hundred stepping boundaries.
///
/// The expected final state is NOT read back from emu86; it is computed
/// independently by a small symbolic model (`Model`) that replays the same
/// deterministic `Op` stream over the loader's initial register state. The test
/// asserts emu86's final registers/segments/memory equal that model's
/// prediction, establishing emu86's deterministic behavior as the reference for
/// the later differential (dosemu2) phase. `CMP` is exercised for its stepping
/// boundary but has no register effect, so the model ignores it; flags are not
/// modeled bit-by-bit (that would merely re-express `alu.rs`), only the strong
/// register/segment/SP/memory invariants are asserted.
#[cfg(test)]
mod mutation_fixtures {
  use super::*;
  use crate::emu86::cpu;
  use crate::emu86::emu::Emulator;
  use std::collections::HashMap;

  // Scratch window for segment-override memory ops: ES always points here
  // (absolute 0x60000..0x61000), and memory-op BX offsets are normalized into
  // [0, 0x1000) so every ES:[bx] access lands inside the declared range.
  const SCRATCH_SEG: u16 = 0x6000;
  const SCRATCH_ABS: usize = (SCRATCH_SEG as usize) << 4;
  const SCRATCH_LEN: usize = 0x1000;

  // ModRM 3-bit register-field encoding order for real-mode 16-bit registers:
  // AX=0, CX=1, DX=2, BX=3, SP=4, BP=5, SI=6, DI=7.
  const R_AX: usize = 0;
  const R_CX: usize = 1;
  const R_DX: usize = 2;
  const R_BX: usize = 3;
  const R_SP: usize = 4;
  const R_BP: usize = 5;
  const R_SI: usize = 6;
  const R_DI: usize = 7;

  // ModRM sreg-field encoding order (sreg16): ES=0, CS=1, SS=2, DS=3.
  const S_ES: usize = 0;
  const S_DS: usize = 3;

  /// Deterministic LCG used to seed the corpus. A fixed seed always yields the
  /// same op stream, keeping every fixture reproducible.
  struct Prng(u64);
  impl Prng {
    fn new(seed: u64) -> Self { Self(seed) }
    fn next_u16(&mut self) -> u16 {
      self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
      (self.0 >> 16) as u16
    }
  }

  /// A single mutation instruction in symbolic form. The byte encoder and the
  /// independent `Model` both consume the same `Op`, so the fixture bytes and
  /// the expected state are consistent by construction, while emu86 executes a
  /// separate decode/step path that must agree with the model.
  #[derive(Debug, Clone, Copy)]
  enum Op {
    Add(usize, usize),
    Sub(usize, usize),
    Xor(usize, usize),
    And(usize, usize),
    Or(usize, usize),
    Mov(usize, usize),
    Xchg(usize, usize),
    XchgAx(usize),
    Inc(usize),
    Dec(usize),
    AddImm(usize, u16),
    MovImm(usize, u16),
    PushReg(usize),
    PopReg(usize),
    PushImm(u16),
    Cmp(usize, usize),
    MovSreg(usize, usize), // mov sreg, r16
    MovMemAX,              // mov word ptr es:[bx], ax
    MovAXMem,              // mov ax, word ptr es:[bx]
  }

  fn imm16(v: u16) -> [u8; 2] {
    [v as u8, (v >> 8) as u8]
  }

  // mod=11 (register) ModRM with reg field `reg` and rm field `rm`.
  fn modrm_rr(reg: usize, rm: usize) -> u8 {
    0xC0 | ((reg as u8) << 3) | (rm as u8)
  }

  fn encode(op: &Op) -> Vec<u8> {
    use Op::*;
    match *op {
      Add(rd, rs) => vec![0x03, modrm_rr(rd, rs)],
      Sub(rd, rs) => vec![0x2B, modrm_rr(rd, rs)],
      Xor(rd, rs) => vec![0x33, modrm_rr(rd, rs)],
      And(rd, rs) => vec![0x23, modrm_rr(rd, rs)],
      Or(rd, rs) => vec![0x0B, modrm_rr(rd, rs)],
      Mov(rd, rs) => vec![0x8B, modrm_rr(rd, rs)],
      Xchg(rd, rs) => vec![0x87, modrm_rr(rd, rs)],
      XchgAx(r) => vec![0x90 + r as u8], // 0x91..=0x97 xchg ax, cx..di
      Inc(rd) => vec![0x40 + rd as u8],
      Dec(rd) => vec![0x48 + rd as u8],
      // 0x81 /0 add r/m16, imm16; reg field 0 selects ADD.
      AddImm(rd, v) => {
        let mut b = vec![0x81, modrm_rr(0, rd)];
        b.extend_from_slice(&imm16(v));
        b
      }
      MovImm(rd, v) => {
        let mut b = vec![0xB8 + rd as u8];
        b.extend_from_slice(&imm16(v));
        b
      }
      PushReg(r) => vec![0x50 + r as u8],
      PopReg(r) => vec![0x58 + r as u8],
      PushImm(v) => {
        let mut b = vec![0x68];
        b.extend_from_slice(&imm16(v));
        b
      }
      Cmp(rd, rs) => vec![0x3B, modrm_rr(rd, rs)],
      // 0x8E /r mov sreg, r/m16; sreg code in the reg field.
      MovSreg(s, r) => vec![0x8E, modrm_rr(s, r)],
      // mov word ptr es:[bx], ax / mov ax, word ptr es:[bx]:
      // ModRM 0x07 = mod 00, reg AX=0, rm 111 = [bx], with an ES (0x26) override.
      MovMemAX => vec![0x26, 0x89, 0x07],
      MovAXMem => vec![0x26, 0x8B, 0x07],
    }
  }

  /// Independent model of the mutation instruction effects, replayed over the
  /// loader's initial register state to predict emu86's final state.
  #[derive(Debug, Clone, Default)]
  struct Model {
    regs: [u16; 8],       // AX,CX,DX,BX,SP,BP,SI,DI
    sreg: [u16; 4],       // ES,CS,SS,DS
    stack: Vec<u16>,
    mem: HashMap<u32, u16>, // absolute linear address -> stored word
  }

  // Mirror the loader's initial state (default psp_segment 0x0813):
  // DS=ES=0x0813, CS=SS=code_seg=0x0823, SP=0x200, all general regs 0.
  fn model_init() -> Model {
    let mut m = Model::default();
    m.sreg[0] = 0x0813; // ES
    m.sreg[1] = 0x0823; // CS
    m.sreg[2] = 0x0823; // SS
    m.sreg[3] = 0x0813; // DS
    m
  }

  fn mem_addr(m: &Model) -> u32 {
    ((m.sreg[S_ES] as u32) << 4) | (m.regs[R_BX] as u32)
  }

  fn apply(m: &mut Model, op: &Op) {
    use Op::*;
    match *op {
      Add(rd, rs) => m.regs[rd] = m.regs[rd].wrapping_add(m.regs[rs]),
      Sub(rd, rs) => m.regs[rd] = m.regs[rd].wrapping_sub(m.regs[rs]),
      Xor(rd, rs) => m.regs[rd] ^= m.regs[rs],
      And(rd, rs) => m.regs[rd] &= m.regs[rs],
      Or(rd, rs) => m.regs[rd] |= m.regs[rs],
      Mov(rd, rs) => m.regs[rd] = m.regs[rs],
      Xchg(rd, rs) => {
        let t = m.regs[rd];
        m.regs[rd] = m.regs[rs];
        m.regs[rs] = t;
      }
      XchgAx(r) => {
        let t = m.regs[R_AX];
        m.regs[R_AX] = m.regs[r];
        m.regs[r] = t;
      }
      Inc(rd) => m.regs[rd] = m.regs[rd].wrapping_add(1),
      Dec(rd) => m.regs[rd] = m.regs[rd].wrapping_sub(1),
      AddImm(rd, v) => m.regs[rd] = m.regs[rd].wrapping_add(v),
      MovImm(rd, v) => m.regs[rd] = v,
      PushReg(r) => {
        m.stack.push(m.regs[r]);
        m.regs[R_SP] = m.regs[R_SP].wrapping_sub(2);
      }
      PopReg(r) => {
        let v = m.stack.pop().expect("model pop underflow");
        m.regs[r] = v;
        m.regs[R_SP] = m.regs[R_SP].wrapping_add(2);
      }
      PushImm(v) => {
        m.stack.push(v);
        m.regs[R_SP] = m.regs[R_SP].wrapping_sub(2);
      }
      Cmp(..) => {}
      MovSreg(s, r) => m.sreg[s] = m.regs[r],
      MovMemAX => {
        m.mem.insert(mem_addr(m), m.regs[R_AX]);
      }
      MovAXMem => {
        let v = *m.mem.get(&mem_addr(m)).unwrap_or(&0);
        m.regs[R_AX] = v;
      }
    }
  }

  fn replay(ops: &[Op]) -> Model {
    let mut m = model_init();
    for op in ops {
      apply(&mut m, op);
    }
    m
  }

  // Samples a general register EXCLUDING SP: SP is reserved for PUSH/POP and
  // init only, so random ALU/MOV/XCHG mutations can never drive it to the 0/1
  // boundary where emu86's `SP -= 2`/`SP += 2` debug arithmetic would overflow.
  fn rng_reg(rng: &mut Prng) -> usize {
    const POOL: [usize; 7] = [R_AX, R_CX, R_DX, R_BX, R_BP, R_SI, R_DI];
    POOL[(rng.next_u16() % 7) as usize]
  }

  /// Builds a deterministic mutation stream: seeded register initialization,
  /// then `len` seeded mutation instructions.
  fn gen_ops(seed: u64, len: usize) -> Vec<Op> {
    let mut rng = Prng::new(seed);
    let mut ops = Vec::new();
    // Register initialization with seeded pseudo-random 16-bit values.
    for rd in 0..8 {
      ops.push(Op::MovImm(rd, rng.next_u16()));
    }
    // Park SP away from the code image so the downward-growing stack can never
    // overwrite not-yet-executed instruction bytes.
    ops.push(Op::MovImm(R_SP, 0x2000));
    // Point ES at the scratch window for the segment-override memory ops.
    ops.push(Op::MovImm(R_AX, SCRATCH_SEG));
    ops.push(Op::MovSreg(S_ES, R_AX));

    // Tracks live stack depth so PUSH/POP stay balanced (a POP on the model's
    // empty stack would underflow, and emu86 would read undefined bytes).
    let mut depth = 0usize;
    for _ in 0..len {
      let k = rng.next_u16() % 16;
      match k {
        0 => ops.push(Op::Add(rng_reg(&mut rng), rng_reg(&mut rng))),
        1 => ops.push(Op::Sub(rng_reg(&mut rng), rng_reg(&mut rng))),
        2 => ops.push(Op::Xor(rng_reg(&mut rng), rng_reg(&mut rng))),
        3 => ops.push(Op::And(rng_reg(&mut rng), rng_reg(&mut rng))),
        4 => ops.push(Op::Or(rng_reg(&mut rng), rng_reg(&mut rng))),
        5 => ops.push(Op::Mov(rng_reg(&mut rng), rng_reg(&mut rng))),
        6 => {
          let rd = rng_reg(&mut rng);
          let rs = rng_reg(&mut rng);
          if rd != rs {
            ops.push(Op::Xchg(rd, rs));
          } else {
            ops.push(Op::Inc(rd));
          }
        }
        // xchg ax, r16 short form for a non-AX/non-SP register (0x91..0x97).
        7 => {
          const REGS: [usize; 6] = [R_CX, R_DX, R_BX, R_BP, R_SI, R_DI];
          ops.push(Op::XchgAx(REGS[(rng.next_u16() % 6) as usize]));
        }
        8 => ops.push(Op::Inc(rng_reg(&mut rng))),
        9 => ops.push(Op::Dec(rng_reg(&mut rng))),
        10 => ops.push(Op::AddImm(rng_reg(&mut rng), rng.next_u16())),
        11 => {
          if rng.next_u16() & 1 == 0 {
            ops.push(Op::PushImm(rng.next_u16()));
          } else {
            ops.push(Op::PushReg(rng_reg(&mut rng)));
          }
          depth += 1;
        }
        12 => {
          if depth > 0 {
            ops.push(Op::PopReg(rng_reg(&mut rng)));
            depth -= 1;
          } else {
            ops.push(Op::MovImm(R_AX, rng.next_u16()));
          }
        }
        13 => ops.push(Op::Cmp(rng_reg(&mut rng), rng_reg(&mut rng))),
        14 => ops.push(Op::MovSreg(S_DS, rng_reg(&mut rng))),
        15 => {
          // Segment-override memory op: normalize BX into the scratch window,
          // then store/load AX through ES:[bx].
          ops.push(Op::MovImm(R_BX, rng.next_u16() % SCRATCH_LEN as u16));
          if rng.next_u16() & 1 == 0 {
            ops.push(Op::MovMemAX);
          } else {
            ops.push(Op::MovAXMem);
          }
        }
        _ => unreachable!(),
      }
    }
    ops
  }

  fn code_from_ops(ops: &[Op]) -> Vec<u8> {
    let mut code = Vec::new();
    for op in ops {
      code.extend_from_slice(&encode(op));
    }
    code
  }

  fn step_emulator(emu: &mut Emulator, n: usize) {
    for _ in 0..n {
      emu.step().unwrap();
    }
  }

  /// Builds, loads, and steps a seeded corpus fixture, returning the independent
  /// model prediction and the stepped emulator for assertion.
  fn run_corpus(seed: u64, len: usize) -> (Model, Emulator) {
    let ops = gen_ops(seed, len);
    let fx = Fixture::new(format!("mut_corpus_{}_{}", seed, len), code_from_ops(&ops))
      .with_scratch(vec![MemoryRange { start: SCRATCH_ABS, end: SCRATCH_ABS + SCRATCH_LEN }]);
    let mut emu = fx.into_emulator().unwrap();
    step_emulator(&mut emu, ops.len());
    let model = replay(&ops);
    (model, emu)
  }

  /// Asserts emu86's final general registers, segments, and override-written
  /// memory all match the independent model's prediction.
  fn assert_model(m: &Model, emu: &Emulator) {
    assert_eq!(emu.machine.reg_read_u16(cpu::AX), m.regs[R_AX], "AX");
    assert_eq!(emu.machine.reg_read_u16(cpu::CX), m.regs[R_CX], "CX");
    assert_eq!(emu.machine.reg_read_u16(cpu::DX), m.regs[R_DX], "DX");
    assert_eq!(emu.machine.reg_read_u16(cpu::BX), m.regs[R_BX], "BX");
    assert_eq!(emu.machine.reg_read_u16(cpu::SP), m.regs[R_SP], "SP");
    assert_eq!(emu.machine.reg_read_u16(cpu::BP), m.regs[R_BP], "BP");
    assert_eq!(emu.machine.reg_read_u16(cpu::SI), m.regs[R_SI], "SI");
    assert_eq!(emu.machine.reg_read_u16(cpu::DI), m.regs[R_DI], "DI");
    assert_eq!(emu.machine.reg_read_u16(cpu::ES), m.sreg[0], "ES");
    assert_eq!(emu.machine.reg_read_u16(cpu::CS), m.sreg[1], "CS");
    assert_eq!(emu.machine.reg_read_u16(cpu::SS), m.sreg[2], "SS");
    assert_eq!(emu.machine.reg_read_u16(cpu::DS), m.sreg[3], "DS");
    for (&addr, &val) in &m.mem {
      let abs = addr as usize;
      let lo = emu.machine.mem.read_u8(seg_off_from_abs(abs)) as u16;
      let hi = emu.machine.mem.read_u8(seg_off_from_abs(abs + 1)) as u16;
      assert_eq!((hi << 8) | lo, val, "mem@{:#x}", abs);
    }
  }

  //////////////////////////////////////////////////////////////////////
  // Seeded mutation-corpus fixtures (one per (seed, length)).
  //////////////////////////////////////////////////////////////////////

  #[test]
  fn corpus_seed_1_len_200() {
    let (m, emu) = run_corpus(1, 200);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_2_len_180() {
    let (m, emu) = run_corpus(2, 180);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_3_len_240() {
    let (m, emu) = run_corpus(3, 240);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_4_len_160() {
    let (m, emu) = run_corpus(4, 160);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_5_len_220() {
    let (m, emu) = run_corpus(5, 220);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_6_len_280() {
    let (m, emu) = run_corpus(6, 280);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_7_len_140() {
    let (m, emu) = run_corpus(7, 140);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_8_len_260() {
    let (m, emu) = run_corpus(8, 260);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_9_len_120() {
    let (m, emu) = run_corpus(9, 120);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_10_len_300() {
    let (m, emu) = run_corpus(10, 300);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_42_len_256() {
    let (m, emu) = run_corpus(42, 256);
    assert_model(&m, &emu);
  }

  #[test]
  fn corpus_seed_1337_len_232() {
    let (m, emu) = run_corpus(1337, 232);
    assert_model(&m, &emu);
  }

  //////////////////////////////////////////////////////////////////////
  // Dedicated segment-override memory fixture.
  //////////////////////////////////////////////////////////////////////

  /// Hand-written program exercising ES/DS/CS segment-override memory operands
  /// at known absolute addresses, asserting the resulting bytes.
  #[test]
  fn segment_override_memory() {
    // Absolute target for ES/DS stores (ES=DS=SCRATCH_SEG, offset 0x0100).
    let abs = SCRATCH_ABS + 0x0100;
    // CS-override target: an offset past the (small) code image in CS's own
    // 64K window, which the program can freely overwrite.
    let cs_abs = 0x08230 + 0x0600;

    let code: Vec<u8> = vec![
      0xB8, 0x00, 0x60, // mov ax, 0x6000
      0x8E, 0xC0, // mov es, ax
      0x8E, 0xD8, // mov ds, ax
      0xBB, 0x00, 0x01, // mov bx, 0x0100
      0xB8, 0xFE, 0xCA, // mov ax, 0xCAFE
      0x26, 0x89, 0x07, // mov word ptr es:[bx], ax
      0xB9, 0xEF, 0xBE, // mov cx, 0xBEEF
      0x3E, 0x89, 0x0F, // mov word ptr ds:[bx], cx
      0x26, 0x8B, 0x17, // mov dx, word ptr es:[bx]
      0xB8, 0x57, 0x13, // mov ax, 0x1357
      0x2E, 0x89, 0x06, 0x00, 0x06, // mov word ptr cs:[0x0600], ax
      0x3E, 0x8B, 0x37, // mov si, word ptr ds:[bx]
    ];

    let fx = Fixture::new("seg_override_mem", code)
      .with_scratch(vec![MemoryRange { start: SCRATCH_ABS, end: SCRATCH_ABS + SCRATCH_LEN }]);
    let mut emu = fx.into_emulator().unwrap();
    step_emulator(&mut emu, 12);

    // ES:[bx] then DS:[bx] both wrote to abs; the DS store (BEEF) is last.
    let lo = emu.machine.mem.read_u8(seg_off_from_abs(abs)) as u16;
    let hi = emu.machine.mem.read_u8(seg_off_from_abs(abs + 1)) as u16;
    assert_eq!((hi << 8) | lo, 0xBEEF, "scratch word");
    // ES load and DS load both fetched 0xBEEF.
    assert_eq!(emu.machine.reg_read_u16(cpu::DX), 0xBEEF);
    assert_eq!(emu.machine.reg_read_u16(cpu::SI), 0xBEEF);
    // CS override store landed at cs_abs.
    let clo = emu.machine.mem.read_u8(seg_off_from_abs(cs_abs)) as u16;
    let chi = emu.machine.mem.read_u8(seg_off_from_abs(cs_abs + 1)) as u16;
    assert_eq!((chi << 8) | clo, 0x1357, "cs word");
  }

  //////////////////////////////////////////////////////////////////////
  // Dedicated stack-balance fixture.
  //////////////////////////////////////////////////////////////////////

  /// Pushes three values and pops them into three registers, asserting both the
  /// popped values and that SP returns to its starting point (net change zero).
  #[test]
  fn stack_balance_push_pop() {
    let code: Vec<u8> = vec![
      0xBC, 0x00, 0x20, // mov sp, 0x2000
      0xB8, 0x11, 0x11, // mov ax, 0x1111
      0xBB, 0x22, 0x22, // mov bx, 0x2222
      0x50, // push ax
      0x53, // push bx
      0x68, 0x33, 0x33, // push 0x3333
      0x59, // pop cx
      0x5A, // pop dx
      0x5E, // pop si
    ];
    let fx = Fixture::new("stack_balance", code);
    let mut emu = fx.into_emulator().unwrap();
    step_emulator(&mut emu, 9);

    assert_eq!(emu.machine.reg_read_u16(cpu::SP), 0x2000, "SP net change");
    assert_eq!(emu.machine.reg_read_u16(cpu::CX), 0x3333);
    assert_eq!(emu.machine.reg_read_u16(cpu::DX), 0x2222);
    assert_eq!(emu.machine.reg_read_u16(cpu::SI), 0x1111);
  }

  //////////////////////////////////////////////////////////////////////
  // Dedicated segment-mutation fixture.
  //////////////////////////////////////////////////////////////////////

  /// mutates ES and DS through `mov sreg, r16` and asserts the resulting
  /// segment register values.
  #[test]
  fn segment_mutation_sreg_mov() {
    let code: Vec<u8> = vec![
      0xB8, 0x11, 0x11, // mov ax, 0x1111
      0xBB, 0x22, 0x22, // mov bx, 0x2222
      0xB9, 0x33, 0x33, // mov cx, 0x3333
      0x8E, 0xC0, // mov es, ax
      0x8E, 0xDB, // mov ds, bx
      0x8E, 0xC1, // mov es, cx
    ];
    let fx = Fixture::new("seg_mutation", code);
    let mut emu = fx.into_emulator().unwrap();
    step_emulator(&mut emu, 6);

    assert_eq!(emu.machine.reg_read_u16(cpu::ES), 0x3333);
    assert_eq!(emu.machine.reg_read_u16(cpu::DS), 0x2222);
  }
}
