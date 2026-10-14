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
//! * Rust control flow only on **constants**: decode-time constants (fields of [`Inst`])
//!   and the **translation context** [`Ctx`] ([`Sem::ctx`], a view of [`TbFlags`]: EL,
//!   alignment-check enables, PAC key enables, …). Values are opaque (`Self::V` is only
//!   `Copy`), so a data-dependent Rust `if` does not even type-check; data-dependent
//!   choices go through [`Sem::select`], [`Sem::select_flags`], [`Sem::branch`] and
//!   [`Sem::raise_if`]. Every write to a field that enters [`Ctx`] ends the translated
//!   block (dbt.md §10.1), so T1/T2 may specialise on it.
//! * Every value is typed by the [`Ty`] passed to the operation that consumes it; a
//!   value must be used at the type it was produced with (widths change only through
//!   [`Sem::zext`], [`Sem::sext`], [`Sem::trunc`]). The IR verifier
//!   ([`crate::ir::Region::verify`]) checks this exactly, so every description must be
//!   covered by a test that lowers it through [`crate::ir::Builder`] and verifies the
//!   result. Debug builds of [`concrete::Concrete`] only check that each operand is
//!   canonical for the consuming type.
//! * **Precise exceptions.** Operations that can raise are [`Sem::raise`],
//!   [`Sem::raise_if`], [`Sem::load`], [`Sem::store`] and [`Sem::probe`]. All of them
//!   come **before** the first register/flag/system-register write
//!   ([`Sem::set_gpr`], [`Sem::set_nzcv`], [`Sem::set_sysreg`]). A store is both "can
//!   raise" and an architectural write: an instruction with **more than one store**
//!   first calls [`Sem::probe`] for the whole range it will write; after the first
//!   store, only further stores into the probed range may follow (they cannot fault
//!   any more), and no load or other raising operation. Single stores and loads need
//!   no probe. So a raising instruction never leaves a partial update behind.
//! * At most one control-transfer operation ([`Sem::branch`], [`Sem::branch_indirect`],
//!   [`Sem::raise`]) per instruction, and it is the last operation. An instruction
//!   without one falls through to `PC + 4`. [`Sem::raise_if`] is not a control
//!   transfer for this rule (it continues when the condition is false).
//! * `Self::F` values never survive an instruction boundary: they are only stored
//!   through [`Sem::set_nzcv`]. T1 may therefore keep them in host `EFLAGS`.
//!
//! The rules are checked **statically** for every decodable instruction form by
//! [`crate::ir::Builder`] (it records [`crate::ir::RuleViolation`]s; a test lowers
//! every class), and dynamically by debug assertions in [`concrete::Concrete`].
//!
//! [`CpuState`]: crate::cpu::CpuState
//! [`Inst`]: crate::decode::Inst
//! [`TbFlags`]: crate::tier::TbFlags

mod a64;
pub mod concrete;
mod prim;

#[cfg(test)]
mod conformance;

pub use a64::lower;
pub use prim::{
    add_with_carry, add_with_carry32, add_with_carry64, binop, condition_holds, flags_logic, icmp,
    not, sext, sign_extend, sub_with_carry, truncate, unop,
};

use crate::cpu::{CpuState, ExceptionLevel, GpReg, SysReg};
use crate::decode::Cond;
use crate::tier::TbFlags;

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

    /// The types a memory access can have (one to eight bytes).
    pub const MEM: [Self; 4] = [Self::I8, Self::I16, Self::I32, Self::I64];

    /// `I64` if `sf` (the `sf` bit of the encoding), else `I32`.
    #[must_use]
    pub const fn from_sf(sf: bool) -> Self {
        if sf { Self::I64 } else { Self::I32 }
    }

    /// The memory access type of `1 << size` bytes (`size` = the `size` field of a
    /// load/store encoding, `0..=3`; larger values saturate to `I64`).
    #[must_use]
    pub const fn from_size(size: u32) -> Self {
        match size {
            0 => Self::I8,
            1 => Self::I16,
            2 => Self::I32,
            _ => Self::I64,
        }
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

    /// Size of a memory access of this type in bytes (`I1` counts as one byte; no
    /// description accesses memory at `I1`).
    #[must_use]
    pub const fn bytes(self) -> u32 {
        match self {
            Self::I1 | Self::I8 => 1,
            Self::I16 => 2,
            Self::I32 => 4,
            Self::I64 => 8,
        }
    }

    /// Mask of the valid bits.
    #[must_use]
    pub const fn mask(self) -> u64 {
        u64::MAX >> (64 - self.bits())
    }
}

