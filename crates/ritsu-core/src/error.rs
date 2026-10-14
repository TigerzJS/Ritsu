//! Project-wide error type.
//!
//! Subsystems define their own detailed error enums and convert into [`Error`] at crate
//! boundaries. Guest-triggerable conditions must never panic; they surface as errors.

/// Result alias using the project-wide [`Error`].
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Top-level error categories shared by all crates.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The guest did something architecturally invalid or unsupported by Ritsu
    /// (malformed descriptor, bad MMIO access, unknown command). Never a host bug.
    #[error("guest fault: {0}")]
    Guest(String),

    /// A feature exists in the architecture/API but Ritsu does not implement it yet.
    #[error("not implemented: {0}")]
    Unimplemented(&'static str),

    /// The host environment cannot satisfy a requirement (missing Vulkan extension, no memory).
    #[error("host: {0}")]
    Host(String),

    /// I/O error from the host OS.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
