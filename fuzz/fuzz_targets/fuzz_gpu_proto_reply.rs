//! `ritsu_gpu_proto::decode_reply_stream` (host -> guest channel) on arbitrary bytes.
//!
//! The harness logic lives in `ritsu_fuzz::harness::gpu_proto_reply` (`fuzz/src/harness/gpu_proto_reply.rs`) so that
//! `cargo test` can replay `fuzz/regressions/fuzz_gpu_proto_reply/` on stable; this file is only the libFuzzer entry.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ritsu_fuzz::harness::gpu_proto_reply::run(data));
