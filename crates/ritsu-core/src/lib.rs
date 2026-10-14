//! Shared vocabulary types for Ritsu.
//!
//! This crate sits at the bottom of the dependency graph (see `docs/ARCHITECTURE.md` §4).
//! It must stay free of logic and of dependencies on other `ritsu-*` crates.

pub mod addr;
pub mod error;

pub use addr::{GuestPhysAddr, GuestVirtAddr, PAGE_SIZE_16K};
pub use error::{Error, Result};
