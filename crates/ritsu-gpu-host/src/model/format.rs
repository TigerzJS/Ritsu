//! `MTLPixelFormat`: the complete public table with block sizes, numeric types and the texture
//! capabilities **Metal guarantees** for each format.
//!
//! What a concrete GPU can do with a format is a separate question answered by
//! [`crate::caps::Capabilities::formats`]; [`crate::caps::format_support`] compares the two.
//!
//! # Sources
//!
//! * Raw values: public Metal SDK header `MTLPixelFormat.h` (`MTLPixelFormat` enum), as
//!   published in Apple's developer documentation; cross-checked against the bindings that
//!   `objc2-metal` generates from the public SDK headers. No header text is copied.
//! * Capabilities ([`PixelFormat::metal_caps`]): Apple, *Metal Feature Set Tables* (PDF, edition
//!   of 2026-05-21), table "Texture capabilities by pixel format", column **Apple7**. Footnote 1
//!   of that table: all Metal 3 and Metal 4 devices support at least the Apple7 column, so it
//!   is the floor a guest on "Apple silicon" may rely on. The macOS notes apply (BC formats are
//!   filterable on all Apple7 macOS devices, footnote 9).
//! * Texture-buffer access ([`PixelFormat::texture_buffer_access`]): same PDF, table "Texture
//!   buffer pixel formats".
//! * View compatibility ([`PixelFormat::view_compatible`]): Apple documentation of
//!   `MTLTexture.makeTextureView(pixelFormat:)` and `MTLTextureUsage.pixelFormatView`.
//! * PVRTC minimum size (2 × 2 blocks): PVRTC1 data-size rule from Imagination's public format
//!   documentation.
//!
//! Everything not covered by these sources is marked **\[Annahme\]** at the place it is used.

use core::fmt;

use super::ModelError;

/// Compression family of a pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Compression {
    /// Uncompressed (including the subsampled 4:2:2 formats).
    None,
    /// BCn (S3TC/RGTC/BPTC): native on RTX 4050 and Iris Xe.
    Bc,
    /// ETC2/EAC: no hardware on either target GPU \[Annahme\] → transcoding.
    Etc2Eac,
    /// ASTC LDR (linear and sRGB): no hardware on either target GPU → transcoding.
    Astc,
    /// ASTC HDR: no hardware on either target GPU → transcoding.
    AstcHdr,
    /// PVRTC1: no Vulkan counterpart at all → transcoding.
    Pvrtc,
}

/// Size of one block (a pixel for ordinary formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInfo {
    /// Block width in pixels.
    pub width: u32,
    /// Block height in pixels.
    pub height: u32,
    /// Bytes per block.
    pub bytes: u32,
}

/// How shaders see the values of a format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NumericType {
    /// Unsigned normalized (`[0, 1]`).
    Unorm,
    /// Signed normalized (`[-1, 1]`).
    Snorm,
    /// Unsigned integer.
    Uint,
    /// Signed integer.
    Sint,
    /// Floating point (including packed and shared-exponent floats).
    Float,
    /// Apple extended-range fixed point (`*_XR`): normalized with a bias, range beyond `[0, 1]`.
    ExtendedRange,
}

impl NumericType {
    /// Whether values are integers (no filtering, no blending, integer clear values).
    #[must_use]
    pub const fn is_integer(self) -> bool {
        matches!(self, Self::Uint | Self::Sint)
    }
}

/// Which aspects a format has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aspects {
    /// Colour.
    Color,
    /// Depth only.
    Depth,
    /// Stencil only (including the stencil views of combined formats).
    Stencil,
    /// Combined depth and stencil.
    DepthStencil,
}

impl Aspects {
    /// Whether a depth aspect exists.
    #[must_use]
    pub const fn has_depth(self) -> bool {
        matches!(self, Self::Depth | Self::DepthStencil)
    }

    /// Whether a stencil aspect exists.
    #[must_use]
    pub const fn has_stencil(self) -> bool {
        matches!(self, Self::Stencil | Self::DepthStencil)
    }
}

/// Texture capabilities of a pixel format, as a bit set.
///
/// The same type describes what Metal guarantees ([`PixelFormat::metal_caps`]) and what a
/// GPU offers ([`crate::caps::FormatCapsTable`]). Names follow the Metal Feature Set Tables;
/// `SAMPLE`, `DEPTH` and `STENCIL` are added because the tables state them implicitly ("all
/// kernels can read or sample any pixel format"; depth/stencil formats are attachments).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FormatCaps(u16);

impl FormatCaps {
    /// No capability (format unusable).
    pub const NONE: Self = Self(0);
    /// Read or sample in shaders, copy in blits.
    pub const SAMPLE: Self = Self(1 << 0);
    /// Linear filtering while sampling.
    pub const FILTER: Self = Self(1 << 1);
    /// Per-pixel shader writes.
    pub const WRITE: Self = Self(1 << 2);
    /// Colour render target.
    pub const COLOR: Self = Self(1 << 3);
    /// Blending as a colour render target.
    pub const BLEND: Self = Self(1 << 4);
    /// Multisample textures.
    pub const MSAA: Self = Self(1 << 5);
    /// Source of a multisample resolve.
    pub const RESOLVE: Self = Self(1 << 6);
    /// Sparse textures.
    pub const SPARSE: Self = Self(1 << 7);
    /// Shader atomics.
    pub const ATOMIC: Self = Self(1 << 8);
    /// Depth attachment.
    pub const DEPTH: Self = Self(1 << 9);
    /// Stencil attachment.
    pub const STENCIL: Self = Self(1 << 10);

    const NAMES: [(Self, &'static str); 11] = [
        (Self::SAMPLE, "sample"),
        (Self::FILTER, "filter"),
        (Self::WRITE, "write"),
        (Self::COLOR, "color"),
        (Self::BLEND, "blend"),
        (Self::MSAA, "msaa"),
        (Self::RESOLVE, "resolve"),
        (Self::SPARSE, "sparse"),
        (Self::ATOMIC, "atomic"),
        (Self::DEPTH, "depth"),
        (Self::STENCIL, "stencil"),
    ];

    /// Raw bits.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether no bit is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
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

    /// Intersection.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Bits of `self` that are not in `other`.
    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl fmt::Debug for FormatCaps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut set = f.debug_set();
        for (bit, name) in Self::NAMES {
            if self.contains(bit) {
                set.entry(&format_args!("{name}"));
            }
        }
        set.finish()
    }
}

/// Access a texture buffer (`MTLTextureTypeTextureBuffer`) of a format allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureBufferAccess {
    /// Not usable as a texture buffer.
    None,
    /// `access::read` only.
    Read,
    /// `access::read` and `access::write`, but not `access::read_write`.
    ReadWrite,
    /// Read, write and `read_write`.
    All,
}

impl TextureBufferAccess {
    /// Whether shader writes are allowed.
    #[must_use]
    pub const fn can_write(self) -> bool {
        matches!(self, Self::ReadWrite | Self::All)
    }
}

/// Component layout (number, size and order of components). Two formats with the same layout
/// differ only in how the bits are interpreted, so a view between them does not need
/// `MTLTextureUsagePixelFormatView` (Apple documentation of that flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Layout {
    A8,
    R8,
    RG8,
    R16,
    RG16,
    RGBA8,
    BGRA8,
    R32,
    RG32,
    RGBA16,
    RGBA32,
    B5G6R5,
    A1BGR5,
    ABGR4,
    BGR5A1,
    RGB10A2,
    BGR10A2,
    RG11B10,
    RGB9E5,
    BGR10Xr,
    BGRA10Xr,
    /// Only equal to itself (compressed, 4:2:2, depth/stencil formats).
    Unique,
}

/// Static description of one pixel format (one table row).
#[derive(Debug, Clone, Copy)]
struct Row {
    block: BlockInfo,
    compression: Compression,
    numeric: NumericType,
    srgb: bool,
    aspects: Aspects,
    layout: Layout,
    caps: FormatCaps,
    texture_buffer: TextureBufferAccess,
}

