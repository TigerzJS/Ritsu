//! T2 intermediate representation (type definitions, builder, verifier; no codegen).
//!
//! Design: `docs/subsystems/dbt.md` §4. A [`Region`] is a control-flow graph of
//! [`Block`]s in SSA form with **block parameters** instead of phi nodes (lowers 1:1 to
//! Cranelift in WP-DBT-17). Guest state is accessed through explicit operations
//! ([`Op::GetGpr`], [`Op::SetNzcv`], …), which the *state promotion* pass later turns
//! into SSA values.
//!
//! The IR is produced exclusively by [`Builder`], the semantic layer's IR backend: the
//! operations here mirror the [`Sem`](crate::sem::Sem) operations one-to-one, so the IR
//! cannot express semantics that the descriptions do not.
//!
//! **Addresses are relative**: [`Op::PcRel`] offsets, [`Exit::Direct`] targets and
//! per-instruction `pc_offset`s are relative to the guest VA of the region entry, so a
//! region is position independent (persistent cache, dbt.md §14).

mod builder;
#[cfg(test)]
mod tests;

pub use builder::{Builder, RuleViolation, RuleViolationKind};

use crate::cpu::{GpReg, SysReg};
use crate::decode::Cond;
use crate::sem::{BinOp, BranchKind, CmpOp, Exception, MemAttrs, Ty, UnOp};
use core::fmt;

/// Type of an IR value (dbt.md §4.1).
///
/// `I128`, the floating-point types and `V128` are reserved for WP-DBT-10/-15; no
/// operation produces them yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// Boolean.
    I1,
    /// 8-bit integer.
    I8,
    /// 16-bit integer.
    I16,
    /// 32-bit integer.
    I32,
    /// 64-bit integer.
    I64,
    /// 128-bit integer (reserved).
    I128,
    /// Half precision (reserved).
    F16,
    /// Single precision (reserved).
    F32,
    /// Double precision (reserved).
    F64,
    /// 128-bit vector (reserved).
    V128,
    /// Abstract `NZCV` value, materialised only when needed (dbt.md §6.3).
    Flags,
}

impl From<Ty> for Type {
    fn from(ty: Ty) -> Self {
        match ty {
            Ty::I1 => Self::I1,
            Ty::I8 => Self::I8,
            Ty::I16 => Self::I16,
            Ty::I32 => Self::I32,
            Ty::I64 => Self::I64,
        }
    }
}

/// An SSA value (index into the region's value table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Value(pub u32);

impl Value {
    fn index(self) -> usize {
        // u32 → usize is lossless on every supported host (64-bit).
        self.0 as usize
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// A basic block of a [`Region`] (index into its block table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Block(pub u32);

impl Block {
    fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "block{}", self.0)
    }
}

