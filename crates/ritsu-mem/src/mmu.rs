//! Guest virtual → guest physical address translation (SoftMMU interface).
//!
//! This module defines the vocabulary shared by the stage-1 page-table walker, the
//! software TLB ([`crate::tlb`]) and the CPU side (`ritsu-dbt`): access kinds, page
//! permissions, architectural fault types and the [`Translate`] trait.
//!
//! **Status (M0):** interface sketch. The real AArch64 stage-1 walker (16 KiB granule,
//! TTBR0/TTBR1 split, ASIDs, TBI, PAN, UXN/PXN) follows in a later work package (see
//! `docs/subsystems/dbt.md`, WP-DBT-06). Until then [`IdentityTranslate`] models a vCPU whose
//! stage-1 MMU is disabled (`SCTLR_EL1.M == 0`): VA == PA.
//!
//! The guest platform has no EL2/EL3, so there is exactly one translation regime
//! (EL1&0) and no stage 2.

use crate::GuestMemory;
use ritsu_core::{GuestPhysAddr, GuestVirtAddr};

/// What kind of access is being translated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessKind {
    /// Data read (loads, the read half of atomics, page-table walks are *not* this).
    Read,
    /// Data write (stores, the write half of atomics, `DC ZVA`).
    Write,
    /// Instruction fetch.
    Execute,
}

/// Privilege the access is checked against.
///
/// This is not always the current exception level: `LDTR`/`STTR` executed at EL1 are
/// checked as [`Privilege::Unprivileged`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Privilege {
    /// EL0 permissions (`AP[1]`, `UXN`).
    Unprivileged,
    /// EL1 permissions (`AP[2]`, `PXN`, `PAN`).
    Privileged,
}

/// A single memory access to translate: kind plus the privilege it is checked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Access {
    /// Read, write or instruction fetch.
    pub kind: AccessKind,
    /// Privilege used for the permission check.
    pub privilege: Privilege,
}

impl Access {
    /// Creates an access descriptor.
    #[must_use]
    pub const fn new(kind: AccessKind, privilege: Privilege) -> Self {
        Self { kind, privilege }
    }
}

/// Effective permissions of a mapped page, already resolved from the descriptor bits
/// (`AP`, `UXN`, `PXN`, and the `SCTLR_EL1.WXN` rule) by the walker.
///
/// Stored separately for EL0 and EL1 so that one TLB entry serves both privileges.
/// `PAN` is *not* folded in here because it depends on `PSTATE`, not on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct PagePerms(u8);

impl PagePerms {
    /// No access at all.
    pub const NONE: Self = Self(0);
    /// EL0 may read.
    pub const EL0_READ: Self = Self(1 << 0);
    /// EL0 may write.
    pub const EL0_WRITE: Self = Self(1 << 1);
    /// EL0 may execute.
    pub const EL0_EXEC: Self = Self(1 << 2);
    /// EL1 may read.
    pub const EL1_READ: Self = Self(1 << 3);
    /// EL1 may write.
    pub const EL1_WRITE: Self = Self(1 << 4);
    /// EL1 may execute.
    pub const EL1_EXEC: Self = Self(1 << 5);
    /// Every permission (used for the MMU-off identity mapping).
    pub const ALL: Self = Self(0x3f);

    /// Builds permissions from raw bits; unknown bits are dropped.
    #[must_use]
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & Self::ALL.0)
    }

    /// Raw bit representation.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Set union.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether all bits of `other` are set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether these permissions allow `access`.
    #[must_use]
    pub const fn allows(self, access: Access) -> bool {
        let needed = match (access.privilege, access.kind) {
            (Privilege::Unprivileged, AccessKind::Read) => Self::EL0_READ,
            (Privilege::Unprivileged, AccessKind::Write) => Self::EL0_WRITE,
            (Privilege::Unprivileged, AccessKind::Execute) => Self::EL0_EXEC,
            (Privilege::Privileged, AccessKind::Read) => Self::EL1_READ,
            (Privilege::Privileged, AccessKind::Write) => Self::EL1_WRITE,
            (Privilege::Privileged, AccessKind::Execute) => Self::EL1_EXEC,
        };
        self.contains(needed)
    }
}

