//! Reply channel (host → guest): handshake answers, per-record errors, command buffer
//! completion and event notifications.
//!
//! A reply stream uses the same framing as a command stream, but the magic
//! [`crate::REPLY_MAGIC`] (`"RTSR"`) and the opcodes of [`ReplyOpcode`]. All payloads are
//! fixed-size, so decoding a reply never allocates per field. The guest driver decodes these
//! streams; [`decode_reply_stream`] is the reference decoder and a fuzz target.

use crate::command::{EventValue, ObjectId};
use crate::error::ProtoError;
use crate::negotiate::{Features, HelloReject};
use crate::reader::Dec;
use crate::wire::{Frames, ReplyOpcode};

/// Error codes of the reply channel. Values are wire ABI and never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(u16)]
pub enum ErrorCode {
    /// Framing or wire-level error in the stream.
    Malformed = 1,
    /// A stream other than a lone HELLO arrived before the handshake.
    HelloRequired = 2,
    /// HELLO in an already negotiated session.
    DuplicateHello = 3,
    /// Stream header version is unsupported or differs from the negotiated version.
    UnsupportedVersion = 4,
    /// Opcode needs a feature that was not negotiated.
    FeatureNotNegotiated = 5,
    /// A guest id does not name a live object of the required kind.
    UnknownObject = 6,
    /// A guest id is already in use.
    DuplicateObject = 7,
    /// A descriptor, enum value, size or offset is invalid.
    InvalidArgument = 8,
    /// The command (buffer) is invalid in its context (encoder nesting, bounds, overlap).
    InvalidCommand = 9,
    /// Device memory budget exhausted.
    OutOfMemory = 10,
    /// A per-session limit was hit.
    LimitExceeded = 11,
    /// Valid Metal, but not implemented by Ritsu yet.
    Unimplemented = 12,
    /// Host or driver failure (device lost, …).
    HostError = 13,
    /// Further record errors of this stream were suppressed; `detail` holds their number.
    TooManyErrors = 14,
}

impl ErrorCode {
    /// All codes.
    pub const ALL: &'static [ErrorCode] = &[
        Self::Malformed,
        Self::HelloRequired,
        Self::DuplicateHello,
        Self::UnsupportedVersion,
        Self::FeatureNotNegotiated,
        Self::UnknownObject,
        Self::DuplicateObject,
        Self::InvalidArgument,
        Self::InvalidCommand,
        Self::OutOfMemory,
        Self::LimitExceeded,
        Self::Unimplemented,
        Self::HostError,
        Self::TooManyErrors,
    ];

    /// Converts a raw value; `None` for unknown values (including 0).
    #[must_use]
    pub const fn from_raw(raw: u16) -> Option<Self> {
        Some(match raw {
            1 => Self::Malformed,
            2 => Self::HelloRequired,
            3 => Self::DuplicateHello,
            4 => Self::UnsupportedVersion,
            5 => Self::FeatureNotNegotiated,
            6 => Self::UnknownObject,
            7 => Self::DuplicateObject,
            8 => Self::InvalidArgument,
            9 => Self::InvalidCommand,
            10 => Self::OutOfMemory,
            11 => Self::LimitExceeded,
            12 => Self::Unimplemented,
            13 => Self::HostError,
            14 => Self::TooManyErrors,
            _ => return None,
        })
    }

    /// Raw wire value.
    #[must_use]
    pub const fn raw(self) -> u16 {
        self as u16
    }
}

/// How a feature reaches the guest (wire form of the host's `Support`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SupportLevel {
    /// Not available; the guest must not report it to Metal clients.
    Unsupported = 0,
    /// Mapped to a native Vulkan feature.
    Native = 1,
    /// Correct emulation (slower).
    Emulated = 2,
}

impl SupportLevel {
    /// Converts a raw value.
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Unsupported),
            1 => Some(Self::Native),
            2 => Some(Self::Emulated),
            _ => None,
        }
    }
}

