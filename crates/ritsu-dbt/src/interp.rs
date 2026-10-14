//! T0: the reference interpreter.
//!
//! T0 executes one instruction per [`step`]: fetch at `PC` through a [`Translate`]
//! implementation, [`decode`], then run the instruction's description from the semantic
//! layer ([`crate::sem::lower`]) on the [`Concrete`] backend. This module contains no
//! instruction semantics of its own (ADR-0006: one semantic source); it only fetches
//! and dispatches. T0 is the oracle for the differential tests of T1/T2 and runs cold
//! code.
//!
//! Guest input is hostile: nothing here panics on any instruction word, address or
//! register value. Everything the guest can trigger is an [`ExecError`], which the
//! exception layer (WP-DBT-07) turns into the architectural guest exception.

use crate::cpu::{CpuState, ExceptionLevel};
use crate::decode::{Inst, decode};
use crate::sem::{self, concrete::Concrete};
use ritsu_core::GuestVirtAddr;
use ritsu_mem::mmu::{
    Access, AccessKind, IdentityTranslate, Privilege, Translate, TranslationFault,
};
use ritsu_mem::{GuestMemory, MemError};

/// What happened in one successfully executed instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    /// Not a branch; `PC` advanced by 4.
    Sequential,
    /// A branch instruction executed (end of a basic block).
    Branch {
        /// Whether control left the fall-through path. Unconditional branches are always
        /// taken.
        taken: bool,
    },
}

/// A guest-triggered condition that stops execution of the current instruction.
///
/// `PC` and all registers are left unchanged (precise state at the faulting
/// instruction), so the exception layer can report `ELR_EL1 = PC`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExecError {
    /// The word is not a (supported) instruction → Undefined Instruction exception
    /// (`ESR_ELx.EC = 0`).
    #[error("undefined instruction {word:#010x} at pc {pc:#x}")]
    Undefined {
        /// Address of the instruction.
        pc: u64,
        /// The instruction word.
        word: u32,
    },
    /// `PC[1:0] != 0` → PC alignment fault (`EC = 0x22`).
    #[error("pc alignment fault at {pc:#x}")]
    PcAlignment {
        /// The misaligned program counter.
        pc: u64,
    },
    /// The instruction fetch failed translation → Instruction Abort.
    #[error("instruction abort: {0}")]
    InstructionAbort(TranslationFault),
    /// The fetch translated to a physical address without backing → synchronous
    /// external abort on instruction fetch.
    #[error("instruction fetch bus error at pc {pc:#x}: {source}")]
    FetchBus {
        /// Address of the instruction.
        pc: u64,
        /// The memory error.
        source: MemError,
    },
}

impl From<ExecError> for ritsu_core::Error {
    fn from(e: ExecError) -> Self {
        ritsu_core::Error::Guest(e.to_string())
    }
}

/// Executes one instruction with the stage-1 MMU off (identity translation).
///
/// # Errors
///
/// See [`ExecError`]; on error the vCPU state is unchanged.
pub fn step(cpu: &mut CpuState, mem: &impl GuestMemory) -> Result<StepOutcome, ExecError> {
    step_with(cpu, mem, &mut IdentityTranslate::default())
}

/// Executes one instruction, fetching through `mmu`.
///
/// # Errors
///
/// See [`ExecError`]; on error the vCPU state is unchanged.
pub fn step_with(
    cpu: &mut CpuState,
    mem: &impl GuestMemory,
    mmu: &mut impl Translate,
) -> Result<StepOutcome, ExecError> {
    let word = fetch(cpu, mem, mmu)?;
    execute(cpu, decode(word))
}

/// Fetches the instruction word at `PC`.
///
/// # Errors
///
/// [`ExecError::PcAlignment`], [`ExecError::InstructionAbort`] or
/// [`ExecError::FetchBus`].
pub fn fetch(
    cpu: &CpuState,
    mem: &impl GuestMemory,
    mmu: &mut impl Translate,
) -> Result<u32, ExecError> {
    let pc = cpu.pc;
    if pc & 3 != 0 {
        return Err(ExecError::PcAlignment { pc });
    }
    let privilege = match cpu.el {
        ExceptionLevel::El0 => Privilege::Unprivileged,
        ExceptionLevel::El1 => Privilege::Privileged,
    };
    let access = Access::new(AccessKind::Execute, privilege);
    let pa = mmu
        .translate(mem, GuestVirtAddr(pc), access)
        .map_err(ExecError::InstructionAbort)?;
    mem.read_u32(pa)
        .map_err(|source| ExecError::FetchBus { pc, source })
}

