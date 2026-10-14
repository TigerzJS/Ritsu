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
//! | `op0`  | Class                                                     | Module |
//! |--------|-----------------------------------------------------------|--------|
//! | `100x` | Data processing — immediate                               | `dp_imm` |
//! | `101x` | Branches, exception generating and system instructions    | `branch` |
//! | `x1x0` | Loads and stores                                          | `ldst` |
//! | `x101` | Data processing — register                                | `dp_reg` |
//!
//! Covered (WP-DBT-02): the integer base ISA — all of data processing (immediate and
//! register) except FEAT_MTE/FEAT_CSSC/FEAT_FlagM/CRC32/PAC forms, all branches
//! except the PAC-authenticating ones, `SVC`, `BRK`, the hint space, `MRS/MSR` for
//! `NZCV`, `FPCR`, `FPSR`, `TPIDR_EL0`, and all integer loads/stores except
//! exclusive, acquire/release, atomic and PAC forms (WP-DBT-09/-11).
//!
//! Register 31 is resolved here, per operand (see [`GpReg`]), following the operand
//! notation of each encoding (`Xn|SP` vs `Xn`).
//!
//! **CONSTRAINED UNPREDICTABLE** encodings get one of the architecturally permitted
//! behaviours, chosen here once: loads/stores with writeback whose base register is also
//! a transfer register (`Rn == Rt`, `Rn != 31`) and `LDP` with `Rt == Rt2` are
//! **UNDEFINED** (`Constraint_UNDEF`), i.e. [`Inst::Unknown`]. `SMULH/UMULH` with
//! `Ra != 31` (the field "should be one") is UNDEFINED as well.

mod bitmask;
mod branch;
mod dp_imm;
mod dp_reg;
mod ldst;
#[cfg(test)]
mod llvm_diff;
#[cfg(test)]
mod tests;

pub use bitmask::decode_bit_masks;

use crate::cpu::{GpReg, Nzcv, SysReg};
use crate::sem::Ty;

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

    /// Whether the condition holds regardless of the flags (`AL`, `NV`).
    #[must_use]
    pub const fn is_always(self) -> bool {
        matches!(self, Self::Al | Self::Nv)
    }

    /// Evaluates the condition on `f` (Arm ARM `ConditionHolds()`). The pseudocode
    /// lives in the semantic layer, [`crate::sem::condition_holds`].
    #[must_use]
    pub const fn holds(self, f: Nzcv) -> bool {
        crate::sem::condition_holds(self, f)
    }
}

/// `ADD` or `SUB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddSubOp {
    /// Addition.
    Add,
    /// Subtraction (`Rn + NOT(operand2) + 1`).
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

/// Logical operations (`opc` field of the logical classes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogicOp {
    /// `00`: `AND` (with `N = 1` in the register form: `BIC`).
    And,
    /// `01`: `ORR` (`ORN`).
    Orr,
    /// `10`: `EOR` (`EON`).
    Eor,
    /// `11`: `ANDS` (`BICS`), sets `N` and `Z`, clears `C` and `V`.
    Ands,
}

/// Shift type of a shifted-register operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shift {
    /// `00` logical shift left.
    Lsl,
    /// `01` logical shift right.
    Lsr,
    /// `10` arithmetic shift right.
    Asr,
    /// `11` rotate right (logical instructions only).
    Ror,
}

impl Shift {
    /// From the 2-bit `shift` field.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        match bits & 3 {
            0 => Self::Lsl,
            1 => Self::Lsr,
            2 => Self::Asr,
            _ => Self::Ror,
        }
    }
}

/// Extension of an extended-register operand (`option` field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Extend {
    /// `000` zero-extend byte.
    Uxtb,
    /// `001` zero-extend halfword.
    Uxth,
    /// `010` zero-extend word.
    Uxtw,
    /// `011` doubleword (no extension).
    Uxtx,
    /// `100` sign-extend byte.
    Sxtb,
    /// `101` sign-extend halfword.
    Sxth,
    /// `110` sign-extend word.
    Sxtw,
    /// `111` doubleword (no extension).
    Sxtx,
}

