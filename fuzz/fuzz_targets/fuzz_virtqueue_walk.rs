//! Split virtqueue (`ritsu_virtio::queue::Queue`) against hostile guest memory.
//!
//! The harness logic lives in `ritsu_fuzz::harness::virtqueue_walk` (`fuzz/src/harness/virtqueue_walk.rs`) so that
//! `cargo test` can replay `fuzz/regressions/fuzz_virtqueue_walk/` on stable; this file is only the libFuzzer entry.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ritsu_fuzz::harness::virtqueue_walk::run(data));
