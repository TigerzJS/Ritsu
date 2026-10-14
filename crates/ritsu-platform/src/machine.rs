//! Machine configuration and its validation.
//!
//! [`MachineConfig`] is what `ritsu-vmm` builds from the command line and what the platform
//! consumes to construct the machine. All limits here are *Ritsu* limits chosen for the first
//! target (G5 MF), not facts about Apple's platform; values marked "Annahme" need review.

use std::path::PathBuf;

use ritsu_core::PAGE_SIZE_16K;

/// Smallest accepted guest RAM in bytes (2 GiB). **Annahme:** macOS 27 may need more to boot;
/// the real minimum is determined by spike S-BOOT-1 / M2.
pub const MIN_RAM_BYTES: u64 = 2 << 30;
/// Largest accepted guest RAM in bytes (64 GiB), a sanity bound against typos.
pub const MAX_RAM_BYTES: u64 = 64 << 30;
/// Default guest RAM in bytes (8 GiB). **Annahme.**
pub const DEFAULT_RAM_BYTES: u64 = 8 << 30;
/// Maximum number of vCPUs. **Annahme:** the i5-12500H has 4 P-cores (8 threads) + 8 E-cores;
/// 8 leaves host threads for the compiler and device threads.
pub const MAX_CPUS: u32 = 8;
/// Default number of vCPUs. **Annahme.**
pub const DEFAULT_CPUS: u32 = 4;

/// Reasons a [`MachineConfig`] is rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// RAM is smaller than [`MIN_RAM_BYTES`].
    #[error("RAM size {bytes} bytes is below the minimum of {MIN_RAM_BYTES} bytes")]
    RamTooSmall {
        /// Requested size in bytes.
        bytes: u64,
    },
    /// RAM is larger than [`MAX_RAM_BYTES`].
    #[error("RAM size {bytes} bytes is above the maximum of {MAX_RAM_BYTES} bytes")]
    RamTooLarge {
        /// Requested size in bytes.
        bytes: u64,
    },
    /// RAM is not a multiple of the 16 KiB guest page size.
    #[error("RAM size {bytes} bytes is not a multiple of 16 KiB")]
    RamNotAligned {
        /// Requested size in bytes.
        bytes: u64,
    },
    /// vCPU count is zero or above [`MAX_CPUS`].
    #[error("vCPU count {cpus} is outside 1..={MAX_CPUS}")]
    CpuCount {
        /// Requested vCPU count.
        cpus: u32,
    },
}

/// Static configuration of one virtual machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineConfig {
    /// Guest RAM size in bytes. Must be a multiple of 16 KiB within
    /// [`MIN_RAM_BYTES`]..=[`MAX_RAM_BYTES`].
    pub ram_bytes: u64,
    /// Number of vCPUs, `1..=`[`MAX_CPUS`].
    pub cpus: u32,
    /// Root disk image (the macOS system volume container).
    pub disk: Option<PathBuf>,
    /// Auxiliary storage image (NVRAM / boot-policy state, see `docs/subsystems/platform.md`).
    pub aux: Option<PathBuf>,
    /// User-supplied boot ROM (never shipped with Ritsu).
    pub boot_rom: Option<PathBuf>,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            ram_bytes: DEFAULT_RAM_BYTES,
            cpus: DEFAULT_CPUS,
            disk: None,
            aux: None,
            boot_rom: None,
        }
    }
}

impl MachineConfig {
    /// Checks all invariants.
    ///
    /// Paths are not checked here (this is a pure function without I/O); existence and
    /// format checks happen when the images are opened.
    ///
    /// # Errors
    ///
    /// Returns the first violated [`ConfigError`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bytes = self.ram_bytes;
        if bytes < MIN_RAM_BYTES {
            return Err(ConfigError::RamTooSmall { bytes });
        }
        if bytes > MAX_RAM_BYTES {
            return Err(ConfigError::RamTooLarge { bytes });
        }
        if !bytes.is_multiple_of(PAGE_SIZE_16K) {
            return Err(ConfigError::RamNotAligned { bytes });
        }
        if self.cpus == 0 || self.cpus > MAX_CPUS {
            return Err(ConfigError::CpuCount { cpus: self.cpus });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        assert_eq!(MachineConfig::default().validate(), Ok(()));
    }

    #[test]
    fn ram_bounds() {
        let mut c = MachineConfig {
            ram_bytes: MIN_RAM_BYTES,
            ..MachineConfig::default()
        };
        assert_eq!(c.validate(), Ok(()));
        c.ram_bytes = MIN_RAM_BYTES - PAGE_SIZE_16K;
        assert!(matches!(c.validate(), Err(ConfigError::RamTooSmall { .. })));
        c.ram_bytes = MAX_RAM_BYTES;
        assert_eq!(c.validate(), Ok(()));
        c.ram_bytes = MAX_RAM_BYTES + PAGE_SIZE_16K;
        assert!(matches!(c.validate(), Err(ConfigError::RamTooLarge { .. })));
        c.ram_bytes = u64::MAX;
        assert!(matches!(c.validate(), Err(ConfigError::RamTooLarge { .. })));
        c.ram_bytes = 0;
        assert!(matches!(c.validate(), Err(ConfigError::RamTooSmall { .. })));
    }

    #[test]
    fn ram_alignment_is_16k_not_4k() {
        let mut c = MachineConfig {
            ram_bytes: MIN_RAM_BYTES + 4096,
            ..MachineConfig::default()
        };
        assert!(matches!(
            c.validate(),
            Err(ConfigError::RamNotAligned { .. })
        ));
        c.ram_bytes = MIN_RAM_BYTES + 16 * 1024;
        assert_eq!(c.validate(), Ok(()));
    }

    #[test]
    fn cpu_bounds() {
        let mut c = MachineConfig::default();
        for ok in [1, 2, MAX_CPUS] {
            c.cpus = ok;
            assert_eq!(c.validate(), Ok(()));
        }
        for bad in [0, MAX_CPUS + 1, u32::MAX] {
            c.cpus = bad;
            assert_eq!(c.validate(), Err(ConfigError::CpuCount { cpus: bad }));
        }
    }

    #[test]
    fn error_messages_are_readable() {
        let msg = ConfigError::CpuCount { cpus: 0 }.to_string();
        assert!(msg.contains("1..=8"), "{msg}");
    }
}
