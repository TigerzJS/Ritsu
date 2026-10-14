//! Stream framing: header, opcodes, records and the zero-copy record iterator.

use crate::error::ProtoError;
use crate::reader::{u16_at, u32_at};

/// Protocol version implemented by this crate. A stream must carry exactly this version;
/// richer negotiation (feature bits per session) is described in `docs/subsystems/gpu.md` §9.
pub const PROTOCOL_VERSION: u16 = 1;

/// Stream magic: the ASCII bytes `"RTSG"` read as a little-endian `u32`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"RTSG");

/// Size of the stream [`Header`] in bytes.
pub const HEADER_LEN: usize = 16;

/// Size of a record header (opcode, flags, payload length) in bytes.
pub const RECORD_HEADER_LEN: usize = 8;

/// Every record (header + payload + padding) is a multiple of this many bytes.
pub const RECORD_ALIGN: usize = 8;

/// Largest payload a single record may carry (16 MiB). Larger uploads are split by the guest.
pub const MAX_PAYLOAD_LEN: u32 = 16 << 20;

/// The fixed stream header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Protocol version of the stream.
    pub version: u16,
    /// Number of records following the header.
    pub record_count: u32,
}

impl Header {
    /// Parses and validates the header at the start of `bytes`.
    ///
    /// # Errors
    ///
    /// Truncation, wrong magic, unsupported version, or non-zero flags/reserved fields.
    pub fn parse(bytes: &[u8]) -> Result<Self, ProtoError> {
        let field = |at| u32_at(bytes, at).ok_or(ProtoError::TruncatedHeader { len: bytes.len() });
        if bytes.len() < HEADER_LEN {
            return Err(ProtoError::TruncatedHeader { len: bytes.len() });
        }
        let magic = field(0)?;
        if magic != MAGIC {
            return Err(ProtoError::BadMagic { found: magic });
        }
        let version = u16_at(bytes, 4).ok_or(ProtoError::TruncatedHeader { len: bytes.len() })?;
        if version != PROTOCOL_VERSION {
            return Err(ProtoError::UnsupportedVersion {
                found: version,
                supported: PROTOCOL_VERSION,
            });
        }
        let flags = u16_at(bytes, 6).ok_or(ProtoError::TruncatedHeader { len: bytes.len() })?;
        if flags != 0 {
            return Err(ProtoError::ReservedNonZero { offset: 6 });
        }
        let record_count = field(8)?;
        if field(12)? != 0 {
            return Err(ProtoError::ReservedNonZero { offset: 12 });
        }
        Ok(Self {
            version,
            record_count,
        })
    }

    /// Serializes the header.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[8..12].copy_from_slice(&self.record_count.to_le_bytes());
        out
    }
}

/// Payload length rule of an opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadLen {
    /// Payload must be exactly this many bytes.
    Exact(u32),
    /// Payload must be at least this many bytes (variable tail).
    AtLeast(u32),
}

impl PayloadLen {
    /// Returns whether `len` satisfies the rule.
    #[must_use]
    pub const fn accepts(self, len: u32) -> bool {
        match self {
            Self::Exact(n) => len == n,
            Self::AtLeast(n) => len >= n,
        }
    }
}

