//! Direct-mapped software TLB for the 16 KiB translation granule.
//!
//! One [`Tlb`] belongs to one vCPU and caches stage-1 translations of the EL1&0 regime.
//! It is the *reference* TLB: lookups check ASID, global bit and permissions explicitly.
//! The JIT-facing layout (per-access-kind comparator words with flag bits in the page
//! offset, host addend) is derived from it in WP-DBT-08; see `docs/subsystems/dbt.md`.
//!
//! Architectural invalidation semantics implemented here:
//!
//! * [`Tlb::flush_all`] — `TLBI VMALLE1`.
//! * [`Tlb::flush_asid`] — `TLBI ASIDE1`: non-global entries of one ASID only.
//! * [`Tlb::flush_va`] — `TLBI VAE1`: the entry for one VA with a matching ASID, or a
//!   global one.
//! * [`Tlb::flush_va_all_asids`] — `TLBI VAAE1`: the entry for one VA, any ASID.
//!
//! Block mappings (e.g. 32 MiB level-2 blocks) are cached as individual 16 KiB
//! fragments, each in the slot of its own page. Architecturally, a TLB maintenance
//! instruction by VA (Arm ARM D8.13, "TLB maintenance instructions") must remove *every*
//! cached translation that could be used to translate that VA. For a block mapping
//! that is each fragment of the block, wherever it sits, even if the VA itself was
//! never looked up. Invalidating *more* than required is always permitted, because a
//! TLB is a cache whose contents software can never rely on.
//!
//! This TLB therefore keeps one conservative summary of all block mappings inserted
//! since the last full flush: the closed VA interval `[lo, hi]` spanning all of them.
//! A by-VA invalidation that falls inside the interval may hit a fragment in an
//! unrelated slot, so it invalidates everything (a superset of what is required). A
//! by-VA invalidation outside the interval cannot belong to any cached block and only
//! touches its own slot. The interval only shrinks on a full flush, so it always covers
//! every fragment still present.

use crate::mmu::{Access, PagePerms};
use ritsu_core::{GuestPhysAddr, GuestVirtAddr, PAGE_SIZE_16K};

/// log2 of the TLB page size (16 KiB).
pub const TLB_PAGE_SHIFT: u32 = 14;

const PAGE_MASK: u64 = PAGE_SIZE_16K - 1;

// The TLB is hard-wired to the 16 KiB granule used by macOS.
const _: () = assert!(1u64 << TLB_PAGE_SHIFT == PAGE_SIZE_16K);

/// One cached translation of a 16 KiB page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TlbEntry {
    /// Page-aligned virtual address (after TBI/PAC stripping).
    pub va_page: GuestVirtAddr,
    /// Page-aligned physical address.
    pub pa_page: GuestPhysAddr,
    /// Resolved EL0/EL1 permissions.
    pub perms: PagePerms,
    /// ASID the entry was created under (ignored if `global`).
    pub asid: u16,
    /// `nG == 0` in the descriptor: valid for every ASID.
    pub global: bool,
    /// log2 size of the original mapping (14 for a page, 25 for a 32 MiB block).
    pub map_shift: u8,
}

impl TlbEntry {
    /// Creates an entry for the 16 KiB page containing `va`, which maps to the page
    /// containing `pa`. Both addresses are aligned down; `map_shift` is clamped to
    /// `TLB_PAGE_SHIFT..=63`.
    #[must_use]
    pub fn new(
        va: GuestVirtAddr,
        pa: GuestPhysAddr,
        perms: PagePerms,
        asid: u16,
        global: bool,
        map_shift: u8,
    ) -> Self {
        // TLB_PAGE_SHIFT is 14 and always fits in a u8.
        let min_shift = TLB_PAGE_SHIFT as u8;
        Self {
            va_page: va.align_down(PAGE_SIZE_16K),
            pa_page: pa.align_down(PAGE_SIZE_16K),
            perms,
            asid,
            global,
            map_shift: map_shift.clamp(min_shift, 63),
        }
    }

    fn matches(&self, va_page: u64, asid: u16) -> bool {
        self.va_page.get() == va_page && (self.global || self.asid == asid)
    }
}

/// Direct-mapped software TLB indexed by virtual page number.
#[derive(Debug, Clone)]
pub struct Tlb {
    slots: Box<[Option<TlbEntry>]>,
    mask: usize,
    /// Closed VA interval `[lo, hi]` covering every block mapping inserted since the
    /// last full flush (see the module documentation).
    blocks: Option<BlockSpan>,
}

impl Tlb {
    /// Default number of entries (16 MiB of reach with 16 KiB pages).
    pub const DEFAULT_ENTRIES: usize = 1024;

