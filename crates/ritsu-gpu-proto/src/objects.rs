//! Payloads of the object-creating opcodes added after M0: texture views, sampler states,
//! depth/stencil states and render pipeline states (gpu.md §9.2).
//!
//! As everywhere in this crate, only wire-level validity is checked here (lengths, reserved
//! bytes, flag bits, non-zero ids, list structure). Metal enum values travel as raw integers
//! (SDK values) and are validated by the host object model (`ritsu-gpu-host::model`).

use crate::command::{MAX_COLOR_ATTACHMENTS, ObjectId};
use crate::error::ProtoError;
use crate::reader::Dec;
use crate::records::{RecordList, WireRecord, sealed};

/// Highest number of vertex attribute and vertex buffer layout slots (Apple7: 31).
pub const MAX_VERTEX_ATTRIBUTES: usize = 31;

/// Raw `MTLTextureSwizzleChannels` of the identity mapping (`red, green, blue, alpha`).
pub const SWIZZLE_IDENTITY: [u8; 4] = [2, 3, 4, 5];

/// `CreateTextureView` payload (32 bytes):
/// `texture_id u32 | parent_id u32 | pixel_format u16 | texture_type u8 | reserved u8 |
/// swizzle [u8; 4] | level_start u32 | level_count u32 | slice_start u32 | slice_count u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateTextureView {
    /// Guest-chosen id of the view (texture namespace).
    pub texture_id: ObjectId,
    /// Texture (or texture view) the view is made of.
    pub parent_id: ObjectId,
    /// Raw `MTLPixelFormat` of the view.
    pub pixel_format: u16,
    /// Raw `MTLTextureType` of the view.
    pub texture_type: u8,
    /// Raw `MTLTextureSwizzle` per output channel (red, green, blue, alpha), relative to the
    /// parent's swizzle.
    pub swizzle: [u8; 4],
    /// First mip level of the parent.
    pub level_start: u32,
    /// Number of mip levels.
    pub level_count: u32,
    /// First array slice of the parent (cube faces count individually).
    pub slice_start: u32,
    /// Number of slices.
    pub slice_count: u32,
}

