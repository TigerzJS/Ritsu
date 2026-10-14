//! `Builder`: the IR backend of the semantic layer (T2's `IrBuilder`, dbt.md §3.2).

use super::{Block, BlockData, Exit, InstData, Op, Region, Terminator, Type, Value};
use crate::cpu::{GpReg, SysReg};
use crate::decode::{Cond, Inst};
use crate::sem::{self, BinOp, BranchKind, CmpOp, Ctx, Exception, MemAttrs, Sem, Ty, UnOp};
use crate::tier::TbFlags;
use core::convert::Infallible;

/// A violation of the ordering rules for descriptions (`sem` module documentation),
/// found statically while lowering (R-DBT-01 P1-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleViolation {
    /// Offset of the offending instruction from the region entry.
    pub pc_offset: i64,
    /// What went wrong.
    pub kind: RuleViolationKind,
}

/// Kinds of [`RuleViolation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleViolationKind {
    /// An operation follows the instruction's control transfer.
    AfterControlTransfer,
    /// An operation that can raise follows a register/flag/system-register write or a
    /// store.
    RaiseAfterWrite,
    /// A store follows a register/flag/system-register write.
    StoreAfterWrite,
    /// A second store without a preceding [`Sem::probe`].
    UnprobedSecondStore,
}

/// Per-instruction state for the rule check.
#[derive(Clone, Copy, Debug, Default)]
struct InsnRules {
    wrote: bool,
    stored: bool,
    probed: bool,
    transferred: bool,
}

/// Builds a [`Region`] by running instruction descriptions on symbolic values.
///
/// Lower each guest instruction with [`crate::sem::lower`] after announcing its offset
/// from the region entry with [`Builder::set_pc_offset`], then call
/// [`Builder::finish`]. Conditional branches and [`Sem::raise_if`] split the current
/// block: the taken edge leaves the region, the fall-through edge continues in a fresh
/// block. After an unconditional exit, further operations go to a fresh, unreachable
/// block (dead code, dropped by [`Builder::finish`] if empty).
///
/// While lowering, the builder checks the description rules of the `sem` module per
/// instruction and records every [`RuleViolation`] ([`Builder::violations`]). Because
/// data-dependent choices are expressed with `select`, the IR of one instruction
/// contains the operations of every runtime path, so the check is complete for the
/// lowered instruction form (and context).
#[derive(Debug)]
pub struct Builder {
    region: Region,
    cur: Block,
    /// Whether `cur` has a predecessor (or is the entry).
    cur_reachable: bool,
    pc_offset: i64,
    ctx: Ctx,
    rules: InsnRules,
    violations: Vec<RuleViolation>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    /// Starts a region with an empty entry block for the default context (EL1, all
    /// checks off); the first instruction is at offset 0.
    #[must_use]
    pub fn new() -> Self {
        Self::with_ctx(Ctx::from_tb_flags(TbFlags(TbFlags::EL1)))
    }

    /// Starts a region specialised on `ctx` (from the block key's [`TbFlags`]).
    #[must_use]
    pub fn with_ctx(ctx: Ctx) -> Self {
        Self {
            region: Region {
                blocks: vec![BlockData::default()],
                value_types: Vec::new(),
            },
            cur: Region::ENTRY,
            cur_reachable: true,
            pc_offset: 0,
            ctx,
            rules: InsnRules::default(),
            violations: Vec::new(),
        }
    }

    /// Sets the offset (from the region entry) of the instruction lowered next. This
    /// is the instruction boundary for the rule check.
    pub fn set_pc_offset(&mut self, offset: i64) {
        self.pc_offset = offset;
        self.rules = InsnRules::default();
    }

    /// Rule violations recorded so far (empty for correct descriptions).
    #[must_use]
    pub fn violations(&self) -> &[RuleViolation] {
        &self.violations
    }

    /// Terminates the open block with a direct exit to region entry + `fallthrough`
    /// (the instruction after the last one lowered) and returns the region.
    #[must_use]
    pub fn finish(mut self, fallthrough: i64) -> Region {
        let open_is_dead_and_empty = !self.cur_reachable
            && self.cur.index() + 1 == self.region.blocks.len()
            && self
                .region
                .block(self.cur)
                .is_some_and(|b| b.insts.is_empty() && b.params.is_empty());
        if open_is_dead_and_empty {
            self.region.blocks.pop();
        } else {
            self.set_term(Terminator::Exit(Exit::Direct {
                offset: fallthrough,
                kind: BranchKind::Jump,
            }));
        }
        self.region
    }

    /// Lowers a straight-line sequence of instructions at offsets `0, 4, 8, …` and
    /// finishes the region after the last one.
    #[must_use]
    pub fn lower_sequence(insts: &[Inst]) -> Region {
        Self::lower_sequence_checked(Self::new(), insts).0
    }

