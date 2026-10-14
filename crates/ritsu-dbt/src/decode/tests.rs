//! Decoder tests: known words, register-31 rules, unallocated encodings, encoder
//! roundtrip, exhaustive `DecodeBitMasks`.

use super::*;
use crate::asm;
use crate::cpu::{SysReg, XReg};
use crate::sem::Ty;
use crate::testutil::{CLASSES, Rng};

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
    // retaa / braa / udf #0 / all-ones.
    for w in [0xd65f_0bff, 0xd71f_0800, 0x0000_0000, 0xffff_ffff] {
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
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for (name, class) in CLASSES {
        let mut decoded = 0u32;
        for _ in 0..20_000 {
            let w = rng.word_in(class);
            let inst = decode(w);
            if inst != Inst::Unknown(w) {
                decoded += 1;
                assert_eq!(asm::encode(&inst), Some(w), "{w:#010x} → {inst:?}");
            }
        }
        // Every class except the catch-all decodes a good share of its words.
        assert!(
            name == "anything" || decoded > 100,
            "{name}: only {decoded} decoded"
        );
    }
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

// ---------------------------------------------------------------------------------
// WP-DBT-02 classes
// ---------------------------------------------------------------------------------

/// `DecodeBitMasks` transcribed bit by bit (vectors of booleans, LSB first), written
/// independently of `bitmask.rs`.
fn o_decode_bit_masks(
    n: bool,
    imms: u32,
    immr: u32,
    immediate: bool,
    m: u32,
) -> Option<(u64, u64)> {
    // len = HighestSetBit(immN:NOT(imms))
    let mut field = [false; 7];
    for (i, f) in field.iter_mut().enumerate().take(6) {
        *f = (imms >> i) & 1 == 0;
    }
    field[6] = n;
    let len = (0..7).rev().find(|&i| field[i])? as u32;
    if len < 1 || (1 << len) > m {
        return None;
    }
    let levels = (1u32 << len) - 1;
    if immediate && (imms & levels) == levels {
        return None;
    }
    let s = (imms & levels) as usize;
    let r = (immr & levels) as usize;
    let esize = 1usize << len;
    let d = ((s as i64 - r as i64).rem_euclid(1 << len)) as usize;
    let welem: Vec<bool> = (0..esize).map(|i| i <= s).collect();
    let telem: Vec<bool> = (0..esize).map(|i| i <= d).collect();
    // ROR(welem, r): result bit i = welem bit (i + r) mod esize.
    let rot: Vec<bool> = (0..esize).map(|i| welem[(i + r) % esize]).collect();
    let pack = |e: &Vec<bool>| -> u64 {
        (0..m as usize)
            .filter(|&i| e[i % esize])
            .map(|i| 1u64 << i)
            .sum()
    };
    Some((pack(&rot), pack(&telem)))
}

#[test]
fn decode_bit_masks_exhaustive() {
    let mut valid = [
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    ];
    for (k, m) in [32u32, 64].into_iter().enumerate() {
        for raw in 0..(1u32 << 13) {
            let (n, immr, imms) = (raw >> 12 & 1 == 1, raw >> 6 & 63, raw & 63);
            for immediate in [true, false] {
                let got = decode_bit_masks(n, imms, immr, immediate, m);
                assert_eq!(
                    got,
                    o_decode_bit_masks(n, imms, immr, immediate, m),
                    "N={n} immr={immr} imms={imms} imm={immediate} M={m}"
                );
                if immediate && let Some((wmask, _)) = got {
                    valid[k].insert(wmask);
                }
            }
        }
    }
    // Distinct encodable logical immediates: sum of e * (e - 1) over the element sizes,
    // 1302 for 32 bits and 5334 for 64.
    assert_eq!([valid[0].len(), valid[1].len()], [1302, 5334]);
}

#[test]
fn logical_immediates_decode_through_bitmasks() {
    // and x0, x1, #0xff / orr w2, wzr, #0x55555555 / and sp, x3, #~0xf / ands x4, x5, #1
    let cases: [(u32, LogicOp, bool, u64, GpReg, GpReg); 4] = [
        (0x9240_1c20, LogicOp::And, true, 0xff, x(0), x(1)),
        (
            0x3200_f3e2,
            LogicOp::Orr,
            false,
            0x5555_5555,
            x(2),
            GpReg::Zr,
        ),
        (0x927c_ec7f, LogicOp::And, true, !0xf, GpReg::Sp, x(3)),
        (0xf240_00a4, LogicOp::Ands, true, 1, x(4), x(5)),
    ];
    for (w, op, sf, imm, rd, rn) in cases {
        match decode(w) {
            Inst::LogicalImm(i) => {
                assert_eq!(
                    (i.op, i.sf, i.imm, i.rd, i.rn),
                    (op, sf, imm, rd, rn),
                    "{w:#x}"
                );
            }
            other => panic!("{w:#x}: {other:?}"),
        }
    }
    // ANDS with Rd = 31 is TST (ZR), AND with Rd = 31 writes SP.
    assert!(matches!(
        decode(0xf240_001f),
        Inst::LogicalImm(LogicalImm { rd: GpReg::Zr, .. })
    ));
    assert!(matches!(
        decode(0x9240_001f),
        Inst::LogicalImm(LogicalImm { rd: GpReg::Sp, .. })
    ));
    // sf = 0 with N = 1, and the all-ones element (imms = 111111 with N = 1), are
    // unallocated.
    for w in [0x1240_0000u32, 0x9240_fc00, 0x1200_7c00] {
        assert_eq!(decode(w), Inst::Unknown(w), "{w:#x}");
    }
}

#[test]
fn dp_immediate_known_words() {
    assert_eq!(
        decode(asm::adr(false, 0x10, 0)),
        Inst::Adr {
            page: false,
            imm: 0x10,
            rd: x(0)
        }
    );
    assert_eq!(
        decode(asm::adr(true, -1, 31)),
        Inst::Adr {
            page: true,
            imm: -4096,
            rd: GpReg::Zr
        }
    );
    // Extremes of the 21-bit immediate: ±1 MiB for ADR, ±4 GiB for ADRP.
    assert!(
        matches!(decode(asm::adr(false, -(1 << 20), 1)), Inst::Adr { imm, .. } if imm == -(1 << 20))
    );
    assert!(
        matches!(decode(asm::adr(true, (1 << 20) - 1, 1)), Inst::Adr { imm, .. } if imm == ((1 << 20) - 1) * 4096)
    );
    // sbfiz x0, x1, #1, #32 (sbfm x0, x1, #63, #31)
    match decode(0x937f_7c20) {
        Inst::Bitfield(b) => {
            assert_eq!(
                (b.op, b.sf, b.immr, b.imms),
                (BitfieldOp::Sbfm, true, 63, 31)
            );
        }
        other => panic!("{other:?}"),
    }
    // ubfx w2, w3, #4, #8 / bfi x4, x5, #8, #16
    assert!(matches!(
        decode(0x5304_2c62),
        Inst::Bitfield(Bitfield {
            op: BitfieldOp::Ubfm,
            sf: false,
            immr: 4,
            imms: 11,
            ..
        })
    ));
    assert!(matches!(
        decode(0xb378_3ca4),
        Inst::Bitfield(Bitfield {
            op: BitfieldOp::Bfm,
            sf: true,
            immr: 56,
            imms: 15,
            ..
        })
    ));
    // extr x6, x7, x8, #12
    assert_eq!(
        decode(0x93c8_30e6),
        Inst::Extract(Extract {
            sf: true,
            lsb: 12,
            rd: x(6),
            rn: x(7),
            rm: x(8)
        })
    );
    // Bitfield: opc = 11, N != sf, 32-bit with immr/imms >= 32 are unallocated.
    for w in [
        asm::bitfield(true, 3, 0, 0, 0, 0),
        0x9300_0000,                          // sf = 1, N = 0
        asm::bitfield(false, 2, 32, 0, 0, 0), // immr<5> = 1
        asm::bitfield(false, 2, 0, 32, 0, 0), // imms<5> = 1
        asm::extr(false, 0, 32, 0, 0),        // imms<5> = 1
        0x93c0_0000 | (1 << 21),              // o0 = 1
        0x93c0_0000 | (1 << 29),              // op21 != 0
    ] {
        assert_eq!(decode(w), Inst::Unknown(w), "{w:#x}");
    }
}

#[test]
fn dp_register_known_words() {
    let lr = |w| match decode(w) {
        Inst::LogicalReg(i) => i,
        other => panic!("{w:#x}: {other:?}"),
    };
    // bic x0, x1, x2, lsl #3 / orn w3, w4, w5, ror #31
    let i = lr(0x8a22_0c20);
    assert_eq!(
        (i.op, i.invert, i.shift, i.amount),
        (LogicOp::And, true, Shift::Lsl, 3)
    );
    let i = lr(0x2ae5_7c83);
    assert_eq!(
        (i.op, i.invert, i.sf, i.shift, i.amount),
        (LogicOp::Orr, true, false, Shift::Ror, 31)
    );

    let asr = |w| match decode(w) {
        Inst::AddSubReg(i) => i,
        other => panic!("{w:#x}: {other:?}"),
    };
    // adds x0, x1, x2, asr #63
    let i = asr(0xab82_fc20);
    assert_eq!(
        i.operand,
        RegOperand::Shifted {
            shift: Shift::Asr,
            amount: 63
        }
    );
    assert!(i.set_flags);
    // add x0, sp, w1, uxtw #2: Rn is SP in the extended form.
    let i = asr(0x8b21_4be0);
    assert_eq!((i.rd, i.rn, i.rm), (x(0), GpReg::Sp, x(1)));
    assert_eq!(
        i.operand,
        RegOperand::Extended {
            extend: Extend::Uxtw,
            amount: 2
        }
    );
    // cmp sp, x5, lsl #4 (subs xzr, sp, x5, uxtx #4): Rd = ZR because flags are set.
    let i = asr(0xeb25_73ff);
    assert_eq!((i.rd, i.rn), (GpReg::Zr, GpReg::Sp));
    // add sp, sp, x1 (extended, uxtx): Rd = SP.
    assert_eq!(
        asr(asm::add_sub_ext(true, false, false, 1, 3, 0, 31, 31)).rd,
        GpReg::Sp
    );
    // Shifted form: register 31 is ZR everywhere.
    let i = asr(asm::add_sub_shifted(true, false, false, 0, 31, 0, 31, 31));
    assert_eq!((i.rd, i.rn, i.rm), (GpReg::Zr, GpReg::Zr, GpReg::Zr));

    assert_eq!(
        decode(0x9a02_0020),
        Inst::AddSubCarry(AddSubCarry {
            op: AddSubOp::Add,
            sf: true,
            set_flags: false,
            rd: x(0),
            rn: x(1),
            rm: x(2)
        })
    );
    assert!(matches!(
        decode(0x7a05_0083),
        Inst::AddSubCarry(AddSubCarry {
            op: AddSubOp::Sub,
            sf: false,
            set_flags: true,
            ..
        })
    ));
    // ccmp x0, #5, #10, ne / ccmn w1, w2, #3, lt
    assert_eq!(
        decode(0xfa45_180a),
        Inst::CondCompare(CondCompare {
            op: AddSubOp::Sub,
            sf: true,
            cond: Cond::Ne,
            nzcv: 10,
            rn: x(0),
            rhs: CondCmpRhs::Imm(5)
        })
    );
    assert_eq!(
        decode(0x3a42_b023),
        Inst::CondCompare(CondCompare {
            op: AddSubOp::Add,
            sf: false,
            cond: Cond::Lt,
            nzcv: 3,
            rn: x(1),
            rhs: CondCmpRhs::Reg(x(2))
        })
    );
    for (w, op) in [
        (0x9a82_0020u32, CondSelOp::Csel),
        (0x1a85_8483, CondSelOp::Csinc),
        (0xda88_a0e6, CondSelOp::Csinv),
        (0x5a8b_6549, CondSelOp::Csneg),
    ] {
        assert!(
            matches!(decode(w), Inst::CondSelect(c) if c.op == op),
            "{w:#x}"
        );
    }
    for (w, op, sf) in [
        (0x9b02_0c20u32, MulOp::Madd, true),
        (0x1b06_9ca4, MulOp::Msub, false),
        (0x9b22_0c20, MulOp::Smaddl, true),
        (0x9ba6_9ca4, MulOp::Umsubl, true),
        (0x9b4a_7d28, MulOp::Smulh, true),
        (0x9bcd_7d8b, MulOp::Umulh, true),
    ] {
        assert!(
            matches!(decode(w), Inst::MulAdd(m) if m.op == op && m.sf == sf),
            "{w:#x}"
        );
    }
    for (w, op, sf) in [
        (0x1ac2_0820u32, DpOp2::Udiv, false),
        (0x9ac5_0c83, DpOp2::Sdiv, true),
        (0x9ac2_2020, DpOp2::Lslv, true),
        (0x1ac5_2c83, DpOp2::Rorv, false),
    ] {
        assert!(
            matches!(decode(w), Inst::DataProc2(d) if d.op == op && d.sf == sf),
            "{w:#x}"
        );
    }
    for (w, op, sf) in [
        (0xdac0_0020u32, DpOp1::Rbit, true),
        (0x5ac0_0462, DpOp1::Rev16, false),
        (0xdac0_08a4, DpOp1::Rev32, true),
        (0xdac0_0ce6, DpOp1::Rev, true),
        (0x5ac0_0928, DpOp1::Rev, false),
        (0xdac0_116a, DpOp1::Clz, true),
        (0x5ac0_15ac, DpOp1::Cls, false),
    ] {
        assert!(
            matches!(decode(w), Inst::DataProc1(d) if d.op == op && d.sf == sf),
            "{w:#x}"
        );
    }
}

#[test]
fn dp_register_unallocated() {
    for w in [
        asm::logical_reg(false, 0, false, 0, 1, 32, 2, 3), // sf = 0, imm6<5> = 1
        asm::add_sub_shifted(true, false, false, 3, 1, 0, 2, 3), // shift = ROR
        asm::add_sub_shifted(false, false, false, 0, 1, 32, 2, 3),
        asm::add_sub_ext(true, false, false, 1, 0, 5, 2, 3), // imm3 = 5
        asm::add_sub_ext(true, false, false, 1, 0, 0, 2, 3) | (1 << 22), // opt != 00
        asm::adc(true, false, false, 1, 2, 3) | (1 << 10),   // RMIF/SETF space
        asm::ccmp(true, true, false, 1, Cond::Eq, 2, 0) & !(1 << 29), // S = 0
        asm::ccmp(true, true, false, 1, Cond::Eq, 2, 0) | (1 << 4), // o3 = 1
        asm::ccmp(true, true, false, 1, Cond::Eq, 2, 0) | (1 << 10), // o2 = 1
        asm::csel(true, false, false, 1, Cond::Eq, 2, 3) | (1 << 29), // S = 1
        asm::csel(true, false, false, 1, Cond::Eq, 2, 3) | (1 << 11), // op2<1> = 1
        asm::dp3(false, 0b001, false, 1, 2, 3, 4),           // smaddl with sf = 0
        asm::dp3(true, 0b010, false, 1, 2, 3, 4),            // smulh with Ra != 31
        asm::dp3(true, 0b011, false, 1, 31, 3, 4),
        asm::dp3(true, 0b000, false, 1, 2, 3, 4) | (1 << 29), // op54 != 00
        asm::dp2(true, 0b01_0000, 1, 2, 3),                   // crc32x (not decoded)
        asm::dp2(true, 0b00_1100, 1, 2, 3),                   // pacga
        asm::dp2(true, 0b00_0000, 1, 2, 3),                   // subp (MTE)
        asm::dp1(false, 0b00_0011, 1, 2),                     // REV64 with sf = 0
        asm::dp1(true, 0b00_0110, 1, 2),                      // ctz (CSSC)
        asm::dp1(true, 0, 1, 2) | (1 << 16),                  // PAC space
        asm::dp1(true, 0, 1, 2) | (1 << 29),                  // S = 1
    ] {
        assert_eq!(decode(w), Inst::Unknown(w), "{w:#x}");
    }
}

#[test]
fn branches_system_and_exceptions_known_words() {
    assert_eq!(
        decode(asm::tb(false, 3, 0x20, 0)),
        Inst::TestBranch {
            nonzero: false,
            bit: 3,
            rt: x(0),
            offset: 0x20
        }
    );
    assert_eq!(
        decode(0xb7fc_0001),
        Inst::TestBranch {
            nonzero: true,
            bit: 63,
            rt: x(1),
            offset: -0x8000
        }
    );
    assert_eq!(asm::tb(true, 63, -0x8000, 1), 0xb7fc_0001);
    assert_eq!(
        decode(asm::tb(true, 0, 0x7ffc, 31)),
        Inst::TestBranch {
            nonzero: true,
            bit: 0,
            rt: GpReg::Zr,
            offset: 0x7ffc
        }
    );
    assert_eq!(decode(0xd400_0001), Inst::Svc { imm: 0 });
    assert_eq!(decode(asm::svc(0xffff)), Inst::Svc { imm: 0xffff });
    assert_eq!(decode(0xd420_7d00), Inst::Brk { imm: 0x3e8 });
    // HVC, SMC, HLT, DCPS1, TCANCEL, and SVC with op2 != 0 stay unknown.
    for w in [
        0xd400_0002u32,
        0xd400_0003,
        0xd440_0000,
        0xd4a0_0001,
        0xd460_0000,
        0xd400_0005,
    ] {
        assert_eq!(decode(w), Inst::Unknown(w), "{w:#x}");
    }
    assert_eq!(decode(0xd503_203f), Inst::Hint { imm: 1 }); // yield
    assert_eq!(decode(0xd503_233f), Inst::Hint { imm: 25 }); // paciasp
    assert_eq!(decode(asm::hint(127)), Inst::Hint { imm: 127 });
    assert_eq!(
        decode(0xd53b_4200),
        Inst::Mrs {
            reg: SysReg::Nzcv,
            rt: x(0)
        }
    );
    assert_eq!(
        decode(0xd51b_4401),
        Inst::Msr {
            reg: SysReg::Fpcr,
            rt: x(1)
        }
    );
    assert_eq!(
        decode(0xd53b_4422),
        Inst::Mrs {
            reg: SysReg::Fpsr,
            rt: x(2)
        }
    );
    assert_eq!(
        decode(0xd51b_d043),
        Inst::Msr {
            reg: SysReg::TpidrEl0,
            rt: x(3)
        }
    );
    assert_eq!(
        decode(asm::sysreg(true, SysReg::Nzcv, 31)),
        Inst::Mrs {
            reg: SysReg::Nzcv,
            rt: GpReg::Zr
        }
    );
    // Other system registers (TPIDRRO_EL0, CNTVCT_EL0, SCTLR_EL1, MIDR_EL1), barriers,
    // MSR (immediate) and SYS stay unknown.
    for w in [
        0xd53b_d060u32,
        0xd53b_e040,
        0xd518_1000,
        0xd538_0000,
        0xd503_3bbf,
        0xd503_3fdf,
        0xd500_409f,
        0xd508_7520,
    ] {
        assert_eq!(decode(w), Inst::Unknown(w), "{w:#x}");
    }
}

#[test]
fn hint_classes() {
    let classes: Vec<HintClass> = (0..128u8).map(hint_class).collect();
    for imm in [2u8, 3, 4, 5, 7] {
        assert_eq!(classes[usize::from(imm)], HintClass::Unimplemented);
    }
    for imm in [8u8, 12, 24, 25, 28, 29] {
        assert_eq!(classes[usize::from(imm)], HintClass::PacKeyA, "{imm}");
    }
    for imm in [10u8, 14, 26, 27, 30, 31] {
        assert_eq!(classes[usize::from(imm)], HintClass::PacKeyB, "{imm}");
    }
    for imm in [
        0u8, 1, 6, 16, 17, 18, 19, 20, 22, 32, 34, 36, 38, 39, 40, 9, 11, 127,
    ] {
        assert_eq!(classes[usize::from(imm)], HintClass::Nop, "{imm}");
    }
}

#[test]
fn loads_and_stores_known_words() {
    let ls = |w| match decode(w) {
        Inst::LoadStore(i) => i,
        other => panic!("{w:#x}: {other:?}"),
    };
    let i = ls(0x397f_fc20); // ldrb w0, [x1, #4095]
    assert_eq!(
        (i.op, i.size, i.rt, i.rn, i.mode),
        (LdStOp::Load, Ty::I8, x(0), x(1), AddrMode::Unsigned(4095))
    );
    let i = ls(0x7980_07e2); // ldrsh x2, [sp, #2]
    assert_eq!(
        (i.op, i.size, i.rn, i.mode),
        (
            LdStOp::LoadSigned64,
            Ty::I16,
            GpReg::Sp,
            AddrMode::Unsigned(2)
        )
    );
    assert_eq!(ls(0xf97f_fc83).mode, AddrMode::Unsigned(32760)); // ldr x3, [x4, #32760]
    let i = ls(0xb800_4cc5); // str w5, [x6, #4]!
    assert_eq!(
        (i.op, i.size, i.mode),
        (LdStOp::Store, Ty::I32, AddrMode::PreIndex(4))
    );
    let i = ls(0x38d0_0507); // ldrsb w7, [x8], #-256
    assert_eq!(
        (i.op, i.mode),
        (LdStOp::LoadSigned32, AddrMode::PostIndex(-256))
    );
    assert_eq!(ls(0x781f_f149).mode, AddrMode::Unscaled(-1)); // sturh w9, [x10, #-1]
    assert_eq!(ls(0xf84f_f98b).mode, AddrMode::Unprivileged(255)); // ldtr x11, [x12, #255]
    let i = ls(0xf86f_d9cd); // ldr x13, [x14, w15, sxtw #3]
    assert_eq!(
        i.mode,
        AddrMode::Register {
            rm: x(15),
            extend: Extend::Sxtw,
            shift: true
        }
    );
    let i = ls(0x3832_7a30); // strb w16, [x17, x18, lsl #0]
    assert_eq!(
        (i.op, i.mode),
        (
            LdStOp::Store,
            AddrMode::Register {
                rm: x(18),
                extend: Extend::Uxtx,
                shift: true
            }
        )
    );
    assert_eq!(ls(0xb8b5_6a93).op, LdStOp::LoadSigned64); // ldrsw x19, [x20, x21]
    assert_eq!(ls(0xf980_0413).op, LdStOp::Prefetch); // prfm pstl2strm, [x0, #8]
    assert_eq!(ls(0xf89f_d000).op, LdStOp::Prefetch); // prfum
    // Register 31: Rt is ZR, Rn is SP.
    let i = ls(asm::ldst_uimm(3, 0, 0, 31, 31));
    assert_eq!((i.rt, i.rn), (GpReg::Zr, GpReg::Sp));

    assert_eq!(
        decode(0x1800_0040),
        Inst::LoadLiteral {
            op: LiteralOp::Ldr32,
            rt: x(0),
            offset: 8
        }
    );
    assert_eq!(
        decode(asm::ldr_lit(2, (1 << 20) - 4, 2)),
        Inst::LoadLiteral {
            op: LiteralOp::Ldrsw,
            rt: x(2),
            offset: (1 << 20) - 4
        }
    );

    let lp = |w| match decode(w) {
        Inst::LoadStorePair(i) => i,
        other => panic!("{w:#x}: {other:?}"),
    };
    let p = lp(0xa960_07e0); // ldp x0, x1, [sp, #-512]
    assert_eq!(
        (p.op, p.sf, p.rt, p.rt2, p.rn, p.mode, p.offset),
        (
            PairOp::Load,
            true,
            x(0),
            x(1),
            GpReg::Sp,
            PairMode::Offset,
            -512
        )
    );
    let p = lp(0x299f_8c82); // stp w2, w3, [x4, #252]!
    assert_eq!(
        (p.op, p.sf, p.mode, p.offset),
        (PairOp::Store, false, PairMode::PreIndex, 252)
    );
    assert_eq!(
        (lp(0xa8c0_98e5).mode, lp(0xa8c0_98e5).offset),
        (PairMode::PostIndex, 8)
    );
    let p = lp(0x6960_2548); // ldpsw x8, x9, [x10, #-256]
    assert_eq!((p.op, p.elem(), p.offset), (PairOp::LoadSw, Ty::I32, -256));
    assert_eq!(lp(0xa81f_b1ab).mode, PairMode::NonTemporal); // stnp
    assert_eq!(lp(0x2840_3e0e).mode, PairMode::NonTemporal); // ldnp
}

#[test]
fn loads_and_stores_unallocated_and_constrained() {
    for w in [
        asm::ldst_uimm(2, 3, 0, 1, 2),              // size 10, opc 11
        asm::ldst_uimm(3, 3, 0, 1, 2),              // size 11, opc 11
        asm::ldst_imm9(3, 2, 0, 1, 1, 2),           // PRFM post-index
        asm::ldst_imm9(3, 2, 0, 2, 1, 2),           // unprivileged size 11 opc 10
        asm::ldst_reg(3, 1, 3, 0b001, false, 1, 2), // option<1> = 0
        asm::ldst_imm9(3, 1, 8, 1, 4, 4),           // ldr x4, [x4], #8: wback overlap
        asm::ldst_imm9(3, 0, 8, 3, 4, 4),           // str x4, [x4, #8]!: wback overlap
        asm::ldst_pair(2, 2, true, 0, 3, 1, 3),     // ldp x3, x3, [x1]
        asm::ldst_pair(2, 1, false, 2, 1, 1, 0),    // stp x0, x1, [x1], #16: overlap Rt2
        asm::ldst_pair(1, 0, true, 0, 1, 2, 3),     // ldnp with opc 01
        asm::ldst_pair(1, 2, false, 0, 1, 2, 3),    // STGP (MTE)
        asm::ldst_pair(3, 2, true, 0, 1, 2, 3),     // opc 11
        0x3820_0000 | (1 << 21),                    // LDADDB (LSE atomics)
        0xc85f_7c00,                                // ldxr x0, [x0]
        0xc8df_fc00,                                // ldar x0, [x0]
        0xf820_0400 | (1 << 21),                    // ldraa space
        0x3d40_0000,                                // ldr b0, [x0] (SIMD&FP)
    ] {
        assert_eq!(decode(w), Inst::Unknown(w), "{w:#x}");
    }
    // Writeback with base SP and Rt = 31 (ZR) is fine: different registers.
    assert!(matches!(
        decode(asm::ldst_imm9(3, 0, -16, 3, 31, 31)),
        Inst::LoadStore(_)
    ));
    // LDP xzr, x1 is fine; LDP with the same register only for loads.
    assert!(matches!(
        decode(asm::ldst_pair(2, 2, false, 0, 3, 1, 3)),
        Inst::LoadStorePair(_)
    ));
}

/// `Inst` is copied into every execution and will fill the T0 decode cache (4096 per
/// page, dbt.md §3.3); growing it costs measurable interpreter throughput.
#[test]
fn inst_stays_small() {
    assert!(core::mem::size_of::<Inst>() <= 32);
}
