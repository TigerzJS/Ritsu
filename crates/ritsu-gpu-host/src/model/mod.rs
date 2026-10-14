//! Protocol-neutral Metal object model (ADR-0005).
//!
//! Both frontends (P: Apple paravirtual GPU, G: Ritsu guest driver) translate their wire
//! formats into these types; backends only ever see these types. Raw numeric values of the
//! enums are the public Metal SDK values (`MTLStorageMode`, `MTLPixelFormat`, …) so that a
//! frontend can pass guest values through unchanged.
//!
//! **Source of the raw values:** the public Metal SDK headers as published in Apple's developer
//! documentation (cross-checked against the `objc2-metal` bindings, which are generated from
//! those headers). Each table names its header. They are public API constants, not
//! reverse-engineered (ADR-0008). Feature and limit values come from Apple's *Metal Feature Set
//! Tables* (edition 2026-05-21), column Apple7 (the floor of every Metal 3/4 device).
//!
//! Every descriptor coming from the guest is validated (`validate`) before a backend sees it.
//! The rules are deliberately conservative: rejecting a valid descriptor is a loud, fixable
//! bug; accepting an invalid one could be a silent misrender or a host crash. Descriptors
//! describe what *Metal* requires; whether a concrete GPU can do it is decided separately in
//! [`crate::caps`] (`format_support`, `texture_support`, `vertex_input_support`).
//!
//! Module map:
//! * [`format`](mod@format) – `MTLPixelFormat` table (blocks, numeric types, capabilities, view classes).
//! * [`view`] – texture swizzle and texture views.
//! * [`sampler`] – `MTLSamplerDescriptor`.
//! * [`depth_stencil`] – `MTLDepthStencilDescriptor`, `MTLCompareFunction`.
//! * [`render_pass`] – `MTLRenderPassDescriptor` (attachments, resolve, clear values).
//! * [`vertex`] – `MTLVertexDescriptor`, `MTLVertexFormat`.
//! * [`pipeline`] – `MTLRenderPipelineDescriptor` without shader contents.

use core::fmt;

pub mod depth_stencil;
pub mod format;
pub mod pipeline;
pub mod render_pass;
pub mod sampler;
pub mod vertex;
pub mod view;

pub use depth_stencil::{CompareFunction, DepthStencilDesc, StencilDesc, StencilOperation};
pub use format::{
    Aspects, BlockInfo, Compression, FormatCaps, NumericType, PixelFormat, TextureBufferAccess,
    optional_pixel_format,
};
pub use pipeline::{
    BlendFactor, BlendOperation, ColorWriteMask, PrimitiveTopologyClass,
    RenderPipelineColorAttachmentDesc, RenderPipelineDesc,
};
pub use render_pass::{
    AttachmentDesc, AttachmentSlot, ClearColor, ClearValue, ColorAttachmentDesc,
    DepthAttachmentDesc, MAX_COLOR_ATTACHMENTS, MultisampleDepthResolveFilter,
    MultisampleStencilResolveFilter, RenderPassDesc, RenderTargetLayout, ResolveTarget,
    StencilAttachmentDesc, StoreActionOptions,
};
pub use sampler::{
    SamplerAddressMode, SamplerBorderColor, SamplerDesc, SamplerMinMagFilter, SamplerMipFilter,
    SamplerReductionMode,
};
pub use vertex::{
    VertexAttributeDesc, VertexBufferLayoutDesc, VertexDescriptor, VertexFormat,
    VertexStepFunction, VertexStride,
};
pub use view::{SubresourceRange, TextureSwizzle, TextureSwizzleChannels, TextureViewDesc};

