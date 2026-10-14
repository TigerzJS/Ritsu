//! Protocol-neutral Metal object model (ADR-0005).
//!
//! Both frontends (P: Apple paravirtual GPU, G: Ritsu guest driver) translate their wire
//! formats into these types; backends only ever see these types. Raw numeric values of the
//! enums follow the public Metal SDK enums (`MTLStorageMode`, `MTLPixelFormat`, …) so that a
//! frontend can pass guest values through unchanged.
//!
//! **Assumption (to verify in WP-GPU-02 against the public SDK headers):** the raw values
//! listed here match the macOS 27 SDK. They are public API constants, not reverse-engineered.
//!
//! Every descriptor coming from the guest is validated by [`BufferDesc::validate`] /
//! [`TextureDesc::validate`] before a backend sees it. The rules are deliberately
//! conservative: rejecting a valid descriptor is a loud, fixable bug; accepting an invalid
//! one could be a silent misrender or a host crash.

use core::fmt;

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
    /// Unknown or unsupported raw `MTLPixelFormat`.
    #[error("unknown or unsupported pixel format {0}")]
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
    /// Unknown raw `MTLPrimitiveType`.
    #[error("unknown primitive type {0}")]
    UnknownPrimitiveType(u32),
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
    /// Invalid sample count for the texture type.
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
}