macro_rules! opcodes {
    ($( $(#[$m:meta])* $name:ident = $val:literal => $rule:expr, )*) => {
        /// Command opcodes. Values are part of the wire ABI and must never be reused.
        ///
        /// Ranges: `0x00xx` control, `0x01xx` resources, `0x02xx` render encoder,
        /// `0x03xx` compute encoder, `0x04xx` synchronization, `0x05xx` submission.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        #[repr(u16)]
        pub enum Opcode {
            $( $(#[$m])* $name = $val, )*
        }

        impl Opcode {
            /// All opcodes known to this protocol version.
            pub const ALL: &'static [Opcode] = &[$(Opcode::$name,)*];

            /// Converts a raw wire value; `None` for unknown values (including 0).
            #[must_use]
            pub const fn from_raw(raw: u16) -> Option<Self> {
                match raw {
                    $( $val => Some(Self::$name), )*
                    _ => None,
                }
            }

            /// Payload length rule for this opcode.
            #[must_use]
            pub const fn payload_len(self) -> PayloadLen {
                match self {
                    $( Self::$name => $rule, )*
                }
            }
        }
    };
}

opcodes! {
    /// No operation. Any payload is ignored (debug markers, padding).
    Nop = 0x0001 => PayloadLen::AtLeast(0),
    /// Create a buffer object with a guest-chosen id.
    CreateBuffer = 0x0100 => PayloadLen::Exact(16),
    /// Destroy a buffer object.
    DestroyBuffer = 0x0101 => PayloadLen::Exact(4),
    /// Copy inline bytes into a buffer (upload path for small data; see gpu.md §9.5 for
    /// zero-copy shared-memory uploads).
    WriteBuffer = 0x0102 => PayloadLen::AtLeast(16),
    /// Blit: copy a byte range between buffers.
    CopyBuffer = 0x0103 => PayloadLen::Exact(32),
    /// Create a texture object with a guest-chosen id.
    CreateTexture = 0x0110 => PayloadLen::Exact(32),
    /// Destroy a texture object.
    DestroyTexture = 0x0111 => PayloadLen::Exact(4),
    /// Begin a render encoder (render pass with colour attachments).
    BeginRenderPass = 0x0200 => PayloadLen::AtLeast(16),
    /// End the current render encoder.
    EndRenderPass = 0x0201 => PayloadLen::Exact(0),
    /// Non-indexed draw.
    Draw = 0x0202 => PayloadLen::Exact(20),
    /// Begin a compute encoder.
    BeginComputePass = 0x0300 => PayloadLen::Exact(0),
    /// End the current compute encoder.
    EndComputePass = 0x0301 => PayloadLen::Exact(0),
    /// Dispatch threadgroups.
    Dispatch = 0x0302 => PayloadLen::Exact(24),
    /// Signal a shared event to a value when prior work completes.
    SignalEvent = 0x0400 => PayloadLen::Exact(16),
    /// Make following work wait until a shared event reaches a value.
    WaitEvent = 0x0401 => PayloadLen::Exact(16),
    /// Commit the recorded command buffer for execution.
    Commit = 0x0500 => PayloadLen::Exact(8),
}

impl Opcode {
    /// Raw wire value.
    #[must_use]
    pub const fn raw(self) -> u16 {
        self as u16
    }
}

/// One framed record, borrowing its payload from the stream (zero-copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    /// Byte offset of the record header within the stream.
    pub offset: usize,
    /// Decoded opcode.
    pub opcode: Opcode,
    /// Record flags (always 0 in version 1).
    pub flags: u16,
    /// Payload without padding.
    pub payload: &'a [u8],
}

/// Zero-copy, bounds-checked iterator over the records of a stream.
///
/// Yields exactly `record_count` records, then reports trailing bytes (if any) as an error.
/// After the first error the iterator is fused and yields `None`.
#[derive(Debug, Clone)]
pub struct RecordIter<'a> {
    bytes: &'a [u8],
    pos: usize,
    remaining: u32,
    header: Header,
    done: bool,
}

impl<'a> RecordIter<'a> {
    /// Parses the header and prepares iteration over the records.
    ///
    /// # Errors
    ///
    /// Any header error, or a record count that cannot fit into the buffer.
    pub fn new(bytes: &'a [u8]) -> Result<Self, ProtoError> {
        let header = Header::parse(bytes)?;
        let body = bytes.len().saturating_sub(HEADER_LEN);
        let max_possible = body / RECORD_HEADER_LEN;
        if usize::try_from(header.record_count).map_or(true, |c| c > max_possible) {
            return Err(ProtoError::RecordCountTooLarge {
                declared: header.record_count,
                max_possible,
            });
        }
        Ok(Self {
            bytes,
            pos: HEADER_LEN,
            remaining: header.record_count,
            header,
            done: false,
        })
    }

    /// The validated stream header.
    #[must_use]
    pub fn header(&self) -> Header {
        self.header
    }

    fn parse_record(&self) -> Result<(Record<'a>, usize), ProtoError> {
        let offset = self.pos;
        let rest = self.bytes.get(offset..).unwrap_or(&[]);
        let (Some(raw_op), Some(flags), Some(len)) =
            (u16_at(rest, 0), u16_at(rest, 2), u32_at(rest, 4))
        else {
            return Err(ProtoError::TruncatedRecordHeader { offset });
        };
        let opcode = Opcode::from_raw(raw_op).ok_or(ProtoError::UnknownOpcode {
            offset,
            raw: raw_op,
        })?;
        if flags != 0 {
            return Err(ProtoError::UnknownFlags { offset, flags });
        }
        if len > MAX_PAYLOAD_LEN {
            return Err(ProtoError::PayloadTooLarge { offset, len });
        }
        if !opcode.payload_len().accepts(len) {
            return Err(ProtoError::BadPayloadLength {
                offset,
                opcode: raw_op,
                len,
            });
        }
        // len <= MAX_PAYLOAD_LEN, so this arithmetic cannot overflow in u64.
        let len64 = u64::from(len);
        let align = RECORD_ALIGN as u64;
        let padded = len64.div_ceil(align) * align;
        let needed = RECORD_HEADER_LEN as u64 + padded;
        let total = usize::try_from(needed)
            .ok()
            .filter(|&t| t <= rest.len())
            .ok_or(ProtoError::TruncatedPayload {
                offset,
                needed,
                available: rest.len(),
            })?;
        let payload_end = RECORD_HEADER_LEN + len as usize;
        let (Some(payload), Some(padding)) = (
            rest.get(RECORD_HEADER_LEN..payload_end),
            rest.get(payload_end..total),
        ) else {
            return Err(ProtoError::TruncatedPayload {
                offset,
                needed,
                available: rest.len(),
            });
        };
        if let Some(i) = padding.iter().position(|&b| b != 0) {
            return Err(ProtoError::ReservedNonZero {
                offset: offset + payload_end + i,
            });
        }
        Ok((
            Record {
                offset,
                opcode,
                flags,
                payload,
            },
            total,
        ))
    }
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = Result<Record<'a>, ProtoError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if self.remaining == 0 {
            self.done = true;
            let len = self.bytes.len().saturating_sub(self.pos);
            return (len != 0).then_some(Err(ProtoError::TrailingBytes {
                offset: self.pos,
                len,
            }));
        }
        match self.parse_record() {
            Ok((record, total)) => {
                self.pos += total;
                self.remaining -= 1;
                Some(Ok(record))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

impl core::iter::FusedIterator for RecordIter<'_> {}
