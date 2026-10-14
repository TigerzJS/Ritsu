//! Generational handle tables.
//!
//! Every host-side GPU object (buffer, texture, event, pipeline, …) lives in a
//! [`HandleTable`]. Handles carry a slot index and a generation; a handle whose object was
//! removed (and whose slot may have been reused) is detected as stale instead of silently
//! aliasing a different object. Lookups never panic, whatever bits a handle contains — the
//! raw form of a handle may originate from the guest.

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

/// Default upper bound on live + retired slots per table (about one million objects).
pub const DEFAULT_MAX_SLOTS: u32 = 1 << 20;

/// Untyped handle: slot index + generation. Generation 0 is never valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RawHandle {
    index: u32,
    generation: u32,
}

impl RawHandle {
    /// Builds a raw handle from parts. Any value is allowed; validity is checked on lookup.
    #[must_use]
    pub const fn from_parts(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// Slot index.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Generation of the slot when the handle was issued.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the handle as `generation << 32 | index`.
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    /// Unpacks a handle produced by [`RawHandle::to_bits`] (or arbitrary bits).
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            index: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }
}

/// Typed handle into a [`HandleTable<T>`].
pub struct Handle<T> {
    raw: RawHandle,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    /// Re-types a raw handle. Validity is checked by the table on lookup.
    #[must_use]
    pub const fn from_raw(raw: RawHandle) -> Self {
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// The untyped handle.
    #[must_use]
    pub const fn raw(self) -> RawHandle {
        self.raw
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Handle<T> {}
impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<T> Eq for Handle<T> {}
impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}
impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({}v{})", self.raw.index, self.raw.generation)
    }
}

/// Lookup or insertion failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HandleError {
    /// The index does not name any slot.
    #[error("handle index {index} out of range")]
    OutOfRange {
        /// Offending index.
        index: u32,
    },
    /// The slot exists, but the object was removed (or replaced by a newer one).
    #[error("stale handle {index}v{generation}")]
    Stale {
        /// Slot index.
        index: u32,
        /// Generation carried by the handle.
        generation: u32,
    },
    /// The table reached its slot limit.
    #[error("handle table full ({max_slots} slots)")]
    Full {
        /// Configured limit.
        max_slots: u32,
    },
}

#[derive(Debug)]
struct Slot<T> {
    /// Current generation; 0 marks a retired slot that is never reused.
    generation: u32,
    value: Option<T>,
}

/// Slot map with generation-checked handles and O(1) insert/get/remove.
///
/// Freed slots are reused LIFO with an incremented generation. When a slot's generation
/// would wrap around, the slot is retired permanently, so a handle can never become valid
/// again for a different object (no ABA).
#[derive(Debug)]
pub struct HandleTable<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
    max_slots: u32,
}

