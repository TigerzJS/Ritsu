//! Test-only A64 encoder for the decoded subset.
//!
//! Every helper is a literal transcription of the field layout from the Arm ARM; the
//! decoder tests cross-check them against hand-verified instruction words before they
//! are used to build test programs.

use crate::cpu::GpReg;
use crate::decode::{AddSubOp, Cond, Inst, MoveWideOp};

/// `sf:1 op:1 S:1 100010 sh:1 imm12:12 Rn:5 Rd:5`
pub(crate) fn add_sub_imm(
    sf: bool,
    sub: bool,
    s: bool,
    sh: bool,
    imm12: u32,
    rn: u32,
    rd: u32,
) -> u32 {
    (u32::from(sf) << 31)
        | (u32::from(sub) << 30)
        | (u32::from(s) << 29)
        | (0b10_0010 << 23)
        | (u32::from(sh) << 22)
        | ((imm12 & 0xfff) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 opc:2 100101 hw:2 imm16:16 Rd:5`
pub(crate) fn move_wide(sf: bool, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | ((opc & 3) << 29)
        | (0b10_0101 << 23)
        | ((hw & 3) << 21)
        | ((imm16 & 0xffff) << 5)
        | (rd & 31)
}

/// `op:1 00101 imm26:26` (`op = 1` is `BL`).
pub(crate) fn b(link: bool, offset: i64) -> u32 {
    (u32::from(link) << 31) | (0b00101 << 26) | ((offset >> 2) as u32 & 0x03ff_ffff)
}

/// `01010100 imm19:19 0 cond:4`
pub(crate) fn b_cond(cond: Cond, offset: i64) -> u32 {
    0x5400_0000 | (((offset >> 2) as u32 & 0x7_ffff) << 5) | cond.bits()
}

/// `sf:1 011010 op:1 imm19:19 Rt:5` (`op = 1` is `CBNZ`).
pub(crate) fn cb(sf: bool, nonzero: bool, offset: i64, rt: u32) -> u32 {
    (u32::from(sf) << 31)
        | (0b01_1010 << 25)
        | (u32::from(nonzero) << 24)
        | (((offset >> 2) as u32 & 0x7_ffff) << 5)
        | (rt & 31)
}

/// `1101011 opc:4 11111 000000 Rn:5 00000` (`opc`: 0 = BR, 1 = BLR, 2 = RET).
pub(crate) fn br(opc: u32, rn: u32) -> u32 {
    0xd61f_0000 | ((opc & 0xf) << 21) | ((rn & 31) << 5)
}

/// `NOP`
pub(crate) const NOP: u32 = 0xd503_201f;

fn field(r: GpReg) -> u32 {
    match r {
        GpReg::X(x) => u32::from(x.index()),
        GpReg::Sp | GpReg::Zr => 31,
    }
}

/// Re-encodes a decoded instruction; `None` for [`Inst::Unknown`].
pub(crate) fn encode(inst: &Inst) -> Option<u32> {
    Some(match *inst {
        Inst::MoveWide {
            op,
            sf,
            shift,
            imm16,
            rd,
        } => {
            let opc = match op {
                MoveWideOp::Movn => 0b00,
                MoveWideOp::Movz => 0b10,
                MoveWideOp::Movk => 0b11,
            };
            move_wide(sf, opc, u32::from(shift / 16), u32::from(imm16), field(rd))
        }
        Inst::AddSubImm {
            op,
            sf,
            set_flags,
            imm12,
            shift12,
            rd,
            rn,
        } => add_sub_imm(
            sf,
            op == AddSubOp::Sub,
            set_flags,
            shift12,
            u32::from(imm12),
            field(rn),
            field(rd),
        ),
        Inst::B { offset } => b(false, offset),
        Inst::Bl { offset } => b(true, offset),
        Inst::Br { rn } => br(0, field(rn)),
        Inst::Blr { rn } => br(1, field(rn)),
        Inst::Ret { rn } => br(2, field(rn)),
        Inst::CompareBranch {
            nonzero,
            sf,
            rt,
            offset,
        } => cb(sf, nonzero, offset, field(rt)),
        Inst::BCond { cond, offset } => b_cond(cond, offset),
        Inst::Nop => NOP,
        Inst::Unknown(_) => return None,
    })
}
