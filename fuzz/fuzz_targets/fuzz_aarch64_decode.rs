//! `ritsu_dbt::decode` on arbitrary 32-bit words (the input is read as little-endian words).
//!
//! The harness logic lives in `ritsu_fuzz::harness::aarch64_decode` (`fuzz/src/harness/aarch64_decode.rs`) so that
//! `cargo test` can replay `fuzz/regressions/fuzz_aarch64_decode/` on stable; this file is only the libFuzzer entry.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ritsu_fuzz::harness::aarch64_decode::run(data));
