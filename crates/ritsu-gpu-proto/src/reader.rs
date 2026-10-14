//! Bounds-checked little-endian reading helpers. Never panic, never index unchecked.

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