/// Errors for guest-invalid object descriptions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ModelError {
    /// Unknown raw `MTLStorageMode`.
    #[error("unknown storage mode {0}")]
    UnknownStorageMode(u32),
    /// Unknown raw `MTLCPUCacheMode`.
    #[error("unknown cpu cache mode {0}")]
    UnknownCpuCacheMode(u32),
    /// Unknown raw `MTLHazardTrackingMode`.
    #[error("unknown hazard tracking mode {0}")]
    UnknownHazardTrackingMode(u32),
    /// Unknown raw `MTLPixelFormat` (including `MTLPixelFormatInvalid` where a format is
    /// required).
    #[error("unknown pixel format {0}")]
    UnknownPixelFormat(u32),
    /// Unknown raw `MTLTextureType`.
    #[error("unknown texture type {0}")]
    UnknownTextureType(u32),
    /// `MTLTextureUsage` contains undefined bits.
    #[error("unknown texture usage bits {0:#x}")]
    UnknownTextureUsage(u32),
    /// Unknown raw `MTLLoadAction`.
    #[error("unknown load action {0}")]
    UnknownLoadAction(u32),
    /// Unknown raw `MTLStoreAction`.
    #[error("unknown store action {0}")]
    UnknownStoreAction(u32),
    /// `MTLStoreActionOptions` contains undefined bits.
    #[error("unknown store action options {0:#x}")]
    UnknownStoreActionOptions(u32),
    /// Unknown raw `MTLMultisampleDepthResolveFilter`.
    #[error("unknown depth resolve filter {0}")]
    UnknownDepthResolveFilter(u32),
    /// Unknown raw `MTLMultisampleStencilResolveFilter`.
    #[error("unknown stencil resolve filter {0}")]
    UnknownStencilResolveFilter(u32),
    /// Unknown raw `MTLPrimitiveType`.
    #[error("unknown primitive type {0}")]
    UnknownPrimitiveType(u32),
    /// Unknown raw `MTLTextureSwizzle`.
    #[error("unknown texture swizzle {0}")]
    UnknownTextureSwizzle(u32),
    /// Unknown raw `MTLSamplerMinMagFilter`.
    #[error("unknown sampler min/mag filter {0}")]
    UnknownSamplerFilter(u32),
    /// Unknown raw `MTLSamplerMipFilter`.
    #[error("unknown sampler mip filter {0}")]
    UnknownSamplerMipFilter(u32),
    /// Unknown raw `MTLSamplerAddressMode`.
    #[error("unknown sampler address mode {0}")]
    UnknownSamplerAddressMode(u32),
    /// Unknown raw `MTLSamplerBorderColor`.
    #[error("unknown sampler border color {0}")]
    UnknownSamplerBorderColor(u32),
    /// Unknown raw `MTLSamplerReductionMode`.
    #[error("unknown sampler reduction mode {0}")]
    UnknownSamplerReductionMode(u32),
    /// Unknown raw `MTLCompareFunction`.
    #[error("unknown compare function {0}")]
    UnknownCompareFunction(u32),
    /// Unknown raw `MTLStencilOperation`.
    #[error("unknown stencil operation {0}")]
    UnknownStencilOperation(u32),
    /// Unknown raw `MTLVertexFormat` (including `MTLVertexFormatInvalid` where a format is
    /// required).
    #[error("unknown vertex format {0}")]
    UnknownVertexFormat(u32),
    /// Unknown raw `MTLVertexStepFunction`.
    #[error("unknown vertex step function {0}")]
    UnknownVertexStepFunction(u32),
    /// Unknown raw `MTLBlendFactor`.
    #[error("unknown blend factor {0}")]
    UnknownBlendFactor(u32),
    /// Unknown raw `MTLBlendOperation`.
    #[error("unknown blend operation {0}")]
    UnknownBlendOperation(u32),
    /// `MTLColorWriteMask` contains undefined bits.
    #[error("unknown color write mask bits {0:#x}")]
    UnknownColorWriteMask(u32),
    /// Unknown raw `MTLPrimitiveTopologyClass`.
    #[error("unknown primitive topology class {0}")]
    UnknownPrimitiveTopologyClass(u32),
    /// Buffer of length 0.
    #[error("buffer length is zero")]
    ZeroLength,
    /// Buffer longer than the reported `maxBufferLength`.
    #[error("buffer length {length} exceeds maximum {max}")]
    BufferTooLarge {
        /// Requested length.
        length: u64,
        /// Reported maximum.
        max: u64,
    },
    /// Memoryless storage is only valid for render-target textures.
    #[error("memoryless storage is not valid for buffers")]
    MemorylessBuffer,
    /// A texture extent is zero.
    #[error("texture extent has a zero dimension")]
    ZeroExtent,
    /// A dimension must be 1 for this texture type.
    #[error("{field} must be 1 for texture type {texture_type:?}")]
    DimensionMustBeOne {
        /// Texture type.
        texture_type: TextureType,
        /// Field name.
        field: &'static str,
    },
    /// A dimension exceeds the device limit.
    #[error("{field} = {value} exceeds limit {max}")]
    DimensionTooLarge {
        /// Field name.
        field: &'static str,
        /// Requested value.
        value: u32,
        /// Limit.
        max: u32,
    },
    /// Cube textures must be square.
    #[error("cube texture is not square")]
    CubeNotSquare,
    /// Mip level count of zero.
    #[error("mip level count is zero")]
    ZeroMipLevels,
    /// More mip levels than the extent allows.
    #[error("mip level count {requested} exceeds maximum {max}")]
    TooManyMipLevels {
        /// Requested count.
        requested: u32,
        /// Maximum for the extent.
        max: u32,
    },
    /// Invalid sample count for the texture type or above the device limit.
    #[error("invalid sample count {0}")]
    InvalidSampleCount(u32),
    /// Multisample textures must have exactly one mip level.
    #[error("multisample textures must have one mip level")]
    MultisampleMipmapped,
    /// Pixel format cannot be used with this texture type.
    #[error("pixel format {format:?} not allowed for texture type {texture_type:?}")]
    FormatNotAllowedForType {
        /// Format.
        format: PixelFormat,
        /// Texture type.
        texture_type: TextureType,
    },
    /// The pixel format exists but cannot be used for this purpose at all on Apple-family
    /// GPUs (e.g. `Depth24Unorm_Stencil8`, or a view-only format in a texture descriptor).
    #[error("pixel format {format:?} not usable here: {reason}")]
    FormatNotUsable {
        /// Format.
        format: PixelFormat,
        /// Why.
        reason: &'static str,
    },
    /// Usage flags are incompatible with the pixel format.
    #[error("usage {usage:?} not allowed for pixel format {format:?}")]
    UsageNotAllowedForFormat {
        /// Format.
        format: PixelFormat,
        /// Usage.
        usage: TextureUsage,
    },
    /// Memoryless textures must be render targets only.
    #[error("memoryless texture must have usage exactly renderTarget")]
    MemorylessUsage,
    /// Managed storage is not valid for this resource (e.g. depth/stencil textures).
    #[error("managed storage not allowed here")]
    ManagedNotAllowed,
    /// Size arithmetic overflowed.
    #[error("resource size overflows")]
    SizeOverflow,
    /// A subresource range (mips, slices, depth planes) lies outside the resource.
    #[error("{field} range {start}+{count} exceeds {max}")]
    RangeOutOfBounds {
        /// Field name.
        field: &'static str,
        /// First element.
        start: u32,
        /// Number of elements.
        count: u32,
        /// Number of elements available.
        max: u32,
    },
    /// A texture view's pixel format is not compatible with the texture's.
    #[error("cannot view pixel format {from:?} as {to:?}")]
    IncompatibleViewFormat {
        /// Texture format.
        from: PixelFormat,
        /// View format.
        to: PixelFormat,
    },
    /// A texture view's type is not a permitted cast of the texture's type.
    #[error("cannot view texture type {from:?} as {to:?}")]
    IncompatibleViewType {
        /// Texture type.
        from: TextureType,
        /// View type.
        to: TextureType,
    },
    /// A field has a value Metal does not allow.
    #[error("invalid {field}: {reason}")]
    InvalidValue {
        /// Field name.
        field: &'static str,
        /// Why.
        reason: &'static str,
    },
    /// A render-pass attachment is invalid.
    #[error("{slot}: {reason}")]
    InvalidAttachment {
        /// Attachment.
        slot: AttachmentSlot,
        /// Why.
        reason: &'static str,
    },
    /// Valid Metal that Ritsu does not support (yet) or cannot map faithfully. Reported to
    /// the guest as "not implemented", never approximated (CLAUDE.md §3.5).
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    /// The device lacks capabilities Metal guarantees for this pixel format and Ritsu has no
    /// enabled emulation for them (see [`crate::caps::format_support`]).
    #[error("pixel format {format:?} lacks {missing:?} on this GPU")]
    FormatUnsupportedOnDevice {
        /// Format.
        format: PixelFormat,
        /// Capabilities that are required but neither native nor emulated.
        missing: FormatCaps,
    },
}

