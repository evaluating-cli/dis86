use super::alu::{self, DivideOp, ShiftOp};
use super::cpu::{AX, BP, BX, CS, DI, DS, DX, IP, SS};
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

// --- ROL flags (SST-D-003): CF = rotated-out bit; OF only when count mod
// width == 1; ZF/SF/PF/AF unchanged (pinned D1.0 sample `rol word [..],1`). ---

#[test]
fn rol_count_1_defines_cf_and_of() {
  // Pinned D1.0: rol word,1 on 0x6B2D -> 0xD65A. CF = old MSB (0), OF =
  // CF XOR new MSB (0 XOR 1) = 1. ZF/SF/PF/AF are undefined and preserved.
  let input = flags_with(false, true);
  let (result, flags) = alu::shift(ShiftOp::Rol, Value::U16(0x6b2d), 1, input);
  assert_eq!(result, Value::U16(0xd65a));
  assert_flags(flags, false, false, false, true, false, true);
}

#[test]
fn rol_count_1_cf_eq_msb_of_original() {
  // 0x80 rol-by-1 (byte): CF = MSB = 1, result = 0x01, OF = CF XOR new MSB
  // (new MSB = 0) = 1. ZF/SF/PF/AF untouched.
  let input = flags_with(false, false);
  let (result, flags) = alu::shift(ShiftOp::Rol, Value::U8(0x80), 1, input);
  assert_eq!(result, Value::U8(0x01));
  assert_flags(flags, true, false, false, true, false, false);
}

#[test]
fn rol_count_gt_1_sets_cf_and_preserves_of() {
  // rol byte,2 on 0x01: CF = original bit 6 = 0, result = 0x04. OF is
  // undefined for count != 1 and left unchanged on the 80C286.
  for incoming_of in [false, true] {
    let input = flags_with(incoming_of, true);
    let (result, flags) = alu::shift(ShiftOp::Rol, Value::U8(0x01), 2, input);
    assert_eq!(result, Value::U8(0x04));
    assert_flags(flags, false, false, false, incoming_of, false, true);
  }
}

#[test]
fn rol_full_circle_sets_cf_to_original_lsb() {
  // rol byte,8 (count mod width == 0, count != 0): value unchanged, CF = old
  // LSB = 1, OF untouched. Derived from C0.0 sample `rol byte [..],8` (exp CF=1).
  let input = flags_with(true, true);
  let (result, flags) = alu::shift(ShiftOp::Rol, Value::U8(0x0d), 8, input);
  assert_eq!(result, Value::U8(0x0d));
  assert_flags(flags, true, false, false, true, false, true);
}

#[test]
fn rol_count_0_is_a_noop() {
  // count == 0: nothing rotates and no flag changes at all (C0.0 sample
  // `rol byte [ds:bx],0` leaves flags identical).
  let input = flags_with(true, true);
  let (result, flags) = alu::shift(ShiftOp::Rol, Value::U8(0xa5), 0, input);
  assert_eq!(result, Value::U8(0xa5));
  assert_eq!(flags.0, input.0);
}

// --- step-level: XCHG with a memory operand whose EA uses the swapped
// register (SST-D-006). Pinned 87 sample `xchg di,[ds:di]` (bytes 87 3D F4):
// the EA must be computed from the pre-swap di. ---

#[test]
fn xchg_mem_ea_uses_pre_swap_register() {
  // ds:di = 0xAB8B:0x8AA6 -> linear 0xB4356, initial mem = 0xA91E, di = 0x8AA6.
  // A correct XCHG stores the old di (0x8AA6) to that address; re-deriving the
  // EA from the post-swap di (0xA91E -> 0xB34CE) would write to the wrong place.
  let mut m = Machine::new(None);
  m.reg_write_u16(CS, 0x0000);
  m.reg_write_u16(IP, 0x0000);
  m.reg_write_u16(DS, 0xab8b);
  m.reg_write_u16(DI, 0x8aa6);
  let code: [u8; 3] = [0x87, 0x3D, 0xF4];
  for (i, b) in code.iter().enumerate() {
    m.mem.write_u8(SegOff::new(0x0000, i as u16), *b);
  }
  let mem_addr = SegOff::new(0xab8b, 0x8aa6);
  m.mem.write_u16(mem_addr, 0xa91e);

  m.step().unwrap();

  assert_eq!(m.reg(DI), 0xa91e);
  assert_eq!(m.mem.read_u16(mem_addr), 0x8aa6);
  assert_eq!(m.exec_count, 1);
}

// --- step-level: far-pointer memory reads wrap the EA offset at the 64KB
// segment boundary (SST-D-007). Pinned FF.5 sample `jmp far [ds:bx+di]` (bytes
// FF 29 F4, ds=0x0FBA, bx=di=0xFFFF): the 4-byte far pointer at EA ds:0xFFFE
// straddles the boundary, so its CS word must come from the wrapped
// ds:0x0000/0x0001 rather than the linear continuation. ---

