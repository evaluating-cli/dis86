pub use super::value::*;
pub use super::mem::*;
pub use super::cpu::*;
pub use super::cpu_flags::*;
pub use super::video::*;
pub use super::adlib::*;
pub use super::alu;

pub use super::dos::Dos;
pub use crate::segoff:: SegOff;

pub struct Machine {
  pub halted: bool,
  pub mem: Memory,
  pub cpu: Cpu,
  pub dos: Dos,
  pub interrupt_vectors: [Option<SegOff>; 256],
  pub video: Video,
  pub adlib: Adlib,
  pub exec_count: u64,
  pub psp_segment: u16,
}

impl Machine {
  pub fn new(root_dir: Option<&str>) -> Machine {
    Self::new_with_psp_segment(root_dir, 0x0813)
  }

  pub fn new_with_psp_segment(root_dir: Option<&str>, psp_segment: u16) -> Machine {
    let mut mem = Memory::default();
    let dos = Dos::new(root_dir, &mut mem);

    Machine {
      halted: false,
      mem,
      cpu: Cpu::default(),
      dos,
      interrupt_vectors: [None; 256],
      video: Video::new(),
      adlib: Adlib::new(),
      exec_count: 0,
      psp_segment,
    }
  }

  pub fn halted(&self) -> bool {
    self.halted
  }

  pub fn instr_addr(&self) -> SegOff {
    SegOff::new(self.reg_read_u16(CS), self.reg_read_u16(IP))
  }

  pub fn stack_push(&mut self, val: Value) {
    self.stack_push_u16(val.unwrap_u16());
  }

  pub fn stack_pop(&mut self) -> Value {
    Value::U16(self.stack_pop_u16())
  }

  pub fn stack_push_u16(&mut self, val: u16) {
    let mut addr = self.reg_read_addr(SS, SP);
    addr.off.0 = addr.off.0.wrapping_sub(2);

    self.reg_write_u16(SP, addr.off.0);

    self.mem.write_u16(addr, val);
  }

  pub fn stack_pop_u16(&mut self) -> u16 {
    let addr = self.reg_read_addr(SS, SP);
    let val = self.mem.read_u16(addr);

    self.reg_write_u16(SP, addr.off.0.wrapping_add(2));

    val
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn machine_with_sp(sp: u16) -> Machine {
    let mut m = Machine::new(None);
    m.reg_write_u16(SS, 0x2000);
    m.reg_write_u16(SP, sp);
    m
  }

  fn seg_off(seg: u16, off: u16) -> SegOff {
    SegOff::new(seg, off)
  }

  #[test]
  fn stack_push_wraps_sp_at_zero() {
    // SP = 0: the 80C286 wraps to 0xFFFE instead of trapping; the value lands
    // at SS:0xFFFE.
    let mut m = machine_with_sp(0x0000);
    m.stack_push_u16(0x1234);
    assert_eq!(m.reg_read_u16(SP), 0xFFFE);
    assert_eq!(m.mem.read_u16(seg_off(0x2000, 0xFFFE)), 0x1234);
  }

  #[test]
  fn stack_pop_wraps_sp_at_max() {
    // SP = 0xFFFE: the 80C286 wraps to 0x0000 instead of trapping; the value
    // read comes from SS:0xFFFE.
    let mut m = machine_with_sp(0xFFFE);
    m.mem.write_u16(seg_off(0x2000, 0xFFFE), 0x5678);
    assert_eq!(m.stack_pop_u16(), 0x5678);
    assert_eq!(m.reg_read_u16(SP), 0x0000);
  }

  #[test]
  fn stack_push_under_sp_gt_zero() {
    let mut m = machine_with_sp(0x0100);
    m.stack_push_u16(0x1234);
    assert_eq!(m.reg_read_u16(SP), 0x00FE);
    assert_eq!(m.mem.read_u16(seg_off(0x2000, 0x00FE)), 0x1234);
  }

  #[test]
  fn stack_pop_over_sp_lt_max() {
    let mut m = machine_with_sp(0x0100);
    m.mem.write_u16(seg_off(0x2000, 0x0100), 0x5678);
    assert_eq!(m.stack_pop_u16(), 0x5678);
    assert_eq!(m.reg_read_u16(SP), 0x0102);
  }
}