impl ModelError {
    /// Whether the error means "valid Metal, not supported by Ritsu/this GPU" rather than
    /// "invalid guest input". Frontends report the former as `Unimplemented`.
    #[must_use]
    pub const fn is_unsupported(&self) -> bool {
        matches!(
            self,
            Self::Unsupported(_) | Self::FormatUnsupportedOnDevice { .. }
        )
    }
}

impl From<ModelError> for ritsu_core::Error {
    fn from(e: ModelError) -> Self {
        match e {
            ModelError::Unsupported(what) => ritsu_core::Error::Unimplemented(what),
            ModelError::FormatUnsupportedOnDevice { .. } => {
                ritsu_core::Error::Unimplemented("pixel format capability missing on this GPU")
            }
            other => ritsu_core::Error::Guest(format!("gpu object model: {other}")),
        }
    }
}

/// Generates a closed enum over raw Metal SDK values with `from_raw`/`raw`.
macro_rules! raw_enum {
    (
        $(#[$m:meta])* $name:ident, $err:ident {
            $( $(#[$vm:meta])* $var:ident = $val:literal, )*
        }
    ) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum $name {
            $( $(#[$vm])* $var, )*
        }

        impl $name {
            /// All values, in SDK order.
            pub const ALL: &'static [$name] = &[$($name::$var,)*];

            /// Converts the raw Metal SDK value.
            ///
            /// # Errors
            ///
            /// Unknown values are reported, never mapped to a default.
            pub const fn from_raw(raw: u32) -> Result<Self, $crate::model::ModelError> {
                match raw {
                    $( $val => Ok(Self::$var), )*
                    _ => Err($crate::model::ModelError::$err(raw)),
                }
            }

            /// Raw Metal SDK value.
            #[must_use]
            pub const fn raw(self) -> u32 {
                match self {
                    $( Self::$var => $val, )*
                }
            }
        }
    };
}
pub(crate) use raw_enum;

raw_enum! {
    /// `MTLStorageMode` (`MTLResource.h`): where a resource lives and who may access it.
    StorageMode, UnknownStorageMode {
        /// CPU and GPU share the memory (coherent at command-buffer boundaries).
        Shared = 0,
        /// Separate CPU/GPU copies, synchronized via `didModifyRange` / blit synchronize.
        Managed = 1,
        /// GPU only.
        Private = 2,
        /// Tile memory only, contents never stored (render-target textures).
        Memoryless = 3,
    }
}

raw_enum! {
    /// `MTLCPUCacheMode` (`MTLResource.h`).
    CpuCacheMode, UnknownCpuCacheMode {
        /// Default caching.
        DefaultCache = 0,
        /// Write-combined CPU mapping (CPU writes only).
        WriteCombined = 1,
    }
}

raw_enum! {
    /// `MTLHazardTrackingMode` (`MTLResource.h`).
    HazardTrackingMode, UnknownHazardTrackingMode {
        /// Default for the resource kind (tracked, except heap sub-allocations).
        Default = 0,
        /// The app synchronizes manually (fences/events/barriers).
        Untracked = 1,
        /// Metal tracks hazards automatically.
        Tracked = 2,
    }
}

raw_enum! {
    /// `MTLTextureType` (`MTLTexture.h`).
    TextureType, UnknownTextureType {
        /// 1D.
        Type1D = 0,
        /// 1D array.
        Type1DArray = 1,
        /// 2D.
        Type2D = 2,
        /// 2D array.
        Type2DArray = 3,
        /// 2D multisample.
        Type2DMultisample = 4,
        /// Cube.
        TypeCube = 5,
        /// Cube array.
        TypeCubeArray = 6,
        /// 3D.
        Type3D = 7,
        /// 2D multisample array.
        Type2DMultisampleArray = 8,
        /// Texture view of a buffer.
        TypeTextureBuffer = 9,
    }
}

impl TextureType {
    /// Whether the type has several array elements (slices).
    #[must_use]
    pub const fn is_array(self) -> bool {
        matches!(
            self,
            Self::Type1DArray
                | Self::Type2DArray
                | Self::Type2DMultisampleArray
                | Self::TypeCubeArray
        )
    }

    /// Whether the type is a multisample type.
    #[must_use]
    pub const fn is_multisample(self) -> bool {
        matches!(self, Self::Type2DMultisample | Self::Type2DMultisampleArray)
    }

    /// Whether the type is a cube or cube array.
    #[must_use]
    pub const fn is_cube(self) -> bool {
        matches!(self, Self::TypeCube | Self::TypeCubeArray)
    }
}

raw_enum! {
    /// `MTLLoadAction` (`MTLRenderPass.h`).
    LoadAction, UnknownLoadAction {
        /// Contents undefined at pass start.
        DontCare = 0,
        /// Preserve contents.
        Load = 1,
        /// Clear to the clear value.
        Clear = 2,
    }
}

raw_enum! {
    /// `MTLStoreAction` (`MTLRenderPass.h`).
    StoreAction, UnknownStoreAction {
        /// Contents may be discarded.
        DontCare = 0,
        /// Store contents.
        Store = 1,
        /// Resolve into the resolve texture, discard the multisample contents.
        MultisampleResolve = 2,
        /// Store and resolve.
        StoreAndMultisampleResolve = 3,
        /// Deferred: must be set before the encoder ends (`setColorStoreAction:` etc.).
        Unknown = 4,
        /// Store depth with custom sample positions applied.
        CustomSampleDepthStore = 5,
    }
}

impl StoreAction {
    /// Whether the action resolves into a resolve texture.
    #[must_use]
    pub const fn resolves(self) -> bool {
        matches!(
            self,
            Self::MultisampleResolve | Self::StoreAndMultisampleResolve
        )
    }

    /// Whether the attachment contents are kept after the pass.
    #[must_use]
    pub const fn stores(self) -> bool {
        matches!(
            self,
            Self::Store | Self::StoreAndMultisampleResolve | Self::CustomSampleDepthStore
        )
    }
}

raw_enum! {
    /// `MTLPrimitiveType` (`MTLRenderCommandEncoder.h`).
    PrimitiveType, UnknownPrimitiveType {
        /// Points.
        Point = 0,
        /// Line list.
        Line = 1,
        /// Line strip.
        LineStrip = 2,
        /// Triangle list.
        Triangle = 3,
        /// Triangle strip.
        TriangleStrip = 4,
    }
}

impl PrimitiveType {
    /// Topology class of the primitive type (for pipeline compatibility).
    #[must_use]
    pub const fn topology_class(self) -> PrimitiveTopologyClass {
        match self {
            Self::Point => PrimitiveTopologyClass::Point,
            Self::Line | Self::LineStrip => PrimitiveTopologyClass::Line,
            Self::Triangle | Self::TriangleStrip => PrimitiveTopologyClass::Triangle,
        }
    }
}

/// `MTLTextureUsage` bit set (`MTLTexture.h`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextureUsage(u32);

impl TextureUsage {
    /// Sampled/read in shaders.
    pub const SHADER_READ: Self = Self(0x1);
    /// Written in shaders.
    pub const SHADER_WRITE: Self = Self(0x2);
    /// Used as render-pass attachment.
    pub const RENDER_TARGET: Self = Self(0x4);
    /// Views with other pixel formats may be created.
    pub const PIXEL_FORMAT_VIEW: Self = Self(0x10);
    /// Atomic operations in shaders.
    pub const SHADER_ATOMIC: Self = Self(0x20);
    const ALL_BITS: u32 = 0x1 | 0x2 | 0x4 | 0x10 | 0x20;

    /// Validates raw usage bits.
    ///
    /// # Errors
    ///
    /// [`ModelError::UnknownTextureUsage`] for undefined bits.
    pub const fn from_raw(raw: u32) -> Result<Self, ModelError> {
        if raw & !Self::ALL_BITS != 0 {
            return Err(ModelError::UnknownTextureUsage(raw));
        }
        Ok(Self(raw))
    }

    /// Raw bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether all bits of `other` are set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any bit of `other` is set.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Union.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl fmt::Debug for TextureUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TextureUsage({:#x})", self.0)
    }
}

/// Device limits that descriptor validation depends on (the values *reported* to the
/// guest, i.e. the minimum of the advertised Metal family and the Vulkan device).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    /// `maxBufferLength`.
    pub max_buffer_length: u64,
    /// Maximum 1D texture width.
    pub max_texture_dimension_1d: u32,
    /// Maximum 2D/cube texture width/height.
    pub max_texture_dimension_2d: u32,
    /// Maximum 3D texture extent.
    pub max_texture_dimension_3d: u32,
    /// Maximum array length.
    pub max_texture_array_layers: u32,
    /// Maximum total threads per threadgroup.
    pub max_threads_per_threadgroup: u32,
    /// Maximum colour attachments per render pass.
    pub max_color_attachments: u32,
    /// Maximum samples per pixel in render passes and multisample textures.
    pub max_sample_count: u32,
    /// Maximum vertex attributes per vertex descriptor.
    pub max_vertex_attributes: u32,
    /// Maximum vertex buffer layouts (= entries of the buffer argument table).
    pub max_vertex_buffers: u32,
    /// Maximum width of a texture buffer in pixels (Metal: "maximum texture buffer width";
    /// Vulkan: `maxTexelBufferElements`). Metal additionally bounds it by
    /// `maxBufferLength / bytes per pixel`.
    pub max_texture_buffer_width: u32,
}

impl ResourceLimits {
    /// Limits of the Apple7 (M1) Metal family, the ceiling for anything reported to the
    /// guest. Values: Metal Feature Set Tables (2026-05-21), "GPU implementation limits by
    /// family", column Apple7 (`maxBufferLength` is a device property, not in the tables:
    /// 4 GiB \[Annahme\]).
    pub const APPLE7: Self = Self {
        max_buffer_length: 1 << 32,
        max_texture_dimension_1d: 16384,
        max_texture_dimension_2d: 16384,
        max_texture_dimension_3d: 2048,
        max_texture_array_layers: 2048,
        max_threads_per_threadgroup: 1024,
        max_color_attachments: 8,
        max_sample_count: 4,
        max_vertex_attributes: 31,
        max_vertex_buffers: 31,
        max_texture_buffer_width: 256 << 20,
    };

    /// Component-wise minimum (what both sides support).
    #[must_use]
    pub fn min(&self, other: &Self) -> Self {
        Self {
            max_buffer_length: self.max_buffer_length.min(other.max_buffer_length),
            max_texture_dimension_1d: self
                .max_texture_dimension_1d
                .min(other.max_texture_dimension_1d),
            max_texture_dimension_2d: self
                .max_texture_dimension_2d
                .min(other.max_texture_dimension_2d),
            max_texture_dimension_3d: self
                .max_texture_dimension_3d
                .min(other.max_texture_dimension_3d),
            max_texture_array_layers: self
                .max_texture_array_layers
                .min(other.max_texture_array_layers),
            max_threads_per_threadgroup: self
                .max_threads_per_threadgroup
                .min(other.max_threads_per_threadgroup),
            max_color_attachments: self.max_color_attachments.min(other.max_color_attachments),
            max_sample_count: self.max_sample_count.min(other.max_sample_count),
            max_vertex_attributes: self.max_vertex_attributes.min(other.max_vertex_attributes),
            max_vertex_buffers: self.max_vertex_buffers.min(other.max_vertex_buffers),
            max_texture_buffer_width: self
                .max_texture_buffer_width
                .min(other.max_texture_buffer_width),
        }
    }

    /// Whether `count` is a valid sample count (1, 2, 4 or 8) within the limit.
    #[must_use]
    pub const fn sample_count_ok(&self, count: u32) -> bool {
        matches!(count, 1 | 2 | 4 | 8) && count <= self.max_sample_count
    }
}

/// Guest description of a buffer (`newBufferWithLength:options:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferDesc {
    /// Length in bytes.
    pub length: u64,
    /// Storage mode.
    pub storage_mode: StorageMode,
    /// CPU cache mode.
    pub cpu_cache_mode: CpuCacheMode,
    /// Hazard tracking mode.
    pub hazard_tracking: HazardTrackingMode,
}

