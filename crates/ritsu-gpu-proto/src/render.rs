//! Payloads of the render-encoder opcodes added after M0: the full render pass descriptor
//! (`BeginRenderPass2`), deferred store actions and the bind commands (gpu.md §9.2).
//!
//! Wire-level checks only (lengths, reserved bytes, list structure, protocol enums such as the
//! attachment kind); Metal values travel raw and are validated by the host object model.

use crate::command::{MAX_COLOR_ATTACHMENTS, ObjectId};
use crate::error::ProtoError;
use crate::reader::Dec;
use crate::records::{RecordList, WireRecord, sealed};

/// Resolve destination of an attachment (16 bytes on the wire, inside [`AttachmentRecord`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveRecord {
    /// Resolve texture.
    pub texture_id: ObjectId,
    /// `resolveLevel`.
    pub level: u32,
    /// `resolveSlice`.
    pub slice: u32,
    /// `resolveDepthPlane`.
    pub depth_plane: u32,
}

/// Common part of every attachment of `BeginRenderPass2` (40 bytes):
///
/// ```text
/// texture_id u32 | load_action u8 | store_action u8 | extra u8 | reserved u8 |
/// store_action_options u32 | level u32 | slice u32 | depth_plane u32 |
/// resolve_texture_id u32 | resolve_level u32 | resolve_slice u32 | resolve_depth_plane u32
/// ```
///
/// `extra` is the slot index for colour attachments, the raw resolve filter for depth and
/// stencil. Without a resolve texture (`resolve_texture_id` = 0) the three resolve fields must
/// be 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentRecord {
    /// Texture rendered into.
    pub texture_id: ObjectId,
    /// Raw `MTLLoadAction`.
    pub load_action: u8,
    /// Raw `MTLStoreAction`.
    pub store_action: u8,
    /// Raw `MTLStoreActionOptions`.
    pub store_action_options: u32,
    /// `level`.
    pub level: u32,
    /// `slice`.
    pub slice: u32,
    /// `depthPlane`.
    pub depth_plane: u32,
    /// Resolve destination (`resolveTexture` and its subresource).
    pub resolve: Option<ResolveRecord>,
}

impl AttachmentRecord {
    /// Level 0, slice 0, no options, no resolve.
    #[must_use]
    pub const fn new(texture_id: ObjectId, load_action: u8, store_action: u8) -> Self {
        Self {
            texture_id,
            load_action,
            store_action,
            store_action_options: 0,
            level: 0,
            slice: 0,
            depth_plane: 0,
            resolve: None,
        }
    }

    /// Decodes the common part; returns it with the `extra` byte.
    fn decode(d: &mut Dec<'_>) -> Result<(Self, u8), ProtoError> {
        let texture_id = d.id("texture_id")?;
        let load_action = d.u8("load_action")?;
        let store_action = d.u8("store_action")?;
        let extra = d.u8("attachment_extra")?;
        d.reserved(1)?;
        let store_action_options = d.u32("store_action_options")?;
        let level = d.u32("level")?;
        let slice = d.u32("slice")?;
        let depth_plane = d.u32("depth_plane")?;
        let resolve_id = d.opt_id("resolve_texture_id")?;
        let resolve_level = d.u32("resolve_level")?;
        let resolve_slice = d.u32("resolve_slice")?;
        let resolve_depth_plane = d.u32("resolve_depth_plane")?;
        let resolve = match resolve_id {
            Some(texture_id) => Some(ResolveRecord {
                texture_id,
                level: resolve_level,
                slice: resolve_slice,
                depth_plane: resolve_depth_plane,
            }),
            // A subresource without a texture would be silently ignored: reject it.
            None if (resolve_level | resolve_slice | resolve_depth_plane) != 0 => {
                return Err(d.err("resolve_texture_id"));
            }
            None => None,
        };
        Ok((
            Self {
                texture_id,
                load_action,
                store_action,
                store_action_options,
                level,
                slice,
                depth_plane,
                resolve,
            },
            extra,
        ))
    }

