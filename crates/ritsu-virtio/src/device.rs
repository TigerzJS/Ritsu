//! The device-side interface of virtio devices and the device-ID registry.
//!
//! Device IDs are defined in the OASIS *Virtual I/O Device (VIRTIO) Specification Version 1.2*,
//! §5 "Device Types" (table in §5, e.g. §5.1 network card = 1, §5.2 block device = 2,
//! §5.3 console = 3, §5.4 entropy = 4, §5.7 GPU = 16, §5.8 input = 18, §5.14 sound = 25).
//! Whether the macOS guest expects a given device over virtio-mmio or virtio-pci is open and
//! documented in `docs/subsystems/platform.md`.

use ritsu_mem::GuestMemory;

use crate::queue::{Queue, QueueError};

/// `VIRTIO_F_VERSION_1` feature bit (bit 32): the device conforms to virtio 1.x (§6).
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// Virtio device types supported or planned by Ritsu (spec §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DeviceId {
    /// Network card (ID 1).
    Net,
    /// Block device (ID 2).
    Block,
    /// Console (ID 3).
    Console,
    /// Entropy source / RNG (ID 4).
    Entropy,
    /// GPU device (ID 16). Not used for the Apple paravirtual GPU, see ADR-0005.
    Gpu,
    /// Input device (ID 18).
    Input,
    /// Sound device (ID 25).
    Sound,
}

impl DeviceId {
    /// The numeric ID from the specification.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Net => 1,
            Self::Block => 2,
            Self::Console => 3,
            Self::Entropy => 4,
            Self::Gpu => 16,
            Self::Input => 18,
            Self::Sound => 25,
        }
    }

    /// Looks up a device by its numeric ID; `None` for IDs Ritsu does not know.
    #[must_use]
    pub const fn from_u32(id: u32) -> Option<Self> {
        match id {
            1 => Some(Self::Net),
            2 => Some(Self::Block),
            3 => Some(Self::Console),
            4 => Some(Self::Entropy),
            16 => Some(Self::Gpu),
            18 => Some(Self::Input),
            25 => Some(Self::Sound),
            _ => None,
        }
    }
}

/// Errors reported by a virtio device implementation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DeviceError {
    /// The guest notified or configured a queue index the device does not have.
    #[error("queue index {0} does not exist")]
    NoSuchQueue(u16),
    /// A config-space access outside the device's config structure or with a bad length.
    #[error("invalid config access at offset {offset:#x}, length {len}")]
    BadConfigAccess {
        /// Offset into the device-specific config space.
        offset: u64,
        /// Access length in bytes.
        len: usize,
    },
    /// Virtqueue protocol violation by the guest.
    #[error(transparent)]
    Queue(#[from] QueueError),
}

impl From<DeviceError> for ritsu_core::Error {
    fn from(e: DeviceError) -> Self {
        match e {
            DeviceError::Queue(q) => q.into(),
            other => ritsu_core::Error::Guest(other.to_string()),
        }
    }
}

/// Device-side behaviour of a virtio device, independent of the transport (MMIO or PCI).
///
/// The transport owns feature negotiation, status, queue registers and interrupt status; it
/// calls into the device for the device-specific parts. This trait is a **sketch** that will be
/// refined together with the first transport and device (WP-PLAT-04/05).
pub trait VirtioDevice: Send {
    /// The device type.
    fn device_id(&self) -> DeviceId;

    /// Feature bits the device offers (64-bit; must include [`VIRTIO_F_VERSION_1`]).
    fn device_features(&self) -> u64;

    /// Number of virtqueues the device has.
    fn num_queues(&self) -> u16;

    /// Maximum queue size the device supports for queue `index` (power of two, ≤ 32768).
    ///
    /// # Errors
    ///
    /// [`DeviceError::NoSuchQueue`] if `index >= num_queues()`.
    fn max_queue_size(&self, index: u16) -> Result<u16, DeviceError>;

    /// Reads device-specific config space into `data`.
    ///
    /// # Errors
    ///
    /// [`DeviceError::BadConfigAccess`] for ranges outside the config structure.
    fn read_config(&self, offset: u64, data: &mut [u8]) -> Result<(), DeviceError>;

    /// Writes device-specific config space.
    ///
    /// # Errors
    ///
    /// [`DeviceError::BadConfigAccess`] for ranges outside the config structure or read-only
    /// fields.
    fn write_config(&mut self, offset: u64, data: &[u8]) -> Result<(), DeviceError>;