impl BufferDesc {
    /// Builds a descriptor from raw wire values (frontend helper).
    ///
    /// # Errors
    ///
    /// Unknown enum values.
    pub fn from_raw(
        length: u64,
        storage_mode: u32,
        cpu_cache_mode: u32,
        hazard_tracking: u32,
    ) -> Result<Self, ModelError> {
        Ok(Self {
            length,
            storage_mode: StorageMode::from_raw(storage_mode)?,
            cpu_cache_mode: CpuCacheMode::from_raw(cpu_cache_mode)?,
            hazard_tracking: HazardTrackingMode::from_raw(hazard_tracking)?,
        })
    }

    /// Validates the descriptor against device limits.
    ///
    /// # Errors
    ///
    /// Zero length, length above `max_buffer_length`, memoryless storage.
    pub fn validate(&self, limits: &ResourceLimits) -> Result<(), ModelError> {
        if self.length == 0 {
            return Err(ModelError::ZeroLength);
        }
        if self.length > limits.max_buffer_length {
            return Err(ModelError::BufferTooLarge {
                length: self.length,
                max: limits.max_buffer_length,
            });
        }
        if self.storage_mode == StorageMode::Memoryless {
            return Err(ModelError::MemorylessBuffer);
        }
        Ok(())
    }
}