impl Extend {
    /// From the 3-bit `option` field.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        match bits & 7 {
            0 => Self::Uxtb,
            1 => Self::Uxth,
            2 => Self::Uxtw,
            3 => Self::Uxtx,
            4 => Self::Sxtb,
            5 => Self::Sxth,
            6 => Self::Sxtw,
            _ => Self::Sxtx,
        }
    }

    /// The 3-bit encoding.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self as u32
    }

    /// Whether the source is sign-extended.
    #[must_use]
    pub const fn signed(self) -> bool {
        self.bits() & 4 != 0
    }

    /// Width of the source part of the register.
    #[must_use]
    pub const fn source(self) -> Ty {
        match self.bits() & 3 {
            0 => Ty::I8,
            1 => Ty::I16,
            2 => Ty::I32,
            _ => Ty::I64,
        }
    }
}

/// `MOVN/MOVZ/MOVK`-free logical immediate: `AND/ORR/EOR/ANDS (immediate)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LogicalImm {
    /// Operation.
    pub op: LogicOp,
    /// 64-bit.
    pub sf: bool,
    /// Raw `N:immr:imms` (13 bits), kept for re-encoding and disassembly.
    pub bitmask: u16,
    /// The decoded immediate (`DecodeBitMasks` `wmask`, canonical for the width).
    pub imm: u64,
    /// Destination: `SP` for `AND/ORR/EOR`, `ZR` for `ANDS` (when 31).
    pub rd: GpReg,
    /// Source (`ZR` when 31).
    pub rn: GpReg,
}

/// Bitfield operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BitfieldOp {
    /// `00` `SBFM` (aliases `ASR`, `SBFIZ`, `SBFX`, `SXTB/H/W`).
    Sbfm,
    /// `01` `BFM` (aliases `BFI`, `BFXIL`, `BFC`).
    Bfm,
    /// `10` `UBFM` (aliases `LSL`, `LSR`, `UBFIZ`, `UBFX`, `UXTB/H`).
    Ubfm,
}

/// `SBFM/BFM/UBFM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Bitfield {
    /// Operation.
    pub op: BitfieldOp,
    /// 64-bit (`N == sf`).
    pub sf: bool,
    /// Rotate amount `R`.
    pub immr: u8,
    /// Source top bit `S`.
    pub imms: u8,
    /// `DecodeBitMasks(N, imms, immr, FALSE).wmask`.
    pub wmask: u64,
    /// `DecodeBitMasks(N, imms, immr, FALSE).tmask`.
    pub tmask: u64,
    /// Destination (`ZR` when 31).
    pub rd: GpReg,
    /// Source (`ZR` when 31).
    pub rn: GpReg,
}

/// `EXTR` (alias `ROR (immediate)` when `Rn == Rm`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extract {
    /// 64-bit.
    pub sf: bool,
    /// Least significant bit position in `Rn:Rm`.
    pub lsb: u8,
    /// Destination.
    pub rd: GpReg,
    /// High part.
    pub rn: GpReg,
    /// Low part.
    pub rm: GpReg,
}

/// Logical (shifted register): `AND/BIC/ORR/ORN/EOR/EON/ANDS/BICS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LogicalReg {
    /// Operation.
    pub op: LogicOp,
    /// `N` bit: the shifted operand is inverted (`BIC/ORN/EON/BICS`).
    pub invert: bool,
    /// 64-bit.
    pub sf: bool,
    /// Shift type.
    pub shift: Shift,
    /// Shift amount (`< datasize`).
    pub amount: u8,
    /// Destination.
    pub rd: GpReg,
    /// First operand.
    pub rn: GpReg,
    /// Shifted operand.
    pub rm: GpReg,
}

/// Second operand of a register-form `ADD/SUB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegOperand {
    /// Shifted register (`LSL/LSR/ASR`, amount `< datasize`); registers use `ZR`.
    Shifted {
        /// Shift type (never `Ror`).
        shift: Shift,
        /// Shift amount.
        amount: u8,
    },
    /// Extended register, then shifted left by `amount` (`0..=4`); `Rd|SP` (unless
    /// flags are set) and `Rn|SP`.
    Extended {
        /// Extension.
        extend: Extend,
        /// Left shift.
        amount: u8,
    },
}

