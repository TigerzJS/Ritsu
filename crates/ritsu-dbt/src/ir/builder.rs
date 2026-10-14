//! `Builder`: the IR backend of the semantic layer (T2's `IrBuilder`, dbt.md §3.2).

use super::{Block, BlockData, Exit, InstData, Op, Region, Terminator, Type, Value};
use crate::cpu::GpReg;
use crate::decode::{Cond, Inst};
use crate::sem::{self, BinOp, BranchKind, CmpOp, Exception, Sem, Ty};
use core::convert::Infallible;

/// Builds a [`Region`] by running instruction descriptions on symbolic values.
///
/// Lower each guest instruction with [`crate::sem::lower`] after announcing its offset
/// from the region entry with [`Builder::set_pc_offset`], then call
/// [`Builder::finish`]. Conditional branches split the current block: the taken edge
/// leaves the region, the fall-through edge continues in a fresh block. After an
/// unconditional exit, further operations go to a fresh, unreachable block (dead code,
/// dropped by [`Builder::finish`] if empty).
#[derive(Debug)]
pub struct Builder {
    region: Region,
    cur: Block,
    /// Whether `cur` has a predecessor (or is the entry).
    cur_reachable: bool,
    pc_offset: i64,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    /// Starts a region with an empty entry block; the first instruction is at offset 0.
    #[must_use]
    pub fn new() -> Self {
        Self {
            region: Region {
                blocks: vec![BlockData::default()],
                value_types: Vec::new(),
            },
            cur: Region::ENTRY,
            cur_reachable: true,
            pc_offset: 0,
        }
    }

    /// Sets the offset (from the region entry) of the instruction lowered next.
    pub fn set_pc_offset(&mut self, offset: i64) {
        self.pc_offset = offset;
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
        let mut b = Self::new();
        let mut offset = 0i64;
        for &inst in insts {
            b.set_pc_offset(offset);
            match sem::lower(&mut b, inst) {
                Ok(()) => {}
                Err(never) => match never {},
            }
            offset = offset.wrapping_add(4);
        }
        b.finish(offset)
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
}

impl Sem for Builder {
    type V = Value;
    type F = Value;
    type Abort = Infallible;

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
    }

    fn nzcv(&mut self) -> Value {
        self.push1(Op::GetNzcv)
    }

    fn set_nzcv(&mut self, flags: Value) {
        self.push(Op::SetNzcv { flags });
    }

    fn add_with_carry(&mut self, ty: Ty, a: Value, b: Value, carry_in: Value) -> (Value, Value) {
        match self.push(Op::AddWithCarry { ty, a, b, carry_in }) {
            [Some(r), Some(f)] => (r, f),
            // Unreachable: AddWithCarry always has two results.
            _ => (self.new_value(ty.into()), self.new_value(Type::Flags)),
        }
    }

    fn binop(&mut self, op: BinOp, ty: Ty, a: Value, b: Value) -> Value {
        self.push1(Op::Binary { op, ty, a, b })
    }

    fn not(&mut self, ty: Ty, a: Value) -> Value {
        self.push1(Op::Not { ty, a })
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

    fn branch(&mut self, cond: Option<Value>, offset: i64, kind: BranchKind) {
        let target = Exit::Direct {
            offset: self.pc_offset.wrapping_add(offset),
            kind,
        };
        match cond {
            None => self.exit(target),
            Some(cond) => {
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
                    block.term = Some(Terminator::Exit(target));
                }
                self.cur = fallthrough;
                self.cur_reachable = true;
            }
        }
    }

    fn branch_indirect(&mut self, target: Value, kind: BranchKind) {
        self.exit(Exit::Indirect { target, kind });
    }

    fn raise(&mut self, exception: Exception) -> Result<(), Infallible> {
        let pc_offset = self.pc_offset;
        self.exit(Exit::Raise {
            exception,
            pc_offset,
        });
        Ok(())
    }
}
