use super::machine::*;

// goal:
//   take two values (generic)
//   perform some operation
//   update flags

pub enum BinaryOp {
  Add,
  Adc,
  Sub,
  Sbb,
  And,
  Or,
  Xor,
}

pub enum MultiplyOp {
  Unsigned,
  Signed,
}

pub enum DivideOp {
  Unsigned,
  Signed,
}

pub enum UnaryOp {
  Neg,
  Inc,
  Dec,
  Not,
}

pub enum ShiftOp {
  Shl,
  Shr,
  Sar,
  Rol,
}

fn flag_generic_sf(r: u16, sign_mask: u16)  -> bool { (r & sign_mask) != 0 }
fn flag_generic_zf(r: u16, value_mask: u16) -> bool { (r & value_mask) == 0 }
fn flag_generic_pf(r: u16)                  -> bool { (r as u8).count_ones() % 2 == 0 } // PF uses low byte only

fn update_flags_sub(f: &mut Flags, a: u16, b: u16, carry_in: u16, r: u16, sign_mask: u16, value_mask: u16, update_cf: bool) {
  assert!(carry_in <= 1);
  let cf = (a as u32) < (b as u32) + (carry_in as u32);

  if update_cf { f.set(FLAG_CF, cf) };
  f.set(FLAG_ZF, flag_generic_zf(r, value_mask));
  f.set(FLAG_SF, flag_generic_sf(r, sign_mask));
  f.set(FLAG_PF, flag_generic_pf(r));
  f.set(FLAG_AF, ((a as u32) & 0x0F) < ((b as u32) & 0x0F) + (carry_in as u32));

  // Overflow cases
  // -------------------------------------------------
  //   positive - negative - cf = negative?  -> OF=1 (should have been positive)
  //   negative - positive - cf = positive?  -> OF=1 (should have been negative)
  f.set(FLAG_OF, ((a ^ b) & (a ^ r) & sign_mask) != 0);
}

fn update_flags_bitwise(f: &mut Flags, r: u16, sign_mask: u16, value_mask: u16) {
  f.set(FLAG_CF, false);
  f.set(FLAG_ZF, flag_generic_zf(r, value_mask));
  f.set(FLAG_SF, flag_generic_sf(r, sign_mask));
  f.set(FLAG_OF, false);
  f.set(FLAG_PF, flag_generic_pf(r));
}

fn update_flags_add(f: &mut Flags, a: u16, b: u16, carry_in: u16, r32: u32, sign_mask: u16, value_mask: u16, update_cf: bool) {
  assert!(carry_in <= 1);
  let r = r32 as u16;
  let cf = ((r32 >> 1) & (sign_mask as u32)) != 0;

  if update_cf { f.set(FLAG_CF, cf) };
  f.set(FLAG_ZF, flag_generic_zf(r, value_mask));
  f.set(FLAG_SF, flag_generic_sf(r, sign_mask));
  f.set(FLAG_PF, flag_generic_pf(r));
  f.set(FLAG_AF, (a & 0x0F) + (b & 0x0F) + carry_in > 0x0F);

  // Overflow cases
  // -------------------------------------------------
  //   positive + positive + cf = negative?  -> OF=1 (should have been positive)
  //   negative + negative + cf = positive?  -> OF=1 (should have been negative)
  //
  // NOTICE: carry flag is 0 or 1, so it cannot the result sign (we can ignore it)
  f.set(FLAG_OF, ((a ^ r) & (b ^ r) & sign_mask) != 0);
}

fn update_flags_shift_result(f: &mut Flags, cf: bool, r: u16, sign_mask: u16, value_mask: u16) {
  f.set(FLAG_CF, cf);
  f.set(FLAG_ZF, flag_generic_zf(r, value_mask));
  f.set(FLAG_SF, flag_generic_sf(r, sign_mask));
  f.set(FLAG_PF, flag_generic_pf(r));
}

fn update_flags_shl(f: &mut Flags, n: u8, r32: u32, sign_mask: u16, value_mask: u16) {
  if n == 0 { return; }

  let r = r32 as u16;
  let cf = ((r32 >> 1) & (sign_mask as u32)) != 0;
  update_flags_shift_result(f, cf, r, sign_mask, value_mask);

  if n == 1 {
    f.set(FLAG_OF, cf ^ flag_generic_sf(r, sign_mask));
  }
}

fn update_flags_shr(f: &mut Flags, a: u16, n: u8, r: u16, sign_mask: u16, value_mask: u16) {
  if n == 0 { return; }

  let cf = (((a as u32) >> (n - 1)) & 1) != 0;
  update_flags_shift_result(f, cf, r, sign_mask, value_mask);

  if n == 1 {
    f.set(FLAG_OF, (a & sign_mask) != 0);
  }
}

fn update_flags_sar(f: &mut Flags, a: i32, n: u8, r: u16, sign_mask: u16, value_mask: u16) {
  if n == 0 { return; }

  let cf = ((a >> (n - 1)) & 1) != 0;
  update_flags_shift_result(f, cf, r, sign_mask, value_mask);

  if n == 1 {
    f.set(FLAG_OF, false);
  }
}

