//! A64 instruction descriptions — the single source of semantics (ADR-0006).
//!
//! One generic function per instruction class, transcribed from the Arm ARM "Operation"
//! pseudocode of each instruction. Rust control flow is used only on decode-time
//! constants and the translation context; see the rules in the [parent module](super).

use super::{BinOp, BranchKind, CmpOp, DataIss, Exception, MemAttrs, Sem, Ty, UnOp};
use crate::cpu::{GpReg, SysReg, XReg};
use crate::decode::{
    AddSubCarry, AddSubOp, AddSubReg, AddrMode, Bitfield, BitfieldOp, CondCmpRhs, CondCompare,
    CondSelOp, CondSelect, DataProc1, DataProc2, DpOp1, DpOp2, Extend, Extract, HintClass, Inst,
    LdStOp, LiteralOp, LoadStore, LoadStorePair, LogicOp, LogicalImm, LogicalReg, MoveWideOp,
    MulAdd, MulOp, PairMode, PairOp, RegOperand, Shift, hint_class,
};

/// Lowers one decoded instruction onto backend `s`.
///
/// # Errors
///
/// Whatever the backend's raising operations return (e.g. [`Exception::Undefined`] for
/// [`Inst::Unknown`], data aborts of loads and stores).
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
        } => {
            let ty = Ty::from_sf(sf);
            let imm = shl_const(u64::from(imm12), if shift12 { 12 } else { 0 });
            let operand1 = s.gpr(rn, ty);
            let operand2 = s.imm(ty, imm);
            add_sub(s, op, ty, set_flags, operand1, operand2, rd);
        }
        Inst::Adr { page, imm, rd } => adr(s, page, imm, rd),
        Inst::LogicalImm(i) => logical_imm(s, i),
        Inst::Bitfield(i) => bitfield(s, i),
        Inst::Extract(i) => extract(s, i),
        Inst::LogicalReg(i) => logical_reg(s, i),
        Inst::AddSubReg(i) => add_sub_reg(s, i),
        Inst::AddSubCarry(i) => add_sub_carry(s, i),
        Inst::CondCompare(i) => cond_compare(s, i),
        Inst::CondSelect(i) => cond_select(s, i),
        Inst::MulAdd(i) => mul_add(s, i),
        Inst::DataProc2(i) => data_proc2(s, i),
        Inst::DataProc1(i) => data_proc1(s, i),
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
        Inst::TestBranch {
            nonzero,
            bit,
            rt,
            offset,
        } => {
            // operand<bit_pos> == op: test the bit through a mask.
            let value = s.gpr(rt, Ty::I64);
            let mask = s.imm(Ty::I64, shl_const(1, u32::from(bit)));
            let masked = s.binop(BinOp::And, Ty::I64, value, mask);
            let zero = s.imm(Ty::I64, 0);
            let cc = if nonzero { CmpOp::Ne } else { CmpOp::Eq };
            let taken = s.icmp(cc, Ty::I64, masked, zero);
            s.branch(Some(taken), offset, BranchKind::Jump);
        }
        Inst::BCond { cond, offset } => {
            // `AL` and `NV` always hold (ConditionHolds); that is a decode-time fact, so
            // the branch is unconditional without consulting the flags.
            let taken = if cond.is_always() {
                None
            } else {
                let flags = s.nzcv();
                Some(s.cond_holds(cond, flags))
            };
            s.branch(taken, offset, BranchKind::Jump);
        }
        Inst::Svc { imm } => return s.raise(Exception::SupervisorCall { imm }),
        Inst::Brk { imm } => return s.raise(Exception::Breakpoint { imm }),
        Inst::Nop => {}
        Inst::Hint { imm } => return hint(s, imm),
        Inst::Mrs { reg, rt } => {
            let value = match reg {
                SysReg::Nzcv => {
                    let f = s.nzcv();
                    s.flags_to_bits(f)
                }
                _ => s.sysreg(reg),
            };
            s.set_gpr(rt, Ty::I64, value);
        }
        Inst::Msr { reg, rt } => {
            let value = s.gpr(rt, Ty::I64);
            match reg {
                // `bits_to_flags` keeps exactly the writable bits 31:28.
                SysReg::Nzcv => {
                    let f = s.bits_to_flags(value);
                    s.set_nzcv(f);
                }
                _ => {
                    let mask = reg.write_mask();
                    let value = if mask == u64::MAX {
                        value
                    } else {
                        let m = s.imm(Ty::I64, mask);
                        s.binop(BinOp::And, Ty::I64, value, m)
                    };
                    s.set_sysreg(reg, value);
                }
            }
        }
        Inst::LoadLiteral { op, rt, offset } => return load_literal(s, op, rt, offset),
        Inst::LoadStore(i) => return load_store(s, i),
        Inst::LoadStorePair(i) => return load_store_pair(s, i),
        Inst::Unknown(word) => return s.raise(Exception::Undefined { word }),
    }
    Ok(())
}