/// Capability block of `HelloAck` (48 bytes): what the guest driver may report to Metal.
///
/// Layout: `families u32 | argument_buffers_tier u8 | 10 × SupportLevel u8 | reserved u8 |
/// max_buffer_length u64 | max_texture_dimension_1d/2d/3d u32 | max_texture_array_layers u32 |
/// max_threads_per_threadgroup u32 | max_color_attachments u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityBlock {
    /// `FAMILY_*` bits answered `YES` by `supportsFamily:`. The host only sends bits defined
    /// in the negotiated version; the decoder drops unknown bits.
    pub families: u32,
    /// `MTLArgumentBuffersTier` (1 or 2).
    pub argument_buffers_tier: u8,
    /// Ray tracing.
    pub ray_tracing: SupportLevel,
    /// Mesh and object shaders.
    pub mesh_shaders: SupportLevel,
    /// ASTC pixel formats.
    pub astc: SupportLevel,
    /// BC pixel formats.
    pub bc: SupportLevel,
    /// `atomic<float>` in device memory.
    pub float32_atomics: SupportLevel,
    /// Framebuffer fetch.
    pub framebuffer_fetch: SupportLevel,
    /// Raster order groups.
    pub raster_order_groups: SupportLevel,
    /// Tile shaders and imageblocks.
    pub tile_shaders: SupportLevel,
    /// Memoryless render targets.
    pub memoryless: SupportLevel,
    /// Unified memory (guest always sees `hasUnifiedMemory = YES`; this says how).
    pub unified_memory: SupportLevel,
    /// `maxBufferLength`.
    pub max_buffer_length: u64,
    /// Maximum 1D texture width.
    pub max_texture_dimension_1d: u32,
    /// Maximum 2D/cube texture extent.
    pub max_texture_dimension_2d: u32,
    /// Maximum 3D texture extent.
    pub max_texture_dimension_3d: u32,
    /// Maximum texture array length.
    pub max_texture_array_layers: u32,
    /// Maximum threads per threadgroup.
    pub max_threads_per_threadgroup: u32,
    /// Maximum colour attachments.
    pub max_color_attachments: u32,
}

impl CapabilityBlock {
    /// Encoded size in bytes.
    pub const LEN: usize = 48;
    /// `MTLGPUFamilyApple7`.
    pub const FAMILY_APPLE7: u32 = 1 << 0;
    /// `MTLGPUFamilyMac2`.
    pub const FAMILY_MAC2: u32 = 1 << 1;
    /// `MTLGPUFamilyCommon3`.
    pub const FAMILY_COMMON3: u32 = 1 << 2;
    /// `MTLGPUFamilyMetal3`.
    pub const FAMILY_METAL3: u32 = 1 << 3;
    /// `MTLGPUFamilyMetal4`.
    pub const FAMILY_METAL4: u32 = 1 << 4;
    /// All family bits defined by this version.
    pub const FAMILY_KNOWN: u32 = Self::FAMILY_APPLE7
        | Self::FAMILY_MAC2
        | Self::FAMILY_COMMON3
        | Self::FAMILY_METAL3
        | Self::FAMILY_METAL4;

