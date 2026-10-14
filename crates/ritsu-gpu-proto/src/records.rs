//! Zero-copy lists of fixed-size sub-records inside a payload (attachment lists, vertex
//! attributes, …).
//!
//! A [`RecordList`] borrows the validated bytes from the stream instead of copying the entries
//! into a fixed-size array, so a decoded [`crate::Command`] stays small no matter how many
//! entries the record carries (memory use of `decode_stream` grows with the input, never with a
//! guest-declared count, and never by more than a small constant per record).
//!
//! A list can only be created through [`RecordList::parse`], which validates every entry
//! (reserved bytes, wire-level field rules) and, for indexed entries, that indices are strictly
//! increasing and below the entry type's limit. Iterating a list therefore never fails.

use core::fmt;
use core::marker::PhantomData;

use crate::error::ProtoError;
use crate::reader::Dec;

pub(crate) mod sealed {
    use crate::error::ProtoError;
    use crate::reader::Dec;

    /// Opaque decoder handle passed to [`Decode::decode`] (contents crate-private).
    #[derive(Debug)]
    pub struct Input<'s, 'a>(pub(crate) &'s mut Dec<'a>);

    /// Decoder half of [`super::WireRecord`]; sealed, so only this crate defines entry types.
    pub trait Decode: Sized {
        /// Decodes and wire-validates one entry (exactly `LEN` bytes available).
        fn decode(input: Input<'_, '_>) -> Result<Self, ProtoError>;
    }
}

/// A fixed-size entry of a [`RecordList`] (sealed: implemented only by this crate's entry
/// types).
pub trait WireRecord: sealed::Decode + Copy {
    /// Encoded size in bytes.
    const LEN: usize;
    /// Exclusive upper bound of [`WireRecord::index`] (ignored for unindexed entries).
    const INDEX_LIMIT: u8;
    /// Most entries a list may hold.
    const MAX_ENTRIES: usize;
    /// Field name used in [`ProtoError::InvalidField`] for list-level errors.
    const FIELD: &'static str;

    /// Slot index of an indexed entry (`None` for dense lists without explicit indices).
    fn index(&self) -> Option<u8>;

    /// Appends exactly [`WireRecord::LEN`] bytes.
    fn encode(&self, out: &mut Vec<u8>);
}

/// A validated list of [`WireRecord`]s borrowed from a stream.
#[derive(Clone, Copy)]
pub struct RecordList<'a, R> {
    bytes: &'a [u8],
    _entries: PhantomData<R>,
}

impl<R> PartialEq for RecordList<'_, R> {
    /// Byte-wise equality (so lists with NaN clear values still compare equal to themselves).
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl<'a, R: WireRecord> RecordList<'a, R> {
    /// The empty list.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            bytes: &[],
            _entries: PhantomData,
        }
    }

    /// Validates `bytes` as a list of `R` (standalone use: error offsets are relative to
    /// `bytes`).
    ///
    /// # Errors
    ///
    /// [`ProtoError::InvalidField`] / [`ProtoError::ReservedNonZero`] for a length that is not
    /// a multiple of [`WireRecord::LEN`], more than [`WireRecord::MAX_ENTRIES`] entries, an
    /// invalid entry, or indices that are not strictly
    /// increasing or reach [`WireRecord::INDEX_LIMIT`].
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ProtoError> {
        Self::parse_at(bytes, 0, 0)
    }

    /// Validates `bytes`, which start `base` bytes into the payload of the record at stream
    /// offset `offset`.
    pub(crate) fn parse_at(
        bytes: &'a [u8],
        offset: usize,
        base: usize,
    ) -> Result<Self, ProtoError> {
        let mut d = Dec::with_base(bytes, offset, base);
        if R::LEN == 0
            || !bytes.len().is_multiple_of(R::LEN)
            || bytes.len() / R::LEN > R::MAX_ENTRIES
        {
            return Err(d.err(R::FIELD));
        }
        let mut last: Option<u8> = None;
        for _ in 0..bytes.len() / R::LEN {
            let entry = R::decode(sealed::Input(&mut d))?;
            if let Some(i) = entry.index() {
                if i >= R::INDEX_LIMIT || last.is_some_and(|l| i <= l) {
                    return Err(d.err(R::FIELD));
                }
                last = Some(i);
            }
        }
        d.finish()?;
        Ok(Self {
            bytes,
            _entries: PhantomData,
        })
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len() / R::LEN
    }

    /// Whether the list has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The encoded entries (exactly as received).
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The entries in order.
    pub fn iter(&self) -> impl Iterator<Item = R> + 'a {
        let bytes = self.bytes;
        // Every chunk was validated by `parse_at`, so decoding cannot fail; `filter_map` keeps
        // this free of panics regardless.
        bytes
            .chunks_exact(R::LEN)
            .filter_map(|chunk| R::decode(sealed::Input(&mut Dec::new(chunk, 0))).ok())
    }
}

impl<R: WireRecord + fmt::Debug> fmt::Debug for RecordList<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// Encodes entries for [`RecordList::parse`] (reference encoder, tests, seed generation).
#[must_use]
pub fn encode_records<R: WireRecord>(records: &[R]) -> Vec<u8> {
    let mut out = Vec::with_capacity(records.len().saturating_mul(R::LEN));
    for r in records {
        r.encode(&mut out);
    }
    out
}
