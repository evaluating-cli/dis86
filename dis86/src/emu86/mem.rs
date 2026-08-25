use crate::emu86::dos_structs::ProgramSegmentPrefix;
pub use crate::segoff::{Seg, SegOff};
use std::collections::HashMap;

// Large enough to allow address ffff:ffff
pub const MEM_SIZE: usize = 0x10fff0;

/// Flat emu86 memory plus optional lightweight write tracking.
///
/// The tracker stores the value that each address had before its first write
/// after tracking was enabled. This lets the SST runner detect undeclared final
/// memory changes without cloning the entire ~1.1 MiB memory image per test.
pub struct Memory(pub Vec<u8>, Option<HashMap<usize, u8>>);

impl Default for Memory {
  fn default() -> Self { Self::new() }
}

impl Memory {
  pub fn new() -> Memory {
    let mut raw = vec![];
    raw.resize(MEM_SIZE, 0);
    Memory(raw, None)
  }

  /// Start recording the pre-write value of each address touched from now on.
  pub fn begin_write_tracking(&mut self) {
    self.1 = Some(HashMap::new());
  }

  /// Addresses touched since [`begin_write_tracking`], mapped to the value they
  /// held before their first write. Callers compare those originals with the
  /// current memory image so write-then-restore sequences are not false positives.
  pub fn tracked_write_originals(&self) -> Option<&HashMap<usize, u8>> {
    self.1.as_ref()
  }

  fn write_abs_u8(&mut self, idx: usize, val: u8) {
    let before = self.0[idx];
    if let Some(writes) = self.1.as_mut() {
      writes.entry(idx).or_insert(before);
    }
    self.0[idx] = val;
  }

  pub fn asciiz(&self, addr: SegOff) -> &str {
    let slice = self.slice_starting_at(addr);
    let cstr = unsafe { std::ffi::CStr::from_ptr(slice.as_ptr() as *const std::ffi::c_char) };
    cstr.to_str().unwrap()
  }

  pub fn read_u8(&self, addr: SegOff) -> u8  {
    self.0[addr.abs_normal()]
  }

  // Multi-byte accesses wrap the EA offset at the 64KB segment boundary
  // (SST-D-007): byte i lives at seg:(off + i) mod 0x10000, not at the linear
  // continuation. This matches the pinned Harris 80C286 captures (a far
  // pointer straddling off 0xFFFE loads its CS word from the wrapped
  // seg:0x0000); note a stock Intel 80286 would instead fault #GP(0) on a word
  // operand at offset 0xFFFF — emu86 follows the Harris anchor.
  pub fn read_u16(&self, addr: SegOff) -> u16 {
    let base = addr.seg.unwrap_normal() as usize * 16;
    let off = addr.off.0 as usize;
    u16::from_le_bytes([
      self.0[base + off],
      self.0[base + ((off + 1) & 0xffff)],
    ])
  }

  pub fn read_u32(&self, addr: SegOff) -> u32 {
    let base = addr.seg.unwrap_normal() as usize * 16;
    let off = addr.off.0 as usize;
    u32::from_le_bytes([
      self.0[base + off],
      self.0[base + ((off + 1) & 0xffff)],
      self.0[base + ((off + 2) & 0xffff)],
      self.0[base + ((off + 3) & 0xffff)],
    ])
  }

  pub fn write_u8(&mut self, addr: SegOff, val: u8) {
    self.write_abs_u8(addr.abs_normal(), val);
  }

  pub fn write_u16(&mut self, addr: SegOff, val: u16) {
    let base = addr.seg.unwrap_normal() as usize * 16;
    let off = addr.off.0 as usize;
    let bytes = val.to_le_bytes();
    self.write_abs_u8(base + off, bytes[0]);
    self.write_abs_u8(base + ((off + 1) & 0xffff), bytes[1]);
  }

  pub fn write_u32(&mut self, addr: SegOff, val: u32) {
    let base = addr.seg.unwrap_normal() as usize * 16;
    let off = addr.off.0 as usize;
    let bytes = val.to_le_bytes();
    self.write_abs_u8(base + off, bytes[0]);
    self.write_abs_u8(base + ((off + 1) & 0xffff), bytes[1]);
    self.write_abs_u8(base + ((off + 2) & 0xffff), bytes[2]);
    self.write_abs_u8(base + ((off + 3) & 0xffff), bytes[3]);
  }

  pub fn slice_starting_at(&self, addr: SegOff) -> &[u8] {
    &self.0[addr.abs_normal()..]
  }

  pub fn slice_mut_starting_at(&mut self, addr: SegOff) -> &mut [u8] {
    &mut self.0[addr.abs_normal()..]
  }

  pub fn program_segment_prefix_mut(&mut self, psp_segment: u16) -> &mut ProgramSegmentPrefix {
    let off = Seg::Normal(psp_segment).abs_normal();
    let slice = &mut self.0[off..off+std::mem::size_of::<ProgramSegmentPrefix>()];
    unsafe { &mut *(slice.as_mut_ptr() as *mut ProgramSegmentPrefix) }
  }

  // pub fn program_segment_prefix(&self) -> &ProgramSegmentPrefix {
  //   let off = Seg::Normal(psp_segment).abs_normal();
  //   let slice = &self.0[off..off+256];
  //   ProgramSegmentPrefix::from_slice(slice)
  // }
}