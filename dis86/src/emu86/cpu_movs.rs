use super::machine::*;
use crate::asm::instr::{self, Instr, Opcode, Operand};

impl Machine {
  pub fn opcode_movs(&mut self, instr: &Instr) -> Result<(), String> {
    assert_eq!(instr.opcode, Opcode::OP_MOVS);

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
      let rhs = self.operand_read(&instr, 1);
      self.operand_write(&instr, 0, rhs);

      let si = self.reg_read_u16(SI);
      self.reg_write_u16(SI, si.wrapping_add(inc));

      let di = self.reg_read_u16(DI);
      self.reg_write_u16(DI, di.wrapping_add(inc));

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

  /// SST-D-013: a segment-override prefix on MOVSB must override only the
  /// SOURCE segment (DS:SI); the DESTINATION is always ES:DI. Before the fix
  /// the decoder applied the override to the dest too, writing to DS:DI.
  #[test]
  fn ds_override_movsb_writes_dest_to_es_not_ds() {
    let mut m = Machine::new(None);
    // 3E A4 = DS: MOVSB
    mem_write_slice(&mut m, SegOff::new(0x0000, 0x0000), &[0x3e, 0xa4, 0xf4]);
    // Source at DS:SI = 0x1000:0000 -> linear 0x10000, value 0x5A.
    m.reg_write_u16(DS, 0x1000);
    m.reg_write_u16(SI, 0x0000);
    m.mem.write_u8(SegOff::new(0x1000, 0x0000), 0x5A);
    // Dest at ES:DI = 0x2000:0000 -> linear 0x20000 (distinct from DS:DI).
    m.reg_write_u16(ES, 0x2000);
    m.reg_write_u16(DI, 0x0000);
    m.flag_write(FLAG_DF, false);
    m.step().unwrap();
    // The byte must land at ES:DI (0x20000), not DS:DI (0x10000).
    assert_eq!(m.mem.read_u8(SegOff::new(0x2000, 0x0000)), 0x5A, "dest must be ES:DI");
    assert_eq!(m.mem.read_u8(SegOff::new(0x1000, 0x0001)), 0x00, "DS:DI must be untouched");
    assert_eq!(m.reg_read_u16(SI), 0x0001);
    assert_eq!(m.reg_read_u16(DI), 0x0001);
  }

  /// Bare MOVSB (no prefix): source DS:SI, dest ES:DI — the canonical path.
  #[test]
  fn bare_movsb_copies_ds_si_to_es_di() {
    let mut m = Machine::new(None);
    mem_write_slice(&mut m, SegOff::new(0x0000, 0x0000), &[0xa4, 0xf4]);
    m.reg_write_u16(DS, 0x1000);
    m.reg_write_u16(SI, 0x0000);
    m.mem.write_u8(SegOff::new(0x1000, 0x0000), 0x77);
    m.reg_write_u16(ES, 0x2000);
    m.reg_write_u16(DI, 0x0000);
    m.flag_write(FLAG_DF, false);
    m.step().unwrap();
    assert_eq!(m.mem.read_u8(SegOff::new(0x2000, 0x0000)), 0x77);
    assert_eq!(m.reg_read_u16(SI), 0x0001);
    assert_eq!(m.reg_read_u16(DI), 0x0001);
  }
}