    fn supports(&self) -> [SupportLevel; 10] {
        [
            self.ray_tracing,
            self.mesh_shaders,
            self.astc,
            self.bc,
            self.float32_atomics,
            self.framebuffer_fetch,
            self.raster_order_groups,
            self.tile_shaders,
            self.memoryless,
            self.unified_memory,
        ]
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.families.to_le_bytes());
        out.push(self.argument_buffers_tier);
        out.extend(self.supports().iter().map(|s| *s as u8));
        out.push(0);
        out.extend_from_slice(&self.max_buffer_length.to_le_bytes());
        for v in [
            self.max_texture_dimension_1d,
            self.max_texture_dimension_2d,
            self.max_texture_dimension_3d,
            self.max_texture_array_layers,
            self.max_threads_per_threadgroup,
            self.max_color_attachments,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn decode(d: &mut Dec<'_>) -> Result<Self, ProtoError> {
        // Unknown family bits are ignored (forward compatibility of the frozen HELLO_ACK: a
        // newer host may know more families; the guest only reports those it knows).
        let families = d.u32("families")? & Self::FAMILY_KNOWN;
        let argument_buffers_tier = d.u8("argument_buffers_tier")?;
        if !matches!(argument_buffers_tier, 1 | 2) {
            return Err(d.err("argument_buffers_tier"));
        }
        let mut level = |f: &'static str| -> Result<SupportLevel, ProtoError> {
            let raw = d.u8(f)?;
            SupportLevel::from_raw(raw).ok_or_else(|| d.err(f))
        };
        let ray_tracing = level("ray_tracing")?;
        let mesh_shaders = level("mesh_shaders")?;
        let astc = level("astc")?;
        let bc = level("bc")?;
        let float32_atomics = level("float32_atomics")?;
        let framebuffer_fetch = level("framebuffer_fetch")?;
        let raster_order_groups = level("raster_order_groups")?;
        let tile_shaders = level("tile_shaders")?;
        let memoryless = level("memoryless")?;
        let unified_memory = level("unified_memory")?;
        d.reserved(1)?;
        Ok(Self {
            families,
            argument_buffers_tier,
            ray_tracing,
            mesh_shaders,
            astc,
            bc,
            float32_atomics,
            framebuffer_fetch,
            raster_order_groups,
            tile_shaders,
            memoryless,
            unified_memory,
            max_buffer_length: d.u64("max_buffer_length")?,
            max_texture_dimension_1d: d.u32("max_texture_dimension_1d")?,
            max_texture_dimension_2d: d.u32("max_texture_dimension_2d")?,
            max_texture_dimension_3d: d.u32("max_texture_dimension_3d")?,
            max_texture_array_layers: d.u32("max_texture_array_layers")?,
            max_threads_per_threadgroup: d.u32("max_threads_per_threadgroup")?,
            max_color_attachments: d.u32("max_color_attachments")?,
        })
    }
}

/// `HelloAck` payload (64 bytes, frozen layout):
/// `version u16 | reserved u16 | reserved u32 | features u64 | CapabilityBlock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelloAck {
    /// Chosen protocol version (≥ 1).
    pub version: u16,
    /// Enabled features.
    pub features: Features,
    /// What the guest may report to Metal clients.
    pub caps: CapabilityBlock,
}

/// `SessionReset` payload (24 bytes):
/// `stream_seq u64 | offset u64 | opcode u16 | code u16 | reserved u32`.
///
/// The stream `stream_seq` was rejected as a whole: nothing in it took effect, every object
/// of the session is destroyed, pending work is dropped, no further statuses for earlier
/// submissions follow, and the guest must start again with HELLO (it sees a GPU reset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFault {
    /// Per-session sequence number of the stream (0 = first stream of the session).
    pub stream_seq: u64,
    /// Byte offset of the offending header/record in that stream.
    pub offset: u64,
    /// Raw opcode of the offending record, 0 if none/unknown.
    pub opcode: u16,
    /// Reason.
    pub code: ErrorCode,
}

/// Index value meaning "not tied to a record".
pub const NO_RECORD: u32 = u32::MAX;

/// `RecordError` payload (32 bytes):
/// `stream_seq u64 | record_index u32 | opcode u16 | code u16 | object_id u32 | reserved u32 |
/// detail u64`.
///
/// The record was not executed; the stream continued with the next record. If the record
/// belongs to a command buffer (encoder or synchronization command), that command buffer is
/// poisoned: its later encoder commands are skipped without further errors and its `Commit`
/// reports [`CommandBufferStatus::Error`] with this code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordError {
    /// Per-session sequence number of the stream.
    pub stream_seq: u64,
    /// Index of the record within the stream ([`NO_RECORD`] for summary entries).
    pub record_index: u32,
    /// Raw opcode of the record (0 for summary entries).
    pub opcode: u16,
    /// Reason.
    pub code: ErrorCode,
    /// Guest object the error refers to, if any.
    pub object_id: Option<ObjectId>,
    /// Code-specific detail (e.g. requested bytes for `OutOfMemory`, number of suppressed
    /// errors for `TooManyErrors`), otherwise 0.
    pub detail: u64,
}