/// Integer binary operations without flags. See [`binop`] for the exact semantics
/// (wrapping arithmetic, shift amounts modulo the width, Arm division rules).
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
    /// Wrapping multiplication (low half of the product).
    Mul,
    /// High half of the unsigned double-width product (`UMULH` at `I64`).
    UMulHi,
    /// High half of the signed double-width product (`SMULH` at `I64`).
    SMulHi,
    /// Unsigned division rounding towards zero; **division by zero gives 0** (Arm
    /// `UDIV`). Translating backends need a guard: x86 `DIV` raises `#DE`.
    UDiv,
    /// Signed division rounding towards zero; division by zero gives 0 and
    /// `MIN / -1` gives `MIN` (Arm `SDIV`). Translating backends need a guard: x86
    /// `IDIV` raises `#DE` in both cases.
    SDiv,
}

impl BinOp {
    /// All operations, for exhaustive tests.
    pub const ALL: [Self; 14] = [
        Self::Add,
        Self::Sub,
        Self::And,
        Self::Or,
        Self::Xor,
        Self::Shl,
        Self::Lshr,
        Self::Ashr,
        Self::Ror,
        Self::Mul,
        Self::UMulHi,
        Self::SMulHi,
        Self::UDiv,
        Self::SDiv,
    ];
}

/// Integer unary operations (besides [`Sem::not`]). See [`unop`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnOp {
    /// Count leading zero bits (`CLZ`; the width for zero).
    Clz,
    /// Reverse the bit order (`RBIT`).
    Rbit,
    /// Reverse the byte order (`REV`; identity for `I1`/`I8`).
    Bswap,
}

impl UnOp {
    /// All operations, for exhaustive tests.
    pub const ALL: [Self; 3] = [Self::Clz, Self::Rbit, Self::Bswap];
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
    /// Plain jump (`B`, `B.cond`, `CBZ`, `TBZ`, `BR`).
    Jump,
    /// Call: the link register was written (`BL`, `BLR`).
    Call,
    /// Return (`RET`).
    Return,
}

/// A synchronous exception raised by an instruction itself (always a constant; data
/// aborts are produced by the memory operations, not by descriptions).
///
/// The *preferred return address* (`ELR_ELx`) is given per variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Exception {
    /// Undefined Instruction (`ESR_ELx.EC = 0x00`). Returns to this instruction.
    Undefined {
        /// The instruction word.
        word: u32,
    },
    /// `SVC #imm` (`EC = 0x15`). Preferred return address: **the next instruction**
    /// (`PC + 4`).
    SupervisorCall {
        /// The 16-bit immediate (`ESR_ELx.ISS`).
        imm: u16,
    },
    /// `BRK #imm` (`EC = 0x3C`). Returns to this instruction.
    Breakpoint {
        /// The 16-bit comment (`ESR_ELx.ISS`).
        imm: u16,
    },
    /// SP alignment fault (`EC = 0x26`, `CheckSPAlignment()`). Returns to this
    /// instruction.
    SpAlignment,
    /// Not a guest exception: the instruction is architecturally valid in this
    /// configuration but its semantics are not implemented yet (CLAUDE.md §3 rule 5:
    /// never silently wrong). Surfaces as [`crate::ExecError::Unimplemented`].
    Unimplemented {
        /// The instruction word.
        word: u32,
    },
}

/// The **translation context**: the second class of constants descriptions may branch
/// on in Rust (R-DBT-01 P1-1). It is a decoded view of [`TbFlags`]; T0 computes it from
/// the [`CpuState`] with [`TbFlags::from_state`] (the same function T1/T2 use for their
/// block keys), so all tiers see identical values.
///
/// Fields are added as instruction classes need them (MMU/TBI/PAN with WP-DBT-06/-07,
/// FPCR modes with WP-DBT-10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ctx {
    /// `PSTATE.EL`.
    pub el: ExceptionLevel,
    /// SP alignment checking for loads/stores with base `SP`: `SCTLR_EL1.SA0` at EL0,
    /// `SCTLR_EL1.SA` at EL1.
    pub sp_align_check: bool,
    /// Alignment checking of normal loads/stores (`SCTLR_EL1.A`).
    pub align_check: bool,
    /// Instruction key A enabled (`SCTLR_EL1.EnIA`).
    pub pac_ia: bool,
    /// Instruction key B enabled (`SCTLR_EL1.EnIB`).
    pub pac_ib: bool,
}

impl Ctx {
    /// The context of `cpu` (via [`TbFlags::from_state`]).
    #[must_use]
    pub fn from_state(cpu: &CpuState) -> Self {
        Self::from_tb_flags(TbFlags::from_state(cpu))
    }