/// A non-terminating IR operation. Each corresponds to one [`Sem`](crate::sem::Sem)
/// operation with the same name and meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// `Sem::imm`; `imm` is already truncated to `ty`. Result: `ty`.
    Iconst {
        /// Type.
        ty: Ty,
        /// Canonical value.
        imm: u64,
    },
    /// `Sem::pc_rel`: region entry VA + `offset`. Result: `I64`.
    PcRel {
        /// Offset from the region entry.
        offset: i64,
    },
    /// `Sem::gpr`. Result: `ty`.
    GetGpr {
        /// Register (31 already resolved).
        reg: GpReg,
        /// Read width.
        ty: Ty,
    },
    /// `Sem::set_gpr`. No result.
    SetGpr {
        /// Register (31 already resolved).
        reg: GpReg,
        /// Write width (zero-extended to 64 bits).
        ty: Ty,
        /// Value of type `ty`.
        value: Value,
    },
    /// `Sem::nzcv`. Result: `Flags`.
    GetNzcv,
    /// `Sem::set_nzcv`. No result.
    SetNzcv {
        /// Value of type `Flags`.
        flags: Value,
    },
    /// `Sem::sysreg`. Result: `I64`.
    GetSysreg {
        /// Register.
        reg: SysReg,
    },
    /// `Sem::set_sysreg`. No result.
    SetSysreg {
        /// Register.
        reg: SysReg,
        /// Value (`I64`).
        value: Value,
    },
    /// `Sem::add_with_carry`. Results: `ty`, `Flags`.
    AddWithCarry {
        /// Width.
        ty: Ty,
        /// First operand (`ty`).
        a: Value,
        /// Second operand (`ty`).
        b: Value,
        /// Carry in (`I1`).
        carry_in: Value,
    },
    /// `Sem::sub_with_carry`. Results: `ty`, `Flags`.
    SubWithCarry {
        /// Width.
        ty: Ty,
        /// Minuend (`ty`).
        a: Value,
        /// Subtrahend (`ty`).
        b: Value,
        /// Carry in (`I1`, 1 = no borrow).
        carry_in: Value,
    },
    /// `Sem::flags_logic`. Result: `Flags`.
    FlagsLogic {
        /// Width.
        ty: Ty,
        /// The logical result (`ty`).
        a: Value,
    },
    /// `Sem::flag_c`. Result: `I1`.
    FlagC {
        /// Flags.
        flags: Value,
    },
    /// `Sem::select_flags`. Result: `Flags`.
    SelectFlags {
        /// Condition (`I1`).
        cond: Value,
        /// Flags if `cond == 1`.
        a: Value,
        /// Flags if `cond == 0`.
        b: Value,
    },
    /// `Sem::flags_to_bits`. Result: `I64`.
    FlagsToBits {
        /// Flags.
        flags: Value,
    },
    /// `Sem::bits_to_flags`. Result: `Flags`.
    BitsToFlags {
        /// `I64` in `NZCV` layout.
        a: Value,
    },
    /// `Sem::binop`. Result: `ty`.
    Binary {
        /// Operation.
        op: BinOp,
        /// Width.
        ty: Ty,
        /// First operand (`ty`).
        a: Value,
        /// Second operand (`ty`).
        b: Value,
    },
    /// `Sem::unop`. Result: `ty`.
    Unary {
        /// Operation.
        op: UnOp,
        /// Width.
        ty: Ty,
        /// Operand (`ty`).
        a: Value,
    },
    /// `Sem::not`. Result: `ty`.
    Not {
        /// Width.
        ty: Ty,
        /// Operand (`ty`).
        a: Value,
    },
    /// `Sem::zext` (`signed == false`) or `Sem::sext`. Result: `to`.
    Ext {
        /// Sign- or zero-extension.
        signed: bool,
        /// Operand type (not wider than `to`).
        from: Ty,
        /// Result type.
        to: Ty,
        /// Operand (`from`).
        a: Value,
    },
    /// `Sem::trunc`. Result: `to`.
    Trunc {
        /// Operand type (not narrower than `to`).
        from: Ty,
        /// Result type.
        to: Ty,
        /// Operand (`from`).
        a: Value,
    },
    /// `Sem::icmp`. Result: `I1`.
    Icmp {
        /// Comparison.
        cc: CmpOp,
        /// Operand width.
        ty: Ty,
        /// First operand (`ty`).
        a: Value,
        /// Second operand (`ty`).
        b: Value,
    },
    /// `Sem::select`. Result: `ty`.
    Select {
        /// Width of `a`, `b` and the result.
        ty: Ty,
        /// Condition (`I1`).
        cond: Value,
        /// Value if `cond == 1`.
        a: Value,
        /// Value if `cond == 0`.
        b: Value,
    },
    /// `Sem::cond_holds`. Result: `I1`.
    CondHolds {
        /// Condition code.
        cond: Cond,
        /// Flags (`Flags`).
        flags: Value,
    },
    /// `Sem::load`. Result: `ty`. May raise a data abort.
    Load {
        /// Access type.
        ty: Ty,
        /// Constant attributes.
        attrs: MemAttrs,
        /// Virtual address (`I64`).
        addr: Value,
    },
    /// `Sem::store`. No result. May raise a data abort.
    Store {
        /// Access type.
        ty: Ty,
        /// Constant attributes.
        attrs: MemAttrs,
        /// Virtual address (`I64`).
        addr: Value,
        /// Value (`ty`).
        value: Value,
    },
    /// `Sem::probe`. No result. May raise a data abort.
    Probe {
        /// Element type.
        ty: Ty,
        /// Number of consecutive elements.
        count: u8,
        /// Constant attributes.
        attrs: MemAttrs,
        /// Virtual address (`I64`).
        addr: Value,
    },
}

