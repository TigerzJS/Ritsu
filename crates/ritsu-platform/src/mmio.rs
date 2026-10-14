//! MMIO device abstraction and bus.
//!
//! The DBT slow path (an access that is not plain RAM) calls [`MmioBus::read`] or
//! [`MmioBus::write`] with the guest physical address, the access width and, for writes,
//! the value. The bus finds the device that owns the address and forwards the access with
//! an offset relative to the device's base.
//!
//! The bus never panics on guest-controlled input. Unmapped, misaligned or region-crossing
//! accesses are reported as [`MmioError`]; the caller decides how that surfaces in the guest
//! (usually a synchronous external abort, or RAZ/WI for known holes).

use std::collections::BTreeMap;
use std::fmt;

/// Width of a single MMIO access in bytes.
///
/// AArch64 loads and stores to device memory are 1, 2, 4 or 8 bytes wide (pair and SIMD
/// accesses are split by the caller).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessSize {
    /// One byte.
    B1,
    /// Two bytes (halfword).
    B2,
    /// Four bytes (word).
    B4,
    /// Eight bytes (doubleword).
    B8,
}

impl AccessSize {
    /// Number of bytes of the access.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        match self {
            Self::B1 => 1,
            Self::B2 => 2,
            Self::B4 => 4,
            Self::B8 => 8,
        }
    }

    /// Bit mask selecting the low `bytes() * 8` bits of a `u64`.
    #[must_use]
    pub const fn mask(self) -> u64 {
        match self {
            Self::B1 => 0xff,
            Self::B2 => 0xffff,
            Self::B4 => 0xffff_ffff,
            Self::B8 => u64::MAX,
        }
    }

    /// Converts a byte count into an [`AccessSize`]; `None` unless it is 1, 2, 4 or 8.
    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Option<Self> {
        match bytes {
            1 => Some(Self::B1),
            2 => Some(Self::B2),
            4 => Some(Self::B4),
            8 => Some(Self::B8),
            _ => None,
        }
    }
}

/// Failure of an MMIO access.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MmioError {
    /// No device is mapped at the address.
    #[error("unmapped MMIO access at {addr:#x} (size {size})")]
    Unmapped {
        /// Guest physical address of the access.
        addr: u64,
        /// Access width in bytes.
        size: u64,
    },
    /// The access is not naturally aligned.
    #[error("misaligned MMIO access at {addr:#x} (size {size})")]
    Misaligned {
        /// Guest physical address of the access.
        addr: u64,
        /// Access width in bytes.
        size: u64,
    },
    /// The access starts inside a device region but extends past its end.
    #[error("MMIO access at {addr:#x} (size {size}) crosses the end of device region")]
    CrossesRegionEnd {
        /// Guest physical address of the access.
        addr: u64,
        /// Access width in bytes.
        size: u64,
    },
    /// The device does not implement a register at this offset.
    #[error("{device}: no register at offset {offset:#x}")]
    BadOffset {
        /// Device name.
        device: String,
        /// Offset relative to the device base.
        offset: u64,
    },
    /// The device does not support this access width at this offset.
    #[error("{device}: unsupported access size {size} at offset {offset:#x}")]
    BadSize {
        /// Device name.
        device: String,
        /// Offset relative to the device base.
        offset: u64,
        /// Access width in bytes.
        size: u64,
    },
}

impl From<MmioError> for ritsu_core::Error {
    fn from(e: MmioError) -> Self {
        ritsu_core::Error::Guest(e.to_string())
    }
}

/// A memory-mapped device.
///
/// `offset` is relative to the base the device was registered at and is guaranteed by
/// [`MmioBus`] to lie inside the registered length and to be naturally aligned for `size`.
/// Devices must still validate that the offset names a register they implement.
///
/// Devices are `Send` because vCPU threads share the bus behind a lock.
pub trait MmioDevice: Send {
    /// Short stable name used in diagnostics.
    fn name(&self) -> &str;

    /// Handles a guest read. The returned value is masked to `size` by the bus.
    ///
    /// # Errors
    ///
    /// Returns [`MmioError::BadOffset`] or [`MmioError::BadSize`] for accesses the device
    /// does not implement.
    fn read(&mut self, offset: u64, size: AccessSize) -> Result<u64, MmioError>;