impl<T> Default for HandleTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> HandleTable<T> {
    /// Empty table with [`DEFAULT_MAX_SLOTS`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_slots(DEFAULT_MAX_SLOTS)
    }

    /// Empty table that never grows beyond `max_slots` slots (bounds guest-driven growth).
    #[must_use]
    pub fn with_max_slots(max_slots: u32) -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
            max_slots,
        }
    }

    /// Number of live objects.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no objects are live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Inserts `value` and returns its handle.
    ///
    /// # Errors
    ///
    /// [`HandleError::Full`] when no free slot exists and the limit is reached.
    pub fn insert(&mut self, value: T) -> Result<Handle<T>, HandleError> {
        if let Some(index) = self.free.pop()
            && let Some(slot) = self.slots.get_mut(index as usize)
        {
            slot.value = Some(value);
            self.len += 1;
            return Ok(Handle::from_raw(RawHandle::from_parts(
                index,
                slot.generation,
            )));
        }
        let index = u32::try_from(self.slots.len())
            .ok()
            .filter(|&i| i < self.max_slots)
            .ok_or(HandleError::Full {
                max_slots: self.max_slots,
            })?;
        self.slots.push(Slot {
            generation: 1,
            value: Some(value),
        });
        self.len += 1;
        Ok(Handle::from_raw(RawHandle::from_parts(index, 1)))
    }

    fn slot(&self, h: Handle<T>) -> Result<&Slot<T>, HandleError> {
        let RawHandle { index, generation } = h.raw;
        let slot = self
            .slots
            .get(index as usize)
            .ok_or(HandleError::OutOfRange { index })?;
        if slot.generation != generation || slot.value.is_none() {
            return Err(HandleError::Stale { index, generation });
        }
        Ok(slot)
    }

    /// Shared access to a live object.
    ///
    /// # Errors
    ///
    /// [`HandleError::OutOfRange`] or [`HandleError::Stale`].
    pub fn get(&self, h: Handle<T>) -> Result<&T, HandleError> {
        let slot = self.slot(h)?;
        slot.value.as_ref().ok_or(HandleError::Stale {
            index: h.raw.index,
            generation: h.raw.generation,
        })
    }

    /// Exclusive access to a live object.
    ///
    /// # Errors
    ///
    /// [`HandleError::OutOfRange`] or [`HandleError::Stale`].
    pub fn get_mut(&mut self, h: Handle<T>) -> Result<&mut T, HandleError> {
        self.slot(h)?;
        let RawHandle { index, generation } = h.raw;
        self.slots
            .get_mut(index as usize)
            .and_then(|s| s.value.as_mut())
            .ok_or(HandleError::Stale { index, generation })
    }

    /// Whether `h` refers to a live object.
    #[must_use]
    pub fn contains(&self, h: Handle<T>) -> bool {
        self.slot(h).is_ok()
    }

    /// Removes and returns the object; the handle (and all copies) become stale.
    ///
    /// # Errors
    ///
    /// [`HandleError::OutOfRange`] or [`HandleError::Stale`].
    pub fn remove(&mut self, h: Handle<T>) -> Result<T, HandleError> {
        self.slot(h)?;
        let RawHandle { index, generation } = h.raw;
        let slot = self
            .slots
            .get_mut(index as usize)
            .ok_or(HandleError::OutOfRange { index })?;
        let value = slot
            .value
            .take()
            .ok_or(HandleError::Stale { index, generation })?;
        self.len -= 1;
        match slot.generation.checked_add(1) {
            Some(g) => {
                slot.generation = g;
                self.free.push(index);
            }
            // Generation exhausted: retire the slot forever.
            None => slot.generation = 0,
        }
        Ok(value)
    }

    /// Iterates over live objects with their handles.
    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            let v = s.value.as_ref()?;
            let index = u32::try_from(i).ok()?;
            Some((
                Handle::from_raw(RawHandle::from_parts(index, s.generation)),
                v,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove() {
        let mut t = HandleTable::new();
        let a = t.insert("a").unwrap();
        let b = t.insert("b").unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(*t.get(a).unwrap(), "a");
        *t.get_mut(b).unwrap() = "B";
        assert_eq!(*t.get(b).unwrap(), "B");
        assert_eq!(t.remove(a).unwrap(), "a");
        assert_eq!(t.len(), 1);
        assert!(!t.contains(a));
        assert!(t.contains(b));
    }

    #[test]
    fn stale_handle_detected_after_slot_reuse() {
        let mut t = HandleTable::new();
        let a = t.insert(1).unwrap();
        t.remove(a).unwrap();
        let b = t.insert(2).unwrap();
        assert_eq!(a.raw().index(), b.raw().index(), "slot is reused");
        assert_ne!(a, b);
        assert_eq!(
            t.get(a),
            Err(HandleError::Stale {
                index: a.raw().index(),
                generation: 1
            })
        );
        assert_eq!(
            t.remove(a),
            Err(HandleError::Stale {
                index: 0,
                generation: 1
            })
        );
        assert!(t.get_mut(a).is_err());
        assert_eq!(*t.get(b).unwrap(), 2);
    }

    #[test]
    fn double_remove_is_error() {
        let mut t = HandleTable::new();
        let a = t.insert(()).unwrap();
        t.remove(a).unwrap();
        assert!(t.remove(a).is_err());
        assert!(t.is_empty());
    }

    #[test]
    fn arbitrary_raw_handles_never_panic() {
        let mut t = HandleTable::new();
        let a = t.insert(5u8).unwrap();
        for bits in [0u64, 1, u64::MAX, a.raw().to_bits() ^ (1 << 32), 2 << 32] {
            let h = Handle::<u8>::from_raw(RawHandle::from_bits(bits));
            assert!(t.get(h).is_err(), "bits {bits:#x}");
            assert!(t.remove(h).is_err());
        }
        // Generation 0 handle for a live slot is rejected.
        let zero_gen = Handle::<u8>::from_raw(RawHandle::from_parts(0, 0));
        assert!(t.get(zero_gen).is_err());
        assert_eq!(*t.get(a).unwrap(), 5);
    }

    #[test]
    fn raw_bits_round_trip() {
        let r = RawHandle::from_parts(0xDEAD, 0xBEEF);
        assert_eq!(RawHandle::from_bits(r.to_bits()), r);
    }

    #[test]
    fn capacity_limit() {
        let mut t = HandleTable::with_max_slots(2);
        let a = t.insert(0).unwrap();
        t.insert(1).unwrap();
        assert_eq!(t.insert(2), Err(HandleError::Full { max_slots: 2 }));
        t.remove(a).unwrap();
        assert!(t.insert(3).is_ok(), "freed slot is reusable");
    }

    #[test]
    fn exhausted_generation_retires_slot() {
        let mut t = HandleTable::new();
        let a = t.insert(1).unwrap();
        t.slots[0].generation = u32::MAX;
        let a_max = Handle::from_raw(RawHandle::from_parts(a.raw().index(), u32::MAX));
        t.remove(a_max).unwrap();
        assert_eq!(t.slots[0].generation, 0);
        let b = t.insert(2).unwrap();
        assert_eq!(b.raw().index(), 1, "retired slot is not reused");
        let zero_gen = Handle::<i32>::from_raw(RawHandle::from_parts(0, 0));
        assert!(t.get(zero_gen).is_err());
    }

    #[test]
    fn iter_yields_live_objects() {
        let mut t = HandleTable::new();
        let a = t.insert('a').unwrap();
        let b = t.insert('b').unwrap();
        t.remove(a).unwrap();
        let live: Vec<_> = t.iter().collect();
        assert_eq!(live, vec![(b, &'b')]);
    }
}
