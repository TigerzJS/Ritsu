//! Stream framing: header, opcodes, records and the zero-copy record iterator.

use crate::error::ProtoError;
use crate::reader::{u16_at, u32_at};

/// Highest protocol version implemented by this crate.
///
/// A session negotiates its version with HELLO (see [`crate::negotiate`]); every later stream
/// must carry exactly the negotiated version.
pub const PROTOCOL_VERSION: u16 = 1;

/// Lowest protocol version this crate still speaks. The host keeps at least the current and
/// the previous major version (gpu.md §9.3).
pub const MIN_PROTOCOL_VERSION: u16 = 1;

/// Header version of bootstrap streams: the guest's HELLO stream and the host's
/// `HelloAck`/`HelloReject` reply stream. The framing (stream header, record header) and the
/// HELLO / HELLO_ACK / HELLO_REJECT payloads are frozen across all protocol versions, so any
/// host can parse any guest's HELLO.
pub const BOOTSTRAP_VERSION: u16 = 1;

// The bootstrap version must stay parseable by every build (see `version_parseable`).
const _: () = assert!(BOOTSTRAP_VERSION >= 1 && MIN_PROTOCOL_VERSION >= 1);
const _: () = assert!(MIN_PROTOCOL_VERSION <= PROTOCOL_VERSION);

/// Header versions any stream may carry: the bootstrap version (always, even after it has
/// left the supported range) and every supported session version.
#[must_use]
pub const fn version_parseable(version: u16) -> bool {
    version == BOOTSTRAP_VERSION || (version >= MIN_PROTOCOL_VERSION && version <= PROTOCOL_VERSION)
}

/// Protocol rule (v1): a guest may have at most this many committed command buffers whose
/// `CommandBufferStatus` it has not yet received. A stream whose `Commit`s would exceed it is
/// a protocol violation and resets the session (`ErrorCode::LimitExceeded`). Real drivers
/// block before that point, so honest guests never hit it.
pub const MAX_OUTSTANDING_SUBMISSIONS: u32 = 1 << 16;

/// Command stream magic (guest → host): the ASCII bytes `"RTSG"` as a little-endian `u32`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"RTSG");

/// Reply stream magic (host → guest): the ASCII bytes `"RTSR"` as a little-endian `u32`.
pub const REPLY_MAGIC: u32 = u32::from_le_bytes(*b"RTSR");

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
    /// Parses and validates a command stream header ([`MAGIC`]) at the start of `bytes`.
    ///
    /// Accepts [`BOOTSTRAP_VERSION`] and every version in
    /// `MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION` ([`version_parseable`]); whether the version
    /// matches the session is checked by [`crate::decode_session_stream`].
    ///
    /// # Errors
    ///
    /// Truncation, wrong magic, unsupported version, or non-zero flags/reserved fields.
    pub fn parse(bytes: &[u8]) -> Result<Self, ProtoError> {
        Self::parse_with_magic(bytes, MAGIC)
    }

    /// Parses and validates a reply stream header ([`REPLY_MAGIC`]).
    ///
    /// # Errors
    ///
    /// See [`Header::parse`].
    pub fn parse_reply(bytes: &[u8]) -> Result<Self, ProtoError> {
        Self::parse_with_magic(bytes, REPLY_MAGIC)
    }

    fn parse_with_magic(bytes: &[u8], expected_magic: u32) -> Result<Self, ProtoError> {
        let field = |at| u32_at(bytes, at).ok_or(ProtoError::TruncatedHeader { len: bytes.len() });
        if bytes.len() < HEADER_LEN {
            return Err(ProtoError::TruncatedHeader { len: bytes.len() });
        }
        let magic = field(0)?;
        if magic != expected_magic {
            return Err(ProtoError::BadMagic { found: magic });
        }
        let version = u16_at(bytes, 4).ok_or(ProtoError::TruncatedHeader { len: bytes.len() })?;
        if !version_parseable(version) {
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

    /// Serializes the header of a command stream.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        self.encode_with_magic(MAGIC)
    }

    /// Serializes the header of a reply stream.
    #[must_use]
    pub fn to_reply_bytes(&self) -> [u8; HEADER_LEN] {
        self.encode_with_magic(REPLY_MAGIC)
    }

    fn encode_with_magic(&self, magic: u32) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&magic.to_le_bytes());
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
    /// Payload must be exactly one of two lengths: a base layout and the base layout plus an
    /// optional extension (e.g. `CreateTexture` with or without swizzle).
    OneOf(u32, u32),
}

