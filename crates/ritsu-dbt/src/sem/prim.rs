//! Arm ARM pseudocode primitives on concrete values.
//!
//! These functions are the *only* place where the arithmetic meaning of a [`Sem`]
//! operation is written down for concrete operands. The [`Concrete`] backend (T0) calls
//! them directly; T2's constant folding and the IR evaluator in the tests reuse them, so
//! no second copy of the arithmetic exists.
//!
//! Width-generic functions take a [`Ty`] and accept any `u64`: inputs are truncated to
//! the type width first and results are zero-extended (canonical form, see [`Ty`]).
//!
//! [`Sem`]: super::Sem
//! [`Concrete`]: super::concrete::Concrete

use super::{BinOp, CmpOp, Ty};
use crate::cpu::Nzcv;
use crate::decode::Cond;

/// `AddWithCarry(x, y, carry_in)` for 64-bit operands.
///
/// Returns the result and the flags `N` (bit 63), `Z` (result == 0), `C` (unsigned
/// carry out) and `V` (signed overflow). Subtraction `x - y` is
/// `add_with_carry64(x, !y, true)`, so `C == 1` means "no borrow".
#[must_use]
pub const fn add_with_carry64(x: u64, y: u64, carry_in: bool) -> (u64, Nzcv) {
    let (partial, c1) = x.overflowing_add(y);
    let (result, c2) = partial.overflowing_add(carry_in as u64);
    // Signed overflow: both inputs have the same sign and the result's sign differs.
    let v = ((x ^ result) & (y ^ result)) >> 63 == 1;
    (
        result,
        Nzcv::new(result >> 63 == 1, result == 0, c1 || c2, v),
    )
}

/// `AddWithCarry(x, y, carry_in)` for 32-bit operands (`Wn` forms).
///
/// The flags are computed at 32 bits: `N` is bit 31, `C` is the carry out of bit 31.
#[must_use]
pub const fn add_with_carry32(x: u32, y: u32, carry_in: bool) -> (u32, Nzcv) {
    let (partial, c1) = x.overflowing_add(y);
    let (result, c2) = partial.overflowing_add(carry_in as u32);
    let v = ((x ^ result) & (y ^ result)) >> 31 == 1;
    (
        result,
        Nzcv::new(result >> 31 == 1, result == 0, c1 || c2, v),
    )
}

/// `AddWithCarry(x, y, carry_in)` at width `ty` (Arm ARM shared pseudocode).
///
/// Returns the zero-extended result and `N` (top bit of the result), `Z`, `C` (unsigned
/// carry out of the top bit) and `V` (signed overflow), all computed at width `ty`.
#[must_use]
pub fn add_with_carry(ty: Ty, x: u64, y: u64, carry_in: bool) -> (u64, Nzcv) {
    match ty {
        Ty::I64 => add_with_carry64(x, y, carry_in),
        Ty::I32 => {
            // Truncation to the operand width is the architected behaviour.
            let (r, f) = add_with_carry32(x as u32, y as u32, carry_in);
            (u64::from(r), f)
        }
        Ty::I1 | Ty::I8 | Ty::I16 => {
            // Widths below 32: the full sum fits in a u64 without overflow.
            let bits = ty.bits();
            let (x, y) = (truncate(ty, x), truncate(ty, y));
            let sum = x + y + u64::from(carry_in);
            let result = truncate(ty, sum);
            let top = bits - 1;
            let v = ((x ^ result) & (y ^ result)) >> top & 1 == 1;
            (
                result,
                Nzcv::new(result >> top & 1 == 1, result == 0, sum >> bits != 0, v),
            )
        }
    }
}

/// Truncates `v` to the width of `ty` (zero-extended canonical form).
#[must_use]
pub const fn truncate(ty: Ty, v: u64) -> u64 {
    v & ty.mask()
}

/// Sign-extends the low `ty.bits()` bits of `v` to 64 bits (Arm `SignExtend`).
#[must_use]
pub const fn sign_extend(ty: Ty, v: u64) -> i64 {
    let s = 64 - ty.bits();
    ((v << s) as i64) >> s
}

/// The integer binary operations of [`BinOp`] at width `ty`.
///
/// Shift and rotate amounts are taken modulo the width, as in the Arm ARM
/// `ShiftReg()` used by `LSLV/LSRV/ASRV/RORV` (`UInt(Rm) MOD datasize`). Immediate
/// shifts are decode-time constants below the width, for which this is the identity.
#[must_use]
pub fn binop(op: BinOp, ty: Ty, a: u64, b: u64) -> u64 {
    let (a, b) = (truncate(ty, a), truncate(ty, b));
    let bits = ty.bits();
    // `bits` is 1..=64, so the amount is < 64 and every shift below is in range.
    let amount = (b % u64::from(bits)) as u32;
    let r = match op {
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::And => a & b,
        BinOp::Or => a | b,
        BinOp::Xor => a ^ b,
        BinOp::Shl => a << amount,
        BinOp::Lshr => a >> amount,
        BinOp::Ashr => (sign_extend(ty, a) >> amount) as u64,
        BinOp::Ror => {
            if amount == 0 {
                a
            } else {
                (a >> amount) | (a << (bits - amount))
            }
        }
    };
    truncate(ty, r)
}

