//! The semantic layer: the single source of instruction semantics (ADR-0006).
//!
//! Every instruction class is described **once**, as a generic Rust function over the
//! [`Sem`] trait ([`lower`] dispatches a decoded [`Inst`] to its description). Backends
//! instantiate the same description (`docs/subsystems/dbt.md` §3.2, E-3):
//!
//! | Backend | `Sem::V` | Result |
//! |---|---|---|
//! | T0 [`concrete::Concrete`] | `u64` | executes immediately on a [`CpuState`] |
//! | T1 `Template` (WP-DBT-12) | host register / temp slot | emits x86-64 directly |
//! | T2 [`crate::ir::Builder`] | [`crate::ir::Value`] (SSA) | builds IR for optimisation |
//!
//! # Rules for descriptions
//!
//! * Rust control flow only on **decode-time constants** (fields of [`Inst`]). Values
//!   are opaque (`Self::V` is only `Copy`), so a data-dependent Rust `if` does not even
//!   type-check; data-dependent choices go through [`Sem::select`] and [`Sem::branch`].
//! * Every value is typed by the [`Ty`] passed to the operation that consumes it; a
//!   value must be used at the type it was produced with (the IR verifier and debug
//!   builds of [`concrete::Concrete`] check this).
//! * All operations that can raise ([`Sem::raise`], later loads/stores) come **before**
//!   the first architectural write, so a raising instruction leaves the state unchanged
//!   (precise exceptions). [`Sem::raise`] is the last operation of a description.
//! * At most one control-transfer operation ([`Sem::branch`], [`Sem::branch_indirect`])
//!   per instruction, and it is the last operation. An instruction without one falls
//!   through to `PC + 4`.
//!
//! [`CpuState`]: crate::cpu::CpuState
//! [`Inst`]: crate::decode::Inst

mod a64;
pub mod concrete;
mod prim;

#[cfg(test)]
mod conformance;

pub use a64::lower;
pub use prim::{
    add_with_carry, add_with_carry32, add_with_carry64, binop, condition_holds, icmp, not,
    sign_extend, truncate,
};

use crate::cpu::GpReg;
use crate::decode::Cond;

/// Type of a scalar integer value in the semantic layer.
///
/// Values are kept in **canonical form**: bits above the width are zero. 128-bit and
/// vector values get their own value type in WP-DBT-10, so that integer code in T0 does
/// not pay for 128-bit arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    /// Boolean (`0` or `1`): conditions, carry-in.
    I1,
    /// Byte.
    I8,
    /// Halfword.
    I16,
    /// Word (`Wn`).
    I32,
    /// Doubleword (`Xn`), addresses.
    I64,
}

impl Ty {
    /// All types, for exhaustive tests.
    pub const ALL: [Self; 5] = [Self::I1, Self::I8, Self::I16, Self::I32, Self::I64];

    /// `I64` if `sf` (the `sf` bit of the encoding), else `I32`.
    #[must_use]
    pub const fn from_sf(sf: bool) -> Self {
        if sf { Self::I64 } else { Self::I32 }
    }

    /// Width in bits (1..=64).
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            Self::I1 => 1,
            Self::I8 => 8,
            Self::I16 => 16,
            Self::I32 => 32,
            Self::I64 => 64,
        }
    }

    /// Mask of the valid bits.
    #[must_use]
    pub const fn mask(self) -> u64 {
        u64::MAX >> (64 - self.bits())
    }
}

/// Integer binary operations without flags. See [`binop`] for the exact semantics
/// (wrapping arithmetic, shift amounts modulo the width).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinOp {
    /// Wrapping addition.
    Add,
    /// Wrapping subtraction.
    Sub,
    /// Bitwise AND.
    And,
    /// Bitwise OR.
    Or,
    /// Bitwise exclusive OR.
    Xor,
    /// Logical shift left (`LSL`).
    Shl,
    /// Logical shift right (`LSR`).
    Lshr,
    /// Arithmetic shift right (`ASR`).
    Ashr,
    /// Rotate right (`ROR`).
    Ror,
}

impl BinOp {
    /// All operations, for exhaustive tests.
    pub const ALL: [Self; 9] = [
        Self::Add,
        Self::Sub,
        Self::And,
        Self::Or,
        Self::Xor,
        Self::Shl,
        Self::Lshr,
        Self::Ashr,
        Self::Ror,
    ];
}

/// Integer comparisons (same set as Cranelift's `IntCC`, so T2 lowers 1:1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CmpOp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Unsigned less than.
    Ult,
    /// Unsigned less than or equal.
    Ule,
    /// Unsigned greater than.
    Ugt,
    /// Unsigned greater than or equal.
    Uge,
    /// Signed less than.
    Slt,
    /// Signed less than or equal.
    Sle,
    /// Signed greater than.
    Sgt,
    /// Signed greater than or equal.
    Sge,
}

