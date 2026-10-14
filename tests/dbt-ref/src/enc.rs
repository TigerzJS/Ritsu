//! A64 instruction encoders for the generator.
//!
//! Written from the Arm ARM encoding tables, independent of `ritsu-dbt`'s decoder and test
//! assembler, so that a decoder bug cannot hide behind a matching encoder bug. The unit tests
//! compare against words produced by `llvm-mc-18` (listed in the tests).
//!
//! Register arguments are the 5-bit fields (31 = `SP` or `XZR` depending on the instruction).

/// `MOVN`/`MOVZ`/`MOVK` selector (`opc` field).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveWide {
    /// `MOVN`
    N = 0,
    /// `MOVZ`
    Z = 2,
    /// `MOVK`
    K = 3,
}

/// `MOVN`/`MOVZ`/`MOVK` `<Rd>, #imm16{, LSL #(16*hw)}`; `hw < 2` when `sf == 0`.
#[must_use]
pub fn move_wide(op: MoveWide, sf: bool, hw: u32, imm16: u32, rd: u32) -> u32 {
    debug_assert!(hw < 4 && (sf || hw < 2) && imm16 < 0x1_0000 && rd < 32);
    (u32::from(sf) << 31) | ((op as u32) << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}

/// `ADD`/`SUB`/`ADDS`/`SUBS` (immediate). `rn`, and `rd` unless `set_flags`, may be `SP` (31).
#[must_use]
pub fn add_sub_imm(
    sf: bool,
    sub: bool,
    set_flags: bool,
    shift12: bool,
    imm12: u32,
    rn: u32,
    rd: u32,
) -> u32 {
    debug_assert!(imm12 < 0x1000 && rn < 32 && rd < 32);
    (u32::from(sf) << 31)
        | (u32::from(sub) << 30)
        | (u32::from(set_flags) << 29)
        | (0b100010 << 23)
        | (u32::from(shift12) << 22)
        | (imm12 << 10)
        | (rn << 5)
        | rd
}

/// `B` with a signed word offset (`imm26`).
#[must_use]
pub fn b(offset_words: i32) -> u32 {
    debug_assert!((-(1 << 25)..(1 << 25)).contains(&offset_words));
    0x1400_0000 | (offset_words as u32 & 0x03ff_ffff)
}

/// `BL` with a signed word offset.
#[must_use]
pub fn bl(offset_words: i32) -> u32 {
    debug_assert!((-(1 << 25)..(1 << 25)).contains(&offset_words));
    0x9400_0000 | (offset_words as u32 & 0x03ff_ffff)
}

/// `BR <Xn>`.
#[must_use]
pub fn br(rn: u32) -> u32 {
    0xd61f_0000 | (rn << 5)
}

/// `BLR <Xn>`.
#[must_use]
pub fn blr(rn: u32) -> u32 {
    0xd63f_0000 | (rn << 5)
}

/// `RET <Xn>`.
#[must_use]
pub fn ret(rn: u32) -> u32 {
    0xd65f_0000 | (rn << 5)
}

/// `CBZ`/`CBNZ` with a signed word offset (`imm19`).
#[must_use]
pub fn cbz(sf: bool, nonzero: bool, offset_words: i32, rt: u32) -> u32 {
    debug_assert!((-(1 << 18)..(1 << 18)).contains(&offset_words) && rt < 32);
    (u32::from(sf) << 31)
        | 0x3400_0000
        | (u32::from(nonzero) << 24)
        | ((offset_words as u32 & 0x7_ffff) << 5)
        | rt
}

/// `TBZ`/`TBNZ` with a signed word offset (`imm14`); `bit` is 0..=63 (`bit >= 32` selects an X
/// register, as required by the architecture).
#[must_use]
pub fn tbz(nonzero: bool, bit: u32, offset_words: i32, rt: u32) -> u32 {
    debug_assert!(bit < 64 && (-(1 << 13)..(1 << 13)).contains(&offset_words) && rt < 32);
    ((bit >> 5) << 31)
        | 0x3600_0000
        | (u32::from(nonzero) << 24)
        | ((bit & 31) << 19)
        | ((offset_words as u32 & 0x3fff) << 5)
        | rt
}

/// `B.<cond>` with a signed word offset (`imm19`); `cond` 0..=15 (14 = AL, 15 = NV).
#[must_use]
pub fn b_cond(cond: u32, offset_words: i32) -> u32 {
    debug_assert!(cond < 16 && (-(1 << 18)..(1 << 18)).contains(&offset_words));
    0x5400_0000 | ((offset_words as u32 & 0x7_ffff) << 5) | cond
}

/// `NOP`.
pub const NOP: u32 = 0xd503_201f;

/// `UDF #imm16` (permanently undefined).
#[must_use]
pub fn udf(imm16: u32) -> u32 {
    debug_assert!(imm16 < 0x1_0000);
    imm16
}

/// `BRK #imm16`.
#[must_use]
pub fn brk(imm16: u32) -> u32 {
    debug_assert!(imm16 < 0x1_0000);
    0xd420_0000 | (imm16 << 5)
}

/// Words that load the 64-bit constant `value` into `Xd`: `MOVZ` for the lowest non-zero
/// halfword, `MOVK` for the other non-zero halfwords (`MOVZ Xd, #0` for zero).
#[must_use]
pub fn load_const(rd: u32, value: u64) -> Vec<u32> {
    let mut words = Vec::with_capacity(4);
    for hw in 0..4u32 {
        let chunk = ((value >> (16 * hw)) & 0xffff) as u32;
        if chunk == 0 {
            continue;
        }
        let op = if words.is_empty() {
            MoveWide::Z
        } else {
            MoveWide::K
        };
        words.push(move_wide(op, true, hw, chunk, rd));
    }
    if words.is_empty() {
        words.push(move_wide(MoveWide::Z, true, 0, 0, rd));
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(assembly, word)` pairs produced by `llvm-mc-18 --triple=aarch64 -show-encoding`.
    const LLVM: [(&str, u32); 35] = [
        ("movz x0, #1", 0xd280_0020),
        ("movz w5, #0xffff, lsl #16", 0x52bf_ffe5),
        ("movz x30, #0xabcd, lsl #48", 0xd2f5_79be),
        ("movn x1, #0x1234, lsl #32", 0x92c2_4681),
        ("movn w2, #0", 0x1280_0002),
        ("movk x3, #0xfeed, lsl #16", 0xf2bf_dda3),
        ("movk w4, #7", 0x7280_00e4),
        ("add x0, x1, #1", 0x9100_0420),
        ("add w0, w1, #0xfff, lsl #12", 0x117f_fc20),
        ("sub sp, sp, #16", 0xd100_43ff),
        ("adds x2, x3, #5", 0xb100_1462),
        ("subs w2, w3, #0x800, lsl #12", 0x7160_0062),
        ("cmp x4, #1", 0xf100_049f),
        ("cmn w4, #0xfff", 0x313f_fc9f),
        ("add sp, x9, #0", 0x9100_013f),
        ("b #8", 0x1400_0002),
        ("b #-4", 0x17ff_ffff),
        ("bl #0x1000", 0x9400_0400),
        ("br x5", 0xd61f_00a0),
        ("blr x30", 0xd63f_03c0),
        ("ret", 0xd65f_03c0),
        ("ret x7", 0xd65f_00e0),
        ("cbz x1, #8", 0xb400_0041),
        ("cbnz w2, #0x10", 0x3500_0082),
        ("cbz xzr, #4", 0xb400_003f),
        ("b.eq #8", 0x5400_0040),
        ("b.nv #12", 0x5400_006f),
        ("b.ne #0x40", 0x5400_0201),
        ("b.hi #-8", 0x54ff_ffc8),
        ("nop", 0xd503_201f),
        ("udf #0xffff", 0x0000_ffff),
        ("brk #0xf001", 0xd43e_0020),
        ("brk #1", 0xd420_0020),
        ("tbz x3, #63, #8", 0xb6f8_0043),
        ("tbnz w4, #0, #12", 0x3700_0064),
    ];

    #[test]
    fn encoders_match_llvm_mc() {
        use MoveWide::{K, N, Z};
        let ours: [u32; 35] = [
            move_wide(Z, true, 0, 1, 0),
            move_wide(Z, false, 1, 0xffff, 5),
            move_wide(Z, true, 3, 0xabcd, 30),
            move_wide(N, true, 2, 0x1234, 1),
            move_wide(N, false, 0, 0, 2),
            move_wide(K, true, 1, 0xfeed, 3),
            move_wide(K, false, 0, 7, 4),
            add_sub_imm(true, false, false, false, 1, 1, 0),
            add_sub_imm(false, false, false, true, 0xfff, 1, 0),
            add_sub_imm(true, true, false, false, 16, 31, 31),
            add_sub_imm(true, false, true, false, 5, 3, 2),
            add_sub_imm(false, true, true, true, 0x800, 3, 2),
            add_sub_imm(true, true, true, false, 1, 4, 31),
            add_sub_imm(false, false, true, false, 0xfff, 4, 31),
            add_sub_imm(true, false, false, false, 0, 9, 31),
            b(2),
            b(-1),
            bl(0x400),
            br(5),
            blr(30),
            ret(30),
            ret(7),
            cbz(true, false, 2, 1),
            cbz(false, true, 4, 2),
            cbz(true, false, 1, 31),
            b_cond(0, 2),
            b_cond(15, 3),
            b_cond(1, 0x10),
            b_cond(8, -2),
            NOP,
            udf(0xffff),
            brk(0xf001),
            brk(1),
            tbz(false, 63, 2, 3),
            tbz(true, 0, 3, 4),
        ];
        for (i, (asm, word)) in LLVM.iter().enumerate() {
            assert_eq!(
                ours[i], *word,
                "{asm}: ours {:#010x}, llvm {word:#010x}",
                ours[i]
            );
        }
    }

    #[test]
    fn load_const_builds_constants() {
        assert_eq!(
            load_const(3, 0),
            vec![move_wide(MoveWide::Z, true, 0, 0, 3)]
        );
        assert_eq!(
            load_const(3, 0x1234),
            vec![move_wide(MoveWide::Z, true, 0, 0x1234, 3)]
        );
        assert_eq!(
            load_const(3, 0x10_0000_0108),
            vec![
                move_wide(MoveWide::Z, true, 0, 0x0108, 3),
                move_wide(MoveWide::K, true, 2, 0x10, 3),
            ]
        );
    }
}
