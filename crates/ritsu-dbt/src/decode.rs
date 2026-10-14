//! A64 instruction decoder.
//!
//! [`decode`] turns a 32-bit instruction word into an [`Inst`]. It is a pure function:
//! no guest state, no allocation, never panics; every word that is not (yet) understood
//! becomes [`Inst::Unknown`], which executors turn into an Undefined Instruction
//! exception. This is the first stage of the single semantic source of ADR-0006
//! (decode → semantics → T0/T1/T2).
//!
//! Encodings follow the Arm Architecture Reference Manual (A64 encoding index).
//! The top-level dispatch uses `op0 = word[28:25]`:
//!
//! | `op0`  | Class                                                     |
//! |--------|-----------------------------------------------------------|
//! | `100x` | Data processing — immediate                               |
//! | `101x` | Branches, exception generating and system instructions    |
//!
//! Covered subset (M0): `MOVN/MOVZ/MOVK`, `ADD/ADDS/SUB/SUBS` (immediate),
//! `B`, `BL`, `BR`, `BLR`, `RET`, `CBZ/CBNZ`, `B.cond`, `NOP`.
//!
//! Register 31 is resolved here, per operand (see [`GpReg`]):
//! `ADD/SUB` (immediate) use `Rd|SP` and `Rn|SP`; `ADDS/SUBS` use `Rd|ZR` and `Rn|SP`;
//! `MOV*`, `CBZ/CBNZ`, `BR/BLR/RET` use the zero register.

use crate::cpu::{GpReg, Nzcv};

/// A condition code (`cond` field) as used by `B.cond`, `CSEL`, `CCMP`, …
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cond {
    /// `0000` Equal: `Z == 1`.
    Eq,
    /// `0001` Not equal: `Z == 0`.
    Ne,
    /// `0010` Carry set / unsigned higher or same: `C == 1`.
    Cs,
    /// `0011` Carry clear / unsigned lower: `C == 0`.
    Cc,
    /// `0100` Minus / negative: `N == 1`.
    Mi,
    /// `0101` Plus / positive or zero: `N == 0`.
    Pl,
    /// `0110` Overflow: `V == 1`.
    Vs,
    /// `0111` No overflow: `V == 0`.
    Vc,
    /// `1000` Unsigned higher: `C == 1 && Z == 0`.
    Hi,
    /// `1001` Unsigned lower or same: `!(C == 1 && Z == 0)`.
    Ls,
    /// `1010` Signed greater or equal: `N == V`.
    Ge,
    /// `1011` Signed less than: `N != V`.
    Lt,
    /// `1100` Signed greater than: `Z == 0 && N == V`.
    Gt,
    /// `1101` Signed less or equal: `!(Z == 0 && N == V)`.
    Le,
    /// `1110` Always.
    Al,
    /// `1111` Always (behaves exactly like `AL` in A64).
    Nv,
}

impl Cond {
    /// All conditions in encoding order.
    pub const ALL: [Self; 16] = [
        Self::Eq,
        Self::Ne,
        Self::Cs,
        Self::Cc,
        Self::Mi,
        Self::Pl,
        Self::Vs,
        Self::Vc,
        Self::Hi,
        Self::Ls,
        Self::Ge,
        Self::Lt,
        Self::Gt,
        Self::Le,
        Self::Al,
        Self::Nv,
    ];

    /// Decodes a 4-bit condition field (higher bits are ignored).
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self::ALL[(bits & 0xf) as usize]
    }

    /// The 4-bit encoding.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self as u32
    }

    /// Evaluates the condition, exactly as the Arm ARM `ConditionHolds()` pseudocode:
    /// `cond[3:1]` selects the base test, `cond[0]` inverts it except for `1111`.
    #[must_use]
    pub const fn holds(self, f: Nzcv) -> bool {
        let bits = self.bits();
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
}

/// `ADD` or `SUB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddSubOp {
    /// Addition.
    Add,
    /// Subtraction (`Rn + NOT(imm) + 1`).
    Sub,
}

