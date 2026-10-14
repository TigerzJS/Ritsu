//! AArch64 → x86-64 dynamic binary translator.
//!
//! Full-system guest (EL0 + EL1, no EL2/EL3, vmapple platform), arm64e. Design:
//! `docs/subsystems/dbt.md`, `docs/adr/0006-dbt-tiered-design.md`.
//!
//! Status (WP-DBT-01):
//!
//! * [`cpu`] — architectural vCPU state (`#[repr(C)]`, future JIT ABI).
//! * [`mod@decode`] — A64 decoder for an initial subset.
//! * [`sem`] — the semantic layer: trait [`Sem`], one description per instruction class
//!   ([`sem::lower`]), Arm pseudocode primitives and the T0 backend
//!   [`sem::concrete::Concrete`] (ADR-0006: one semantic source).
//! * [`interp`] — T0 reference interpreter: fetch, decode, dispatch to [`sem::lower`].
//! * [`ir`] — T2 IR types, the IR backend [`ir::Builder`] and a verifier (no codegen).
//! * [`tier`] — interfaces for T1/T2 and the translation cache (no implementation yet).

#![warn(missing_docs)]

pub mod cpu;
pub mod decode;
pub mod interp;
pub mod ir;
pub mod sem;
pub mod tier;

#[cfg(test)]
mod asm;

pub use cpu::{CpuState, ExceptionLevel, GpReg, Nzcv, XReg};
pub use decode::{Cond, Inst, decode};
pub use interp::{ExecError, StepOutcome, step, step_with};
pub use sem::{Sem, Ty};
