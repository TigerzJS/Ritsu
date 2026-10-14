//! Ritsu benchmark suite (docs/process/testing.md section 8, bench/README.md).
//!
//! The workloads live in this library so that `cargo test` can check them (each program and
//! stream has hand-derived expected results); `src/main.rs` runs them and writes the result
//! document. Nothing here is used by production code.

pub mod a64;
pub mod gpu;
pub mod harness;
pub mod ir;
pub mod json;
pub mod meta;
pub mod stats;
pub mod suite;
pub mod t0;
pub mod virtq;
