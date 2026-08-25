use super::machine::*;
use crate::asm::instr::{self, Instr, Opcode, Operand};

impl Machine {
  pub fn opcode_lods(&mut self, instr: &Instr) -> Result<(), String> {
    assert_eq!(instr.opcode, Opcode::OP_LODS);

    let Operand::Mem(mem) = instr.operands[1] else { panic!("Expected memory operand") };
    let size = match mem.sz {
      instr::Size::Size8  => 1,
      instr::Size::Size16 => 2,
      _ => panic!("unsupported size"),
    };

    let dir = self.flag_read(FLAG_DF);
    let inc = if !dir { size } else { (-(size as i16)) as u16 };
    let rep = instr.rep;

    let mut count = if rep.is_some() {
      self.reg_read_u16(CX)
    } else {
      1
    };

    while count != 0 {
      let value = self.operand_read(&instr, 1);
      self.operand_write(&instr, 0, value);

      let si = self.reg_read_u16(SI);
      self.reg_write_u16(SI, si.wrapping_add(inc));

      count -= 1;
    }

    if rep.is_some() {
      self.reg_write_u16(CX, count);
    }

    Ok(())
  }
}

#[cfg(test)]
mod test {
  use super::*;

  fn mem_write_slice(m: &mut Machine, addr: SegOff, data: &[u8]) {
    for i in 0..data.len() {
      m.mem.write_u8(addr.add_offset(i as u16), data[i]);
    }
  }

  fn run_impl(data: &[u8], code: &[u8], df: u8, si: u16, cx: u16) -> (u16, u16, u16) {
    let mut m = Machine::new(None);
    let code_addr = SegOff::new(0x0000, 0x0000);
    mem_write_slice(&mut m, code_addr, code);
    let data_addr = SegOff::new(0x1000, 0x0000);
    mem_write_slice(&mut m, data_addr, data);
    m.reg_write_addr(DS, SI, data_addr);
    m.flag_write(FLAG_DF, df != 0);
    m.reg_write_u16(SI, si);
    m.reg_write_u16(CX, cx);
    m.step().unwrap();
    (m.reg_read_u16(SI), m.reg_read_u16(CX), m.reg_read_u16(AX))
  }

  #[test]
  fn lodsb_single_advances_si_by_1() {
    let (si, _cx, ax) = run_impl(&[0x42, 0x99], &[0xac], 0, 0, 0);
    assert_eq!(si, 1);
    assert_eq!(ax & 0xff, 0x42);
  }

  #[test]
  fn rep_lodsb_loops_cx_times_and_keeps_last_byte() {
    // rep lodsb with CX=4: SI advances 4, AL = last byte, CX=0.
    let (si, cx, ax) = run_impl(&[0x10, 0x20, 0x30, 0x40, 0x50], &[0xf3, 0xac], 0, 0, 4);
    assert_eq!(si, 4);
    assert_eq!(cx, 0);
    assert_eq!(ax & 0xff, 0x40);
  }

  #[test]
  fn rep_lodsb_backward_decrements_si() {
    // DF=1, rep lodsb with CX=3 starting at SI=3: SI -> 0.
    let (si, cx, _ax) = run_impl(&[0x10, 0x20, 0x30, 0x40], &[0xf3, 0xac], 1, 3, 3);
    assert_eq!(si, 0);
    assert_eq!(cx, 0);
  }

  #[test]
  fn repne_lodsb_same_as_rep_lodsb() {
    // LODS sets no flags, so REPNE and REP behave identically (loop CX times).
    let (si, cx, ax) = run_impl(&[0x10, 0x20, 0x30], &[0xf2, 0xac], 0, 0, 3);
    assert_eq!(si, 3);
    assert_eq!(cx, 0);
    assert_eq!(ax & 0xff, 0x30);
  }

  #[test]
  fn rep_lodsb_cx_zero_no_iteration() {
    let (si, cx, _ax) = run_impl(&[0x10], &[0xf3, 0xac], 0, 0, 0);
    assert_eq!(si, 0);
    assert_eq!(cx, 0);
  }

  #[test]
  fn rep_lodsw_loops_cx_times() {
    // rep lodsw with CX=2: SI advances 4, AX = last word (0x4030).
    let (si, cx, ax) = run_impl(&[0x10, 0x20, 0x30, 0x40], &[0xf3, 0xad], 0, 0, 2);
    assert_eq!(si, 4);
    assert_eq!(cx, 0);
    assert_eq!(ax, 0x4030);
  }
}