    /// Creates an empty TLB. `entries` is rounded up to a power of two (minimum 1).
    #[must_use]
    pub fn new(entries: usize) -> Self {
        let n = entries.max(1).next_power_of_two();
        Self {
            slots: vec![None; n].into_boxed_slice(),
            mask: n - 1,
            blocks: None,
        }
    }

    /// Number of slots.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Number of currently valid entries (O(n), for tests and statistics).
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Whether no entry is valid.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn index(&self, va: u64) -> usize {
        // Truncation is intended: only the low bits select the slot.
        (va >> TLB_PAGE_SHIFT) as usize & self.mask
    }

    /// Inserts `entry`, replacing whatever occupied its slot.
    pub fn insert(&mut self, entry: TlbEntry) {
        let va = entry.va_page.get();
        if u32::from(entry.map_shift) > TLB_PAGE_SHIFT {
            self.note_block(va, u32::from(entry.map_shift));
        }
        let i = self.index(va);
        if let Some(slot) = self.slots.get_mut(i) {
            *slot = Some(entry);
        }
    }

    /// Looks up the entry for `va` under `asid`, ignoring permissions.
    #[must_use]
    pub fn lookup_entry(&self, va: GuestVirtAddr, asid: u16) -> Option<&TlbEntry> {
        let page = va.get() & !PAGE_MASK;
        self.slots
            .get(self.index(page))
            .and_then(Option::as_ref)
            .filter(|e| e.matches(page, asid))
    }

    /// Translates `va` for `access` under `asid`.
    ///
    /// Returns `None` on a miss *or* if the cached permissions deny the access; the
    /// caller then takes the slow path (page-table walk), which either refills the
    /// entry or raises the precise architectural fault.
    #[must_use]
    pub fn lookup(&self, va: GuestVirtAddr, asid: u16, access: Access) -> Option<GuestPhysAddr> {
        let e = self.lookup_entry(va, asid)?;
        if !e.perms.allows(access) {
            return None;
        }
        Some(GuestPhysAddr(
            e.pa_page.get() | va.page_offset(PAGE_SIZE_16K),
        ))
    }

    /// Invalidates everything (`TLBI VMALLE1`).
    pub fn flush_all(&mut self) {
        self.slots.fill(None);
        self.blocks = None;
    }

    /// Invalidates all non-global entries of `asid` (`TLBI ASIDE1`).
    pub fn flush_asid(&mut self, asid: u16) {
        for slot in &mut *self.slots {
            if slot.is_some_and(|e| !e.global && e.asid == asid) {
                *slot = None;
            }
        }
    }

    /// Invalidates the translation of `va` for `asid`, including a global one
    /// (`TLBI VAE1`).
    pub fn flush_va(&mut self, va: GuestVirtAddr, asid: u16) {
        self.flush_va_where(va, |e| e.global || e.asid == asid);
    }

    /// Invalidates the translation of `va` for every ASID (`TLBI VAAE1`).
    pub fn flush_va_all_asids(&mut self, va: GuestVirtAddr) {
        self.flush_va_where(va, |_| true);
    }

    fn flush_va_where(&mut self, va: GuestVirtAddr, pred: impl Fn(&TlbEntry) -> bool) {
        let page = va.get() & !PAGE_MASK;
        if self.blocks.is_some_and(|b| b.contains(page)) {
            // The VA may belong to a block mapping whose fragments sit in other slots.
            // Invalidating everything is a permitted superset of the required effect.
            self.flush_all();
            return;
        }
        let i = self.index(page);
        if let Some(slot) = self.slots.get_mut(i)
            && slot.is_some_and(|e| e.va_page.get() == page && pred(&e))
        {
            *slot = None;
        }
    }

    /// Records that a fragment of the block mapping of size `2^shift` containing `va`
    /// is now cached.
    fn note_block(&mut self, va: u64, shift: u32) {
        // `shift` comes from `TlbEntry::map_shift`, which is clamped to `14..=63`.
        let size_minus_one = (1u64 << shift.min(63)) - 1;
        let lo = va & !size_minus_one;
        // `lo` is aligned to the block size, so `lo + size - 1` cannot overflow.
        let hi = lo | size_minus_one;
        self.blocks = Some(match self.blocks {
            None => BlockSpan { lo, hi },
            Some(b) => BlockSpan {
                lo: b.lo.min(lo),
                hi: b.hi.max(hi),
            },
        });
    }
}

/// Closed VA interval `[lo, hi]` (inclusive, so the top of the address space is
/// representable without overflow).
#[derive(Debug, Clone, Copy)]
struct BlockSpan {
    lo: u64,
    hi: u64,
}

impl BlockSpan {
    fn contains(self, va: u64) -> bool {
        self.lo <= va && va <= self.hi
    }
}