/// `ADD/ADDS/SUB/SUBS` (shifted or extended register).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AddSubReg {
    /// Add or subtract.
    pub op: AddSubOp,
    /// 64-bit.
    pub sf: bool,
    /// Update `NZCV`.
    pub set_flags: bool,
    /// Destination.
    pub rd: GpReg,
    /// First operand.
    pub rn: GpReg,
    /// Second operand register.
    pub rm: GpReg,
    /// How `rm` is modified.
    pub operand: RegOperand,
}

/// `ADC/ADCS/SBC/SBCS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AddSubCarry {
    /// `Add` = `ADC`, `Sub` = `SBC`.
    pub op: AddSubOp,
    /// 64-bit.
    pub sf: bool,
    /// Update `NZCV`.
    pub set_flags: bool,
    /// Destination.
    pub rd: GpReg,
    /// First operand.
    pub rn: GpReg,
    /// Second operand.
    pub rm: GpReg,
}

/// Second operand of `CCMP/CCMN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CondCmpRhs {
    /// Register form.
    Reg(GpReg),
    /// 5-bit unsigned immediate.
    Imm(u8),
}

/// `CCMN` (`op = Add`) / `CCMP` (`op = Sub`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CondCompare {
    /// `Add` = `CCMN`, `Sub` = `CCMP`.
    pub op: AddSubOp,
    /// 64-bit.
    pub sf: bool,
    /// Condition.
    pub cond: Cond,
    /// Flags to set when the condition fails (bit 3 = N … bit 0 = V).
    pub nzcv: u8,
    /// First operand.
    pub rn: GpReg,
    /// Second operand.
    pub rhs: CondCmpRhs,
}

/// Conditional-select operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CondSelOp {
    /// `Rd = cond ? Rn : Rm`.
    Csel,
    /// `Rd = cond ? Rn : Rm + 1`.
    Csinc,
    /// `Rd = cond ? Rn : NOT(Rm)`.
    Csinv,
    /// `Rd = cond ? Rn : -Rm`.
    Csneg,
}

/// `CSEL/CSINC/CSINV/CSNEG` (and aliases `CSET`, `CSETM`, `CINC`, `CINV`, `CNEG`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CondSelect {
    /// Operation.
    pub op: CondSelOp,
    /// 64-bit.
    pub sf: bool,
    /// Condition.
    pub cond: Cond,
    /// Destination.
    pub rd: GpReg,
    /// Selected if the condition holds.
    pub rn: GpReg,
    /// Modified and selected otherwise.
    pub rm: GpReg,
}

/// Data processing (3 source) operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MulOp {
    /// `Ra + Rn * Rm`.
    Madd,
    /// `Ra - Rn * Rm`.
    Msub,
    /// `Xa + SInt(Wn) * SInt(Wm)`.
    Smaddl,
    /// `Xa - SInt(Wn) * SInt(Wm)`.
    Smsubl,
    /// `Xa + UInt(Wn) * UInt(Wm)`.
    Umaddl,
    /// `Xa - UInt(Wn) * UInt(Wm)`.
    Umsubl,
    /// `(SInt(Xn) * SInt(Xm))<127:64>`.
    Smulh,
    /// `(UInt(Xn) * UInt(Xm))<127:64>`.
    Umulh,
}

/// `MADD/MSUB/SMADDL/SMSUBL/UMADDL/UMSUBL/SMULH/UMULH`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MulAdd {
    /// Operation.
    pub op: MulOp,
    /// 64-bit destination (always `true` except for 32-bit `MADD/MSUB`).
    pub sf: bool,
    /// Destination.
    pub rd: GpReg,
    /// First factor.
    pub rn: GpReg,
    /// Second factor.
    pub rm: GpReg,
    /// Addend (`ZR` for `SMULH/UMULH`, where the field must be 31).
    pub ra: GpReg,
}