/// Side-effect classes of an [`Op`] (dbt.md §4.2, R-DBT-01 P2-5): what state promotion,
/// dead-code elimination and load/store elimination must respect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Effects {
    /// Reads guest registers/flags/system registers.
    pub reads_state: bool,
    /// Writes guest registers/flags/system registers.
    pub writes_state: bool,
    /// Reads guest memory.
    pub reads_mem: bool,
    /// Writes guest memory.
    pub writes_mem: bool,
    /// May leave the region with an exception (data abort).
    pub may_raise: bool,
}

impl Op {
    /// The side effects of this operation; all-false means pure.
    #[must_use]
    pub fn effects(&self) -> Effects {
        let none = Effects::default();
        match self {
            Self::GetGpr { .. } | Self::GetNzcv | Self::GetSysreg { .. } => Effects {
                reads_state: true,
                ..none
            },
            Self::SetGpr { .. } | Self::SetNzcv { .. } | Self::SetSysreg { .. } => Effects {
                writes_state: true,
                ..none
            },
            Self::Load { .. } => Effects {
                reads_mem: true,
                may_raise: true,
                ..none
            },
            Self::Store { .. } => Effects {
                writes_mem: true,
                may_raise: true,
                ..none
            },
            // A probe neither reads nor writes guest-visible memory, but it must stay
            // before the stores it covers.
            Self::Probe { .. } => Effects {
                may_raise: true,
                ..none
            },
            _ => none,
        }
    }

    /// Types of the results (at most two).
    #[must_use]
    pub fn result_types(&self) -> [Option<Type>; 2] {
        match *self {
            Self::Iconst { ty, .. }
            | Self::GetGpr { ty, .. }
            | Self::Binary { ty, .. }
            | Self::Unary { ty, .. }
            | Self::Not { ty, .. }
            | Self::Select { ty, .. }
            | Self::Load { ty, .. } => [Some(ty.into()), None],
            Self::Ext { to, .. } | Self::Trunc { to, .. } => [Some(to.into()), None],
            Self::PcRel { .. } | Self::GetSysreg { .. } | Self::FlagsToBits { .. } => {
                [Some(Type::I64), None]
            }
            Self::GetNzcv
            | Self::FlagsLogic { .. }
            | Self::SelectFlags { .. }
            | Self::BitsToFlags { .. } => [Some(Type::Flags), None],
            Self::AddWithCarry { ty, .. } | Self::SubWithCarry { ty, .. } => {
                [Some(ty.into()), Some(Type::Flags)]
            }
            Self::Icmp { .. } | Self::CondHolds { .. } | Self::FlagC { .. } => {
                [Some(Type::I1), None]
            }
            Self::SetGpr { .. }
            | Self::SetNzcv { .. }
            | Self::SetSysreg { .. }
            | Self::Store { .. }
            | Self::Probe { .. } => [None, None],
        }
    }

    /// Operands with the type each must have (at most three).
    #[must_use]
    pub fn operands(&self) -> [Option<(Value, Type)>; 3] {
        let one = |v: Value, t: Type| [Some((v, t)), None, None];
        match *self {
            Self::Iconst { .. }
            | Self::PcRel { .. }
            | Self::GetGpr { .. }
            | Self::GetNzcv
            | Self::GetSysreg { .. } => [None, None, None],
            Self::SetGpr { ty, value, .. } => one(value, ty.into()),
            Self::SetSysreg { value, .. } => one(value, Type::I64),
            Self::SetNzcv { flags }
            | Self::CondHolds { flags, .. }
            | Self::FlagC { flags }
            | Self::FlagsToBits { flags } => one(flags, Type::Flags),
            Self::BitsToFlags { a } => one(a, Type::I64),
            Self::FlagsLogic { ty, a } | Self::Not { ty, a } | Self::Unary { ty, a, .. } => {
                one(a, ty.into())
            }
            Self::Ext { from, a, .. } | Self::Trunc { from, a, .. } => one(a, from.into()),
            Self::AddWithCarry { ty, a, b, carry_in }
            | Self::SubWithCarry { ty, a, b, carry_in } => [
                Some((a, ty.into())),
                Some((b, ty.into())),
                Some((carry_in, Type::I1)),
            ],
            Self::Binary { ty, a, b, .. } | Self::Icmp { ty, a, b, .. } => {
                [Some((a, ty.into())), Some((b, ty.into())), None]
            }
            Self::Select { ty, cond, a, b } => [
                Some((cond, Type::I1)),
                Some((a, ty.into())),
                Some((b, ty.into())),
            ],
            Self::SelectFlags { cond, a, b } => [
                Some((cond, Type::I1)),
                Some((a, Type::Flags)),
                Some((b, Type::Flags)),
            ],
            Self::Load { addr, .. } | Self::Probe { addr, .. } => one(addr, Type::I64),
            Self::Store {
                ty, addr, value, ..
            } => [Some((addr, Type::I64)), Some((value, ty.into())), None],
        }
    }

