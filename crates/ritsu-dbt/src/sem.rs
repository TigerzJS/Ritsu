//! Shared instruction semantics helpers.
//!
//! These are the Arm ARM pseudocode primitives that every tier must agree on. In M0 the
//! T0 interpreter calls them directly; WP-DBT-02 turns this module into the generic
//! semantic layer (one description, instantiated for T0, T1 and T2, see
//! `docs/subsystems/dbt.md` §3) and these functions become its constant-folding /
//! test-oracle implementation.

use crate::cpu::Nzcv;

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
