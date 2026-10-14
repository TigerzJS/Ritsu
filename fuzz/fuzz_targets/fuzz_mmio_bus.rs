//! `ritsu_platform::mmio::MmioBus`: random registrations and accesses.
//!
//! The harness logic lives in `ritsu_fuzz::harness::mmio_bus` (`fuzz/src/harness/mmio_bus.rs`) so that
//! `cargo test` can replay `fuzz/regressions/fuzz_mmio_bus/` on stable; this file is only the libFuzzer entry.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ritsu_fuzz::harness::mmio_bus::run(data));