    /// The guest kicked queue `index`. The device drains `queue`, accessing buffers through
    /// `mem`, and returns `true` if used buffers were published so the transport should raise
    /// the interrupt (subject to the queue's interrupt suppression).
    ///
    /// # Errors
    ///
    /// [`DeviceError::NoSuchQueue`] or a [`DeviceError::Queue`] protocol violation (the
    /// transport then sets `DEVICE_NEEDS_RESET`).
    fn queue_notify(
        &mut self,
        index: u16,
        queue: &mut Queue,
        mem: &mut dyn GuestMemory,
    ) -> Result<bool, DeviceError>;

    /// Resets the device to its power-on state (guest wrote status 0).
    fn reset(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::QueueConfig;
    use ritsu_core::GuestPhysAddr;
    use ritsu_mem::RamRegion;

    #[test]
    fn spec_ids() {
        let all = [
            (DeviceId::Net, 1),
            (DeviceId::Block, 2),
            (DeviceId::Console, 3),
            (DeviceId::Entropy, 4),
            (DeviceId::Gpu, 16),
            (DeviceId::Input, 18),
            (DeviceId::Sound, 25),
        ];
        for (d, n) in all {
            assert_eq!(d.as_u32(), n);
            assert_eq!(DeviceId::from_u32(n), Some(d));
        }
        for unknown in [0, 5, 17, 26, u32::MAX] {
            assert_eq!(DeviceId::from_u32(unknown), None);
        }
    }

    /// A trivial device that completes every request without touching the buffers.
    struct Nop;

    impl VirtioDevice for Nop {
        fn device_id(&self) -> DeviceId {
            DeviceId::Entropy
        }
        fn device_features(&self) -> u64 {
            VIRTIO_F_VERSION_1
        }
        fn num_queues(&self) -> u16 {
            1
        }
        fn max_queue_size(&self, index: u16) -> Result<u16, DeviceError> {
            if index == 0 {
                Ok(128)
            } else {
                Err(DeviceError::NoSuchQueue(index))
            }
        }
        fn read_config(&self, offset: u64, data: &mut [u8]) -> Result<(), DeviceError> {
            Err(DeviceError::BadConfigAccess {
                offset,
                len: data.len(),
            })
        }
        fn write_config(&mut self, offset: u64, data: &[u8]) -> Result<(), DeviceError> {
            Err(DeviceError::BadConfigAccess {
                offset,
                len: data.len(),
            })
        }
        fn queue_notify(
            &mut self,
            index: u16,
            queue: &mut Queue,
            mem: &mut dyn GuestMemory,
        ) -> Result<bool, DeviceError> {
            if index != 0 {
                return Err(DeviceError::NoSuchQueue(index));
            }
            let mut any = false;
            while let Some(chain) = queue.pop(mem)? {
                queue.push_used(mem, chain.head, 0)?;
                any = true;
            }
            Ok(any)
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn trait_object_drives_a_queue() {
        let base = 0x4000_0000;
        let mut ram = RamRegion::new(GuestPhysAddr(base), 0x4000);
        let cfg = QueueConfig {
            size: 4,
            desc_table: GuestPhysAddr(base),
            avail_ring: GuestPhysAddr(base + 0x1000),
            used_ring: GuestPhysAddr(base + 0x2000),
        };
        // one single-descriptor chain, head 0
        ram.write_u64(GuestPhysAddr(base), base + 0x3000).unwrap();
        ram.write_u32(GuestPhysAddr(base + 8), 16).unwrap();
        ram.write(GuestPhysAddr(base + 0x1000 + 2), &1u16.to_le_bytes())
            .unwrap();

        let mut q = Queue::new(cfg).unwrap();
        let mut dev: Box<dyn VirtioDevice> = Box::new(Nop);
        assert!(dev.queue_notify(0, &mut q, &mut ram).unwrap());
        assert!(!dev.queue_notify(0, &mut q, &mut ram).unwrap());
        assert_eq!(
            dev.queue_notify(1, &mut q, &mut ram),
            Err(DeviceError::NoSuchQueue(1))
        );
        assert_eq!(dev.max_queue_size(0), Ok(128));
        assert!(dev.read_config(0, &mut [0; 4]).is_err());
    }

    #[test]
    fn device_error_converts_to_core_error() {
        let e: ritsu_core::Error = DeviceError::NoSuchQueue(9).into();
        assert!(matches!(e, ritsu_core::Error::Guest(_)));
        let e: ritsu_core::Error = DeviceError::Queue(QueueError::Unsupported("x")).into();
        assert!(matches!(e, ritsu_core::Error::Unimplemented(_)));
    }
}