// Returns (quotient, remainder, flags)
pub fn divmod(op: DivideOp, a: Value, b: Value, mut f: Flags) -> (Value, Value, Flags) {
  let Value::U32(a) = a else { panic!("expected u32 for lhs") };
  let Value::U16(b) = b else { panic!("expected u16 for rhs") };

  // DIV: unsigned 32/16 division, quotient must fit 16 bits. IDIV: signed
  // 32/16 division (both operands sign-extended), quotient must fit signed 16
  // bits. Both raise #DE (divide error) on overflow or a zero divisor; emu86
  // has no exception machinery, so the corpus marks #DE tests SkipException
  // and the panic below is the divide-error surface.
  let quotient;
  let remainder;
  match op {
    DivideOp::Unsigned => {
      let b = b as u32;
      if b == 0 { panic!("Divide Error"); }
      quotient = a / b;
      remainder = a % b;
      if quotient > 0xffff {
        panic!("Divide Error"); // What should be done about this??
      }
    }
    DivideOp::Signed => {
      let dividend = a as i32 as i64;
      let divisor = b as i16 as i64;
      if divisor == 0 { panic!("Divide Error"); }
      quotient = (dividend / divisor) as u32;
      remainder = (dividend % divisor) as u32;
      if (quotient as i32) > 0x7fff || (quotient as i32) < -0x8000 {
        panic!("Divide Error");
      }
    }
  }

  // Mirroring the behaviour of dosbox-x
  f.set(FLAG_CF, (remainder&3) >= 1 && (remainder&3) <= 2);  // Set iff low 2 bits of remainder are 01 or 10 )
  f.set(FLAG_ZF, remainder == 0 && (quotient&1) != 0);       // Set iff remainder is zero AND quotient is odd
  f.set(FLAG_SF, false);
  f.set(FLAG_OF, false);
  f.set(FLAG_AF, false);

  // Set iff rem and quo have the same parity
  let rem_parity = remainder.count_ones() % 2 != 0;
  let quo_parity = quotient.count_ones() % 2 != 0;
  f.set(FLAG_PF, rem_parity == quo_parity);

  (Value::U16(quotient as u16), Value::U16(remainder as u16), f)
}

pub fn multiply(op: MultiplyOp, a: Value, b: Value, mut f: Flags) -> (Value, Flags) {
  // Unpack common case
  let (size, _sign_mask, value_mask, a, b) = match (a, b) {
    (Value::U8(a),  Value::U8(b))  => (1, 0x80,   0xff,   a as u16, b as u16),
    (Value::U16(a), Value::U16(b)) => (2, 0x8000, 0xffff, a, b),
    _ => panic!("Mismatched sizes"),
  };

  let result: u32;
  match op {
    MultiplyOp::Unsigned => {
      result = (a as u32) * (b as u32);

      let ovf = (result & (value_mask as u32)) != result;
      f.set(FLAG_CF, ovf);
      f.set(FLAG_OF, ovf);
      f.set(FLAG_ZF, result == 0);
    }
    MultiplyOp::Signed => {
      result = match size {
        1 => ((a as i8 as i16)  * (b as i8 as i16))  as u16 as u32,
        2 => ((a as i16 as i32) * (b as i16 as i32)) as u32,
        _ => unreachable!(),
      };

      // IMUL sets CF/OF only when the product does not fit the destination
      // half (AX for size 1, DX:AX for size 2): i.e. when the result is not a
      // sign-extension of its low half. `result & value_mask != result` was
      // wrong: e.g. imul word AX*0xFFFF = 0xFFFFFE25 has high word 0xFFFF
      // (sign-extended 0xFE25) and fits, but that check reported ovf.
      let ovf = match size {
        1 => (result as u16) != ((result & 0xff) as i8 as i16 as u16),
        2 => result != ((result & 0xffff) as i16 as i32 as u32),
        _ => unreachable!(),
      };
      f.set(FLAG_CF, ovf);
      f.set(FLAG_OF, ovf);
      //f.set(FLAG_ZF, result == 0);
    }
  }

  // Special re-pack because they return larger types
  let val = match size {
    1 => Value::U16(result as u16),
    2 => Value::U32(result),
    _ => unreachable!(),
  };

  (val, f)
}

