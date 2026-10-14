//! Differential-test infrastructure for the DBT (WP-DBT-04, docs/process/testing.md section 4).
//!
//! * [`gen`] builds vectors (initial state + instruction words) from a class table, seeded.
//! * `runner/ritsu-ref-runner.c` executes them on a real AArch64 CPU or under qemu-user.
//! * [`golden`] stores recorded results compactly; [`t0`] produces the same results from the
//!   T0 interpreter; [`compare`] diffs two result sets with a reproducible report.
//!
//! See `tests/dbt-ref/README.md` for the workflow and the file formats.

pub mod compare;
pub mod enc;
pub mod format;
pub mod generator;
pub mod golden;
pub mod handmade;
pub mod rng;
pub mod set;
pub mod store;
pub mod t0;
