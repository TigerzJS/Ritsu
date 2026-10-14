//! Typed payloads of the version-1 opcodes.
//!
//! Decoding here checks only *wire-level* validity (lengths, reserved bytes, non-zero ids,
//! internal consistency). Semantic validation against the Metal object model (storage modes,
//! pixel formats, limits) happens in `ritsu-gpu-host`, which knows the device capabilities.
//! Enum-like fields are therefore carried as raw integers.

use core::num::NonZeroU32;

use crate::error::ProtoError;
use crate::negotiate::{Features, Hello};
use crate::objects::{
    CreateDepthStencilState, CreateRenderPipelineState, CreateSampler, CreateTextureView,
    SWIZZLE_IDENTITY,
};
use crate::reader::Dec;
use crate::render::{BeginRenderPass2, Binding, SetStoreAction, SetVertexBuffer};
use crate::wire::{Opcode, Record};

/// Maximum number of colour attachments per render pass (Metal limit on Apple GPUs).
pub const MAX_COLOR_ATTACHMENTS: usize = 8;

/// Guest-chosen object id. Id 0 is reserved as "null" and rejected by the decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub NonZeroU32);

impl ObjectId {
    /// Creates an id from a raw value; `None` for 0.
    #[must_use]
    pub const fn new(raw: u32) -> Option<Self> {
        match NonZeroU32::new(raw) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// Raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// `CreateBuffer` payload (16 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateBuffer {
    /// Guest-chosen id of the new buffer.
    pub buffer_id: ObjectId,
    /// Raw `MTLStorageMode`.
    pub storage_mode: u8,
    /// Raw `MTLCPUCacheMode`.
    pub cpu_cache_mode: u8,
    /// Raw `MTLHazardTrackingMode`.
    pub hazard_tracking: u8,
    /// Length in bytes.
    pub length: u64,
}

/// `WriteBuffer` payload (16 bytes + data), borrowing the data from the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteBuffer<'a> {
    /// Destination buffer.
    pub buffer_id: ObjectId,
    /// Destination byte offset.
    pub offset: u64,
    /// Bytes to write.
    pub data: &'a [u8],
}

/// `CopyBuffer` payload (32 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyBuffer {
    /// Source buffer.
    pub src: ObjectId,
    /// Destination buffer.
    pub dst: ObjectId,
    /// Source byte offset.
    pub src_offset: u64,
    /// Destination byte offset.
    pub dst_offset: u64,
    /// Number of bytes.
    pub size: u64,
}

/// `CreateTexture` payload: 32 bytes, or 40 bytes with the swizzle extension
/// (`swizzle [u8; 4] | reserved u32`). The encoder emits the extension only for a non-identity
/// swizzle, so streams without swizzles keep the original 32-byte layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateTexture {
    /// Guest-chosen id of the new texture.
    pub texture_id: ObjectId,
    /// Raw `MTLPixelFormat`.
    pub pixel_format: u16,
    /// Raw `MTLTextureUsage` bits.
    pub usage: u16,
    /// Raw `MTLTextureType`.
    pub texture_type: u8,
    /// Raw `MTLStorageMode`.
    pub storage_mode: u8,
    /// Samples per pixel.
    pub sample_count: u8,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Depth in pixels (3D textures).
    pub depth: u32,
    /// Number of array elements (cube arrays: number of cubes).
    pub array_length: u32,
    /// Number of mip levels.
    pub mip_level_count: u32,
    /// Raw `MTLTextureSwizzle` per output channel (red, green, blue, alpha);
    /// [`SWIZZLE_IDENTITY`] when the 32-byte form is used.
    pub swizzle: [u8; 4],
}

/// Length of the `CreateTexture` base layout.
const CREATE_TEXTURE_LEN: usize = 32;

