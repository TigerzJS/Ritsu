//! Decoder errors.

/// Every way a guest-provided command stream can be malformed.
///
/// Offsets are byte offsets from the start of the stream (the header starts at 0).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProtoError {
    /// The buffer is shorter than the stream header.
    #[error("stream too short for header: {len} bytes")]
    TruncatedHeader {
        /// Length of the buffer.
        len: usize,
    },

    /// The header magic does not match [`crate::MAGIC`].
    #[error("bad magic {found:#010x}")]
    BadMagic {
        /// Magic value found.
        found: u32,
    },

    /// The stream uses a protocol version this host does not speak.
    #[error("unsupported protocol version {found} (supported: {supported})")]
    UnsupportedVersion {
        /// Version found in the header.
        found: u16,
        /// Version this decoder implements.
        supported: u16,
    },

    /// A field or padding byte that must be zero is not.
    #[error("reserved field non-zero at offset {offset}")]
    ReservedNonZero {
        /// Offset of the offending field.
        offset: usize,
    },

    /// The header declares more records than could possibly fit in the buffer.
    #[error("record count {declared} exceeds what fits in the stream (max {max_possible})")]
    RecordCountTooLarge {
        /// Declared record count.
        declared: u32,
        /// Upper bound derived from the buffer length.
        max_possible: usize,
    },

    /// A record header is cut off by the end of the buffer.
    #[error("truncated record header at offset {offset}")]
    TruncatedRecordHeader {
        /// Offset of the record.
        offset: usize,
    },

    /// A record payload (plus padding) extends past the end of the buffer.
    #[error("truncated record at offset {offset}: needs {needed} bytes, {available} available")]
    TruncatedPayload {
        /// Offset of the record.
        offset: usize,
        /// Bytes the record needs including header and padding.
        needed: u64,
        /// Bytes remaining in the buffer from `offset`.
        available: usize,
    },

    /// A payload length exceeds [`crate::MAX_PAYLOAD_LEN`].
    #[error("payload length {len} at offset {offset} exceeds maximum")]
    PayloadTooLarge {
        /// Offset of the record.
        offset: usize,
        /// Declared payload length.
        len: u32,
    },

    /// The opcode is not known to this protocol version.
    #[error("unknown opcode {raw:#06x} at offset {offset}")]
    UnknownOpcode {
        /// Offset of the record.
        offset: usize,
        /// Raw opcode value.
        raw: u16,
    },

    /// A record sets flag bits that this protocol version does not define.
    #[error("unknown record flags {flags:#06x} at offset {offset}")]
    UnknownFlags {
        /// Offset of the record.
        offset: usize,
        /// Raw flags.
        flags: u16,
    },

    /// The payload length does not match what the opcode requires.
    #[error("bad payload length {len} for opcode {opcode:#06x} at offset {offset}")]
    BadPayloadLength {
        /// Offset of the record.
        offset: usize,
        /// Raw opcode value.
        opcode: u16,
        /// Declared payload length.
        len: u32,
    },

    /// A typed payload field holds a value that is invalid at the wire level
    /// (e.g. object id 0, inconsistent attachment count).
    #[error("invalid field `{field}` in record at offset {offset}")]
    InvalidField {
        /// Offset of the record.
        offset: usize,
        /// Field name.
        field: &'static str,
    },

    /// Bytes remain after the declared number of records.
    #[error("{len} trailing bytes at offset {offset}")]
    TrailingBytes {
        /// Offset of the first trailing byte.
        offset: usize,
        /// Number of trailing bytes.
        len: usize,
    },

    /// The encoder was asked to produce a stream that cannot be represented
    /// (too many records or payload too large). Encoder-side only.
    #[error("encoder limit exceeded: {0}")]
    EncoderLimit(&'static str),
}

impl From<ProtoError> for ritsu_core::Error {
    fn from(e: ProtoError) -> Self {
        ritsu_core::Error::Guest(format!("gpu command stream: {e}"))
    }
}
