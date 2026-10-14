//! `Concrete`: the T0 backend of the semantic layer.
//!
//! Every [`Sem`] operation is performed immediately on a [`CpuState`] and a
//! [`DataBus`], using the Arm pseudocode primitives (`sem/prim.rs`). This defines the
//! reference meaning of each operation; the other backends are tested against it.
//!
//! # Memory accesses
//!
//! * An access of `n` bytes at `va` is split at 4 KiB boundaries (correct for every
//!   granule; the guest uses 16 KiB, E-2); each part is translated separately. All
//!   parts of a store are translated and probed before the first byte is written, so a
//!   failing store writes nothing.
//! * Privilege: EL0 permissions at EL0 or for [`MemAttrs::unprivileged`], else EL1.
//! * Alignment: an access must be aligned to its size if [`Align::Required`] or
//!   [`Ctx::align_check`] (`SCTLR_EL1.A`); otherwise unaligned accesses are allowed
//!   (Normal memory). Memory types are not modelled yet (see [`crate::interp::step`]).
//! * Ordering ([`MemAttrs::order`]) has no effect in T0: one vCPU executes one
//!   instruction at a time, in program order.

use super::{Align, BinOp, BranchKind, CmpOp, Ctx, Exception, MemAttrs, Sem, Ty, UnOp, prim};
use crate::cpu::{CpuState, ExceptionLevel, GpReg, Nzcv, SysReg};
use crate::decode::Cond;
use crate::interp::{DataAbort, DataFaultKind, ExecError, StepOutcome};
use crate::mem::DataBus;
use ritsu_core::GuestPhysAddr;
use ritsu_mem::mmu::{Access, AccessKind, Privilege};

/// Accesses never cross this boundary in one piece (smallest translation granule).
const SPLIT: u64 = 4096;

/// Largest single access or probe in bytes (`STP X`: 16; room for `DC ZVA` later).
const MAX_ACCESS: u32 = 64;

/// Executes [`Sem`] operations for the instruction at `cpu.pc`.
///
/// Usage: [`Concrete::new`], [`super::lower`], then [`Concrete::finish`] to commit the
/// new `PC`. If the description raises, drop the backend instead: no write has
/// happened yet (descriptions raise before their first architectural write) and `PC`
/// is unchanged, so the state is precise at the faulting instruction.
pub struct Concrete<'a, B: DataBus + ?Sized = dyn DataBus + 'a> {
    cpu: &'a mut CpuState,
    bus: &'a mut B,
    /// Address of the instruction being executed.
    pc: u64,
    /// `(next PC, taken)` once a control-transfer operation ran.
    branch: Option<(u64, bool)>,
    /// Rule bookkeeping for the debug checks (absent in release builds, where it would
    /// cost every instruction; the static check in `ir::Builder` covers all forms).
    #[cfg(debug_assertions)]
    rules: DebugRules,
}

/// What an instruction has done so far (debug builds only).
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Default)]
struct DebugRules {
    /// A register, flag or system register was written.
    wrote: bool,
    /// A store was performed.
    stored: bool,
    /// A probe was performed.
    probed: bool,
}

impl<B: DataBus + ?Sized> core::fmt::Debug for Concrete<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Concrete")
            .field("pc", &self.pc)
            .field("branch", &self.branch)
            .finish_non_exhaustive()
    }
}

