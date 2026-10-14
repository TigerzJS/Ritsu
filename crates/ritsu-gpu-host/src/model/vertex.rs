//! `MTLVertexDescriptor`: attribute formats, buffer layouts, step functions.
//!
//! Raw values: public SDK header `MTLVertexDescriptor.h` (`MTLVertexFormat`,
//! `MTLVertexStepFunction`, `MTLBufferLayoutStrideDynamic` = `NSUIntegerMax`). Rules: Apple
//! documentation of `MTLVertexBufferLayoutDescriptor` (`perVertex` needs `stepRate` 1,
//! `perInstance` needs `stepRate` > 0, `constant` needs `stepRate` 0) and the Metal Feature Set
//! Tables, column Apple7: 31 attributes, 31 buffer argument table entries, minimum alignment of
//! strides and attribute offsets **1 byte**, no maximum stride, dynamic strides supported,
//! `floatRG11B10`/`floatRGB9E5` supported.
//!
//! Metal is more permissive than core Vulkan here (unaligned offsets and strides, arbitrary
//! strides, instance step rates). These gaps are checked against the device in
//! [`crate::caps::vertex_input_support`], not here: the descriptor only encodes what Metal
//! allows.

use super::{ModelError, NumericType, ResourceLimits, raw_enum};

/// Number of attribute slots of a vertex descriptor (Apple7: 31).
pub const MAX_VERTEX_ATTRIBUTES: usize = 31;
/// Number of buffer layout slots of a vertex descriptor (Apple7 buffer argument table: 31).
pub const MAX_VERTEX_BUFFER_LAYOUTS: usize = 31;
/// `MTLBufferLayoutStrideDynamic` (`NSUIntegerMax` on arm64).
pub const BUFFER_LAYOUT_STRIDE_DYNAMIC: u64 = u64::MAX;
/// Raw `MTLVertexFormatInvalid` (attribute slot unused).
pub const VERTEX_FORMAT_INVALID_RAW: u32 = 0;