impl PayloadLen {
    /// Returns whether `len` satisfies the rule.
    #[must_use]
    pub const fn accepts(self, len: u32) -> bool {
        match self {
            Self::Exact(n) => len == n,
            Self::AtLeast(n) => len >= n,
            Self::OneOf(a, b) => len == a || len == b,
        }
    }
}

/// Opcode table of one stream direction; lets the framing code serve both directions.
pub(crate) trait OpcodeTable: Copy {
    /// Expected stream magic.
    const MAGIC: u32;
    /// Converts a raw opcode.
    fn from_raw_opcode(raw: u16) -> Option<Self>;
    /// Payload length rule.
    fn rule(self) -> PayloadLen;
}

/// How a command relates to Metal's object model; decides error handling in the host.
///
/// Every command opcode declares its class in the opcode table (mandatory column), so a new
/// opcode cannot be added without deciding whether it is part of a command buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OpcodeClass {
    /// Session control (`Nop`, `Hello`); not part of any command buffer.
    Control,
    /// Device-level resource operation (create, destroy, CPU write); executes immediately.
    Resource,
    /// Recorded into the current command buffer (render, compute, blit encoders).
    Encoder,
    /// Synchronization recorded into the current command buffer (events, fences).
    Sync,
    /// Ends the current command buffer (`Commit`).
    Submission,
}

