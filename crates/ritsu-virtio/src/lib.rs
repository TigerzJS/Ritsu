//! virtio building blocks for the guest platform.
//!
//! * [`queue`]: split virtqueue (virtio 1.2 §2.7) with validating descriptor-chain walker,
//! * [`device`]: the [`device::VirtioDevice`] trait and the virtio device-ID registry.
//!
//! The transport (MMIO or PCI, see `docs/subsystems/platform.md` §virtio) and the concrete
//! devices are added in later work packages. All data read from guest memory is untrusted.

pub mod device;
pub mod queue;