macro_rules! vertex_formats {
    ($( $(#[$m:meta])* $name:ident = $val:literal : ($size:literal, $comps:literal, $align:literal, $num:ident), )*) => {
        raw_enum! {
            /// `MTLVertexFormat` (every value of the public SDK enum except `Invalid` = 0).
            #[allow(non_camel_case_types)] // keep Metal's spelling (UChar4Normalized_BGRA)
            VertexFormat, UnknownVertexFormat {
                $( $(#[$m])* $name = $val, )*
            }
        }

        impl VertexFormat {
            /// Size of one attribute value in bytes.
            #[must_use]
            pub const fn size(self) -> u32 {
                match self { $( Self::$name => $size, )* }
            }

            /// Number of components.
            #[must_use]
            pub const fn components(self) -> u32 {
                match self { $( Self::$name => $comps, )* }
            }

            /// Natural fetch alignment: component size, or the whole value for packed formats.
            /// Metal (Apple7) does not require it; core Vulkan does.
            #[must_use]
            pub const fn natural_alignment(self) -> u32 {
                match self { $( Self::$name => $align, )* }
            }

            /// How the vertex shader sees the value (normalized formats arrive as floats).
            #[must_use]
            pub const fn numeric_type(self) -> NumericType {
                match self { $( Self::$name => NumericType::$num, )* }
            }
        }
    };
}

vertex_formats! {
    /// 2 × u8.
    UChar2 = 1: (2, 2, 1, Uint),
    /// 3 × u8.
    UChar3 = 2: (3, 3, 1, Uint),
    /// 4 × u8.
    UChar4 = 3: (4, 4, 1, Uint),
    /// 2 × i8.
    Char2 = 4: (2, 2, 1, Sint),
    /// 3 × i8.
    Char3 = 5: (3, 3, 1, Sint),
    /// 4 × i8.
    Char4 = 6: (4, 4, 1, Sint),
    /// 2 × u8 normalized.
    UChar2Normalized = 7: (2, 2, 1, Unorm),
    /// 3 × u8 normalized.
    UChar3Normalized = 8: (3, 3, 1, Unorm),
    /// 4 × u8 normalized.
    UChar4Normalized = 9: (4, 4, 1, Unorm),
    /// 2 × i8 normalized.
    Char2Normalized = 10: (2, 2, 1, Snorm),
    /// 3 × i8 normalized.
    Char3Normalized = 11: (3, 3, 1, Snorm),
    /// 4 × i8 normalized.
    Char4Normalized = 12: (4, 4, 1, Snorm),
    /// 2 × u16.
    UShort2 = 13: (4, 2, 2, Uint),
    /// 3 × u16.
    UShort3 = 14: (6, 3, 2, Uint),
    /// 4 × u16.
    UShort4 = 15: (8, 4, 2, Uint),
    /// 2 × i16.
    Short2 = 16: (4, 2, 2, Sint),
    /// 3 × i16.
    Short3 = 17: (6, 3, 2, Sint),
    /// 4 × i16.
    Short4 = 18: (8, 4, 2, Sint),
    /// 2 × u16 normalized.
    UShort2Normalized = 19: (4, 2, 2, Unorm),
    /// 3 × u16 normalized.
    UShort3Normalized = 20: (6, 3, 2, Unorm),
    /// 4 × u16 normalized.
    UShort4Normalized = 21: (8, 4, 2, Unorm),
    /// 2 × i16 normalized.
    Short2Normalized = 22: (4, 2, 2, Snorm),
    /// 3 × i16 normalized.
    Short3Normalized = 23: (6, 3, 2, Snorm),
    /// 4 × i16 normalized.
    Short4Normalized = 24: (8, 4, 2, Snorm),
    /// 2 × f16.
    Half2 = 25: (4, 2, 2, Float),
    /// 3 × f16.
    Half3 = 26: (6, 3, 2, Float),
    /// 4 × f16.
    Half4 = 27: (8, 4, 2, Float),
    /// f32.
    Float = 28: (4, 1, 4, Float),
    /// 2 × f32.
    Float2 = 29: (8, 2, 4, Float),
    /// 3 × f32.
    Float3 = 30: (12, 3, 4, Float),
    /// 4 × f32.
    Float4 = 31: (16, 4, 4, Float),
    /// i32.
    Int = 32: (4, 1, 4, Sint),
    /// 2 × i32.
    Int2 = 33: (8, 2, 4, Sint),
    /// 3 × i32.
    Int3 = 34: (12, 3, 4, Sint),
    /// 4 × i32.
    Int4 = 35: (16, 4, 4, Sint),
    /// u32.
    UInt = 36: (4, 1, 4, Uint),
    /// 2 × u32.
    UInt2 = 37: (8, 2, 4, Uint),
    /// 3 × u32.
    UInt3 = 38: (12, 3, 4, Uint),
    /// 4 × u32.
    UInt4 = 39: (16, 4, 4, Uint),
    /// Packed 10:10:10:2 signed normalized.
    Int1010102Normalized = 40: (4, 4, 4, Snorm),
    /// Packed 10:10:10:2 unsigned normalized.
    UInt1010102Normalized = 41: (4, 4, 4, Unorm),
    /// 4 × u8 normalized, stored BGRA, delivered RGBA.
    UChar4Normalized_BGRA = 42: (4, 4, 1, Unorm),
    /// u8.
    UChar = 45: (1, 1, 1, Uint),
    /// i8.
    Char = 46: (1, 1, 1, Sint),
    /// u8 normalized.
    UCharNormalized = 47: (1, 1, 1, Unorm),
    /// i8 normalized.
    CharNormalized = 48: (1, 1, 1, Snorm),
    /// u16.
    UShort = 49: (2, 1, 2, Uint),
    /// i16.
    Short = 50: (2, 1, 2, Sint),
    /// u16 normalized.
    UShortNormalized = 51: (2, 1, 2, Unorm),
    /// i16 normalized.
    ShortNormalized = 52: (2, 1, 2, Snorm),
    /// f16.
    Half = 53: (2, 1, 2, Float),
    /// Packed 11:11:10 float.
    FloatRG11B10 = 54: (4, 3, 4, Float),
    /// Shared-exponent 9:9:9:5 float.
    FloatRGB9E5 = 55: (4, 3, 4, Float),
}

/// Parses an optional vertex format where `MTLVertexFormatInvalid` (0) means "slot unused".
///
/// # Errors
///
/// [`ModelError::UnknownVertexFormat`] for unknown non-zero values.
pub const fn optional_vertex_format(raw: u32) -> Result<Option<VertexFormat>, ModelError> {
    if raw == VERTEX_FORMAT_INVALID_RAW {
        return Ok(None);
    }
    match VertexFormat::from_raw(raw) {
        Ok(f) => Ok(Some(f)),
        Err(e) => Err(e),
    }
}

raw_enum! {
    /// `MTLVertexStepFunction`.
    VertexStepFunction, UnknownVertexStepFunction {
        /// Fetched once for all vertices (`stepRate` must be 0).
        Constant = 0,
        /// Fetched per vertex (`stepRate` must be 1).
        PerVertex = 1,
        /// Fetched every `stepRate` instances (`stepRate` > 0).
        PerInstance = 2,
        /// Per tessellation patch (post-tessellation vertex functions only).
        PerPatch = 3,
        /// Per patch control point (post-tessellation vertex functions only).
        PerPatchControlPoint = 4,
    }
}

/// Stride of a buffer layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VertexStride {
    /// Fixed number of bytes between vertices.
    Fixed(u64),
    /// Given at bind time (`setVertexBuffer:offset:attributeStride:atIndex:`).
    Dynamic,
}

impl VertexStride {
    /// From the raw SDK value ([`BUFFER_LAYOUT_STRIDE_DYNAMIC`] = dynamic).
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        if raw == BUFFER_LAYOUT_STRIDE_DYNAMIC {
            Self::Dynamic
        } else {
            Self::Fixed(raw)
        }
    }
}

/// `MTLVertexBufferLayoutDescriptor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VertexBufferLayoutDesc {
    /// Stride.
    pub stride: VertexStride,
    /// Step function.
    pub step_function: VertexStepFunction,
    /// Step rate (`NSUInteger`).
    pub step_rate: u64,
}

impl Default for VertexBufferLayoutDesc {
    /// SDK defaults: stride 0, per vertex, step rate 1.
    fn default() -> Self {
        Self {
            stride: VertexStride::Fixed(0),
            step_function: VertexStepFunction::PerVertex,
            step_rate: 1,
        }
    }
}

/// `MTLVertexAttributeDescriptor` of a used slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VertexAttributeDesc {
    /// Format.
    pub format: VertexFormat,
    /// Byte offset within a vertex.
    pub offset: u64,
    /// Buffer layout index.
    pub buffer_index: u32,
}