/// Executes an already decoded instruction located at `cpu.pc`.
///
/// # Errors
///
/// [`ExecError::Undefined`] for [`Inst::Unknown`]; the state is then unchanged.
pub fn execute(cpu: &mut CpuState, inst: Inst) -> Result<StepOutcome, ExecError> {
    let mut backend = Concrete::new(cpu);
    sem::lower(&mut backend, inst)?;
    Ok(backend.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm;
    use crate::cpu::{GpReg, Nzcv};
    use crate::decode::Cond;
    use ritsu_core::GuestPhysAddr;
    use ritsu_mem::RamRegion;
    use ritsu_mem::mmu::FaultKind;

    const BASE: u64 = 0x4000_0000;

    fn load(words: &[u32]) -> RamRegion {
        let mut ram = RamRegion::new(GuestPhysAddr(BASE), 0x1_0000);
        for (i, w) in words.iter().enumerate() {
            ram.write_u32(GuestPhysAddr(BASE + 4 * i as u64), *w)
                .unwrap();
        }
        ram
    }

    /// Runs until `PC == stop` or `limit` steps; returns the number of steps.
    fn run_until(cpu: &mut CpuState, ram: &RamRegion, stop: u64, limit: usize) -> usize {
        for n in 0..limit {
            if cpu.pc == stop {
                return n;
            }
            step(cpu, ram).unwrap();
        }
        panic!(
            "did not reach {stop:#x} within {limit} steps (pc = {:#x})",
            cpu.pc
        );
    }

    fn x(n: u8) -> GpReg {
        GpReg::x(n).unwrap()
    }

    // Short-hands for test programs.
    fn movz(rd: u32, imm: u32) -> u32 {
        asm::move_wide(true, 0b10, 0, imm, rd)
    }
    fn addi(rd: u32, rn: u32, imm: u32) -> u32 {
        asm::add_sub_imm(true, false, false, false, imm, rn, rd)
    }
    fn subsi(rd: u32, rn: u32, imm: u32) -> u32 {
        asm::add_sub_imm(true, true, true, false, imm, rn, rd)
    }

    #[test]
    fn countdown_loop_runs_to_completion() {
        // 0x00 movz x0, #10
        // 0x04 movz x1, #0
        // 0x08 loop: add x1, x1, #3
        // 0x0c       subs x0, x0, #1
        // 0x10       b.ne loop
        // 0x14 end:
        let prog = [
            movz(0, 10),
            movz(1, 0),
            addi(1, 1, 3),
            subsi(0, 0, 1),
            asm::b_cond(Cond::Ne, -8),
            asm::NOP,
        ];
        assert_eq!(prog[2], 0x9100_0c21);
        assert_eq!(prog[3], 0xf100_0400);
        assert_eq!(prog[4], 0x54ff_ffc1);
        let ram = load(&prog);
        let mut cpu = CpuState::new(BASE);
        let steps = run_until(&mut cpu, &ram, BASE + 0x14, 1000);
        assert_eq!(steps, 2 + 10 * 3);
        assert_eq!(cpu.read(x(0)), 0);
        assert_eq!(cpu.read(x(1)), 30);
        // Last SUBS was 1 - 1: Z = 1, C = 1 (no borrow).
        assert_eq!(cpu.nzcv, Nzcv::new(false, true, true, false));
    }

    #[test]
    fn call_and_return() {
        // 0x00 bl f           ; f at 0x10
        // 0x04 add x2, x2, #1
        // 0x08 b end          ; end at 0x18
        // 0x0c nop
        // 0x10 f: movz x2, #41
        // 0x14    ret
        // 0x18 end:
        let prog = [
            asm::b(true, 0x10),
            addi(2, 2, 1),
            asm::b(false, 0x10),
            asm::NOP,
            movz(2, 41),
            asm::br(2, 30),
        ];
        let ram = load(&prog);
        let mut cpu = CpuState::new(BASE);
        run_until(&mut cpu, &ram, BASE + 0x18, 100);
        assert_eq!(cpu.read(x(2)), 42);
        assert_eq!(cpu.read(x(30)), BASE + 4);
    }

    #[test]
    fn blr_x30_jumps_to_old_lr() {
        let ram = load(&[asm::br(1, 30)]);
        let mut cpu = CpuState::new(BASE);
        cpu.write(x(30), 0x1234_5678);
        assert_eq!(
            step(&mut cpu, &ram),
            Ok(StepOutcome::Branch { taken: true })
        );
        assert_eq!(cpu.pc, 0x1234_5678);
        assert_eq!(cpu.read(x(30)), BASE + 4);
    }

    #[test]
    fn br_to_xzr_goes_to_zero() {
        let ram = load(&[asm::br(0, 31)]);
        let mut cpu = CpuState::new(BASE);
        step(&mut cpu, &ram).unwrap();
        assert_eq!(cpu.pc, 0);
    }

    #[test]
    fn cbz_32_bit_tests_only_low_word() {
        // cbz w0, +8 ; cbnz x0, +8 (both taken when x0 = 1 << 32)
        let ram = load(&[
            asm::cb(false, false, 8, 0),
            asm::NOP,
            asm::cb(true, true, 8, 0),
        ]);
        let mut cpu = CpuState::new(BASE);
        cpu.write(x(0), 1 << 32);
        assert_eq!(
            step(&mut cpu, &ram),
            Ok(StepOutcome::Branch { taken: true })
        );
        assert_eq!(cpu.pc, BASE + 8);
        assert_eq!(
            step(&mut cpu, &ram),
            Ok(StepOutcome::Branch { taken: true })
        );
        assert_eq!(cpu.pc, BASE + 16);

        // cbz x0 not taken.
        let ram = load(&[asm::cb(true, false, 8, 0)]);
        let mut cpu = CpuState::new(BASE);
        cpu.write(x(0), 1 << 32);
        assert_eq!(
            step(&mut cpu, &ram),
            Ok(StepOutcome::Branch { taken: false })
        );
        assert_eq!(cpu.pc, BASE + 4);
    }

    #[test]
    fn move_wide_semantics() {
        let prog = [
            // movz x3, #0xbeef, lsl #48
            asm::move_wide(true, 0b10, 3, 0xbeef, 3),
            // movk x3, #0x1234, lsl #16
            asm::move_wide(true, 0b11, 1, 0x1234, 3),
            // movn x4, #0 → -1
            asm::move_wide(true, 0b00, 0, 0, 4),
            // movn w5, #0 → 0x0000_0000_ffff_ffff
            asm::move_wide(false, 0b00, 0, 0, 5),
            // movk w6, #0xaaaa, lsl #16 on x6 = all ones → upper 32 cleared
            asm::move_wide(false, 0b11, 1, 0xaaaa, 6),
            // movz xzr, #7 → discarded
            asm::move_wide(true, 0b10, 0, 7, 31),
        ];
        let ram = load(&prog);
        let mut cpu = CpuState::new(BASE);
        cpu.write(x(6), u64::MAX);
        run_until(&mut cpu, &ram, BASE + 4 * prog.len() as u64, 100);
        assert_eq!(cpu.read(x(3)), 0xbeef_0000_1234_0000);
        assert_eq!(cpu.read(x(4)), u64::MAX);
        assert_eq!(cpu.read(x(5)), 0xffff_ffff);
        assert_eq!(cpu.read(x(6)), 0xaaaa_ffff);
        assert_eq!(cpu.read(GpReg::Zr), 0);
    }

    #[test]
    fn reg31_sp_vs_zr_in_execution() {
        let prog = [
            // sub sp, sp, #16
            0xd100_43ff,
            // mov x0, sp
            0x9100_03e0,
            // cmp sp, #0x10 (subs xzr, sp, #16): must not touch SP
            asm::add_sub_imm(true, true, true, false, 16, 31, 31),
            // add w1, wsp, #0 → reads low 32 bits of SP
            asm::add_sub_imm(false, false, false, false, 0, 31, 1),
            // add wsp, w1, #1 → 32-bit write to SP zero-extends
            asm::add_sub_imm(false, false, false, false, 1, 1, 31),
        ];
        let ram = load(&prog);
        let mut cpu = CpuState::new(BASE);
        cpu.sp_el1 = 0x1_0000_0110;
        cpu.write(x(0), 7);
        for _ in 0..3 {
            step(&mut cpu, &ram).unwrap();
        }
        assert_eq!(cpu.sp_el1, 0x1_0000_0100);
        assert_eq!(cpu.read(x(0)), 0x1_0000_0100);
        // 0x1_0000_0100 - 16 > 0: no borrow, not zero.
        assert_eq!(cpu.nzcv, Nzcv::new(false, false, true, false));
        step(&mut cpu, &ram).unwrap();
        assert_eq!(cpu.read(x(1)), 0x100);
        step(&mut cpu, &ram).unwrap();
        assert_eq!(cpu.sp_el1, 0x101);
        assert_eq!(cpu.sp_el0, 0);
    }

    #[test]
    fn add_imm_lsl12() {
        // adds x0, x1, #1, lsl #12
        let ram = load(&[0xb140_0420]);
        let mut cpu = CpuState::new(BASE);
        cpu.write(x(1), 5);
        step(&mut cpu, &ram).unwrap();
        assert_eq!(cpu.read(x(0)), 0x1005);
    }

    /// Executes a single ADDS/SUBS (immediate) and returns (result, flags).
    fn flags_of(sf: bool, sub: bool, a: u64, imm: u32) -> (u64, Nzcv) {
        let ram = load(&[asm::add_sub_imm(sf, sub, true, false, imm, 1, 0)]);
        let mut cpu = CpuState::new(BASE);
        cpu.write(x(1), a);
        step(&mut cpu, &ram).unwrap();
        (cpu.read(x(0)), cpu.nzcv)
    }

    #[test]
    fn flags_64_bit_edges() {
        let f = Nzcv::new;
        assert_eq!(
            flags_of(true, false, i64::MAX as u64, 1),
            (1 << 63, f(true, false, false, true))
        );
        assert_eq!(
            flags_of(true, false, u64::MAX, 1),
            (0, f(false, true, true, false))
        );
        assert_eq!(
            flags_of(true, true, 0, 1),
            (u64::MAX, f(true, false, false, false))
        );
        assert_eq!(flags_of(true, true, 1, 1), (0, f(false, true, true, false)));
        assert_eq!(
            flags_of(true, true, 1 << 63, 1),
            (i64::MAX as u64, f(false, false, true, true))
        );
        // SUBS x, #0 always sets C (x + NOT(0) + 1 carries out).
        assert_eq!(flags_of(true, true, 0, 0), (0, f(false, true, true, false)));
        // ADDS with #0 never sets C or V.
        assert_eq!(
            flags_of(true, false, u64::MAX, 0),
            (u64::MAX, f(true, false, false, false))
        );
    }

    #[test]
    fn flags_32_bit_edges_ignore_upper_bits() {
        let f = Nzcv::new;
        let junk = 0xdead_beef_0000_0000u64;
        assert_eq!(
            flags_of(false, false, junk | 0x7fff_ffff, 1),
            (0x8000_0000, f(true, false, false, true))
        );
        assert_eq!(
            flags_of(false, false, junk | 0xffff_ffff, 1),
            (0, f(false, true, true, false))
        );
        assert_eq!(
            flags_of(false, true, junk, 1),
            (0xffff_ffff, f(true, false, false, false))
        );
        assert_eq!(
            flags_of(false, true, junk | 0x8000_0000, 1),
            (0x7fff_ffff, f(false, false, true, true))
        );
        // 64-bit view of the same input would not be zero; 32-bit view is.
        assert_eq!(
            flags_of(false, true, junk | 5, 5),
            (0, f(false, true, true, false))
        );
    }

    #[test]
    fn non_flag_setting_add_keeps_nzcv() {
        let ram = load(&[addi(0, 1, 1)]);
        let mut cpu = CpuState::new(BASE);
        cpu.nzcv = Nzcv::new(true, true, true, true);
        cpu.write(x(1), u64::MAX);
        step(&mut cpu, &ram).unwrap();
        assert_eq!(cpu.read(x(0)), 0);
        assert_eq!(cpu.nzcv, Nzcv::new(true, true, true, true));
    }

    #[test]
    fn condition_code_table() {
        // Independent table straight from the Arm ARM condition code descriptions.
        let expected = |c: Cond, n: bool, z: bool, cf: bool, v: bool| -> bool {
            match c {
                Cond::Eq => z,
                Cond::Ne => !z,
                Cond::Cs => cf,
                Cond::Cc => !cf,
                Cond::Mi => n,
                Cond::Pl => !n,
                Cond::Vs => v,
                Cond::Vc => !v,
                Cond::Hi => cf && !z,
                Cond::Ls => !cf || z,
                Cond::Ge => n == v,
                Cond::Lt => n != v,
                Cond::Gt => !z && n == v,
                Cond::Le => z || n != v,
                Cond::Al | Cond::Nv => true,
            }
        };
        for bits in 0..16u32 {
            let (n, z, c, v) = (bits & 8 != 0, bits & 4 != 0, bits & 2 != 0, bits & 1 != 0);
            let flags = Nzcv::new(n, z, c, v);
            for (i, cond) in Cond::ALL.into_iter().enumerate() {
                assert_eq!(cond.bits() as usize, i);
                assert_eq!(Cond::from_bits(cond.bits()), cond);
                assert_eq!(
                    cond.holds(flags),
                    expected(cond, n, z, c, v),
                    "{cond:?} {flags:?}"
                );
                // And through the interpreter: B.cond +8 at BASE.
                let ram = load(&[asm::b_cond(cond, 8)]);
                let mut cpu = CpuState::new(BASE);
                cpu.nzcv = flags;
                let taken = expected(cond, n, z, c, v);
                assert_eq!(step(&mut cpu, &ram), Ok(StepOutcome::Branch { taken }));
                assert_eq!(cpu.pc, if taken { BASE + 8 } else { BASE + 4 });
            }
        }
    }

    #[test]
    fn signed_compare_after_subs() {
        // cmp x0, #1 ; b.lt → taken for x0 = -5, not taken for x0 = 1 (GE).
        for (val, taken) in [(-5i64 as u64, true), (1, false), (i64::MIN as u64, true)] {
            let ram = load(&[subsi(31, 0, 1), asm::b_cond(Cond::Lt, 8)]);
            let mut cpu = CpuState::new(BASE);
            cpu.write(x(0), val);
            step(&mut cpu, &ram).unwrap();
            assert_eq!(step(&mut cpu, &ram), Ok(StepOutcome::Branch { taken }));
        }
    }

    #[test]
    fn unknown_instruction_is_undefined_and_state_unchanged() {
        let ram = load(&[0x0000_0000]);
        let mut cpu = CpuState::new(BASE);
        let before = cpu.clone();
        assert_eq!(
            step(&mut cpu, &ram),
            Err(ExecError::Undefined { pc: BASE, word: 0 })
        );
        assert_eq!(cpu, before);
    }

    #[test]
    fn misaligned_pc_faults() {
        let ram = load(&[asm::NOP]);
        let mut cpu = CpuState::new(BASE + 2);
        assert_eq!(
            step(&mut cpu, &ram),
            Err(ExecError::PcAlignment { pc: BASE + 2 })
        );
    }

    #[test]
    fn fetch_outside_ram_is_bus_error() {
        let ram = load(&[asm::b(false, -8)]);
        let mut cpu = CpuState::new(BASE);
        step(&mut cpu, &ram).unwrap();
        assert_eq!(cpu.pc, BASE - 8);
        assert!(
            matches!(step(&mut cpu, &ram), Err(ExecError::FetchBus { pc, .. }) if pc == BASE - 8)
        );
    }

    #[test]
    fn fetch_beyond_pa_size_is_instruction_abort() {
        let ram = load(&[]);
        let mut cpu = CpuState::new(1 << 60);
        match step(&mut cpu, &ram) {
            Err(ExecError::InstructionAbort(f)) => {
                assert_eq!(f.kind, FaultKind::AddressSize { level: 0 });
                assert_eq!(f.access.kind, AccessKind::Execute);
                assert_eq!(f.access.privilege, Privilege::Privileged);
            }
            other => panic!("unexpected {other:?}"),
        }
        cpu.el = ExceptionLevel::El0;
        assert!(matches!(
            step(&mut cpu, &ram),
            Err(ExecError::InstructionAbort(f)) if f.access.privilege == Privilege::Unprivileged
        ));
    }

    #[test]
    fn pc_wraps_without_panic() {
        // b .-4 at address 0 → PC wraps to 0xffff_ffff_ffff_fffc.
        let mut ram = RamRegion::new(GuestPhysAddr(0), 0x10);
        ram.write_u32(GuestPhysAddr(0), asm::b(false, -4)).unwrap();
        let mut cpu = CpuState::new(0);
        step(&mut cpu, &ram).unwrap();
        assert_eq!(cpu.pc, 0xffff_ffff_ffff_fffc);
        assert!(step(&mut cpu, &ram).is_err());
    }

    #[test]
    fn exec_error_converts_to_core_error() {
        let e: ritsu_core::Error = ExecError::PcAlignment { pc: 2 }.into();
        assert!(matches!(e, ritsu_core::Error::Guest(_)));
    }
}