    /// Handles a guest write. `value` is already masked to `size` by the bus.
    ///
    /// # Errors
    ///
    /// Returns [`MmioError::BadOffset`] or [`MmioError::BadSize`] for accesses the device
    /// does not implement.
    fn write(&mut self, offset: u64, size: AccessSize, value: u64) -> Result<(), MmioError>;
}

/// Failure to register a device on the [`MmioBus`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BusError {
    /// The region length is zero.
    #[error("device {name:?}: zero-length MMIO region")]
    ZeroLength {
        /// Name of the device being registered.
        name: String,
    },
    /// `base + len` does not fit into 64 bits.
    #[error("device {name:?}: region base {base:#x} + len {len:#x} overflows")]
    Overflow {
        /// Name of the device being registered.
        name: String,
        /// Region base.
        base: u64,
        /// Region length.
        len: u64,
    },
    /// The region overlaps an already registered one.
    #[error("device {name:?}: region {base:#x}+{len:#x} overlaps {other:?}")]
    Overlap {
        /// Name of the device being registered.
        name: String,
        /// Region base.
        base: u64,
        /// Region length.
        len: u64,
        /// Name of the existing device it collides with.
        other: String,
    },
}

struct Region {
    len: u64,
    device: Box<dyn MmioDevice>,
}

/// Dispatches MMIO accesses to devices by guest physical address.
///
/// Regions are kept in a `BTreeMap` keyed by base address, so a lookup is `O(log n)`.
#[derive(Default)]
pub struct MmioBus {
    regions: BTreeMap<u64, Region>,
}

impl fmt::Debug for MmioBus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_list();
        for (base, r) in &self.regions {
            list.entry(&format_args!("{base:#x}+{:#x} {}", r.len, r.device.name()));
        }
        list.finish()
    }
}

impl MmioBus {
    /// Creates an empty bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of registered devices.
    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Whether no device is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Registers `device` at guest physical `[base, base + len)`.
    ///
    /// # Errors
    ///
    /// * [`BusError::ZeroLength`] if `len == 0`,
    /// * [`BusError::Overflow`] if `base + len` overflows `u64`,
    /// * [`BusError::Overlap`] if the range intersects an existing region.
    pub fn register(
        &mut self,
        base: u64,
        len: u64,
        device: Box<dyn MmioDevice>,
    ) -> Result<(), BusError> {
        let name = device.name().to_owned();
        if len == 0 {
            return Err(BusError::ZeroLength { name });
        }
        let Some(end) = base.checked_add(len) else {
            return Err(BusError::Overflow { name, base, len });
        };
        // Predecessor: the region with the greatest base <= `base`.
        // (`pbase + p.len` cannot overflow: it was checked at registration.)
        if let Some((pbase, p)) = self.regions.range(..=base).next_back()
            && pbase + p.len > base
        {
            let other = p.device.name().to_owned();
            return Err(BusError::Overlap {
                name,
                base,
                len,
                other,
            });
        }
        // Successor: the region with the smallest base > `base`.
        if let Some((nbase, n)) = self.regions.range(base.saturating_add(1)..).next()
            && *nbase < end
        {
            let other = n.device.name().to_owned();
            return Err(BusError::Overlap {
                name,
                base,
                len,
                other,
            });
        }
        self.regions.insert(base, Region { len, device });
        Ok(())
    }

    /// Finds the region that contains `addr` and validates the access against it.
    ///
    /// Returns the region and the offset of the access inside it.
    fn locate(&mut self, addr: u64, size: AccessSize) -> Result<(&mut Region, u64), MmioError> {
        let bytes = size.bytes();
        let unmapped = MmioError::Unmapped { addr, size: bytes };
        let (base, region) = self
            .regions
            .range_mut(..=addr)
            .next_back()
            .ok_or(unmapped.clone())?;
        let offset = addr - *base;
        if offset >= region.len {
            return Err(unmapped);
        }
        if !addr.is_multiple_of(bytes) {
            return Err(MmioError::Misaligned { addr, size: bytes });
        }
        // `checked_add` keeps this overflow-free even for regions that end near `u64::MAX`.
        match offset.checked_add(bytes) {
            Some(e) if e <= region.len => Ok((region, offset)),
            _ => Err(MmioError::CrossesRegionEnd { addr, size: bytes }),
        }
    }