impl CreateTextureView {
    #[inline(never)] // keep `Command::decode` small (Q-PERF-3)
    pub(crate) fn decode(d: &mut Dec<'_>) -> Result<Self, ProtoError> {
        let texture_id = d.id("texture_id")?;
        let parent_id = d.id("parent_id")?;
        let pixel_format = d.u16("pixel_format")?;
        let texture_type = d.u8("texture_type")?;
        d.reserved(1)?;
        let swizzle = [
            d.u8("swizzle")?,
            d.u8("swizzle")?,
            d.u8("swizzle")?,
            d.u8("swizzle")?,
        ];
        Ok(Self {
            texture_id,
            parent_id,
            pixel_format,
            texture_type,
            swizzle,
            level_start: d.u32("level_start")?,
            level_count: d.u32("level_count")?,
            slice_start: d.u32("slice_start")?,
            slice_count: d.u32("slice_count")?,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.texture_id.get().to_le_bytes());
        out.extend_from_slice(&self.parent_id.get().to_le_bytes());
        out.extend_from_slice(&self.pixel_format.to_le_bytes());
        out.extend_from_slice(&[self.texture_type, 0]);
        out.extend_from_slice(&self.swizzle);
        for v in [
            self.level_start,
            self.level_count,
            self.slice_start,
            self.slice_count,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// `CreateSampler` payload (32 bytes):
/// `sampler_id u32 | min u8 | mag u8 | mip u8 | reserved u8 | s u8 | t u8 | r u8 | border u8 |
/// reduction u8 | compare u8 | flags u8 | reserved u8 | max_anisotropy u32 | lod_min_clamp f32 |
/// lod_max_clamp f32 | lod_bias f32`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreateSampler {
    /// Guest-chosen id of the sampler state.
    pub sampler_id: ObjectId,
    /// Raw `MTLSamplerMinMagFilter` (minification).
    pub min_filter: u8,
    /// Raw `MTLSamplerMinMagFilter` (magnification).
    pub mag_filter: u8,
    /// Raw `MTLSamplerMipFilter`.
    pub mip_filter: u8,
    /// Raw `MTLSamplerAddressMode` for s, t, r.
    pub address_modes: [u8; 3],
    /// Raw `MTLSamplerBorderColor`.
    pub border_color: u8,
    /// Raw `MTLSamplerReductionMode`.
    pub reduction_mode: u8,
    /// Raw `MTLCompareFunction`.
    pub compare_function: u8,
    /// [`CreateSampler::NORMALIZED_COORDINATES`] | [`CreateSampler::LOD_AVERAGE`] |
    /// [`CreateSampler::SUPPORT_ARGUMENT_BUFFERS`].
    pub flags: u8,
    /// `maxAnisotropy`.
    pub max_anisotropy: u32,
    /// `lodMinClamp`.
    pub lod_min_clamp: f32,
    /// `lodMaxClamp`.
    pub lod_max_clamp: f32,
    /// `lodBias`.
    pub lod_bias: f32,
}

impl CreateSampler {
    /// Flag: `normalizedCoordinates`.
    pub const NORMALIZED_COORDINATES: u8 = 1 << 0;
    /// Flag: `lodAverage`.
    pub const LOD_AVERAGE: u8 = 1 << 1;
    /// Flag: `supportArgumentBuffers`.
    pub const SUPPORT_ARGUMENT_BUFFERS: u8 = 1 << 2;
    const KNOWN_FLAGS: u8 =
        Self::NORMALIZED_COORDINATES | Self::LOD_AVERAGE | Self::SUPPORT_ARGUMENT_BUFFERS;

    #[inline(never)] // keep `Command::decode` small (Q-PERF-3)
    pub(crate) fn decode(d: &mut Dec<'_>) -> Result<Self, ProtoError> {
        let sampler_id = d.id("sampler_id")?;
        let min_filter = d.u8("min_filter")?;
        let mag_filter = d.u8("mag_filter")?;
        let mip_filter = d.u8("mip_filter")?;
        d.reserved(1)?;
        let address_modes = [
            d.u8("s_address_mode")?,
            d.u8("t_address_mode")?,
            d.u8("r_address_mode")?,
        ];
        let border_color = d.u8("border_color")?;
        let reduction_mode = d.u8("reduction_mode")?;
        let compare_function = d.u8("compare_function")?;
        let flags = d.flags("flags", Self::KNOWN_FLAGS)?;
        d.reserved(1)?;
        Ok(Self {
            sampler_id,
            min_filter,
            mag_filter,
            mip_filter,
            address_modes,
            border_color,
            reduction_mode,
            compare_function,
            flags,
            max_anisotropy: d.u32("max_anisotropy")?,
            lod_min_clamp: d.f32("lod_min_clamp")?,
            lod_max_clamp: d.f32("lod_max_clamp")?,
            lod_bias: d.f32("lod_bias")?,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.sampler_id.get().to_le_bytes());
        out.extend_from_slice(&[self.min_filter, self.mag_filter, self.mip_filter, 0]);
        out.extend_from_slice(&self.address_modes);
        out.push(self.border_color);
        debug_assert_eq!(
            self.flags & !Self::KNOWN_FLAGS,
            0,
            "undefined sampler flags"
        );
        out.extend_from_slice(&[self.reduction_mode, self.compare_function, self.flags, 0]);
        out.extend_from_slice(&self.max_anisotropy.to_le_bytes());
        for v in [self.lod_min_clamp, self.lod_max_clamp, self.lod_bias] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// One stencil face of `CreateDepthStencilState` (16 bytes):
/// `compare u8 | stencil_failure u8 | depth_failure u8 | depth_stencil_pass u8 | read_mask u32 |
/// write_mask u32 | reserved u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StencilRecord {
    /// Raw `MTLCompareFunction`.
    pub compare: u8,
    /// Raw `MTLStencilOperation` on stencil failure.
    pub stencil_failure: u8,
    /// Raw `MTLStencilOperation` on depth failure.
    pub depth_failure: u8,
    /// Raw `MTLStencilOperation` when both pass.
    pub depth_stencil_pass: u8,
    /// `readMask`.
    pub read_mask: u32,
    /// `writeMask`.
    pub write_mask: u32,
}

impl StencilRecord {
    fn decode(d: &mut Dec<'_>) -> Result<Self, ProtoError> {
        let s = Self {
            compare: d.u8("stencil_compare")?,
            stencil_failure: d.u8("stencil_failure")?,
            depth_failure: d.u8("depth_failure")?,
            depth_stencil_pass: d.u8("depth_stencil_pass")?,
            read_mask: d.u32("read_mask")?,
            write_mask: d.u32("write_mask")?,
        };
        d.reserved(4)?;
        Ok(s)
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[
            self.compare,
            self.stencil_failure,
            self.depth_failure,
            self.depth_stencil_pass,
        ]);
        out.extend_from_slice(&self.read_mask.to_le_bytes());
        out.extend_from_slice(&self.write_mask.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
    }
}

/// `CreateDepthStencilState` payload (40 bytes):
/// `dss_id u32 | depth_compare u8 | flags u8 | reserved u16 | front StencilRecord |
/// back StencilRecord`. An absent face is 16 zero bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateDepthStencilState {
    /// Guest-chosen id of the depth/stencil state.
    pub dss_id: ObjectId,
    /// Raw `MTLCompareFunction` of the depth test.
    pub depth_compare: u8,
    /// `depthWriteEnabled`.
    pub depth_write_enabled: bool,
    /// Front-face stencil (`None` = `frontFaceStencil` is nil, stencil test off).
    pub front_face_stencil: Option<StencilRecord>,
    /// Back-face stencil.
    pub back_face_stencil: Option<StencilRecord>,
}

const DSS_DEPTH_WRITE: u8 = 1 << 0;
const DSS_FRONT: u8 = 1 << 1;
const DSS_BACK: u8 = 1 << 2;

impl CreateDepthStencilState {
    #[inline(never)] // keep `Command::decode` small (Q-PERF-3)
    pub(crate) fn decode(d: &mut Dec<'_>) -> Result<Self, ProtoError> {
        let dss_id = d.id("dss_id")?;
        let depth_compare = d.u8("depth_compare")?;
        let flags = d.flags("flags", DSS_DEPTH_WRITE | DSS_FRONT | DSS_BACK)?;
        d.reserved(2)?;
        let mut face = |present: bool| {
            if present {
                StencilRecord::decode(d).map(Some)
            } else {
                d.reserved(16).map(|()| None)
            }
        };
        let front_face_stencil = face(flags & DSS_FRONT != 0)?;
        let back_face_stencil = face(flags & DSS_BACK != 0)?;
        Ok(Self {
            dss_id,
            depth_compare,
            depth_write_enabled: flags & DSS_DEPTH_WRITE != 0,
            front_face_stencil,
            back_face_stencil,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        let mut flags = 0;
        if self.depth_write_enabled {
            flags |= DSS_DEPTH_WRITE;
        }
        if self.front_face_stencil.is_some() {
            flags |= DSS_FRONT;
        }
        if self.back_face_stencil.is_some() {
            flags |= DSS_BACK;
        }
        out.extend_from_slice(&self.dss_id.get().to_le_bytes());
        out.extend_from_slice(&[self.depth_compare, flags, 0, 0]);
        for face in [self.front_face_stencil, self.back_face_stencil] {
            match face {
                Some(f) => f.encode(out),
                None => out.extend_from_slice(&[0; 16]),
            }
        }
    }
}

/// One colour attachment of a render pipeline (16 bytes, dense: entry `i` describes slot `i`):
/// `pixel_format u16 | flags u8 | write_mask u8 | src_rgb u8 | dst_rgb u8 | rgb_op u8 |
/// src_alpha u8 | dst_alpha u8 | alpha_op u8 | reserved [u8; 6]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineColorRecord {
    /// Raw `MTLPixelFormat` (0 = `Invalid`, slot unused).
    pub pixel_format: u16,
    /// `blendingEnabled`.
    pub blending_enabled: bool,
    /// Raw `MTLColorWriteMask`.
    pub write_mask: u8,
    /// Raw `MTLBlendFactor` (source RGB).
    pub source_rgb_blend_factor: u8,
    /// Raw `MTLBlendFactor` (destination RGB).
    pub destination_rgb_blend_factor: u8,
    /// Raw `MTLBlendOperation` (RGB).
    pub rgb_blend_operation: u8,
    /// Raw `MTLBlendFactor` (source alpha).
    pub source_alpha_blend_factor: u8,
    /// Raw `MTLBlendFactor` (destination alpha).
    pub destination_alpha_blend_factor: u8,
    /// Raw `MTLBlendOperation` (alpha).
    pub alpha_blend_operation: u8,
}

impl PipelineColorRecord {
    /// SDK defaults for a slot with `pixel_format` (blending off, `one`/`zero`/`add`, all
    /// channels written).
    #[must_use]
    pub const fn with_format(pixel_format: u16) -> Self {
        Self {
            pixel_format,
            blending_enabled: false,
            write_mask: 0xf,
            source_rgb_blend_factor: 1,
            destination_rgb_blend_factor: 0,
            rgb_blend_operation: 0,
            source_alpha_blend_factor: 1,
            destination_alpha_blend_factor: 0,
            alpha_blend_operation: 0,
        }
    }
}

impl sealed::Decode for PipelineColorRecord {
    fn decode(input: sealed::Input<'_, '_>) -> Result<Self, ProtoError> {
        let d = input.0;
        let pixel_format = d.u16("pixel_format")?;
        let flags = d.flags("blend_flags", 1)?;
        let r = Self {
            pixel_format,
            blending_enabled: flags & 1 != 0,
            write_mask: d.u8("write_mask")?,
            source_rgb_blend_factor: d.u8("source_rgb_blend_factor")?,
            destination_rgb_blend_factor: d.u8("destination_rgb_blend_factor")?,
            rgb_blend_operation: d.u8("rgb_blend_operation")?,
            source_alpha_blend_factor: d.u8("source_alpha_blend_factor")?,
            destination_alpha_blend_factor: d.u8("destination_alpha_blend_factor")?,
            alpha_blend_operation: d.u8("alpha_blend_operation")?,
        };
        d.reserved(6)?;
        Ok(r)
    }
}

impl WireRecord for PipelineColorRecord {
    const LEN: usize = 16;
    const INDEX_LIMIT: u8 = 0;
    const MAX_ENTRIES: usize = MAX_COLOR_ATTACHMENTS;
    const FIELD: &'static str = "color_attachments";

    fn index(&self) -> Option<u8> {
        None
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.pixel_format.to_le_bytes());
        out.extend_from_slice(&[
            u8::from(self.blending_enabled),
            self.write_mask,
            self.source_rgb_blend_factor,
            self.destination_rgb_blend_factor,
            self.rgb_blend_operation,
            self.source_alpha_blend_factor,
            self.destination_alpha_blend_factor,
            self.alpha_blend_operation,
        ]);
        out.extend_from_slice(&[0; 6]);
    }
}

/// One used vertex attribute (16 bytes):
/// `index u8 | reserved u8 | format u16 | buffer_index u32 | offset u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexAttributeRecord {
    /// Attribute slot (< [`MAX_VERTEX_ATTRIBUTES`], strictly increasing in a list).
    pub index: u8,
    /// Raw `MTLVertexFormat` (0 = `Invalid`, slot unused).
    pub format: u16,
    /// `bufferIndex`.
    pub buffer_index: u32,
    /// `offset`.
    pub offset: u64,
}

impl sealed::Decode for VertexAttributeRecord {
    fn decode(input: sealed::Input<'_, '_>) -> Result<Self, ProtoError> {
        let d = input.0;
        let index = d.u8("attribute_index")?;
        d.reserved(1)?;
        Ok(Self {
            index,
            format: d.u16("vertex_format")?,
            buffer_index: d.u32("buffer_index")?,
            offset: d.u64("offset")?,
        })
    }
}

impl WireRecord for VertexAttributeRecord {
    const LEN: usize = 16;
    // MAX_VERTEX_ATTRIBUTES = 31 fits in u8.
    const INDEX_LIMIT: u8 = MAX_VERTEX_ATTRIBUTES as u8;
    const MAX_ENTRIES: usize = MAX_VERTEX_ATTRIBUTES;
    const FIELD: &'static str = "vertex_attributes";

    fn index(&self) -> Option<u8> {
        Some(self.index)
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[self.index, 0]);
        out.extend_from_slice(&self.format.to_le_bytes());
        out.extend_from_slice(&self.buffer_index.to_le_bytes());
        out.extend_from_slice(&self.offset.to_le_bytes());
    }
}

/// One vertex buffer layout that differs from the SDK default (24 bytes):
/// `index u8 | step_function u8 | reserved [u8; 6] | stride u64 | step_rate u64`. Layouts not
/// listed keep the SDK defaults (stride 0, per vertex, step rate 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexLayoutRecord {
    /// Buffer layout slot (< [`MAX_VERTEX_ATTRIBUTES`], strictly increasing in a list).
    pub index: u8,
    /// Raw `MTLVertexStepFunction`.
    pub step_function: u8,
    /// `stride` (`u64::MAX` = `MTLBufferLayoutStrideDynamic`).
    pub stride: u64,
    /// `stepRate`.
    pub step_rate: u64,
}

impl sealed::Decode for VertexLayoutRecord {
    fn decode(input: sealed::Input<'_, '_>) -> Result<Self, ProtoError> {
        let d = input.0;
        let index = d.u8("layout_index")?;
        let step_function = d.u8("step_function")?;
        d.reserved(6)?;
        Ok(Self {
            index,
            step_function,
            stride: d.u64("stride")?,
            step_rate: d.u64("step_rate")?,
        })
    }
}

impl WireRecord for VertexLayoutRecord {
    const LEN: usize = 24;
    const INDEX_LIMIT: u8 = MAX_VERTEX_ATTRIBUTES as u8;
    const MAX_ENTRIES: usize = MAX_VERTEX_ATTRIBUTES;
    const FIELD: &'static str = "vertex_layouts";

