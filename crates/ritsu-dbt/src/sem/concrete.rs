//! `Concrete`: the T0 backend of the semantic layer.
//!
//! Every [`Sem`] operation is performed immediately on a [`CpuState`], using the
//! Arm pseudocode primitives (`sem/prim.rs`). This defines the reference meaning of each
//! operation; the other backends are tested against it.

use super::{BinOp, BranchKind, CmpOp, Exception, Sem, Ty, prim};
use crate::cpu::{CpuState, GpReg, Nzcv};
use crate::decode::Cond;
use crate::interp::{ExecError, StepOutcome};

/// Executes [`Sem`] operations for the instruction at `cpu.pc`.
///
/// Usage: [`Concrete::new`], [`super::lower`], then [`Concrete::finish`] to commit the
/// new `PC`. If the description raises, drop the backend instead: no write has
/// happened yet (descriptions raise before their first architectural write) and `PC`
/// is unchanged, so the state is precise at the faulting instruction.
#[derive(Debug)]
pub struct Concrete<'a> {
    cpu: &'a mut CpuState,
    /// Address of the instruction being executed.
    pc: u64,
    /// `(next PC, taken)` once a control-transfer operation ran.
    branch: Option<(u64, bool)>,
    /// Whether architectural state was written (for the precise-exception check).
    wrote: bool,
}

impl<'a> Concrete<'a> {
    /// Starts executing the instruction at `cpu.pc`.
    pub fn new(cpu: &'a mut CpuState) -> Self {
        let pc = cpu.pc;
        Self {
            cpu,
            pc,
            branch: None,
            wrote: false,
        }
    }

    /// Commits the next `PC` (branch target, or `PC + 4` if the instruction did not
    /// branch) and reports what happened.
    pub fn finish(self) -> StepOutcome {
        match self.branch {
            Some((next, taken)) => {
                self.cpu.pc = next;
                StepOutcome::Branch { taken }
            }
            None => {
                self.cpu.pc = self.pc.wrapping_add(4);
                StepOutcome::Sequential
            }
        }
    }

    fn note_write(&mut self) {
        debug_assert!(
            self.branch.is_none(),
            "description bug: state written after a branch"
        );
        self.wrote = true;
    }

    fn set_branch(&mut self, next: u64, taken: bool) {
        debug_assert!(
            self.branch.is_none(),
            "description bug: two control transfers in one instruction"
        );
        self.branch = Some((next, taken));
    }
}

/// Debug check that `v` is in canonical form for `ty`: catches descriptions that use a
/// value at a different type than it was produced with.
fn arg(ty: Ty, v: u64) -> u64 {
    debug_assert!(
        v & !ty.mask() == 0,
        "description bug: {v:#x} is not a canonical {ty:?}"
    );
    v
}

impl Sem for Concrete<'_> {
    type V = u64;
    type F = Nzcv;
    type Abort = ExecError;

    fn imm(&mut self, ty: Ty, value: u64) -> u64 {
        prim::truncate(ty, value)
    }

    fn pc_rel(&mut self, offset: i64) -> u64 {
        self.pc.wrapping_add_signed(offset)
    }

    fn gpr(&mut self, r: GpReg, ty: Ty) -> u64 {
        prim::truncate(ty, self.cpu.read(r))
    }

    fn set_gpr(&mut self, r: GpReg, ty: Ty, v: u64) {
        self.note_write();
        self.cpu.write(r, prim::truncate(ty, arg(ty, v)));
    }

    fn nzcv(&mut self) -> Nzcv {
        self.cpu.nzcv
    }

    fn set_nzcv(&mut self, f: Nzcv) {
        self.note_write();
        self.cpu.nzcv = f;
    }

    fn add_with_carry(&mut self, ty: Ty, a: u64, b: u64, carry_in: u64) -> (u64, Nzcv) {
        prim::add_with_carry(ty, arg(ty, a), arg(ty, b), arg(Ty::I1, carry_in) != 0)
    }

    fn binop(&mut self, op: BinOp, ty: Ty, a: u64, b: u64) -> u64 {
        prim::binop(op, ty, arg(ty, a), arg(ty, b))
    }

    fn not(&mut self, ty: Ty, a: u64) -> u64 {
        prim::not(ty, arg(ty, a))
    }

    fn icmp(&mut self, cc: CmpOp, ty: Ty, a: u64, b: u64) -> u64 {
        u64::from(prim::icmp(cc, ty, arg(ty, a), arg(ty, b)))
    }

    fn select(&mut self, ty: Ty, cond: u64, a: u64, b: u64) -> u64 {
        if arg(Ty::I1, cond) != 0 {
            arg(ty, a)
        } else {
            arg(ty, b)
        }
    }

    fn cond_holds(&mut self, cond: Cond, f: Nzcv) -> u64 {
        u64::from(prim::condition_holds(cond, f))
    }

    fn branch(&mut self, cond: Option<u64>, offset: i64, _kind: BranchKind) {
        let taken = cond.is_none_or(|c| arg(Ty::I1, c) != 0);
        let next = if taken {
            self.pc.wrapping_add_signed(offset)
        } else {
            self.pc.wrapping_add(4)
        };
        self.set_branch(next, taken);
    }

    fn branch_indirect(&mut self, target: u64, _kind: BranchKind) {
        self.set_branch(arg(Ty::I64, target), true);
    }

    fn raise(&mut self, e: Exception) -> Result<(), ExecError> {
        debug_assert!(
            !self.wrote && self.branch.is_none(),
            "description bug: raise after an architectural write"
        );
        Err(match e {
            Exception::Undefined { word } => ExecError::Undefined { pc: self.pc, word },
        })
    }
}
