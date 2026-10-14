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

    /// A session that has not negotiated yet received something other than a stream
    /// consisting of exactly one HELLO record. `raw` is the opcode found (0 if the stream
    /// has no record).
    #[error("HELLO required before opcode {raw:#06x} at offset {offset}")]
    HelloRequired {
        /// Offset of the offending record (or of the end of the header for empty streams).
        offset: usize,
        /// Raw opcode found.
        raw: u16,
    },

    /// A HELLO stream contains more than the single HELLO record.
    #[error("HELLO stream has extra record at offset {offset}")]
    HelloNotAlone {
        /// Offset of the first extra record.
        offset: usize,
    },

    /// HELLO received in a session that has already negotiated.
    #[error("duplicate HELLO at offset {offset}")]
    UnexpectedHello {
        /// Offset of the HELLO record.
        offset: usize,
    },

    /// The stream header version differs from the version negotiated for the session.
    #[error("stream version {found} differs from negotiated version {negotiated}")]
    VersionMismatch {
        /// Version in the stream header.
        found: u16,
        /// Version negotiated with HELLO.
        negotiated: u16,
    },

    /// The opcode requires a feature bit the session did not negotiate (treated like an
    /// unknown opcode, gpu.md §9.3).
    #[error("opcode {raw:#06x} at offset {offset} needs a feature that was not negotiated")]
    FeatureNotNegotiated {
        /// Offset of the record.
        offset: usize,
        /// Raw opcode.
        raw: u16,
    },

    /// The encoder was asked to produce a stream that cannot be represented
    /// (too many records or payload too large). Encoder-side only.
    #[error("encoder limit exceeded: {0}")]
    EncoderLimit(&'static str),
}

impl ProtoError {
    /// Byte offset in the stream the error refers to, if any (0 = stream header).
    #[must_use]
    pub const fn offset(&self) -> Option<usize> {
        match *self {
            Self::TruncatedHeader { .. }
            | Self::BadMagic { .. }
            | Self::UnsupportedVersion { .. }
            | Self::RecordCountTooLarge { .. }
            | Self::VersionMismatch { .. } => Some(0),
            Self::ReservedNonZero { offset }
            | Self::TruncatedRecordHeader { offset }
            | Self::TruncatedPayload { offset, .. }
            | Self::PayloadTooLarge { offset, .. }
            | Self::UnknownOpcode { offset, .. }
            | Self::UnknownFlags { offset, .. }
            | Self::BadPayloadLength { offset, .. }
            | Self::InvalidField { offset, .. }
            | Self::TrailingBytes { offset, .. }
            | Self::HelloRequired { offset, .. }
            | Self::HelloNotAlone { offset }
            | Self::UnexpectedHello { offset }
            | Self::FeatureNotNegotiated { offset, .. } => Some(offset),
            Self::EncoderLimit(_) => None,
        }
    }

    /// Raw opcode of the offending record, if the error is tied to a known opcode value.
    #[must_use]
    pub const fn raw_opcode(&self) -> Option<u16> {
        match *self {
            Self::UnknownOpcode { raw, .. }
            | Self::HelloRequired { raw, .. }
            | Self::FeatureNotNegotiated { raw, .. } => Some(raw),
            Self::BadPayloadLength { opcode, .. } => Some(opcode),
            Self::UnexpectedHello { .. } => Some(crate::Opcode::Hello.raw()),
            _ => None,
        }
    }

    /// Reply-channel error code for this protocol violation.
    #[must_use]
    pub const fn reply_code(&self) -> crate::ErrorCode {
        use crate::ErrorCode as C;
        match self {
            Self::UnsupportedVersion { .. } | Self::VersionMismatch { .. } => C::UnsupportedVersion,
            Self::HelloRequired { .. } => C::HelloRequired,
            Self::UnexpectedHello { .. } => C::DuplicateHello,
            Self::FeatureNotNegotiated { .. } => C::FeatureNotNegotiated,
            _ => C::Malformed,
        }
    }
}

impl From<ProtoError> for ritsu_core::Error {
    fn from(e: ProtoError) -> Self {
        ritsu_core::Error::Guest(format!("gpu command stream: {e}"))
    }
}