/// Final status of a committed command buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandBufferStatus {
    /// Executed completely (`MTLCommandBufferStatusCompleted`).
    Completed,
    /// Not executed (`MTLCommandBufferStatusError`).
    Error(ErrorCode),
}

/// `CommandBufferStatus` payload (16 bytes):
/// `submission_id u64 | status u8 (0 = completed, 1 = error) | reserved u8 | code u16 |
/// reserved u32`. `code` is 0 for completed command buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmissionStatus {
    /// Guest submission id from the `Commit` record.
    pub submission_id: u64,
    /// Outcome.
    pub status: CommandBufferStatus,
}

/// A decoded reply record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reply {
    /// See [`ReplyOpcode::HelloAck`].
    HelloAck(HelloAck),
    /// See [`ReplyOpcode::HelloReject`].
    HelloReject(HelloReject),
    /// See [`ReplyOpcode::SessionReset`].
    SessionReset(StreamFault),
    /// See [`ReplyOpcode::RecordError`].
    RecordError(RecordError),
    /// See [`ReplyOpcode::CommandBufferStatus`].
    CommandBufferStatus(SubmissionStatus),
    /// See [`ReplyOpcode::EventSignaled`].
    EventSignaled(EventValue),
}

impl Reply {
    /// Opcode of this reply.
    #[must_use]
    pub const fn opcode(&self) -> ReplyOpcode {
        match self {
            Self::HelloAck(_) => ReplyOpcode::HelloAck,
            Self::HelloReject(_) => ReplyOpcode::HelloReject,
            Self::SessionReset(_) => ReplyOpcode::SessionReset,
            Self::RecordError(_) => ReplyOpcode::RecordError,
            Self::CommandBufferStatus(_) => ReplyOpcode::CommandBufferStatus,
            Self::EventSignaled(_) => ReplyOpcode::EventSignaled,
        }
    }

    /// Appends the payload bytes (without record header or padding) to `out`.
    pub fn encode_payload(&self, out: &mut Vec<u8>) {
        match self {
            Self::HelloAck(a) => {
                out.extend_from_slice(&a.version.to_le_bytes());
                out.extend_from_slice(&[0; 6]);
                out.extend_from_slice(&a.features.bits().to_le_bytes());
                a.caps.encode(out);
            }
            Self::HelloReject(r) => {
                out.extend_from_slice(&r.host_min_version.to_le_bytes());
                out.extend_from_slice(&r.host_max_version.to_le_bytes());
                out.extend_from_slice(&[0; 4]);
            }
            Self::SessionReset(f) => {
                out.extend_from_slice(&f.stream_seq.to_le_bytes());
                out.extend_from_slice(&f.offset.to_le_bytes());
                out.extend_from_slice(&f.opcode.to_le_bytes());
                out.extend_from_slice(&f.code.raw().to_le_bytes());
                out.extend_from_slice(&[0; 4]);
            }
            Self::RecordError(e) => {
                out.extend_from_slice(&e.stream_seq.to_le_bytes());
                out.extend_from_slice(&e.record_index.to_le_bytes());
                out.extend_from_slice(&e.opcode.to_le_bytes());
                out.extend_from_slice(&e.code.raw().to_le_bytes());
                out.extend_from_slice(&e.object_id.map_or(0, ObjectId::get).to_le_bytes());
                out.extend_from_slice(&[0; 4]);
                out.extend_from_slice(&e.detail.to_le_bytes());
            }
            Self::CommandBufferStatus(s) => {
                let (status, code) = match s.status {
                    CommandBufferStatus::Completed => (0u8, 0u16),
                    CommandBufferStatus::Error(c) => (1, c.raw()),
                };
                out.extend_from_slice(&s.submission_id.to_le_bytes());
                out.extend_from_slice(&[status, 0]);
                out.extend_from_slice(&code.to_le_bytes());
                out.extend_from_slice(&[0; 4]);
            }
            Self::EventSignaled(ev) => {
                out.extend_from_slice(&ev.event_id.get().to_le_bytes());
                out.extend_from_slice(&[0; 4]);
                out.extend_from_slice(&ev.value.to_le_bytes());
            }
        }
    }