/// `MTLVertexDescriptor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VertexDescriptor {
    /// Attributes by index; `None` = `MTLVertexFormatInvalid`.
    pub attributes: [Option<VertexAttributeDesc>; MAX_VERTEX_ATTRIBUTES],
    /// Buffer layouts by buffer index.
    pub layouts: [VertexBufferLayoutDesc; MAX_VERTEX_BUFFER_LAYOUTS],
}

impl Default for VertexDescriptor {
    fn default() -> Self {
        Self {
            attributes: [None; MAX_VERTEX_ATTRIBUTES],
            layouts: [VertexBufferLayoutDesc::default(); MAX_VERTEX_BUFFER_LAYOUTS],
        }
    }
}

impl VertexDescriptor {
    /// Used attributes with their index.
    pub fn used_attributes(&self) -> impl Iterator<Item = (usize, &VertexAttributeDesc)> {
        self.attributes
            .iter()
            .enumerate()
            .filter_map(|(i, a)| a.as_ref().map(|a| (i, a)))
    }

    /// Layout of an attribute's buffer, `None` if the index is out of range.
    #[must_use]
    pub fn layout_of(&self, attribute: &VertexAttributeDesc) -> Option<&VertexBufferLayoutDesc> {
        usize::try_from(attribute.buffer_index)
            .ok()
            .and_then(|i| self.layouts.get(i))
    }