    /// Decodes the context from block-key flags.
    #[must_use]
    pub const fn from_tb_flags(f: TbFlags) -> Self {
        Self {
            el: if f.0 & TbFlags::EL1 != 0 {
                ExceptionLevel::El1
            } else {
                ExceptionLevel::El0
            },
            sp_align_check: f.0 & TbFlags::SP_ALIGN != 0,
            align_check: f.0 & TbFlags::ALIGN != 0,
            pac_ia: f.0 & TbFlags::PAC_IA != 0,
            pac_ib: f.0 & TbFlags::PAC_IB != 0,
        }
    }
}

/// Memory ordering of an access (dbt.md §8.2). Only [`MemOrder::Plain`] is used by
/// WP-DBT-02; the others are reserved for the acquire/release classes (WP-DBT-09), so
/// that the trait does not change when they arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemOrder {
    /// Normal access.
    Plain,
    /// RCsc acquire (`LDAR`).
    Acquire,
    /// RCpc acquire (`LDAPR`).
    AcquirePc,
    /// Release (`STLR`).
    Release,
}

/// Alignment requirement of an access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    /// Normal memory: unaligned accesses are allowed unless alignment checking is on
    /// ([`Ctx::align_check`]); then the access must be aligned to its size.
    Normal,
    /// Must always be aligned to its size (exclusives, atomics; WP-DBT-09).
    Required,
}

/// Instruction-syndrome information for data aborts (`ESR_ELx.ISS` with `ISV = 1`).
///
/// Only set for single-register loads/stores without writeback, the forms for which the
/// architecture can report a valid syndrome. `SAS` and `WnR` follow from the access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DataIss {
    /// `SRT`: the transfer register number (31 = `ZR`).
    pub srt: u8,
    /// `SSE`: the load sign-extends.
    pub sign_extend: bool,
    /// `SF`: the transfer register is 64 bits wide.
    pub sixty_four: bool,
}

/// Constant attributes of a memory operation (R-DBT-01 P1-2). Everything here is known
/// at decode time; the backend derives privilege, alignment check, fault syndrome and
/// TLB index from it together with [`Ctx`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemAttrs {
    /// Checked with EL0 permissions (`LDTR/STTR`). With `PSTATE.UAO = 0` (always, UAO
    /// is not writable yet) this applies at EL1 as well; at EL0 it changes nothing.
    pub unprivileged: bool,
    /// Ordering.
    pub order: MemOrder,
    /// Alignment requirement.
    pub align: Align,
    /// Syndrome for data aborts, if the instruction form has a valid one.
    pub iss: Option<DataIss>,
}

impl MemAttrs {
    /// A normal access without a syndrome.
    pub const PLAIN: Self = Self {
        unprivileged: false,
        order: MemOrder::Plain,
        align: Align::Normal,
        iss: None,
    };
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

    /// The translation context (a constant for the whole translated block).
    fn ctx(&self) -> Ctx;

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

    /// Reads a system register as an `I64` (`NZCV` in its register layout).
    fn sysreg(&mut self, r: SysReg) -> Self::V;

    /// Writes a system register from an `I64`. The value is stored as given: masking
    /// RES0 bits is the description's job ([`SysReg::write_mask`]). Writing
    /// [`SysReg::Nzcv`] keeps only bits 31:28.
    fn set_sysreg(&mut self, r: SysReg, v: Self::V);

    /// `AddWithCarry(a, b, carry_in)` at width `ty`; `carry_in` is an `I1`. Returns the
    /// result and its flags.
    fn add_with_carry(
        &mut self,
        ty: Ty,
        a: Self::V,
        b: Self::V,
        carry_in: Self::V,
    ) -> (Self::V, Self::F);

    /// `AddWithCarry(a, NOT b, carry_in)` at width `ty` (subtraction: `SUBS/CMP` with
    /// `carry_in = 1`, `SBC/SBCS` with `carry_in = C`). A separate operation so that T1
    /// emits `sub/cmp/sbb` directly (R-DBT-01 P2-1).
    fn sub_with_carry(
        &mut self,
        ty: Ty,
        a: Self::V,
        b: Self::V,
        carry_in: Self::V,
    ) -> (Self::V, Self::F);

    /// Flags of a logical operation (`ANDS/BICS/TST`): `N` = top bit of `result` at
    /// width `ty`, `Z` = `result == 0`, `C = V = 0`.
    fn flags_logic(&mut self, ty: Ty, result: Self::V) -> Self::F;

    /// The carry flag of `f` as an `I1` (`ADC/SBC`).
    fn flag_c(&mut self, f: Self::F) -> Self::V;

    /// `cond ? a : b` on flags; `cond` is an `I1` (`CCMP/CCMN`).
    fn select_flags(&mut self, cond: Self::V, a: Self::F, b: Self::F) -> Self::F;

    /// The flags in `NZCV` register layout (bits 31:28) as an `I64` (`MRS NZCV`).
    fn flags_to_bits(&mut self, f: Self::F) -> Self::V;