    /// Like [`Builder::lower_sequence`] on a given builder, also returning the rule
    /// violations.
    #[must_use]
    pub fn lower_sequence_checked(mut b: Self, insts: &[Inst]) -> (Region, Vec<RuleViolation>) {
        let mut offset = 0i64;
        for &inst in insts {
            b.set_pc_offset(offset);
            match sem::lower(&mut b, inst) {
                Ok(()) => {}
                Err(never) => match never {},
            }
            offset = offset.wrapping_add(4);
        }
        let violations = core::mem::take(&mut b.violations);
        (b.finish(offset), violations)
    }

    fn violate(&mut self, kind: RuleViolationKind) {
        self.violations.push(RuleViolation {
            pc_offset: self.pc_offset,
            kind,
        });
    }

    /// Rule bookkeeping for every operation.
    fn note_op(&mut self) {
        if self.rules.transferred {
            self.violate(RuleViolationKind::AfterControlTransfer);
        }
    }

    fn note_write(&mut self) {
        self.rules.wrote = true;
    }

    /// An operation that can raise (other than a store).
    fn note_may_raise(&mut self) {
        if self.rules.wrote || self.rules.stored {
            self.violate(RuleViolationKind::RaiseAfterWrite);
        }
    }

    fn note_transfer(&mut self) {
        self.note_op();
        self.rules.transferred = true;
    }

    fn new_value(&mut self, ty: Type) -> Value {
        // A region holds at most a few thousand instructions; u32 cannot overflow in
        // practice, saturating keeps this panic-free regardless.
        let v = Value(u32::try_from(self.region.value_types.len()).unwrap_or(u32::MAX));
        self.region.value_types.push(ty);
        v
    }

    fn new_block(&mut self) -> Block {
        let b = super::block_at(self.region.blocks.len());
        self.region.blocks.push(BlockData::default());
        b
    }

    fn push(&mut self, op: Op) -> [Option<Value>; 2] {
        self.note_op();
        let [t0, t1] = op.result_types();
        let results = [t0.map(|t| self.new_value(t)), t1.map(|t| self.new_value(t))];
        let pc_offset = self.pc_offset;
        if let Some(block) = self.region.blocks.get_mut(self.cur.index()) {
            block.insts.push(InstData {
                op,
                results,
                pc_offset,
            });
        }
        results
    }

    /// Pushes an op with exactly one result.
    fn push1(&mut self, op: Op) -> Value {
        match self.push(op) {
            [Some(v), _] => v,
            // Every caller passes an op with one result (see `Op::result_types`); a
            // fresh value keeps this total without panicking.
            [None, _] => self.new_value(Type::I64),
        }
    }

    /// Pushes an op with exactly two results.
    fn push2(&mut self, op: Op, ty: Ty) -> (Value, Value) {
        match self.push(op) {
            [Some(r), Some(f)] => (r, f),
            // Unreachable for the ops passed here (two results each).
            _ => (self.new_value(ty.into()), self.new_value(Type::Flags)),
        }
    }

    fn set_term(&mut self, term: Terminator) {
        if let Some(block) = self.region.blocks.get_mut(self.cur.index()) {
            block.term = Some(term);
        }
    }

    /// Ends the current block with an exit; following ops go to a dead block.
    fn exit(&mut self, exit: Exit) {
        self.set_term(Terminator::Exit(exit));
        self.cur = self.new_block();
        self.cur_reachable = false;
    }

    /// Ends the current block with a two-way branch whose taken edge leaves the region
    /// through `exit`; continues in the fall-through block.
    fn exit_if(&mut self, cond: Value, exit: Exit) {
        let taken = self.new_block();
        let fallthrough = self.new_block();
        self.set_term(Terminator::Branch {
            cond,
            then_dest: taken,
            then_args: Vec::new(),
            else_dest: fallthrough,
            else_args: Vec::new(),
        });
        if let Some(block) = self.region.blocks.get_mut(taken.index()) {
            block.term = Some(Terminator::Exit(exit));
        }
        self.cur = fallthrough;
        self.cur_reachable = true;
    }
}

impl Sem for Builder {
    type V = Value;
    type F = Value;
    type Abort = Infallible;

    fn ctx(&self) -> Ctx {
        self.ctx
    }

    fn imm(&mut self, ty: Ty, value: u64) -> Value {
        self.push1(Op::Iconst {
            ty,
            imm: sem::truncate(ty, value),
        })
    }

    fn pc_rel(&mut self, offset: i64) -> Value {
        let offset = self.pc_offset.wrapping_add(offset);
        self.push1(Op::PcRel { offset })
    }

    fn gpr(&mut self, reg: GpReg, ty: Ty) -> Value {
        self.push1(Op::GetGpr { reg, ty })
    }

    fn set_gpr(&mut self, reg: GpReg, ty: Ty, value: Value) {
        self.push(Op::SetGpr { reg, ty, value });
        self.note_write();
    }

    fn nzcv(&mut self) -> Value {
        self.push1(Op::GetNzcv)
    }

    fn set_nzcv(&mut self, flags: Value) {
        self.push(Op::SetNzcv { flags });
        self.note_write();
    }