/// `x << shift` for a decode-time constant; 0 if the shift is out of range.
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

/// The common tail of every `ADD/SUB(S)` form.
///
/// ```text
/// if sub_op then (result, nzcv) = AddWithCarry(operand1, NOT(operand2), '1');
/// else           (result, nzcv) = AddWithCarry(operand1, operand2, '0');
/// if setflags then PSTATE.<N,Z,C,V> = nzcv;
/// X[d] = result;   // SP[] for the non-flag-setting immediate/extended forms
/// ```
///
/// Register 31 (`SP` vs `ZR`) is already resolved in `rd` by the decoder.
fn add_sub<S: Sem>(
    s: &mut S,
    op: AddSubOp,
    ty: Ty,
    set_flags: bool,
    operand1: S::V,
    operand2: S::V,
    rd: GpReg,
) {
    let (result, nzcv) = match op {
        AddSubOp::Add => {
            let cin = s.imm(Ty::I1, 0);
            s.add_with_carry(ty, operand1, operand2, cin)
        }
        AddSubOp::Sub => {
            let cin = s.imm(Ty::I1, 1);
            s.sub_with_carry(ty, operand1, operand2, cin)
        }
    };
    if set_flags {
        s.set_nzcv(nzcv);
    }
    s.set_gpr(rd, ty, result);
}

/// `ADR/ADRP`: `base = PC[]; if page then base<11:0> = Zeros(12); X[d] = base + imm`.
#[inline(never)]
fn adr<S: Sem>(s: &mut S, page: bool, imm: i64, rd: GpReg) {
    let result = if page {
        let pc = s.pc_rel(0);
        let mask = s.imm(Ty::I64, !0xfff);
        let base = s.binop(BinOp::And, Ty::I64, pc, mask);
        // Two's complement: adding the bit pattern of a negative offset wraps.
        let offset = s.imm(Ty::I64, imm as u64);
        s.binop(BinOp::Add, Ty::I64, base, offset)
    } else {
        s.pc_rel(imm)
    };
    s.set_gpr(rd, Ty::I64, result);
}

/// Applies a logical operation; `ANDS` also produces flags.
fn logic<S: Sem>(s: &mut S, op: LogicOp, ty: Ty, a: S::V, b: S::V) -> (S::V, Option<S::F>) {
    match op {
        LogicOp::And => (s.binop(BinOp::And, ty, a, b), None),
        LogicOp::Orr => (s.binop(BinOp::Or, ty, a, b), None),
        LogicOp::Eor => (s.binop(BinOp::Xor, ty, a, b), None),
        LogicOp::Ands => {
            let r = s.binop(BinOp::And, ty, a, b);
            let f = s.flags_logic(ty, r);
            (r, Some(f))
        }
    }
}

/// `AND/ORR/EOR/ANDS (immediate)`:
/// `result = operand1 OP imm; if setflags then PSTATE.<N,Z,C,V> = result<N-1>:IsZeroBit(result):'00';`
/// `X[d] = result` (`SP[]` for the non-flag-setting forms, resolved in `rd`).
#[inline(never)]
fn logical_imm<S: Sem>(s: &mut S, i: LogicalImm) {
    let ty = Ty::from_sf(i.sf);
    let a = s.gpr(i.rn, ty);
    let b = s.imm(ty, i.imm);
    let (r, f) = logic(s, i.op, ty, a, b);
    if let Some(f) = f {
        s.set_nzcv(f);
    }
    s.set_gpr(i.rd, ty, r);
}

