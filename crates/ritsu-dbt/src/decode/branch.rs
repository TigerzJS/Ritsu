//! Branches, exception generating and system instructions (`op0 = 101x`).

use super::{Cond, Inst, bit, bits, branch_offset, zr};
use crate::cpu::SysReg;

pub(super) fn decode(word: u32) -> Inst {
    // Unconditional branch (immediate): op:1 00101 imm26:26
    if word & 0x7c00_0000 == 0x1400_0000 {
        let offset = branch_offset(bits(word, 0, 26), 26);
        return if bit(word, 31) {
            Inst::Bl { offset }
        } else {
            Inst::B { offset }
        };
    }
    // Compare and branch (immediate): sf:1 011010 op:1 imm19:19 Rt:5
    if word & 0x7e00_0000 == 0x3400_0000 {
        return Inst::CompareBranch {
            nonzero: bit(word, 24),
            sf: bit(word, 31),
            rt: zr(word, 0),
            offset: branch_offset(bits(word, 5, 19), 19),
        };
    }
    // Test and branch (immediate): b5:1 011011 op:1 b40:5 imm14:14 Rt:5
    if word & 0x7e00_0000 == 0x3600_0000 {
        return Inst::TestBranch {
            nonzero: bit(word, 24),
            // b5:b40 is 6 bits.
            bit: ((bits(word, 31, 1) << 5) | bits(word, 19, 5)) as u8,
            rt: zr(word, 0),
            offset: branch_offset(bits(word, 5, 14), 14),
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
    if word & 0xff00_0000 == 0xd400_0000 || word & 0xffc0_0000 == 0xd500_0000 {
        return decode_system(word);
    }
    // Unconditional branch (register): 1101011 opc:4 op2=11111 op3=000000 Rn:5 op4=00000
    // Only the non-authenticating forms (op3 = 0, op4 = 0) are decoded here.
    let rn = zr(word, 5);
    match word & 0xffff_fc1f {
        0xd61f_0000 => Inst::Br { rn },
        0xd63f_0000 => Inst::Blr { rn },
        0xd65f_0000 => Inst::Ret { rn },
        _ => Inst::Unknown(word),
    }
}

/// Exception generation, hints and system register moves (`0xd4xx_xxxx`,
/// `0xd500_0000..=0xd53f_ffff`); out of line to keep the branch decoder small.
#[inline(never)]
fn decode_system(word: u32) -> Inst {
    // Exception generation: 11010100 opc:3 imm16:16 op2:3 LL:2
    // SVC = opc 000, op2 000, LL 01; BRK = opc 001, op2 000, LL 00. HVC/SMC are
    // UNDEFINED without EL2/EL3, HLT without halting debug, DCPS outside Debug state;
    // TCANCEL is FEAT_TME. All of those stay Unknown.
    if word & 0xffe0_001f == 0xd400_0001 {
        return Inst::Svc {
            imm: bits(word, 5, 16) as u16,
        };
    }
    if word & 0xffe0_001f == 0xd420_0000 {
        return Inst::Brk {
            imm: bits(word, 5, 16) as u16,
        };
    }
    // Hints: 1101010100 0 00 011 0010 CRm:4 op2:3 11111
    if word & 0xffff_f01f == 0xd503_201f {
        // CRm:op2 is 7 bits.
        let imm = bits(word, 5, 7) as u8;
        return if imm == 0 {
            Inst::Nop
        } else {
            Inst::Hint { imm }
        };
    }
    // System register move: 1101010100 L 1 o0 op1:3 CRn:4 CRm:4 op2:3 Rt:5
    if word & 0xffd0_0000 == 0xd510_0000 {
        let Some(reg) = SysReg::from_encoding(bits(word, 5, 15)) else {
            return Inst::Unknown(word);
        };
        let rt = zr(word, 0);
        return if bit(word, 21) {
            Inst::Mrs { reg, rt }
        } else {
            Inst::Msr { reg, rt }
        };
    }
    Inst::Unknown(word)
}