/// One colour attachment of a `BeginRenderPass` record (24 bytes on the wire).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorAttachmentRecord {
    /// Attachment texture.
    pub texture_id: ObjectId,
    /// Raw `MTLLoadAction`.
    pub load_action: u8,
    /// Raw `MTLStoreAction`.
    pub store_action: u8,
    /// Clear colour (RGBA). `MTLClearColor` is double precision; the guest driver converts.
    pub clear_color: [f32; 4],
}

/// `BeginRenderPass` payload: 16-byte fixed part + `count` × 24-byte attachments.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeginRenderPass {
    /// Render area width (0 = derive from attachments).
    pub width: u32,
    /// Render area height (0 = derive from attachments).
    pub height: u32,
    count: u8,
    attachments: [Option<ColorAttachmentRecord>; MAX_COLOR_ATTACHMENTS],
}

impl BeginRenderPass {
    /// Builds a render pass record. Returns `None` if more than [`MAX_COLOR_ATTACHMENTS`]
    /// attachments are given.
    #[must_use]
    pub fn new(width: u32, height: u32, attachments: &[ColorAttachmentRecord]) -> Option<Self> {
        if attachments.len() > MAX_COLOR_ATTACHMENTS {
            return None;
        }
        let mut arr = [None; MAX_COLOR_ATTACHMENTS];
        for (slot, a) in arr.iter_mut().zip(attachments) {
            *slot = Some(*a);
        }
        Some(Self {
            width,
            height,
            count: u8::try_from(attachments.len()).ok()?,
            attachments: arr,
        })
    }

    /// The colour attachments in order.
    pub fn color_attachments(&self) -> impl Iterator<Item = &ColorAttachmentRecord> {
        self.attachments
            .iter()
            .take(usize::from(self.count))
            .flatten()
    }

    /// Number of colour attachments.
    #[must_use]
    pub fn color_attachment_count(&self) -> usize {
        usize::from(self.count)
    }
}

/// `Draw` payload (20 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Draw {
    /// Raw `MTLPrimitiveType`.
    pub primitive_type: u8,
    /// First vertex.
    pub vertex_start: u32,
    /// Number of vertices.
    pub vertex_count: u32,
    /// Number of instances.
    pub instance_count: u32,
    /// First instance.
    pub base_instance: u32,
}

/// `Dispatch` payload (24 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dispatch {
    /// Threadgroups per grid (x, y, z).
    pub threadgroups: [u32; 3],
    /// Threads per threadgroup (x, y, z).
    pub threads_per_threadgroup: [u32; 3],
}

/// `SignalEvent` / `WaitEvent` payload (16 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventValue {
    /// Shared event id.
    pub event_id: ObjectId,
    /// Timeline value.
    pub value: u64,
}

