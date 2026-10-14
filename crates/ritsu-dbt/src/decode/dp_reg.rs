//! Data processing — register (`op0 = x101`).
//!
//! Fields: `op0 = word[30]`, `op1 = word[28]`, `op2 = word[24:21]`,
//! `op3 = word[15:10]`.

use super::{
    AddSubCarry, AddSubOp, AddSubReg, Cond, CondCmpRhs, CondCompare, CondSelOp, CondSelect,
    DataProc1, DataProc2, DpOp1, DpOp2, Extend, Inst, LogicOp, LogicalReg, MulAdd, MulOp,
    RegOperand, Shift, bit, bits, sp, zr,
};

#[inline(never)]
pub(super) fn decode(word: u32) -> Inst {
    let op1 = bit(word, 28);
    let op2 = bits(word, 21, 4);
    match (op1, op2) {
        (false, 0b0000..=0b0111) => decode_logical(word),
        (false, _) if op2 & 1 == 0 => decode_add_sub_shifted(word),
        (false, _) => decode_add_sub_extended(word),
        (true, 0b0000) => decode_carry(word),
        (true, 0b0010) => decode_cond_compare(word),
        (true, 0b0100) => decode_cond_select(word),
        (true, 0b0110) => {
            if bit(word, 30) {
                decode_dp1(word)
            } else {
                decode_dp2(word)
            }
        }
        (true, 0b1000..=0b1111) => decode_dp3(word),
        _ => Inst::Unknown(word),
    }
}

/// `sf:1 opc:2 01010 shift:2 N:1 Rm:5 imm6:6 Rn:5 Rd:5`
fn decode_logical(word: u32) -> Inst {
    let sf = bit(word, 31);
    let amount = bits(word, 10, 6);
    if !sf && amount >= 32 {
        return Inst::Unknown(word);
    }
    let op = match bits(word, 29, 2) {
        0b00 => LogicOp::And,
        0b01 => LogicOp::Orr,
        0b10 => LogicOp::Eor,
        _ => LogicOp::Ands,
    };
    Inst::LogicalReg(LogicalReg {
        op,
        invert: bit(word, 21),
        sf,
        shift: Shift::from_bits(bits(word, 22, 2)),
        amount: amount as u8,
        rd: zr(word, 0),
        rn: zr(word, 5),
        rm: zr(word, 16),
    })
}

fn add_sub_op(word: u32) -> AddSubOp {
    if bit(word, 30) {
        AddSubOp::Sub
    } else {
        AddSubOp::Add
    }
}

/// `sf:1 op:1 S:1 01011 shift:2 0 Rm:5 imm6:6 Rn:5 Rd:5`
fn decode_add_sub_shifted(word: u32) -> Inst {
    let sf = bit(word, 31);
    let shift = bits(word, 22, 2);
    let amount = bits(word, 10, 6);
    if shift == 0b11 || (!sf && amount >= 32) {
        return Inst::Unknown(word);
    }
    Inst::AddSubReg(AddSubReg {
        op: add_sub_op(word),
        sf,
        set_flags: bit(word, 29),
        rd: zr(word, 0),
        rn: zr(word, 5),
        rm: zr(word, 16),
        operand: RegOperand::Shifted {
            shift: Shift::from_bits(shift),
            amount: amount as u8,
        },
    })
}

/// `sf:1 op:1 S:1 01011 opt:2 1 Rm:5 option:3 imm3:3 Rn:5 Rd:5`
fn decode_add_sub_extended(word: u32) -> Inst {
    let amount = bits(word, 10, 3);
    if bits(word, 22, 2) != 0 || amount > 4 {
        return Inst::Unknown(word);
    }
    let set_flags = bit(word, 29);
    Inst::AddSubReg(AddSubReg {
        op: add_sub_op(word),
        sf: bit(word, 31),
        set_flags,
        rd: if set_flags { zr(word, 0) } else { sp(word, 0) },
        rn: sp(word, 5),
        rm: zr(word, 16),
        operand: RegOperand::Extended {
            extend: Extend::from_bits(bits(word, 13, 3)),
            amount: amount as u8,
        },
    })
}

/// `sf:1 op:1 S:1 11010000 Rm:5 000000 Rn:5 Rd:5`
fn decode_carry(word: u32) -> Inst {
    if bits(word, 10, 6) != 0 {
        // RMIF / SETF8 / SETF16 (FEAT_FlagM) and unallocated.
        return Inst::Unknown(word);
    }
    Inst::AddSubCarry(AddSubCarry {
        op: add_sub_op(word),
        sf: bit(word, 31),
        set_flags: bit(word, 29),
        rd: zr(word, 0),
        rn: zr(word, 5),
        rm: zr(word, 16),
    })
}