/// Architectural fault type of a failed translation.
///
/// Mirrors the fault status codes (`DFSC`/`IFSC`) of `ESR_EL1`; see [`FaultKind::fsc`].
/// `level` is the translation table level (0..=3) at which the fault was detected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FaultKind {
    /// Output or input address exceeds the configured size (`TCR_EL1.IPS`, `TxSZ`).
    AddressSize {
        /// Lookup level (0..=3).
        level: u8,
    },
    /// No valid descriptor.
    Translation {
        /// Lookup level (0..=3).
        level: u8,
    },
    /// Descriptor has `AF == 0` (and hardware AF update is not implemented).
    AccessFlag {
        /// Lookup level (0..=3).
        level: u8,
    },
    /// Descriptor forbids the access.
    Permission {
        /// Lookup level (0..=3).
        level: u8,
    },
    /// Misaligned access where alignment is required (e.g. exclusives, Device memory).
    Alignment,
    /// The page-table walk itself hit unbacked guest physical memory.
    ExternalAbortOnWalk {
        /// Lookup level (0..=3).
        level: u8,
    },
    /// Conflicting TLB entries (architecturally allowed; Ritsu's TLB never produces it,
    /// listed for completeness of the syndrome mapping).
    TlbConflict,
}

impl FaultKind {
    /// The 6-bit fault status code for `ESR_EL1.ISS.{D,I}FSC`.
    ///
    /// Encodings per the Arm ARM (`ESR_ELx`, ISS encoding for Data/Instruction Abort):
    /// address size `0b0000ll`, translation `0b0001ll`, access flag `0b0010ll`,
    /// permission `0b0011ll`, synchronous external abort on walk `0b0101ll`,
    /// alignment `0b100001`, TLB conflict `0b110000` (`ll` = level).
    #[must_use]
    pub const fn fsc(self) -> u8 {
        match self {
            Self::AddressSize { level } => level & 3,
            Self::Translation { level } => 0b00_0100 | (level & 3),
            Self::AccessFlag { level } => 0b00_1000 | (level & 3),
            Self::Permission { level } => 0b00_1100 | (level & 3),
            Self::ExternalAbortOnWalk { level } => 0b01_0100 | (level & 3),
            Self::Alignment => 0b10_0001,
            Self::TlbConflict => 0b11_0000,
        }
    }
}

/// A failed translation: everything needed to build the guest exception
/// (`ESR_EL1`, `FAR_EL1`) precisely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{kind:?} fault on {access:?} at {va:?}")]
pub struct TranslationFault {
    /// Faulting virtual address (becomes `FAR_EL1`).
    pub va: GuestVirtAddr,
    /// The access that faulted.
    pub access: Access,
    /// Architectural fault type.
    pub kind: FaultKind,
}

/// Translates guest virtual addresses of the EL1&0 regime to guest physical addresses.
///
/// Implementations: [`IdentityTranslate`] (MMU off) now; the stage-1 walker later.
/// The walker reads page tables through `mem` (guest physical memory) and may fill a
/// [`crate::tlb::Tlb`], hence `&mut self`.
///
/// Contract: never panics on guest-controlled input; every failure is a
/// [`TranslationFault`] that maps 1:1 onto an architectural Data/Instruction Abort.
pub trait Translate {
    /// Translates `va` for `access`. Returns the physical address of the same byte.
    ///
    /// # Errors
    ///
    /// Returns a [`TranslationFault`] describing the architectural abort to raise.
    fn translate(
        &mut self,
        mem: &dyn GuestMemory,
        va: GuestVirtAddr,
        access: Access,
    ) -> Result<GuestPhysAddr, TranslationFault>;
}

/// Flat VA == PA translation, as with the stage-1 MMU disabled (`SCTLR_EL1.M == 0`).
///
/// Architecturally, with the MMU off an address with bits set above the implemented
/// physical address size raises an Address Size fault at level 0; that check is
/// implemented. Not modeled yet: the Device-nGnRnE memory type of data accesses with
/// the MMU off (which makes unaligned data accesses fault).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityTranslate {
    pa_bits: u8,
}