    fn decode(opcode: ReplyOpcode, payload: &[u8], offset: usize) -> Result<Self, ProtoError> {
        let mut d = Dec::new(payload, offset);
        let code = |d: &mut Dec<'_>| -> Result<ErrorCode, ProtoError> {
            let raw = d.u16("code")?;
            ErrorCode::from_raw(raw).ok_or_else(|| d.err("code"))
        };
        let reply = match opcode {
            ReplyOpcode::HelloAck => {
                let version = d.u16("version")?;
                if version == 0 {
                    return Err(d.err("version"));
                }
                d.reserved(6)?;
                let features = Features::from_bits_retain(d.u64("features")?);
                let caps = CapabilityBlock::decode(&mut d)?;
                Self::HelloAck(HelloAck {
                    version,
                    features,
                    caps,
                })
            }
            ReplyOpcode::HelloReject => {
                let host_min_version = d.u16("host_min_version")?;
                let host_max_version = d.u16("host_max_version")?;
                d.reserved(4)?;
                if host_min_version == 0 || host_max_version < host_min_version {
                    return Err(d.err("host_version_range"));
                }
                Self::HelloReject(HelloReject {
                    host_min_version,
                    host_max_version,
                })
            }
            ReplyOpcode::SessionReset => {
                let stream_seq = d.u64("stream_seq")?;
                let offset = d.u64("offset")?;
                let opcode = d.u16("opcode")?;
                let code = code(&mut d)?;
                d.reserved(4)?;
                Self::SessionReset(StreamFault {
                    stream_seq,
                    offset,
                    opcode,
                    code,
                })
            }
            ReplyOpcode::RecordError => {
                let stream_seq = d.u64("stream_seq")?;
                let record_index = d.u32("record_index")?;
                let opcode = d.u16("opcode")?;
                let code = code(&mut d)?;
                let object_id = ObjectId::new(d.u32("object_id")?);
                d.reserved(4)?;
                let detail = d.u64("detail")?;
                Self::RecordError(RecordError {
                    stream_seq,
                    record_index,
                    opcode,
                    code,
                    object_id,
                    detail,
                })
            }
            ReplyOpcode::CommandBufferStatus => {
                let submission_id = d.u64("submission_id")?;
                let status = d.u8("status")?;
                d.reserved(1)?;
                let raw_code = d.u16("code")?;
                d.reserved(4)?;
                let status = match (status, raw_code) {
                    (0, 0) => CommandBufferStatus::Completed,
                    (1, c) => CommandBufferStatus::Error(
                        ErrorCode::from_raw(c).ok_or_else(|| d.err("code"))?,
                    ),
                    (0, _) => return Err(d.err("code")),
                    _ => return Err(d.err("status")),
                };
                Self::CommandBufferStatus(SubmissionStatus {
                    submission_id,
                    status,
                })
            }
            ReplyOpcode::EventSignaled => {
                let event_id = d.id("event_id")?;
                d.reserved(4)?;
                Self::EventSignaled(EventValue {
                    event_id,
                    value: d.u64("value")?,
                })
            }
        };
        d.finish()?;
        Ok(reply)
    }
}

/// Decodes and fully validates a complete reply stream.
///
/// Reference decoder for the guest driver and fuzz target for the reply channel: returns
/// `Ok` or `Err` for every input and never panics. Memory use is bounded by the input
/// length (one entry per decoded record, never by the declared count).
///
/// # Errors
///
/// The first [`ProtoError`] encountered (framing, unknown reply opcode, invalid field).
pub fn decode_reply_stream(bytes: &[u8]) -> Result<Vec<Reply>, ProtoError> {
    let mut out = Vec::new();
    for frame in Frames::<ReplyOpcode>::new(bytes)? {
        let frame = frame?;
        out.push(Reply::decode(frame.opcode, frame.payload, frame.offset)?);
    }
    Ok(out)
}
