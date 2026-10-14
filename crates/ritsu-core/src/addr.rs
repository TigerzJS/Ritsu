//! Strongly typed guest addresses.
//!
//! Guest physical and guest virtual addresses are different types so that a missing
//! translation step is a compile error rather than a silent bug.

use core::fmt;

/// Translation granule used by macOS arm64 (16 KiB pages).
pub const PAGE_SIZE_16K: u64 = 16 * 1024;

macro_rules! guest_addr {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        #[repr(transparent)]
        pub struct $name(pub u64);

        impl $name {
            /// Returns the raw address value.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }

            /// Adds `offset`, returning `None` on overflow.
            #[must_use]
            pub const fn checked_add(self, offset: u64) -> Option<Self> {
                match self.0.checked_add(offset) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }

            /// Rounds down to the start of the containing page of size `page` (power of two).
            #[must_use]
            pub const fn align_down(self, page: u64) -> Self {
                debug_assert!(page.is_power_of_two());
                Self(self.0 & !(page - 1))
            }

            /// Offset within a page of size `page` (power of two).
            #[must_use]
            pub const fn page_offset(self, page: u64) -> u64 {
                debug_assert!(page.is_power_of_two());
                self.0 & (page - 1)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:#018x})"), self.0)
            }
        }

        impl fmt::LowerHex for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::LowerHex::fmt(&self.0, f)
            }
        }
    };
}

guest_addr!(
    /// An address in the guest's physical address space (output of the guest MMU).
    GuestPhysAddr
);
guest_addr!(
    /// An address in a guest virtual address space (input to the guest MMU).
    GuestVirtAddr
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_and_offset_16k() {
        let a = GuestVirtAddr(0x1_0000_4123);
        assert_eq!(a.align_down(PAGE_SIZE_16K), GuestVirtAddr(0x1_0000_4000));
        assert_eq!(a.page_offset(PAGE_SIZE_16K), 0x123);
    }

    #[test]
    fn checked_add_overflow() {
        assert_eq!(GuestPhysAddr(u64::MAX).checked_add(1), None);
        assert_eq!(GuestPhysAddr(1).checked_add(1), Some(GuestPhysAddr(2)));
    }

    #[test]
    fn debug_format_is_hex() {
        assert_eq!(format!("{:?}", GuestPhysAddr(0x10)), "GuestPhysAddr(0x0000000000000010)");
    }
}