/// Guest description of a texture (`MTLTextureDescriptor`; sparse and lossy compression are
/// not modelled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDesc {
    /// Texture type.
    pub texture_type: TextureType,
    /// Pixel format.
    pub pixel_format: PixelFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Depth in pixels.
    pub depth: u32,
    /// Array length (number of cubes for cube arrays).
    pub array_length: u32,
    /// Mip level count.
    pub mip_level_count: u32,
    /// Samples per pixel.
    pub sample_count: u32,
    /// Storage mode.
    pub storage_mode: StorageMode,
    /// Usage flags.
    pub usage: TextureUsage,
    /// Swizzle applied when shaders read or sample (`MTLTextureDescriptor.swizzle`).
    pub swizzle: TextureSwizzleChannels,
}

/// Number of mip levels of a full chain for the largest dimension `max_dim` (≥ 1).
const fn full_mip_chain(max_dim: u32) -> u32 {
    32 - max_dim.leading_zeros()
}

/// Extent of mip `level` of a dimension of `size` pixels (never below 1).
#[must_use]
pub const fn mip_extent(size: u32, level: u32) -> u32 {
    if level >= 32 {
        1
    } else {
        let v = size >> level;
        if v == 0 { 1 } else { v }
    }
}

impl TextureDesc {
    /// A 2D texture with one mip level, one sample, private storage, identity swizzle.
    #[must_use]
    pub const fn new_2d(format: PixelFormat, width: u32, height: u32, usage: TextureUsage) -> Self {
        Self {
            texture_type: TextureType::Type2D,
            pixel_format: format,
            width,
            height,
            depth: 1,
            array_length: 1,
            mip_level_count: 1,
            sample_count: 1,
            storage_mode: StorageMode::Private,
            usage,
            swizzle: TextureSwizzleChannels::IDENTITY,
        }
    }