    /// Whether a width conversion goes in its declared direction.
    fn conversion_ok(&self) -> bool {
        match *self {
            Self::Ext { from, to, .. } => from.bits() <= to.bits(),
            Self::Trunc { from, to, .. } => from.bits() >= to.bits(),
            _ => true,
        }
    }
}

/// One operation with its results and origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstData {
    /// The operation.
    pub op: Op,
    /// Result values, in the order of [`Op::result_types`].
    pub results: [Option<Value>; 2],
    /// Offset of the originating guest instruction from the region entry (precise
    /// exceptions, profiling, jitdump).
    pub pc_offset: i64,
}

/// How control leaves a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// To the guest address region entry + `offset` (chainable in T1/T2).
    Direct {
        /// Target offset from the region entry.
        offset: i64,
        /// Predictor hint.
        kind: BranchKind,
    },
    /// To a computed guest address (jump cache / return stack lookup).
    Indirect {
        /// Target address (`I64`).
        target: Value,
        /// Predictor hint.
        kind: BranchKind,
    },
    /// Raise a synchronous exception at the instruction at `pc_offset`.
    Raise {
        /// The exception.
        exception: Exception,
        /// Offset of the raising instruction from the region entry.
        pc_offset: i64,
    },
}

/// The last operation of a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Terminator {
    /// Unconditional jump within the region.
    Jump {
        /// Target block.
        dest: Block,
        /// Arguments for the target's parameters.
        args: Vec<Value>,
    },
    /// Two-way branch within the region on an `I1`.
    Branch {
        /// Condition (`I1`).
        cond: Value,
        /// Target if `cond == 1`.
        then_dest: Block,
        /// Arguments for `then_dest`.
        then_args: Vec<Value>,
        /// Target if `cond == 0`.
        else_dest: Block,
        /// Arguments for `else_dest`.
        else_args: Vec<Value>,
    },
    /// Leave the region.
    Exit(Exit),
}

impl Terminator {
    /// Successor blocks with their argument lists.
    fn successors(&self) -> impl Iterator<Item = (Block, &[Value])> {
        let (first, second) = match self {
            Self::Jump { dest, args } => (Some((*dest, args.as_slice())), None),
            Self::Branch {
                then_dest,
                then_args,
                else_dest,
                else_args,
                ..
            } => (
                Some((*then_dest, then_args.as_slice())),
                Some((*else_dest, else_args.as_slice())),
            ),
            Self::Exit(_) => (None, None),
        };
        first.into_iter().chain(second)
    }

    /// Values used directly by the terminator (not as block arguments), with type.
    fn operands(&self) -> Option<(Value, Type)> {
        match self {
            Self::Branch { cond, .. } => Some((*cond, Type::I1)),
            Self::Exit(Exit::Indirect { target, .. }) => Some((*target, Type::I64)),
            Self::Jump { .. } | Self::Exit(_) => None,
        }
    }
}

/// Contents of one block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockData {
    /// Block parameters (SSA values defined on entry).
    pub params: Vec<Value>,
    /// Operations in order.
    pub insts: Vec<InstData>,
    /// Terminator; `None` only while the block is under construction.
    pub term: Option<Terminator>,
}

/// A translation unit of T2: entry block [`Region::ENTRY`] plus a value table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Region {
    blocks: Vec<BlockData>,
    value_types: Vec<Type>,
}