/// The three move-wide-immediate operations (`opc` field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveWideOp {
    /// `opc = 00`: `Rd = NOT(imm16 << shift)`.
    Movn,
    /// `opc = 10`: `Rd = imm16 << shift`.
    Movz,
    /// `opc = 11`: replace 16 bits of `Rd` at `shift`, keep the rest.
    Movk,
}

/// A decoded A64 instruction (M0 subset).
///
/// Branch offsets are byte offsets relative to the address of the instruction itself,
/// already sign-extended and scaled by 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Inst {
    /// `MOVN/MOVZ/MOVK` (move wide immediate).
    MoveWide {
        /// Which of the three operations.
        op: MoveWideOp,
        /// `true` = 64-bit (`Xd`), `false` = 32-bit (`Wd`).
        sf: bool,
        /// Left shift in bits: `hw * 16` (0/16 for 32-bit, 0/16/32/48 for 64-bit).
        shift: u8,
        /// The 16-bit immediate.
        imm16: u16,
        /// Destination (`Rd`, 31 = `ZR`).
        rd: GpReg,
    },
    /// `ADD/ADDS/SUB/SUBS` (immediate). `CMP`, `CMN` and `MOV (to/from SP)` are aliases.
    AddSubImm {
        /// Add or subtract.
        op: AddSubOp,
        /// `true` = 64-bit, `false` = 32-bit.
        sf: bool,
        /// `S` bit: update `NZCV` (`ADDS/SUBS`).
        set_flags: bool,
        /// The unsigned 12-bit immediate.
        imm12: u16,
        /// `sh` bit: immediate is shifted left by 12.
        shift12: bool,
        /// Destination: `SP` if `!set_flags`, else `ZR`, when encoded as 31.
        rd: GpReg,
        /// First operand: always `SP` when encoded as 31.
        rn: GpReg,
    },
    /// `B label`.
    B {
        /// Byte offset from this instruction (±128 MiB).
        offset: i64,
    },
    /// `BL label`: branch with link (`X30 = PC + 4`).
    Bl {
        /// Byte offset from this instruction (±128 MiB).
        offset: i64,
    },
    /// `BR Xn`.
    Br {
        /// Target register (31 = `XZR`).
        rn: GpReg,
    },
    /// `BLR Xn`: branch to register with link.
    Blr {
        /// Target register (31 = `XZR`).
        rn: GpReg,
    },
    /// `RET {Xn}` (default `X30`).
    Ret {
        /// Return-address register (31 = `XZR`).
        rn: GpReg,
    },
    /// `CBZ/CBNZ Rt, label`.
    CompareBranch {
        /// `true` = `CBNZ`, `false` = `CBZ`.
        nonzero: bool,
        /// `true` = test `Xt`, `false` = test `Wt`.
        sf: bool,
        /// Tested register (31 = `ZR`).
        rt: GpReg,
        /// Byte offset from this instruction (±1 MiB).
        offset: i64,
    },
    /// `B.cond label`.
    BCond {
        /// Branch condition.
        cond: Cond,
        /// Byte offset from this instruction (±1 MiB).
        offset: i64,
    },
    /// `NOP` (`HINT #0`). Other hints are deliberately *not* folded into this: several
    /// of them (`PACIASP`, `AUTIASP`, `BTI`, …) have real semantics on arm64e.
    Nop,
    /// Any word outside the decoded subset, including unallocated encodings.
    Unknown(u32),
}

/// Decodes one A64 instruction word. Never panics.
#[must_use]
pub fn decode(word: u32) -> Inst {
    match (word >> 25) & 0xf {
        0b1000 | 0b1001 => decode_dp_imm(word),
        0b1010 | 0b1011 => decode_branch_sys(word),
        _ => Inst::Unknown(word),
    }
}

/// Extracts `word[lsb + len - 1 : lsb]`.
const fn bits(word: u32, lsb: u32, len: u32) -> u32 {
    (word >> lsb) & ((1 << len) - 1)
}