impl IdentityTranslate {
    /// Physical address size used by [`Default`] (48 bits). Placeholder until the
    /// reported `ID_AA64MMFR0_EL1.PARange` is decided.
    pub const DEFAULT_PA_BITS: u8 = 48;

    /// Creates an identity translation with an implemented PA size of `pa_bits`,
    /// clamped to the architectural range 32..=52.
    #[must_use]
    pub const fn new(pa_bits: u8) -> Self {
        let pa_bits = if pa_bits < 32 {
            32
        } else if pa_bits > 52 {
            52
        } else {
            pa_bits
        };
        Self { pa_bits }
    }

    /// Implemented physical address size in bits.
    #[must_use]
    pub const fn pa_bits(self) -> u8 {
        self.pa_bits
    }
}

impl Default for IdentityTranslate {
    fn default() -> Self {
        Self::new(Self::DEFAULT_PA_BITS)
    }
}

impl Translate for IdentityTranslate {
    fn translate(
        &mut self,
        _mem: &dyn GuestMemory,
        va: GuestVirtAddr,
        access: Access,
    ) -> Result<GuestPhysAddr, TranslationFault> {
        if va.get() >> self.pa_bits != 0 {
            return Err(TranslationFault {
                va,
                access,
                kind: FaultKind::AddressSize { level: 0 },
            });
        }
        Ok(GuestPhysAddr(va.get()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RamRegion;

    const EL0_R: Access = Access::new(AccessKind::Read, Privilege::Unprivileged);
    const EL1_X: Access = Access::new(AccessKind::Execute, Privilege::Privileged);

    #[test]
    fn perms_allow_matrix() {
        let p = PagePerms::EL0_READ.union(PagePerms::EL1_EXEC);
        assert!(p.allows(EL0_R));
        assert!(p.allows(EL1_X));
        assert!(!p.allows(Access::new(AccessKind::Write, Privilege::Unprivileged)));
        assert!(!p.allows(Access::new(AccessKind::Read, Privilege::Privileged)));
        assert!(PagePerms::ALL.allows(Access::new(AccessKind::Write, Privilege::Privileged)));
        assert!(!PagePerms::NONE.allows(EL0_R));
        assert_eq!(PagePerms::from_bits_truncate(0xff), PagePerms::ALL);
    }

    #[test]
    fn fault_status_codes_match_arm_arm() {
        assert_eq!(FaultKind::AddressSize { level: 0 }.fsc(), 0b00_0000);
        assert_eq!(FaultKind::Translation { level: 3 }.fsc(), 0b00_0111);
        assert_eq!(FaultKind::AccessFlag { level: 2 }.fsc(), 0b00_1010);
        assert_eq!(FaultKind::Permission { level: 3 }.fsc(), 0b00_1111);
        assert_eq!(FaultKind::ExternalAbortOnWalk { level: 1 }.fsc(), 0b01_0101);
        assert_eq!(FaultKind::Alignment.fsc(), 0b10_0001);
        assert_eq!(FaultKind::TlbConflict.fsc(), 0b11_0000);
    }

    #[test]
    fn identity_translate_and_address_size_fault() {
        let ram = RamRegion::new(GuestPhysAddr(0), 0);
        let mut t = IdentityTranslate::default();
        assert_eq!(
            t.translate(&ram, GuestVirtAddr(0x1234_5678), EL1_X),
            Ok(GuestPhysAddr(0x1234_5678))
        );
        let top = GuestVirtAddr(1 << 48);
        assert_eq!(
            t.translate(&ram, top, EL0_R),
            Err(TranslationFault {
                va: top,
                access: EL0_R,
                kind: FaultKind::AddressSize { level: 0 }
            })
        );
    }

    #[test]
    fn identity_pa_bits_clamped() {
        assert_eq!(IdentityTranslate::new(8).pa_bits(), 32);
        assert_eq!(IdentityTranslate::new(64).pa_bits(), 52);
        assert_eq!(IdentityTranslate::new(40).pa_bits(), 40);
    }
}