impl Default for Tlb {
    fn default() -> Self {
        Self::new(Self::DEFAULT_ENTRIES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmu::{AccessKind, Privilege};

    const R0: Access = Access::new(AccessKind::Read, Privilege::Unprivileged);
    const W0: Access = Access::new(AccessKind::Write, Privilege::Unprivileged);
    const RW0: PagePerms =
        PagePerms::from_bits_truncate(PagePerms::EL0_READ.bits() | PagePerms::EL0_WRITE.bits());

    fn page(va: u64, pa: u64, asid: u16, global: bool) -> TlbEntry {
        TlbEntry::new(GuestVirtAddr(va), GuestPhysAddr(pa), RW0, asid, global, 14)
    }

    #[test]
    fn insert_lookup_keeps_page_offset() {
        let mut tlb = Tlb::new(64);
        tlb.insert(page(0x1_0000_4123, 0x8000_c000, 7, false));
        assert_eq!(
            tlb.lookup(GuestVirtAddr(0x1_0000_7ffc), 7, R0),
            Some(GuestPhysAddr(0x8000_fffc))
        );
        // Next 16K page is a miss.
        assert_eq!(tlb.lookup(GuestVirtAddr(0x1_0000_8000), 7, R0), None);
        // Other ASID is a miss for a non-global entry.
        assert_eq!(tlb.lookup(GuestVirtAddr(0x1_0000_4000), 8, R0), None);
    }

    #[test]
    fn permissions_are_checked() {
        let mut tlb = Tlb::new(64);
        let ro = TlbEntry::new(
            GuestVirtAddr(0x4000),
            GuestPhysAddr(0x4000),
            PagePerms::EL0_READ,
            1,
            false,
            14,
        );
        tlb.insert(ro);
        assert!(tlb.lookup(GuestVirtAddr(0x4000), 1, R0).is_some());
        assert!(tlb.lookup(GuestVirtAddr(0x4000), 1, W0).is_none());
        assert!(tlb.lookup_entry(GuestVirtAddr(0x4000), 1).is_some());
    }

    #[test]
    fn global_entries_match_any_asid() {
        let mut tlb = Tlb::new(64);
        tlb.insert(page(0xffff_fe00_0000_0000, 0x4000, 0, true));
        assert!(
            tlb.lookup(GuestVirtAddr(0xffff_fe00_0000_0010), 1234, R0)
                .is_some()
        );
    }

    #[test]
    fn direct_mapped_conflict_evicts() {
        let mut tlb = Tlb::new(4);
        let stride = 4 * PAGE_SIZE_16K; // same slot
        tlb.insert(page(0x10_0000, 0x1000_0000, 1, false));
        tlb.insert(page(0x10_0000 + stride, 0x2000_0000, 1, false));
        assert!(tlb.lookup(GuestVirtAddr(0x10_0000), 1, R0).is_none());
        assert_eq!(
            tlb.lookup(GuestVirtAddr(0x10_0000 + stride), 1, R0),
            Some(GuestPhysAddr(0x2000_0000))
        );
        assert_eq!(tlb.len(), 1);
    }

    #[test]
    fn flush_asid_spares_globals_and_other_asids() {
        let mut tlb = Tlb::new(64);
        tlb.insert(page(0x0000, 0x0, 1, false));
        tlb.insert(page(0x4000, 0x0, 2, false));
        tlb.insert(page(0x8000, 0x0, 1, true));
        tlb.flush_asid(1);
        assert!(tlb.lookup(GuestVirtAddr(0x0000), 1, R0).is_none());
        assert!(tlb.lookup(GuestVirtAddr(0x4000), 2, R0).is_some());
        assert!(tlb.lookup(GuestVirtAddr(0x8000), 1, R0).is_some());
    }

    #[test]
    fn flush_va_semantics() {
        let mut tlb = Tlb::new(64);
        tlb.insert(page(0x4000, 0x0, 1, false));
        tlb.insert(page(0x8000, 0x0, 0, true));
        // Wrong ASID leaves a non-global entry alone.
        tlb.flush_va(GuestVirtAddr(0x4abc), 2);
        assert!(tlb.lookup(GuestVirtAddr(0x4000), 1, R0).is_some());
        // Matching ASID removes it; offset within the page is irrelevant.
        tlb.flush_va(GuestVirtAddr(0x7fff), 1);
        assert!(tlb.lookup(GuestVirtAddr(0x4000), 1, R0).is_none());
        // Global entries are removed by VAE1 with any ASID.
        tlb.flush_va(GuestVirtAddr(0x8000), 99);
        assert!(tlb.is_empty());

        tlb.insert(page(0xc000, 0x0, 5, false));
        tlb.flush_va_all_asids(GuestVirtAddr(0xc000));
        assert!(tlb.is_empty());
    }

    #[test]
    fn flush_all_empties() {
        let mut tlb = Tlb::default();
        assert_eq!(tlb.capacity(), Tlb::DEFAULT_ENTRIES);
        for i in 0..100u64 {
            tlb.insert(page(i << 14, i << 14, 1, i % 2 == 0));
        }
        assert_eq!(tlb.len(), 100);
        tlb.flush_all();
        assert!(tlb.is_empty());
    }

    #[test]
    fn block_fragments_are_flushed_by_any_va_in_block() {
        // A 32 MiB level-2 block, cached as two 16K fragments in different slots.
        let mut tlb = Tlb::new(64);
        let block_va = 0x4000_0000u64;
        for off in [0x0u64, 0x1_4000] {
            tlb.insert(TlbEntry::new(
                GuestVirtAddr(block_va + off),
                GuestPhysAddr(0x8000_0000 + off),
                RW0,
                1,
                false,
                25,
            ));
        }
        // An unrelated small page outside the block range.
        tlb.insert(page(0x10_0000_8000, 0x0, 1, false));
        assert_eq!(tlb.len(), 3);
        // Invalidate a VA inside the block that is not itself cached: all fragments
        // must go (conservatively the whole TLB).
        tlb.flush_va(GuestVirtAddr(block_va + 0x1ff_0000), 1);
        assert!(tlb.lookup(GuestVirtAddr(block_va), 1, R0).is_none());
        assert!(
            tlb.lookup(GuestVirtAddr(block_va + 0x1_4000), 1, R0)
                .is_none()
        );
        assert!(tlb.lookup(GuestVirtAddr(0x10_0000_8000), 1, R0).is_none());
    }

    #[test]
    fn small_page_flush_does_not_trigger_full_flush() {
        let mut tlb = Tlb::new(64);
        tlb.insert(TlbEntry::new(
            GuestVirtAddr(0x4000_0000),
            GuestPhysAddr(0),
            RW0,
            1,
            false,
            25,
        ));
        tlb.insert(page(0x10_0000_8000, 0x0, 1, false));
        tlb.insert(page(0x10_0000_c000, 0x0, 1, false));
        tlb.flush_va(GuestVirtAddr(0x10_0000_8000), 1);
        assert!(tlb.lookup(GuestVirtAddr(0x10_0000_8000), 1, R0).is_none());
        assert!(tlb.lookup(GuestVirtAddr(0x10_0000_c000), 1, R0).is_some());
        assert!(tlb.lookup(GuestVirtAddr(0x4000_0000), 1, R0).is_some());
    }

    #[test]
    fn large_range_widens_to_cover_both_blocks() {
        let mut tlb = Tlb::new(64);
        tlb.insert(TlbEntry::new(
            GuestVirtAddr(0x4000_0000),
            GuestPhysAddr(0),
            RW0,
            1,
            false,
            25,
        ));
        tlb.insert(TlbEntry::new(
            GuestVirtAddr(0x8000_0000),
            GuestPhysAddr(0),
            RW0,
            1,
            false,
            25,
        ));
        tlb.insert(page(0x2_0000_0000, 0x0, 1, false));
        // Second block is covered by the widened range → full flush.
        tlb.flush_va(GuestVirtAddr(0x8100_0000), 1);
        assert!(tlb.is_empty());
    }

    #[test]
    fn block_span_at_top_of_address_space_does_not_overflow() {
        let mut tlb = Tlb::new(64);
        // A 32 MiB kernel block ending at the last byte of the VA space, and the
        // largest representable mapping size.
        for shift in [25u8, 63] {
            tlb.insert(TlbEntry::new(
                GuestVirtAddr(0xffff_ffff_fe00_0000),
                GuestPhysAddr(0),
                RW0,
                0,
                true,
                shift,
            ));
            tlb.insert(page(0x8000, 0x0, 1, false));
            tlb.flush_va(GuestVirtAddr(u64::MAX), 1);
            assert!(tlb.is_empty(), "shift {shift}");
        }
        // Outside every recorded block only the own slot is touched.
        tlb.insert(TlbEntry::new(
            GuestVirtAddr(0xffff_ffff_fe00_0000),
            GuestPhysAddr(0),
            RW0,
            0,
            true,
            25,
        ));
        tlb.insert(page(0x8000, 0x0, 1, false));
        tlb.flush_va(GuestVirtAddr(0xffff_ffff_fdff_c000), 1);
        assert_eq!(tlb.len(), 2);
    }

    #[test]
    fn entry_constructor_normalizes() {
        let e = TlbEntry::new(
            GuestVirtAddr(0x7fff),
            GuestPhysAddr(0x1_2345),
            RW0,
            0,
            false,
            3,
        );
        assert_eq!(e.va_page, GuestVirtAddr(0x4000));
        assert_eq!(e.pa_page, GuestPhysAddr(0x1_0000));
        assert_eq!(e.map_shift, 14);
        assert_eq!(Tlb::new(0).capacity(), 1);
        assert_eq!(Tlb::new(100).capacity(), 128);
    }
}