    /// Validates the descriptor against Metal's rules and the limits the guest knows (pass
    /// `ReportedFeatures::guest_limits`: Apple7's 31 attributes and buffers). Device limits
    /// below that are a capability gap, checked by [`crate::caps::vertex_input_support`].
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidValue`] for out-of-range indices, step-rate violations and
    /// overflowing offsets; [`ModelError::Unsupported`] for the tessellation step functions.
    pub fn validate(&self, limits: &ResourceLimits) -> Result<(), ModelError> {
        let invalid = |field, reason| Err(ModelError::InvalidValue { field, reason });
        for (i, a) in self.used_attributes() {
            if i as u64 >= u64::from(limits.max_vertex_attributes) {
                return invalid("vertex attribute index", "exceeds the attribute limit");
            }
            if a.buffer_index >= limits.max_vertex_buffers {
                return invalid("vertex attribute bufferIndex", "exceeds the buffer limit");
            }
            let Some(layout) = self.layout_of(a) else {
                return invalid("vertex attribute bufferIndex", "exceeds the layout table");
            };
            if a.offset.checked_add(u64::from(a.format.size())).is_none() {
                return invalid("vertex attribute offset", "offset + size overflows");
            }
            match (layout.step_function, layout.step_rate) {
                (VertexStepFunction::PerVertex, 1) | (VertexStepFunction::Constant, 0) => {}
                (VertexStepFunction::PerInstance, r) if r > 0 => {}
                (VertexStepFunction::PerPatch | VertexStepFunction::PerPatchControlPoint, _) => {
                    return Err(ModelError::Unsupported(
                        "per-patch vertex step functions (tessellation)",
                    ));
                }
                (VertexStepFunction::PerVertex, _) => {
                    return invalid("vertex layout stepRate", "must be 1 for perVertex");
                }
                (VertexStepFunction::PerInstance, _) => {
                    return invalid("vertex layout stepRate", "must be > 0 for perInstance");
                }
                (VertexStepFunction::Constant, _) => {
                    return invalid("vertex layout stepRate", "must be 0 for constant");
                }
            }
        }
        Ok(())
    }

    /// Whether every used attribute offset and every fixed stride of a used layout is a
    /// multiple of the attribute's natural alignment (what core Vulkan requires; dynamic
    /// strides are checked at bind time).
    #[must_use]
    pub fn is_naturally_aligned(&self) -> bool {
        self.used_attributes().all(|(_, a)| {
            let align = u64::from(a.format.natural_alignment());
            let stride_ok = match self.layout_of(a).map(|l| l.stride) {
                Some(VertexStride::Fixed(s)) => s % align == 0,
                Some(VertexStride::Dynamic) | None => true,
            };
            a.offset % align == 0 && stride_ok
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: ResourceLimits = ResourceLimits::APPLE7;

    fn attr(format: VertexFormat, offset: u64, buffer_index: u32) -> Option<VertexAttributeDesc> {
        Some(VertexAttributeDesc {
            format,
            offset,
            buffer_index,
        })
    }

    fn triangle() -> VertexDescriptor {
        let mut d = VertexDescriptor::default();
        d.attributes[0] = attr(VertexFormat::Float3, 0, 0);
        d.attributes[1] = attr(VertexFormat::UChar4Normalized_BGRA, 12, 0);
        d.layouts[0].stride = VertexStride::Fixed(16);
        d
    }

    /// Independent restatement of the raw SDK values (`MTLVertexFormat` in
    /// `MTLVertexDescriptor.h`), so that a typo in the table macro cannot go unnoticed.
    const SDK: &[(u32, &str)] = &[
        (1, "UChar2"),
        (2, "UChar3"),
        (3, "UChar4"),
        (4, "Char2"),
        (5, "Char3"),
        (6, "Char4"),
        (7, "UChar2Normalized"),
        (8, "UChar3Normalized"),
        (9, "UChar4Normalized"),
        (10, "Char2Normalized"),
        (11, "Char3Normalized"),
        (12, "Char4Normalized"),
        (13, "UShort2"),
        (14, "UShort3"),
        (15, "UShort4"),
        (16, "Short2"),
        (17, "Short3"),
        (18, "Short4"),
        (19, "UShort2Normalized"),
        (20, "UShort3Normalized"),
        (21, "UShort4Normalized"),
        (22, "Short2Normalized"),
        (23, "Short3Normalized"),
        (24, "Short4Normalized"),
        (25, "Half2"),
        (26, "Half3"),
        (27, "Half4"),
        (28, "Float"),
        (29, "Float2"),
        (30, "Float3"),
        (31, "Float4"),
        (32, "Int"),
        (33, "Int2"),
        (34, "Int3"),
        (35, "Int4"),
        (36, "UInt"),
        (37, "UInt2"),
        (38, "UInt3"),
        (39, "UInt4"),
        (40, "Int1010102Normalized"),
        (41, "UInt1010102Normalized"),
        (42, "UChar4Normalized_BGRA"),
        (45, "UChar"),
        (46, "Char"),
        (47, "UCharNormalized"),
        (48, "CharNormalized"),
        (49, "UShort"),
        (50, "Short"),
        (51, "UShortNormalized"),
        (52, "ShortNormalized"),
        (53, "Half"),
        (54, "FloatRG11B10"),
        (55, "FloatRGB9E5"),
    ];

    #[test]
    fn table_matches_sdk_values_exactly() {
        assert_eq!(VertexFormat::ALL.len(), SDK.len());
        for (raw, name) in SDK {
            let f = VertexFormat::from_raw(*raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(format!("{f:?}"), *name, "raw {raw}");
            assert_eq!(f.raw(), *raw);
        }
        for raw in 0..=256 {
            let known = SDK.iter().any(|(r, _)| *r == raw);
            assert_eq!(VertexFormat::from_raw(raw).is_ok(), known, "{raw}");
        }
    }

    /// Vertex format → size table, plus the SDK numbering (43 and 44 are unassigned).
    #[test]
    fn vertex_format_table() {
        use VertexFormat as V;
        assert_eq!(V::ALL.len(), 53);
        for (f, size, comps, num) in [
            (V::UChar2, 2, 2, NumericType::Uint),
            (V::Char3Normalized, 3, 3, NumericType::Snorm),
            (V::UShort3, 6, 3, NumericType::Uint),
            (V::Half4, 8, 4, NumericType::Float),
            (V::Float3, 12, 3, NumericType::Float),
            (V::Int4, 16, 4, NumericType::Sint),
            (V::UInt1010102Normalized, 4, 4, NumericType::Unorm),
            (V::UChar4Normalized_BGRA, 4, 4, NumericType::Unorm),
            (V::UCharNormalized, 1, 1, NumericType::Unorm),
            (V::Half, 2, 1, NumericType::Float),
            (V::FloatRG11B10, 4, 3, NumericType::Float),
            (V::FloatRGB9E5, 4, 3, NumericType::Float),
        ] {
            assert_eq!(
                (f.size(), f.components(), f.numeric_type()),
                (size, comps, num),
                "{f:?}"
            );
        }
        for raw in 0..64 {
            let expect_ok = matches!(raw, 1..=42 | 45..=55);
            assert_eq!(VertexFormat::from_raw(raw).is_ok(), expect_ok, "{raw}");
        }
        for f in V::ALL {
            assert_eq!(VertexFormat::from_raw(f.raw()), Ok(*f));
            assert_eq!(f.size() % f.natural_alignment(), 0, "{f:?}");
            assert!(f.components() >= 1 && f.components() <= 4);
        }
        assert_eq!(optional_vertex_format(0), Ok(None));
        assert_eq!(
            optional_vertex_format(56),
            Err(ModelError::UnknownVertexFormat(56))
        );
    }

    #[test]
    fn step_functions_and_strides() {
        assert_eq!(
            VertexStepFunction::from_raw(4),
            Ok(VertexStepFunction::PerPatchControlPoint)
        );
        assert!(VertexStepFunction::from_raw(5).is_err());
        assert_eq!(VertexStride::from_raw(u64::MAX), VertexStride::Dynamic);
        assert_eq!(VertexStride::from_raw(12), VertexStride::Fixed(12));
    }

    #[test]
    fn valid_descriptors() {
        let d = triangle();
        assert!(d.validate(&L).is_ok());
        assert!(d.is_naturally_aligned());
        // Instanced data every 3 instances, constant colour, dynamic stride.
        let mut d = triangle();
        d.attributes[2] = attr(VertexFormat::Float4, 0, 1);
        d.layouts[1] = VertexBufferLayoutDesc {
            stride: VertexStride::Fixed(16),
            step_function: VertexStepFunction::PerInstance,
            step_rate: 3,
        };
        d.attributes[3] = attr(VertexFormat::Float4, 0, 2);
        d.layouts[2] = VertexBufferLayoutDesc {
            stride: VertexStride::Dynamic,
            step_function: VertexStepFunction::Constant,
            step_rate: 0,
        };
        assert!(d.validate(&L).is_ok());
        // Unaligned is valid Metal (1-byte alignment on Apple7) but not natural.
        let mut u = triangle();
        u.attributes[0] = attr(VertexFormat::Float3, 1, 0);
        assert!(u.validate(&L).is_ok());
        assert!(!u.is_naturally_aligned());
        let mut s = triangle();
        s.layouts[0].stride = VertexStride::Fixed(13);
        assert!(!s.is_naturally_aligned());
        // Unused layouts are not checked.
        let mut unused = triangle();
        unused.layouts[30].step_rate = 0;
        assert!(unused.validate(&L).is_ok());
    }

    #[test]
    fn invalid_descriptors() {
        let cases: Vec<(VertexDescriptor, bool)> = {
            let mut v = Vec::new();
            let mut d = triangle();
            d.attributes[0] = attr(VertexFormat::Float, 0, 31);
            v.push((d, false));
            let mut d = triangle();
            d.attributes[0] = attr(VertexFormat::Float, 0, u32::MAX);
            v.push((d, false));
            let mut d = triangle();
            d.attributes[0] = attr(VertexFormat::Float4, u64::MAX - 8, 0);
            v.push((d, false));
            let mut d = triangle();
            d.layouts[0].step_rate = 2;
            v.push((d, false));
            let mut d = triangle();
            d.layouts[0].step_function = VertexStepFunction::PerInstance;
            d.layouts[0].step_rate = 0;
            v.push((d, false));
            let mut d = triangle();
            d.layouts[0].step_function = VertexStepFunction::Constant;
            v.push((d, false));
            let mut d = triangle();
            d.layouts[0].step_function = VertexStepFunction::PerPatch;
            v.push((d, true));
            v
        };
        for (i, (d, unsupported)) in cases.iter().enumerate() {
            let e = d.validate(&L).expect_err(&format!("case {i}"));
            assert_eq!(e.is_unsupported(), *unsupported, "case {i}: {e:?}");
        }
        // Attribute indices above the (reported) limit.
        let small = ResourceLimits {
            max_vertex_attributes: 1,
            ..L
        };
        assert!(triangle().validate(&small).is_err());
        let few_buffers = ResourceLimits {
            max_vertex_buffers: 0,
            ..L
        };
        assert!(triangle().validate(&few_buffers).is_err());
    }
}
