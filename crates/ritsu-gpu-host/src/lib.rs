//! Metal object model translated to Vulkan on the host.
//!
//! Layering (see `docs/subsystems/gpu.md` §1):
//!
//! ```text
//! frontend P (Apple paravirt, later)  ─┐
//! frontend G (ritsu-gpu-proto)  ───────┼─► model (protocol-neutral Metal objects)
//!                                      │       │ validated descriptors + GpuCommand
//!                                      │       ▼
//!                                      └─► backend::GpuBackend ─► Vulkan backends (WP-GPU-03/04)
//!                                                               └► NullBackend (tests)
//! ```
//!
//! * [`handle`]: generational handle tables for every host object.
//! * [`model`]: Metal object model types and guest-descriptor validation.
//! * [`caps`]: device capabilities and the feature set reported to the guest.
//! * [`encoder`]: encoder state machine (command order, pass/pipeline agreement) shared by
//!   frontends (record time) and backends (submit time).
//! * [`backend`]: the backend trait and the validating [`backend::NullBackend`].
//! * [`frontend_g`]: executes `ritsu-gpu-proto` streams (ADR-0005 variant G, `Proposed`).
//!
//! No `unsafe` in this crate so far (`deny`); Vulkan FFI will live in a dedicated, explicitly
//! allowed `ffi`/`vk` module with `// SAFETY:` comments (CLAUDE.md §3.3).

#![deny(unsafe_code)]

pub mod backend;
pub mod caps;
pub mod encoder;
pub mod frontend_g;
pub mod handle;
pub mod model;