impl<'a, B: DataBus + ?Sized> Concrete<'a, B> {
    /// Starts executing the instruction at `cpu.pc` with memory `bus`. Generic over the
    /// bus so that the run loop monomorphises it (no indirect call per access);
    /// `B = dyn DataBus` works as well.
    pub fn new(cpu: &'a mut CpuState, bus: &'a mut B) -> Self {
        let pc = cpu.pc;
        Self {
            cpu,
            bus,
            pc,
            branch: None,
            #[cfg(debug_assertions)]
            rules: DebugRules::default(),
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

    #[inline]
    fn note_write(&mut self) {
        #[cfg(debug_assertions)]
        {
            assert!(
                self.branch.is_none(),
                "description bug: state written after a branch"
            );
            self.rules.wrote = true;
        }
    }

    /// Debug check before an operation that can raise (other than a store).
    #[inline]
    fn note_may_raise(&self) {
        #[cfg(debug_assertions)]
        assert!(
            !self.rules.wrote && !self.rules.stored && self.branch.is_none(),
            "description bug: raise after an architectural write"
        );
    }

    /// Debug check before a store.
    #[inline]
    fn note_store(&mut self) {
        #[cfg(debug_assertions)]
        {
            assert!(
                !self.rules.wrote && self.branch.is_none(),
                "description bug: store after a register write"
            );
            assert!(
                !self.rules.stored || self.rules.probed,
                "description bug: second store without probe"
            );
            self.rules.stored = true;
        }
    }

    #[inline]
    fn note_probe(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.rules.probed = true;
        }
    }

    fn set_branch(&mut self, next: u64, taken: bool) {
        debug_assert!(
            self.branch.is_none(),
            "description bug: two control transfers in one instruction"
        );
        self.branch = Some((next, taken));
    }

    fn exception(&self, e: Exception) -> ExecError {
        let pc = self.pc;
        match e {
            Exception::Undefined { word } => ExecError::Undefined { pc, word },
            Exception::SupervisorCall { imm } => ExecError::SupervisorCall { pc, imm },
            Exception::Breakpoint { imm } => ExecError::Breakpoint { pc, imm },
            Exception::SpAlignment => ExecError::SpAlignment { pc },
            Exception::Unimplemented { word } => ExecError::Unimplemented { pc, word },
        }
    }

    fn data_abort(
        &self,
        va: u64,
        write: bool,
        size: u32,
        attrs: MemAttrs,
        kind: DataFaultKind,
    ) -> ExecError {
        ExecError::DataAbort {
            pc: self.pc,
            abort: DataAbort {
                va,
                write,
                // Sizes are at most `MAX_ACCESS`, which fits in a u8.
                size: u8::try_from(size).unwrap_or(u8::MAX),
                kind,
                iss: attrs.iss,
            },
        }
    }

    fn access(&self, kind: AccessKind, attrs: MemAttrs) -> Access {
        let privilege = if attrs.unprivileged || self.cpu.el == ExceptionLevel::El0 {
            Privilege::Unprivileged
        } else {
            Privilege::Privileged
        };
        Access::new(kind, privilege)
    }

    /// Alignment check of an access of `size` bytes at `va`.
    fn check_align(
        &self,
        va: u64,
        size: u32,
        write: bool,
        attrs: MemAttrs,
    ) -> Result<(), ExecError> {
        let required = attrs.align == Align::Required || self.ctx().align_check;
        if required && !va.is_multiple_of(u64::from(size)) {
            return Err(self.data_abort(va, write, size, attrs, DataFaultKind::Alignment));
        }
        Ok(())
    }

    /// Translates the bytes `[va, va + len)` (`len <= MAX_ACCESS`) piece by piece and
    /// calls `f(pa, offset, piece_len)` for each piece, in address order. Stops at the
    /// first fault.
    fn for_each_piece(
        &mut self,
        va: u64,
        len: u32,
        kind: AccessKind,
        size: u32,
        attrs: MemAttrs,
        mut f: impl FnMut(&mut B, GuestPhysAddr, usize, usize) -> Result<(), ritsu_mem::MemError>,
    ) -> Result<(), ExecError> {
        let access = self.access(kind, attrs);
        let write = kind == AccessKind::Write;
        let mut done = 0u64;
        let len = u64::from(len.min(MAX_ACCESS));
        while done < len {
            let at = va.wrapping_add(done);
            let n = (SPLIT - at % SPLIT).min(len - done);
            let pa = self.bus.translate(at, access).map_err(|fault| {
                self.data_abort(
                    at,
                    write,
                    size,
                    attrs,
                    DataFaultKind::Translation(fault.kind),
                )
            })?;
            // `done` and `n` are at most `MAX_ACCESS`.
            f(&mut *self.bus, pa, done as usize, n as usize)
                .map_err(|e| self.data_abort(at, write, size, attrs, DataFaultKind::External(e)))?;
            done += n;
        }
        Ok(())
    }
}

/// Debug check that `v` is in canonical form for the consuming type `ty`.
///
/// This is weaker than type equality: it catches a wider value used at a narrower type
/// when its high bits are set, but not a narrower value used at a wider type. Exact
/// type checking is the IR verifier's job (see the rules in `sem/mod.rs`); a full type
/// tag here would make `Concrete`'s value representation differ between debug and
/// release builds for no additional coverage.
fn arg(ty: Ty, v: u64) -> u64 {
    debug_assert!(
        v & !ty.mask() == 0,
        "description bug: {v:#x} is not a canonical {ty:?}"
    );
    v
}

impl<B: DataBus + ?Sized> Sem for Concrete<'_, B> {
    type V = u64;
    type F = Nzcv;
    type Abort = ExecError;

