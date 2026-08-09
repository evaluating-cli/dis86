use super::machine::*;
use crate::asm::instr::{self, Instr, Opcode, Operand};

impl Machine {
    pub fn opcode_cmps(&mut self, instr: &Instr) -> Result<(), String> {
        assert_eq!(instr.opcode, Opcode::OP_CMPS);
        let Operand::Mem(source) = instr.operands[1] else {
            panic!("Expected source memory operand")
        };
        let size = match source.sz {
            instr::Size::Size8 => 1,
            instr::Size::Size16 => 2,
            _ => panic!("unsupported size"),
        };
        let increment = if self.flag_read(FLAG_DF) {
            0u16.wrapping_sub(size)
        } else {
            size
        };
        let rep = instr.rep;
        let mut count = if rep.is_some() {
            self.reg_read_u16(CX)
        } else {
            1
        };

        while count != 0 {
            // CMPS sets flags from DS:[SI] - ES:[DI], without storing the result.
            let lhs = self.operand_read(instr, 1);
            let rhs = self.operand_read(instr, 0);
            let (_, flags) = alu::binary(alu::BinaryOp::Sub, lhs, rhs, self.flag_read_all());
            self.flag_write_all(flags);
            self.reg_write_u16(SI, self.reg_read_u16(SI).wrapping_add(increment));
            self.reg_write_u16(DI, self.reg_read_u16(DI).wrapping_add(increment));
            count -= 1;

            if let Some(rep) = rep {
                let zf = self.flag_read(FLAG_ZF);
                match rep {
                    instr::Rep::EQ if !zf => break,
                    instr::Rep::NE if zf => break,
                    _ => (),
                }
            }
        }
        if rep.is_some() {
            self.reg_write_u16(CX, count);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        code: &[u8],
        source: &[u8],
        destination: &[u8],
        si: u16,
        di: u16,
        cx: u16,
        df: bool,
    ) -> Machine {
        let mut m = Machine::new(None);
        for (i, value) in code.iter().enumerate() {
            m.mem.write_u8(SegOff::new(0, i as u16), *value);
        }
        for (i, value) in source.iter().enumerate() {
            m.mem.write_u8(SegOff::new(0x1000, i as u16), *value);
        }
        for (i, value) in destination.iter().enumerate() {
            m.mem.write_u8(SegOff::new(0x2000, i as u16), *value);
        }
        m.reg_write_u16(DS, 0x1000);
        m.reg_write_u16(ES, 0x2000);
        m.reg_write_u16(SI, si);
        m.reg_write_u16(DI, di);
        m.reg_write_u16(CX, cx);
        m.flag_write(FLAG_DF, df);
        m.step().unwrap();
        m
    }

    #[test]
    fn byte_forward_and_subtraction_flags() {
        let m = run(&[0xa6], &[3], &[5], 0, 0, 9, false);
        assert_eq!((m.reg_read_u16(SI), m.reg_read_u16(DI)), (1, 1));
        assert!(m.flag_read(FLAG_CF));
        assert!(!m.flag_read(FLAG_ZF));
    }
    #[test]
    fn word_backward() {
        let m = run(
            &[0xa7],
            &[0, 0, 0x34, 0x12],
            &[0, 0, 0x34, 0x12],
            2,
            2,
            9,
            true,
        );
        assert_eq!((m.reg_read_u16(SI), m.reg_read_u16(DI)), (0, 0));
        assert!(m.flag_read(FLAG_ZF));
    }
    #[test]
    fn repe_stops_on_mismatch() {
        let m = run(&[0xf3, 0xa6], &[1, 2, 3], &[1, 9, 3], 0, 0, 3, false);
        assert_eq!(
            (m.reg_read_u16(SI), m.reg_read_u16(DI), m.reg_read_u16(CX)),
            (2, 2, 1)
        );
    }
    #[test]
    fn repne_stops_on_equality() {
        let m = run(&[0xf2, 0xa6], &[1, 2, 3], &[9, 2, 8], 0, 0, 3, false);
        assert_eq!(
            (m.reg_read_u16(SI), m.reg_read_u16(DI), m.reg_read_u16(CX)),
            (2, 2, 1)
        );
    }
    #[test]
    fn zero_count_does_not_access_memory_or_update_indices() {
        let m = run(&[0xf3, 0xa6], &[], &[], 0xffff, 0xffff, 0, false);
        assert_eq!(
            (m.reg_read_u16(SI), m.reg_read_u16(DI), m.reg_read_u16(CX)),
            (0xffff, 0xffff, 0)
        );
    }
}
