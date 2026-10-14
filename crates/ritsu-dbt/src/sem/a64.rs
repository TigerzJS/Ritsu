//! A64 instruction descriptions — the single source of semantics (ADR-0006).
//!
//! One generic function per instruction class, transcribed from the Arm ARM "Operation"
//! pseudocode of each instruction. Rust control flow is used only on decode-time
//! constants; see the rules in the [parent module](super).

use super::{BinOp, BranchKind, CmpOp, Exception, Sem, Ty};
use crate::cpu::{GpReg, XReg};
use crate::decode::{AddSubOp, Cond, Inst, MoveWideOp};

/// Lowers one decoded instruction onto backend `s`.
///
/// # Errors
///
/// Whatever the backend's [`Sem::raise`] returns for an exception the instruction
/// raises (e.g. [`Exception::Undefined`] for [`Inst::Unknown`]).
pub fn lower<S: Sem>(s: &mut S, inst: Inst) -> Result<(), S::Abort> {
    match inst {
        Inst::MoveWide {
            op,
            sf,
            shift,
            imm16,
            rd,
        } => move_wide(s, op, sf, shift, imm16, rd),
        Inst::AddSubImm {
            op,
            sf,
            set_flags,
            imm12,
            shift12,
            rd,
            rn,
        } => add_sub_imm(s, op, sf, set_flags, imm12, shift12, rd, rn),
        Inst::B { offset } => s.branch(None, offset, BranchKind::Jump),
        Inst::Bl { offset } => {
            let ret = s.pc_rel(4);
            s.set_gpr(GpReg::X(XReg::LR), Ty::I64, ret);
            s.branch(None, offset, BranchKind::Call);
        }
        Inst::Br { rn } => branch_reg(s, rn, false, BranchKind::Jump),
        Inst::Blr { rn } => branch_reg(s, rn, true, BranchKind::Call),
        Inst::Ret { rn } => branch_reg(s, rn, false, BranchKind::Return),
        Inst::CompareBranch {
            nonzero,
            sf,
            rt,
            offset,
        } => {
            let ty = Ty::from_sf(sf);
            let value = s.gpr(rt, ty);
            let zero = s.imm(ty, 0);
            let cc = if nonzero { CmpOp::Ne } else { CmpOp::Eq };
            let taken = s.icmp(cc, ty, value, zero);
            s.branch(Some(taken), offset, BranchKind::Jump);
        }
        Inst::BCond { cond, offset } => {
            // `AL` and `NV` always hold (ConditionHolds); that is a decode-time fact, so
            // the branch is unconditional without consulting the flags.
            let taken = match cond {
                Cond::Al | Cond::Nv => None,
                _ => {
                    let flags = s.nzcv();
                    Some(s.cond_holds(cond, flags))
                }
            };
            s.branch(taken, offset, BranchKind::Jump);
        }
        Inst::Nop => {}
        Inst::Unknown(word) => return s.raise(Exception::Undefined { word }),
    }
    Ok(())
}

/// `x << shift` for a decode-time constant; 0 if the shift is out of range (never for
/// decoder output, where `shift <= 48`).
fn shl_const(x: u64, shift: u32) -> u64 {
    x.checked_shl(shift).unwrap_or(0)
}

/// `MOVN/MOVZ/MOVK`.
///
/// ```text
/// result = if opcode == MoveWideOp_K then X[d, datasize] else Zeros(datasize);
/// result<pos+15:pos> = imm16;
/// if opcode == MoveWideOp_N then result = NOT(result);
/// X[d, datasize] = result;
/// ```
fn move_wide<S: Sem>(s: &mut S, op: MoveWideOp, sf: bool, shift: u8, imm16: u16, rd: GpReg) {
    let ty = Ty::from_sf(sf);
    let shift = u32::from(shift);
    let field = shl_const(u64::from(imm16), shift);
    let result = match op {
        // `imm` truncates to the datasize, so `NOT` is exactly `NOT(result)` at `ty`.
        MoveWideOp::Movz => s.imm(ty, field),
        MoveWideOp::Movn => s.imm(ty, !field),
        MoveWideOp::Movk => {
            let old = s.gpr(rd, ty);
            let keep = s.imm(ty, !shl_const(0xffff, shift));
            let kept = s.binop(BinOp::And, ty, old, keep);
            let field = s.imm(ty, field);
            s.binop(BinOp::Or, ty, kept, field)
        }
    };
    s.set_gpr(rd, ty, result);
}

/// `ADD/ADDS/SUB/SUBS` (immediate).
///
/// ```text
/// operand1 = if n == 31 then SP[] else X[n];
/// if sub_op then operand2 = NOT(imm); carry_in = '1'; else operand2 = imm; carry_in = '0';
/// (result, nzcv) = AddWithCarry(operand1, operand2, carry_in);
/// if setflags then PSTATE.<N,Z,C,V> = nzcv;
/// if d == 31 && !setflags then SP[] = result; else X[d] = result;
/// ```
///
/// Register 31 (`SP` vs `ZR`) is already resolved in `rd`/`rn` by the decoder.
#[allow(clippy::too_many_arguments)] // mirrors the encoding fields one-to-one
fn add_sub_imm<S: Sem>(
    s: &mut S,
    op: AddSubOp,
    sf: bool,
    set_flags: bool,
    imm12: u16,
    shift12: bool,
    rd: GpReg,
    rn: GpReg,
) {
    let ty = Ty::from_sf(sf);
    let imm = shl_const(u64::from(imm12), if shift12 { 12 } else { 0 });
    let operand1 = s.gpr(rn, ty);
    let (operand2, carry_in) = match op {
        AddSubOp::Add => (s.imm(ty, imm), s.imm(Ty::I1, 0)),
        AddSubOp::Sub => (s.imm(ty, !imm), s.imm(Ty::I1, 1)),
    };
    let (result, nzcv) = s.add_with_carry(ty, operand1, operand2, carry_in);
    if set_flags {
        s.set_nzcv(nzcv);
    }
    s.set_gpr(rd, ty, result);
}

/// `BR`, `BLR`, `RET`: the target is read **before** the link register is written, so
/// `BLR X30` jumps to the old `X30`.
fn branch_reg<S: Sem>(s: &mut S, rn: GpReg, link: bool, kind: BranchKind) {
    let target = s.gpr(rn, Ty::I64);
    if link {
        let ret = s.pc_rel(4);
        s.set_gpr(GpReg::X(XReg::LR), Ty::I64, ret);
    }
    s.branch_indirect(target, kind);
}
