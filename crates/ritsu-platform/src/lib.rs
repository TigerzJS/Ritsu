//! Apple VM platform (vmapple) machine model.
//!
//! This crate will own the guest physical memory map, the interrupt controller, the timers
//! and all platform devices (see `docs/subsystems/platform.md`). For now it contains the
//! device-independent building blocks:
//!
//! * [`mmio`]: the MMIO device trait and the address-dispatching [`mmio::MmioBus`],
//! * [`pl011`]: a minimal PL011 UART model,
//! * [`machine`]: the validated [`machine::MachineConfig`].
//!
//! Everything that arrives from the guest (addresses, sizes, values) is untrusted: the
//! bus and the devices return errors instead of panicking.

pub mod machine;
pub mod mmio;
pub mod pl011;
