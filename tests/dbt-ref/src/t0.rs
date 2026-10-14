//! Runs vectors through T0 (`ritsu_dbt::interp`) and produces [`Outcome`]s in the same shape as
//! the reference runner.
//!
//! The memory image matches what the runner sets up: a code region (`REGION_SIZE` bytes at
//! `code_base - PROLOGUE_BYTES`; the prologue area is zero here, the sentinels follow the test
//! words) and a data region at `mem_base` holding the window followed by zeros. Generated vectors
//! never read the code region, so the prologue difference is not observable. Only the code region
//! is executable; a branch anywhere else ends the vector with `SIGSEGV`, as on the reference.

use crate::format::{
    Config, Outcome, PAD_WORDS, PROLOGUE_BYTES, REGION_SIZE, SENTINEL, State, Vector, sig,
};
use crate::rng::fill_window;
use ritsu_core::GuestPhysAddr;
use ritsu_dbt::interp::{ExecError, step_mut};
use ritsu_dbt::mem::PlainRam;
use ritsu_dbt::{CpuState, ExceptionLevel, cpu::Nzcv};
use ritsu_mem::{GuestMemory, MemError};

/// Sparse memory made of a few fixed regions.
#[derive(Debug, Default)]
pub struct Regions {
    regions: Vec<(u64, Vec<u8>)>,
}

impl Regions {
    fn find(&self, addr: u64, len: usize) -> Option<(usize, usize)> {
        for (i, (base, data)) in self.regions.iter().enumerate() {
            let end = base + data.len() as u64;
            if addr >= *base && addr.checked_add(len as u64)? <= end {
                return Some((i, (addr - base) as usize));
            }
        }
        None
    }
}

// Marker: all regions are plain RAM (reads have no side effects).
impl PlainRam for Regions {}

impl GuestMemory for Regions {
    fn read(&self, addr: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemError> {
        let (i, off) = self.find(addr.get(), buf.len()).ok_or(MemError::Unmapped {
            addr,
            len: buf.len(),
        })?;
        buf.copy_from_slice(&self.regions[i].1[off..off + buf.len()]);
        Ok(())
    }

    fn write(&mut self, addr: GuestPhysAddr, data: &[u8]) -> Result<(), MemError> {
        let (i, off) = self
            .find(addr.get(), data.len())
            .ok_or(MemError::Unmapped {
                addr,
                len: data.len(),
            })?;
        self.regions[i].1[off..off + data.len()].copy_from_slice(data);
        Ok(())
    }
}

/// Maps a T0 error to the signal Linux would deliver for the same condition.
fn signal_of(e: &ExecError) -> u16 {
    match e {
        ExecError::Undefined { .. } => sig::ILL,
        ExecError::Breakpoint { .. } => sig::TRAP,
        ExecError::PcAlignment { .. } | ExecError::SpAlignment { .. } => sig::BUS,
        ExecError::InstructionAbort(_)
        | ExecError::FetchBus { .. }
        | ExecError::DataAbort { .. } => sig::SEGV,
        // `SVC` would be a system call on Linux and is never generated; everything else is an
        // instruction T0 cannot execute yet.
        _ => sig::T0_UNIMPLEMENTED,
    }
}

/// Executes one vector in T0.
#[must_use]
pub fn run_vector(cfg: &Config, v: &Vector) -> Outcome {
    let window = fill_window(cfg.mem_len as usize, v.mem_seed);

    let mut code = vec![0u8; REGION_SIZE as usize];
    let at = PROLOGUE_BYTES as usize;
    for (i, w) in v.words.iter().enumerate() {
        code[at + 4 * i..at + 4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    for i in 0..PAD_WORDS as usize {
        let o = at + 4 * (v.words.len() + i);
        code[o..o + 4].copy_from_slice(&SENTINEL.to_le_bytes());
    }
    let mut data = vec![0u8; REGION_SIZE as usize];
    data[..window.len()].copy_from_slice(&window);
    let mut mem = Regions {
        regions: vec![(cfg.code_base - PROLOGUE_BYTES, code), (cfg.mem_base, data)],
    };

    let mut cpu = CpuState::new(cfg.code_base);
    cpu.el = ExceptionLevel::El0;
    cpu.x = v.x;
    cpu.set_sp(v.sp);
    cpu.nzcv = Nzcv::from_sysreg(v.nzcv);

    let exec = cfg.code_base - PROLOGUE_BYTES..cfg.code_base - PROLOGUE_BYTES + REGION_SIZE;
    let code_end = cfg.code_end(v.words.len());
    let sentinel_end = code_end + 4 * PAD_WORDS;
    let limit = v.words.len() as u64 + PAD_WORDS + 64;
    let mut signal = sig::T0_STEP_LIMIT;
    for _ in 0..limit {
        if (code_end..sentinel_end).contains(&cpu.pc) {
            signal = sig::TRAP; // the BRK sentinel
            break;
        }
        // The runner maps only the code region executable (the data window is read/write), but
        // T0 has no page permissions with the MMU off: model them here. A misaligned PC is left
        // to T0 (alignment fault, SIGBUS before any permission check).
        if cpu.pc.is_multiple_of(4) && !exec.contains(&cpu.pc) {
            signal = sig::SEGV;
            break;
        }
        if let Err(e) = step_mut(&mut cpu, &mut mem) {
            signal = signal_of(&e);
            break;
        }
    }

    let final_window = &mem.regions[1].1[..window.len()];
    Outcome {
        signal,
        si_code: 0,
        state: State {
            x: cpu.x,
            sp: cpu.sp(),
            nzcv: cpu.nzcv.to_sysreg() & 0xf000_0000,
            pc: cpu.pc,
        },
        mem: (final_window != window.as_slice()).then(|| final_window.to_vec()),
    }
}

/// Executes all vectors.
#[must_use]
pub fn run_all(cfg: &Config, vectors: &[Vector]) -> Vec<Outcome> {
    vectors.iter().map(|v| run_vector(cfg, v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handmade;

    /// T0 against the hand-computed expectations (independent of any recording).
    #[test]
    fn t0_reproduces_the_handmade_cases() {
        let cfg = crate::generator::default_config(0);
        let mut failures = Vec::new();
        for case in handmade::cases() {
            let got = run_vector(&cfg, &case.to_vector(&cfg, 0));
            let want = case.expected(&cfg);
            if got.signal != want.signal || got.state != want.state || got.mem != want.mem {
                failures.push(format!("{}: want {want:?}\n got {got:?}", case.name));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