    /// Flags from an `I64` in `NZCV` register layout; bits other than 31:28 are
    /// ignored (`MSR NZCV`, `CCMP` immediate flags).
    fn bits_to_flags(&mut self, v: Self::V) -> Self::F;

    /// A flag-less binary operation at width `ty` (see [`binop`]).
    fn binop(&mut self, op: BinOp, ty: Ty, a: Self::V, b: Self::V) -> Self::V;

    /// A unary operation at width `ty` (see [`unop`]).
    fn unop(&mut self, op: UnOp, ty: Ty, a: Self::V) -> Self::V;

    /// Bitwise `NOT` at width `ty`.
    fn not(&mut self, ty: Ty, a: Self::V) -> Self::V;

    /// Zero-extends `v` from `from` to `to` (`from.bits() <= to.bits()`).
    fn zext(&mut self, from: Ty, to: Ty, v: Self::V) -> Self::V;

    /// Sign-extends `v` from `from` to `to` (`from.bits() <= to.bits()`).
    fn sext(&mut self, from: Ty, to: Ty, v: Self::V) -> Self::V;

    /// Truncates `v` from `from` to `to` (`from.bits() >= to.bits()`).
    fn trunc(&mut self, from: Ty, to: Ty, v: Self::V) -> Self::V;

    /// Integer comparison at width `ty`; returns an `I1`.
    fn icmp(&mut self, cc: CmpOp, ty: Ty, a: Self::V, b: Self::V) -> Self::V;

    /// `cond ? a : b` where `cond` is an `I1` and `a`, `b` are of type `ty`.
    fn select(&mut self, ty: Ty, cond: Self::V, a: Self::V, b: Self::V) -> Self::V;

    /// `ConditionHolds(cond)` evaluated on `f`; returns an `I1`.
    fn cond_holds(&mut self, cond: Cond, f: Self::F) -> Self::V;

    /// Loads `ty` (one of [`Ty::MEM`]) from the virtual address `addr` (an `I64`),
    /// little-endian, zero-extended into the canonical value.
    ///
    /// The backend translates (privilege from [`Ctx`] and `attrs`), checks alignment
    /// and backing, and raises the data abort itself.
    ///
    /// # Errors
    ///
    /// T0: the data abort as [`crate::ExecError`]. Translating backends never fail.
    fn load(&mut self, ty: Ty, addr: Self::V, attrs: MemAttrs) -> Result<Self::V, Self::Abort>;

    /// Stores `value` (of type `ty`, one of [`Ty::MEM`]) to `addr`, little-endian.
    ///
    /// # Errors
    ///
    /// As [`Sem::load`]; a failing store writes nothing.
    fn store(
        &mut self,
        ty: Ty,
        addr: Self::V,
        value: Self::V,
        attrs: MemAttrs,
    ) -> Result<(), Self::Abort>;

    /// Checks that `count` consecutive stores of `ty` starting at `addr` would succeed
    /// (translation for writing, alignment of each element, backing) **without**
    /// writing. Required before the first of several stores of one instruction.
    ///
    /// # Errors
    ///
    /// As [`Sem::store`] would fail for the first failing element.
    fn probe(
        &mut self,
        ty: Ty,
        count: u8,
        addr: Self::V,
        attrs: MemAttrs,
    ) -> Result<(), Self::Abort>;

    /// PC-relative branch to `PC + offset` (wrapping). With `Some(c)` (an `I1`) the
    /// branch is conditional and falls through to `PC + 4` when `c == 0`.
    fn branch(&mut self, cond: Option<Self::V>, offset: i64, kind: BranchKind);

    /// Branch to the address `target` (an `I64`).
    ///
    /// Not yet modelled: `AArch64.BranchAddr()` top-byte handling with TBI enabled. It
    /// belongs into a description helper driven by [`Ctx`] (R-DBT-01 P2-3) and lands
    /// with TBI in WP-DBT-06. With TBI off this is exact; a non-canonical target faults
    /// on the next fetch, as architected.
    fn branch_indirect(&mut self, target: Self::V, kind: BranchKind);

    /// Raises a synchronous exception at the current instruction.
    ///
    /// # Errors
    ///
    /// T0 always returns `Err` (the exception); translating backends emit an exit and
    /// return `Ok`. Either way it must be the last operation of a description.
    fn raise(&mut self, e: Exception) -> Result<(), Self::Abort>;

    /// Raises `e` if `cond` (an `I1`) is 1, otherwise continues (R-DBT-01 P1-3). Counts
    /// as "can raise" (before any write), but not as a control transfer.
    ///
    /// # Errors
    ///
    /// T0: `Err` if the condition holds. Translating backends never fail.
    fn raise_if(&mut self, cond: Self::V, e: Exception) -> Result<(), Self::Abort>;
}