impl CmpOp {
    /// All comparisons, for exhaustive tests.
    pub const ALL: [Self; 10] = [
        Self::Eq,
        Self::Ne,
        Self::Ult,
        Self::Ule,
        Self::Ugt,
        Self::Uge,
        Self::Slt,
        Self::Sle,
        Self::Sgt,
        Self::Sge,
    ];
}

/// What a control transfer is, as a hint for branch prediction structures (T1 return
/// stack, `docs/subsystems/dbt.md` §5.2). It never changes the architectural effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BranchKind {
    /// Plain jump (`B`, `B.cond`, `CBZ`, `BR`).
    Jump,
    /// Call: the link register was written (`BL`, `BLR`).
    Call,
    /// Return (`RET`).
    Return,
}

/// A synchronous exception raised by an instruction itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Exception {
    /// Undefined Instruction (`ESR_ELx.EC = 0x00`).
    Undefined {
        /// The instruction word.
        word: u32,
    },
}

/// The abstract machine every instruction description is written against.
///
/// See the [module documentation](self) for the rules descriptions must follow and
/// [`concrete::Concrete`] for the reference meaning of each operation. Each operation
/// is covered by conformance tests against independent transcriptions of the Arm ARM
/// pseudocode (`sem/conformance.rs`).
pub trait Sem {
    /// A scalar value of some [`Ty`].
    type V: Copy;
    /// An abstract `NZCV` value; backends may keep it lazy (dbt.md §6.3).
    type F: Copy;
    /// How a raised exception surfaces: T0 returns it as an error; translating backends
    /// emit an exit and use [`core::convert::Infallible`].
    type Abort;

    /// The constant `value` truncated to `ty`.
    fn imm(&mut self, ty: Ty, value: u64) -> Self::V;

    /// `I64`: the address of the current instruction plus `offset` (wrapping).
    ///
    /// The only way a description may observe `PC`; keeping it relative lets T1/T2
    /// emit position-independent translations (dbt.md §14).
    fn pc_rel(&mut self, offset: i64) -> Self::V;

    /// Reads a general-purpose register (`XZR` reads 0, `SP` per `PSTATE.{EL,SP}`),
    /// truncated to `ty`.
    fn gpr(&mut self, r: GpReg, ty: Ty) -> Self::V;

    /// Writes a general-purpose register; the value is zero-extended to 64 bits (every
    /// `Wd` write clears the upper half, including `WSP`). Writes to `XZR` are dropped.
    fn set_gpr(&mut self, r: GpReg, ty: Ty, v: Self::V);

    /// Reads `PSTATE.{N,Z,C,V}`.
    fn nzcv(&mut self) -> Self::F;

    /// Writes `PSTATE.{N,Z,C,V}`.
    fn set_nzcv(&mut self, f: Self::F);

    /// `AddWithCarry(a, b, carry_in)` at width `ty`; `carry_in` is an `I1`. Returns the
    /// result and its flags. Subtraction `a - b` is `add_with_carry(a, NOT b, 1)`.
    fn add_with_carry(
        &mut self,
        ty: Ty,
        a: Self::V,
        b: Self::V,
        carry_in: Self::V,
    ) -> (Self::V, Self::F);

    /// A flag-less binary operation at width `ty` (see [`binop`]).
    fn binop(&mut self, op: BinOp, ty: Ty, a: Self::V, b: Self::V) -> Self::V;

    /// Bitwise `NOT` at width `ty`.
    fn not(&mut self, ty: Ty, a: Self::V) -> Self::V;

    /// Integer comparison at width `ty`; returns an `I1`.
    fn icmp(&mut self, cc: CmpOp, ty: Ty, a: Self::V, b: Self::V) -> Self::V;

    /// `cond ? a : b` where `cond` is an `I1` and `a`, `b` are of type `ty`.
    fn select(&mut self, ty: Ty, cond: Self::V, a: Self::V, b: Self::V) -> Self::V;

    /// `ConditionHolds(cond)` evaluated on `f`; returns an `I1`.
    fn cond_holds(&mut self, cond: Cond, f: Self::F) -> Self::V;

    /// PC-relative branch to `PC + offset` (wrapping). With `Some(c)` (an `I1`) the
    /// branch is conditional and falls through to `PC + 4` when `c == 0`.
    fn branch(&mut self, cond: Option<Self::V>, offset: i64, kind: BranchKind);

    /// Branch to the address `target` (an `I64`).
    ///
    /// Not yet modelled: `AArch64.BranchAddr()` top-byte handling with TBI enabled
    /// (WP-DBT-06). With TBI off this is exact; a non-canonical target faults on the
    /// next fetch, as architected. The backends implement it, so the fix lands once.
    fn branch_indirect(&mut self, target: Self::V, kind: BranchKind);

    /// Raises a synchronous exception at the current instruction.
    ///
    /// # Errors
    ///
    /// T0 always returns `Err` (the exception); translating backends emit an exit and
    /// return `Ok`. Either way it must be the last operation of a description.
    fn raise(&mut self, e: Exception) -> Result<(), Self::Abort>;
}