/// `ShiftReg(m, shift_type, amount)` with a decode-time amount `< datasize`.
fn shift_reg<S: Sem>(s: &mut S, ty: Ty, rm: GpReg, shift: Shift, amount: u8) -> S::V {
    let value = s.gpr(rm, ty);
    if amount == 0 {
        return value;
    }
    let op = match shift {
        Shift::Lsl => BinOp::Shl,
        Shift::Lsr => BinOp::Lshr,
        Shift::Asr => BinOp::Ashr,
        Shift::Ror => BinOp::Ror,
    };
    let n = s.imm(ty, u64::from(amount));
    s.binop(op, ty, value, n)
}

/// `ExtendReg(m, extend_type, shift)` at width `ty` (`shift <= 4`):
///
/// ```text
/// val = X[m]; len = Min(<source width>, N - shift);
/// return Extend(val<len-1:0>:Zeros(shift), N, unsigned);
/// ```
///
/// which equals `Extend(val<source>, N) << shift` truncated to `N`.
fn extend_reg<S: Sem>(s: &mut S, ty: Ty, rm: GpReg, extend: Extend, shift: u8) -> S::V {
    let src = extend.source();
    let value = if src.bits() >= ty.bits() {
        // UXTX/SXTX, or UXTW/SXTW at 32 bits: no extension, just the low N bits.
        s.gpr(rm, ty)
    } else {
        let v = s.gpr(rm, src);
        if extend.signed() {
            s.sext(src, ty, v)
        } else {
            s.zext(src, ty, v)
        }
    };
    if shift == 0 {
        return value;
    }
    let n = s.imm(ty, u64::from(shift));
    s.binop(BinOp::Shl, ty, value, n)
}

/// Logical (shifted register):
/// `operand2 = ShiftReg(m, shift_type, shift_amount); if invert then operand2 = NOT(operand2);`
/// then as [`logical_imm`] (all registers are `ZR` when 31).
#[inline(never)]
fn logical_reg<S: Sem>(s: &mut S, i: LogicalReg) {
    let ty = Ty::from_sf(i.sf);
    let a = s.gpr(i.rn, ty);
    let b = shift_reg(s, ty, i.rm, i.shift, i.amount);
    let b = if i.invert { s.not(ty, b) } else { b };
    let (r, f) = logic(s, i.op, ty, a, b);
    if let Some(f) = f {
        s.set_nzcv(f);
    }
    s.set_gpr(i.rd, ty, r);
}

/// `ADD/SUB(S)` (shifted register: `operand2 = ShiftReg(m, …)`; extended register:
/// `operand1 = if n == 31 then SP[] else X[n]; operand2 = ExtendReg(m, …)`).
#[inline(never)]
fn add_sub_reg<S: Sem>(s: &mut S, i: AddSubReg) {
    let ty = Ty::from_sf(i.sf);
    let operand1 = s.gpr(i.rn, ty);
    let operand2 = match i.operand {
        RegOperand::Shifted { shift, amount } => shift_reg(s, ty, i.rm, shift, amount),
        RegOperand::Extended { extend, amount } => extend_reg(s, ty, i.rm, extend, amount),
    };
    add_sub(s, i.op, ty, i.set_flags, operand1, operand2, i.rd);
}

/// `ADC/ADCS/SBC/SBCS`:
/// `(result, nzcv) = AddWithCarry(X[n], sub ? NOT(X[m]) : X[m], PSTATE.C)`.
#[inline(never)]
fn add_sub_carry<S: Sem>(s: &mut S, i: AddSubCarry) {
    let ty = Ty::from_sf(i.sf);
    let a = s.gpr(i.rn, ty);
    let b = s.gpr(i.rm, ty);
    let flags = s.nzcv();
    let c = s.flag_c(flags);
    let (result, nzcv) = match i.op {
        AddSubOp::Add => s.add_with_carry(ty, a, b, c),
        AddSubOp::Sub => s.sub_with_carry(ty, a, b, c),
    };
    if i.set_flags {
        s.set_nzcv(nzcv);
    }
    s.set_gpr(i.rd, ty, result);
}