    fn index(&self) -> Option<u8> {
        Some(self.index)
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[self.index, self.step_function, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&self.stride.to_le_bytes());
        out.extend_from_slice(&self.step_rate.to_le_bytes());
    }
}

/// `CreateRenderPipelineState` payload: 32-byte header, then `color_count` ×
/// [`PipelineColorRecord`], `attribute_count` × [`VertexAttributeRecord`], `layout_count` ×
/// [`VertexLayoutRecord`]:
///
/// ```text
/// pipeline_id u32 | vertex_function u32 | fragment_function u32 |
/// depth_pixel_format u16 | stencil_pixel_format u16 |
/// raster_sample_count u8 | input_primitive_topology u8 | flags u8 | color_count u8 |
/// max_vertex_amplification_count u32 |
/// attribute_count u8 | layout_count u8 | reserved u16 | reserved u32
/// ```
///
/// **Shader functions (placeholder).** `vertex_function` / `fragment_function` are reserved
/// for guest ids of function objects (`MTLFunction`). Protocol v1 has no library/function
/// opcodes yet; until WP-SC-09/12 add them (planned: `CreateLibrary`/`CreateFunction` in the
/// `0x015x` range, ids resolved by the host), only 0 (= no function) is accepted by the host;
/// any other value is answered with `Unimplemented`. The layout does not change when the
/// functions arrive, so pipelines encoded today stay decodable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreateRenderPipelineState<'a> {
    /// Guest-chosen id of the pipeline state.
    pub pipeline_id: ObjectId,
    /// Vertex function (placeholder, see type docs).
    pub vertex_function: Option<ObjectId>,
    /// Fragment function (placeholder, see type docs).
    pub fragment_function: Option<ObjectId>,
    /// Raw `MTLPixelFormat` of the depth attachment (0 = none).
    pub depth_pixel_format: u16,
    /// Raw `MTLPixelFormat` of the stencil attachment (0 = none).
    pub stencil_pixel_format: u16,
    /// `rasterSampleCount`.
    pub raster_sample_count: u8,
    /// Raw `MTLPrimitiveTopologyClass`.
    pub input_primitive_topology: u8,
    /// [`CreateRenderPipelineState::ALPHA_TO_COVERAGE`] | `ALPHA_TO_ONE` |
    /// `RASTERIZATION_ENABLED` | `VERTEX_DESCRIPTOR`.
    pub flags: u8,
    /// `maxVertexAmplificationCount`.
    pub max_vertex_amplification_count: u32,
    /// Colour attachments 0..n (at most [`MAX_COLOR_ATTACHMENTS`]); later slots are unused.
    pub color_attachments: RecordList<'a, PipelineColorRecord>,
    /// Used vertex attributes (only with [`CreateRenderPipelineState::VERTEX_DESCRIPTOR`]).
    pub vertex_attributes: RecordList<'a, VertexAttributeRecord>,
    /// Non-default vertex buffer layouts (only with `VERTEX_DESCRIPTOR`).
    pub vertex_layouts: RecordList<'a, VertexLayoutRecord>,
}