    /// Number of 2D layers (array length × 6 for cubes).
    ///
    /// # Errors
    ///
    /// [`ModelError::SizeOverflow`].
    pub fn layer_count(&self) -> Result<u32, ModelError> {
        if self.texture_type.is_cube() {
            self.array_length
                .checked_mul(6)
                .ok_or(ModelError::SizeOverflow)
        } else {
            Ok(self.array_length)
        }
    }

    /// Width, height and depth of mip `level`.
    #[must_use]
    pub const fn level_extent(&self, level: u32) -> (u32, u32, u32) {
        let depth = if matches!(self.texture_type, TextureType::Type3D) {
            mip_extent(self.depth, level)
        } else {
            1
        };
        (
            mip_extent(self.width, level),
            mip_extent(self.height, level),
            depth,
        )
    }

    /// Validates the descriptor against what Metal allows (Apple7 floor) and device limits.
    ///
    /// # Errors
    ///
    /// Any [`ModelError`] describing why the guest descriptor is invalid.
    pub fn validate(&self, limits: &ResourceLimits) -> Result<(), ModelError> {
        use TextureType as T;
        let t = self.texture_type;
        let one = |v: u32, field| {
            if v == 1 {
                Ok(())
            } else {
                Err(ModelError::DimensionMustBeOne {
                    texture_type: t,
                    field,
                })
            }
        };
        let max = |v: u32, m: u32, field| {
            if v <= m {
                Ok(())
            } else {
                Err(ModelError::DimensionTooLarge {
                    field,
                    value: v,
                    max: m,
                })
            }
        };

        if self.width == 0 || self.height == 0 || self.depth == 0 || self.array_length == 0 {
            return Err(ModelError::ZeroExtent);
        }
        if self.mip_level_count == 0 {
            return Err(ModelError::ZeroMipLevels);
        }

        // Shape rules per type.
        match t {
            T::Type1D | T::Type1DArray => {
                one(self.height, "height")?;
                one(self.depth, "depth")?;
                max(self.width, limits.max_texture_dimension_1d, "width")?;
            }
            T::TypeTextureBuffer => {
                one(self.height, "height")?;
                one(self.depth, "depth")?;
                // Apple: texture buffer width ≤ the family limit and ≤ maxBufferLength / pixel
                // size (Feature Set Tables, footnote 9).
                let bytes = u64::from(self.pixel_format.block_info().bytes);
                let by_length = u32::try_from(limits.max_buffer_length / bytes).unwrap_or(u32::MAX);
                max(
                    self.width,
                    limits.max_texture_buffer_width.min(by_length),
                    "width",
                )?;
            }
            T::Type2D | T::Type2DArray | T::Type2DMultisample | T::Type2DMultisampleArray => {
                one(self.depth, "depth")?;
                max(self.width, limits.max_texture_dimension_2d, "width")?;
                max(self.height, limits.max_texture_dimension_2d, "height")?;
            }
            T::TypeCube | T::TypeCubeArray => {
                one(self.depth, "depth")?;
                if self.width != self.height {
                    return Err(ModelError::CubeNotSquare);
                }
                max(self.width, limits.max_texture_dimension_2d, "width")?;
            }
            T::Type3D => {
                max(self.width, limits.max_texture_dimension_3d, "width")?;
                max(self.height, limits.max_texture_dimension_3d, "height")?;
                max(self.depth, limits.max_texture_dimension_3d, "depth")?;
            }
        }
        if t.is_array() {
            max(
                self.layer_count()?,
                limits.max_texture_array_layers,
                "array_length",
            )?;
        } else {
            one(self.array_length, "array_length")?;
        }

        // Sampling and mips.
        let multisample = t.is_multisample();
        if multisample {
            if !limits.sample_count_ok(self.sample_count) {
                return Err(ModelError::InvalidSampleCount(self.sample_count));
            }
            if self.mip_level_count != 1 {
                return Err(ModelError::MultisampleMipmapped);
            }
        } else if self.sample_count != 1 {
            return Err(ModelError::InvalidSampleCount(self.sample_count));
        }
        if t == T::TypeTextureBuffer {
            one(self.mip_level_count, "mip_level_count")?;
        }
        let largest = match t {
            T::Type3D => self.width.max(self.height).max(self.depth),
            _ => self.width.max(self.height),
        };
        let max_mips = full_mip_chain(largest);
        if self.mip_level_count > max_mips {
            return Err(ModelError::TooManyMipLevels {
                requested: self.mip_level_count,
                max: max_mips,
            });
        }

        self.validate_format_rules()?;

        // Storage rules.
        let fmt = self.pixel_format;
        match self.storage_mode {
            StorageMode::Memoryless => {
                if self.usage != TextureUsage::RENDER_TARGET {
                    return Err(ModelError::MemorylessUsage);
                }
            }
            StorageMode::Managed if fmt.is_depth_stencil() || multisample => {
                return Err(ModelError::ManagedNotAllowed);
            }
            _ => {}
        }

        // Make sure the total size is representable.
        self.size_in_bytes().map(|_| ())
    }