    /// Performs a guest read of `size` bytes at `addr`.
    ///
    /// # Errors
    ///
    /// Returns [`MmioError`] for unmapped, misaligned or region-crossing accesses and for
    /// errors reported by the device.
    pub fn read(&mut self, addr: u64, size: AccessSize) -> Result<u64, MmioError> {
        let (region, offset) = self.locate(addr, size)?;
        region.device.read(offset, size).map(|v| v & size.mask())
    }

    /// Performs a guest write of the low `size` bytes of `value` at `addr`.
    ///
    /// # Errors
    ///
    /// Returns [`MmioError`] for unmapped, misaligned or region-crossing accesses and for
    /// errors reported by the device.
    pub fn write(&mut self, addr: u64, size: AccessSize, value: u64) -> Result<(), MmioError> {
        let (region, offset) = self.locate(addr, size)?;
        region.device.write(offset, size, value & size.mask())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 KiB scratch device: one 64-bit word per 8 bytes of offset, remembers the last write.
    struct Scratch {
        name: &'static str,
        last: Option<(u64, AccessSize, u64)>,
    }

    impl Scratch {
        fn boxed(name: &'static str) -> Box<dyn MmioDevice> {
            Box::new(Self { name, last: None })
        }
    }

    impl MmioDevice for Scratch {
        fn name(&self) -> &str {
            self.name
        }
        fn read(&mut self, offset: u64, _size: AccessSize) -> Result<u64, MmioError> {
            // Returns more bits than the access width on purpose: the bus must mask.
            Ok(self
                .last
                .map_or(0x1122_3344_5566_7788 + offset, |(_, _, v)| v)
                | 0xffff_ffff_0000_0000)
        }
        fn write(&mut self, offset: u64, size: AccessSize, value: u64) -> Result<(), MmioError> {
            self.last = Some((offset, size, value));
            Ok(())
        }
    }

    #[test]
    fn access_size_helpers() {
        for (b, s) in [
            (1, AccessSize::B1),
            (2, AccessSize::B2),
            (4, AccessSize::B4),
            (8, AccessSize::B8),
        ] {
            assert_eq!(AccessSize::from_bytes(b), Some(s));
            assert_eq!(s.bytes(), b);
        }
        assert_eq!(AccessSize::from_bytes(0), None);
        assert_eq!(AccessSize::from_bytes(3), None);
        assert_eq!(AccessSize::from_bytes(16), None);
        assert_eq!(AccessSize::B4.mask(), 0xffff_ffff);
        assert_eq!(AccessSize::B8.mask(), u64::MAX);
    }

    #[test]
    fn rejects_zero_length_and_overflow() {
        let mut bus = MmioBus::new();
        assert!(matches!(
            bus.register(0x1000, 0, Scratch::boxed("a")),
            Err(BusError::ZeroLength { .. })
        ));
        assert!(matches!(
            bus.register(u64::MAX, 2, Scratch::boxed("a")),
            Err(BusError::Overflow { .. })
        ));
        assert!(matches!(
            bus.register(u64::MAX - 0xf, 0x20, Scratch::boxed("a")),
            Err(BusError::Overflow { .. })
        ));
        assert!(bus.is_empty());
    }

    #[test]
    fn region_ending_exactly_at_u64_max_is_accepted() {
        let mut bus = MmioBus::new();
        // end == u64::MAX does not overflow (the last byte is u64::MAX - 1).
        bus.register(u64::MAX - 0xf, 0xf, Scratch::boxed("top"))
            .unwrap();
        assert!(bus.read(u64::MAX - 0xf, AccessSize::B1).is_ok());
        assert!(matches!(
            bus.read(u64::MAX, AccessSize::B1),
            Err(MmioError::Unmapped { .. })
        ));
    }

    #[test]
    fn rejects_overlaps() {
        let mut bus = MmioBus::new();
        bus.register(0x1000, 0x1000, Scratch::boxed("a")).unwrap();
        // identical, contained, containing, overlapping start, overlapping end
        for (base, len) in [
            (0x1000, 0x1000),
            (0x1800, 0x10),
            (0x0, 0x3000),
            (0x0800, 0x1000),
            (0x1f00, 0x1000),
        ] {
            assert!(
                matches!(
                    bus.register(base, len, Scratch::boxed("b")),
                    Err(BusError::Overlap { .. })
                ),
                "{base:#x}+{len:#x}"
            );
        }
        // adjacent regions are fine, on both sides
        bus.register(0x2000, 0x1000, Scratch::boxed("c")).unwrap();
        bus.register(0x0, 0x1000, Scratch::boxed("d")).unwrap();
        assert_eq!(bus.len(), 3);
    }

    #[test]
    fn dispatches_with_relative_offset_and_masks() {
        let mut bus = MmioBus::new();
        bus.register(0x1000, 0x100, Scratch::boxed("a")).unwrap();
        bus.register(0x9000, 0x100, Scratch::boxed("b")).unwrap();
        // value is masked down to the access width
        assert_eq!(
            bus.read(0x1008, AccessSize::B4).unwrap(),
            (0x1122_3344_5566_7788u64 + 8) & 0xffff_ffff
        );
        bus.write(0x9010, AccessSize::B2, 0xabcd_ef01).unwrap();
        assert_eq!(bus.read(0x9010, AccessSize::B2).unwrap(), 0xef01);
        // other device unaffected
        assert_eq!(
            bus.read(0x1000, AccessSize::B8).unwrap(),
            0xffff_ffff_5566_7788
        );
    }

    #[test]
    fn boundary_accesses() {
        let mut bus = MmioBus::new();
        bus.register(0x1000, 0x100, Scratch::boxed("a")).unwrap();
        // first and last byte inside
        assert!(bus.read(0x1000, AccessSize::B1).is_ok());
        assert!(bus.read(0x10ff, AccessSize::B1).is_ok());
        // last aligned doubleword inside
        assert!(bus.read(0x10f8, AccessSize::B8).is_ok());
        // one before / one past
        assert!(matches!(
            bus.read(0x0fff, AccessSize::B1),
            Err(MmioError::Unmapped { .. })
        ));
        assert!(matches!(
            bus.read(0x1100, AccessSize::B1),
            Err(MmioError::Unmapped { .. })
        ));
        assert!(matches!(
            bus.write(0x1100, AccessSize::B4, 0),
            Err(MmioError::Unmapped { .. })
        ));
    }

    #[test]
    fn rejects_misaligned_and_region_crossing() {
        let mut bus = MmioBus::new();
        // 6-byte region: an aligned 8-byte access at its base would run past the end.
        bus.register(0x1000, 6, Scratch::boxed("a")).unwrap();
        assert!(matches!(
            bus.read(0x1001, AccessSize::B2),
            Err(MmioError::Misaligned { .. })
        ));
        assert!(matches!(
            bus.write(0x1002, AccessSize::B4, 0),
            Err(MmioError::Misaligned { .. })
        ));
        assert!(matches!(
            bus.read(0x1000, AccessSize::B8),
            Err(MmioError::CrossesRegionEnd { .. })
        ));
        assert!(bus.read(0x1004, AccessSize::B2).is_ok());
        assert!(matches!(
            bus.read(0x1004, AccessSize::B4),
            Err(MmioError::CrossesRegionEnd { .. })
        ));
    }

    #[test]
    fn unmapped_on_empty_bus_and_between_regions() {
        let mut bus = MmioBus::new();
        assert!(matches!(
            bus.read(0, AccessSize::B1),
            Err(MmioError::Unmapped { .. })
        ));
        bus.register(0x1000, 0x10, Scratch::boxed("a")).unwrap();
        bus.register(0x3000, 0x10, Scratch::boxed("b")).unwrap();
        assert!(matches!(
            bus.read(0x2000, AccessSize::B4),
            Err(MmioError::Unmapped { .. })
        ));
        assert!(matches!(
            bus.read(u64::MAX, AccessSize::B1),
            Err(MmioError::Unmapped { .. })
        ));
    }

    #[test]
    fn mmio_error_converts_to_guest_error() {
        let e: ritsu_core::Error = MmioError::Unmapped {
            addr: 0x10,
            size: 4,
        }
        .into();
        assert!(matches!(e, ritsu_core::Error::Guest(_)));
    }

    #[test]
    fn debug_lists_regions() {
        let mut bus = MmioBus::new();
        bus.register(0x1000, 0x10, Scratch::boxed("uart")).unwrap();
        assert!(format!("{bus:?}").contains("uart"));
    }
}
