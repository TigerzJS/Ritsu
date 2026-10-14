//! Translation tiers and the translation cache — interface sketch (M0).
//!
//! Nothing here is implemented yet; the types pin down the vocabulary that T1
//! (WP-DBT-12), T2 (WP-DBT-17) and the persistent cache (WP-DBT-18) build on. The design
//! rationale is in `docs/subsystems/dbt.md` §5 and §14.
//!
//! Key ideas:
//!
//! * Translations are keyed by **guest physical** address of the first instruction plus
//!   a [`TbFlags`] word with every piece of vCPU state the generated code was
//!   specialised on. Virtual-address aliases of the same code share one translation;
//!   lookups by virtual PC go through a small per-vCPU jump cache in front of the
//!   [`TranslationCache`].
//! * A translation never spans a 16 KiB page boundary, so invalidation and the
//!   persistent cache work per physical page.
//! * Translated code is position independent with respect to the host (helpers are
//!   called through a table reachable from the CPU-state pointer).

use crate::cpu::{CpuState, ExceptionLevel};
use crate::interp::ExecError;
use ritsu_core::{GuestPhysAddr, GuestVirtAddr};

/// The execution tiers of ADR-0006.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    /// Interpreter (reference semantics, cold code).
    T0,
    /// Template / baseline JIT: one basic block at a time, target < 1 µs per block.
    T1,
    /// Optimising region JIT (Cranelift), compiled asynchronously.
    T2,
}

/// The vCPU state a translation is specialised on (besides the code bytes).
///
/// It is also the source of the semantic layer's translation context
/// ([`crate::sem::Ctx`], R-DBT-01 P1-1): [`TbFlags::from_state`] is the **only**
/// function that derives these bits from a [`CpuState`], for T0 as well as for T1/T2
/// block keys. A change of any of these bits selects different translations instead
/// of invalidating, and every guest write that changes one of them ends the block.
///
/// Bits defined so far (WP-DBT-02) are the associated constants. Planned (WP-DBT-06/-07):
/// `PSTATE.SP`, stage-1 MMU on/off, `TBI0/TBI1`, `SCTLR_EL1.{EnDA,EnDB}`, `PSTATE.PAN`,
/// FP access enabled (`CPACR_EL1.FPEN`), single-step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct TbFlags(pub u32);

impl TbFlags {
    /// `PSTATE.EL == 1`.
    pub const EL1: u32 = 1 << 0;
    /// SP alignment check active at the current EL (`SCTLR_EL1.SA` at EL1, `SA0` at
    /// EL0).
    pub const SP_ALIGN: u32 = 1 << 1;
    /// Alignment check of normal accesses (`SCTLR_EL1.A`).
    pub const ALIGN: u32 = 1 << 2;
    /// `SCTLR_EL1.EnIA`.
    pub const PAC_IA: u32 = 1 << 3;
    /// `SCTLR_EL1.EnIB`.
    pub const PAC_IB: u32 = 1 << 4;

    /// `SCTLR_EL1` bit positions used here (Arm ARM, `SCTLR_EL1`).
    const SCTLR_A: u64 = 1 << 1;
    const SCTLR_SA: u64 = 1 << 3;
    const SCTLR_SA0: u64 = 1 << 4;
    const SCTLR_ENIB: u64 = 1 << 30;
    const SCTLR_ENIA: u64 = 1 << 31;

    /// The flags of the current state of `cpu`.
    #[must_use]
    #[inline]
    pub fn from_state(cpu: &CpuState) -> Self {
        let sctlr = cpu.sys.sctlr_el1;
        let el1 = cpu.el == ExceptionLevel::El1;
        let sp_align_bit = if el1 { Self::SCTLR_SA } else { Self::SCTLR_SA0 };
        let bit = |on: bool, flag: u32| if on { flag } else { 0 };
        Self(
            bit(el1, Self::EL1)
                | bit(sctlr & sp_align_bit != 0, Self::SP_ALIGN)
                | bit(sctlr & Self::SCTLR_A != 0, Self::ALIGN)
                | bit(sctlr & Self::SCTLR_ENIA != 0, Self::PAC_IA)
                | bit(sctlr & Self::SCTLR_ENIB != 0, Self::PAC_IB),
        )
    }
}

