use super::alu::{self, ShiftOp};
use super::cpu::{BP, CS, DI, IP, SS};
use super::cpu_flags::*;
use super::machine::Machine;
use super::value::Value;
use crate::segoff::SegOff;

fn flags_with(of: bool, af: bool) -> Flags {
  let mut f = Flags(0);
  f.set(FLAG_OF, of);
  f.set(FLAG_AF, af);
  f
}

fn assert_flags(f: Flags, cf: bool, zf: bool, sf: bool, of: bool, pf: bool, af: bool) {
  assert_eq!(f.get(FLAG_CF), cf, "CF mismatch");
  assert_eq!(f.get(FLAG_ZF), zf, "ZF mismatch");
  assert_eq!(f.get(FLAG_SF), sf, "SF mismatch");
  assert_eq!(f.get(FLAG_OF), of, "OF mismatch");
  assert_eq!(f.get(FLAG_PF), pf, "PF mismatch");
  assert_eq!(f.get(FLAG_AF), af, "AF mismatch");
}

#[test]
fn effective_count_32_preserves_state() {
  let input = Flags(FLAG_MASK);

  for (op, value) in [
    (ShiftOp::Shl, Value::U8(0xa5)),
    (ShiftOp::Shr, Value::U8(0xa5)),
    (ShiftOp::Sar, Value::U8(0xa5)),
  ] {
    let (result, flags) = alu::shift(op, value, 32, input);
    assert_eq!(result, value);
    assert_eq!(flags.0, input.0);
  }
}

#[test]
fn effective_count_33_matches_count_1() {
  let input = flags_with(true, true);

  let (shl_1, shl_flags_1) = alu::shift(ShiftOp::Shl, Value::U16(0x8001), 1, input);
  let (shl_33, shl_flags_33) = alu::shift(ShiftOp::Shl, Value::U16(0x8001), 33, input);
  assert_eq!(shl_33, shl_1);
  assert_eq!(shl_flags_33.0, shl_flags_1.0);

  let (shr_1, shr_flags_1) = alu::shift(ShiftOp::Shr, Value::U16(0x8001), 1, input);
  let (shr_33, shr_flags_33) = alu::shift(ShiftOp::Shr, Value::U16(0x8001), 33, input);
  assert_eq!(shr_33, shr_1);
  assert_eq!(shr_flags_33.0, shr_flags_1.0);

  let (sar_1, sar_flags_1) = alu::shift(ShiftOp::Sar, Value::U16(0x8001), 1, input);
  let (sar_33, sar_flags_33) = alu::shift(ShiftOp::Sar, Value::U16(0x8001), 33, input);
  assert_eq!(sar_33, sar_1);
  assert_eq!(sar_flags_33.0, sar_flags_1.0);
}

#[test]
fn shl_count_1_defines_of_and_preserves_af() {
  let input = flags_with(false, true);
  let (result, flags) = alu::shift(ShiftOp::Shl, Value::U8(0x40), 1, input);

  assert_eq!(result, Value::U8(0x80));
  assert_flags(flags, false, false, true, true, false, true);
}

#[test]
fn shl_count_gt_1_preserves_of_and_af() {
  for incoming_of in [false, true] {
    let input = flags_with(incoming_of, true);
    let (result, flags) = alu::shift(ShiftOp::Shl, Value::U8(0x01), 8, input);

    assert_eq!(result, Value::U8(0x00));
    assert_flags(flags, true, true, false, incoming_of, true, true);
  }
}

#[test]
fn shl_count_above_width_clears_cf() {
  let input = flags_with(true, true);

  let (byte_result, byte_flags) = alu::shift(ShiftOp::Shl, Value::U8(0xff), 9, input);
  assert_eq!(byte_result, Value::U8(0x00));
  assert_flags(byte_flags, false, true, false, true, true, true);

  let (word_result, word_flags) = alu::shift(ShiftOp::Shl, Value::U16(0xffff), 17, input);
  assert_eq!(word_result, Value::U16(0x0000));
  assert_flags(word_flags, false, true, false, true, true, true);
}

#[test]
fn shr_count_1_uses_original_sign_for_of_and_preserves_af() {
  let input = flags_with(false, true);
  let (result, flags) = alu::shift(ShiftOp::Shr, Value::U8(0x80), 1, input);

  assert_eq!(result, Value::U8(0x40));
  assert_flags(flags, false, false, false, true, false, true);
}

#[test]
fn shr_count_gt_1_preserves_of_and_af() {
  for incoming_of in [false, true] {
    let input = flags_with(incoming_of, true);
    let (result, flags) = alu::shift(ShiftOp::Shr, Value::U8(0x80), 2, input);

    assert_eq!(result, Value::U8(0x20));
    assert_flags(flags, false, false, false, incoming_of, false, true);
  }
}

#[test]
fn shr_operand_width_count_uses_original_msb_for_cf() {
  let input = flags_with(true, true);

  let (byte_result, byte_flags) = alu::shift(ShiftOp::Shr, Value::U8(0x80), 8, input);
  assert_eq!(byte_result, Value::U8(0x00));
  assert_flags(byte_flags, true, true, false, true, true, true);

  let (word_result, word_flags) = alu::shift(ShiftOp::Shr, Value::U16(0x8000), 16, input);
  assert_eq!(word_result, Value::U16(0x0000));
  assert_flags(word_flags, true, true, false, true, true, true);
}

