//! T0 interpreter (`ritsu_dbt::step`) on arbitrary instruction words in RAM.
//!
//! The harness logic lives in `ritsu_fuzz::harness::t0_step` (`fuzz/src/harness/t0_step.rs`) so that
//! `cargo test` can replay `fuzz/regressions/fuzz_t0_step/` on stable; this file is only the libFuzzer entry.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ritsu_fuzz::harness::t0_step::run(data));
