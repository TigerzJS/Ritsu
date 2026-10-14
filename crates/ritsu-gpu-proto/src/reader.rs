//! Bounds-checked little-endian reading helpers. Never panic, never index unchecked.

use crate::command::ObjectId;
use crate::error::ProtoError;

/// Reads `N` bytes at `at`, or `None` if out of range (including offset overflow).
pub(crate) fn array_at<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    let end = at.checked_add(N)?;
    bytes.get(at..end)?.try_into().ok()
}

/// Reads a little-endian `u16` at `at`.
pub(crate) fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    array_at(bytes, at).map(u16::from_le_bytes)
}

/// Reads a little-endian `u32` at `at`.
pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    array_at(bytes, at).map(u32::from_le_bytes)
}

/// Sequential reader over a payload whose length has already been checked against the
/// opcode's [`crate::PayloadLen`] rule. Reads still fail gracefully instead of panicking.
#[derive(Debug)]
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let v = array_at(self.bytes, self.pos)?;
        self.pos += N;
        Some(v)
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|[b]| b)
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }

    pub(crate) fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }

    pub(crate) fn f32(&mut self) -> Option<f32> {
        self.take().map(f32::from_le_bytes)
    }

    pub(crate) fn f64(&mut self) -> Option<f64> {
        self.take().map(f64::from_le_bytes)
    }

    /// Returns the next `n` bytes, or `None` if fewer remain (nothing is consumed then).
    pub(crate) fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let v = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(v)
    }

    /// Returns all remaining bytes.
    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let rest = self.bytes.get(self.pos..).unwrap_or(&[]);
        self.pos = self.bytes.len();
        rest
    }

    /// Number of bytes not yet consumed.
    pub(crate) fn rest_len(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// Current position within the payload.
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }
}

/// Payload decoding context: turns `None` from the cursor into a positioned error.
#[derive(Debug)]
pub(crate) struct Dec<'a> {
    pub(crate) cur: Cursor<'a>,
    /// Offset of the record header within the stream.
    pub(crate) offset: usize,
    /// Position of `cur`'s first byte within the record payload (non-zero for sub-decoders
    /// over a part of the payload), so reserved-byte offsets stay stream-relative.
    base: usize,
}

impl<'a> Dec<'a> {
    pub(crate) fn new(payload: &'a [u8], offset: usize) -> Self {
        Self::with_base(payload, offset, 0)
    }
    /// Decoder over a part of a payload that starts `base` bytes into the payload.
    pub(crate) fn with_base(bytes: &'a [u8], offset: usize, base: usize) -> Self {
        Self {
            cur: Cursor::new(bytes),
            offset,
            base,
        }
    }
    /// Position of the next byte within the record payload.
    pub(crate) fn payload_pos(&self) -> usize {
        self.base + self.cur.pos()
    }
    pub(crate) fn err(&self, field: &'static str) -> ProtoError {
        ProtoError::InvalidField {
            offset: self.offset,
            field,
        }
    }
    pub(crate) fn u8(&mut self, f: &'static str) -> Result<u8, ProtoError> {
        self.cur.u8().ok_or_else(|| self.err(f))
    }
    pub(crate) fn u16(&mut self, f: &'static str) -> Result<u16, ProtoError> {
        self.cur.u16().ok_or_else(|| self.err(f))
    }
    pub(crate) fn u32(&mut self, f: &'static str) -> Result<u32, ProtoError> {
        self.cur.u32().ok_or_else(|| self.err(f))
    }
    pub(crate) fn u64(&mut self, f: &'static str) -> Result<u64, ProtoError> {
        self.cur.u64().ok_or_else(|| self.err(f))
    }
    pub(crate) fn f32(&mut self, f: &'static str) -> Result<f32, ProtoError> {
        self.cur.f32().ok_or_else(|| self.err(f))
    }
    pub(crate) fn f64(&mut self, f: &'static str) -> Result<f64, ProtoError> {
        self.cur.f64().ok_or_else(|| self.err(f))
    }
    /// Reads an object id where 0 means "none" (`nil` in Metal).
    pub(crate) fn opt_id(&mut self, f: &'static str) -> Result<Option<ObjectId>, ProtoError> {
        Ok(ObjectId::new(self.u32(f)?))
    }
    /// Reads a flag byte; bits outside `known` are a wire error.
    pub(crate) fn flags(&mut self, f: &'static str, known: u8) -> Result<u8, ProtoError> {
        let v = self.u8(f)?;
        if v & !known != 0 {
            return Err(self.err(f));
        }
        Ok(v)
    }
    /// Reads a non-zero object id.
    pub(crate) fn id(&mut self, f: &'static str) -> Result<ObjectId, ProtoError> {
        let raw = self.u32(f)?;
        ObjectId::new(raw).ok_or_else(|| self.err(f))
    }
    /// Reads `n` reserved bytes that must be zero.
    pub(crate) fn reserved(&mut self, n: usize) -> Result<(), ProtoError> {
        let start = self.payload_pos();
        let bytes = self.cur.bytes(n).ok_or_else(|| self.err("reserved"))?;
        match bytes.iter().position(|&b| b != 0) {
            None => Ok(()),
            // Record offset + record header + position inside the payload.
            Some(i) => Err(ProtoError::ReservedNonZero {
                offset: self.offset + crate::RECORD_HEADER_LEN + start + i,
            }),
        }
    }
    /// Fails if payload bytes remain (guards against layout/length-rule mismatches).
    pub(crate) fn finish(&mut self) -> Result<(), ProtoError> {
        if self.cur.rest().is_empty() {
            Ok(())
        } else {
            Err(self.err("payload_tail"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_reads_are_none() {
        let b = [1u8, 2, 3];
        assert_eq!(u16_at(&b, 1), Some(0x0302));
        assert_eq!(u16_at(&b, 2), None);
        assert_eq!(u32_at(&b, 0), None);
        assert_eq!(u32_at(&b, usize::MAX), None);
        let mut c = Cursor::new(&b);
        assert_eq!(c.u16(), Some(0x0201));
        assert_eq!(c.u16(), None);
        assert_eq!(c.u8(), Some(3));
        assert_eq!(c.u8(), None);
        assert!(c.rest().is_empty());
    }
}
