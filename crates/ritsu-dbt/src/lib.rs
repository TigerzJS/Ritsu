//! AArch64 → x86-64 dynamic binary translator.
//!
//! Full-system guest (EL0 + EL1, no EL2/EL3, vmapple platform), arm64e. Design:
//! `docs/subsystems/dbt.md`, `docs/adr/0006-dbt-tiered-design.md`.
//!
//! Status (WP-DBT-02):
//!
//! * [`cpu`] — architectural vCPU state (`#[repr(C)]`, future JIT ABI).
//! * [`mod@decode`] — A64 decoder for the integer base ISA (data processing, branches,
//!   hints, `SVC/BRK`, four system registers, all integer loads/stores).
//! * [`mem`] — the narrow memory interface of T0 (`DataBus`).
//! * [`sem`] — the semantic layer: trait [`Sem`], one description per instruction class
//!   ([`sem::lower`]), Arm pseudocode primitives and the T0 backend
//!   [`sem::concrete::Concrete`] (ADR-0006: one semantic source).
//! * [`interp`] — T0 reference interpreter: fetch, decode, dispatch to [`sem::lower`].
//! * [`ir`] — T2 IR types, the IR backend [`ir::Builder`] (with the static rule check)
//!   and a verifier (no codegen).
//! * [`tier`] — interfaces for T1/T2 and the translation cache (no implementation yet).

#![warn(missing_docs)]

pub mod cpu;
pub mod decode;
pub mod interp;
pub mod ir;
pub mod mem;
pub mod sem;
pub mod tier;

#[cfg(test)]
mod asm;
#[cfg(test)]
mod t0_tests;
#[cfg(test)]
mod testutil;

pub use cpu::{CpuState, ExceptionLevel, GpReg, Nzcv, SysReg, XReg};
pub use decode::{Cond, Inst, decode};
pub use interp::{ExecError, StepOutcome, step, step_bus, step_mut, step_with};
pub use sem::{Ctx, Sem, Ty};