/// `sf:1 op:1 1 11010010 Rm|imm5:5 cond:4 imm:1 0 Rn:5 0 nzcv:4`
fn decode_cond_compare(word: u32) -> Inst {
    if !bit(word, 29) || bit(word, 10) || bit(word, 4) {
        return Inst::Unknown(word);
    }
    Inst::CondCompare(CondCompare {
        op: add_sub_op(word),
        sf: bit(word, 31),
        cond: Cond::from_bits(bits(word, 12, 4)),
        nzcv: bits(word, 0, 4) as u8,
        rn: zr(word, 5),
        rhs: if bit(word, 11) {
            CondCmpRhs::Imm(bits(word, 16, 5) as u8)
        } else {
            CondCmpRhs::Reg(zr(word, 16))
        },
    })
}

/// `sf:1 op:1 S:1 11010100 Rm:5 cond:4 op2:2 Rn:5 Rd:5`
fn decode_cond_select(word: u32) -> Inst {
    if bit(word, 29) || bit(word, 11) {
        return Inst::Unknown(word);
    }
    let op = match (bit(word, 30), bit(word, 10)) {
        (false, false) => CondSelOp::Csel,
        (false, true) => CondSelOp::Csinc,
        (true, false) => CondSelOp::Csinv,
        (true, true) => CondSelOp::Csneg,
    };
    Inst::CondSelect(CondSelect {
        op,
        sf: bit(word, 31),
        cond: Cond::from_bits(bits(word, 12, 4)),
        rd: zr(word, 0),
        rn: zr(word, 5),
        rm: zr(word, 16),
    })
}

/// `sf:1 0 S:1 11010110 Rm:5 opcode:6 Rn:5 Rd:5`
fn decode_dp2(word: u32) -> Inst {
    if bit(word, 29) {
        return Inst::Unknown(word);
    }
    let op = match bits(word, 10, 6) {
        0b00_0010 => DpOp2::Udiv,
        0b00_0011 => DpOp2::Sdiv,
        0b00_1000 => DpOp2::Lslv,
        0b00_1001 => DpOp2::Lsrv,
        0b00_1010 => DpOp2::Asrv,
        0b00_1011 => DpOp2::Rorv,
        // CRC32*, PACGA, SUBP/IRG/GMI (FEAT_MTE), SMAX/UMIN… (FEAT_CSSC).
        _ => return Inst::Unknown(word),
    };
    Inst::DataProc2(DataProc2 {
        op,
        sf: bit(word, 31),
        rd: zr(word, 0),
        rn: zr(word, 5),
        rm: zr(word, 16),
    })
}

/// `sf:1 1 S:1 11010110 opcode2:5 opcode:6 Rn:5 Rd:5`
fn decode_dp1(word: u32) -> Inst {
    if bit(word, 29) || bits(word, 16, 5) != 0 {
        // opcode2 = 00001: PAC/AUT/XPAC (WP-DBT-11).
        return Inst::Unknown(word);
    }
    let sf = bit(word, 31);
    let op = match (bits(word, 10, 6), sf) {
        (0b00_0000, _) => DpOp1::Rbit,
        (0b00_0001, _) => DpOp1::Rev16,
        (0b00_0010, false) | (0b00_0011, true) => DpOp1::Rev,
        (0b00_0010, true) => DpOp1::Rev32,
        (0b00_0100, _) => DpOp1::Clz,
        (0b00_0101, _) => DpOp1::Cls,
        // CTZ/CNT/ABS (FEAT_CSSC) and unallocated.
        _ => return Inst::Unknown(word),
    };
    Inst::DataProc1(DataProc1 {
        op,
        sf,
        rd: zr(word, 0),
        rn: zr(word, 5),
    })
}

/// `sf:1 op54:2 11011 op31:3 Rm:5 o0:1 Ra:5 Rn:5 Rd:5`
fn decode_dp3(word: u32) -> Inst {
    let sf = bit(word, 31);
    if bits(word, 29, 2) != 0 {
        return Inst::Unknown(word);
    }
    let ra_field = bits(word, 10, 5);
    let op = match (bits(word, 21, 3), bit(word, 15), sf) {
        (0b000, false, _) => MulOp::Madd,
        (0b000, true, _) => MulOp::Msub,
        (0b001, false, true) => MulOp::Smaddl,
        (0b001, true, true) => MulOp::Smsubl,
        (0b010, false, true) if ra_field == 31 => MulOp::Smulh,
        (0b101, false, true) => MulOp::Umaddl,
        (0b101, true, true) => MulOp::Umsubl,
        (0b110, false, true) if ra_field == 31 => MulOp::Umulh,
        _ => return Inst::Unknown(word),
    };
    Inst::MulAdd(MulAdd {
        op,
        sf,
        rd: zr(word, 0),
        rn: zr(word, 5),
        rm: zr(word, 16),
        ra: zr(word, 10),
    })
}
