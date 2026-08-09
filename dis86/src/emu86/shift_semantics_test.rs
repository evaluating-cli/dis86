use super::alu::{self, ShiftOp};
use super::cpu_flags::*;
use super::value::Value;

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