    /// Computed on demand from the current state (most instructions never ask). No
    /// description writes a field that enters `Ctx` before reading it, so this is the
    /// context the instruction started with.
    fn ctx(&self) -> Ctx {
        Ctx::from_state(self.cpu)
    }

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

    fn sysreg(&mut self, r: SysReg) -> u64 {
        match r {
            SysReg::Nzcv => self.cpu.nzcv.to_sysreg(),
            SysReg::Fpcr => self.cpu.sys.fpcr,
            SysReg::Fpsr => self.cpu.sys.fpsr,
            SysReg::TpidrEl0 => self.cpu.sys.tpidr_el0,
        }
    }

    fn set_sysreg(&mut self, r: SysReg, v: u64) {
        self.note_write();
        let v = arg(Ty::I64, v);
        match r {
            SysReg::Nzcv => self.cpu.nzcv = Nzcv::from_sysreg(v),
            SysReg::Fpcr => self.cpu.sys.fpcr = v,
            SysReg::Fpsr => self.cpu.sys.fpsr = v,
            SysReg::TpidrEl0 => self.cpu.sys.tpidr_el0 = v,
        }
    }

    fn add_with_carry(&mut self, ty: Ty, a: u64, b: u64, carry_in: u64) -> (u64, Nzcv) {
        prim::add_with_carry(ty, arg(ty, a), arg(ty, b), arg(Ty::I1, carry_in) != 0)
    }

    fn sub_with_carry(&mut self, ty: Ty, a: u64, b: u64, carry_in: u64) -> (u64, Nzcv) {
        prim::sub_with_carry(ty, arg(ty, a), arg(ty, b), arg(Ty::I1, carry_in) != 0)
    }

    fn flags_logic(&mut self, ty: Ty, result: u64) -> Nzcv {
        prim::flags_logic(ty, arg(ty, result))
    }

    fn flag_c(&mut self, f: Nzcv) -> u64 {
        u64::from(f.c())
    }

    fn select_flags(&mut self, cond: u64, a: Nzcv, b: Nzcv) -> Nzcv {
        if arg(Ty::I1, cond) != 0 { a } else { b }
    }

    fn flags_to_bits(&mut self, f: Nzcv) -> u64 {
        f.to_sysreg()
    }

    fn bits_to_flags(&mut self, v: u64) -> Nzcv {
        Nzcv::from_sysreg(arg(Ty::I64, v))
    }

    fn binop(&mut self, op: BinOp, ty: Ty, a: u64, b: u64) -> u64 {
        prim::binop(op, ty, arg(ty, a), arg(ty, b))
    }

