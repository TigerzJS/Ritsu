//! `ritsu_gpu_host::frontend_g::GuestSession<NullBackend>`: a valid HELLO, then hostile streams.
//!
//! The harness logic lives in `ritsu_fuzz::harness::gpu_host_session` (`fuzz/src/harness/gpu_host_session.rs`) so that
//! `cargo test` can replay `fuzz/regressions/fuzz_gpu_host_session/` on stable; this file is only the libFuzzer entry.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ritsu_fuzz::harness::gpu_host_session::run(data));