/// `CCMN/CCMP`:
///
/// ```text
/// flags = nzcv;
/// if ConditionHolds(cond) then
///     (-, flags) = AddWithCarry(operand1, sub_op ? NOT(operand2) : operand2, sub_op);
/// PSTATE.<N,Z,C,V> = flags;
/// ```
#[inline(never)]
fn cond_compare<S: Sem>(s: &mut S, i: CondCompare) {
    let ty = Ty::from_sf(i.sf);
    let a = s.gpr(i.rn, ty);
    let b = match i.rhs {
        CondCmpRhs::Reg(rm) => s.gpr(rm, ty),
        CondCmpRhs::Imm(imm) => s.imm(ty, u64::from(imm)),
    };
    let (_, cmp) = match i.op {
        AddSubOp::Add => {
            let cin = s.imm(Ty::I1, 0);
            s.add_with_carry(ty, a, b, cin)
        }
        AddSubOp::Sub => {
            let cin = s.imm(Ty::I1, 1);
            s.sub_with_carry(ty, a, b, cin)
        }
    };
    let result = if i.cond.is_always() {
        cmp
    } else {
        let flags = s.nzcv();
        let holds = s.cond_holds(i.cond, flags);
        let imm = s.imm(Ty::I64, u64::from(i.nzcv & 0xf) << 28);
        let fallback = s.bits_to_flags(imm);
        s.select_flags(holds, cmp, fallback)
    };
    s.set_nzcv(result);
}

/// `CSEL/CSINC/CSINV/CSNEG`:
///
/// ```text
/// if ConditionHolds(cond) then result = X[n];
/// else result = X[m]; if else_inv then result = NOT(result); if else_inc then result = result + 1;
/// ```
#[inline(never)]
fn cond_select<S: Sem>(s: &mut S, i: CondSelect) {
    let ty = Ty::from_sf(i.sf);
    let a = s.gpr(i.rn, ty);
    let m = s.gpr(i.rm, ty);
    let b = match i.op {
        CondSelOp::Csel => m,
        CondSelOp::Csinc => {
            let one = s.imm(ty, 1);
            s.binop(BinOp::Add, ty, m, one)
        }
        CondSelOp::Csinv => s.not(ty, m),
        CondSelOp::Csneg => {
            // NOT(m) + 1 == 0 - m (wrapping).
            let zero = s.imm(ty, 0);
            s.binop(BinOp::Sub, ty, zero, m)
        }
    };
    let result = if i.cond.is_always() {
        a
    } else {
        let flags = s.nzcv();
        let holds = s.cond_holds(i.cond, flags);
        s.select(ty, holds, a, b)
    };
    s.set_gpr(i.rd, ty, result);
}

/// Data processing (3 source).
///
/// ```text
/// MADD/MSUB:    result = X[a] ± X[n] * X[m]                (datasize)
/// S/UMADDL/SUBL: result = X[a] ± Int(W[n], unsigned) * Int(W[m], unsigned)   (64)
/// S/UMULH:      result = (Int(X[n]) * Int(X[m]))<127:64>
/// ```
#[inline(never)]
fn mul_add<S: Sem>(s: &mut S, i: MulAdd) {
    let ty = Ty::from_sf(i.sf);
    let (product, subtract) = match i.op {
        MulOp::Madd | MulOp::Msub => {
            let n = s.gpr(i.rn, ty);
            let m = s.gpr(i.rm, ty);
            (s.binop(BinOp::Mul, ty, n, m), i.op == MulOp::Msub)
        }
        MulOp::Smaddl | MulOp::Smsubl | MulOp::Umaddl | MulOp::Umsubl => {
            let signed = matches!(i.op, MulOp::Smaddl | MulOp::Smsubl);
            let n = s.gpr(i.rn, Ty::I32);
            let m = s.gpr(i.rm, Ty::I32);
            let (n, m) = if signed {
                (s.sext(Ty::I32, Ty::I64, n), s.sext(Ty::I32, Ty::I64, m))
            } else {
                (s.zext(Ty::I32, Ty::I64, n), s.zext(Ty::I32, Ty::I64, m))
            };
            // The exact 64-bit product of two 32-bit values; mod 2^64 is exact.
            (
                s.binop(BinOp::Mul, Ty::I64, n, m),
                matches!(i.op, MulOp::Smsubl | MulOp::Umsubl),
            )
        }
        MulOp::Smulh | MulOp::Umulh => {
            let n = s.gpr(i.rn, Ty::I64);
            let m = s.gpr(i.rm, Ty::I64);
            let op = if i.op == MulOp::Smulh {
                BinOp::SMulHi
            } else {
                BinOp::UMulHi
            };
            let hi = s.binop(op, Ty::I64, n, m);
            s.set_gpr(i.rd, Ty::I64, hi);
            return;
        }
    };
    let a = s.gpr(i.ra, ty);
    let op = if subtract { BinOp::Sub } else { BinOp::Add };
    let result = s.binop(op, ty, a, product);
    s.set_gpr(i.rd, ty, result);
}

