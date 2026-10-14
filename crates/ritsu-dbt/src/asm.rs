//! Test-only A64 encoder for the decoded subset.
//!
//! Every helper is a literal transcription of the field layout from the Arm ARM; the
//! decoder tests cross-check them against hand-verified instruction words (and the
//! LLVM differential test checks the decoder) before they are used to build test
//! programs. [`encode`] re-encodes any decoded [`Inst`] (roundtrip test).

use crate::cpu::{GpReg, SysReg};
use crate::decode::{
    AddSubOp, AddrMode, BitfieldOp, Cond, CondCmpRhs, CondSelOp, DpOp1, DpOp2, Inst, LdStOp,
    LiteralOp, LogicOp, MoveWideOp, MulOp, PairMode, PairOp, RegOperand, Shift,
};
use crate::sem::Ty;

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

/// `op:1 immlo:2 10000 immhi:19 Rd:5` (`imm` in units: bytes for ADR, pages for ADRP).
pub(crate) fn adr(page: bool, imm: i64, rd: u32) -> u32 {
    let imm = imm as u32 & 0x1f_ffff;
    (u32::from(page) << 31) | ((imm & 3) << 29) | (0b1_0000 << 24) | ((imm >> 2) << 5) | (rd & 31)
}

/// `sf:1 opc:2 100100 N:immr:imms (13) Rn:5 Rd:5`
pub(crate) fn logical_imm(sf: bool, opc: u32, bitmask: u32, rn: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | ((opc & 3) << 29)
        | (0b10_0100 << 23)
        | ((bitmask & 0x1fff) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 opc:2 100110 N:1 immr:6 imms:6 Rn:5 Rd:5` (`N = sf`).
pub(crate) fn bitfield(sf: bool, opc: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | ((opc & 3) << 29)
        | (0b10_0110 << 23)
        | (u32::from(sf) << 22)
        | ((immr & 63) << 16)
        | ((imms & 63) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 00 100111 N:1 0 Rm:5 imms:6 Rn:5 Rd:5` (`N = sf`).
pub(crate) fn extr(sf: bool, rm: u32, lsb: u32, rn: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | (0b10_0111 << 23)
        | (u32::from(sf) << 22)
        | ((rm & 31) << 16)
        | ((lsb & 63) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 opc:2 01010 shift:2 N:1 Rm:5 imm6:6 Rn:5 Rd:5`
#[allow(clippy::too_many_arguments)]
pub(crate) fn logical_reg(
    sf: bool,
    opc: u32,
    n: bool,
    shift: u32,
    rm: u32,
    amount: u32,
    rn: u32,
    rd: u32,
) -> u32 {
    (u32::from(sf) << 31)
        | ((opc & 3) << 29)
        | (0b01010 << 24)
        | ((shift & 3) << 22)
        | (u32::from(n) << 21)
        | ((rm & 31) << 16)
        | ((amount & 63) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 op:1 S:1 01011 shift:2 0 Rm:5 imm6:6 Rn:5 Rd:5`
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_sub_shifted(
    sf: bool,
    sub: bool,
    s: bool,
    shift: u32,
    rm: u32,
    amount: u32,
    rn: u32,
    rd: u32,
) -> u32 {
    (u32::from(sf) << 31)
        | (u32::from(sub) << 30)
        | (u32::from(s) << 29)
        | (0b01011 << 24)
        | ((shift & 3) << 22)
        | ((rm & 31) << 16)
        | ((amount & 63) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 op:1 S:1 01011 00 1 Rm:5 option:3 imm3:3 Rn:5 Rd:5`
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_sub_ext(
    sf: bool,
    sub: bool,
    s: bool,
    rm: u32,
    option: u32,
    amount: u32,
    rn: u32,
    rd: u32,
) -> u32 {
    (u32::from(sf) << 31)
        | (u32::from(sub) << 30)
        | (u32::from(s) << 29)
        | (0b01011 << 24)
        | (1 << 21)
        | ((rm & 31) << 16)
        | ((option & 7) << 13)
        | ((amount & 7) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 op:1 S:1 11010000 Rm:5 000000 Rn:5 Rd:5`
pub(crate) fn adc(sf: bool, sub: bool, s: bool, rm: u32, rn: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | (u32::from(sub) << 30)
        | (u32::from(s) << 29)
        | (0b1101_0000 << 21)
        | ((rm & 31) << 16)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 op:1 1 11010010 Rm|imm5:5 cond:4 imm:1 0 Rn:5 0 nzcv:4`
pub(crate) fn ccmp(sf: bool, sub: bool, imm: bool, rm: u32, cond: Cond, rn: u32, nzcv: u32) -> u32 {
    (u32::from(sf) << 31)
        | (u32::from(sub) << 30)
        | (1 << 29)
        | (0b1101_0010 << 21)
        | ((rm & 31) << 16)
        | (cond.bits() << 12)
        | (u32::from(imm) << 11)
        | ((rn & 31) << 5)
        | (nzcv & 15)
}

/// `sf:1 op:1 0 11010100 Rm:5 cond:4 0 o2:1 Rn:5 Rd:5`
pub(crate) fn csel(sf: bool, op: bool, o2: bool, rm: u32, cond: Cond, rn: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | (u32::from(op) << 30)
        | (0b1101_0100 << 21)
        | ((rm & 31) << 16)
        | (cond.bits() << 12)
        | (u32::from(o2) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 00 11011 op31:3 Rm:5 o0:1 Ra:5 Rn:5 Rd:5`
#[allow(clippy::too_many_arguments)]
pub(crate) fn dp3(sf: bool, op31: u32, o0: bool, rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | (0b11011 << 24)
        | ((op31 & 7) << 21)
        | ((rm & 31) << 16)
        | (u32::from(o0) << 15)
        | ((ra & 31) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 0 0 11010110 Rm:5 opcode:6 Rn:5 Rd:5`
pub(crate) fn dp2(sf: bool, opcode: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | (0b1101_0110 << 21)
        | ((rm & 31) << 16)
        | ((opcode & 63) << 10)
        | ((rn & 31) << 5)
        | (rd & 31)
}

/// `sf:1 1 0 11010110 00000 opcode:6 Rn:5 Rd:5`
pub(crate) fn dp1(sf: bool, opcode: u32, rn: u32, rd: u32) -> u32 {
    (u32::from(sf) << 31)
        | (1 << 30)
        | (0b1101_0110 << 21)
        | ((opcode & 63) << 10)
        | ((rn & 31) << 5)
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

/// `b5:1 011011 op:1 b40:5 imm14:14 Rt:5`
pub(crate) fn tb(nonzero: bool, bit: u32, offset: i64, rt: u32) -> u32 {
    ((bit >> 5 & 1) << 31)
        | (0b01_1011 << 25)
        | (u32::from(nonzero) << 24)
        | ((bit & 31) << 19)
        | (((offset >> 2) as u32 & 0x3fff) << 5)
        | (rt & 31)
}

/// `1101011 opc:4 11111 000000 Rn:5 00000` (`opc`: 0 = BR, 1 = BLR, 2 = RET).
pub(crate) fn br(opc: u32, rn: u32) -> u32 {
    0xd61f_0000 | ((opc & 0xf) << 21) | ((rn & 31) << 5)
}

/// `SVC #imm`
pub(crate) fn svc(imm: u32) -> u32 {
    0xd400_0001 | ((imm & 0xffff) << 5)
}

/// `BRK #imm`
pub(crate) fn brk(imm: u32) -> u32 {
    0xd420_0000 | ((imm & 0xffff) << 5)
}

/// `HINT #imm`
pub(crate) fn hint(imm: u32) -> u32 {
    0xd503_201f | ((imm & 0x7f) << 5)
}

/// `MRS Xt, reg` (`read`) / `MSR reg, Xt`.
pub(crate) fn sysreg(read: bool, reg: SysReg, rt: u32) -> u32 {
    0xd510_0000 | (u32::from(read) << 21) | (reg.encoding() << 5) | (rt & 31)
}

/// `opc:2 011 0 00 imm19:19 Rt:5`
pub(crate) fn ldr_lit(opc: u32, offset: i64, rt: u32) -> u32 {
    ((opc & 3) << 30) | (0b011 << 27) | (((offset >> 2) as u32 & 0x7_ffff) << 5) | (rt & 31)
}

/// `size:2 111 0 01 opc:2 imm12:12 Rn:5 Rt:5` (`imm12` in units of the size).
pub(crate) fn ldst_uimm(size: u32, opc: u32, imm12: u32, rn: u32, rt: u32) -> u32 {
    ((size & 3) << 30)
        | (0b11_1001 << 24)
        | ((opc & 3) << 22)
        | ((imm12 & 0xfff) << 10)
        | ((rn & 31) << 5)
        | (rt & 31)
}

/// `size:2 111 0 00 opc:2 0 imm9:9 kind:2 Rn:5 Rt:5` (`kind`: 00 unscaled, 01 post,
/// 10 unprivileged, 11 pre).
pub(crate) fn ldst_imm9(size: u32, opc: u32, imm9: i64, kind: u32, rn: u32, rt: u32) -> u32 {
    ((size & 3) << 30)
        | (0b111 << 27)
        | ((opc & 3) << 22)
        | ((imm9 as u32 & 0x1ff) << 12)
        | ((kind & 3) << 10)
        | ((rn & 31) << 5)
        | (rt & 31)
}

/// `size:2 111 0 00 opc:2 1 Rm:5 option:3 S:1 10 Rn:5 Rt:5`
#[allow(clippy::too_many_arguments)]
pub(crate) fn ldst_reg(
    size: u32,
    opc: u32,
    rm: u32,
    option: u32,
    s: bool,
    rn: u32,
    rt: u32,
) -> u32 {
    ((size & 3) << 30)
        | (0b111 << 27)
        | ((opc & 3) << 22)
        | (1 << 21)
        | ((rm & 31) << 16)
        | ((option & 7) << 13)
        | (u32::from(s) << 12)
        | (0b10 << 10)
        | ((rn & 31) << 5)
        | (rt & 31)
}

/// `opc:2 101 0 type:3 L:1 imm7:7 Rt2:5 Rn:5 Rt:5` (`imm7` in element units; `type`:
/// 000 non-temporal, 001 post, 010 offset, 011 pre).
#[allow(clippy::too_many_arguments)]
pub(crate) fn ldst_pair(
    opc: u32,
    kind: u32,
    load: bool,
    imm7: i64,
    rt2: u32,
    rn: u32,
    rt: u32,
) -> u32 {
    ((opc & 3) << 30)
        | (0b101 << 27)
        | ((kind & 7) << 23)
        | (u32::from(load) << 22)
        | ((imm7 as u32 & 0x7f) << 15)
        | ((rt2 & 31) << 10)
        | ((rn & 31) << 5)
        | (rt & 31)
}

/// `NOP`
pub(crate) const NOP: u32 = 0xd503_201f;

fn field(r: GpReg) -> u32 {
    match r {
        GpReg::X(x) => u32::from(x.index()),
        GpReg::Sp | GpReg::Zr => 31,
    }
}

fn logic_opc(op: LogicOp) -> u32 {
    match op {
        LogicOp::And => 0,
        LogicOp::Orr => 1,
        LogicOp::Eor => 2,
        LogicOp::Ands => 3,
    }
}

fn shift_bits(s: Shift) -> u32 {
    match s {
        Shift::Lsl => 0,
        Shift::Lsr => 1,
        Shift::Asr => 2,
        Shift::Ror => 3,
    }
}

fn size_bits(t: Ty) -> u32 {
    t.bytes().trailing_zeros()
}

fn ldst_opc(op: LdStOp) -> u32 {
    match op {
        LdStOp::Store => 0,
        LdStOp::Load => 1,
        LdStOp::LoadSigned64 | LdStOp::Prefetch => 2,
        LdStOp::LoadSigned32 => 3,
    }
}

/// Re-encodes a decoded instruction; `None` for [`Inst::Unknown`].
#[allow(clippy::too_many_lines)]
pub(crate) fn encode(inst: &Inst) -> Option<u32> {
    let sub = |op: AddSubOp| op == AddSubOp::Sub;
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
            sub(op),
            set_flags,
            shift12,
            u32::from(imm12),
            field(rn),
            field(rd),
        ),
        Inst::Adr { page, imm, rd } => adr(page, if page { imm >> 12 } else { imm }, field(rd)),
        Inst::LogicalImm(i) => logical_imm(
            i.sf,
            logic_opc(i.op),
            u32::from(i.bitmask),
            field(i.rn),
            field(i.rd),
        ),
        Inst::Bitfield(i) => {
            let opc = match i.op {
                BitfieldOp::Sbfm => 0,
                BitfieldOp::Bfm => 1,
                BitfieldOp::Ubfm => 2,
            };
            bitfield(
                i.sf,
                opc,
                u32::from(i.immr),
                u32::from(i.imms),
                field(i.rn),
                field(i.rd),
            )
        }
        Inst::Extract(i) => extr(
            i.sf,
            field(i.rm),
            u32::from(i.lsb),
            field(i.rn),
            field(i.rd),
        ),
        Inst::LogicalReg(i) => logical_reg(
            i.sf,
            logic_opc(i.op),
            i.invert,
            shift_bits(i.shift),
            field(i.rm),
            u32::from(i.amount),
            field(i.rn),
            field(i.rd),
        ),
        Inst::AddSubReg(i) => match i.operand {
            RegOperand::Shifted { shift, amount } => add_sub_shifted(
                i.sf,
                sub(i.op),
                i.set_flags,
                shift_bits(shift),
                field(i.rm),
                u32::from(amount),
                field(i.rn),
                field(i.rd),
            ),
            RegOperand::Extended { extend, amount } => add_sub_ext(
                i.sf,
                sub(i.op),
                i.set_flags,
                field(i.rm),
                extend.bits(),
                u32::from(amount),
                field(i.rn),
                field(i.rd),
            ),
        },
        Inst::AddSubCarry(i) => adc(
            i.sf,
            sub(i.op),
            i.set_flags,
            field(i.rm),
            field(i.rn),
            field(i.rd),
        ),
        Inst::CondCompare(i) => {
            let (imm, rm) = match i.rhs {
                CondCmpRhs::Reg(r) => (false, field(r)),
                CondCmpRhs::Imm(v) => (true, u32::from(v)),
            };
            ccmp(
                i.sf,
                sub(i.op),
                imm,
                rm,
                i.cond,
                field(i.rn),
                u32::from(i.nzcv),
            )
        }
        Inst::CondSelect(i) => {
            let (op, o2) = match i.op {
                CondSelOp::Csel => (false, false),
                CondSelOp::Csinc => (false, true),
                CondSelOp::Csinv => (true, false),
                CondSelOp::Csneg => (true, true),
            };
            csel(i.sf, op, o2, field(i.rm), i.cond, field(i.rn), field(i.rd))
        }
        Inst::MulAdd(i) => {
            let (op31, o0) = match i.op {
                MulOp::Madd => (0b000, false),
                MulOp::Msub => (0b000, true),
                MulOp::Smaddl => (0b001, false),
                MulOp::Smsubl => (0b001, true),
                MulOp::Smulh => (0b010, false),
                MulOp::Umaddl => (0b101, false),
                MulOp::Umsubl => (0b101, true),
                MulOp::Umulh => (0b110, false),
            };
            dp3(
                i.sf,
                op31,
                o0,
                field(i.rm),
                field(i.ra),
                field(i.rn),
                field(i.rd),
            )
        }
        Inst::DataProc2(i) => {
            let opcode = match i.op {
                DpOp2::Udiv => 0b00_0010,
                DpOp2::Sdiv => 0b00_0011,
                DpOp2::Lslv => 0b00_1000,
                DpOp2::Lsrv => 0b00_1001,
                DpOp2::Asrv => 0b00_1010,
                DpOp2::Rorv => 0b00_1011,
            };
            dp2(i.sf, opcode, field(i.rm), field(i.rn), field(i.rd))
        }
        Inst::DataProc1(i) => {
            let opcode = match (i.op, i.sf) {
                (DpOp1::Rbit, _) => 0,
                (DpOp1::Rev16, _) => 1,
                (DpOp1::Rev, false) | (DpOp1::Rev32, _) => 2,
                (DpOp1::Rev, true) => 3,
                (DpOp1::Clz, _) => 4,
                (DpOp1::Cls, _) => 5,
            };
            dp1(i.sf, opcode, field(i.rn), field(i.rd))
        }
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
        Inst::TestBranch {
            nonzero,
            bit,
            rt,
            offset,
        } => tb(nonzero, u32::from(bit), offset, field(rt)),
        Inst::BCond { cond, offset } => b_cond(cond, offset),
        Inst::Svc { imm } => svc(u32::from(imm)),
        Inst::Brk { imm } => brk(u32::from(imm)),
        Inst::Nop => NOP,
        Inst::Hint { imm } => hint(u32::from(imm)),
        Inst::Mrs { reg, rt } => sysreg(true, reg, field(rt)),
        Inst::Msr { reg, rt } => sysreg(false, reg, field(rt)),
        Inst::LoadLiteral { op, rt, offset } => {
            let opc = match op {
                LiteralOp::Ldr32 => 0,
                LiteralOp::Ldr64 => 1,
                LiteralOp::Ldrsw => 2,
                LiteralOp::Prfm => 3,
            };
            ldr_lit(opc, offset, field(rt))
        }
        Inst::LoadStore(i) => {
            let (size, opc) = (size_bits(i.size), ldst_opc(i.op));
            let (rn, rt) = (field(i.rn), field(i.rt));
            match i.mode {
                AddrMode::Unsigned(off) => ldst_uimm(size, opc, (off >> size) as u32, rn, rt),
                AddrMode::Unscaled(off) => ldst_imm9(size, opc, off, 0, rn, rt),
                AddrMode::PostIndex(off) => ldst_imm9(size, opc, off, 1, rn, rt),
                AddrMode::Unprivileged(off) => ldst_imm9(size, opc, off, 2, rn, rt),
                AddrMode::PreIndex(off) => ldst_imm9(size, opc, off, 3, rn, rt),
                AddrMode::Register { rm, extend, shift } => {
                    ldst_reg(size, opc, field(rm), extend.bits(), shift, rn, rt)
                }
            }
        }
        Inst::LoadStorePair(i) => {
            let opc = match (i.op, i.sf) {
                (PairOp::LoadSw, _) => 1,
                (_, false) => 0,
                (_, true) => 2,
            };
            let kind = match i.mode {
                PairMode::NonTemporal => 0,
                PairMode::PostIndex => 1,
                PairMode::Offset => 2,
                PairMode::PreIndex => 3,
            };
            let scale = i.elem().bytes().trailing_zeros();
            ldst_pair(
                opc,
                kind,
                i.op != PairOp::Store,
                i.offset >> scale,
                field(i.rt2),
                field(i.rn),
                field(i.rt),
            )
        }
        Inst::Unknown(_) => return None,
    })
}