    fn sysreg(&mut self, reg: SysReg) -> Value {
        self.push1(Op::GetSysreg { reg })
    }

    fn set_sysreg(&mut self, reg: SysReg, value: Value) {
        self.push(Op::SetSysreg { reg, value });
        self.note_write();
    }

    fn add_with_carry(&mut self, ty: Ty, a: Value, b: Value, carry_in: Value) -> (Value, Value) {
        self.push2(Op::AddWithCarry { ty, a, b, carry_in }, ty)
    }

    fn sub_with_carry(&mut self, ty: Ty, a: Value, b: Value, carry_in: Value) -> (Value, Value) {
        self.push2(Op::SubWithCarry { ty, a, b, carry_in }, ty)
    }

    fn flags_logic(&mut self, ty: Ty, a: Value) -> Value {
        self.push1(Op::FlagsLogic { ty, a })
    }

    fn flag_c(&mut self, flags: Value) -> Value {
        self.push1(Op::FlagC { flags })
    }

    fn select_flags(&mut self, cond: Value, a: Value, b: Value) -> Value {
        self.push1(Op::SelectFlags { cond, a, b })
    }

    fn flags_to_bits(&mut self, flags: Value) -> Value {
        self.push1(Op::FlagsToBits { flags })
    }

    fn bits_to_flags(&mut self, a: Value) -> Value {
        self.push1(Op::BitsToFlags { a })
    }

    fn binop(&mut self, op: BinOp, ty: Ty, a: Value, b: Value) -> Value {
        self.push1(Op::Binary { op, ty, a, b })
    }

    fn unop(&mut self, op: UnOp, ty: Ty, a: Value) -> Value {
        self.push1(Op::Unary { op, ty, a })
    }

    fn not(&mut self, ty: Ty, a: Value) -> Value {
        self.push1(Op::Not { ty, a })
    }

    fn zext(&mut self, from: Ty, to: Ty, a: Value) -> Value {
        self.push1(Op::Ext {
            signed: false,
            from,
            to,
            a,
        })
    }

    fn sext(&mut self, from: Ty, to: Ty, a: Value) -> Value {
        self.push1(Op::Ext {
            signed: true,
            from,
            to,
            a,
        })
    }

    fn trunc(&mut self, from: Ty, to: Ty, a: Value) -> Value {
        self.push1(Op::Trunc { from, to, a })
    }

    fn icmp(&mut self, cc: CmpOp, ty: Ty, a: Value, b: Value) -> Value {
        self.push1(Op::Icmp { cc, ty, a, b })
    }

    fn select(&mut self, ty: Ty, cond: Value, a: Value, b: Value) -> Value {
        self.push1(Op::Select { ty, cond, a, b })
    }

    fn cond_holds(&mut self, cond: Cond, flags: Value) -> Value {
        self.push1(Op::CondHolds { cond, flags })
    }

    fn load(&mut self, ty: Ty, addr: Value, attrs: MemAttrs) -> Result<Value, Infallible> {
        self.note_may_raise();
        Ok(self.push1(Op::Load { ty, attrs, addr }))
    }

    fn store(
        &mut self,
        ty: Ty,
        addr: Value,
        value: Value,
        attrs: MemAttrs,
    ) -> Result<(), Infallible> {
        if self.rules.wrote {
            self.violate(RuleViolationKind::StoreAfterWrite);
        }
        if self.rules.stored && !self.rules.probed {
            self.violate(RuleViolationKind::UnprobedSecondStore);
        }
        self.push(Op::Store {
            ty,
            attrs,
            addr,
            value,
        });
        self.rules.stored = true;
        Ok(())
    }

    fn probe(&mut self, ty: Ty, count: u8, addr: Value, attrs: MemAttrs) -> Result<(), Infallible> {
        self.note_may_raise();
        self.push(Op::Probe {
            ty,
            count,
            attrs,
            addr,
        });
        self.rules.probed = true;
        Ok(())
    }

    fn branch(&mut self, cond: Option<Value>, offset: i64, kind: BranchKind) {
        self.note_transfer();
        let target = Exit::Direct {
            offset: self.pc_offset.wrapping_add(offset),
            kind,
        };
        match cond {
            None => self.exit(target),
            Some(cond) => self.exit_if(cond, target),
        }
    }

    fn branch_indirect(&mut self, target: Value, kind: BranchKind) {
        self.note_transfer();
        self.exit(Exit::Indirect { target, kind });
    }

    fn raise(&mut self, exception: Exception) -> Result<(), Infallible> {
        self.note_may_raise();
        self.note_transfer();
        let pc_offset = self.pc_offset;
        self.exit(Exit::Raise {
            exception,
            pc_offset,
        });
        Ok(())
    }

    fn raise_if(&mut self, cond: Value, exception: Exception) -> Result<(), Infallible> {
        self.note_op();
        self.note_may_raise();
        let pc_offset = self.pc_offset;
        self.exit_if(
            cond,
            Exit::Raise {
                exception,
                pc_offset,
            },
        );
        Ok(())
    }
}