/// Data processing (2 source) operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DpOp2 {
    /// Unsigned divide.
    Udiv,
    /// Signed divide.
    Sdiv,
    /// Shift left by `Rm MOD datasize`.
    Lslv,
    /// Logical shift right by `Rm MOD datasize`.
    Lsrv,
    /// Arithmetic shift right by `Rm MOD datasize`.
    Asrv,
    /// Rotate right by `Rm MOD datasize`.
    Rorv,
}

/// `UDIV/SDIV/LSLV/LSRV/ASRV/RORV`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DataProc2 {
    /// Operation.
    pub op: DpOp2,
    /// 64-bit.
    pub sf: bool,
    /// Destination.
    pub rd: GpReg,
    /// First operand.
    pub rn: GpReg,
    /// Second operand.
    pub rm: GpReg,
}

/// Data processing (1 source) operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DpOp1 {
    /// Reverse bits.
    Rbit,
    /// Reverse bytes in each halfword.
    Rev16,
    /// Reverse bytes in each word (64-bit only).
    Rev32,
    /// Reverse bytes in the whole register (`REV` for 32-bit, `REV`/`REV64` for 64).
    Rev,
    /// Count leading zeros.
    Clz,
    /// Count leading sign bits.
    Cls,
}

/// `RBIT/REV16/REV32/REV/CLZ/CLS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DataProc1 {
    /// Operation.
    pub op: DpOp1,
    /// 64-bit.
    pub sf: bool,
    /// Destination.
    pub rd: GpReg,
    /// Source.
    pub rn: GpReg,
}

/// How a hint behaves in this configuration (feature set of dbt.md §12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HintClass {
    /// Architecturally a NOP here: `YIELD`, `DGH`, `CSDB`, hints of features that are
    /// not reported (`ESB` without RAS, `PSB/TSB`, `GCSB`, `CLRBHB`, `BTI` (E-15),
    /// `PACM`, `CHKFEAT`, `STSHH`) and every unallocated hint (Arm ARM: unallocated
    /// `HINT` encodings execute as `NOP`).
    Nop,
    /// A PAC hint using instruction key A (`PACIASP`, `AUTIAZ`, `PACIA1716`, …): a NOP
    /// while `SCTLR_EL1.EnIA = 0`, otherwise not implemented yet (WP-DBT-11).
    PacKeyA,
    /// As [`HintClass::PacKeyA`] for key B (`SCTLR_EL1.EnIB`).
    PacKeyB,
    /// Has semantics that are not implemented yet: `WFE`, `WFI`, `SEV`, `SEVL`
    /// (event register, parking: WP-DBT-08) and `XPACLRI` (strips the PAC field
    /// regardless of key enables: WP-DBT-11).
    Unimplemented,
}

/// Classifies `HINT #imm` (`imm = CRm:op2`, 7 bits).
#[must_use]
pub const fn hint_class(imm: u8) -> HintClass {
    match imm {
        2..=5 | 7 => HintClass::Unimplemented,
        8 | 12 | 24 | 25 | 28 | 29 => HintClass::PacKeyA,
        10 | 14 | 26 | 27 | 30 | 31 => HintClass::PacKeyB,
        _ => HintClass::Nop,
    }
}

/// Literal-load operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LiteralOp {
    /// `LDR Wt, label`.
    Ldr32,
    /// `LDR Xt, label`.
    Ldr64,
    /// `LDRSW Xt, label`.
    Ldrsw,
    /// `PRFM <prfop>, label` (a hint: no access, no fault).
    Prfm,
}

/// Single-register load/store operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LdStOp {
    /// `STRB/STRH/STR/STUR*/STTR*`.
    Store,
    /// `LDRB/LDRH/LDR`: zero-extending load into `Wt` (sizes < 64) or `Xt`.
    Load,
    /// `LDRSB/LDRSH`: sign-extending load into `Wt`.
    LoadSigned32,
    /// `LDRSB/LDRSH/LDRSW`: sign-extending load into `Xt`.
    LoadSigned64,
    /// `PRFM/PRFUM` (a hint: no access, no fault). `rt` holds the `prfop` field.
    Prefetch,
}