#[test]
fn jmp_far_mem_wraps_far_pointer_at_64kb_boundary() {
  // ds:bx+di = 0x0FBA:(0xFFFF+0xFFFF) -> EA ds:0xFFFE (linear 0x1F99E). IP
  // bytes at ds:0xFFFE/0xFFFF = CD 74 -> 0x74CD; CS bytes at the *wrapped*
  // ds:0x0000/0x0001 = 8F 84 -> 0x848F. The linear continuation of the 4-byte
  // read (linear 0x1F9A0/0x1F9A1) holds 12 34, so a non-wrapping read would
  // load CS=0x3412 and jump to the wrong segment.
  let mut m = Machine::new(None);
  m.reg_write_u16(CS, 0x0000);
  m.reg_write_u16(IP, 0x0100);
  m.reg_write_u16(DS, 0x0fba);
  m.reg_write_u16(BX, 0xffff);
  m.reg_write_u16(DI, 0xffff);
  // `jmp far [ds:bx+di]` = FF 29 (modrm 0x29: mod=00 reg=5/JMPf rm=001/[bx+di])
  for (i, &b) in [0xFFu8, 0x29].iter().enumerate() {
    m.mem.write_u8(SegOff::new(0x0000, 0x0100 + i as u16), b);
  }
  // Far pointer at ds:0xFFFE: IP=0x74CD (CD 74), CS=0x848F (8F 84 at the
  // wrapped ds:0x0000/0x0001). The linear-continuation bytes (0x1F9A:0x0000/
  // 0x0001) hold 12 34, so a non-wrapping read would load CS=0x3412.
  m.mem.write_u8(SegOff::new(0x0fba, 0xfffe), 0xCD);
  m.mem.write_u8(SegOff::new(0x0fba, 0xffff), 0x74);
  m.mem.write_u8(SegOff::new(0x0fba, 0x0000), 0x8F);
  m.mem.write_u8(SegOff::new(0x0fba, 0x0001), 0x84);
  m.mem.write_u8(SegOff::new(0x1f9a, 0x0000), 0x12);
  m.mem.write_u8(SegOff::new(0x1f9a, 0x0001), 0x34);

  m.step().unwrap();

  assert_eq!(m.reg(CS), 0x848f);
  assert_eq!(m.reg(IP), 0x74cd);
  assert_eq!(m.exec_count, 1);
}

// --- signed IDIV (SST-D-005): IDIV signed 32/16 division. Pinned F7.7 sample
// `idiv word [ss:bp+di]` (bytes F7 3B, ss=0xEED4 bp=0x97C7 di=0x35DC): the
// 32-bit dividend DX:AX = 0x0B1E:0x9A19 (186,554,905) divided by the i16
// divisor 0x93ED (-27,667) gives quotient -6742 (0xE5AA) and remainder +23991
// (0x5DB7). The old unsigned-only divmod produced 0x133E / 0x2FB3. ---

#[test]
fn idiv_word_signed_division_matches_pinned_sample() {
  let mut m = Machine::new(None);
  m.reg_write_u16(CS, 0x0000);
  m.reg_write_u16(IP, 0x0000);
  m.reg_write_u16(SS, 0xeed4);
  m.reg_write_u16(BP, 0x97c7);
  m.reg_write_u16(DI, 0x35dc);
  m.reg_write_u16(AX, 0x9a19);
  m.reg_write_u16(DX, 0x0b1e);
  // `idiv word [ss:bp+di]` = F7 3B (modrm 0x3B: mod=00 reg=111/IDIV rm=011/[bp+di])
  for (i, &b) in [0xF7u8, 0x3B].iter().enumerate() {
    m.mem.write_u8(SegOff::new(0x0000, i as u16), b);
  }
  // divisor at ss:bp+di = 0xEED4:(0x97C7+0x35DC=0xCDA3) = 0x93ED
  let mem_addr = SegOff::new(0xeed4, 0xcda3);
  m.mem.write_u16(mem_addr, 0x93ed);

  m.step().unwrap();

  assert_eq!(m.reg(AX), 0xe5aa);
  assert_eq!(m.reg(DX), 0x5db7);
  assert_eq!(m.exec_count, 1);
}

#[test]
fn divmod_signed_truncates_toward_zero_and_keeps_dividend_sign() {
  // 186,554,905 / -27,667 = -6742 (truncated toward zero), remainder +23991.
  let f = Flags(0);
  let (q, r, _) = alu::divmod(
    DivideOp::Signed,
    Value::U32(0x0b1e9a19),
    Value::U16(0x93ed),
    f,
  );
  assert_eq!(q, Value::U16(0xe5aa));
  assert_eq!(r, Value::U16(0x5db7));

  // -10 / 3 = -3 (truncation toward zero, not floor), remainder -1.
  let (q, r, _) = alu::divmod(DivideOp::Signed, Value::U32(0xfffffff6), Value::U16(3), Flags(0));
  assert_eq!(q, Value::U16(0xfffd)); // -3
  assert_eq!(r, Value::U16(0xffff)); // -1
}

#[test]
fn divmod_unsigned_still_wraps_like_div() {
  // 0x0B1E9A19 / 0x93ED (unsigned 37,869) = 4,926 (0x133E), remainder 12,211
  // (0x2FB3) — matches the pre-fix emu86 behaviour for the same operands.
  let (q, r, _) = alu::divmod(DivideOp::Unsigned, Value::U32(0x0b1e9a19), Value::U16(0x93ed), Flags(0));
  assert_eq!(q, Value::U16(0x133e));
  assert_eq!(r, Value::U16(0x2fb3));
}

#[test]
#[should_panic(expected = "Divide Error")]
fn divmod_signed_divide_by_zero_panics() {
  alu::divmod(DivideOp::Signed, Value::U32(0x100), Value::U16(0), Flags(0));
}

#[test]
#[should_panic(expected = "Divide Error")]
fn divmod_signed_quotient_overflow_panics() {
  // -32768 / -1 overflows signed 16 bits -> #DE.
  alu::divmod(DivideOp::Signed, Value::U32(0xffff8000), Value::U16(0xffff), Flags(0));
}