impl From<ModelError> for ritsu_core::Error {
    fn from(e: ModelError) -> Self {
        ritsu_core::Error::Guest(format!("gpu object model: {e}"))
    }
}

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
            /// Converts the raw Metal SDK value.
            ///
            /// # Errors
            ///
            /// Unknown values are reported, never mapped to a default.
            pub const fn from_raw(raw: u32) -> Result<Self, ModelError> {
                match raw {
                    $( $val => Ok(Self::$var), )*
                    _ => Err(ModelError::$err(raw)),
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

raw_enum! {
    /// `MTLStorageMode`: where a resource lives and who may access it.
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
    /// `MTLCPUCacheMode`.
    CpuCacheMode, UnknownCpuCacheMode {
        /// Default caching.
        DefaultCache = 0,
        /// Write-combined CPU mapping (CPU writes only).
        WriteCombined = 1,
    }
}

raw_enum! {
    /// `MTLHazardTrackingMode`.
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
    /// `MTLTextureType`.
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

raw_enum! {
    /// `MTLLoadAction`.
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
    /// `MTLStoreAction` (subset: resolve actions are not yet modelled → rejected).
    StoreAction, UnknownStoreAction {
        /// Contents may be discarded.
        DontCare = 0,
        /// Store contents.
        Store = 1,
    }
}

raw_enum! {
    /// `MTLPrimitiveType`.
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

/// `MTLTextureUsage` bit set.
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

/// Compression family of a pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compression {
    /// Uncompressed.
    None,
    /// BCn (S3TC/RGTC/BPTC): native on RTX 4050 and Iris Xe.
    Bc,
    /// ASTC: native on Apple GPUs, missing on both target GPUs → transcoding.
    Astc,
}

/// Size of one block (a pixel for uncompressed formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInfo {
    /// Block width in pixels.
    pub width: u32,
    /// Block height in pixels.
    pub height: u32,
    /// Bytes per block.
    pub bytes: u32,
}

macro_rules! pixel_formats {
    ($( $(#[$m:meta])* $name:ident = $val:literal : ($bw:literal, $bh:literal, $bytes:literal, $comp:ident, $ds:literal), )*) => {
        /// Subset of `MTLPixelFormat` modelled so far (raw values from the public SDK).
        ///
        /// Each entry knows its block size, compression family and whether it is a
        /// depth/stencil format. The full list follows in WP-GPU-05 together with the
        /// Vulkan format mapping table.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        #[allow(non_camel_case_types)] // keep Metal's spelling (e.g. BGRA8Unorm_sRGB)
        pub enum PixelFormat {
            $( $(#[$m])* $name, )*
        }

        impl PixelFormat {
            /// All modelled formats.
            pub const ALL: &'static [PixelFormat] = &[$(PixelFormat::$name,)*];

            /// Converts the raw SDK value.
            ///
            /// # Errors
            ///
            /// [`ModelError::UnknownPixelFormat`] for values not (yet) modelled,
            /// including `MTLPixelFormatInvalid` (0).
            pub const fn from_raw(raw: u32) -> Result<Self, ModelError> {
                match raw {
                    $( $val => Ok(Self::$name), )*
                    _ => Err(ModelError::UnknownPixelFormat(raw)),
                }
            }

            /// Raw SDK value.
            #[must_use]
            pub const fn raw(self) -> u32 {
                match self { $( Self::$name => $val, )* }
            }

            /// Block dimensions and bytes per block.
            #[must_use]
            pub const fn block_info(self) -> BlockInfo {
                match self {
                    $( Self::$name => BlockInfo { width: $bw, height: $bh, bytes: $bytes }, )*
                }
            }

            /// Compression family.
            #[must_use]
            pub const fn compression(self) -> Compression {
                match self { $( Self::$name => Compression::$comp, )* }
            }

            /// Whether this is a depth and/or stencil format.
            #[must_use]
            pub const fn is_depth_stencil(self) -> bool {
                match self { $( Self::$name => $ds, )* }
            }
        }
    };
}

pixel_formats! {
    /// 8-bit alpha.
    A8Unorm = 1: (1, 1, 1, None, false),
    /// 8-bit red.
    R8Unorm = 10: (1, 1, 1, None, false),
    /// 8-bit red, unsigned integer.
    R8Uint = 13: (1, 1, 1, None, false),
    /// 16-bit float red.
    R16Float = 25: (1, 1, 2, None, false),
    /// 8-bit red/green.
    RG8Unorm = 30: (1, 1, 2, None, false),
    /// 32-bit unsigned integer red.
    R32Uint = 53: (1, 1, 4, None, false),
    /// 32-bit float red.
    R32Float = 55: (1, 1, 4, None, false),
    /// 16-bit float red/green.
    RG16Float = 65: (1, 1, 4, None, false),
    /// 8-bit RGBA.
    RGBA8Unorm = 70: (1, 1, 4, None, false),
    /// 8-bit RGBA, sRGB.
    RGBA8Unorm_sRGB = 71: (1, 1, 4, None, false),
    /// 8-bit BGRA (the usual drawable format).
    BGRA8Unorm = 80: (1, 1, 4, None, false),
    /// 8-bit BGRA, sRGB.
    BGRA8Unorm_sRGB = 81: (1, 1, 4, None, false),
    /// 10:10:10:2 RGBA.
    RGB10A2Unorm = 90: (1, 1, 4, None, false),
    /// Packed 11:11:10 float.
    RG11B10Float = 92: (1, 1, 4, None, false),
    /// 10:10:10:2 BGRA.
    BGR10A2Unorm = 94: (1, 1, 4, None, false),
    /// 32-bit float red/green.
    RG32Float = 105: (1, 1, 8, None, false),
    /// 16-bit float RGBA (EDR/HDR surfaces).
    RGBA16Float = 115: (1, 1, 8, None, false),
    /// 32-bit float RGBA.
    RGBA32Float = 125: (1, 1, 16, None, false),
    /// BC1 (DXT1).
    BC1_RGBA = 130: (4, 4, 8, Bc, false),
    /// BC3 (DXT5).
    BC3_RGBA = 134: (4, 4, 16, Bc, false),
    /// BC5 two-channel.
    BC5_RGUnorm = 142: (4, 4, 16, Bc, false),
    /// BC7.
    BC7_RGBAUnorm = 152: (4, 4, 16, Bc, false),
    /// ASTC 4×4 sRGB.
    ASTC_4x4_sRGB = 186: (4, 4, 16, Astc, false),
    /// ASTC 8×8 sRGB.
    ASTC_8x8_sRGB = 194: (8, 8, 16, Astc, false),
    /// ASTC 4×4 LDR.
    ASTC_4x4_LDR = 204: (4, 4, 16, Astc, false),
    /// ASTC 8×8 LDR.
    ASTC_8x8_LDR = 212: (8, 8, 16, Astc, false),
    /// 16-bit depth.
    Depth16Unorm = 250: (1, 1, 2, None, true),
    /// 32-bit float depth.
    Depth32Float = 252: (1, 1, 4, None, true),
    /// 8-bit stencil.
    Stencil8 = 253: (1, 1, 1, None, true),
    /// 32-bit float depth + 8-bit stencil (8 bytes incl. padding).
    Depth32Float_Stencil8 = 260: (1, 1, 8, None, true),
}

impl PixelFormat {
    /// Whether the format is block-compressed.
    #[must_use]
    pub const fn is_compressed(self) -> bool {
        !matches!(self.compression(), Compression::None)
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
}

impl ResourceLimits {
    /// Limits of the Apple7 (M1) Metal family, the ceiling for anything reported to the
    /// guest. **Assumption:** values from Apple's public Metal feature set tables.
    pub const APPLE7: Self = Self {
        max_buffer_length: 1 << 32,
        max_texture_dimension_1d: 16384,
        max_texture_dimension_2d: 16384,
        max_texture_dimension_3d: 2048,
        max_texture_array_layers: 2048,
        max_threads_per_threadgroup: 1024,
        max_color_attachments: 8,
    };
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

/// Guest description of a texture (`MTLTextureDescriptor`, subset).
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
}

/// Number of mip levels of a full chain for the largest dimension `max_dim` (≥ 1).
const fn full_mip_chain(max_dim: u32) -> u32 {
    32 - max_dim.leading_zeros()
}

impl TextureDesc {
    /// A 2D texture with one mip level, one sample, private storage.
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
        }
    }

    /// Number of 2D layers (array length × 6 for cubes).
    ///
    /// # Errors
    ///
    /// [`ModelError::SizeOverflow`].
    pub fn layer_count(&self) -> Result<u32, ModelError> {
        match self.texture_type {
            TextureType::TypeCube | TextureType::TypeCubeArray => self
                .array_length
                .checked_mul(6)
                .ok_or(ModelError::SizeOverflow),
            _ => Ok(self.array_length),
        }
    }

    /// Validates the descriptor against device limits.
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
            T::Type1D | T::Type1DArray | T::TypeTextureBuffer => {
                one(self.height, "height")?;
                one(self.depth, "depth")?;
                max(self.width, limits.max_texture_dimension_1d, "width")?;
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
        match t {
            T::Type1D
            | T::Type2D
            | T::Type2DMultisample
            | T::TypeCube
            | T::Type3D
            | T::TypeTextureBuffer => one(self.array_length, "array_length")?,
            T::Type1DArray | T::Type2DArray | T::Type2DMultisampleArray | T::TypeCubeArray => {
                max(
                    self.layer_count()?,
                    limits.max_texture_array_layers,
                    "array_length",
                )?;
            }
        }

        // Sampling and mips.
        let multisample = matches!(t, T::Type2DMultisample | T::Type2DMultisampleArray);
        if multisample {
            if !matches!(self.sample_count, 1 | 2 | 4 | 8) {
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

        // Format/type/usage compatibility (conservative; see module docs).
        let fmt = self.pixel_format;
        let fmt_type_ok = if fmt.is_compressed() {
            matches!(
                t,
                T::Type2D | T::Type2DArray | T::TypeCube | T::TypeCubeArray
            )
        } else if fmt.is_depth_stencil() {
            !matches!(
                t,
                T::Type1D | T::Type1DArray | T::Type3D | T::TypeTextureBuffer
            )
        } else {
            true
        };
        if !fmt_type_ok {
            return Err(ModelError::FormatNotAllowedForType {
                format: fmt,
                texture_type: t,
            });
        }
        let write_like = TextureUsage::RENDER_TARGET
            .union(TextureUsage::SHADER_WRITE)
            .union(TextureUsage::SHADER_ATOMIC);
        if fmt.is_compressed() && self.usage.intersects(write_like) {
            return Err(ModelError::UsageNotAllowedForFormat {
                format: fmt,
                usage: self.usage,
            });
        }
        if self.usage.contains(TextureUsage::SHADER_ATOMIC) && !matches!(fmt, PixelFormat::R32Uint)
        {
            return Err(ModelError::UsageNotAllowedForFormat {
                format: fmt,
                usage: self.usage,
            });
        }

        // Storage rules.
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

    /// Bytes needed to store all mips, layers and samples (tightly packed, block-rounded).
    /// Memoryless textures report their logical size too; backends decide not to back them.
    ///
    /// # Errors
    ///
    /// [`ModelError::SizeOverflow`].
    pub fn size_in_bytes(&self) -> Result<u64, ModelError> {
        let b = self.pixel_format.block_info();
        let layers = u64::from(self.layer_count()?);
        let mut total: u64 = 0;
        for level in 0..self.mip_level_count.min(32) {
            let w = (self.width >> level).max(1);
            let h = (self.height >> level).max(1);
            let d = if self.texture_type == TextureType::Type3D {
                (self.depth >> level).max(1)
            } else {
                1
            };
            let level_bytes = u64::from(w.div_ceil(b.width))
                .checked_mul(u64::from(h.div_ceil(b.height)))
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
mod tests {
    use super::*;

    const L: ResourceLimits = ResourceLimits::APPLE7;
    const RT_READ: TextureUsage = TextureUsage::RENDER_TARGET.union(TextureUsage::SHADER_READ);

    fn tex2d() -> TextureDesc {
        TextureDesc::new_2d(PixelFormat::BGRA8Unorm, 1920, 1080, RT_READ)
    }

    #[test]
    fn raw_enums_round_trip_and_reject_unknown() {
        for v in 0..4 {
            assert_eq!(StorageMode::from_raw(v).unwrap().raw(), v);
        }
        assert_eq!(
            StorageMode::from_raw(4),
            Err(ModelError::UnknownStorageMode(4))
        );
        assert!(CpuCacheMode::from_raw(2).is_err());
        assert!(HazardTrackingMode::from_raw(3).is_err());
        assert!(TextureType::from_raw(10).is_err());
        assert!(LoadAction::from_raw(3).is_err());
        assert!(
            StoreAction::from_raw(2).is_err(),
            "resolve not modelled yet"
        );
        assert!(PrimitiveType::from_raw(5).is_err());
        assert!(TextureUsage::from_raw(0x8).is_err());
        assert!(TextureUsage::from_raw(0x37).is_ok());
    }

    #[test]
    fn pixel_formats_round_trip_and_blocks() {
        for f in PixelFormat::ALL {
            assert_eq!(PixelFormat::from_raw(f.raw()), Ok(*f));
            let b = f.block_info();
            assert!(b.width >= 1 && b.height >= 1 && b.bytes >= 1);
            assert_eq!(f.is_compressed(), b.width > 1, "{f:?}");
        }
        assert_eq!(
            PixelFormat::from_raw(0),
            Err(ModelError::UnknownPixelFormat(0))
        );
        assert_eq!(PixelFormat::BGRA8Unorm.block_info().bytes, 4);
        assert_eq!(PixelFormat::ASTC_8x8_LDR.block_info().width, 8);
        assert_eq!(PixelFormat::BC1_RGBA.compression(), Compression::Bc);
        assert!(PixelFormat::Depth32Float.is_depth_stencil());
    }

    #[test]
    fn buffer_validation() {
        let mut d = BufferDesc::from_raw(4096, 0, 0, 0).unwrap();
        assert!(d.validate(&L).is_ok());
        d.length = 0;
        assert_eq!(d.validate(&L), Err(ModelError::ZeroLength));
        d.length = L.max_buffer_length + 1;
        assert!(matches!(
            d.validate(&L),
            Err(ModelError::BufferTooLarge { .. })
        ));
        d.length = 16;
        d.storage_mode = StorageMode::Memoryless;
        assert_eq!(d.validate(&L), Err(ModelError::MemorylessBuffer));
        assert!(BufferDesc::from_raw(16, 9, 0, 0).is_err());
    }

    #[test]
    fn valid_textures() {
        assert!(tex2d().validate(&L).is_ok());
        let mut t = tex2d();
        t.mip_level_count = 11; // 1920 → 11 levels
        assert!(t.validate(&L).is_ok());
        let cube = TextureDesc {
            texture_type: TextureType::TypeCube,
            width: 256,
            height: 256,
            ..tex2d()
        };
        assert!(cube.validate(&L).is_ok());
        let ms = TextureDesc {
            texture_type: TextureType::Type2DMultisample,
            sample_count: 4,
            usage: TextureUsage::RENDER_TARGET,
            storage_mode: StorageMode::Memoryless,
            ..tex2d()
        };
        assert!(ms.validate(&L).is_ok());
        let bc = TextureDesc::new_2d(
            PixelFormat::BC7_RGBAUnorm,
            1024,
            1024,
            TextureUsage::SHADER_READ,
        );
        assert!(bc.validate(&L).is_ok());
        let vol = TextureDesc {
            texture_type: TextureType::Type3D,
            width: 64,
            height: 64,
            depth: 128,
            mip_level_count: 8,
            ..tex2d()
        };
        assert!(vol.validate(&L).is_ok());
    }

    #[test]
    fn invalid_textures() {
        type Case = (TextureDesc, fn(&ModelError) -> bool);
        let cases: Vec<Case> = vec![
            (
                TextureDesc {
                    width: 0,
                    ..tex2d()
                },
                |e| *e == ModelError::ZeroExtent,
            ),
            (
                TextureDesc {
                    array_length: 0,
                    ..tex2d()
                },
                |e| *e == ModelError::ZeroExtent,
            ),
            (
                TextureDesc {
                    width: 16385,
                    ..tex2d()
                },
                |e| matches!(e, ModelError::DimensionTooLarge { field: "width", .. }),
            ),
            (
                TextureDesc {
                    depth: 2,
                    ..tex2d()
                },
                |e| matches!(e, ModelError::DimensionMustBeOne { field: "depth", .. }),
            ),
            (
                TextureDesc {
                    array_length: 2,
                    ..tex2d()
                },
                |e| {
                    matches!(
                        e,
                        ModelError::DimensionMustBeOne {
                            field: "array_length",
                            ..
                        }
                    )
                },
            ),
            (
                TextureDesc {
                    mip_level_count: 12,
                    ..tex2d()
                },
                |e| {
                    *e == ModelError::TooManyMipLevels {
                        requested: 12,
                        max: 11,
                    }
                },
            ),
            (
                TextureDesc {
                    mip_level_count: 0,
                    ..tex2d()
                },
                |e| *e == ModelError::ZeroMipLevels,
            ),
            (
                TextureDesc {
                    sample_count: 4,
                    ..tex2d()
                },
                |e| *e == ModelError::InvalidSampleCount(4),
            ),
            (
                TextureDesc {
                    texture_type: TextureType::Type2DMultisample,
                    sample_count: 3,
                    ..tex2d()
                },
                |e| *e == ModelError::InvalidSampleCount(3),
            ),
            (
                TextureDesc {
                    texture_type: TextureType::Type2DMultisample,
                    sample_count: 4,
                    mip_level_count: 2,
                    ..tex2d()
                },
                |e| *e == ModelError::MultisampleMipmapped,
            ),
            (
                TextureDesc {
                    texture_type: TextureType::TypeCube,
                    width: 64,
                    height: 32,
                    ..tex2d()
                },
                |e| *e == ModelError::CubeNotSquare,
            ),
            (
                TextureDesc {
                    texture_type: TextureType::Type1D,
                    height: 2,
                    ..tex2d()
                },
                |e| {
                    matches!(
                        e,
                        ModelError::DimensionMustBeOne {
                            field: "height",
                            ..
                        }
                    )
                },
            ),
            (
                TextureDesc {
                    pixel_format: PixelFormat::ASTC_4x4_LDR,
                    ..tex2d()
                },
                |e| matches!(e, ModelError::UsageNotAllowedForFormat { .. }),
            ),
            (
                TextureDesc {
                    texture_type: TextureType::Type1D,
                    height: 1,
                    pixel_format: PixelFormat::BC1_RGBA,
                    usage: TextureUsage::SHADER_READ,
                    ..tex2d()
                },
                |e| matches!(e, ModelError::FormatNotAllowedForType { .. }),
            ),
            (
                TextureDesc {
                    texture_type: TextureType::Type3D,
                    pixel_format: PixelFormat::Depth32Float,
                    width: 8,
                    height: 8,
                    depth: 8,
                    ..tex2d()
                },
                |e| matches!(e, ModelError::FormatNotAllowedForType { .. }),
            ),
            (
                TextureDesc {
                    storage_mode: StorageMode::Memoryless,
                    ..tex2d()
                },
                |e| *e == ModelError::MemorylessUsage,
            ),
            (
                TextureDesc {
                    pixel_format: PixelFormat::Depth32Float,
                    storage_mode: StorageMode::Managed,
                    ..tex2d()
                },
                |e| *e == ModelError::ManagedNotAllowed,
            ),
            (
                TextureDesc {
                    usage: TextureUsage::SHADER_ATOMIC.union(TextureUsage::SHADER_READ),
                    ..tex2d()
                },
                |e| matches!(e, ModelError::UsageNotAllowedForFormat { .. }),
            ),
            (
                TextureDesc {
                    texture_type: TextureType::Type2DArray,
                    array_length: 4096,
                    ..tex2d()
                },
                |e| {
                    matches!(
                        e,
                        ModelError::DimensionTooLarge {
                            field: "array_length",
                            ..
                        }
                    )
                },
            ),
        ];
        for (i, (desc, pred)) in cases.iter().enumerate() {
            let err = desc
                .validate(&L)
                .expect_err(&format!("case {i} should fail"));
            assert!(pred(&err), "case {i}: unexpected error {err:?}");
        }
    }

    #[test]
    fn size_in_bytes() {
        // 4×4 RGBA8, full mip chain: 64 + 16 + 4 = 84 bytes.
        let t = TextureDesc {
            mip_level_count: 3,
            ..TextureDesc::new_2d(PixelFormat::RGBA8Unorm, 4, 4, RT_READ)
        };
        assert_eq!(t.size_in_bytes(), Ok(84));
        // BC1 5×5 → 2×2 blocks × 8 bytes; cube ×6.
        let bc = TextureDesc {
            texture_type: TextureType::TypeCube,
            ..TextureDesc::new_2d(PixelFormat::BC1_RGBA, 5, 5, TextureUsage::SHADER_READ)
        };
        assert_eq!(bc.size_in_bytes(), Ok(32 * 6));
        // Overflow is detected, not wrapped (bypassing validate on purpose).
        let huge = TextureDesc {
            texture_type: TextureType::TypeCubeArray,
            width: u32::MAX,
            height: u32::MAX,
            array_length: u32::MAX,
            pixel_format: PixelFormat::RGBA32Float,
            ..tex2d()
        };
        assert_eq!(huge.size_in_bytes(), Err(ModelError::SizeOverflow));
    }

    #[test]
    fn model_error_converts_to_guest_error() {
        let e: ritsu_core::Error = ModelError::ZeroLength.into();
        assert!(matches!(e, ritsu_core::Error::Guest(_)));
    }
}
