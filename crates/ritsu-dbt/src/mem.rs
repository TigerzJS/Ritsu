//! The narrow memory interface of T0's data and instruction accesses.
//!
//! The semantic layer never touches memory directly: [`crate::sem::Sem::load`],
//! [`crate::sem::Sem::store`] and [`crate::sem::Sem::probe`] are implemented by the T0
//! backend ([`crate::sem::concrete::Concrete`]) on top of a [`DataBus`]. The trait is
//! deliberately small (translate one page, read, probe, write bytes) so that it can be
//! re-implemented on the shared guest RAM of ADR-0009 (`GuestMemory` v2 with `&self`,
//! ordered `load/store`, `compare_exchange`, `HostRange`) without touching any
//! description: only the implementations in this module change.
//!
//! Today's implementations sit on the frozen `ritsu_mem::GuestMemory` and a
//! [`Translate`] implementation:
//!
//! * [`MemBus`] — read/write access (`&mut GuestMemory`).
//! * [`ReadOnlyBus`] — a shared `&GuestMemory`: every data write fails with a bus error
//!   (synchronous external abort), as if the memory were ROM that aborts on writes.
//! * [`NoMemory`] — no memory at all: every access is a bus error.
//!
//! # Probing writes
//!
//! [`DataBus::probe_write`] must decide whether a later write would succeed without
//! performing it. The frozen `GuestMemory` has no such query, so [`MemBus`] probes by
//! *reading* the range. That is only allowed for memory whose reads have no side
//! effects and may be repeated: Arm DDI 0487 B2.10.2 forbids repeating accesses to
//! Device memory (`docs/research/R-ARCH-06-ddi0487.md`). [`MemBus`] therefore only
//! accepts memory that implements the marker trait [`PlainRam`] (today:
//! [`ritsu_mem::RamRegion`]). Once MMIO is reachable from loads/stores, the probe must
//! use the side-effect-free region attributes of ADR-0009 (§9, `region_attrs`) instead of
//! a read; a `GuestMemory` with devices behind it must never implement [`PlainRam`].
//!
use ritsu_core::{GuestPhysAddr, GuestVirtAddr};
use ritsu_mem::mmu::{Access, Translate, TranslationFault};
use ritsu_mem::{GuestMemory, MemError};

/// Memory as seen by one vCPU executing in T0.
///
/// Addresses passed to the physical methods come from [`DataBus::translate`] of the
/// same bus; an access never crosses a 4 KiB boundary (the caller splits), so one
/// translation covers it for every granule size.
pub trait DataBus {
    /// Translates the virtual address `va` for `access`.
    ///
    /// # Errors
    ///
    /// The stage-1 fault.
    fn translate(&mut self, va: u64, access: Access) -> Result<GuestPhysAddr, TranslationFault>;

    /// Reads `buf.len()` bytes at `pa`.
    ///
    /// # Errors
    ///
    /// No backing at `pa` (synchronous external abort).
    fn read(&mut self, pa: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemError>;

    /// Whether a write of `len` bytes at `pa` would succeed. Never writes.
    ///
    /// # Errors
    ///
    /// The error the write would return.
    fn probe_write(&mut self, pa: GuestPhysAddr, len: usize) -> Result<(), MemError>;

    /// Writes `data` at `pa`; all or nothing.
    ///
    /// # Errors
    ///
    /// No (writable) backing at `pa`.
    fn write(&mut self, pa: GuestPhysAddr, data: &[u8]) -> Result<(), MemError>;
}

/// Marker: every access to this memory is to Normal memory (RAM); reads have no side
/// effects and may be repeated. Required by [`MemBus`], whose write probe reads (see
/// the module documentation). Must not be implemented for memory with MMIO/Device
/// regions.
pub trait PlainRam: GuestMemory {}

impl PlainRam for ritsu_mem::RamRegion {}

/// Probes `len` bytes at `pa` by reading them into a scratch buffer (exact for
/// [`PlainRam`] only, see the module documentation).
fn probe_by_read<M: PlainRam + ?Sized>(
    mem: &M,
    pa: GuestPhysAddr,
    len: usize,
) -> Result<(), MemError> {
    let mut scratch = [0u8; 64];
    let mut done = 0usize;
    while done < len {
        let n = (len - done).min(scratch.len());
        let at = GuestPhysAddr(pa.get().wrapping_add(done as u64));
        // `n <= scratch.len()`, so the slice is in bounds.
        mem.read(at, scratch.get_mut(..n).unwrap_or(&mut []))?;
        done += n;
    }
    Ok(())
}

/// Read/write access to RAM ([`PlainRam`]) through a [`Translate`] implementation.
#[derive(Debug)]
pub struct MemBus<'a, M: PlainRam, T: Translate> {
    mem: &'a mut M,
    mmu: &'a mut T,
}

impl<'a, M: PlainRam, T: Translate> MemBus<'a, M, T> {
    /// Combines memory and MMU.
    pub fn new(mem: &'a mut M, mmu: &'a mut T) -> Self {
        Self { mem, mmu }
    }
}