/// Sign-extends the low `len` bits of `value` and multiplies by 4.
const fn branch_offset(value: u32, len: u32) -> i64 {
    let shift = 64 - len;
    (((value as u64) << shift) as i64 >> shift) * 4
}

/// Data processing — immediate (`op0 = 100x`), sub-class in `word[25:23]`.
fn decode_dp_imm(word: u32) -> Inst {
    match bits(word, 23, 3) {
        0b010 => decode_add_sub_imm(word),
        0b101 => decode_move_wide(word),
        _ => Inst::Unknown(word),
    }
}

/// `sf:1 op:1 S:1 100010 sh:1 imm12:12 Rn:5 Rd:5`
fn decode_add_sub_imm(word: u32) -> Inst {
    let set_flags = bits(word, 29, 1) == 1;
    Inst::AddSubImm {
        op: if bits(word, 30, 1) == 0 {
            AddSubOp::Add
        } else {
            AddSubOp::Sub
        },
        sf: bits(word, 31, 1) == 1,
        set_flags,
        // 12-bit field always fits.
        imm12: bits(word, 10, 12) as u16,
        shift12: bits(word, 22, 1) == 1,
        rd: if set_flags {
            GpReg::zr_from_field(bits(word, 0, 5))
        } else {
            GpReg::sp_from_field(bits(word, 0, 5))
        },
        rn: GpReg::sp_from_field(bits(word, 5, 5)),
    }
}

/// `sf:1 opc:2 100101 hw:2 imm16:16 Rd:5`
fn decode_move_wide(word: u32) -> Inst {
    let sf = bits(word, 31, 1) == 1;
    let hw = bits(word, 21, 2);
    let op = match bits(word, 29, 2) {
        0b00 => MoveWideOp::Movn,
        0b10 => MoveWideOp::Movz,
        0b11 => MoveWideOp::Movk,
        _ => return Inst::Unknown(word), // opc = 01 is unallocated
    };
    if !sf && hw >= 2 {
        return Inst::Unknown(word); // sf = 0 && hw<1> = 1 is unallocated
    }
    Inst::MoveWide {
        op,
        sf,
        // hw <= 3, so the shift is at most 48.
        shift: (hw * 16) as u8,
        imm16: bits(word, 5, 16) as u16,
        rd: GpReg::zr_from_field(bits(word, 0, 5)),
    }
}