    fn encode(&self, extra: u8, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.texture_id.get().to_le_bytes());
        out.extend_from_slice(&[self.load_action, self.store_action, extra, 0]);
        for v in [
            self.store_action_options,
            self.level,
            self.slice,
            self.depth_plane,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let r = self.resolve.map_or([0; 4], |r| {
            [r.texture_id.get(), r.level, r.slice, r.depth_plane]
        });
        for v in r {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// Colour attachment of `BeginRenderPass2` (72 bytes): [`AttachmentRecord`] with
/// `extra` = slot index, then `clear_color f64 × 4` (`MTLClearColor`, double precision).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorAttachment2 {
    /// Colour slot (< [`MAX_COLOR_ATTACHMENTS`], strictly increasing in a list).
    pub index: u8,
    /// Texture, actions, subresource, resolve.
    pub attachment: AttachmentRecord,
    /// Clear colour (red, green, blue, alpha).
    pub clear_color: [f64; 4],
}

impl sealed::Decode for ColorAttachment2 {
    fn decode(input: sealed::Input<'_, '_>) -> Result<Self, ProtoError> {
        let d = input.0;
        let (attachment, index) = AttachmentRecord::decode(d)?;
        Ok(Self {
            index,
            attachment,
            clear_color: [
                d.f64("clear_red")?,
                d.f64("clear_green")?,
                d.f64("clear_blue")?,
                d.f64("clear_alpha")?,
            ],
        })
    }
}

impl WireRecord for ColorAttachment2 {
    const LEN: usize = 72;
    // MAX_COLOR_ATTACHMENTS = 8 fits in u8.
    const INDEX_LIMIT: u8 = MAX_COLOR_ATTACHMENTS as u8;
    const MAX_ENTRIES: usize = MAX_COLOR_ATTACHMENTS;
    const FIELD: &'static str = "color_attachments";

    fn index(&self) -> Option<u8> {
        Some(self.index)
    }

    fn encode(&self, out: &mut Vec<u8>) {
        self.attachment.encode(self.index, out);
        for c in self.clear_color {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
}

/// Depth attachment of `BeginRenderPass2` (48 bytes): [`AttachmentRecord`] with
/// `extra` = raw `MTLMultisampleDepthResolveFilter`, then `clear_depth f64`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthAttachment2 {
    /// Texture, actions, subresource, resolve.
    pub attachment: AttachmentRecord,
    /// Raw `MTLMultisampleDepthResolveFilter`.
    pub resolve_filter: u8,
    /// `clearDepth`.
    pub clear_depth: f64,
}

/// Stencil attachment of `BeginRenderPass2` (48 bytes): [`AttachmentRecord`] with
/// `extra` = raw `MTLMultisampleStencilResolveFilter`, then `clear_stencil u32 | reserved u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StencilAttachment2 {
    /// Texture, actions, subresource, resolve.
    pub attachment: AttachmentRecord,
    /// Raw `MTLMultisampleStencilResolveFilter`.
    pub resolve_filter: u8,
    /// `clearStencil`.
    pub clear_stencil: u32,
}

const DEPTH_LEN: usize = 48;
const STENCIL_LEN: usize = 48;
const PASS_HAS_DEPTH: u8 = 1 << 0;
const PASS_HAS_STENCIL: u8 = 1 << 1;

/// `BeginRenderPass2` payload: 24-byte header, `color_count` × [`ColorAttachment2`], then the
/// depth and stencil attachment if flagged.
///
/// ```text
/// render_target_width u32 | render_target_height u32 | render_target_array_length u32 |
/// default_raster_sample_count u32 | color_count u8 | flags u8 | reserved u16 | reserved u32
/// ```
///
/// `flags`: bit 0 depth attachment present, bit 1 stencil attachment present.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeginRenderPass2<'a> {
    /// `renderTargetWidth` (0 = smallest attachment).
    pub render_target_width: u32,
    /// `renderTargetHeight` (0 = smallest attachment).
    pub render_target_height: u32,
    /// `renderTargetArrayLength` (0 = not layered).
    pub render_target_array_length: u32,
    /// `defaultRasterSampleCount` (passes without attachments).
    pub default_raster_sample_count: u32,
    /// Colour attachments, sparse, ordered by slot.
    pub color_attachments: RecordList<'a, ColorAttachment2>,
    /// Depth attachment.
    pub depth_attachment: Option<DepthAttachment2>,
    /// Stencil attachment.
    pub stencil_attachment: Option<StencilAttachment2>,
}

impl<'a> BeginRenderPass2<'a> {
    #[inline(never)] // keep `Command::decode` small (Q-PERF-3)
    pub(crate) fn decode(d: &mut Dec<'a>) -> Result<Self, ProtoError> {
        let render_target_width = d.u32("render_target_width")?;
        let render_target_height = d.u32("render_target_height")?;
        let render_target_array_length = d.u32("render_target_array_length")?;
        let default_raster_sample_count = d.u32("default_raster_sample_count")?;
        let color_count = usize::from(d.u8("color_count")?);
        let flags = d.flags("flags", PASS_HAS_DEPTH | PASS_HAS_STENCIL)?;
        d.reserved(6)?;
        if color_count > MAX_COLOR_ATTACHMENTS {
            return Err(d.err("color_count"));
        }
        let colors_len = color_count * ColorAttachment2::LEN;
        let mut needed = colors_len;
        if flags & PASS_HAS_DEPTH != 0 {
            needed += DEPTH_LEN;
        }
        if flags & PASS_HAS_STENCIL != 0 {
            needed += STENCIL_LEN;
        }
        if d.cur.rest_len() != needed {
            return Err(d.err("render_pass_tail"));
        }
        let colors_at = d.payload_pos();
        let colors = d
            .cur
            .bytes(colors_len)
            .ok_or_else(|| d.err("color_attachments"))?;
        let color_attachments = RecordList::parse_at(colors, d.offset, colors_at)?;
        let depth_attachment = if flags & PASS_HAS_DEPTH != 0 {
            let (attachment, resolve_filter) = AttachmentRecord::decode(d)?;
            Some(DepthAttachment2 {
                attachment,
                resolve_filter,
                clear_depth: d.f64("clear_depth")?,
            })
        } else {
            None
        };
        let stencil_attachment = if flags & PASS_HAS_STENCIL != 0 {
            let (attachment, resolve_filter) = AttachmentRecord::decode(d)?;
            let clear_stencil = d.u32("clear_stencil")?;
            d.reserved(4)?;
            Some(StencilAttachment2 {
                attachment,
                resolve_filter,
                clear_stencil,
            })
        } else {
            None
        };
        Ok(Self {
            render_target_width,
            render_target_height,
            render_target_array_length,
            default_raster_sample_count,
            color_attachments,
            depth_attachment,
            stencil_attachment,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        for v in [
            self.render_target_width,
            self.render_target_height,
            self.render_target_array_length,
            self.default_raster_sample_count,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let mut flags = 0;
        if self.depth_attachment.is_some() {
            flags |= PASS_HAS_DEPTH;
        }
        if self.stencil_attachment.is_some() {
            flags |= PASS_HAS_STENCIL;
        }
        // At most MAX_COLOR_ATTACHMENTS entries by construction of the list.
        debug_assert!(self.color_attachments.len() <= MAX_COLOR_ATTACHMENTS);
        let count = u8::try_from(self.color_attachments.len()).unwrap_or(u8::MAX);
        out.extend_from_slice(&[count, flags, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(self.color_attachments.as_bytes());
        if let Some(depth) = &self.depth_attachment {
            depth.attachment.encode(depth.resolve_filter, out);
            out.extend_from_slice(&depth.clear_depth.to_le_bytes());
        }
        if let Some(stencil) = &self.stencil_attachment {
            stencil.attachment.encode(stencil.resolve_filter, out);
            out.extend_from_slice(&stencil.clear_stencil.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
        }
    }
}

/// Which attachment a [`SetStoreAction`] addresses (protocol enum, checked on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttachmentKind {
    /// Colour attachment with this slot index (< [`MAX_COLOR_ATTACHMENTS`]).
    Color(u8),
    /// The depth attachment.
    Depth,
    /// The stencil attachment.
    Stencil,
}

/// `SetStoreAction` payload (8 bytes):
/// `kind u8 (0 colour, 1 depth, 2 stencil) | index u8 | store_action u8 | reserved u8 |
/// reserved u32`. `index` must be 0 for depth and stencil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetStoreAction {
    /// Attachment.
    pub attachment: AttachmentKind,
    /// Raw `MTLStoreAction`.
    pub store_action: u8,
}

impl SetStoreAction {
    #[inline(never)] // keep `Command::decode` small (Q-PERF-3)
    pub(crate) fn decode(d: &mut Dec<'_>) -> Result<Self, ProtoError> {
        let kind = d.u8("attachment_kind")?;
        let index = d.u8("attachment_index")?;
        let store_action = d.u8("store_action")?;
        d.reserved(5)?;
        let attachment = match (kind, index) {
            (0, i) if usize::from(i) < MAX_COLOR_ATTACHMENTS => AttachmentKind::Color(i),
            (1, 0) => AttachmentKind::Depth,
            (2, 0) => AttachmentKind::Stencil,
            _ => return Err(d.err("attachment")),
        };
        Ok(Self {
            attachment,
            store_action,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        let (kind, index) = match self.attachment {
            AttachmentKind::Color(i) => {
                debug_assert!(
                    usize::from(i) < MAX_COLOR_ATTACHMENTS,
                    "colour attachment index out of range"
                );
                (0, i)
            }
            AttachmentKind::Depth => (1, 0),
            AttachmentKind::Stencil => (2, 0),
        };
        out.extend_from_slice(&[kind, index, self.store_action, 0, 0, 0, 0, 0]);
    }
}

/// `SetVertexBuffer` payload (24 bytes):
/// `buffer_id u32 (0 = nil) | index u32 | offset u64 | attribute_stride u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetVertexBuffer {
    /// Buffer (`None` = unbind).
    pub buffer_id: Option<ObjectId>,
    /// Buffer argument table index.
    pub index: u32,
    /// Byte offset.
    pub offset: u64,
    /// `attributeStride` for layouts with dynamic stride; [`SetVertexBuffer::STRIDE_STATIC`]
    /// = not given (`setVertexBuffer:offset:atIndex:`).
    pub attribute_stride: u64,
}

impl SetVertexBuffer {
    /// `MTLAttributeStrideStatic` (`NSUIntegerMax`): the layout's stride applies.
    pub const STRIDE_STATIC: u64 = u64::MAX;

    #[inline(never)] // keep `Command::decode` small (Q-PERF-3)
    pub(crate) fn decode(d: &mut Dec<'_>) -> Result<Self, ProtoError> {
        Ok(Self {
            buffer_id: d.opt_id("buffer_id")?,
            index: d.u32("index")?,
            offset: d.u64("offset")?,
            attribute_stride: d.u64("attribute_stride")?,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.buffer_id.map_or(0, ObjectId::get).to_le_bytes());
        out.extend_from_slice(&self.index.to_le_bytes());
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.attribute_stride.to_le_bytes());
    }
}

/// Payload of the texture and sampler bind opcodes (8 bytes):
/// `object_id u32 (0 = nil) | index u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Texture or sampler state (`None` = unbind).
    pub object_id: Option<ObjectId>,
    /// Argument table index.
    pub index: u32,
}

impl Binding {
    #[inline(never)] // keep `Command::decode` small (Q-PERF-3)
    pub(crate) fn decode(d: &mut Dec<'_>) -> Result<Self, ProtoError> {
        Ok(Self {
            object_id: d.opt_id("object_id")?,
            index: d.u32("index")?,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.object_id.map_or(0, ObjectId::get).to_le_bytes());
        out.extend_from_slice(&self.index.to_le_bytes());
    }
}
