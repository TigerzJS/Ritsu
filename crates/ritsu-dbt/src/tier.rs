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
/// Planned bits (WP-DBT-07/-08): current EL, `PSTATE.SP`, stage-1 MMU on/off,
/// `TBI0/TBI1`, PAC enable bits from `SCTLR_EL1.{EnIA,EnIB,EnDA,EnDB}`, `PSTATE.PAN`,
/// alignment-check enable, FP access enabled (`CPACR_EL1.FPEN`), single-step. A change
/// of any of these selects different translations instead of invalidating.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct TbFlags(pub u32);

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