/// Addressing mode of a single-register load/store. Offsets are in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddrMode {
    /// `[Xn|SP, #imm]` with unsigned, size-scaled 12-bit immediate.
    Unsigned(u64),
    /// `[Xn|SP, #simm]` unscaled 9-bit (`LDUR/STUR/PRFUM`).
    Unscaled(i64),
    /// `[Xn|SP, #simm]!` (address = base + offset, written back).
    PreIndex(i64),
    /// `[Xn|SP], #simm` (address = base, base + offset written back).
    PostIndex(i64),
    /// `[Xn|SP, #simm]` with EL0 permissions (`LDTR/STTR`).
    Unprivileged(i64),
    /// `[Xn|SP, Rm{, extend {#amount}}]`.
    Register {
        /// Offset register (`ZR` when 31).
        rm: GpReg,
        /// `UXTW`, `UXTX` (= `LSL`), `SXTW` or `SXTX`.
        extend: Extend,
        /// `S` bit: shift by the access size.
        shift: bool,
    },
}

/// `LDR*/STR*/LDUR*/STUR*/LDTR*/STTR*/PRFM/PRFUM` (integer registers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoadStore {
    /// Operation.
    pub op: LdStOp,
    /// Access size (`I8`, `I16`, `I32`, `I64`).
    pub size: Ty,
    /// Transfer register (`ZR` when 31), or the `prfop` field for prefetches.
    pub rt: GpReg,
    /// Base register (`SP` when 31).
    pub rn: GpReg,
    /// Addressing mode.
    pub mode: AddrMode,
}

/// Pair load/store operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PairOp {
    /// `STP/STNP`.
    Store,
    /// `LDP/LDNP`.
    Load,
    /// `LDPSW` (two sign-extended words into `Xt`, `Xt2`).
    LoadSw,
}

/// Addressing mode of a pair load/store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PairMode {
    /// `[Xn|SP, #imm]`.
    Offset,
    /// `[Xn|SP, #imm]!`.
    PreIndex,
    /// `[Xn|SP], #imm`.
    PostIndex,
    /// `LDNP/STNP` (non-temporal hint, otherwise like `Offset`).
    NonTemporal,
}

/// `LDP/STP/LDNP/STNP/LDPSW` (integer registers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoadStorePair {
    /// Operation.
    pub op: PairOp,
    /// 64-bit registers (always `true` for `LoadSw`, which loads words).
    pub sf: bool,
    /// First transfer register.
    pub rt: GpReg,
    /// Second transfer register.
    pub rt2: GpReg,
    /// Base register (`SP` when 31).
    pub rn: GpReg,
    /// Addressing mode.
    pub mode: PairMode,
    /// Byte offset (`imm7` scaled by the element size).
    pub offset: i64,
}

impl LoadStorePair {
    /// Size of one element in memory.
    #[must_use]
    pub const fn elem(&self) -> Ty {
        match (self.op, self.sf) {
            (PairOp::LoadSw, _) | (_, false) => Ty::I32,
            (_, true) => Ty::I64,
        }
    }
}