/// Why [`Region::verify`] rejected a region.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum VerifyError {
    /// The region has no blocks.
    #[error("region has no entry block")]
    Empty,
    /// A block has no terminator.
    #[error("{0} has no terminator")]
    Unterminated(Block),
    /// A terminator names a block that does not exist.
    #[error("{from} branches to nonexistent {to}")]
    NoSuchBlock {
        /// Branching block.
        from: Block,
        /// Missing target.
        to: Block,
    },
    /// The entry block has parameters (nothing can supply them).
    #[error("entry block has parameters")]
    EntryParams,
    /// Wrong number of block arguments.
    #[error("{from} passes {given} arguments to {to}, which has {expected} parameters")]
    ArgCount {
        /// Branching block.
        from: Block,
        /// Target block.
        to: Block,
        /// Number of parameters of `to`.
        expected: usize,
        /// Number of arguments passed.
        given: usize,
    },
    /// A value is used at the wrong type.
    #[error("{value} used as {expected:?} but has type {found:?}")]
    Type {
        /// The value.
        value: Value,
        /// Type required by the use.
        expected: Type,
        /// Type of the value.
        found: Type,
    },
    /// A value is used where it is not defined on every path from the entry.
    #[error("{value} used in {block} without a dominating definition")]
    Undefined {
        /// The value.
        value: Value,
        /// Using block.
        block: Block,
    },
    /// An `Iconst` immediate has bits set above its type's width.
    #[error("{block}: constant {imm:#x} is not a canonical {ty:?}")]
    NonCanonicalConst {
        /// Block containing the constant.
        block: Block,
        /// Declared type.
        ty: Ty,
        /// The immediate.
        imm: u64,
    },
    /// A value is defined more than once, or a result slot does not match the op.
    #[error("{0} is defined more than once or by a mismatching result slot")]
    BadDefinition(Value),
    /// An `Ext` widens to a narrower type or a `Trunc` narrows to a wider one.
    #[error("{0}: width conversion in the wrong direction")]
    BadConversion(Block),
}

impl Region {
    /// The entry block.
    pub const ENTRY: Block = Block(0);

    /// All blocks, indexed by [`Block`].
    #[must_use]
    pub fn blocks(&self) -> &[BlockData] {
        &self.blocks
    }

    /// One block.
    #[must_use]
    pub fn block(&self, b: Block) -> Option<&BlockData> {
        self.blocks.get(b.index())
    }

    /// Number of values in the value table.
    #[must_use]
    pub fn num_values(&self) -> usize {
        self.value_types.len()
    }

    /// Type of a value.
    #[must_use]
    pub fn value_type(&self, v: Value) -> Option<Type> {
        self.value_types.get(v.index()).copied()
    }

