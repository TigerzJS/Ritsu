//! T0 interpreter workloads (Q-PERF-1): guest programs run through `ritsu_dbt::interp::step`.
//!
//! Both programs use only the M0 subset and terminate when the PC reaches a marker instruction;
//! the loop around `step` is the same one a plain T0 run loop needs (fetch, decode and execute
//! happen inside `step`). Expected final states and instruction counts are derived by hand in
//! the tests, independent of the interpreter.

use crate::a64::{Asm, Cond, to_bytes};
use ritsu_core::GuestPhysAddr;
use ritsu_dbt::{CpuState, step};
use ritsu_mem::{GuestMemory, RamRegion};

/// Guest physical base address of the program (and of the RAM region).
pub const BASE: u64 = 0x4000_0000;
const RAM_LEN: usize = 0x1_0000;

/// An assembled program in guest RAM.
#[derive(Debug)]
pub struct Program {
    ram: RamRegion,
    halt_pc: u64,
    /// The program words (for tests).
    pub words: Vec<u32>,
}

impl Program {
    fn new(words: Vec<u32>, halt_off: u64) -> Self {
        let mut ram = RamRegion::new(GuestPhysAddr(BASE), RAM_LEN);
        ram.write(GuestPhysAddr(BASE), &to_bytes(&words))
            .expect("program fits RAM");
        Self {
            ram,
            halt_pc: BASE + halt_off,
            words,
        }
    }

    /// Runs from the reset state until the PC reaches the halt marker; returns the final state
    /// and the number of executed instructions. Panics if the guest faults or does not halt
    /// within `step_limit` instructions (the programs are fixed, so that is a bug).
    #[must_use]
    pub fn run(&self, step_limit: u64) -> (CpuState, u64) {
        let mut cpu = CpuState::new(BASE);
        let mut executed = 0u64;
        while cpu.pc != self.halt_pc {
            step(&mut cpu, &self.ram).expect("benchmark program faulted");
            executed += 1;
            assert!(executed <= step_limit, "benchmark program did not halt");
        }
        (cpu, executed)
    }
}

/// `x0 = n; do { x0 -= 1 } while (x0 != 0)`: two instructions per iteration, one taken branch.
///
/// Executed instructions: `2 + 2n` (MOVZ, MOVK, then SUBS + B.NE per iteration). `n >= 1`.
#[must_use]
pub fn countdown(n: u32) -> Program {
    assert!(n >= 1);
    let mut a = Asm::new();
    a.mov32(0, n);
    let top = a.label();
    a.bind(top);
    a.subs_imm(0, 0, 1);
    a.b_cond(Cond::Ne, top);
    let halt = a.here();
    a.nop();
    Program::new(a.finish(), halt)
}

/// Number of executed instructions of [`countdown`]`(n)`.
#[must_use]
pub const fn countdown_instructions(n: u32) -> u64 {
    2 + 2 * n as u64
}

/// Iteration threshold of the data-dependent branch in [`mix`]: `x1 = 7 * i` is below 100 for
/// `i <= 14`.
const MIX_LO_ITERATIONS: u64 = 14;

/// A mix of every M0 instruction class per iteration (16 instructions, 17 once `x1 >= 100`):
/// MOVZ/MOVK, ADD/SUB immediate (with `LSL #12`), flag-setting compare, `B.LO` (taken for the
/// first 14 iterations, then not), `CBZ` (taken) and `CBNZ` (not taken), `BL`/`RET`, `B`, `BR`
/// and the loop branch.
///
/// Final state after `n` iterations: `x0 = 0`, `x1 = 7n`, `x2 = 5 - n` (mod 2^64), `x4 =
/// 0x5678_1234`, `x5 = 7n + 4096`, `x6 = n` (calls), `x30` = address after the `BL`.
#[must_use]
pub fn mix(n: u32) -> Program {
    assert!(n >= 1);
    // The address of the BR target is loaded with a fixed-size MOVZ/MOVK pair, so the layout
    // does not depend on its value: assemble once to learn the offset, then for real.
    let (_, ind_off, _) = build_mix(n, 0);
    let ind_addr = BASE + ind_off;
    let (words, _, halt_off) = build_mix(n, u32::try_from(ind_addr).expect("32-bit address"));
    Program::new(words, halt_off)
}

