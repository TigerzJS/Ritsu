//! AArch64 → x86-64 dynamic binary translator.
//!
//! Full-system guest (EL0 + EL1, no EL2/EL3, vmapple platform), arm64e. Design:
//! `docs/subsystems/dbt.md`, `docs/adr/0006-dbt-tiered-design.md`.
//!
//! Status (M0 skeleton):
//!
//! * [`cpu`] — architectural vCPU state.
//! * [`mod@decode`] — A64 decoder for an initial subset.
//! * [`sem`] — shared semantic primitives (`AddWithCarry`, …).
//! * [`interp`] — T0 reference interpreter.
//! * [`tier`] — interfaces for T1/T2 and the translation cache (no implementation yet).

#![warn(missing_docs)]

pub mod cpu;
pub mod decode;
pub mod interp;
pub mod sem;
pub mod tier;

#[cfg(test)]
mod asm;

pub use cpu::{CpuState, ExceptionLevel, GpReg, Nzcv, XReg};
pub use decode::{Cond, Inst, decode};
pub use interp::{ExecError, StepOutcome, step, step_with};