/// `UDIV/SDIV` (`x / 0 = 0`, `MIN / -1 = MIN`, rounding towards zero) and
/// `LSLV/LSRV/ASRV/RORV` (`ShiftReg(n, type, UInt(X[m]) MOD datasize)`).
#[inline(never)]
fn data_proc2<S: Sem>(s: &mut S, i: DataProc2) {
    let ty = Ty::from_sf(i.sf);
    let n = s.gpr(i.rn, ty);
    let m = s.gpr(i.rm, ty);
    let op = match i.op {
        DpOp2::Udiv => BinOp::UDiv,
        DpOp2::Sdiv => BinOp::SDiv,
        DpOp2::Lslv => BinOp::Shl,
        DpOp2::Lsrv => BinOp::Lshr,
        DpOp2::Asrv => BinOp::Ashr,
        DpOp2::Rorv => BinOp::Ror,
    };
    let r = s.binop(op, ty, n, m);
    s.set_gpr(i.rd, ty, r);
}

/// `RBIT`, `REV16`, `REV32`, `REV`, `CLZ`, `CLS`.
///
/// `REV16/REV32` are `Reverse(operand, container)` for 16/32-bit containers;
/// `CLS(x) = CountLeadingZeroBits(x<N-1:1> EOR x<N-2:0>)`, computed as
/// `CLZ(((x LSR 1) EOR x) AND Ones(N-1)) - 1`.
#[inline(never)]
fn data_proc1<S: Sem>(s: &mut S, i: DataProc1) {
    let ty = Ty::from_sf(i.sf);
    let x = s.gpr(i.rn, ty);
    let r = match i.op {
        DpOp1::Rbit => s.unop(UnOp::Rbit, ty, x),
        DpOp1::Rev => s.unop(UnOp::Bswap, ty, x),
        DpOp1::Clz => s.unop(UnOp::Clz, ty, x),
        DpOp1::Rev32 => {
            // 64-bit only: swapping all bytes reverses the word order too; rotating by
            // 32 puts the words back.
            let swapped = s.unop(UnOp::Bswap, ty, x);
            let n = s.imm(ty, 32);
            s.binop(BinOp::Ror, ty, swapped, n)
        }
        DpOp1::Rev16 => {
            // ((x LSR 8) AND 0x00ff..) OR ((x LSL 8) AND 0xff00..)
            let lo = s.imm(ty, 0x00ff_00ff_00ff_00ff);
            let hi = s.imm(ty, 0xff00_ff00_ff00_ff00);
            let eight = s.imm(ty, 8);
            let down = s.binop(BinOp::Lshr, ty, x, eight);
            let down = s.binop(BinOp::And, ty, down, lo);
            let up = s.binop(BinOp::Shl, ty, x, eight);
            let up = s.binop(BinOp::And, ty, up, hi);
            s.binop(BinOp::Or, ty, down, up)
        }
        DpOp1::Cls => {
            let one = s.imm(ty, 1);
            let shifted = s.binop(BinOp::Lshr, ty, x, one);
            let diff = s.binop(BinOp::Xor, ty, shifted, x);
            let low = s.imm(ty, ty.mask() >> 1);
            let diff = s.binop(BinOp::And, ty, diff, low);
            let lz = s.unop(UnOp::Clz, ty, diff);
            s.binop(BinOp::Sub, ty, lz, one)
        }
    };
    s.set_gpr(i.rd, ty, r);
}