/// A decoded, wire-valid command.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Command<'a> {
    /// See [`Opcode::Nop`].
    Nop,
    /// See [`Opcode::Hello`]. Only valid as the single record of the first stream of a
    /// session (see [`crate::decode_hello_stream`]).
    Hello(Hello),
    /// See [`Opcode::CreateBuffer`].
    CreateBuffer(CreateBuffer),
    /// See [`Opcode::DestroyBuffer`].
    DestroyBuffer {
        /// Buffer to destroy.
        buffer_id: ObjectId,
    },
    /// See [`Opcode::WriteBuffer`].
    WriteBuffer(WriteBuffer<'a>),
    /// See [`Opcode::CopyBuffer`].
    CopyBuffer(CopyBuffer),
    /// See [`Opcode::CreateTexture`].
    CreateTexture(CreateTexture),
    /// See [`Opcode::DestroyTexture`].
    DestroyTexture {
        /// Texture (or texture view) to destroy.
        texture_id: ObjectId,
    },
    /// See [`Opcode::CreateTextureView`].
    CreateTextureView(CreateTextureView),
    /// See [`Opcode::CreateSampler`].
    CreateSampler(CreateSampler),
    /// See [`Opcode::DestroySampler`].
    DestroySampler {
        /// Sampler state to destroy.
        sampler_id: ObjectId,
    },
    /// See [`Opcode::CreateDepthStencilState`].
    CreateDepthStencilState(CreateDepthStencilState),
    /// See [`Opcode::DestroyDepthStencilState`].
    DestroyDepthStencilState {
        /// Depth/stencil state to destroy.
        dss_id: ObjectId,
    },
    /// See [`Opcode::CreateRenderPipelineState`].
    CreateRenderPipelineState(CreateRenderPipelineState<'a>),
    /// See [`Opcode::DestroyRenderPipelineState`].
    DestroyRenderPipelineState {
        /// Pipeline state to destroy.
        pipeline_id: ObjectId,
    },
    /// See [`Opcode::BeginRenderPass`].
    BeginRenderPass(BeginRenderPass),
    /// See [`Opcode::EndRenderPass`].
    EndRenderPass,
    /// See [`Opcode::Draw`].
    Draw(Draw),
    /// See [`Opcode::BeginRenderPass2`].
    BeginRenderPass2(BeginRenderPass2<'a>),
    /// See [`Opcode::SetStoreAction`].
    SetStoreAction(SetStoreAction),
    /// See [`Opcode::SetRenderPipelineState`].
    SetRenderPipelineState {
        /// Pipeline state.
        pipeline_id: ObjectId,
    },
    /// See [`Opcode::SetDepthStencilState`].
    SetDepthStencilState {
        /// Depth/stencil state (`None` = `nil`, the default state).
        dss_id: Option<ObjectId>,
    },
    /// See [`Opcode::SetVertexBuffer`].
    SetVertexBuffer(SetVertexBuffer),
    /// See [`Opcode::SetVertexTexture`].
    SetVertexTexture(Binding),
    /// See [`Opcode::SetFragmentTexture`].
    SetFragmentTexture(Binding),
    /// See [`Opcode::SetVertexSamplerState`].
    SetVertexSamplerState(Binding),
    /// See [`Opcode::SetFragmentSamplerState`].
    SetFragmentSamplerState(Binding),
    /// See [`Opcode::BeginComputePass`].
    BeginComputePass,
    /// See [`Opcode::EndComputePass`].
    EndComputePass,
    /// See [`Opcode::Dispatch`].
    Dispatch(Dispatch),
    /// See [`Opcode::SignalEvent`].
    SignalEvent(EventValue),
    /// See [`Opcode::WaitEvent`].
    WaitEvent(EventValue),
    /// See [`Opcode::Commit`].
    Commit {
        /// Guest-assigned submission id, echoed in the completion notification.
        submission_id: u64,
    },
}

/// Size of one colour attachment on the wire.
const ATTACHMENT_LEN: usize = 24;

impl<'a> Command<'a> {
    /// Opcode of this command.
    #[must_use]
    pub const fn opcode(&self) -> Opcode {
        match self {
            Self::Nop => Opcode::Nop,
            Self::Hello(_) => Opcode::Hello,
            Self::CreateBuffer(_) => Opcode::CreateBuffer,
            Self::DestroyBuffer { .. } => Opcode::DestroyBuffer,
            Self::WriteBuffer(_) => Opcode::WriteBuffer,
            Self::CopyBuffer(_) => Opcode::CopyBuffer,
            Self::CreateTexture(_) => Opcode::CreateTexture,
            Self::DestroyTexture { .. } => Opcode::DestroyTexture,
            Self::CreateTextureView(_) => Opcode::CreateTextureView,
            Self::CreateSampler(_) => Opcode::CreateSampler,
            Self::DestroySampler { .. } => Opcode::DestroySampler,
            Self::CreateDepthStencilState(_) => Opcode::CreateDepthStencilState,
            Self::DestroyDepthStencilState { .. } => Opcode::DestroyDepthStencilState,
            Self::CreateRenderPipelineState(_) => Opcode::CreateRenderPipelineState,
            Self::DestroyRenderPipelineState { .. } => Opcode::DestroyRenderPipelineState,
            Self::BeginRenderPass(_) => Opcode::BeginRenderPass,
            Self::EndRenderPass => Opcode::EndRenderPass,
            Self::Draw(_) => Opcode::Draw,
            Self::BeginRenderPass2(_) => Opcode::BeginRenderPass2,
            Self::SetStoreAction(_) => Opcode::SetStoreAction,
            Self::SetRenderPipelineState { .. } => Opcode::SetRenderPipelineState,
            Self::SetDepthStencilState { .. } => Opcode::SetDepthStencilState,
            Self::SetVertexBuffer(_) => Opcode::SetVertexBuffer,
            Self::SetVertexTexture(_) => Opcode::SetVertexTexture,
            Self::SetFragmentTexture(_) => Opcode::SetFragmentTexture,
            Self::SetVertexSamplerState(_) => Opcode::SetVertexSamplerState,
            Self::SetFragmentSamplerState(_) => Opcode::SetFragmentSamplerState,
            Self::BeginComputePass => Opcode::BeginComputePass,
            Self::EndComputePass => Opcode::EndComputePass,
            Self::Dispatch(_) => Opcode::Dispatch,
            Self::SignalEvent(_) => Opcode::SignalEvent,
            Self::WaitEvent(_) => Opcode::WaitEvent,
            Self::Commit { .. } => Opcode::Commit,
        }
    }

    /// Decodes the payload of a framed record.
    ///
    /// The record's payload length has already been checked against
    /// [`Opcode::payload_len`] by the [`crate::RecordIter`].
    ///
    /// # Errors
    ///
    /// [`ProtoError::InvalidField`] or [`ProtoError::ReservedNonZero`] for wire-invalid payloads.
    pub fn decode(record: &Record<'a>) -> Result<Self, ProtoError> {
        let mut d = Dec::new(record.payload, record.offset);
        // Decoded straight into the result (no extra move of the ~200-byte `Command` on the
        // per-record hot path; Q-PERF-3 instruction-count gate).
        let result = Self::decode_fields(record, &mut d);
        // Exact-length opcodes are fully consumed by construction; this guards against a
        // mismatch between `payload_len` rules and the field lists.
        if result.is_ok() {
            d.finish()?;
        }
        result
    }

    fn decode_fields(record: &Record<'a>, d: &mut Dec<'a>) -> Result<Self, ProtoError> {
        Ok(match record.opcode {
            Opcode::Nop => {
                d.cur.rest();
                Self::Nop
            }
            Opcode::Hello => {
                let min_version = d.u16("min_version")?;
                let max_version = d.u16("max_version")?;
                d.reserved(4)?;
                let features = Features::from_bits_retain(d.u64("features")?);
                // Version 0 does not exist; an empty range is meaningless.
                if min_version == 0 {
                    return Err(d.err("min_version"));
                }
                if max_version < min_version {
                    return Err(d.err("max_version"));
                }
                Self::Hello(Hello {
                    min_version,
                    max_version,
                    features,
                })
            }
            Opcode::CreateBuffer => {
                let buffer_id = d.id("buffer_id")?;
                let storage_mode = d.u8("storage_mode")?;
                let cpu_cache_mode = d.u8("cpu_cache_mode")?;
                let hazard_tracking = d.u8("hazard_tracking")?;
                d.reserved(1)?;
                let length = d.u64("length")?;
                Self::CreateBuffer(CreateBuffer {
                    buffer_id,
                    storage_mode,
                    cpu_cache_mode,
                    hazard_tracking,
                    length,
                })
            }
            Opcode::DestroyBuffer => Self::DestroyBuffer {
                buffer_id: d.id("buffer_id")?,
            },
            Opcode::WriteBuffer => {
                let buffer_id = d.id("buffer_id")?;
                d.reserved(4)?;
                let offset = d.u64("offset")?;
                Self::WriteBuffer(WriteBuffer {
                    buffer_id,
                    offset,
                    data: d.cur.rest(),
                })
            }
            Opcode::CopyBuffer => Self::CopyBuffer(CopyBuffer {
                src: d.id("src")?,
                dst: d.id("dst")?,
                src_offset: d.u64("src_offset")?,
                dst_offset: d.u64("dst_offset")?,
                size: d.u64("size")?,
            }),
            Opcode::CreateTexture => {
                let texture_id = d.id("texture_id")?;
                let pixel_format = d.u16("pixel_format")?;
                let usage = d.u16("usage")?;
                let texture_type = d.u8("texture_type")?;
                let storage_mode = d.u8("storage_mode")?;
                let sample_count = d.u8("sample_count")?;
                d.reserved(1)?;
                let mut t = CreateTexture {
                    texture_id,
                    pixel_format,
                    usage,
                    texture_type,
                    storage_mode,
                    sample_count,
                    width: d.u32("width")?,
                    height: d.u32("height")?,
                    depth: d.u32("depth")?,
                    array_length: d.u32("array_length")?,
                    mip_level_count: d.u32("mip_level_count")?,
                    swizzle: SWIZZLE_IDENTITY,
                };
                // Swizzle extension (the length rule allows exactly 32 or 40 bytes).
                if record.payload.len() > CREATE_TEXTURE_LEN {
                    t.swizzle = [
                        d.u8("swizzle")?,
                        d.u8("swizzle")?,
                        d.u8("swizzle")?,
                        d.u8("swizzle")?,
                    ];
                    d.reserved(4)?;
                }
                Self::CreateTexture(t)
            }
            Opcode::DestroyTexture => Self::DestroyTexture {
                texture_id: d.id("texture_id")?,
            },
            Opcode::CreateTextureView => Self::CreateTextureView(CreateTextureView::decode(d)?),
            Opcode::CreateSampler => Self::CreateSampler(CreateSampler::decode(d)?),
            Opcode::DestroySampler => Self::DestroySampler {
                sampler_id: d.id("sampler_id")?,
            },
            Opcode::CreateDepthStencilState => {
                Self::CreateDepthStencilState(CreateDepthStencilState::decode(d)?)
            }
            Opcode::DestroyDepthStencilState => Self::DestroyDepthStencilState {
                dss_id: d.id("dss_id")?,
            },
            Opcode::CreateRenderPipelineState => {
                Self::CreateRenderPipelineState(CreateRenderPipelineState::decode(d)?)
            }
            Opcode::DestroyRenderPipelineState => Self::DestroyRenderPipelineState {
                pipeline_id: d.id("pipeline_id")?,
            },
            Opcode::BeginRenderPass2 => Self::BeginRenderPass2(BeginRenderPass2::decode(d)?),
            Opcode::SetStoreAction => Self::SetStoreAction(SetStoreAction::decode(d)?),
            Opcode::SetRenderPipelineState => Self::SetRenderPipelineState {
                pipeline_id: d.id("pipeline_id")?,
            },
            Opcode::SetDepthStencilState => Self::SetDepthStencilState {
                dss_id: d.opt_id("dss_id")?,
            },
            Opcode::SetVertexBuffer => Self::SetVertexBuffer(SetVertexBuffer::decode(d)?),
            Opcode::SetVertexTexture => Self::SetVertexTexture(Binding::decode(d)?),
            Opcode::SetFragmentTexture => Self::SetFragmentTexture(Binding::decode(d)?),
            Opcode::SetVertexSamplerState => Self::SetVertexSamplerState(Binding::decode(d)?),
            Opcode::SetFragmentSamplerState => Self::SetFragmentSamplerState(Binding::decode(d)?),
            Opcode::BeginRenderPass => Self::BeginRenderPass(decode_render_pass(d)?),
            Opcode::EndRenderPass => Self::EndRenderPass,
            Opcode::Draw => {
                let primitive_type = d.u8("primitive_type")?;
                d.reserved(3)?;
                Self::Draw(Draw {
                    primitive_type,
                    vertex_start: d.u32("vertex_start")?,
                    vertex_count: d.u32("vertex_count")?,
                    instance_count: d.u32("instance_count")?,
                    base_instance: d.u32("base_instance")?,
                })
            }
            Opcode::BeginComputePass => Self::BeginComputePass,
            Opcode::EndComputePass => Self::EndComputePass,
            Opcode::Dispatch => Self::Dispatch(Dispatch {
                threadgroups: [d.u32("tg_x")?, d.u32("tg_y")?, d.u32("tg_z")?],
                threads_per_threadgroup: [d.u32("tpt_x")?, d.u32("tpt_y")?, d.u32("tpt_z")?],
            }),
            Opcode::SignalEvent | Opcode::WaitEvent => {
                let event_id = d.id("event_id")?;
                d.reserved(4)?;
                let ev = EventValue {
                    event_id,
                    value: d.u64("value")?,
                };
                if record.opcode == Opcode::SignalEvent {
                    Self::SignalEvent(ev)
                } else {
                    Self::WaitEvent(ev)
                }
            }
            Opcode::Commit => Self::Commit {
                submission_id: d.u64("submission_id")?,
            },
        })
    }

    /// Appends the payload bytes (without record header or padding) to `out`.
    pub fn encode_payload(&self, out: &mut Vec<u8>) {
        match self {
            Self::Nop | Self::EndRenderPass | Self::BeginComputePass | Self::EndComputePass => {}
            Self::Hello(h) => {
                out.extend_from_slice(&h.min_version.to_le_bytes());
                out.extend_from_slice(&h.max_version.to_le_bytes());
                out.extend_from_slice(&[0; 4]);
                out.extend_from_slice(&h.features.bits().to_le_bytes());
            }
            Self::CreateBuffer(c) => {
                out.extend_from_slice(&c.buffer_id.get().to_le_bytes());
                out.extend_from_slice(&[c.storage_mode, c.cpu_cache_mode, c.hazard_tracking, 0]);
                out.extend_from_slice(&c.length.to_le_bytes());
            }
            Self::DestroyBuffer { buffer_id } => {
                out.extend_from_slice(&buffer_id.get().to_le_bytes());
            }
            Self::WriteBuffer(w) => {
                out.extend_from_slice(&w.buffer_id.get().to_le_bytes());
                out.extend_from_slice(&[0; 4]);
                out.extend_from_slice(&w.offset.to_le_bytes());
                out.extend_from_slice(w.data);
            }
            Self::CopyBuffer(c) => {
                out.extend_from_slice(&c.src.get().to_le_bytes());
                out.extend_from_slice(&c.dst.get().to_le_bytes());
                out.extend_from_slice(&c.src_offset.to_le_bytes());
                out.extend_from_slice(&c.dst_offset.to_le_bytes());
                out.extend_from_slice(&c.size.to_le_bytes());
            }
            Self::CreateTexture(t) => {
                out.extend_from_slice(&t.texture_id.get().to_le_bytes());
                out.extend_from_slice(&t.pixel_format.to_le_bytes());
                out.extend_from_slice(&t.usage.to_le_bytes());
                out.extend_from_slice(&[t.texture_type, t.storage_mode, t.sample_count, 0]);
                for v in [
                    t.width,
                    t.height,
                    t.depth,
                    t.array_length,
                    t.mip_level_count,
                ] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                if t.swizzle != SWIZZLE_IDENTITY {
                    out.extend_from_slice(&t.swizzle);
                    out.extend_from_slice(&[0; 4]);
                }
            }
            Self::DestroyTexture { texture_id: id }
            | Self::DestroySampler { sampler_id: id }
            | Self::DestroyDepthStencilState { dss_id: id }
            | Self::DestroyRenderPipelineState { pipeline_id: id }
            | Self::SetRenderPipelineState { pipeline_id: id } => {
                out.extend_from_slice(&id.get().to_le_bytes());
            }
            Self::SetDepthStencilState { dss_id } => {
                out.extend_from_slice(&dss_id.map_or(0, ObjectId::get).to_le_bytes());
            }
            Self::CreateTextureView(v) => v.encode(out),
            Self::CreateSampler(c) => c.encode(out),
            Self::CreateDepthStencilState(c) => c.encode(out),
            Self::CreateRenderPipelineState(c) => c.encode(out),
            Self::BeginRenderPass2(rp) => rp.encode(out),
            Self::SetStoreAction(s) => s.encode(out),
            Self::SetVertexBuffer(b) => b.encode(out),
            Self::SetVertexTexture(b)
            | Self::SetFragmentTexture(b)
            | Self::SetVertexSamplerState(b)
            | Self::SetFragmentSamplerState(b) => b.encode(out),
            Self::BeginRenderPass(rp) => {
                out.extend_from_slice(&rp.width.to_le_bytes());
                out.extend_from_slice(&rp.height.to_le_bytes());
                out.extend_from_slice(&u32::from(rp.count).to_le_bytes());
                out.extend_from_slice(&[0; 4]);
                for a in rp.color_attachments() {
                    out.extend_from_slice(&a.texture_id.get().to_le_bytes());
                    out.extend_from_slice(&[a.load_action, a.store_action, 0, 0]);
                    for c in a.clear_color {
                        out.extend_from_slice(&c.to_le_bytes());
                    }
                }
            }
            Self::Draw(dr) => {
                out.extend_from_slice(&[dr.primitive_type, 0, 0, 0]);
                for v in [
                    dr.vertex_start,
                    dr.vertex_count,
                    dr.instance_count,
                    dr.base_instance,
                ] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            Self::Dispatch(di) => {
                for v in di.threadgroups.iter().chain(&di.threads_per_threadgroup) {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            Self::SignalEvent(ev) | Self::WaitEvent(ev) => {
                out.extend_from_slice(&ev.event_id.get().to_le_bytes());
                out.extend_from_slice(&[0; 4]);
                out.extend_from_slice(&ev.value.to_le_bytes());
            }
            Self::Commit { submission_id } => {
                out.extend_from_slice(&submission_id.to_le_bytes());
            }
        }
    }
}

fn decode_render_pass(d: &mut Dec<'_>) -> Result<BeginRenderPass, ProtoError> {
    let width = d.u32("width")?;
    let height = d.u32("height")?;
    let count_raw = d.u32("color_attachment_count")?;
    d.reserved(4)?;
    let count = usize::try_from(count_raw)
        .ok()
        .filter(|&c| c <= MAX_COLOR_ATTACHMENTS)
        .ok_or_else(|| d.err("color_attachment_count"))?;
    // The tail must hold exactly `count` attachments; checked before reading them.
    let tail = d.cur.rest();
    if count.checked_mul(ATTACHMENT_LEN) != Some(tail.len()) {
        return Err(d.err("color_attachments"));
    }
    let mut ad = Dec::new(tail, d.offset);
    let mut attachments = [None; MAX_COLOR_ATTACHMENTS];
    for slot in attachments.iter_mut().take(count) {
        let texture_id = ad.id("texture_id")?;
        let load_action = ad.u8("load_action")?;
        let store_action = ad.u8("store_action")?;
        if ad.u16("reserved")? != 0 {
            return Err(ad.err("reserved"));
        }
        let clear_color = [
            ad.f32("clear_r")?,
            ad.f32("clear_g")?,
            ad.f32("clear_b")?,
            ad.f32("clear_a")?,
        ];
        *slot = Some(ColorAttachmentRecord {
            texture_id,
            load_action,
            store_action,
            clear_color,
        });
    }
    Ok(BeginRenderPass {
        width,
        height,
        count: u8::try_from(count).map_err(|_| d.err("color_attachment_count"))?,
        attachments,
    })
}