/// Lookup key of a translated block or region entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockKey {
    /// Guest physical address of the first instruction (primary key).
    pub pa: GuestPhysAddr,
    /// Guest virtual address of the first instruction. Translated code may embed
    /// VA-dependent values (return addresses, `ADR/ADRP` results); in persistable
    /// translations those are emitted relative to this VA (see dbt.md §11).
    pub va: GuestVirtAddr,
    /// Specialisation flags.
    pub flags: TbFlags,
}

/// Opaque handle of a translation in a [`TranslationCache`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// Why a translation could not be produced.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TranslateError {
    /// The tier cannot translate this code (yet); the caller falls back to a lower tier.
    /// This is not a guest error.
    #[error("not implemented: {0}")]
    Unimplemented(&'static str),
    /// Fetching the first instruction faulted; this *is* a guest exception and must be
    /// raised exactly as T0 would raise it.
    #[error(transparent)]
    Fetch(#[from] ExecError),
    /// The code cache is full; the caller flushes or evicts and retries.
    #[error("code cache full")]
    CacheFull,
}

/// Source of instruction words for a translator.
///
/// Implementations translate through the vCPU's MMU and record every physical page
/// touched, so the resulting translation can be registered for invalidation.
pub trait CodeSource {
    /// Fetches the instruction word at `va`.
    ///
    /// # Errors
    ///
    /// The same [`ExecError`] the interpreter would raise for this fetch.
    fn fetch(&mut self, va: GuestVirtAddr) -> Result<u32, ExecError>;
}

/// A JIT tier (T1 or T2).
///
/// Contract: a translator derives semantics only from the shared semantic layer
/// (ADR-0006); it must produce code whose observable behaviour equals T0 for every
/// input, including faults and precise exception state. Anything it cannot translate
/// exactly yields [`TranslateError::Unimplemented`], never approximate code.
pub trait Translator {
    /// Which tier this is.
    fn tier(&self) -> Tier;

    /// Translates the block or region starting at `key` and installs it in `cache`.
    ///
    /// # Errors
    ///
    /// See [`TranslateError`].
    fn translate(
        &mut self,
        key: &BlockKey,
        code: &mut dyn CodeSource,
        cache: &mut dyn TranslationCache,
    ) -> Result<BlockId, TranslateError>;
}

/// Shared store of translated code, indexed by [`BlockKey`].
///
/// One instance is shared by all vCPUs (behind its own synchronisation); invalidation
/// is by physical page, triggered by self-modifying-code detection, `IC IVAU/IALLU`
/// and DMA writes. Freed code is reclaimed only after every vCPU has passed a
/// quiescent point (epoch-based reclamation).
pub trait TranslationCache {
    /// Finds the best available translation for `key` (highest tier wins).
    fn lookup(&self, key: &BlockKey) -> Option<(BlockId, Tier)>;

    /// Registers a new translation covering the physical pages in `pages`.
    ///
    /// # Errors
    ///
    /// [`TranslateError::CacheFull`] if there is no space left.
    fn insert(
        &mut self,
        key: BlockKey,
        tier: Tier,
        pages: &[GuestPhysAddr],
    ) -> Result<BlockId, TranslateError>;

    /// Invalidates every translation that covers any byte of the 16 KiB physical page
    /// containing `page`. Returns the number of translations removed.
    fn invalidate_page(&mut self, page: GuestPhysAddr) -> usize;

    /// Invalidates everything (e.g. `IC IALLUIS` with an unknown range, or a flush after
    /// [`TranslateError::CacheFull`]).
    fn invalidate_all(&mut self);
}