/// A decoded A64 instruction.
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
    /// `ADR/ADRP`.
    Adr {
        /// `ADRP`: the result is relative to the 4 KiB page of `PC`.
        page: bool,
        /// Byte offset (for `ADRP` already multiplied by 4096).
        imm: i64,
        /// Destination (`ZR` when 31).
        rd: GpReg,
    },
    /// `AND/ORR/EOR/ANDS (immediate)`.
    LogicalImm(LogicalImm),
    /// `SBFM/BFM/UBFM`.
    Bitfield(Bitfield),
    /// `EXTR`.
    Extract(Extract),
    /// Logical (shifted register).
    LogicalReg(LogicalReg),
    /// `ADD/SUB(S)` (shifted or extended register).
    AddSubReg(AddSubReg),
    /// `ADC/ADCS/SBC/SBCS`.
    AddSubCarry(AddSubCarry),
    /// `CCMN/CCMP` (register or immediate).
    CondCompare(CondCompare),
    /// `CSEL/CSINC/CSINV/CSNEG`.
    CondSelect(CondSelect),
    /// Data processing (3 source).
    MulAdd(MulAdd),
    /// Data processing (2 source).
    DataProc2(DataProc2),
    /// Data processing (1 source).
    DataProc1(DataProc1),
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
    /// `TBZ/TBNZ Rt, #bit, label`.
    TestBranch {
        /// `true` = `TBNZ`.
        nonzero: bool,
        /// Tested bit (0..=63; `b5:b40`).
        bit: u8,
        /// Tested register (31 = `ZR`).
        rt: GpReg,
        /// Byte offset from this instruction (±32 KiB).
        offset: i64,
    },
    /// `B.cond label`.
    BCond {
        /// Branch condition.
        cond: Cond,
        /// Byte offset from this instruction (±1 MiB).
        offset: i64,
    },
    /// `SVC #imm16`.
    Svc {
        /// Immediate.
        imm: u16,
    },
    /// `BRK #imm16`.
    Brk {
        /// Comment.
        imm: u16,
    },
    /// `NOP` (`HINT #0`).
    Nop,
    /// `HINT #imm` with `imm != 0`; see [`hint_class`] for the behaviour.
    Hint {
        /// `CRm:op2` (1..=127).
        imm: u8,
    },
    /// `MRS Xt, <sysreg>`.
    Mrs {
        /// Register read.
        reg: SysReg,
        /// Destination (`ZR` when 31).
        rt: GpReg,
    },
    /// `MSR <sysreg>, Xt`.
    Msr {
        /// Register written.
        reg: SysReg,
        /// Source (`ZR` when 31).
        rt: GpReg,
    },
    /// `LDR (literal)`, `LDRSW (literal)`, `PRFM (literal)`.
    LoadLiteral {
        /// Operation.
        op: LiteralOp,
        /// Transfer register (`ZR` when 31), or `prfop` for `PRFM`.
        rt: GpReg,
        /// Byte offset from this instruction (±1 MiB).
        offset: i64,
    },
    /// Single-register integer load/store.
    LoadStore(LoadStore),
    /// Pair integer load/store.
    LoadStorePair(LoadStorePair),
    /// Any word outside the decoded subset, including unallocated encodings.
    Unknown(u32),
}

/// Decodes one A64 instruction word. Never panics.
#[must_use]
pub fn decode(word: u32) -> Inst {
    let op0 = (word >> 25) & 0xf;
    match op0 {
        0b1000 | 0b1001 => dp_imm::decode(word),
        0b1010 | 0b1011 => branch::decode(word),
        _ if op0 & 0b0101 == 0b0100 => ldst::decode(word),
        _ if op0 & 0b0111 == 0b0101 => dp_reg::decode(word),
        _ => Inst::Unknown(word),
    }
}

/// Extracts `word[lsb + len - 1 : lsb]`.
const fn bits(word: u32, lsb: u32, len: u32) -> u32 {
    (word >> lsb) & ((1 << len) - 1)
}

/// `word[i] == 1`.
const fn bit(word: u32, i: u32) -> bool {
    (word >> i) & 1 == 1
}

/// Sign-extends the low `len` bits of `value`.
const fn sext(value: u32, len: u32) -> i64 {
    let shift = 64 - len;
    ((value as u64) << shift) as i64 >> shift
}

/// Sign-extends the low `len` bits of `value` and multiplies by 4.
const fn branch_offset(value: u32, len: u32) -> i64 {
    sext(value, len) * 4
}

/// Register field at `lsb` with 31 = `ZR`.
const fn zr(word: u32, lsb: u32) -> GpReg {
    GpReg::zr_from_field(bits(word, lsb, 5))
}

/// Register field at `lsb` with 31 = `SP`.
const fn sp(word: u32, lsb: u32) -> GpReg {
    GpReg::sp_from_field(bits(word, lsb, 5))
}