impl CreateRenderPipelineState<'_> {
    /// Flag: `alphaToCoverageEnabled`.
    pub const ALPHA_TO_COVERAGE: u8 = 1 << 0;
    /// Flag: `alphaToOneEnabled`.
    pub const ALPHA_TO_ONE: u8 = 1 << 1;
    /// Flag: `rasterizationEnabled`.
    pub const RASTERIZATION_ENABLED: u8 = 1 << 2;
    /// Flag: a vertex descriptor is set (`vertexDescriptor` non-nil).
    pub const VERTEX_DESCRIPTOR: u8 = 1 << 3;
    const KNOWN_FLAGS: u8 = Self::ALPHA_TO_COVERAGE
        | Self::ALPHA_TO_ONE
        | Self::RASTERIZATION_ENABLED
        | Self::VERTEX_DESCRIPTOR;
}

impl<'a> CreateRenderPipelineState<'a> {
    #[inline(never)] // keep `Command::decode` small (Q-PERF-3)
    pub(crate) fn decode(d: &mut Dec<'a>) -> Result<Self, ProtoError> {
        let pipeline_id = d.id("pipeline_id")?;
        let vertex_function = d.opt_id("vertex_function")?;
        let fragment_function = d.opt_id("fragment_function")?;
        let depth_pixel_format = d.u16("depth_pixel_format")?;
        let stencil_pixel_format = d.u16("stencil_pixel_format")?;
        let raster_sample_count = d.u8("raster_sample_count")?;
        let input_primitive_topology = d.u8("input_primitive_topology")?;
        let flags = d.flags("flags", Self::KNOWN_FLAGS)?;
        let color_count = usize::from(d.u8("color_count")?);
        let max_vertex_amplification_count = d.u32("max_vertex_amplification_count")?;
        let attribute_count = usize::from(d.u8("attribute_count")?);
        let layout_count = usize::from(d.u8("layout_count")?);
        d.reserved(6)?;
        if color_count > MAX_COLOR_ATTACHMENTS {
            return Err(d.err("color_count"));
        }
        if attribute_count > MAX_VERTEX_ATTRIBUTES || layout_count > MAX_VERTEX_ATTRIBUTES {
            return Err(d.err("attribute_count"));
        }
        if flags & Self::VERTEX_DESCRIPTOR == 0 && (attribute_count | layout_count) != 0 {
            return Err(d.err("vertex_descriptor"));
        }
        // Counts are bounded above, so the products cannot overflow.
        let lens = [
            color_count * PipelineColorRecord::LEN,
            attribute_count * VertexAttributeRecord::LEN,
            layout_count * VertexLayoutRecord::LEN,
        ];
        let total: usize = lens.iter().sum();
        let tail_at = d.payload_pos();
        let tail = d.cur.rest();
        if tail.len() != total {
            return Err(d.err("pipeline_tail"));
        }
        let (colors, rest) = tail
            .split_at_checked(lens[0])
            .ok_or_else(|| d.err("pipeline_tail"))?;
        let (attributes, layouts) = rest
            .split_at_checked(lens[1])
            .ok_or_else(|| d.err("pipeline_tail"))?;
        Ok(Self {
            pipeline_id,
            vertex_function,
            fragment_function,
            depth_pixel_format,
            stencil_pixel_format,
            raster_sample_count,
            input_primitive_topology,
            flags,
            max_vertex_amplification_count,
            color_attachments: RecordList::parse_at(colors, d.offset, tail_at)?,
            vertex_attributes: RecordList::parse_at(attributes, d.offset, tail_at + lens[0])?,
            vertex_layouts: RecordList::parse_at(layouts, d.offset, tail_at + lens[0] + lens[1])?,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        // Lists hold at most `MAX_ENTRIES` (≤ 31) entries by construction.
        let count = |n: usize| {
            debug_assert!(n <= MAX_VERTEX_ATTRIBUTES);
            u8::try_from(n).unwrap_or(u8::MAX)
        };
        debug_assert_eq!(
            self.flags & !Self::KNOWN_FLAGS,
            0,
            "undefined pipeline flags"
        );
        // Vertex attribute/layout lists imply a vertex descriptor; deriving the flag keeps
        // the reference encoder from emitting a stream its own decoder rejects.
        let flags = if self.vertex_attributes.is_empty() && self.vertex_layouts.is_empty() {
            self.flags
        } else {
            self.flags | Self::VERTEX_DESCRIPTOR
        };
        out.extend_from_slice(&self.pipeline_id.get().to_le_bytes());
        for f in [self.vertex_function, self.fragment_function] {
            out.extend_from_slice(&f.map_or(0, ObjectId::get).to_le_bytes());
        }
        out.extend_from_slice(&self.depth_pixel_format.to_le_bytes());
        out.extend_from_slice(&self.stencil_pixel_format.to_le_bytes());
        out.extend_from_slice(&[
            self.raster_sample_count,
            self.input_primitive_topology,
            flags,
            count(self.color_attachments.len()),
        ]);
        out.extend_from_slice(&self.max_vertex_amplification_count.to_le_bytes());
        out.extend_from_slice(&[
            count(self.vertex_attributes.len()),
            count(self.vertex_layouts.len()),
            0,
            0,
            0,
            0,
            0,
            0,
        ]);
        out.extend_from_slice(self.color_attachments.as_bytes());
        out.extend_from_slice(self.vertex_attributes.as_bytes());
        out.extend_from_slice(self.vertex_layouts.as_bytes());
    }
}
