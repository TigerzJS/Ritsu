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

pub use builder::Builder;

use crate::cpu::GpReg;
use crate::decode::Cond;
use crate::sem::{BinOp, BranchKind, CmpOp, Exception, Ty};
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
    /// `Sem::not`. Result: `ty`.
    Not {
        /// Width.
        ty: Ty,
        /// Operand (`ty`).
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
}

impl Op {
    /// Types of the results (at most two).
    #[must_use]
    pub fn result_types(&self) -> [Option<Type>; 2] {
        match *self {
            Self::Iconst { ty, .. }
            | Self::GetGpr { ty, .. }
            | Self::Binary { ty, .. }
            | Self::Not { ty, .. }
            | Self::Select { ty, .. } => [Some(ty.into()), None],
            Self::PcRel { .. } => [Some(Type::I64), None],
            Self::GetNzcv => [Some(Type::Flags), None],
            Self::AddWithCarry { ty, .. } => [Some(ty.into()), Some(Type::Flags)],
            Self::Icmp { .. } | Self::CondHolds { .. } => [Some(Type::I1), None],
            Self::SetGpr { .. } | Self::SetNzcv { .. } => [None, None],
        }
    }

    /// Operands with the type each must have (at most three).
    #[must_use]
    pub fn operands(&self) -> [Option<(Value, Type)>; 3] {
        match *self {
            Self::Iconst { .. } | Self::PcRel { .. } | Self::GetGpr { .. } | Self::GetNzcv => {
                [None, None, None]
            }
            Self::SetGpr { ty, value, .. } => [Some((value, ty.into())), None, None],
            Self::SetNzcv { flags } | Self::CondHolds { flags, .. } => {
                [Some((flags, Type::Flags)), None, None]
            }
            Self::AddWithCarry { ty, a, b, carry_in } => [
                Some((a, ty.into())),
                Some((b, ty.into())),
                Some((carry_in, Type::I1)),
            ],
            Self::Binary { ty, a, b, .. } | Self::Icmp { ty, a, b, .. } => {
                [Some((a, ty.into())), Some((b, ty.into())), None]
            }
            Self::Not { ty, a } => [Some((a, ty.into())), None, None],
            Self::Select { ty, cond, a, b } => [
                Some((cond, Type::I1)),
                Some((a, ty.into())),
                Some((b, ty.into())),
            ],
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
    /// A value is defined more than once, or a result slot does not match the op.
    #[error("{0} is defined more than once or by a mismatching result slot")]
    BadDefinition(Value),
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
    /// exist with matching argument counts, every use has the type its operation
    /// requires, every value is defined exactly once and every use is reached only
    /// through paths that define it (SSA dominance).
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
        self.verify_definitions()?;
        let preds = self.verify_edges()?;
        self.verify_uses(&preds)
    }

    /// Every value has exactly one definition whose slot type matches.
    fn verify_definitions(&self) -> Result<(), VerifyError> {
        let mut defined = vec![false; self.value_types.len()];
        let mut define = |v: Value, ty: Option<Type>| -> Result<(), VerifyError> {
            match (defined.get_mut(v.index()), self.value_type(v)) {
                (Some(slot), Some(found)) if !*slot && ty.is_none_or(|t| t == found) => {
                    *slot = true;
                    Ok(())
                }
                _ => Err(VerifyError::BadDefinition(v)),
            }
        };
        for data in &self.blocks {
            for &p in &data.params {
                define(p, None)?;
            }
            for inst in &data.insts {
                for (slot, ty) in inst.results.iter().zip(inst.op.result_types()) {
                    match (slot, ty) {
                        (Some(v), Some(_)) => define(*v, ty)?,
                        (None, None) => {}
                        (Some(v), None) => return Err(VerifyError::BadDefinition(*v)),
                        // A result the op produces but nobody named: harmless.
                        (None, Some(_)) => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// Terminators present, targets valid, argument counts and types match. Returns the
    /// predecessor lists.
    fn verify_edges(&self) -> Result<Vec<Vec<Block>>, VerifyError> {
        let mut preds = vec![Vec::new(); self.blocks.len()];
        for (i, data) in self.blocks.iter().enumerate() {
            let from = block_at(i);
            let term = data.term.as_ref().ok_or(VerifyError::Unterminated(from))?;
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
                if let Some(list) = preds.get_mut(to.index()) {
                    list.push(from);
                }
            }
        }
        Ok(preds)
    }

    /// Dominance via "available on every path" dataflow, then a linear scan per block.
    fn verify_uses(&self, preds: &[Vec<Block>]) -> Result<(), VerifyError> {
        let n = self.value_types.len();
        let reachable = self.reachable(preds.len());
        // avail_out[b]: values defined on every path from the entry to the end of b.
        // Optimistic start (all) for reachable blocks, refined to the greatest fixpoint.
        let mut avail_out: Vec<Vec<bool>> = reachable.iter().map(|&r| vec![r; n]).collect();
        let mut changed = true;
        while changed {
            changed = false;
            for (i, data) in self.blocks.iter().enumerate() {
                if !reachable[i] {
                    continue;
                }
                let mut avail = self.avail_in(i, preds, &avail_out, n);
                for v in defs(data) {
                    mark(&mut avail, v);
                }
                if avail != avail_out[i] {
                    avail_out[i] = avail;
                    changed = true;
                }
            }
        }
        for (i, data) in self.blocks.iter().enumerate() {
            let block = block_at(i);
            let mut avail = if reachable[i] {
                self.avail_in(i, preds, &avail_out, n)
            } else {
                vec![false; n]
            };
            let use_value = |v: Value, avail: &[bool]| {
                if avail.get(v.index()).copied().unwrap_or(false) {
                    Ok(())
                } else {
                    Err(VerifyError::Undefined { value: v, block })
                }
            };
            for &p in &data.params {
                mark(&mut avail, p);
            }
            for inst in &data.insts {
                for (v, ty) in inst.op.operands().into_iter().flatten() {
                    use_value(v, &avail)?;
                    self.check_type(v, Some(ty), block)?;
                }
                for &v in inst.results.iter().flatten() {
                    mark(&mut avail, v);
                }
            }
            if let Some(term) = &data.term {
                if let Some((v, ty)) = term.operands() {
                    use_value(v, &avail)?;
                    self.check_type(v, Some(ty), block)?;
                }
                for (_, args) in term.successors() {
                    for &a in args {
                        use_value(a, &avail)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn avail_in(
        &self,
        i: usize,
        preds: &[Vec<Block>],
        avail_out: &[Vec<bool>],
        n: usize,
    ) -> Vec<bool> {
        if i == Self::ENTRY.index() {
            return vec![false; n];
        }
        let mut avail: Option<Vec<bool>> = None;
        for p in preds.get(i).into_iter().flatten() {
            let Some(out) = avail_out.get(p.index()) else {
                continue;
            };
            avail = Some(match avail {
                None => out.clone(),
                Some(a) => a.iter().zip(out).map(|(&x, &y)| x && y).collect(),
            });
        }
        avail.unwrap_or_else(|| vec![false; n])
    }

    fn reachable(&self, n: usize) -> Vec<bool> {
        let mut seen = vec![false; n];
        let mut stack = vec![Self::ENTRY];
        while let Some(b) = stack.pop() {
            match seen.get_mut(b.index()) {
                Some(s) if !*s => *s = true,
                _ => continue,
            }
            if let Some(term) = self.block(b).and_then(|d| d.term.as_ref()) {
                stack.extend(term.successors().map(|(to, _)| to));
            }
        }
        seen
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

/// Marks `v` as available (out-of-range values were rejected by
/// `verify_definitions` already; ignore them here rather than index out of bounds).
fn mark(avail: &mut [bool], v: Value) {
    if let Some(slot) = avail.get_mut(v.index()) {
        *slot = true;
    }
}

/// Values defined in a block (parameters and results).
fn defs(data: &BlockData) -> impl Iterator<Item = Value> + '_ {
    data.params.iter().copied().chain(
        data.insts
            .iter()
            .flat_map(|inst| inst.results.iter().flatten().copied()),
    )
}

/// The block with index `i`. Regions never come near `u32::MAX` blocks (one guest page
/// holds 4096 instructions); saturate instead of wrapping just in case.
fn block_at(i: usize) -> Block {
    Block(u32::try_from(i).unwrap_or(u32::MAX))
}