fn build_mix(n: u32, ind_addr: u32) -> (Vec<u32>, u64, u64) {
    let mut a = Asm::new();
    a.mov32(0, n); //            x0 = iteration counter
    a.movz(1, 0, 0); //          x1 = accumulator
    a.movz(2, 5, 0); //          x2 = 5, counts down
    a.movz(6, 0, 0); //          x6 = number of calls
    a.mov32(9, ind_addr); //     x9 = address of `ind`
    let (lo_taken, z_taken, nz_skip, next, ind, func) = (
        a.label(),
        a.label(),
        a.label(),
        a.label(),
        a.label(),
        a.label(),
    );
    let top = a.label();
    a.bind(top);
    a.add_imm(1, 1, 7, false);
    a.sub_imm(2, 2, 1);
    a.movz(4, 0x1234, 0);
    a.movk(4, 0x5678, 1);
    a.add_imm(5, 1, 1, true);
    a.subs_imm(31, 1, 100); // cmp x1, #100
    a.b_cond(Cond::Lo, lo_taken);
    a.nop(); //                  executed once x1 >= 100
    a.bind(lo_taken);
    a.cbz(3, z_taken); //        x3 == 0: always taken
    a.nop(); //                  never executed
    a.bind(z_taken);
    a.cbnz(3, nz_skip); //       never taken
    a.bl(func);
    a.bind(nz_skip);
    a.b(next);
    a.nop(); //                  never executed
    a.bind(next);
    a.br(9);
    a.nop(); //                  never executed
    a.bind(ind);
    a.subs_imm(0, 0, 1);
    a.b_cond(Cond::Ne, top);
    let halt = a.here();
    a.nop();
    a.bind(func);
    a.add_imm(6, 6, 1, false);
    a.ret();
    let ind_off = a.offset_of(ind);
    (a.finish(), ind_off, halt)
}

/// Number of executed instructions of [`mix`]`(n)`: 7 setup, 16 per iteration, plus one `NOP`
/// per iteration after the first 14 (the `B.LO` is then not taken).
#[must_use]
pub const fn mix_instructions(n: u32) -> u64 {
    let n = n as u64;
    7 + 16 * n + n.saturating_sub(MIX_LO_ITERATIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ritsu_dbt::{Inst, Nzcv, decode};

    #[test]
    fn countdown_final_state_and_count() {
        for n in [1u32, 2, 1000, 70_000] {
            let p = countdown(n);
            let (cpu, executed) = p.run(10 * countdown_instructions(n));
            assert_eq!(executed, countdown_instructions(n), "n = {n}");
            assert_eq!(cpu.x[0], 0);
            assert_eq!(cpu.pc, BASE + 4 * 4);
            // The last SUBS produced 0: Z and C (no borrow) set, N and V clear.
            assert_eq!(cpu.nzcv, Nzcv::new(false, true, true, false), "n = {n}");
        }
    }

    #[test]
    fn mix_final_state_and_count() {
        // n = 3 stays below the data-dependent threshold, 14 sits on it, 100 is above it.
        for n in [1u32, 3, 14, 15, 100] {
            let p = mix(n);
            let (cpu, executed) = p.run(10 * mix_instructions(n));
            assert_eq!(executed, mix_instructions(n), "n = {n}");
            let n64 = u64::from(n);
            assert_eq!(cpu.x[0], 0, "n = {n}");
            assert_eq!(cpu.x[1], 7 * n64);
            assert_eq!(cpu.x[2], 5u64.wrapping_sub(n64));
            assert_eq!(cpu.x[3], 0);
            assert_eq!(cpu.x[4], 0x5678_1234);
            assert_eq!(cpu.x[5], 7 * n64 + 4096);
            assert_eq!(cpu.x[6], n64, "one call per iteration");
            assert_eq!(cpu.x[30], BASE + 4 * 19, "return address after BL");
        }
    }

    #[test]
    fn mix_hand_counts_for_small_n() {
        // n = 1: 7 + 16 = 23; n = 15: 7 + 240 + 1 = 248.
        assert_eq!(mix_instructions(1), 23);
        assert_eq!(mix_instructions(14), 7 + 224);
        assert_eq!(mix_instructions(15), 248);
    }

    #[test]
    fn every_word_is_a_known_instruction() {
        for p in [countdown(5), mix(5)] {
            for (i, &w) in p.words.iter().enumerate() {
                assert!(
                    !matches!(decode(w), Inst::Unknown(_)),
                    "word {i} = {w:#010x} does not decode"
                );
            }
        }
    }

    #[test]
    fn mix_covers_the_expected_instruction_classes() {
        let insts: Vec<Inst> = mix(5).words.iter().map(|&w| decode(w)).collect();
        let has = |f: fn(&Inst) -> bool| insts.iter().any(f);
        assert!(has(|i| matches!(i, Inst::MoveWide { .. })));
        assert!(has(|i| matches!(i, Inst::AddSubImm { .. })));
        assert!(has(|i| matches!(i, Inst::BCond { .. })));
        assert!(has(|i| matches!(
            i,
            Inst::CompareBranch { nonzero: false, .. }
        )));
        assert!(has(|i| matches!(
            i,
            Inst::CompareBranch { nonzero: true, .. }
        )));
        assert!(has(|i| matches!(i, Inst::B { .. })));
        assert!(has(|i| matches!(i, Inst::Bl { .. })));
        assert!(has(|i| matches!(i, Inst::Br { .. })));
        assert!(has(|i| matches!(i, Inst::Ret { .. })));
        assert!(has(|i| matches!(i, Inst::Nop)));
    }
}