    /// Format/type/usage compatibility against the Metal capability table.
    fn validate_format_rules(&self) -> Result<(), ModelError> {
        use TextureType as T;
        let t = self.texture_type;
        let fmt = self.pixel_format;
        let caps = fmt.metal_caps();
        if caps.is_empty() {
            return Err(ModelError::FormatNotUsable {
                format: fmt,
                reason: "not supported by any Apple-family GPU",
            });
        }
        if !fmt.is_creatable() {
            return Err(ModelError::FormatNotUsable {
                format: fmt,
                reason: "only valid for texture views",
            });
        }
        let type_ok = if fmt.is_compressed() {
            match fmt.compression() {
                // Apple's page of every ETC2/EAC and PVRTC format: only 2D, 2DArray and Cube
                // textures are supported.
                Compression::Etc2Eac | Compression::Pvrtc => {
                    matches!(t, T::Type2D | T::Type2DArray | T::TypeCube)
                }
                // BC/ASTC pages name no type restriction. Cube arrays are a generic Apple4
                // feature; "compressed volume texture formats" (Apple3, Feature Set Tables)
                // names no formats, and ETC2/EAC/PVRTC are excluded by their own pages, so 3D
                // is attributed to BC and ASTC [Annahme: this attribution].
                _ => matches!(
                    t,
                    T::Type2D | T::Type2DArray | T::TypeCube | T::TypeCubeArray | T::Type3D
                ),
            }
        } else if fmt.is_422() {
            // Apple's GBGR422/BGRG422 pages: only 2D non-mipmapped textures with an even
            // width; neither 2DArray nor Cube.
            matches!(t, T::Type2D)
        } else if fmt.is_depth_stencil() {
            !matches!(
                t,
                T::Type1D | T::Type1DArray | T::Type3D | T::TypeTextureBuffer
            )
        } else if t == T::TypeTextureBuffer {
            !matches!(fmt.texture_buffer_access(), TextureBufferAccess::None)
        } else {
            true
        };
        if !type_ok {
            return Err(ModelError::FormatNotAllowedForType {
                format: fmt,
                texture_type: t,
            });
        }
        if fmt.is_422() {
            if self.mip_level_count != 1 {
                return Err(ModelError::FormatNotUsable {
                    format: fmt,
                    reason: "4:2:2 textures with more than one mip level",
                });
            }
            if !self.width.is_multiple_of(2) {
                return Err(ModelError::InvalidValue {
                    field: "width",
                    reason: "4:2:2 textures need an even width",
                });
            }
        }
        if t.is_multisample() && !caps.contains(FormatCaps::MSAA) {
            return Err(ModelError::FormatNotAllowedForType {
                format: fmt,
                texture_type: t,
            });
        }

        let usage_err = || ModelError::UsageNotAllowedForFormat {
            format: fmt,
            usage: self.usage,
        };
        if self.usage.contains(TextureUsage::RENDER_TARGET) {
            let renderable = FormatCaps::COLOR
                .union(FormatCaps::DEPTH)
                .union(FormatCaps::STENCIL);
            if !caps.intersects(renderable) || t == T::TypeTextureBuffer {
                return Err(usage_err());
            }
        }
        if self.usage.contains(TextureUsage::SHADER_WRITE) {
            let ok = if t == T::TypeTextureBuffer {
                fmt.texture_buffer_access().can_write()
            } else {
                caps.contains(FormatCaps::WRITE)
            };
            if !ok {
                return Err(usage_err());
            }
        }
        if self.usage.contains(TextureUsage::SHADER_ATOMIC) && !caps.contains(FormatCaps::ATOMIC) {
            return Err(usage_err());
        }
        Ok(())
    }