    fn unop(&mut self, op: UnOp, ty: Ty, a: u64) -> u64 {
        prim::unop(op, ty, arg(ty, a))
    }

    fn not(&mut self, ty: Ty, a: u64) -> u64 {
        prim::not(ty, arg(ty, a))
    }

    fn zext(&mut self, from: Ty, to: Ty, v: u64) -> u64 {
        debug_assert!(from.bits() <= to.bits(), "description bug: zext narrows");
        prim::truncate(to, arg(from, v))
    }

    fn sext(&mut self, from: Ty, to: Ty, v: u64) -> u64 {
        debug_assert!(from.bits() <= to.bits(), "description bug: sext narrows");
        prim::sext(from, to, arg(from, v))
    }

    fn trunc(&mut self, from: Ty, to: Ty, v: u64) -> u64 {
        debug_assert!(from.bits() >= to.bits(), "description bug: trunc widens");
        prim::truncate(to, arg(from, v))
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

    fn load(&mut self, ty: Ty, addr: u64, attrs: MemAttrs) -> Result<u64, ExecError> {
        self.note_may_raise();
        let va = arg(Ty::I64, addr);
        let size = ty.bytes();
        self.check_align(va, size, false, attrs)?;
        let mut buf = [0u8; 8];
        self.for_each_piece(
            va,
            size,
            AccessKind::Read,
            size,
            attrs,
            |bus, pa, off, n| bus.read(pa, buf.get_mut(off..off + n).unwrap_or(&mut [])),
        )?;
        Ok(prim::truncate(ty, u64::from_le_bytes(buf)))
    }

    fn store(&mut self, ty: Ty, addr: u64, value: u64, attrs: MemAttrs) -> Result<(), ExecError> {
        self.note_store();
        let va = arg(Ty::I64, addr);
        let size = ty.bytes();
        let bytes = arg(ty, value).to_le_bytes();
        self.check_align(va, size, true, attrs)?;
        // Translate and probe every piece first, then write: a failing store writes
        // nothing. A store crosses at most one boundary (size <= 8), so two pieces.
        let mut pieces = [(GuestPhysAddr(0), 0usize, 0usize); 2];
        let mut count = 0;
        self.for_each_piece(
            va,
            size,
            AccessKind::Write,
            size,
            attrs,
            |bus, pa, off, n| {
                bus.probe_write(pa, n)?;
                if let Some(slot) = pieces.get_mut(count) {
                    *slot = (pa, off, n);
                }
                count += 1;
                Ok(())
            },
        )?;
        for &(pa, off, n) in pieces.iter().take(count) {
            let at = va.wrapping_add(off as u64);
            self.bus
                .write(pa, bytes.get(off..off + n).unwrap_or(&[]))
                .map_err(|e| self.data_abort(at, true, size, attrs, DataFaultKind::External(e)))?;
        }
        Ok(())
    }

    fn probe(&mut self, ty: Ty, count: u8, addr: u64, attrs: MemAttrs) -> Result<(), ExecError> {
        self.note_may_raise();
        let va = arg(Ty::I64, addr);
        let size = ty.bytes();
        // Contiguous elements: if the first is aligned, all are.
        self.check_align(va, size, true, attrs)?;
        let total = size.saturating_mul(u32::from(count));
        debug_assert!(total <= MAX_ACCESS, "description bug: probe too large");
        self.for_each_piece(
            va,
            total,
            AccessKind::Write,
            size,
            attrs,
            |bus, pa, _, n| bus.probe_write(pa, n),
        )?;
        self.note_probe();
        Ok(())
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
        self.note_may_raise();
        Err(self.exception(e))
    }

    fn raise_if(&mut self, cond: u64, e: Exception) -> Result<(), ExecError> {
        self.note_may_raise();
        if arg(Ty::I1, cond) != 0 {
            Err(self.exception(e))
        } else {
            Ok(())
        }
    }
}