/// Bitwise `NOT` at width `ty`.
#[must_use]
pub const fn not(ty: Ty, a: u64) -> u64 {
    truncate(ty, !a)
}

/// Integer comparison at width `ty` (signed variants interpret the operands as two's
/// complement of that width).
#[must_use]
pub const fn icmp(cc: CmpOp, ty: Ty, a: u64, b: u64) -> bool {
    let (ua, ub) = (truncate(ty, a), truncate(ty, b));
    let (sa, sb) = (sign_extend(ty, a), sign_extend(ty, b));
    match cc {
        CmpOp::Eq => ua == ub,
        CmpOp::Ne => ua != ub,
        CmpOp::Ult => ua < ub,
        CmpOp::Ule => ua <= ub,
        CmpOp::Ugt => ua > ub,
        CmpOp::Uge => ua >= ub,
        CmpOp::Slt => sa < sb,
        CmpOp::Sle => sa <= sb,
        CmpOp::Sgt => sa > sb,
        CmpOp::Sge => sa >= sb,
    }
}

/// `ConditionHolds(cond)` from the Arm ARM: `cond[3:1]` selects the base test,
/// `cond[0]` inverts it, except for `1111` (`NV`), which is "always" like `AL`.
#[must_use]
pub const fn condition_holds(cond: Cond, f: Nzcv) -> bool {
    let bits = cond.bits();
    let base = match bits >> 1 {
        0b000 => f.z(),
        0b001 => f.c(),
        0b010 => f.n(),
        0b011 => f.v(),
        0b100 => f.c() && !f.z(),
        0b101 => f.n() == f.v(),
        0b110 => f.n() == f.v() && !f.z(),
        _ => true,
    };
    if bits & 1 == 1 && bits != 0b1111 {
        !base
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent oracle: the literal Arm pseudocode using 128-bit arithmetic.
    fn reference(x: u64, y: u64, cin: bool, bits: u32) -> (u64, Nzcv) {
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let (x, y) = (x & mask, y & mask);
        let sext = |v: u64| -> i128 {
            let s = 128 - bits;
            (i128::from(v) << s) >> s
        };
        let unsigned_sum = u128::from(x) + u128::from(y) + u128::from(cin);
        let signed_sum = sext(x) + sext(y) + i128::from(cin);
        let result = (unsigned_sum as u64) & mask;
        let n = (result >> (bits - 1)) & 1 == 1;
        let z = result == 0;
        let c = u128::from(result) != unsigned_sum;
        let v = sext(result) != signed_sum;
        (result, Nzcv::new(n, z, c, v))
    }

    const EDGE: [u64; 12] = [
        0,
        1,
        2,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_ffff,
        0x1_0000_0000,
        0x7fff_ffff_ffff_ffff,
        0x8000_0000_0000_0000,
        0xffff_ffff_ffff_fffe,
        0xffff_ffff_ffff_ffff,
        0x1234_5678_9abc_def0,
    ];

    #[test]
    fn matches_reference_on_edge_values() {
        for &x in &EDGE {
            for &y in &EDGE {
                for cin in [false, true] {
                    assert_eq!(add_with_carry64(x, y, cin), reference(x, y, cin, 64));
                    let (r32, f32) = add_with_carry32(x as u32, y as u32, cin);
                    assert_eq!((u64::from(r32), f32), reference(x, y, cin, 32));
                }
            }
        }
    }

    #[test]
    fn classic_cases_64() {
        // ADDS: INT64_MAX + 1 → signed overflow, no carry.
        assert_eq!(
            add_with_carry64(i64::MAX as u64, 1, false),
            (1 << 63, Nzcv::new(true, false, false, true))
        );
        // ADDS: UINT64_MAX + 1 → zero with carry.
        assert_eq!(
            add_with_carry64(u64::MAX, 1, false),
            (0, Nzcv::new(false, true, true, false))
        );
        // SUBS 0 - 1 → borrow, so C = 0.
        assert_eq!(
            add_with_carry64(0, !1, true),
            (u64::MAX, Nzcv::new(true, false, false, false))
        );
        // SUBS 5 - 5 → Z and C (no borrow).
        assert_eq!(
            add_with_carry64(5, !5, true),
            (0, Nzcv::new(false, true, true, false))
        );
        // SUBS INT64_MIN - 1 → signed overflow, no borrow.
        assert_eq!(
            add_with_carry64(1 << 63, !1, true),
            (i64::MAX as u64, Nzcv::new(false, false, true, true))
        );
    }

    #[test]
    fn classic_cases_32() {
        assert_eq!(
            add_with_carry32(0x7fff_ffff, 1, false),
            (0x8000_0000, Nzcv::new(true, false, false, true))
        );
        assert_eq!(
            add_with_carry32(u32::MAX, 1, false),
            (0, Nzcv::new(false, true, true, false))
        );
        assert_eq!(
            add_with_carry32(0, !1, true),
            (u32::MAX, Nzcv::new(true, false, false, false))
        );
        assert_eq!(
            add_with_carry32(0x8000_0000, !1, true),
            (0x7fff_ffff, Nzcv::new(false, false, true, true))
        );
    }
}