    /// Bytes needed to store all mips, layers and samples (tightly packed, block-rounded,
    /// respecting minimum block counts). Memoryless textures report their logical size too;
    /// backends decide not to back them.
    ///
    /// # Errors
    ///
    /// [`ModelError::SizeOverflow`].
    pub fn size_in_bytes(&self) -> Result<u64, ModelError> {
        let b = self.pixel_format.block_info();
        let (min_bw, min_bh) = self.pixel_format.min_blocks();
        let layers = u64::from(self.layer_count()?);
        let mut total: u64 = 0;
        for level in 0..self.mip_level_count.min(32) {
            let (w, h, d) = self.level_extent(level);
            let level_bytes = u64::from(w.div_ceil(b.width).max(min_bw))
                .checked_mul(u64::from(h.div_ceil(b.height).max(min_bh)))
                .and_then(|v| v.checked_mul(u64::from(b.bytes)))
                .and_then(|v| v.checked_mul(u64::from(d)))
                .ok_or(ModelError::SizeOverflow)?;
            total = total
                .checked_add(level_bytes)
                .ok_or(ModelError::SizeOverflow)?;
        }
        total
            .checked_mul(layers)
            .and_then(|v| v.checked_mul(u64::from(self.sample_count)))
            .ok_or(ModelError::SizeOverflow)
    }
}

#[cfg(test)]
mod tests;
