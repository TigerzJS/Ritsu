//! Typed payloads of the version-1 opcodes.
//!
//! Decoding here checks only *wire-level* validity (lengths, reserved bytes, non-zero ids,
//! internal consistency). Semantic validation against the Metal object model (storage modes,
//! pixel formats, limits) happens in `ritsu-gpu-host`, which knows the device capabilities.
//! Enum-like fields are therefore carried as raw integers.

use core::num::NonZeroU32;

use crate::error::ProtoError;
use crate::reader::Cursor;
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

/// `CreateTexture` payload (32 bytes).
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
}

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
        /// Texture to destroy.
        texture_id: ObjectId,
    },
    /// See [`Opcode::BeginRenderPass`].
    BeginRenderPass(BeginRenderPass),
    /// See [`Opcode::EndRenderPass`].
    EndRenderPass,
    /// See [`Opcode::Draw`].
    Draw(Draw),
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

/// Payload decoding context: turns `None` from the cursor into a positioned error.
struct Dec<'a> {
    cur: Cursor<'a>,
    offset: usize,
}

impl<'a> Dec<'a> {
    fn err(&self, field: &'static str) -> ProtoError {
        ProtoError::InvalidField {
            offset: self.offset,
            field,
        }
    }
    fn u8(&mut self, f: &'static str) -> Result<u8, ProtoError> {
        self.cur.u8().ok_or_else(|| self.err(f))
    }
    fn u16(&mut self, f: &'static str) -> Result<u16, ProtoError> {
        self.cur.u16().ok_or_else(|| self.err(f))
    }
    fn u32(&mut self, f: &'static str) -> Result<u32, ProtoError> {
        self.cur.u32().ok_or_else(|| self.err(f))
    }
    fn u64(&mut self, f: &'static str) -> Result<u64, ProtoError> {
        self.cur.u64().ok_or_else(|| self.err(f))
    }
    fn f32(&mut self, f: &'static str) -> Result<f32, ProtoError> {
        self.cur.f32().ok_or_else(|| self.err(f))
    }
    fn id(&mut self, f: &'static str) -> Result<ObjectId, ProtoError> {
        let raw = self.u32(f)?;
        ObjectId::new(raw).ok_or_else(|| self.err(f))
    }
    /// Reads `N` reserved bytes that must be zero.
    fn reserved(&mut self, n: usize) -> Result<(), ProtoError> {
        for _ in 0..n {
            // Record offset + record header + position inside payload.
            let at = self.offset + crate::RECORD_HEADER_LEN + self.cur.pos();
            if self.u8("reserved")? != 0 {
                return Err(ProtoError::ReservedNonZero { offset: at });
            }
        }
        Ok(())
    }
}

/// Size of one colour attachment on the wire.
const ATTACHMENT_LEN: usize = 24;

impl<'a> Command<'a> {
    /// Opcode of this command.
    #[must_use]
    pub const fn opcode(&self) -> Opcode {
        match self {
            Self::Nop => Opcode::Nop,
            Self::CreateBuffer(_) => Opcode::CreateBuffer,
            Self::DestroyBuffer { .. } => Opcode::DestroyBuffer,
            Self::WriteBuffer(_) => Opcode::WriteBuffer,
            Self::CopyBuffer(_) => Opcode::CopyBuffer,
            Self::CreateTexture(_) => Opcode::CreateTexture,
            Self::DestroyTexture { .. } => Opcode::DestroyTexture,
            Self::BeginRenderPass(_) => Opcode::BeginRenderPass,
            Self::EndRenderPass => Opcode::EndRenderPass,
            Self::Draw(_) => Opcode::Draw,
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
        let mut d = Dec {
            cur: Cursor::new(record.payload),
            offset: record.offset,
        };
        let cmd = match record.opcode {
            Opcode::Nop => {
                d.cur.rest();
                Self::Nop
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
                Self::CreateTexture(CreateTexture {
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
                })
            }
            Opcode::DestroyTexture => Self::DestroyTexture {
                texture_id: d.id("texture_id")?,
            },
            Opcode::BeginRenderPass => Self::BeginRenderPass(decode_render_pass(&mut d)?),
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
        };
        // Exact-length opcodes are fully consumed by construction; this guards against a
        // mismatch between `payload_len` rules and the field lists above.
        if !d.cur.rest().is_empty() {
            return Err(d.err("payload_tail"));
        }
        Ok(cmd)
    }

    /// Appends the payload bytes (without record header or padding) to `out`.
    pub fn encode_payload(&self, out: &mut Vec<u8>) {
        match self {
            Self::Nop | Self::EndRenderPass | Self::BeginComputePass | Self::EndComputePass => {}
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
            }
            Self::DestroyTexture { texture_id } => {
                out.extend_from_slice(&texture_id.get().to_le_bytes());
            }
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
    let mut ad = Dec {
        cur: Cursor::new(tail),
        offset: d.offset,
    };
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
