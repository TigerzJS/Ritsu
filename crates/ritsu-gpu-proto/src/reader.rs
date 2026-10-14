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

    /// Returns all remaining bytes.
    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let rest = self.bytes.get(self.pos..).unwrap_or(&[]);
        self.pos = self.bytes.len();
        rest
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
}

impl<'a> Dec<'a> {
    pub(crate) fn new(payload: &'a [u8], offset: usize) -> Self {
        Self {
            cur: Cursor::new(payload),
            offset,
        }
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
    /// Reads a non-zero object id.
    pub(crate) fn id(&mut self, f: &'static str) -> Result<ObjectId, ProtoError> {
        let raw = self.u32(f)?;
        ObjectId::new(raw).ok_or_else(|| self.err(f))
    }
    /// Reads `n` reserved bytes that must be zero.
    pub(crate) fn reserved(&mut self, n: usize) -> Result<(), ProtoError> {
        for _ in 0..n {
            // Record offset + record header + position inside payload.
            let at = self.offset + crate::RECORD_HEADER_LEN + self.cur.pos();
            if self.u8("reserved")? != 0 {
                return Err(ProtoError::ReservedNonZero { offset: at });
            }
        }
        Ok(())
    }
    /// Fails if payload bytes remain (guards against layout/length-rule mismatches).
    pub(crate) fn finish(mut self) -> Result<(), ProtoError> {
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