// Capability sets of the Apple7 column, named after the table cells they encode.
const S: FormatCaps = FormatCaps::SAMPLE;
/// "All": Filter, Write, Color, Blend, MSAA, Sparse, Resolve.
const ALL: FormatCaps = FormatCaps(
    S.0 | FormatCaps::FILTER.0
        | FormatCaps::WRITE.0
        | FormatCaps::COLOR.0
        | FormatCaps::BLEND.0
        | FormatCaps::MSAA.0
        | FormatCaps::RESOLVE.0
        | FormatCaps::SPARSE.0,
);
/// Integer formats: Write, Color, MSAA, Sparse.
const INT: FormatCaps = FormatCaps(
    S.0 | FormatCaps::WRITE.0 | FormatCaps::COLOR.0 | FormatCaps::MSAA.0 | FormatCaps::SPARSE.0,
);
/// `R32Uint`/`R32Sint`: Atomic, Write, Color, MSAA, Sparse.
const INT_ATOMIC: FormatCaps = FormatCaps(INT.0 | FormatCaps::ATOMIC.0);
/// 32-bit float formats on Apple7: Write, Color, MSAA, Blend, Sparse (no Filter, no Resolve).
const F32: FormatCaps = FormatCaps(INT.0 | FormatCaps::BLEND.0);
/// Packed 16-bit formats: Filter, Color, MSAA, Resolve, Blend, Sparse (no Write).
const PACKED16: FormatCaps = FormatCaps(
    S.0 | FormatCaps::FILTER.0
        | FormatCaps::COLOR.0
        | FormatCaps::MSAA.0
        | FormatCaps::RESOLVE.0
        | FormatCaps::BLEND.0
        | FormatCaps::SPARSE.0,
);
/// Compressed formats: Filter, Sparse.
const COMPRESSED: FormatCaps = FormatCaps(S.0 | FormatCaps::FILTER.0 | FormatCaps::SPARSE.0);
/// 4:2:2 formats: Filter.
const YUV: FormatCaps = FormatCaps(S.0 | FormatCaps::FILTER.0);
/// `Depth16Unorm`: Filter, MSAA, Resolve, Sparse.
const D16: FormatCaps = FormatCaps(
    S.0 | FormatCaps::FILTER.0
        | FormatCaps::MSAA.0
        | FormatCaps::RESOLVE.0
        | FormatCaps::SPARSE.0
        | FormatCaps::DEPTH.0,
);
/// `Depth32Float`: MSAA, Resolve, Sparse (Filter only from Apple9).
const D32: FormatCaps = FormatCaps(
    S.0 | FormatCaps::MSAA.0 | FormatCaps::RESOLVE.0 | FormatCaps::SPARSE.0 | FormatCaps::DEPTH.0,
);
/// `Stencil8`: MSAA, Resolve, Sparse.
const S8: FormatCaps = FormatCaps(
    S.0 | FormatCaps::MSAA.0 | FormatCaps::RESOLVE.0 | FormatCaps::SPARSE.0 | FormatCaps::STENCIL.0,
);
/// `Depth32Float_Stencil8`: MSAA, Resolve.
const D32S8: FormatCaps = FormatCaps(
    S.0 | FormatCaps::MSAA.0 | FormatCaps::RESOLVE.0 | FormatCaps::DEPTH.0 | FormatCaps::STENCIL.0,
);
/// `X32_Stencil8` (stencil view): MSAA; never an attachment of its own.
const X32S8: FormatCaps = FormatCaps(S.0 | FormatCaps::MSAA.0);
/// Not in the Apple7 column at all (Mac-only formats of Intel/AMD Macs).
const NONE: FormatCaps = FormatCaps::NONE;

const TB_NO: TextureBufferAccess = TextureBufferAccess::None;
const TB_R: TextureBufferAccess = TextureBufferAccess::Read;
const TB_RW: TextureBufferAccess = TextureBufferAccess::ReadWrite;
const TB_ALL: TextureBufferAccess = TextureBufferAccess::All;