/// Branches, exception generating and system instructions (`op0 = 101x`).
fn decode_branch_sys(word: u32) -> Inst {
    // Unconditional branch (immediate): op:1 00101 imm26:26
    if word & 0x7c00_0000 == 0x1400_0000 {
        let offset = branch_offset(bits(word, 0, 26), 26);
        return if bits(word, 31, 1) == 0 {
            Inst::B { offset }
        } else {
            Inst::Bl { offset }
        };
    }
    // Compare and branch (immediate): sf:1 011010 op:1 imm19:19 Rt:5
    if word & 0x7e00_0000 == 0x3400_0000 {
        return Inst::CompareBranch {
            nonzero: bits(word, 24, 1) == 1,
            sf: bits(word, 31, 1) == 1,
            rt: GpReg::zr_from_field(bits(word, 0, 5)),
            offset: branch_offset(bits(word, 5, 19), 19),
        };
    }
    // Conditional branch (immediate): 0101010 o1=0 imm19:19 o0=0 cond:4
    // (o0 = 1 is BC.cond, FEAT_HBC, which Ritsu does not report.)
    if word & 0xff00_0010 == 0x5400_0000 {
        return Inst::BCond {
            cond: Cond::from_bits(bits(word, 0, 4)),
            offset: branch_offset(bits(word, 5, 19), 19),
        };
    }
    // HINT #0
    if word == 0xd503_201f {
        return Inst::Nop;
    }
    // Unconditional branch (register): 1101011 opc:4 op2=11111 op3=000000 Rn:5 op4=00000
    // Only the non-authenticating forms (op3 = 0, op4 = 0) are decoded here.
    let rn = GpReg::zr_from_field(bits(word, 5, 5));
    match word & 0xffff_fc1f {
        0xd61f_0000 => Inst::Br { rn },
        0xd63f_0000 => Inst::Blr { rn },
        0xd65f_0000 => Inst::Ret { rn },
        _ => Inst::Unknown(word),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm;
    use crate::cpu::XReg;

    fn x(n: u8) -> GpReg {
        GpReg::x(n).unwrap()
    }

    #[test]
    fn well_known_encodings() {
        assert_eq!(decode(0xd503_201f), Inst::Nop);
        assert_eq!(
            decode(0xd65f_03c0),
            Inst::Ret {
                rn: GpReg::X(XReg::LR)
            }
        );
        // add x1, x1, #1
        assert_eq!(
            decode(0x9100_0421),
            Inst::AddSubImm {
                op: AddSubOp::Add,
                sf: true,
                set_flags: false,
                imm12: 1,
                shift12: false,
                rd: x(1),
                rn: x(1)
            }
        );
        // mov x0, #0 (movz)
        assert_eq!(
            decode(0xd280_0000),
            Inst::MoveWide {
                op: MoveWideOp::Movz,
                sf: true,
                shift: 0,
                imm16: 0,
                rd: x(0)
            }
        );
        // mov w0, #1
        assert_eq!(
            decode(0x5280_0020),
            Inst::MoveWide {
                op: MoveWideOp::Movz,
                sf: false,
                shift: 0,
                imm16: 1,
                rd: x(0)
            }
        );
        // mov x0, #-1 (movn x0, #0)
        assert_eq!(
            decode(0x9280_0000),
            Inst::MoveWide {
                op: MoveWideOp::Movn,
                sf: true,
                shift: 0,
                imm16: 0,
                rd: x(0)
            }
        );
        // movk x0, #0x1234, lsl #16
        assert_eq!(
            decode(0xf2a2_4680),
            Inst::MoveWide {
                op: MoveWideOp::Movk,
                sf: true,
                shift: 16,
                imm16: 0x1234,
                rd: x(0)
            }
        );
        // b . / bl . / b .-4
        assert_eq!(decode(0x1400_0000), Inst::B { offset: 0 });
        assert_eq!(decode(0x9400_0000), Inst::Bl { offset: 0 });
        assert_eq!(decode(0x17ff_ffff), Inst::B { offset: -4 });
        // br x17 / blr x8 / br x0
        assert_eq!(decode(0xd61f_0220), Inst::Br { rn: x(17) });
        assert_eq!(decode(0xd63f_0100), Inst::Blr { rn: x(8) });
        assert_eq!(decode(0xd61f_0000), Inst::Br { rn: x(0) });
        // b.ne . / b.ne .-4
        assert_eq!(
            decode(0x5400_0001),
            Inst::BCond {
                cond: Cond::Ne,
                offset: 0
            }
        );
        assert_eq!(
            decode(0x54ff_ffe1),
            Inst::BCond {
                cond: Cond::Ne,
                offset: -4
            }
        );
        // cbz x0, .+8 / cbnz w0, .
        assert_eq!(
            decode(0xb400_0040),
            Inst::CompareBranch {
                nonzero: false,
                sf: true,
                rt: x(0),
                offset: 8
            }
        );
        assert_eq!(
            decode(0x3500_0000),
            Inst::CompareBranch {
                nonzero: true,
                sf: false,
                rt: x(0),
                offset: 0
            }
        );
    }

    #[test]
    fn register_31_is_sp_for_add_sub_imm() {
        // mov sp, x0 == add sp, x0, #0
        assert!(matches!(
            decode(0x9100_001f),
            Inst::AddSubImm { rd: GpReg::Sp, .. }
        ));
        // mov x0, sp == add x0, sp, #0
        assert!(matches!(
            decode(0x9100_03e0),
            Inst::AddSubImm { rn: GpReg::Sp, .. }
        ));
        // sub sp, sp, #16
        assert_eq!(
            decode(0xd100_43ff),
            Inst::AddSubImm {
                op: AddSubOp::Sub,
                sf: true,
                set_flags: false,
                imm12: 16,
                shift12: false,
                rd: GpReg::Sp,
                rn: GpReg::Sp
            }
        );
        // add sp, sp, #16
        assert!(matches!(
            decode(0x9100_43ff),
            Inst::AddSubImm {
                op: AddSubOp::Add,
                rd: GpReg::Sp,
                rn: GpReg::Sp,
                imm12: 16,
                ..
            }
        ));
    }

    #[test]
    fn register_31_is_zr_for_flag_setting_and_moves() {
        // cmp x0, #0 == subs xzr, x0, #0
        assert_eq!(
            decode(0xf100_001f),
            Inst::AddSubImm {
                op: AddSubOp::Sub,
                sf: true,
                set_flags: true,
                imm12: 0,
                shift12: false,
                rd: GpReg::Zr,
                rn: x(0)
            }
        );
        // cmp w0, #1 / cmn w0, #1
        assert!(matches!(
            decode(0x7100_041f),
            Inst::AddSubImm {
                op: AddSubOp::Sub,
                sf: false,
                rd: GpReg::Zr,
                ..
            }
        ));
        assert!(matches!(
            decode(0x3100_041f),
            Inst::AddSubImm {
                op: AddSubOp::Add,
                sf: false,
                rd: GpReg::Zr,
                ..
            }
        ));
        // cmp sp, #0: Rn = 31 is SP even for SUBS.
        assert!(matches!(
            decode(0xf100_03ff),
            Inst::AddSubImm {
                rd: GpReg::Zr,
                rn: GpReg::Sp,
                ..
            }
        ));
        // movz xzr, #5
        assert!(matches!(
            decode(0xd280_00bf),
            Inst::MoveWide { rd: GpReg::Zr, .. }
        ));
        // cbz xzr / br xzr / ret xzr
        assert!(matches!(
            decode(0xb400_001f),
            Inst::CompareBranch { rt: GpReg::Zr, .. }
        ));
        assert_eq!(decode(0xd61f_03e0), Inst::Br { rn: GpReg::Zr });
        assert_eq!(decode(0xd65f_03e0), Inst::Ret { rn: GpReg::Zr });
    }

    #[test]
    fn shifted_immediate() {
        // adds x0, x1, #1, lsl #12
        assert_eq!(
            decode(0xb140_0420),
            Inst::AddSubImm {
                op: AddSubOp::Add,
                sf: true,
                set_flags: true,
                imm12: 1,
                shift12: true,
                rd: x(0),
                rn: x(1)
            }
        );
    }

    #[test]
    fn unallocated_and_unsupported_are_unknown() {
        // Move wide with opc = 01.
        assert_eq!(decode(0xb280_0000), Inst::Unknown(0xb280_0000));
        // movz w0, #0, lsl #32 (sf = 0, hw = 2) is unallocated.
        let w = asm::move_wide(false, 0b10, 2, 0, 0);
        assert_eq!(decode(w), Inst::Unknown(w));
        // BC.cond (o0 = 1) is FEAT_HBC, not reported.
        assert_eq!(decode(0x5400_0010), Inst::Unknown(0x5400_0010));
        // B.cond with o1 = 1 is unallocated.
        assert_eq!(decode(0x5500_0000), Inst::Unknown(0x5500_0000));
        // retaa / braa / yield / paciasp / udf #0 / all-ones.
        for w in [
            0xd65f_0bff,
            0xd71f_0800,
            0xd503_203f,
            0xd503_233f,
            0x0000_0000,
            0xffff_ffff,
        ] {
            assert_eq!(decode(w), Inst::Unknown(w), "{w:#010x}");
        }
        // ADDG (add/sub immediate with tags, word[25:23] = 011) is not ADD.
        assert_eq!(decode(0x9180_0000), Inst::Unknown(0x9180_0000));
    }

    #[test]
    fn encoder_matches_known_words() {
        // Cross-check the test encoder against hand-verified constants, so that tests
        // built from it are trustworthy.
        assert_eq!(
            asm::add_sub_imm(true, false, false, false, 1, 1, 1),
            0x9100_0421
        );
        assert_eq!(
            asm::add_sub_imm(true, true, false, false, 16, 31, 31),
            0xd100_43ff
        );
        assert_eq!(
            asm::add_sub_imm(true, true, true, false, 0, 0, 31),
            0xf100_001f
        );
        assert_eq!(asm::move_wide(true, 0b10, 0, 0, 0), 0xd280_0000);
        assert_eq!(asm::move_wide(false, 0b10, 0, 1, 0), 0x5280_0020);
        assert_eq!(asm::move_wide(true, 0b11, 1, 0x1234, 0), 0xf2a2_4680);
        assert_eq!(asm::b(false, -4), 0x17ff_ffff);
        assert_eq!(asm::b(true, 0), 0x9400_0000);
        assert_eq!(asm::b_cond(Cond::Ne, -4), 0x54ff_ffe1);
        assert_eq!(asm::cb(true, false, 8, 0), 0xb400_0040);
        assert_eq!(asm::br(0b0010, 30), 0xd65f_03c0);
        assert_eq!(asm::br(0b0001, 8), 0xd63f_0100);
    }

    #[test]
    fn branch_offset_extremes() {
        // imm26 = 0x2000000 → -2^25 * 4 = -128 MiB; 0x1ffffff → +128 MiB - 4.
        assert_eq!(
            decode(0x1600_0000),
            Inst::B {
                offset: -(128 << 20)
            }
        );
        assert_eq!(
            decode(0x15ff_ffff),
            Inst::B {
                offset: (128 << 20) - 4
            }
        );
        // imm19 extremes for B.cond: ±1 MiB.
        assert_eq!(asm::b_cond(Cond::Al, -(1 << 20)), 0x5480_000e);
        assert_eq!(
            decode(0x5480_000e),
            Inst::BCond {
                cond: Cond::Al,
                offset: -(1 << 20)
            }
        );
        assert_eq!(
            decode(asm::b_cond(Cond::Eq, (1 << 20) - 4)),
            Inst::BCond {
                cond: Cond::Eq,
                offset: (1 << 20) - 4
            }
        );
    }

    #[test]
    fn decode_encode_roundtrip_on_random_words() {
        // For every word in the covered classes, re-encoding the decoded instruction must
        // give back the exact word: no field is lost or misplaced. Words are random but
        // forced into each class by (mask, pattern).
        const CLASSES: [(u32, u32); 7] = [
            (0x1f80_0000, 0x1100_0000), // add/sub immediate
            (0x1f80_0000, 0x1280_0000), // move wide
            (0x7c00_0000, 0x1400_0000), // b/bl
            (0x7e00_0000, 0x3400_0000), // cbz/cbnz
            (0xff00_0010, 0x5400_0000), // b.cond
            (0xffff_fc1f, 0xd61f_0000), // br (rn varies)
            (0xfe9f_fc1f, 0xd61f_0000), // br/blr/ret (+ unallocated opc values)
        ];
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let mut decoded = 0u32;
        for _ in 0..20_000 {
            for (mask, pattern) in CLASSES {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                let w = (rng as u32 & !mask) | pattern;
                let inst = decode(w);
                if inst != Inst::Unknown(w) {
                    decoded += 1;
                    assert_eq!(asm::encode(&inst), Some(w), "{w:#010x} → {inst:?}");
                }
            }
        }
        assert!(decoded > 100_000);
    }

    #[test]
    fn decode_never_panics_on_sampled_space() {
        // Exhaustive over the top 12 bits with fixed low bits; plus a random sample.
        for hi in 0..(1u32 << 12) {
            for lo in [0u32, 0x1f, 0xfffff, 0x3ff, 0x7_ffe0] {
                let _ = decode((hi << 20) | lo);
            }
        }
    }
}