impl<M: PlainRam, T: Translate> DataBus for MemBus<'_, M, T> {
    fn translate(&mut self, va: u64, access: Access) -> Result<GuestPhysAddr, TranslationFault> {
        self.mmu.translate(&*self.mem, GuestVirtAddr(va), access)
    }

    fn read(&mut self, pa: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemError> {
        self.mem.read(pa, buf)
    }

    fn probe_write(&mut self, pa: GuestPhysAddr, len: usize) -> Result<(), MemError> {
        probe_by_read(&*self.mem, pa, len)
    }

    fn write(&mut self, pa: GuestPhysAddr, data: &[u8]) -> Result<(), MemError> {
        self.mem.write(pa, data)
    }
}

/// Read-only access to a shared `GuestMemory`: data writes fail with
/// [`MemError::Unmapped`] (a synchronous external abort for the guest).
#[derive(Debug)]
pub struct ReadOnlyBus<'a, M: GuestMemory, T: Translate> {
    mem: &'a M,
    mmu: &'a mut T,
}

impl<'a, M: GuestMemory, T: Translate> ReadOnlyBus<'a, M, T> {
    /// Combines memory and MMU.
    pub fn new(mem: &'a M, mmu: &'a mut T) -> Self {
        Self { mem, mmu }
    }
}

impl<M: GuestMemory, T: Translate> DataBus for ReadOnlyBus<'_, M, T> {
    fn translate(&mut self, va: u64, access: Access) -> Result<GuestPhysAddr, TranslationFault> {
        self.mmu.translate(self.mem, GuestVirtAddr(va), access)
    }

    fn read(&mut self, pa: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemError> {
        self.mem.read(pa, buf)
    }

    fn probe_write(&mut self, pa: GuestPhysAddr, len: usize) -> Result<(), MemError> {
        Err(MemError::Unmapped { addr: pa, len })
    }

    fn write(&mut self, pa: GuestPhysAddr, data: &[u8]) -> Result<(), MemError> {
        Err(MemError::Unmapped {
            addr: pa,
            len: data.len(),
        })
    }
}

/// No memory: translation is the identity, every read and write is a bus error.
/// Used by [`crate::interp::execute`] for instructions without memory operands.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMemory;

impl DataBus for NoMemory {
    fn translate(&mut self, va: u64, _access: Access) -> Result<GuestPhysAddr, TranslationFault> {
        Ok(GuestPhysAddr(va))
    }

    fn read(&mut self, pa: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemError> {
        Err(MemError::Unmapped {
            addr: pa,
            len: buf.len(),
        })
    }

    fn probe_write(&mut self, pa: GuestPhysAddr, len: usize) -> Result<(), MemError> {
        Err(MemError::Unmapped { addr: pa, len })
    }

    fn write(&mut self, pa: GuestPhysAddr, data: &[u8]) -> Result<(), MemError> {
        Err(MemError::Unmapped {
            addr: pa,
            len: data.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ritsu_mem::RamRegion;
    use ritsu_mem::mmu::{AccessKind, IdentityTranslate, Privilege};

    const RD: Access = Access::new(AccessKind::Read, Privilege::Privileged);

    #[test]
    fn mem_bus_reads_writes_and_probes() {
        let mut ram = RamRegion::new(GuestPhysAddr(0x1000), 0x100);
        let mut mmu = IdentityTranslate::default();
        let mut bus = MemBus::new(&mut ram, &mut mmu);
        let pa = bus.translate(0x1010, RD).unwrap();
        assert_eq!(pa, GuestPhysAddr(0x1010));
        bus.write(pa, &[1, 2, 3]).unwrap();
        let mut b = [0u8; 3];
        bus.read(pa, &mut b).unwrap();
        assert_eq!(b, [1, 2, 3]);
        assert!(bus.probe_write(GuestPhysAddr(0x1000), 0x100).is_ok());
        // Longer than the scratch buffer and past the end.
        assert!(bus.probe_write(GuestPhysAddr(0x1000), 0x101).is_err());
        assert!(bus.probe_write(GuestPhysAddr(0x10ff), 2).is_err());
        assert!(bus.probe_write(GuestPhysAddr(0x10ff), 0).is_ok());
        assert!(bus.translate(1 << 60, RD).is_err());
    }

    #[test]
    fn read_only_bus_rejects_writes() {
        let ram = RamRegion::new(GuestPhysAddr(0), 0x10);
        let mut mmu = IdentityTranslate::default();
        let mut bus = ReadOnlyBus::new(&ram, &mut mmu);
        let mut b = [0u8; 4];
        assert!(bus.read(GuestPhysAddr(0), &mut b).is_ok());
        assert!(bus.probe_write(GuestPhysAddr(0), 4).is_err());
        assert!(bus.write(GuestPhysAddr(0), &b).is_err());
    }

    #[test]
    fn no_memory_fails_everything() {
        let mut bus = NoMemory;
        let mut b = [0u8; 1];
        assert_eq!(bus.translate(5, RD), Ok(GuestPhysAddr(5)));
        assert!(bus.read(GuestPhysAddr(0), &mut b).is_err());
        assert!(bus.probe_write(GuestPhysAddr(0), 1).is_err());
        assert!(bus.write(GuestPhysAddr(0), &b).is_err());
    }
}
