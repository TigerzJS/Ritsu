//! The harness logic of every fuzz target as an ordinary function.
//!
//! `fuzz_targets/fuzz_<x>.rs` only forwards to `harness::<x>::run`, so the same code runs under
//! libFuzzer (nightly, `cargo fuzz`) and in `cargo test` on stable, where `tests/replay.rs`
//! replays the checked-in regression inputs and the seed corpus (docs/process/testing.md §5.3).
//!
//! A new target is registered in [`TARGETS`]; `tests/replay.rs` fails if this table, the
//! `fuzz_targets/` directory and the `[[bin]]` list in `Cargo.toml` disagree.

pub mod aarch64_decode;
pub mod gpu_host_session;
pub mod gpu_proto_decode;
pub mod gpu_proto_hello;
pub mod gpu_proto_reply;
pub mod mmio_bus;
pub mod t0_step;
pub mod virtqueue_walk;

/// A harness entry point: panics when an invariant is violated.
pub type Harness = fn(&[u8]);

/// All Rust fuzz targets: libFuzzer target name (also the directory name below `corpus/` and
/// `regressions/`) and its harness.
pub const TARGETS: &[(&str, Harness)] = &[
    ("fuzz_aarch64_decode", aarch64_decode::run),
    ("fuzz_gpu_host_session", gpu_host_session::run),
    ("fuzz_gpu_proto_decode", gpu_proto_decode::run),
    ("fuzz_gpu_proto_hello", gpu_proto_hello::run),
    ("fuzz_gpu_proto_reply", gpu_proto_reply::run),
    ("fuzz_mmio_bus", mmio_bus::run),
    ("fuzz_t0_step", t0_step::run),
    ("fuzz_virtqueue_walk", virtqueue_walk::run),
];

/// The harness registered for `target`, if any.
#[must_use]
pub fn find(target: &str) -> Option<Harness> {
    TARGETS.iter().find(|(n, _)| *n == target).map(|(_, h)| *h)
}
