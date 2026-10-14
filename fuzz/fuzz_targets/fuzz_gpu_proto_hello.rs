//! Handshake and session decoders of `ritsu-gpu-proto` on arbitrary bytes:
//!
//! The harness logic lives in `ritsu_fuzz::harness::gpu_proto_hello` (`fuzz/src/harness/gpu_proto_hello.rs`) so that
//! `cargo test` can replay `fuzz/regressions/fuzz_gpu_proto_hello/` on stable; this file is only the libFuzzer entry.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ritsu_fuzz::harness::gpu_proto_hello::run(data));
