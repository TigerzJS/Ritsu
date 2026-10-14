//! A single contiguous RAM region. Simple reference implementation of [`GuestMemory`];
//! the production backend (host `mmap`, multiple regions) replaces it later.

use crate::{GuestMemory, MemError};
use ritsu_core::GuestPhysAddr;

/// Contiguous guest RAM starting at `base`.
#[derive(Debug)]
pub struct RamRegion {
    base: GuestPhysAddr,
    bytes: Vec<u8>,
}

impl RamRegion {
    /// Creates zeroed RAM of `size` bytes at guest physical address `base`.
    #[must_use]
    pub fn new(base: GuestPhysAddr, size: usize) -> Self {
        Self {
            base,
            bytes: vec![0; size],
        }
    }

    /// Guest physical base address.
    #[must_use]
    pub fn base(&self) -> GuestPhysAddr {
        self.base
    }

    /// Size in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether the region has zero size.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn range(&self, addr: GuestPhysAddr, len: usize) -> Result<core::ops::Range<usize>, MemError> {
        let err = MemError::Unmapped { addr, len };
        let off = addr.get().checked_sub(self.base.get()).ok_or(err.clone())?;
        let start = usize::try_from(off).map_err(|_| err.clone())?;
        let end = start.checked_add(len).ok_or(err.clone())?;
        if end > self.bytes.len() {
            return Err(err);
        }
        Ok(start..end)
    }
}

impl GuestMemory for RamRegion {
    fn read(&self, addr: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemError> {
        let r = self.range(addr, buf.len())?;
        buf.copy_from_slice(&self.bytes[r]);
        Ok(())
    }

    fn write(&mut self, addr: GuestPhysAddr, data: &[u8]) -> Result<(), MemError> {
        let r = self.range(addr, data.len())?;
        self.bytes[r].copy_from_slice(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_u64() {
        let mut ram = RamRegion::new(GuestPhysAddr(0x4000_0000), 0x1000);
        ram.write_u64(GuestPhysAddr(0x4000_0008), 0xdead_beef_cafe_f00d)
            .unwrap();
        assert_eq!(
            ram.read_u64(GuestPhysAddr(0x4000_0008)).unwrap(),
            0xdead_beef_cafe_f00d
        );
    }

    #[test]
    fn rejects_out_of_bounds_without_partial_write() {
        let mut ram = RamRegion::new(GuestPhysAddr(0x1000), 16);
        assert!(ram.write(GuestPhysAddr(0x100c), &[1; 8]).is_err());
        assert_eq!(ram.read_u32(GuestPhysAddr(0x100c)).unwrap(), 0);
    }

    #[test]
    fn rejects_below_base_and_overflow() {
        let ram = RamRegion::new(GuestPhysAddr(0x1000), 16);
        let mut b = [0u8; 4];
        assert!(ram.read(GuestPhysAddr(0xfff), &mut b).is_err());
        assert!(ram.read(GuestPhysAddr(u64::MAX - 1), &mut b).is_err());
    }
}