/// `SBFM/BFM/UBFM`:
///
/// ```text
/// dst = if inzero then Zeros() else X[d]; src = X[n];
/// bot = (dst AND NOT(wmask)) OR (ROR(src, R) AND wmask);
/// top = if extend then Replicate(src<S>) else dst;
/// X[d] = (top AND NOT(tmask)) OR (bot AND tmask);
/// ```
#[inline(never)]
fn bitfield<S: Sem>(s: &mut S, i: Bitfield) {
    let ty = Ty::from_sf(i.sf);
    let (inzero, extend) = match i.op {
        BitfieldOp::Sbfm => (true, true),
        BitfieldOp::Bfm => (false, false),
        BitfieldOp::Ubfm => (true, false),
    };
    let src = s.gpr(i.rn, ty);
    let rotated = if i.immr == 0 {
        src
    } else {
        let r = s.imm(ty, u64::from(i.immr));
        s.binop(BinOp::Ror, ty, src, r)
    };
    let wmask = s.imm(ty, i.wmask);
    let not_wmask = s.imm(ty, !i.wmask);
    let tmask = s.imm(ty, i.tmask);
    let not_tmask = s.imm(ty, !i.tmask);
    let rot_w = s.binop(BinOp::And, ty, rotated, wmask);
    let (bot, dst) = if inzero {
        (rot_w, None)
    } else {
        let dst = s.gpr(i.rd, ty);
        let kept = s.binop(BinOp::And, ty, dst, not_wmask);
        (s.binop(BinOp::Or, ty, kept, rot_w), Some(dst))
    };
    let top = if extend {
        // Replicate(src<S>): move bit S to the top, then shift it across the word.
        let up = s.imm(ty, u64::from(ty.bits() - 1 - u32::from(i.imms)));
        let top_bit = s.binop(BinOp::Shl, ty, src, up);
        let down = s.imm(ty, u64::from(ty.bits() - 1));
        Some(s.binop(BinOp::Ashr, ty, top_bit, down))
    } else {
        dst
    };
    let bot_t = s.binop(BinOp::And, ty, bot, tmask);
    let result = match top {
        // `top` is zero (UBFM): `(0 AND NOT(tmask)) OR (bot AND tmask)`.
        None => bot_t,
        Some(top) => {
            let top_t = s.binop(BinOp::And, ty, top, not_tmask);
            s.binop(BinOp::Or, ty, top_t, bot_t)
        }
    };
    s.set_gpr(i.rd, ty, result);
}