macro_rules! pixel_formats {
    ($(
        $(#[$m:meta])*
        $name:ident = $val:literal : ($bw:literal, $bh:literal, $bytes:literal),
            $comp:ident, $num:ident, $srgb:literal, $asp:ident, $lay:ident, $caps:ident, $tb:ident;
    )*) => {
        /// `MTLPixelFormat`: every value of the public SDK enum except `Invalid` (0) and
        /// `Unspecialized` (263, a Metal 4 pipeline placeholder, see
        /// [`PixelFormat::from_raw`]).
        ///
        /// Raw values: `MTLPixelFormat.h` of the public SDK (see module docs).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        #[allow(non_camel_case_types)] // keep Metal's spelling (e.g. BGRA8Unorm_sRGB)
        pub enum PixelFormat {
            $( $(#[$m])* $name, )*
        }

        #[allow(non_camel_case_types, dead_code, clippy::upper_case_acronyms)]
        #[derive(Clone, Copy)]
        enum Index {
            $( $name, )*
        }

        impl PixelFormat {
            /// All formats, in table order.
            pub const ALL: &'static [PixelFormat] = &[$(PixelFormat::$name,)*];

            /// Number of formats ([`PixelFormat::index`] is below this).
            pub const COUNT: usize = Self::ALL.len();

            /// Dense index in `0..COUNT` (for per-format tables).
            #[must_use]
            pub const fn index(self) -> usize {
                match self { $( Self::$name => Index::$name as usize, )* }
            }

            /// Converts the raw SDK value.
            ///
            /// # Errors
            ///
            /// [`ModelError::UnknownPixelFormat`] for values not in the SDK enum and for
            /// `MTLPixelFormatInvalid` (0); [`ModelError::Unsupported`] for
            /// `MTLPixelFormatUnspecialized` (263), which only Metal 4 flexible pipelines use.
            pub const fn from_raw(raw: u32) -> Result<Self, ModelError> {
                match raw {
                    $( $val => Ok(Self::$name), )*
                    UNSPECIALIZED_RAW => Err(ModelError::Unsupported(
                        "MTLPixelFormatUnspecialized (Metal 4 flexible render pipelines)",
                    )),
                    _ => Err(ModelError::UnknownPixelFormat(raw)),
                }
            }

            /// Raw SDK value.
            #[must_use]
            pub const fn raw(self) -> u32 {
                match self { $( Self::$name => $val, )* }
            }

            const fn row(self) -> Row {
                match self {
                    $( Self::$name => Row {
                        block: BlockInfo { width: $bw, height: $bh, bytes: $bytes },
                        compression: Compression::$comp,
                        numeric: NumericType::$num,
                        srgb: $srgb,
                        aspects: Aspects::$asp,
                        layout: Layout::$lay,
                        caps: $caps,
                        texture_buffer: $tb,
                    }, )*
                }
            }
        }
    };
}

/// Raw value of `MTLPixelFormatInvalid`.
pub const INVALID_RAW: u32 = 0;
/// Raw value of `MTLPixelFormatUnspecialized` (Metal 4).
pub const UNSPECIALIZED_RAW: u32 = 263;

pixel_formats! {
    // Ordinary 8-bit.
    /// 8-bit alpha.
    A8Unorm = 1: (1, 1, 1), None, Unorm, false, Color, A8, ALL, TB_ALL;
    /// 8-bit red.
    R8Unorm = 10: (1, 1, 1), None, Unorm, false, Color, R8, ALL, TB_ALL;
    /// 8-bit red, sRGB.
    R8Unorm_sRGB = 11: (1, 1, 1), None, Unorm, true, Color, R8, ALL, TB_NO;
    /// 8-bit red, signed normalized.
    R8Snorm = 12: (1, 1, 1), None, Snorm, false, Color, R8, ALL, TB_RW;
    /// 8-bit red, unsigned integer.
    R8Uint = 13: (1, 1, 1), None, Uint, false, Color, R8, INT, TB_ALL;
    /// 8-bit red, signed integer.
    R8Sint = 14: (1, 1, 1), None, Sint, false, Color, R8, INT, TB_ALL;
    // Ordinary 16-bit.
    /// 16-bit red.
    R16Unorm = 20: (1, 1, 2), None, Unorm, false, Color, R16, ALL, TB_RW;
    /// 16-bit red, signed normalized.
    R16Snorm = 22: (1, 1, 2), None, Snorm, false, Color, R16, ALL, TB_RW;
    /// 16-bit red, unsigned integer.
    R16Uint = 23: (1, 1, 2), None, Uint, false, Color, R16, INT, TB_ALL;
    /// 16-bit red, signed integer.
    R16Sint = 24: (1, 1, 2), None, Sint, false, Color, R16, INT, TB_ALL;
    /// 16-bit float red.
    R16Float = 25: (1, 1, 2), None, Float, false, Color, R16, ALL, TB_ALL;
    /// 8-bit red/green.
    RG8Unorm = 30: (1, 1, 2), None, Unorm, false, Color, RG8, ALL, TB_RW;
    /// 8-bit red/green, sRGB.
    RG8Unorm_sRGB = 31: (1, 1, 2), None, Unorm, true, Color, RG8, ALL, TB_NO;
    /// 8-bit red/green, signed normalized.
    RG8Snorm = 32: (1, 1, 2), None, Snorm, false, Color, RG8, ALL, TB_RW;
    /// 8-bit red/green, unsigned integer.
    RG8Uint = 33: (1, 1, 2), None, Uint, false, Color, RG8, INT, TB_RW;
    /// 8-bit red/green, signed integer.
    RG8Sint = 34: (1, 1, 2), None, Sint, false, Color, RG8, INT, TB_RW;
    // Packed 16-bit.
    /// 5:6:5 BGR.
    B5G6R5Unorm = 40: (1, 1, 2), None, Unorm, false, Color, B5G6R5, PACKED16, TB_NO;
    /// 1:5:5:5 ABGR.
    A1BGR5Unorm = 41: (1, 1, 2), None, Unorm, false, Color, A1BGR5, PACKED16, TB_NO;
    /// 4:4:4:4 ABGR.
    ABGR4Unorm = 42: (1, 1, 2), None, Unorm, false, Color, ABGR4, PACKED16, TB_NO;
    /// 5:5:5:1 BGRA.
    BGR5A1Unorm = 43: (1, 1, 2), None, Unorm, false, Color, BGR5A1, PACKED16, TB_NO;
    // Ordinary 32-bit.
    /// 32-bit red, unsigned integer.
    R32Uint = 53: (1, 1, 4), None, Uint, false, Color, R32, INT_ATOMIC, TB_ALL;
    /// 32-bit red, signed integer.
    R32Sint = 54: (1, 1, 4), None, Sint, false, Color, R32, INT_ATOMIC, TB_ALL;
    /// 32-bit float red.
    R32Float = 55: (1, 1, 4), None, Float, false, Color, R32, F32, TB_ALL;
    /// 16-bit red/green.
    RG16Unorm = 60: (1, 1, 4), None, Unorm, false, Color, RG16, ALL, TB_RW;
    /// 16-bit red/green, signed normalized.
    RG16Snorm = 62: (1, 1, 4), None, Snorm, false, Color, RG16, ALL, TB_RW;
    /// 16-bit red/green, unsigned integer.
    RG16Uint = 63: (1, 1, 4), None, Uint, false, Color, RG16, INT, TB_RW;
    /// 16-bit red/green, signed integer.
    RG16Sint = 64: (1, 1, 4), None, Sint, false, Color, RG16, INT, TB_RW;
    /// 16-bit float red/green.
    RG16Float = 65: (1, 1, 4), None, Float, false, Color, RG16, ALL, TB_RW;
    /// 8-bit RGBA.
    RGBA8Unorm = 70: (1, 1, 4), None, Unorm, false, Color, RGBA8, ALL, TB_ALL;
    /// 8-bit RGBA, sRGB.
    RGBA8Unorm_sRGB = 71: (1, 1, 4), None, Unorm, true, Color, RGBA8, ALL, TB_NO;
    /// 8-bit RGBA, signed normalized.
    RGBA8Snorm = 72: (1, 1, 4), None, Snorm, false, Color, RGBA8, ALL, TB_RW;
    /// 8-bit RGBA, unsigned integer.
    RGBA8Uint = 73: (1, 1, 4), None, Uint, false, Color, RGBA8, INT, TB_ALL;
    /// 8-bit RGBA, signed integer.
    RGBA8Sint = 74: (1, 1, 4), None, Sint, false, Color, RGBA8, INT, TB_ALL;
    /// 8-bit BGRA (the usual drawable format).
    BGRA8Unorm = 80: (1, 1, 4), None, Unorm, false, Color, BGRA8, ALL, TB_R;
    /// 8-bit BGRA, sRGB.
    BGRA8Unorm_sRGB = 81: (1, 1, 4), None, Unorm, true, Color, BGRA8, ALL, TB_NO;
    // Packed 32-bit.
    /// 10:10:10:2 RGBA.
    RGB10A2Unorm = 90: (1, 1, 4), None, Unorm, false, Color, RGB10A2, ALL, TB_RW;
    /// 10:10:10:2 RGBA, unsigned integer.
    RGB10A2Uint = 91: (1, 1, 4), None, Uint, false, Color, RGB10A2, INT, TB_RW;
    /// Packed 11:11:10 float.
    RG11B10Float = 92: (1, 1, 4), None, Float, false, Color, RG11B10, ALL, TB_RW;
    /// Shared-exponent 9:9:9:5 float.
    RGB9E5Float = 93: (1, 1, 4), None, Float, false, Color, RGB9E5, ALL, TB_NO;
    /// 10:10:10:2 BGRA.
    BGR10A2Unorm = 94: (1, 1, 4), None, Unorm, false, Color, BGR10A2, ALL, TB_NO;
    /// Extended-range 10-bit BGR (32 bits per pixel).
    BGR10_XR = 554: (1, 1, 4), None, ExtendedRange, false, Color, BGR10Xr, ALL, TB_NO;
    /// Extended-range 10-bit BGR, sRGB.
    BGR10_XR_sRGB = 555: (1, 1, 4), None, ExtendedRange, true, Color, BGR10Xr, ALL, TB_NO;
    // Ordinary 64-bit.
    /// 32-bit red/green, unsigned integer.
    RG32Uint = 103: (1, 1, 8), None, Uint, false, Color, RG32, INT, TB_RW;
    /// 32-bit red/green, signed integer.
    RG32Sint = 104: (1, 1, 8), None, Sint, false, Color, RG32, INT, TB_RW;
    /// 32-bit float red/green.
    RG32Float = 105: (1, 1, 8), None, Float, false, Color, RG32, F32, TB_RW;
    /// 16-bit RGBA.
    RGBA16Unorm = 110: (1, 1, 8), None, Unorm, false, Color, RGBA16, ALL, TB_RW;
    /// 16-bit RGBA, signed normalized.
    RGBA16Snorm = 112: (1, 1, 8), None, Snorm, false, Color, RGBA16, ALL, TB_RW;
    /// 16-bit RGBA, unsigned integer.
    RGBA16Uint = 113: (1, 1, 8), None, Uint, false, Color, RGBA16, INT, TB_ALL;
    /// 16-bit RGBA, signed integer.
    RGBA16Sint = 114: (1, 1, 8), None, Sint, false, Color, RGBA16, INT, TB_ALL;
    /// 16-bit float RGBA (EDR/HDR surfaces).
    RGBA16Float = 115: (1, 1, 8), None, Float, false, Color, RGBA16, ALL, TB_ALL;
    /// Extended-range 10-bit BGRA (64 bits per pixel).
    BGRA10_XR = 552: (1, 1, 8), None, ExtendedRange, false, Color, BGRA10Xr, ALL, TB_NO;
    /// Extended-range 10-bit BGRA, sRGB.
    BGRA10_XR_sRGB = 553: (1, 1, 8), None, ExtendedRange, true, Color, BGRA10Xr, ALL, TB_NO;
    // Ordinary 128-bit.
    /// 32-bit RGBA, unsigned integer.
    RGBA32Uint = 123: (1, 1, 16), None, Uint, false, Color, RGBA32, INT, TB_ALL;
    /// 32-bit RGBA, signed integer.
    RGBA32Sint = 124: (1, 1, 16), None, Sint, false, Color, RGBA32, INT, TB_ALL;
    /// 32-bit float RGBA.
    RGBA32Float = 125: (1, 1, 16), None, Float, false, Color, RGBA32, F32, TB_ALL;
    // BC.
    /// BC1 (DXT1).
    BC1_RGBA = 130: (4, 4, 8), Bc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC1, sRGB.
    BC1_RGBA_sRGB = 131: (4, 4, 8), Bc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// BC2 (DXT3).
    BC2_RGBA = 132: (4, 4, 16), Bc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC2, sRGB.
    BC2_RGBA_sRGB = 133: (4, 4, 16), Bc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// BC3 (DXT5).
    BC3_RGBA = 134: (4, 4, 16), Bc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC3, sRGB.
    BC3_RGBA_sRGB = 135: (4, 4, 16), Bc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// BC4 one-channel.
    BC4_RUnorm = 140: (4, 4, 8), Bc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC4 one-channel, signed.
    BC4_RSnorm = 141: (4, 4, 8), Bc, Snorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC5 two-channel.
    BC5_RGUnorm = 142: (4, 4, 16), Bc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC5 two-channel, signed.
    BC5_RGSnorm = 143: (4, 4, 16), Bc, Snorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC6H signed float.
    BC6H_RGBFloat = 150: (4, 4, 16), Bc, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC6H unsigned float.
    BC6H_RGBUfloat = 151: (4, 4, 16), Bc, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC7.
    BC7_RGBAUnorm = 152: (4, 4, 16), Bc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// BC7, sRGB.
    BC7_RGBAUnorm_sRGB = 153: (4, 4, 16), Bc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    // PVRTC (2 bpp: 8×4 blocks, 4 bpp: 4×4 blocks, 64 bits each).
    /// PVRTC RGB 2 bpp.
    PVRTC_RGB_2BPP = 160: (8, 4, 8), Pvrtc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// PVRTC RGB 2 bpp, sRGB.
    PVRTC_RGB_2BPP_sRGB = 161: (8, 4, 8), Pvrtc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// PVRTC RGB 4 bpp.
    PVRTC_RGB_4BPP = 162: (4, 4, 8), Pvrtc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// PVRTC RGB 4 bpp, sRGB.
    PVRTC_RGB_4BPP_sRGB = 163: (4, 4, 8), Pvrtc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// PVRTC RGBA 2 bpp.
    PVRTC_RGBA_2BPP = 164: (8, 4, 8), Pvrtc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// PVRTC RGBA 2 bpp, sRGB.
    PVRTC_RGBA_2BPP_sRGB = 165: (8, 4, 8), Pvrtc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// PVRTC RGBA 4 bpp.
    PVRTC_RGBA_4BPP = 166: (4, 4, 8), Pvrtc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// PVRTC RGBA 4 bpp, sRGB.
    PVRTC_RGBA_4BPP_sRGB = 167: (4, 4, 8), Pvrtc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    // EAC/ETC2.
    /// EAC one-channel.
    EAC_R11Unorm = 170: (4, 4, 8), Etc2Eac, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// EAC one-channel, signed.
    EAC_R11Snorm = 172: (4, 4, 8), Etc2Eac, Snorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// EAC two-channel.
    EAC_RG11Unorm = 174: (4, 4, 16), Etc2Eac, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// EAC two-channel, signed.
    EAC_RG11Snorm = 176: (4, 4, 16), Etc2Eac, Snorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ETC2 RGB + EAC alpha.
    EAC_RGBA8 = 178: (4, 4, 16), Etc2Eac, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ETC2 RGB + EAC alpha, sRGB.
    EAC_RGBA8_sRGB = 179: (4, 4, 16), Etc2Eac, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ETC2 RGB.
    ETC2_RGB8 = 180: (4, 4, 8), Etc2Eac, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ETC2 RGB, sRGB.
    ETC2_RGB8_sRGB = 181: (4, 4, 8), Etc2Eac, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ETC2 RGB with 1-bit alpha.
    ETC2_RGB8A1 = 182: (4, 4, 8), Etc2Eac, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ETC2 RGB with 1-bit alpha, sRGB.
    ETC2_RGB8A1_sRGB = 183: (4, 4, 8), Etc2Eac, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    // ASTC sRGB (LDR).
    /// ASTC 4×4 sRGB.
    ASTC_4x4_sRGB = 186: (4, 4, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 5×4 sRGB.
    ASTC_5x4_sRGB = 187: (5, 4, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 5×5 sRGB.
    ASTC_5x5_sRGB = 188: (5, 5, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 6×5 sRGB.
    ASTC_6x5_sRGB = 189: (6, 5, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 6×6 sRGB.
    ASTC_6x6_sRGB = 190: (6, 6, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×5 sRGB.
    ASTC_8x5_sRGB = 192: (8, 5, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×6 sRGB.
    ASTC_8x6_sRGB = 193: (8, 6, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×8 sRGB.
    ASTC_8x8_sRGB = 194: (8, 8, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×5 sRGB.
    ASTC_10x5_sRGB = 195: (10, 5, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×6 sRGB.
    ASTC_10x6_sRGB = 196: (10, 6, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×8 sRGB.
    ASTC_10x8_sRGB = 197: (10, 8, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×10 sRGB.
    ASTC_10x10_sRGB = 198: (10, 10, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 12×10 sRGB.
    ASTC_12x10_sRGB = 199: (12, 10, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 12×12 sRGB.
    ASTC_12x12_sRGB = 200: (12, 12, 16), Astc, Unorm, true, Color, Unique, COMPRESSED, TB_NO;
    // ASTC LDR.
    /// ASTC 4×4 LDR.
    ASTC_4x4_LDR = 204: (4, 4, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 5×4 LDR.
    ASTC_5x4_LDR = 205: (5, 4, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 5×5 LDR.
    ASTC_5x5_LDR = 206: (5, 5, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 6×5 LDR.
    ASTC_6x5_LDR = 207: (6, 5, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 6×6 LDR.
    ASTC_6x6_LDR = 208: (6, 6, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×5 LDR.
    ASTC_8x5_LDR = 210: (8, 5, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×6 LDR.
    ASTC_8x6_LDR = 211: (8, 6, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×8 LDR.
    ASTC_8x8_LDR = 212: (8, 8, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×5 LDR.
    ASTC_10x5_LDR = 213: (10, 5, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×6 LDR.
    ASTC_10x6_LDR = 214: (10, 6, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×8 LDR.
    ASTC_10x8_LDR = 215: (10, 8, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×10 LDR.
    ASTC_10x10_LDR = 216: (10, 10, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 12×10 LDR.
    ASTC_12x10_LDR = 217: (12, 10, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 12×12 LDR.
    ASTC_12x12_LDR = 218: (12, 12, 16), Astc, Unorm, false, Color, Unique, COMPRESSED, TB_NO;
    // ASTC HDR.
    /// ASTC 4×4 HDR.
    ASTC_4x4_HDR = 222: (4, 4, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 5×4 HDR.
    ASTC_5x4_HDR = 223: (5, 4, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 5×5 HDR.
    ASTC_5x5_HDR = 224: (5, 5, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 6×5 HDR.
    ASTC_6x5_HDR = 225: (6, 5, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 6×6 HDR.
    ASTC_6x6_HDR = 226: (6, 6, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×5 HDR.
    ASTC_8x5_HDR = 228: (8, 5, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×6 HDR.
    ASTC_8x6_HDR = 229: (8, 6, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 8×8 HDR.
    ASTC_8x8_HDR = 230: (8, 8, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×5 HDR.
    ASTC_10x5_HDR = 231: (10, 5, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×6 HDR.
    ASTC_10x6_HDR = 232: (10, 6, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×8 HDR.
    ASTC_10x8_HDR = 233: (10, 8, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 10×10 HDR.
    ASTC_10x10_HDR = 234: (10, 10, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 12×10 HDR.
    ASTC_12x10_HDR = 235: (12, 10, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    /// ASTC 12×12 HDR.
    ASTC_12x12_HDR = 236: (12, 12, 16), AstcHdr, Float, false, Color, Unique, COMPRESSED, TB_NO;
    // 4:2:2 (one 32-bit block per 2×1 pixels).
    /// Y'CbCr 4:2:2, G-B-G-R order.
    GBGR422 = 240: (2, 1, 4), None, Unorm, false, Color, Unique, YUV, TB_NO;
    /// Y'CbCr 4:2:2, B-G-R-G order.
    BGRG422 = 241: (2, 1, 4), None, Unorm, false, Color, Unique, YUV, TB_NO;
    // Depth and stencil.
    /// 16-bit depth.
    Depth16Unorm = 250: (1, 1, 2), None, Unorm, false, Depth, Unique, D16, TB_NO;
    /// 32-bit float depth.
    Depth32Float = 252: (1, 1, 4), None, Float, false, Depth, Unique, D32, TB_NO;
    /// 8-bit stencil.
    Stencil8 = 253: (1, 1, 1), None, Uint, false, Stencil, Unique, S8, TB_NO;
    /// 24-bit depth + 8-bit stencil: Intel/AMD Macs only, not in any Apple family column.
    Depth24Unorm_Stencil8 = 255: (1, 1, 4), None, Unorm, false, DepthStencil, Unique, NONE, TB_NO;
    /// 32-bit float depth + 8-bit stencil; Apple: "some Metal device objects allocate 64 bits
    /// per pixel", so sizes are accounted with the worst case of 8 bytes.
    Depth32Float_Stencil8 = 260: (1, 1, 8), None, Float, false, DepthStencil, Unique, D32S8, TB_NO;
    /// Stencil view of `Depth32Float_Stencil8`.
    X32_Stencil8 = 261: (1, 1, 8), None, Uint, false, Stencil, Unique, X32S8, TB_NO;
    /// Stencil view of `Depth24Unorm_Stencil8` (Intel/AMD Macs only).
    X24_Stencil8 = 262: (1, 1, 4), None, Uint, false, Stencil, Unique, NONE, TB_NO;
}

/// Class of formats a texture view may reinterpret into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ViewClass {
    /// Uncompressed colour formats of this many bits per pixel.
    ColorBits(u32),
    /// `Depth32Float_Stencil8` and its stencil view.
    DepthStencil32,
    /// `Depth24Unorm_Stencil8` and its stencil view.
    DepthStencil24,
    /// Only the format itself (and its sRGB counterpart, handled separately).
    Exact,
}

impl PixelFormat {
    /// Block dimensions and bytes per block.
    #[must_use]
    pub const fn block_info(self) -> BlockInfo {
        self.row().block
    }

    /// Minimum number of blocks per dimension a mip level occupies (PVRTC1: 2 × 2).
    #[must_use]
    pub const fn min_blocks(self) -> (u32, u32) {
        match self.compression() {
            Compression::Pvrtc => (2, 2),
            _ => (1, 1),
        }
    }

    /// Compression family.
    #[must_use]
    pub const fn compression(self) -> Compression {
        self.row().compression
    }

    /// Whether the format is block-compressed.
    #[must_use]
    pub const fn is_compressed(self) -> bool {
        !matches!(self.compression(), Compression::None)
    }

    /// Whether the format is a subsampled 4:2:2 format (`GBGR422`, `BGRG422`).
    #[must_use]
    pub const fn is_422(self) -> bool {
        matches!(self, Self::GBGR422 | Self::BGRG422)
    }

    /// Numeric interpretation.
    #[must_use]
    pub const fn numeric_type(self) -> NumericType {
        self.row().numeric
    }

    /// Whether reads decode from and writes encode to sRGB.
    #[must_use]
    pub const fn is_srgb(self) -> bool {
        self.row().srgb
    }

    /// Aspects (colour, depth, stencil).
    #[must_use]
    pub const fn aspects(self) -> Aspects {
        self.row().aspects
    }

    /// Whether this is a depth and/or stencil format.
    #[must_use]
    pub const fn is_depth_stencil(self) -> bool {
        !matches!(self.aspects(), Aspects::Color)
    }

    /// Capabilities Metal guarantees for this format on every Apple7-or-later device (the
    /// floor of all Metal 3 / Metal 4 GPUs). Empty for formats no Apple-family GPU supports
    /// (`Depth24Unorm_Stencil8`, `X24_Stencil8`).
    #[must_use]
    pub const fn metal_caps(self) -> FormatCaps {
        self.row().caps
    }

    /// Whether textures may be created with this format (as opposed to views only).
    ///
    /// `X32_Stencil8` only exists as a view of `Depth32Float_Stencil8` [Annahme: Metal
    /// rejects it in a texture descriptor]; formats without Apple-family support are never
    /// creatable.
    #[must_use]
    pub const fn is_creatable(self) -> bool {
        !self.metal_caps().is_empty() && !matches!(self, Self::X32_Stencil8)
    }

    /// Access allowed for texture buffers of this format.
    #[must_use]
    pub const fn texture_buffer_access(self) -> TextureBufferAccess {
        self.row().texture_buffer
    }

    /// Bits per pixel of an uncompressed, non-subsampled format; `None` otherwise.
    #[must_use]
    pub const fn bits_per_pixel(self) -> Option<u32> {
        let b = self.block_info();
        if b.width == 1 && b.height == 1 {
            Some(b.bytes * 8)
        } else {
            None
        }
    }

    /// The sRGB form of a linear format or the linear form of an sRGB format.
    ///
    /// The SDK numbers every pair as `linear + 1 = sRGB`, except ASTC, where the LDR block is
    /// 18 above its sRGB block (`ASTC_4x4_sRGB` = 186, `ASTC_4x4_LDR` = 204). The candidate
    /// is only accepted if it has the same block and compression, so the rule cannot pair
    /// unrelated formats (checked for every format in the tests).
    #[must_use]
    pub const fn srgb_counterpart(self) -> Option<Self> {
        let raw = self.raw();
        let astc = matches!(self.compression(), Compression::Astc);
        let candidate = match (self.is_srgb(), astc) {
            (true, true) => raw + 18,
            (true, false) => raw - 1,
            (false, true) => raw - 18,
            (false, false) => raw + 1,
        };
        match Self::from_raw(candidate) {
            Ok(other)
                if other.is_srgb() != self.is_srgb()
                    && other.block_info().width == self.block_info().width
                    && other.block_info().height == self.block_info().height
                    && other.block_info().bytes == self.block_info().bytes
                    && other.compression() as u8 == self.compression() as u8
                    && other.layout_eq(self) =>
            {
                Some(other)
            }
            _ => None,
        }
    }

    const fn layout_eq(self, other: Self) -> bool {
        let (a, b) = (self.row().layout, other.row().layout);
        if matches!(a, Layout::Unique) || matches!(b, Layout::Unique) {
            // Unique layouts only match within an sRGB pair of the same compressed format;
            // the caller checks block and compression.
            matches!(a, Layout::Unique) && matches!(b, Layout::Unique)
        } else {
            a as u8 == b as u8
        }
    }

    const fn view_class(self) -> ViewClass {
        match self {
            Self::Depth32Float_Stencil8 | Self::X32_Stencil8 => ViewClass::DepthStencil32,
            Self::Depth24Unorm_Stencil8 | Self::X24_Stencil8 => ViewClass::DepthStencil24,
            _ => {
                if self.is_compressed() || self.is_422() || self.is_depth_stencil() {
                    ViewClass::Exact
                } else {
                    match self.bits_per_pixel() {
                        Some(bits) => ViewClass::ColorBits(bits),
                        None => ViewClass::Exact,
                    }
                }
            }
        }
    }

    /// Whether a texture of format `self` may be viewed as `view`
    /// (`makeTextureView(pixelFormat:)` compatibility groups):
    ///
    /// * uncompressed colour formats with the same bits per pixel,
    /// * sRGB and linear forms of the same format (also for compressed formats),
    /// * a combined depth/stencil format and its stencil view (`X32_Stencil8`, `X24_Stencil8`).
    ///
    /// 4:2:2 formats are only compatible with themselves [Annahme: Apple's list of colour
    /// formats does not mention them; conservative].
    #[must_use]
    pub fn view_compatible(self, view: Self) -> bool {
        if self == view || self.srgb_counterpart() == Some(view) {
            return true;
        }
        match (self.view_class(), view.view_class()) {
            (ViewClass::Exact, _) | (_, ViewClass::Exact) => false,
            (a, b) => a == b,
        }
    }

    /// Whether a view from `self` to `view` needs `MTLTextureUsagePixelFormatView` on the
    /// texture. Not needed for the same component layout (number, size, order of components),
    /// for linear↔sRGB, and for RGBA8↔BGRA8 linear↔sRGB, which Apple's documentation of the flag
    /// names as an example that must not set it. Stencil views of combined depth/stencil
    /// formats are treated as not needing it \[Annahme\].
    ///
    /// Only meaningful if [`PixelFormat::view_compatible`] holds.
    #[must_use]
    pub fn view_needs_pixel_format_view_usage(self, view: Self) -> bool {
        if self == view || self.srgb_counterpart() == Some(view) {
            return false;
        }
        if matches!(
            (self.view_class(), view.view_class()),
            (ViewClass::DepthStencil32, ViewClass::DepthStencil32)
                | (ViewClass::DepthStencil24, ViewClass::DepthStencil24)
        ) {
            return false;
        }
        let rgba_bgra = |f: Self| {
            matches!(
                f,
                Self::RGBA8Unorm | Self::RGBA8Unorm_sRGB | Self::BGRA8Unorm | Self::BGRA8Unorm_sRGB
            )
        };
        if rgba_bgra(self) && rgba_bgra(view) && self.is_srgb() != view.is_srgb() {
            return false;
        }
        let (a, b) = (self.row().layout, view.row().layout);
        matches!(a, Layout::Unique) || a != b
    }
}

/// Parses an optional pixel format where `MTLPixelFormatInvalid` (0) means "none"
/// (pipeline attachment formats, render-pass slots).
///
/// # Errors
///
/// As [`PixelFormat::from_raw`] for every non-zero value.
pub const fn optional_pixel_format(raw: u32) -> Result<Option<PixelFormat>, ModelError> {
    if raw == INVALID_RAW {
        return Ok(None);
    }
    match PixelFormat::from_raw(raw) {
        Ok(f) => Ok(Some(f)),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent restatement of the raw SDK values (`MTLPixelFormat.h`), so that a typo in
    /// the table macro cannot go unnoticed: every value, in SDK order.
    const SDK: &[(u32, &str)] = &[
        (1, "A8Unorm"),
        (10, "R8Unorm"),
        (11, "R8Unorm_sRGB"),
        (12, "R8Snorm"),
        (13, "R8Uint"),
        (14, "R8Sint"),
        (20, "R16Unorm"),
        (22, "R16Snorm"),
        (23, "R16Uint"),
        (24, "R16Sint"),
        (25, "R16Float"),
        (30, "RG8Unorm"),
        (31, "RG8Unorm_sRGB"),
        (32, "RG8Snorm"),
        (33, "RG8Uint"),
        (34, "RG8Sint"),
        (40, "B5G6R5Unorm"),
        (41, "A1BGR5Unorm"),
        (42, "ABGR4Unorm"),
        (43, "BGR5A1Unorm"),
        (53, "R32Uint"),
        (54, "R32Sint"),
        (55, "R32Float"),
        (60, "RG16Unorm"),
        (62, "RG16Snorm"),
        (63, "RG16Uint"),
        (64, "RG16Sint"),
        (65, "RG16Float"),
        (70, "RGBA8Unorm"),
        (71, "RGBA8Unorm_sRGB"),
        (72, "RGBA8Snorm"),
        (73, "RGBA8Uint"),
        (74, "RGBA8Sint"),
        (80, "BGRA8Unorm"),
        (81, "BGRA8Unorm_sRGB"),
        (90, "RGB10A2Unorm"),
        (91, "RGB10A2Uint"),
        (92, "RG11B10Float"),
        (93, "RGB9E5Float"),
        (94, "BGR10A2Unorm"),
        (554, "BGR10_XR"),
        (555, "BGR10_XR_sRGB"),
        (103, "RG32Uint"),
        (104, "RG32Sint"),
        (105, "RG32Float"),
        (110, "RGBA16Unorm"),
        (112, "RGBA16Snorm"),
        (113, "RGBA16Uint"),
        (114, "RGBA16Sint"),
        (115, "RGBA16Float"),
        (552, "BGRA10_XR"),
        (553, "BGRA10_XR_sRGB"),
        (123, "RGBA32Uint"),
        (124, "RGBA32Sint"),
        (125, "RGBA32Float"),
        (130, "BC1_RGBA"),
        (131, "BC1_RGBA_sRGB"),
        (132, "BC2_RGBA"),
        (133, "BC2_RGBA_sRGB"),
        (134, "BC3_RGBA"),
        (135, "BC3_RGBA_sRGB"),
        (140, "BC4_RUnorm"),
        (141, "BC4_RSnorm"),
        (142, "BC5_RGUnorm"),
        (143, "BC5_RGSnorm"),
        (150, "BC6H_RGBFloat"),
        (151, "BC6H_RGBUfloat"),
        (152, "BC7_RGBAUnorm"),
        (153, "BC7_RGBAUnorm_sRGB"),
        (160, "PVRTC_RGB_2BPP"),
        (161, "PVRTC_RGB_2BPP_sRGB"),
        (162, "PVRTC_RGB_4BPP"),
        (163, "PVRTC_RGB_4BPP_sRGB"),
        (164, "PVRTC_RGBA_2BPP"),
        (165, "PVRTC_RGBA_2BPP_sRGB"),
        (166, "PVRTC_RGBA_4BPP"),
        (167, "PVRTC_RGBA_4BPP_sRGB"),
        (170, "EAC_R11Unorm"),
        (172, "EAC_R11Snorm"),
        (174, "EAC_RG11Unorm"),
        (176, "EAC_RG11Snorm"),
        (178, "EAC_RGBA8"),
        (179, "EAC_RGBA8_sRGB"),
        (180, "ETC2_RGB8"),
        (181, "ETC2_RGB8_sRGB"),
        (182, "ETC2_RGB8A1"),
        (183, "ETC2_RGB8A1_sRGB"),
        (186, "ASTC_4x4_sRGB"),
        (187, "ASTC_5x4_sRGB"),
        (188, "ASTC_5x5_sRGB"),
        (189, "ASTC_6x5_sRGB"),
        (190, "ASTC_6x6_sRGB"),
        (192, "ASTC_8x5_sRGB"),
        (193, "ASTC_8x6_sRGB"),
        (194, "ASTC_8x8_sRGB"),
        (195, "ASTC_10x5_sRGB"),
        (196, "ASTC_10x6_sRGB"),
        (197, "ASTC_10x8_sRGB"),
        (198, "ASTC_10x10_sRGB"),
        (199, "ASTC_12x10_sRGB"),
        (200, "ASTC_12x12_sRGB"),
        (204, "ASTC_4x4_LDR"),
        (205, "ASTC_5x4_LDR"),
        (206, "ASTC_5x5_LDR"),
        (207, "ASTC_6x5_LDR"),
        (208, "ASTC_6x6_LDR"),
        (210, "ASTC_8x5_LDR"),
        (211, "ASTC_8x6_LDR"),
        (212, "ASTC_8x8_LDR"),
        (213, "ASTC_10x5_LDR"),
        (214, "ASTC_10x6_LDR"),
        (215, "ASTC_10x8_LDR"),
        (216, "ASTC_10x10_LDR"),
        (217, "ASTC_12x10_LDR"),
        (218, "ASTC_12x12_LDR"),
        (222, "ASTC_4x4_HDR"),
        (223, "ASTC_5x4_HDR"),
        (224, "ASTC_5x5_HDR"),
        (225, "ASTC_6x5_HDR"),
        (226, "ASTC_6x6_HDR"),
        (228, "ASTC_8x5_HDR"),
        (229, "ASTC_8x6_HDR"),
        (230, "ASTC_8x8_HDR"),
        (231, "ASTC_10x5_HDR"),
        (232, "ASTC_10x6_HDR"),
        (233, "ASTC_10x8_HDR"),
        (234, "ASTC_10x10_HDR"),
        (235, "ASTC_12x10_HDR"),
        (236, "ASTC_12x12_HDR"),
        (240, "GBGR422"),
        (241, "BGRG422"),
        (250, "Depth16Unorm"),
        (252, "Depth32Float"),
        (253, "Stencil8"),
        (255, "Depth24Unorm_Stencil8"),
        (260, "Depth32Float_Stencil8"),
        (261, "X32_Stencil8"),
        (262, "X24_Stencil8"),
    ];

    #[test]
    fn table_matches_sdk_values_exactly() {
        assert_eq!(PixelFormat::COUNT, SDK.len());
        for (raw, name) in SDK {
            let f = PixelFormat::from_raw(*raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(format!("{f:?}"), *name, "raw {raw}");
            assert_eq!(f.raw(), *raw);
        }
        for raw in 0..=1024 {
            let known = SDK.iter().any(|(r, _)| *r == raw);
            match PixelFormat::from_raw(raw) {
                Ok(_) => assert!(known, "{raw} accepted but not in SDK"),
                Err(ModelError::Unsupported(_)) => assert_eq!(raw, UNSPECIALIZED_RAW),
                Err(e) => {
                    assert!(!known, "{raw}: {e}");
                    assert_eq!(e, ModelError::UnknownPixelFormat(raw));
                }
            }
        }
        assert_eq!(optional_pixel_format(0), Ok(None));
        assert_eq!(optional_pixel_format(80), Ok(Some(PixelFormat::BGRA8Unorm)));
        assert!(optional_pixel_format(263).is_err());
    }

    #[test]
    fn indices_are_dense_and_unique() {
        let mut seen = [false; PixelFormat::COUNT];
        for (i, f) in PixelFormat::ALL.iter().enumerate() {
            assert_eq!(f.index(), i);
            assert!(!seen[i]);
            seen[i] = true;
        }
    }

    /// Format → block size table (bytes per block, block extent).
    #[test]
    fn block_sizes() {
        use PixelFormat as P;
        let cases: &[(P, u32, u32, u32)] = &[
            (P::A8Unorm, 1, 1, 1),
            (P::R8Uint, 1, 1, 1),
            (P::R16Float, 1, 1, 2),
            (P::B5G6R5Unorm, 1, 1, 2),
            (P::RGBA8Unorm_sRGB, 1, 1, 4),
            (P::RGB9E5Float, 1, 1, 4),
            (P::BGR10_XR, 1, 1, 4),
            (P::BGRA10_XR_sRGB, 1, 1, 8),
            (P::RGBA16Float, 1, 1, 8),
            (P::RGBA32Float, 1, 1, 16),
            (P::BC1_RGBA, 4, 4, 8),
            (P::BC4_RSnorm, 4, 4, 8),
            (P::BC6H_RGBUfloat, 4, 4, 16),
            (P::PVRTC_RGBA_2BPP, 8, 4, 8),
            (P::PVRTC_RGB_4BPP_sRGB, 4, 4, 8),
            (P::EAC_R11Unorm, 4, 4, 8),
            (P::EAC_RG11Snorm, 4, 4, 16),
            (P::ETC2_RGB8A1, 4, 4, 8),
            (P::ASTC_5x4_LDR, 5, 4, 16),
            (P::ASTC_10x8_sRGB, 10, 8, 16),
            (P::ASTC_12x12_HDR, 12, 12, 16),
            (P::GBGR422, 2, 1, 4),
            (P::Depth16Unorm, 1, 1, 2),
            (P::Stencil8, 1, 1, 1),
            (P::Depth32Float_Stencil8, 1, 1, 8),
        ];
        for &(f, w, h, bytes) in cases {
            assert_eq!(
                f.block_info(),
                BlockInfo {
                    width: w,
                    height: h,
                    bytes
                },
                "{f:?}"
            );
        }
        for f in PixelFormat::ALL {
            let b = f.block_info();
            assert!(b.width >= 1 && b.height >= 1 && b.bytes >= 1, "{f:?}");
            // Only compressed and 4:2:2 formats have blocks larger than a pixel.
            assert_eq!(f.is_compressed() || f.is_422(), b.width > 1, "{f:?}");
            if f.is_compressed() {
                assert!(matches!(b.bytes, 8 | 16), "{f:?}");
            }
            // ASTC block extents come from the name (`ASTC_<w>x<h>_…`).
            if matches!(f.compression(), Compression::Astc | Compression::AstcHdr) {
                let name = format!("{f:?}");
                let dims = name.trim_start_matches("ASTC_").split('_').next().unwrap();
                let (w, h) = dims.split_once('x').unwrap();
                assert_eq!(b.width, w.parse::<u32>().unwrap(), "{f:?}");
                assert_eq!(b.height, h.parse::<u32>().unwrap(), "{f:?}");
            }
        }
        assert_eq!(PixelFormat::PVRTC_RGB_2BPP.min_blocks(), (2, 2));
        assert_eq!(PixelFormat::BC1_RGBA.min_blocks(), (1, 1));
    }

    /// Format → capability table: the Apple7 column of the Metal Feature Set Tables.
    #[test]
    fn metal_capabilities_apple7_column() {
        use PixelFormat as P;
        let c = |f: P| f.metal_caps();
        let all = [
            FormatCaps::FILTER,
            FormatCaps::WRITE,
            FormatCaps::COLOR,
            FormatCaps::BLEND,
            FormatCaps::MSAA,
            FormatCaps::RESOLVE,
            FormatCaps::SPARSE,
        ]
        .into_iter()
        .fold(FormatCaps::SAMPLE, FormatCaps::union);
        for f in [
            P::A8Unorm,
            P::R8Unorm_sRGB,
            P::RG16Snorm,
            P::RGBA8Unorm,
            P::BGRA8Unorm_sRGB,
            P::RGB9E5Float,
            P::BGR10_XR_sRGB,
            P::RGBA16Float,
        ] {
            assert_eq!(c(f), all, "{f:?}");
        }
        // Integer formats: no filter, no blend, no resolve.
        for f in [P::R8Uint, P::RG16Sint, P::RGB10A2Uint, P::RGBA32Uint] {
            assert!(c(f).contains(FormatCaps::WRITE.union(FormatCaps::COLOR)));
            assert!(
                !c(f).intersects(
                    FormatCaps::FILTER
                        .union(FormatCaps::BLEND)
                        .union(FormatCaps::RESOLVE)
                )
            );
        }
        // Atomics only on R32Uint/R32Sint at Apple7 (RG32 needs Apple8).
        let atomic: Vec<_> = PixelFormat::ALL
            .iter()
            .filter(|f| c(**f).contains(FormatCaps::ATOMIC))
            .collect();
        assert_eq!(atomic, [&P::R32Uint, &P::R32Sint]);
        // 32-bit floats: blendable but not filterable on Apple7.
        for f in [P::R32Float, P::RG32Float, P::RGBA32Float] {
            assert!(c(f).contains(FormatCaps::BLEND.union(FormatCaps::MSAA)));
            assert!(!c(f).intersects(FormatCaps::FILTER.union(FormatCaps::RESOLVE)));
        }
        // Packed 16-bit: renderable, not writable.
        for f in [
            P::B5G6R5Unorm,
            P::A1BGR5Unorm,
            P::ABGR4Unorm,
            P::BGR5A1Unorm,
        ] {
            assert!(c(f).contains(FormatCaps::COLOR.union(FormatCaps::RESOLVE)));
            assert!(!c(f).contains(FormatCaps::WRITE));
        }
        // Compressed: sample + filter only, never render targets.
        for f in PixelFormat::ALL.iter().filter(|f| f.is_compressed()) {
            assert_eq!(c(*f), COMPRESSED, "{f:?}");
        }
        assert_eq!(c(P::GBGR422), YUV);
        // Depth/stencil.
        assert!(c(P::Depth16Unorm).contains(FormatCaps::FILTER.union(FormatCaps::DEPTH)));
        assert!(!c(P::Depth32Float).contains(FormatCaps::FILTER));
        assert!(c(P::Depth32Float_Stencil8).contains(FormatCaps::DEPTH.union(FormatCaps::STENCIL)));
        assert!(c(P::Stencil8).contains(FormatCaps::STENCIL));
        assert!(!c(P::X32_Stencil8).intersects(FormatCaps::DEPTH.union(FormatCaps::STENCIL)));
        // Not on Apple GPUs at all.
        assert!(c(P::Depth24Unorm_Stencil8).is_empty());
        assert!(c(P::X24_Stencil8).is_empty());
        // Every colour-renderable format is a colour format; depth caps only on depth formats.
        for f in PixelFormat::ALL {
            let caps = c(*f);
            if caps.contains(FormatCaps::COLOR) {
                assert_eq!(f.aspects(), Aspects::Color, "{f:?}");
            }
            assert_eq!(
                caps.contains(FormatCaps::DEPTH),
                f.aspects().has_depth() && f.is_creatable(),
                "{f:?}"
            );
            if caps.contains(FormatCaps::BLEND) || caps.contains(FormatCaps::FILTER) {
                assert!(!f.numeric_type().is_integer(), "{f:?}");
            }
            if !caps.is_empty() {
                assert!(caps.contains(FormatCaps::SAMPLE), "{f:?}");
            }
        }
        assert!(!P::X32_Stencil8.is_creatable());
        assert!(P::Stencil8.is_creatable());
    }

    #[test]
    fn texture_buffer_access_table() {
        use PixelFormat as P;
        use TextureBufferAccess as A;
        for (f, a) in [
            (P::A8Unorm, A::All),
            (P::R8Snorm, A::ReadWrite),
            (P::R16Unorm, A::ReadWrite),
            (P::R16Uint, A::All),
            (P::RG8Uint, A::ReadWrite),
            (P::R32Float, A::All),
            (P::RG16Float, A::ReadWrite),
            (P::RGBA8Unorm, A::All),
            (P::RGBA8Snorm, A::ReadWrite),
            (P::BGRA8Unorm, A::Read),
            (P::RG11B10Float, A::ReadWrite),
            (P::RG32Float, A::ReadWrite),
            (P::RGBA16Float, A::All),
            (P::RGBA32Sint, A::All),
            (P::RGBA8Unorm_sRGB, A::None),
            (P::BGR10A2Unorm, A::None),
            (P::RGB9E5Float, A::None),
            (P::BC1_RGBA, A::None),
            (P::Depth32Float, A::None),
        ] {
            assert_eq!(f.texture_buffer_access(), a, "{f:?}");
        }
    }

    #[test]
    fn srgb_pairs() {
        use PixelFormat as P;
        for (lin, srgb) in [
            (P::R8Unorm, P::R8Unorm_sRGB),
            (P::RG8Unorm, P::RG8Unorm_sRGB),
            (P::RGBA8Unorm, P::RGBA8Unorm_sRGB),
            (P::BGRA8Unorm, P::BGRA8Unorm_sRGB),
            (P::BGR10_XR, P::BGR10_XR_sRGB),
            (P::BGRA10_XR, P::BGRA10_XR_sRGB),
            (P::BC1_RGBA, P::BC1_RGBA_sRGB),
            (P::BC7_RGBAUnorm, P::BC7_RGBAUnorm_sRGB),
            (P::PVRTC_RGBA_2BPP, P::PVRTC_RGBA_2BPP_sRGB),
            (P::EAC_RGBA8, P::EAC_RGBA8_sRGB),
            (P::ETC2_RGB8A1, P::ETC2_RGB8A1_sRGB),
            (P::ASTC_4x4_LDR, P::ASTC_4x4_sRGB),
            (P::ASTC_12x12_LDR, P::ASTC_12x12_sRGB),
        ] {
            assert_eq!(lin.srgb_counterpart(), Some(srgb), "{lin:?}");
            assert_eq!(srgb.srgb_counterpart(), Some(lin), "{srgb:?}");
        }
        for f in PixelFormat::ALL {
            if let Some(o) = f.srgb_counterpart() {
                assert_eq!(o.srgb_counterpart(), Some(*f), "symmetric: {f:?}");
                assert_ne!(o.is_srgb(), f.is_srgb());
            }
            if f.is_srgb() {
                assert!(
                    f.srgb_counterpart().is_some(),
                    "every sRGB format has a pair: {f:?}"
                );
            }
        }
        for f in [P::R8Snorm, P::BC4_RUnorm, P::ASTC_4x4_HDR, P::Depth32Float] {
            assert_eq!(f.srgb_counterpart(), None, "{f:?}");
        }
    }

    #[test]
    fn view_compatibility_classes() {
        use PixelFormat as P;
        // Same bit length colour formats.
        assert!(P::RGBA8Unorm.view_compatible(P::R32Uint));
        assert!(P::RG16Uint.view_compatible(P::R32Uint));
        assert!(P::BGR10A2Unorm.view_compatible(P::RG11B10Float));
        assert!(P::RGBA16Float.view_compatible(P::RG32Uint));
        assert!(P::R8Unorm.view_compatible(P::A8Unorm));
        assert!(!P::RGBA8Unorm.view_compatible(P::RG8Unorm));
        assert!(!P::RGBA8Unorm.view_compatible(P::Depth32Float));
        // Compressed: only the sRGB counterpart.
        assert!(P::BC1_RGBA.view_compatible(P::BC1_RGBA_sRGB));
        assert!(!P::BC1_RGBA.view_compatible(P::BC4_RUnorm));
        assert!(!P::BC1_RGBA.view_compatible(P::RG32Uint));
        assert!(!P::ASTC_4x4_LDR.view_compatible(P::ASTC_4x4_HDR));
        // Depth/stencil and their stencil views.
        assert!(P::Depth32Float_Stencil8.view_compatible(P::X32_Stencil8));
        assert!(!P::Depth32Float_Stencil8.view_compatible(P::X24_Stencil8));
        assert!(!P::Depth32Float.view_compatible(P::R32Float));
        assert!(!P::GBGR422.view_compatible(P::RG16Unorm));
        // Usage flag requirement.
        assert!(!P::RGBA8Unorm.view_needs_pixel_format_view_usage(P::RGBA8Unorm_sRGB));
        assert!(!P::RGBA8Unorm.view_needs_pixel_format_view_usage(P::RGBA8Uint));
        assert!(!P::RGBA8Unorm.view_needs_pixel_format_view_usage(P::BGRA8Unorm_sRGB));
        assert!(P::RGBA8Unorm.view_needs_pixel_format_view_usage(P::BGRA8Unorm));
        assert!(P::RGBA8Unorm.view_needs_pixel_format_view_usage(P::R32Uint));
        assert!(!P::BC7_RGBAUnorm.view_needs_pixel_format_view_usage(P::BC7_RGBAUnorm_sRGB));
        assert!(!P::Depth32Float_Stencil8.view_needs_pixel_format_view_usage(P::X32_Stencil8));
        // Compatibility is symmetric.
        for a in PixelFormat::ALL {
            for b in PixelFormat::ALL {
                assert_eq!(a.view_compatible(*b), b.view_compatible(*a), "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn format_caps_debug_lists_names() {
        let c = FormatCaps::SAMPLE.union(FormatCaps::ATOMIC);
        assert_eq!(format!("{c:?}"), "{sample, atomic}");
        assert_eq!(c.difference(FormatCaps::SAMPLE), FormatCaps::ATOMIC);
        assert!(FormatCaps::NONE.is_empty());
    }
}
