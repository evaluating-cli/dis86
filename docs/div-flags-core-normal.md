# DIV Instruction Flag Behavior

This note records observed emulator behavior for DIV/IDIV flags. These flag
values are not an architectural guarantee: Intel documents the status flags as
undefined after divide instructions.

## Execution Flow

All DIV/IDIV variants (DIVB/DIVW/DIVD, IDIVB/IDIVW/IDIVD) follow this pattern:

1. Compute quotient and remainder
2. Check for divide-by-zero or quotient overflow → `EXCEPTION(0)` if triggered
3. Store results into registers (AL/AH, AX/DX, EAX/EDX)
4. `FillFlags()` — flushes any pending lazy flags from the *previous* instruction
5. Explicitly set all six status flags via `SETFLAGBIT`

## Flag Values Set

| Flag | Value | Notes |
|------|-------|-------|
| **AF** | Always **0** | Marked `/*FIXME*/` |
| **SF** | Always **0** | Marked `/*FIXME*/` |
| **OF** | Always **0** | Marked `/*FIXME*/` |
| **ZF** | `(rem==0) && ((quo&1)!=0)` | Set iff remainder is zero AND quotient is odd |
| **CF** | `((rem&3)>=1 && (rem&3)<=2)` | Set iff low 2 bits of remainder are `01` or `10` |
| **PF** | `parity(rem) XOR parity(quo) XOR FLAG_PF` | Set iff rem and quo have the **same** parity |

For 16-bit and 32-bit, parity is computed over the full width:

```
# Illustrative parity calculation:
#define PARITY16(x)  (parity_lookup[((x)>>8)&0xff] ^ parity_lookup[(x)&0xff] ^ FLAG_PF)
#define PARITY32(x)  (PARITY16((x)&0xffff) ^ PARITY16(((x)>>16)&0xffff) ^ FLAG_PF)
```

`parity_lookup[byte]` returns `FLAG_PF` (0x4) if the byte has **even** bit-count, `0` if odd.

## Example: DIVB (8-bit)

For the 8-bit divide observation, the quotient is stored in AL and remainder in
AH. The sampled implementation cleared AF/SF/OF, set ZF when the remainder was
zero and quotient odd, set CF when the low two remainder bits were 1 or 2, and
derived PF from the parity of remainder and quotient.

## Notes

- All flags after DIV are officially **undefined** on real x86 hardware. The `/*FIXME*/`
  comments on AF, SF, OF indicate the author knows these are uncertain approximations.
- The ZF and CF formulas are non-standard approximations of observed real-hardware behavior.
- The sampled implementation committed pending flags from the prior instruction
  before setting the divide result flags.
- IDIV variants use identical flag-setting logic, just with signed arithmetic for the
  quotient/remainder computation.