/// `EXTR`: `concat = X[n]:X[m]; X[d] = concat<lsb+datasize-1:lsb>`.
#[inline(never)]
fn extract<S: Sem>(s: &mut S, i: Extract) {
    let ty = Ty::from_sf(i.sf);
    let low = s.gpr(i.rm, ty);
    let result = if i.lsb == 0 {
        low
    } else {
        let high = s.gpr(i.rn, ty);
        let lsb = s.imm(ty, u64::from(i.lsb));
        let rest = s.imm(ty, u64::from(ty.bits() - u32::from(i.lsb)));
        let lo = s.binop(BinOp::Lshr, ty, low, lsb);
        let hi = s.binop(BinOp::Shl, ty, high, rest);
        s.binop(BinOp::Or, ty, hi, lo)
    };
    s.set_gpr(i.rd, ty, result);
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

/// `HINT #imm` (see [`hint_class`]). The PAC hints are NOPs exactly when their key is
/// disabled (`AddPACIA`/`AuthIA` return the pointer unchanged), which the translation
/// context knows.
#[inline(never)]
fn hint<S: Sem>(s: &mut S, imm: u8) -> Result<(), S::Abort> {
    let implemented = match hint_class(imm) {
        HintClass::Nop => true,
        HintClass::PacKeyA => !s.ctx().pac_ia,
        HintClass::PacKeyB => !s.ctx().pac_ib,
        HintClass::Unimplemented => false,
    };
    if implemented {
        Ok(())
    } else {
        s.raise(Exception::Unimplemented {
            word: 0xd503_201f | (u32::from(imm) << 5),
        })
    }
}

/// `CheckSPAlignment()` for a load/store with base `SP`, if the context enables it.
fn check_sp_alignment<S: Sem>(s: &mut S, rn: GpReg, base: S::V) -> Result<(), S::Abort> {
    if rn == GpReg::Sp && s.ctx().sp_align_check {
        let fifteen = s.imm(Ty::I64, 15);
        let low = s.binop(BinOp::And, Ty::I64, base, fifteen);
        let zero = s.imm(Ty::I64, 0);
        let misaligned = s.icmp(CmpOp::Ne, Ty::I64, low, zero);
        s.raise_if(misaligned, Exception::SpAlignment)?;
    }
    Ok(())
}

/// Register number of `r` for the syndrome (`ZR` and `SP` are 31).
fn reg_number(r: GpReg) -> u8 {
    match r {
        GpReg::X(x) => x.index(),
        GpReg::Sp | GpReg::Zr => 31,
    }
}

/// Extends a loaded value of type `size` into the register type and writes it.
fn write_loaded<S: Sem>(s: &mut S, op: LdStOp, size: Ty, rt: GpReg, data: S::V) {
    let (ty, value) = match op {
        LdStOp::LoadSigned32 => (Ty::I32, s.sext(size, Ty::I32, data)),
        LdStOp::LoadSigned64 => (Ty::I64, s.sext(size, Ty::I64, data)),
        // Zero-extending: a `Wt` write of a byte/halfword clears bits 63:8/16 anyway.
        _ => (size, data),
    };
    s.set_gpr(rt, ty, value);
}

/// `LDR/LDRSW/PRFM (literal)`: `address = PC[] + offset; data = Mem[address, size];`
/// `X[t] = signed ? SignExtend(data, 64) : data`. `PRFM` is a hint without access.
#[inline(never)]
fn load_literal<S: Sem>(s: &mut S, op: LiteralOp, rt: GpReg, offset: i64) -> Result<(), S::Abort> {
    let (size, ldst, sixty_four) = match op {
        LiteralOp::Prfm => return Ok(()),
        LiteralOp::Ldr32 => (Ty::I32, LdStOp::Load, false),
        LiteralOp::Ldr64 => (Ty::I64, LdStOp::Load, true),
        LiteralOp::Ldrsw => (Ty::I32, LdStOp::LoadSigned64, true),
    };
    let attrs = MemAttrs {
        iss: Some(DataIss {
            srt: reg_number(rt),
            sign_extend: ldst == LdStOp::LoadSigned64,
            sixty_four,
        }),
        ..MemAttrs::PLAIN
    };
    let address = s.pc_rel(offset);
    let data = s.load(size, address, attrs)?;
    write_loaded(s, ldst, size, rt, data);
    Ok(())
}

/// Single-register loads and stores (all addressing modes):
///
/// ```text
/// if n == 31 then (if memop != PREFETCH then CheckSPAlignment()); address = SP[];
/// else address = X[n];
/// if !postindex then address = address + offset;
/// STORE: Mem[address, datasize] = X[t];
/// LOAD:  data = Mem[address, datasize]; X[t] = Extend(data, regsize, signed);
/// PREFETCH: Prefetch(address, t)   // a hint: no access, no fault
/// if wback then (if postindex then address = address + offset); X[n]/SP[] = address;
/// ```
///
/// For the register-offset form `offset = ExtendReg(m, extend_type, shift)`.
#[inline(never)]
fn load_store<S: Sem>(s: &mut S, i: LoadStore) -> Result<(), S::Abort> {
    if i.op == LdStOp::Prefetch {
        return Ok(());
    }
    let base = s.gpr(i.rn, Ty::I64);
    check_sp_alignment(s, i.rn, base)?;
    let (offset, wback, post, unprivileged) = match i.mode {
        AddrMode::Unsigned(off) => (Some(s.imm(Ty::I64, off)), false, false, false),
        AddrMode::Unscaled(off) => (Some(s.imm(Ty::I64, off as u64)), false, false, false),
        AddrMode::Unprivileged(off) => (Some(s.imm(Ty::I64, off as u64)), false, false, true),
        AddrMode::PreIndex(off) => (Some(s.imm(Ty::I64, off as u64)), true, false, false),
        AddrMode::PostIndex(off) => (Some(s.imm(Ty::I64, off as u64)), true, true, false),
        AddrMode::Register { rm, extend, shift } => {
            let amount = if shift {
                i.size.bytes().trailing_zeros()
            } else {
                0
            };
            // `amount <= 3`.
            let off = extend_reg(s, Ty::I64, rm, extend, amount as u8);
            (Some(off), false, false, false)
        }
    };
    let offset_address = match offset {
        Some(off) => s.binop(BinOp::Add, Ty::I64, base, off),
        None => base,
    };
    let address = if post { base } else { offset_address };
    let sign_extend = matches!(i.op, LdStOp::LoadSigned32 | LdStOp::LoadSigned64);
    let attrs = MemAttrs {
        unprivileged,
        iss: if wback {
            None
        } else {
            Some(DataIss {
                srt: reg_number(i.rt),
                sign_extend,
                sixty_four: i.op == LdStOp::LoadSigned64
                    || (i.size == Ty::I64 && i.op != LdStOp::LoadSigned32),
            })
        },
        ..MemAttrs::PLAIN
    };
    if i.op == LdStOp::Store {
        let data = s.gpr(i.rt, i.size);
        s.store(i.size, address, data, attrs)?;
    } else {
        let data = s.load(i.size, address, attrs)?;
        write_loaded(s, i.op, i.size, i.rt, data);
    }
    if wback {
        s.set_gpr(i.rn, Ty::I64, offset_address);
    }
    Ok(())
}

/// `LDP/STP/LDNP/STNP/LDPSW`:
///
/// ```text
/// address = (n == 31) ? (CheckSPAlignment(), SP[]) : X[n];
/// if !postindex then address = address + offset;
/// STORE: Mem[address, dbytes] = X[t]; Mem[address + dbytes, dbytes] = X[t2];
/// LOAD:  data1 = Mem[address, dbytes]; data2 = Mem[address + dbytes, dbytes];
///        X[t] = Extend(data1); X[t2] = Extend(data2);   // sign-extended for LDPSW
/// if wback then X[n]/SP[] = postindex ? address + offset : address;
/// ```
///
/// The two stores are covered by one [`Sem::probe`] first (rule for multiple stores).
#[inline(never)]
fn load_store_pair<S: Sem>(s: &mut S, i: LoadStorePair) -> Result<(), S::Abort> {
    let elem = i.elem();
    let base = s.gpr(i.rn, Ty::I64);
    check_sp_alignment(s, i.rn, base)?;
    let off = s.imm(Ty::I64, i.offset as u64);
    let offset_address = s.binop(BinOp::Add, Ty::I64, base, off);
    let (address, wback) = match i.mode {
        PairMode::Offset | PairMode::NonTemporal => (offset_address, false),
        PairMode::PreIndex => (offset_address, true),
        PairMode::PostIndex => (base, true),
    };
    let step = s.imm(Ty::I64, u64::from(elem.bytes()));
    let address2 = s.binop(BinOp::Add, Ty::I64, address, step);
    let attrs = MemAttrs::PLAIN;
    match i.op {
        PairOp::Store => {
            let d1 = s.gpr(i.rt, elem);
            let d2 = s.gpr(i.rt2, elem);
            s.probe(elem, 2, address, attrs)?;
            s.store(elem, address, d1, attrs)?;
            s.store(elem, address2, d2, attrs)?;
        }
        PairOp::Load | PairOp::LoadSw => {
            let d1 = s.load(elem, address, attrs)?;
            let d2 = s.load(elem, address2, attrs)?;
            let op = if i.op == PairOp::LoadSw {
                LdStOp::LoadSigned64
            } else {
                LdStOp::Load
            };
            write_loaded(s, op, elem, i.rt, d1);
            write_loaded(s, op, elem, i.rt2, d2);
        }
    }
    if wback {
        s.set_gpr(i.rn, Ty::I64, offset_address);
    }
    Ok(())
}