#[test]
fn shr_count_above_width_clears_cf() {
  let input = flags_with(true, true);

  let (byte_result, byte_flags) = alu::shift(ShiftOp::Shr, Value::U8(0xff), 9, input);
  assert_eq!(byte_result, Value::U8(0x00));
  assert_flags(byte_flags, false, true, false, true, true, true);

  let (word_result, word_flags) = alu::shift(ShiftOp::Shr, Value::U16(0xffff), 17, input);
  assert_eq!(word_result, Value::U16(0x0000));
  assert_flags(word_flags, false, true, false, true, true, true);
}

#[test]
fn sar_count_1_clears_of_and_preserves_af() {
  let input = flags_with(true, true);
  let (result, flags) = alu::shift(ShiftOp::Sar, Value::U8(0x81), 1, input);

  assert_eq!(result, Value::U8(0xc0));
  assert_flags(flags, true, false, true, false, true, true);
}

#[test]
fn sar_count_gt_1_preserves_of_and_af() {
  for incoming_of in [false, true] {
    let input = flags_with(incoming_of, true);
    let (result, flags) = alu::shift(ShiftOp::Sar, Value::U8(0x80), 2, input);

    assert_eq!(result, Value::U8(0xe0));
    assert_flags(flags, false, false, true, incoming_of, false, true);
  }
}

#[test]
fn sar_counts_at_or_above_width_saturate_to_sign() {
  let input = flags_with(true, true);

  let (byte_negative, byte_negative_flags) = alu::shift(ShiftOp::Sar, Value::U8(0x80), 8, input);
  assert_eq!(byte_negative, Value::U8(0xff));
  assert_flags(byte_negative_flags, true, false, true, true, true, true);

  let (byte_positive, byte_positive_flags) = alu::shift(ShiftOp::Sar, Value::U8(0x7f), 8, input);
  assert_eq!(byte_positive, Value::U8(0x00));
  assert_flags(byte_positive_flags, false, true, false, true, true, true);

  let (word_negative, word_negative_flags) = alu::shift(ShiftOp::Sar, Value::U16(0x8000), 16, input);
  assert_eq!(word_negative, Value::U16(0xffff));
  assert_flags(word_negative_flags, true, false, true, true, true, true);

  let (word_positive, word_positive_flags) = alu::shift(ShiftOp::Sar, Value::U16(0x7fff), 16, input);
  assert_eq!(word_positive, Value::U16(0x0000));
  assert_flags(word_positive_flags, false, true, false, true, true, true);
}

#[test]
fn sar_count_31_keeps_the_sign_bit_in_result_and_cf() {
  let input = flags_with(false, true);
  let (result, flags) = alu::shift(ShiftOp::Sar, Value::U8(0x80), 31, input);

  assert_eq!(result, Value::U8(0xff));
  assert_flags(flags, true, false, true, false, true, true);
}

// --- step-level: C1.x sign-extended imm8 count (SST-D-009) ---

fn run_shift_at() -> (Machine, Value) {
  // `shl word [ss:bp+di+6BDh],CBh` decode path (pinned C1.4 sample: bytes
  // C1 A3 BD 06 CB): C1 /4 with a mem operand exercises OPER_IMM8_EXT (imm8
  // sign-extended to u16). The count byte 0xCB sign-extends to 0xFFCB, which
  // must be masked to the low byte (0xCB -> 5-bit 0x0B) rather than tripping
  // an assertion.
  let mut m = Machine::new(None);
  m.reg_write_u16(CS, 0x0000);
  m.reg_write_u16(IP, 0x0000);
  m.reg_write_u16(SS, 0x1000);
  // shl word [ss:bp+di+6BDh],CBh  =>  C1 A3 BD 06 CB
  // (mod=10 reg=100/SHL rm=011/bp+di, disp16 06BDh, imm8 CBh)
  let code: [u8; 5] = [0xC1, 0xA3, 0xBD, 0x06, 0xCB];
  for (i, b) in code.iter().enumerate() {
    m.mem.write_u8(SegOff::new(0x0000, i as u16), *b);
  }
  m.reg_write_u16(BP, 0x0100);
  m.reg_write_u16(DI, 0x0004);
  // seed the memory operand at ss:bp+di+06BDh = 0x1000:0x07C1
  let mem_addr = SegOff::new(0x1000, 0x07C1);
  m.mem.write_u16(mem_addr, 0x8000);

  m.step().unwrap();

  let result = m.mem.read_u16(mem_addr);
  (m, Value::U16(result))
}

#[test]
fn c1x_sign_extended_count_masks_to_low_byte() {
  // 0x8000 << 0x0B (11) = 0x0000; the old assert `val as u8 as u16 == val`
  // panicked on the sign-extended 0xFFCB. The shift must complete (no trap,
  // machine advances) and produce the 5-bit-masked result.
  let (m, result) = run_shift_at();
  assert_eq!(result, Value::U16(0x0000));
  assert_eq!(m.exec_count, 1);
}