pub fn binary(op: BinaryOp, a: Value, b: Value, mut f: Flags) -> (Value, Flags) {
  // Unpack common case
  let (size, sign_mask, value_mask, a, b) = match (a, b) {
    (Value::U8(a),  Value::U8(b))  => (1, 0x80,   0xff,   a as u16, b as u16),
    (Value::U16(a), Value::U16(b)) => (2, 0x8000, 0xffff, a, b),
    _ => panic!("Mismatched sizes"),
  };

  let result;
  match op {
    BinaryOp::Add => {
      let r32 = (a as u32) + (b as u32);
      result = r32 as u16;
      update_flags_add(&mut f, a, b, 0, r32, sign_mask, value_mask, true);
    }
    BinaryOp::Adc => {
      let carry_in = f.get(FLAG_CF) as u16;
      let r32 = (a as u32) + (b as u32) + (carry_in as u32);
      result = r32 as u16;
      update_flags_add(&mut f, a, b, carry_in, r32, sign_mask, value_mask, true);
    }
    BinaryOp::Sub => {
      result = a.wrapping_sub(b);
      update_flags_sub(&mut f, a, b, 0, result, sign_mask, value_mask, true);
    }
    BinaryOp::Sbb => {
      let carry_in = f.get(FLAG_CF) as u16;
      result = a.wrapping_sub(b).wrapping_sub(carry_in);
      update_flags_sub(&mut f, a, b, carry_in, result, sign_mask, value_mask, true);
    }
    BinaryOp::And => {
      result = a & b;
      update_flags_bitwise(&mut f, result, sign_mask, value_mask);
    }
    BinaryOp::Or => {
      result = a | b;
      update_flags_bitwise(&mut f, result, sign_mask, value_mask);
    }
    BinaryOp::Xor => {
      result = a ^ b;
      update_flags_bitwise(&mut f, result, sign_mask, value_mask);
    }
  };

  // Re-pack
  let result_value = match size {
    1 => Value::U8(result as u8),
    2 => Value::U16(result),
    _ => unreachable!(),
  };

  (result_value, f)
}

pub fn unary(op: UnaryOp, a: Value, mut f: Flags) -> (Value, Flags) {
  // Unpack
  let (size, sign_mask, value_mask, a) = match a {
    Value::U8(a)  => (1, 0x80,   0xff,   a as u16),
    Value::U16(a) => (2, 0x8000, 0xffff, a),
    _ => panic!("Mismatched sizes"),
  };

  let result;
  match op {
    UnaryOp::Neg => {
      result = (a as i16).wrapping_neg() as u16;
      update_flags_sub(&mut f, 0, a, 0, result, sign_mask, value_mask, true);
    }
    UnaryOp::Inc => {
      let r32 = (a as u32) + (1 as u32);
      result = r32 as u16;
      update_flags_add(&mut f, a, 1, 0, r32, sign_mask, value_mask, false);
    }
    UnaryOp::Dec => {
      result = a.wrapping_sub(1);
      update_flags_sub(&mut f, a, 1, 0, result, sign_mask, value_mask, false);
    }
    UnaryOp::Not => {
      result = !a;
    }
  };


  // Re-pack
  let result_value = match size {
    1 => Value::U8(result as u8),
    2 => Value::U16(result),
    _ => unreachable!(),
  };

  (result_value, f)
}

pub fn shift(op: ShiftOp, a: Value, n: u8, mut f: Flags) -> (Value, Flags) {
  // Unpack
  let (size, sign_mask, value_mask, a) = match a {
    Value::U8(a)  => (1, 0x80,   0xff,   a as u16),
    Value::U16(a) => (2, 0x8000, 0xffff, a),
    _ => panic!("Mismatched sizes"),
  };

  let result;
  match op {
    ShiftOp::Shl => {
      let n = n & 0x1f;
      if n == 0 {
        result = a;
      } else {
        let r32 = (a as u32) << (n as u32);
        result = r32 as u16;
        update_flags_shl(&mut f, n, r32, sign_mask, value_mask);
      }
    }
    ShiftOp::Shr => {
      let n = n & 0x1f;
      if n == 0 {
        result = a;
      } else {
        result = ((a as u32) >> (n as u32)) as u16;
        update_flags_shr(&mut f, a, n, result, sign_mask, value_mask);
      }
    }
    ShiftOp::Sar => {
      let n = n & 0x1f;
      if n == 0 {
        result = a;
      } else {
        let signed = match size {
          1 => a as u8 as i8 as i32,
          2 => a as i16 as i32,
          _ => unreachable!(),
        };
        result = (signed >> (n as u32)) as u16;
        update_flags_sar(&mut f, signed, n, result, sign_mask, value_mask);
      }
    }
    ShiftOp::Rol => {
      let n = n & 0x1f;
      let width = match size {
        1 => 8,
        2 => 16,
        _ => unreachable!(),
      };
      let n_mod = n % width;
      result = match size {
        1 => (a as u8).rotate_left(n as u32) as u16,
        2 => a.rotate_left(n as u32),
        _ => unreachable!(),
      };
      if n != 0 {
        // CF = the last bit rotated out: the original bit at position
        // (width - n_mod) mod width. A full-circle rotate (n_mod == 0, n != 0)
        // therefore sets CF = original LSB. Count 0 is a complete no-op and
        // preserves every flag. ROL preserves ZF/SF/PF/AF for non-zero counts;
        // OF is defined only for the actual effective count == 1.
        let cf = ((a >> ((width - n_mod) % width)) & 1) != 0;
        f.set(FLAG_CF, cf);
        if n == 1 {
          let new_msb = ((result >> (width - 1)) & 1) != 0;
          f.set(FLAG_OF, cf ^ new_msb);
        }
      }
    }
  };


  // Re-pack
  let result_value = match size {
    1 => Value::U8(result as u8),
    2 => Value::U16(result),
    _ => unreachable!(),
  };

  (result_value, f)
}