    /// Checks structural and type invariants: every block terminated, branch targets
    /// exist with matching argument counts, constants are canonical, every use has the
    /// type its operation requires, every value is defined at most once and every use is
    /// dominated by its definition (SSA). Values in the value table that are never
    /// defined are allowed as long as nothing uses them.
    ///
    /// Cost: O(blocks + values + edges) memory; time is dominated by the iterative
    /// dominator computation (Cooper, Harvey, Kennedy: "A Simple, Fast Dominance
    /// Algorithm"), which needs few passes on the reducible graphs the builder emits.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn verify(&self) -> Result<(), VerifyError> {
        if self.blocks.is_empty() {
            return Err(VerifyError::Empty);
        }
        if self
            .block(Self::ENTRY)
            .is_some_and(|entry| !entry.params.is_empty())
        {
            return Err(VerifyError::EntryParams);
        }
        let sites = self.verify_definitions()?;
        let succs = self.verify_edges()?;
        let dom = Dominance::compute(&succs);
        self.verify_uses(&sites, &dom)
    }

    /// Every value is defined at most once, by a result slot whose type matches, and
    /// every constant is canonical. Returns the definition site of each value.
    fn verify_definitions(&self) -> Result<Vec<Option<Site>>, VerifyError> {
        let mut sites: Vec<Option<Site>> = vec![None; self.value_types.len()];
        let mut define = |v: Value, ty: Option<Type>, site: Site| -> Result<(), VerifyError> {
            match (sites.get_mut(v.index()), self.value_type(v)) {
                (Some(slot @ None), Some(found)) if ty.is_none_or(|t| t == found) => {
                    *slot = Some(site);
                    Ok(())
                }
                _ => Err(VerifyError::BadDefinition(v)),
            }
        };
        for (b, data) in self.blocks.iter().enumerate() {
            let block = block_at(b);
            for &p in &data.params {
                define(p, None, Site { block: b, pos: 0 })?;
            }
            for (k, inst) in data.insts.iter().enumerate() {
                if let Op::Iconst { ty, imm } = inst.op
                    && imm & !ty.mask() != 0
                {
                    return Err(VerifyError::NonCanonicalConst { block, ty, imm });
                }
                if !inst.op.conversion_ok() {
                    return Err(VerifyError::BadConversion(block));
                }
                let site = Site {
                    block: b,
                    pos: k + 1,
                };
                for (slot, ty) in inst.results.iter().zip(inst.op.result_types()) {
                    match (slot, ty) {
                        (Some(v), Some(_)) => define(*v, ty, site)?,
                        (None, None) => {}
                        (Some(v), None) => return Err(VerifyError::BadDefinition(*v)),
                        // A result the op produces but nobody named: harmless.
                        (None, Some(_)) => {}
                    }
                }
            }
        }
        Ok(sites)
    }

    /// Terminators present, targets valid, argument counts and types match. Returns the
    /// successor lists (block indices).
    fn verify_edges(&self) -> Result<Vec<Vec<usize>>, VerifyError> {
        let mut succs = Vec::with_capacity(self.blocks.len());
        for (i, data) in self.blocks.iter().enumerate() {
            let from = block_at(i);
            let term = data.term.as_ref().ok_or(VerifyError::Unterminated(from))?;
            let mut list = Vec::with_capacity(2);
            for (to, args) in term.successors() {
                let target = self
                    .block(to)
                    .ok_or(VerifyError::NoSuchBlock { from, to })?;
                if target.params.len() != args.len() {
                    return Err(VerifyError::ArgCount {
                        from,
                        to,
                        expected: target.params.len(),
                        given: args.len(),
                    });
                }
                for (&arg, &param) in args.iter().zip(&target.params) {
                    self.check_type(arg, self.value_type(param), from)?;
                }
                list.push(to.index());
            }
            succs.push(list);
        }
        Ok(succs)
    }

    /// Every use is dominated by its definition: earlier in the same block, or in a
    /// block that dominates the using block. Block arguments are uses at the
    /// terminator of the branching block. Unreachable blocks may only use their own
    /// earlier definitions.
    fn verify_uses(&self, sites: &[Option<Site>], dom: &Dominance) -> Result<(), VerifyError> {
        for (b, data) in self.blocks.iter().enumerate() {
            let block = block_at(b);
            let check = |v: Value, pos: usize| -> Result<(), VerifyError> {
                let ok = match sites.get(v.index()).copied().flatten() {
                    Some(def) if def.block == b => def.pos < pos,
                    Some(def) => dom.dominates(def.block, b),
                    None => false,
                };
                if ok {
                    Ok(())
                } else {
                    Err(VerifyError::Undefined { value: v, block })
                }
            };
            for (k, inst) in data.insts.iter().enumerate() {
                for (v, ty) in inst.op.operands().into_iter().flatten() {
                    check(v, k + 1)?;
                    self.check_type(v, Some(ty), block)?;
                }
            }
            let term_pos = data.insts.len() + 1;
            if let Some(term) = &data.term {
                if let Some((v, ty)) = term.operands() {
                    check(v, term_pos)?;
                    self.check_type(v, Some(ty), block)?;
                }
                for (_, args) in term.successors() {
                    for &a in args {
                        check(a, term_pos)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn check_type(
        &self,
        v: Value,
        expected: Option<Type>,
        block: Block,
    ) -> Result<(), VerifyError> {
        let found = self
            .value_type(v)
            .ok_or(VerifyError::Undefined { value: v, block })?;
        match expected {
            Some(expected) if expected != found => Err(VerifyError::Type {
                value: v,
                expected,
                found,
            }),
            _ => Ok(()),
        }
    }
}

/// Where a value is defined: block index and position inside it (0 = block parameter,
/// `k + 1` = instruction `k`; the terminator uses position `insts.len() + 1`).
#[derive(Clone, Copy, Debug)]
struct Site {
    block: usize,
    pos: usize,
}

/// Dominator tree of the blocks reachable from the entry, with pre/post-order numbers
/// for O(1) dominance queries.
#[derive(Debug)]
struct Dominance {
    /// Pre-order number in the dominator tree; `None` for unreachable blocks.
    pre: Vec<Option<usize>>,
    /// Post-order number in the dominator tree (valid where `pre` is `Some`).
    post: Vec<usize>,
}

impl Dominance {
    const UNDEF: usize = usize::MAX;

    /// `succs[b]`: successor block indices of block `b` (all in range, checked by
    /// `verify_edges`). Block 0 is the entry.
    fn compute(succs: &[Vec<usize>]) -> Self {
        let n = succs.len();
        let succ = |b: usize| succs.get(b).map_or(&[][..], Vec::as_slice);

        // Reverse post-order of the reachable blocks (iterative DFS).
        let mut rpo_num = vec![Self::UNDEF; n];
        let mut postorder = Vec::with_capacity(n);
        let mut visited = vec![false; n];
        let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
        if let Some(v) = visited.get_mut(0) {
            *v = true;
        }
        while let Some(top) = stack.last_mut() {
            let (b, next) = *top;
            if let Some(&s) = succ(b).get(next) {
                top.1 += 1;
                if let Some(v) = visited.get_mut(s)
                    && !*v
                {
                    *v = true;
                    stack.push((s, 0));
                }
            } else {
                postorder.push(b);
                stack.pop();
            }
        }
        let rpo: Vec<usize> = postorder.iter().rev().copied().collect();
        for (k, &b) in rpo.iter().enumerate() {
            rpo_num[b] = k;
        }

        // Predecessors restricted to reachable blocks: unreachable predecessors do not
        // constrain dominance.
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &b in &rpo {
            for &s in succ(b) {
                if let Some(list) = preds.get_mut(s) {
                    list.push(b);
                }
            }
        }

        // Cooper/Harvey/Kennedy iterative immediate dominators.
        let mut idom = vec![Self::UNDEF; n];
        if n > 0 {
            idom[0] = 0;
        }
        let intersect = |idom: &[usize], mut a: usize, mut b: usize| -> usize {
            while a != b {
                while rpo_num[a] > rpo_num[b] {
                    a = idom[a];
                }
                while rpo_num[b] > rpo_num[a] {
                    b = idom[b];
                }
            }
            a
        };
        let mut changed = true;
        while changed {
            changed = false;
            for &b in rpo.iter().skip(1) {
                let mut new = Self::UNDEF;
                for &p in &preds[b] {
                    if idom[p] == Self::UNDEF {
                        continue;
                    }
                    new = if new == Self::UNDEF {
                        p
                    } else {
                        intersect(&idom, p, new)
                    };
                }
                if new != Self::UNDEF && idom[b] != new {
                    idom[b] = new;
                    changed = true;
                }
            }
        }

        // Dominator tree children, then pre/post numbering (iterative DFS).
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &b in rpo.iter().skip(1) {
            if let Some(list) = children.get_mut(idom[b]) {
                list.push(b);
            }
        }
        let mut pre = vec![None; n];
        let mut post = vec![0; n];
        let (mut pre_n, mut post_n) = (0, 0);
        let mut stack: Vec<(usize, usize)> = Vec::new();
        if n > 0 {
            pre[0] = Some(0);
            pre_n = 1;
            stack.push((0, 0));
        }
        while let Some(top) = stack.last_mut() {
            let (b, next) = *top;
            if let Some(&c) = children[b].get(next) {
                top.1 += 1;
                pre[c] = Some(pre_n);
                pre_n += 1;
                stack.push((c, 0));
            } else {
                post[b] = post_n;
                post_n += 1;
                stack.pop();
            }
        }
        Self { pre, post }
    }

    /// Whether block `a` dominates block `b` (reflexive). False if either is
    /// unreachable.
    fn dominates(&self, a: usize, b: usize) -> bool {
        match (
            self.pre.get(a).copied().flatten(),
            self.pre.get(b).copied().flatten(),
        ) {
            (Some(pa), Some(pb)) => {
                pa <= pb && self.post.get(b).copied() <= self.post.get(a).copied()
            }
            _ => false,
        }
    }
}

/// The block with index `i`. Regions never come near `u32::MAX` blocks (one guest page
/// holds 4096 instructions); saturate instead of wrapping just in case.
fn block_at(i: usize) -> Block {
    Block(u32::try_from(i).unwrap_or(u32::MAX))
}