macro_rules! opcodes {
    // Command opcodes: mandatory class column.
    (
        $(#[$em:meta])* classified enum $enm:ident (magic = $magic:expr);
        $( $(#[$m:meta])* $name:ident = $val:literal => $rule:expr, $class:ident; )*
    ) => {
        opcodes!(@base [$(#[$em])*] $enm ($magic); $( [$(#[$m])*] $name = $val => $rule; )*);

        impl $enm {
            /// Class of this opcode (see [`OpcodeClass`]).
            #[must_use]
            pub const fn class(self) -> OpcodeClass {
                match self {
                    $( Self::$name => OpcodeClass::$class, )*
                }
            }
        }
    };
    // Reply opcodes: no class.
    (
        $(#[$em:meta])* enum $enm:ident (magic = $magic:expr);
        $( $(#[$m:meta])* $name:ident = $val:literal => $rule:expr, )*
    ) => {
        opcodes!(@base [$(#[$em])*] $enm ($magic); $( [$(#[$m])*] $name = $val => $rule; )*);
    };
    (@base [$(#[$em:meta])*] $enm:ident ($magic:expr);
        $( [$(#[$m:meta])*] $name:ident = $val:literal => $rule:expr; )*
    ) => {
        $(#[$em])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        #[repr(u16)]
        pub enum $enm {
            $( $(#[$m])* $name = $val, )*
        }

        impl $enm {
            /// All opcodes known to this protocol version.
            pub const ALL: &'static [$enm] = &[$($enm::$name,)*];

            /// Converts a raw wire value; `None` for unknown values (including 0).
            #[must_use]
            #[inline]
            pub const fn from_raw(raw: u16) -> Option<Self> {
                match raw {
                    $( $val => Some(Self::$name), )*
                    _ => None,
                }
            }

            /// Payload length rule for this opcode.
            #[must_use]
            #[inline]
            pub const fn payload_len(self) -> PayloadLen {
                match self {
                    $( Self::$name => $rule, )*
                }
            }

            /// Raw wire value.
            #[must_use]
            pub const fn raw(self) -> u16 {
                self as u16
            }
        }

        impl OpcodeTable for $enm {
            const MAGIC: u32 = $magic;
            // Per-record hot path of the decoder (Q-PERF-3 instruction-count gate).
            #[inline]
            fn from_raw_opcode(raw: u16) -> Option<Self> {
                Self::from_raw(raw)
            }
            #[inline]
            fn rule(self) -> PayloadLen {
                self.payload_len()
            }
        }
    };
}

opcodes! {
    /// Command opcodes (guest → host). Values are part of the wire ABI and must never be reused.
    ///
    /// Ranges: `0x00xx` control, `0x01xx` resources, `0x02xx` render encoder,
    /// `0x03xx` compute encoder, `0x04xx` synchronization, `0x05xx` submission.
    classified enum Opcode (magic = MAGIC);
    /// No operation. Any payload is ignored (debug markers, padding).
    Nop = 0x0001 => PayloadLen::AtLeast(0), Control;
    /// Session handshake: version range and feature bits offered by the guest (bootstrap,
    /// frozen layout). The first stream of a session holds exactly one HELLO (plus `Nop`s).
    Hello = 0x0002 => PayloadLen::Exact(16), Control;
    /// Create a buffer object with a guest-chosen id.
    CreateBuffer = 0x0100 => PayloadLen::Exact(16), Resource;
    /// Destroy a buffer object.
    DestroyBuffer = 0x0101 => PayloadLen::Exact(4), Resource;
    /// Copy inline bytes into a buffer (upload path for small data; see gpu.md §9.5 for
    /// zero-copy shared-memory uploads).
    WriteBuffer = 0x0102 => PayloadLen::AtLeast(16), Resource;
    /// Blit: copy a byte range between buffers (recorded into the command buffer, hence
    /// class `Encoder` despite its `0x01xx` value).
    CopyBuffer = 0x0103 => PayloadLen::Exact(32), Encoder;
    /// Create a texture object with a guest-chosen id. 32 bytes, or 40 bytes with the
    /// swizzle extension (`MTLTextureDescriptor.swizzle`).
    CreateTexture = 0x0110 => PayloadLen::OneOf(32, 40), Resource;
    /// Destroy a texture object (textures and texture views share one namespace).
    DestroyTexture = 0x0111 => PayloadLen::Exact(4), Resource;
    /// Create a texture view of a texture (or of another view) with a guest-chosen id in the
    /// texture namespace (`newTextureViewWithPixelFormat:textureType:levels:slices:swizzle:`).
    CreateTextureView = 0x0112 => PayloadLen::Exact(32), Resource;
    /// Create a sampler state (`MTLSamplerDescriptor`).
    CreateSampler = 0x0120 => PayloadLen::Exact(32), Resource;
    /// Destroy a sampler state.
    DestroySampler = 0x0121 => PayloadLen::Exact(4), Resource;
    /// Create a depth/stencil state (`MTLDepthStencilDescriptor`).
    CreateDepthStencilState = 0x0130 => PayloadLen::Exact(40), Resource;
    /// Destroy a depth/stencil state.
    DestroyDepthStencilState = 0x0131 => PayloadLen::Exact(4), Resource;
    /// Create a render pipeline state (`MTLRenderPipelineDescriptor`; shader functions are
    /// placeholders until the library/function opcodes exist).
    CreateRenderPipelineState = 0x0140 => PayloadLen::AtLeast(32), Resource;
    /// Destroy a render pipeline state.
    DestroyRenderPipelineState = 0x0141 => PayloadLen::Exact(4), Resource;
    /// Begin a render encoder (compact form: dense colour attachments at level/slice 0, no
    /// resolve, `float` clear colours). See `BeginRenderPass2` for the full descriptor.
    BeginRenderPass = 0x0200 => PayloadLen::AtLeast(16), Encoder;
    /// End the current render encoder.
    EndRenderPass = 0x0201 => PayloadLen::Exact(0), Encoder;
    /// Non-indexed draw.
    Draw = 0x0202 => PayloadLen::Exact(20), Encoder;
    /// Begin a render encoder with the full `MTLRenderPassDescriptor`: sparse colour
    /// attachments, depth and stencil attachments, level/slice/depth plane, resolve targets,
    /// `double` clear values, store action options, layered rendering.
    BeginRenderPass2 = 0x0203 => PayloadLen::AtLeast(24), Encoder;
    /// Replace a deferred (`MTLStoreActionUnknown`) store action
    /// (`setColorStoreAction:atIndex:`, `setDepthStoreAction:`, `setStencilStoreAction:`).
    SetStoreAction = 0x0204 => PayloadLen::Exact(8), Encoder;
    /// `setRenderPipelineState:`.
    SetRenderPipelineState = 0x0210 => PayloadLen::Exact(4), Encoder;
    /// `setDepthStencilState:` (id 0 = `nil`, the default state).
    SetDepthStencilState = 0x0211 => PayloadLen::Exact(4), Encoder;
    /// `setVertexBuffer:offset:atIndex:` / `…attributeStride:atIndex:`.
    SetVertexBuffer = 0x0212 => PayloadLen::Exact(24), Encoder;
    /// `setVertexTexture:atIndex:`.
    SetVertexTexture = 0x0213 => PayloadLen::Exact(8), Encoder;
    /// `setFragmentTexture:atIndex:`.
    SetFragmentTexture = 0x0214 => PayloadLen::Exact(8), Encoder;
    /// `setVertexSamplerState:atIndex:`.
    SetVertexSamplerState = 0x0215 => PayloadLen::Exact(8), Encoder;
    /// `setFragmentSamplerState:atIndex:`.
    SetFragmentSamplerState = 0x0216 => PayloadLen::Exact(8), Encoder;
    /// Begin a compute encoder.
    BeginComputePass = 0x0300 => PayloadLen::Exact(0), Encoder;
    /// End the current compute encoder.
    EndComputePass = 0x0301 => PayloadLen::Exact(0), Encoder;
    /// Dispatch threadgroups.
    Dispatch = 0x0302 => PayloadLen::Exact(24), Encoder;
    /// Signal a shared event to a value when prior work completes.
    SignalEvent = 0x0400 => PayloadLen::Exact(16), Sync;
    /// Make following work wait until a shared event reaches a value.
    WaitEvent = 0x0401 => PayloadLen::Exact(16), Sync;
    /// Commit the recorded command buffer for execution.
    Commit = 0x0500 => PayloadLen::Exact(8), Submission;
}

opcodes! {
    /// Reply opcodes (host → guest, reply channel). Values are part of the wire ABI and must
    /// never be reused. They live in `0x80xx` so a reply record is never mistaken for a
    /// command record in dumps (the streams also differ by magic).
    enum ReplyOpcode (magic = REPLY_MAGIC);
    /// Answer to HELLO: chosen version, enabled features, capability block (bootstrap,
    /// frozen layout).
    HelloAck = 0x8001 => PayloadLen::Exact(64),
    /// HELLO had no version in common with the host (bootstrap, frozen layout).
    HelloReject = 0x8002 => PayloadLen::Exact(8),
    /// The stream was rejected as a whole (protocol violation). Nothing in it took effect and
    /// all session state is gone; the guest must send HELLO again.
    SessionReset = 0x8003 => PayloadLen::Exact(24),
    /// One record failed semantic validation and was not executed; the stream continued.
    RecordError = 0x8004 => PayloadLen::Exact(32),
    /// Final status of a committed command buffer (completed or error), in commit order.
    CommandBufferStatus = 0x8005 => PayloadLen::Exact(16),
    /// A shared event reached a value (only with [`crate::Features::SHARED_EVENTS`]).
    EventSignaled = 0x8006 => PayloadLen::Exact(16),
}

impl Opcode {
    /// Feature bit that must have been negotiated before this opcode may be used, if any.
    ///
    /// A gated opcode in a session that did not negotiate its feature is treated like an
    /// unknown opcode (gpu.md §9.3).
    #[must_use]
    pub const fn required_feature(self) -> Option<crate::Features> {
        match self {
            Self::SignalEvent | Self::WaitEvent => Some(crate::Features::SHARED_EVENTS),
            _ => None,
        }
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

/// A framed record of either direction (internal).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame<'a, O> {
    pub(crate) offset: usize,
    pub(crate) opcode: O,
    pub(crate) flags: u16,
    pub(crate) payload: &'a [u8],
}

/// Direction-independent framing iterator (internal). See [`RecordIter`] for the contract.
#[derive(Debug, Clone)]
pub(crate) struct Frames<'a, O> {
    bytes: &'a [u8],
    pos: usize,
    remaining: u32,
    header: Header,
    done: bool,
    _opcodes: core::marker::PhantomData<O>,
}

impl<'a, O: OpcodeTable> Frames<'a, O> {
    pub(crate) fn new(bytes: &'a [u8]) -> Result<Self, ProtoError> {
        let header = Header::parse_with_magic(bytes, O::MAGIC)?;
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
            _opcodes: core::marker::PhantomData,
        })
    }

    pub(crate) fn header(&self) -> Header {
        self.header
    }

    fn parse_record(&self) -> Result<(Frame<'a, O>, usize), ProtoError> {
        let offset = self.pos;
        let rest = self.bytes.get(offset..).unwrap_or(&[]);
        let (Some(raw_op), Some(flags), Some(len)) =
            (u16_at(rest, 0), u16_at(rest, 2), u32_at(rest, 4))
        else {
            return Err(ProtoError::TruncatedRecordHeader { offset });
        };
        let opcode = O::from_raw_opcode(raw_op).ok_or(ProtoError::UnknownOpcode {
            offset,
            raw: raw_op,
        })?;
        if flags != 0 {
            return Err(ProtoError::UnknownFlags { offset, flags });
        }
        if len > MAX_PAYLOAD_LEN {
            return Err(ProtoError::PayloadTooLarge { offset, len });
        }
        if !opcode.rule().accepts(len) {
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
            Frame {
                offset,
                opcode,
                flags,
                payload,
            },
            total,
        ))
    }
}

impl<'a, O: OpcodeTable> Iterator for Frames<'a, O> {
    type Item = Result<Frame<'a, O>, ProtoError>;

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
            Ok((frame, total)) => {
                self.pos += total;
                self.remaining -= 1;
                Some(Ok(frame))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// Zero-copy, bounds-checked iterator over the records of a command stream.
///
/// Yields exactly `record_count` records, then reports trailing bytes (if any) as an error.
/// After the first error the iterator is fused and yields `None`.
#[derive(Debug, Clone)]
pub struct RecordIter<'a> {
    inner: Frames<'a, Opcode>,
}

impl<'a> RecordIter<'a> {
    /// Parses the header and prepares iteration over the records.
    ///
    /// # Errors
    ///
    /// Any header error, or a record count that cannot fit into the buffer.
    pub fn new(bytes: &'a [u8]) -> Result<Self, ProtoError> {
        Ok(Self {
            inner: Frames::new(bytes)?,
        })
    }

    /// The validated stream header.
    #[must_use]
    pub fn header(&self) -> Header {
        self.inner.header()
    }
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = Result<Record<'a>, ProtoError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|r| {
            r.map(|f| Record {
                offset: f.offset,
                opcode: f.opcode,
                flags: f.flags,
                payload: f.payload,
            })
        })
    }
}

impl core::iter::FusedIterator for RecordIter<'_> {}
