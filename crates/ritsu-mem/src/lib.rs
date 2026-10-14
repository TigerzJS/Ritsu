//! Guest physical memory, SoftMMU and TLB.
//!
//! The [`GuestMemory`] trait is the **frozen contract** between the CPU side
//! (`ritsu-dbt`) and the device side (`ritsu-virtio`, `ritsu-platform`). Changing its
//! signature requires the `architect` role and an ADR (see `CLAUDE.md` §2.3).
//!
//! Modules:
//! * [`ram`] — reference RAM backend.
//! * [`mmu`] — address translation vocabulary and the [`mmu::Translate`] trait.
//! * [`tlb`] — direct-mapped software TLB for the 16 KiB granule.

#![warn(missing_docs)]

pub mod mmu;
pub mod ram;
pub mod tlb;

pub use ram::RamRegion;

use ritsu_core::GuestPhysAddr;

/// Errors from guest physical memory accesses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MemError {
    /// The access touches an address that is not backed by RAM.
    #[error("unmapped guest physical access at {addr:?} (len {len})")]
    Unmapped {
        /// First address of the access.
        addr: GuestPhysAddr,
        /// Length of the access in bytes.
        len: usize,
    },
}

impl From<MemError> for ritsu_core::Error {
    fn from(e: MemError) -> Self {
        ritsu_core::Error::Guest(e.to_string())
    }
}

/// Byte-addressable guest physical memory.
///
/// Implementations must be bounds-checked: every access with guest-controlled
/// address/length either succeeds completely or fails with [`MemError`] without
/// partial writes. Accesses are little-endian where integer helpers are used.
pub trait GuestMemory {
    /// Copies `buf.len()` bytes starting at `addr` into `buf`.
    fn read(&self, addr: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemError>;

    /// Copies `data` into guest memory starting at `addr`.
    fn write(&mut self, addr: GuestPhysAddr, data: &[u8]) -> Result<(), MemError>;

    /// Reads a little-endian `u32`.
    fn read_u32(&self, addr: GuestPhysAddr) -> Result<u32, MemError> {
        let mut b = [0u8; 4];
        self.read(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    /// Reads a little-endian `u64`.
    fn read_u64(&self, addr: GuestPhysAddr) -> Result<u64, MemError> {
        let mut b = [0u8; 8];
        self.read(addr, &mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    /// Writes a little-endian `u32`.
    fn write_u32(&mut self, addr: GuestPhysAddr, v: u32) -> Result<(), MemError> {
        self.write(addr, &v.to_le_bytes())
    }

    /// Writes a little-endian `u64`.
    fn write_u64(&mut self, addr: GuestPhysAddr, v: u64) -> Result<(), MemError> {
        self.write(addr, &v.to_le_bytes())
    }
}
