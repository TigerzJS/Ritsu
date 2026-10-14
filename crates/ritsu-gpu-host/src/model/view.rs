//! Texture swizzle (`MTLTextureSwizzle`, `MTLTextureSwizzleChannels`) and texture views
//! (`newTextureViewWithPixelFormat:textureType:levels:slices:swizzle:`).
//!
//! Raw values: public SDK header `MTLTexture.h`. Type-cast table and format rules: Apple
//! documentation of `MTLTexture.makeTextureView(pixelFormat:textureType:levels:slices:)` and
//! `MTLTextureUsage.pixelFormatView`. Swizzle composition: Apple documentation of
//! `makeTextureView(pixelFormat:textureType:levels:slices:swizzle:)` ("the swizzle pattern of the
//! view is combined with that of the parent texture"; example `[R,G,A,B]` on `[R,1,1,G]` gives
//! `[R,1,G,1]`), see [`TextureSwizzleChannels::compose`].
//!
//! Vulkan note (for WP-GPU-05): `VkComponentMapping` must be the identity on views used as
//! attachments or storage images, so a backend keeps a second, unswizzled view for those uses.
//! Metal applies the swizzle only to reads and samples, so both views together are exact.

use super::{ModelError, PixelFormat, TextureDesc, TextureType, TextureUsage, raw_enum};

raw_enum! {
    /// `MTLTextureSwizzle`: source of one output channel.
    TextureSwizzle, UnknownTextureSwizzle {
        /// Constant 0.
        Zero = 0,
        /// Constant 1.
        One = 1,
        /// Red channel.
        Red = 2,
        /// Green channel.
        Green = 3,
        /// Blue channel.
        Blue = 4,
        /// Alpha channel.
        Alpha = 5,
    }
}

/// `MTLTextureSwizzleChannels`: four [`TextureSwizzle`] values (one byte each in the SDK).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureSwizzleChannels {
    /// Source of the red output.
    pub red: TextureSwizzle,
    /// Source of the green output.
    pub green: TextureSwizzle,
    /// Source of the blue output.
    pub blue: TextureSwizzle,
    /// Source of the alpha output.
    pub alpha: TextureSwizzle,
}

impl Default for TextureSwizzleChannels {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl TextureSwizzleChannels {
    /// No swizzle (`MTLTextureSwizzleChannelsDefault`).
    pub const IDENTITY: Self = Self {
        red: TextureSwizzle::Red,
        green: TextureSwizzle::Green,
        blue: TextureSwizzle::Blue,
        alpha: TextureSwizzle::Alpha,
    };

    /// Parses four raw `MTLTextureSwizzle` bytes (red, green, blue, alpha).
    ///
    /// # Errors
    ///
    /// [`ModelError::UnknownTextureSwizzle`] for any unknown value.
    pub const fn from_raw(raw: [u8; 4]) -> Result<Self, ModelError> {
        macro_rules! ch {
            ($v:expr) => {
                match TextureSwizzle::from_raw($v as u32) {
                    Ok(s) => s,
                    Err(e) => return Err(e),
                }
            };
        }
        Ok(Self {
            red: ch!(raw[0]),
            green: ch!(raw[1]),
            blue: ch!(raw[2]),
            alpha: ch!(raw[3]),
        })
    }

    /// Raw bytes (red, green, blue, alpha).
    #[must_use]
    pub const fn raw(self) -> [u8; 4] {
        // Values are 0..=5 by construction.
        [
            self.red.raw() as u8,
            self.green.raw() as u8,
            self.blue.raw() as u8,
            self.alpha.raw() as u8,
        ]
    }

    /// Whether this is the identity mapping.
    #[must_use]
    pub fn is_identity(self) -> bool {
        self == Self::IDENTITY
    }

    /// Output channel `channel` of this swizzle applied to an already swizzled `input`.
    const fn select(input: Self, channel: TextureSwizzle) -> TextureSwizzle {
        match channel {
            TextureSwizzle::Zero => TextureSwizzle::Zero,
            TextureSwizzle::One => TextureSwizzle::One,
            TextureSwizzle::Red => input.red,
            TextureSwizzle::Green => input.green,
            TextureSwizzle::Blue => input.blue,
            TextureSwizzle::Alpha => input.alpha,
        }
    }

    /// Swizzle of a view with swizzle `self` on a texture with swizzle `parent`:
    /// `final[c] = parent[self[c]]`, constants stay constants (Apple: the view's pattern is
    /// combined with the parent's; `[R,G,A,B]` on `[R,1,1,G]` → `[R,1,G,1]`).
    #[must_use]
    pub const fn compose(self, parent: Self) -> Self {
        Self {
            red: Self::select(parent, self.red),
            green: Self::select(parent, self.green),
            blue: Self::select(parent, self.blue),
            alpha: Self::select(parent, self.alpha),
        }
    }
}

/// A range of mip levels or array slices (`NSRange` in the SDK).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubresourceRange {
    /// First element.
    pub start: u32,
    /// Number of elements (≥ 1).
    pub count: u32,
}

impl SubresourceRange {
    /// Range `start..start + count`.
    #[must_use]
    pub const fn new(start: u32, count: u32) -> Self {
        Self { start, count }
    }

    /// Checks `count ≥ 1` and `start + count ≤ max` with checked arithmetic.
    ///
    /// # Errors
    ///
    /// [`ModelError::RangeOutOfBounds`].
    pub fn check(&self, max: u32, field: &'static str) -> Result<(), ModelError> {
        match self.start.checked_add(self.count) {
            Some(end) if self.count >= 1 && end <= max => Ok(()),
            _ => Err(ModelError::RangeOutOfBounds {
                field,
                start: self.start,
                count: self.count,
                max,
            }),
        }
    }
}

/// Guest description of a texture view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureViewDesc {
    /// View pixel format.
    pub pixel_format: PixelFormat,
    /// View texture type.
    pub texture_type: TextureType,
    /// Mip levels of the parent visible in the view.
    pub levels: SubresourceRange,
    /// Array slices (cube faces count individually) of the parent visible in the view.
    pub slices: SubresourceRange,
    /// Swizzle of the view, relative to the parent texture's swizzle (the effective swizzle of
    /// the view is [`TextureSwizzleChannels::compose`] of both).
    pub swizzle: TextureSwizzleChannels,
}

impl TextureViewDesc {
    /// Whether `from` may be viewed as `to` (Apple's cast table; types not listed there only
    /// as themselves \[Entscheidung: conservative\]).
    #[must_use]
    pub const fn type_cast_allowed(from: TextureType, to: TextureType) -> bool {
        use TextureType as T;
        match from {
            T::Type2D => matches!(to, T::Type2D | T::Type2DArray),
            T::Type2DArray | T::TypeCube | T::TypeCubeArray => {
                matches!(
                    to,
                    T::Type2D | T::Type2DArray | T::TypeCube | T::TypeCubeArray
                )
            }
            T::Type1D => matches!(to, T::Type1D),
            T::Type3D => matches!(to, T::Type3D),
            T::Type1DArray => matches!(to, T::Type1DArray),
            T::Type2DMultisample => matches!(to, T::Type2DMultisample),
            T::Type2DMultisampleArray => matches!(to, T::Type2DMultisampleArray),
            T::TypeTextureBuffer => matches!(to, T::TypeTextureBuffer),
        }
    }

    /// Validates the view against its parent texture (which must itself be valid) and returns
    /// the descriptor the view behaves like (extent of its base level, its mip and slice
    /// counts, the parent's storage and usage).
    ///
    /// # Errors
    ///
    /// Incompatible format or type, missing `PixelFormatView` usage, out-of-range levels or
    /// slices, wrong slice count for 2D/cube views, non-square cube views.
    pub fn validate(&self, parent: &TextureDesc) -> Result<TextureDesc, ModelError> {
        use TextureType as T;
        let from = parent.pixel_format;
        let to = self.pixel_format;
        if !from.view_compatible(to) {
            return Err(ModelError::IncompatibleViewFormat { from, to });
        }
        if from.view_needs_pixel_format_view_usage(to)
            && !parent.usage.contains(TextureUsage::PIXEL_FORMAT_VIEW)
        {
            return Err(ModelError::UsageNotAllowedForFormat {
                format: to,
                usage: parent.usage,
            });
        }
        if !Self::type_cast_allowed(parent.texture_type, self.texture_type) {
            return Err(ModelError::IncompatibleViewType {
                from: parent.texture_type,
                to: self.texture_type,
            });
        }
        self.levels
            .check(parent.mip_level_count, "texture view levels")?;
        self.slices
            .check(parent.layer_count()?, "texture view slices")?;

        let slice_rule = match self.texture_type {
            T::Type1D | T::Type2D | T::Type2DMultisample | T::Type3D | T::TypeTextureBuffer => {
                self.slices.count == 1
            }
            T::TypeCube => self.slices.count == 6,
            T::TypeCubeArray => self.slices.count.is_multiple_of(6),
            T::Type1DArray | T::Type2DArray | T::Type2DMultisampleArray => true,
        };
        if !slice_rule {
            return Err(ModelError::InvalidValue {
                field: "texture view slices",
                reason: "slice count does not fit the view type (2D: 1, cube: 6, cube array: n × 6)",
            });
        }
        let (width, height, depth) = parent.level_extent(self.levels.start);
        if self.texture_type.is_cube() && width != height {
            return Err(ModelError::CubeNotSquare);
        }
        let array_length = if self.texture_type.is_cube() {
            self.slices.count / 6
        } else {
            self.slices.count
        };
        Ok(TextureDesc {
            texture_type: self.texture_type,
            pixel_format: to,
            width,
            height,
            depth: if self.texture_type == T::Type3D {
                depth
            } else {
                1
            },
            array_length,
            mip_level_count: self.levels.count,
            sample_count: parent.sample_count,
            storage_mode: parent.storage_mode,
            usage: parent.usage,
            swizzle: self.swizzle.compose(parent.swizzle),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ResourceLimits, StorageMode};

    fn parent() -> TextureDesc {
        TextureDesc {
            texture_type: TextureType::Type2DArray,
            array_length: 12,
            mip_level_count: 5,
            ..TextureDesc::new_2d(
                PixelFormat::RGBA8Unorm,
                64,
                64,
                TextureUsage::SHADER_READ.union(TextureUsage::RENDER_TARGET),
            )
        }
    }

    fn view(
        fmt: PixelFormat,
        ty: TextureType,
        levels: (u32, u32),
        slices: (u32, u32),
    ) -> TextureViewDesc {
        TextureViewDesc {
            pixel_format: fmt,
            texture_type: ty,
            levels: SubresourceRange::new(levels.0, levels.1),
            slices: SubresourceRange::new(slices.0, slices.1),
            swizzle: TextureSwizzleChannels::IDENTITY,
        }
    }

    #[test]
    fn swizzle_raw_round_trip() {
        for r in 0..=5u8 {
            let s = TextureSwizzleChannels::from_raw([r, 5 - r, 2, 3]).unwrap();
            assert_eq!(s.raw(), [r, 5 - r, 2, 3]);
        }
        assert_eq!(
            TextureSwizzleChannels::from_raw([2, 3, 4, 6]),
            Err(ModelError::UnknownTextureSwizzle(6))
        );
        assert!(TextureSwizzleChannels::default().is_identity());
        assert_eq!(
            TextureSwizzleChannels::from_raw([2, 3, 4, 5]),
            Ok(TextureSwizzleChannels::IDENTITY)
        );
    }

    #[test]
    fn valid_views() {
        let p = parent();
        assert!(p.validate(&ResourceLimits::APPLE7).is_ok());
        // sRGB view without PixelFormatView usage.
        let v = view(
            PixelFormat::RGBA8Unorm_sRGB,
            TextureType::Type2D,
            (1, 2),
            (3, 1),
        )
        .validate(&p)
        .unwrap();
        assert_eq!(
            (v.width, v.height, v.mip_level_count, v.array_length),
            (32, 32, 2, 1)
        );
        // Cube and cube-array views of a 2D array.
        let cube = view(
            PixelFormat::RGBA8Unorm,
            TextureType::TypeCube,
            (0, 5),
            (6, 6),
        )
        .validate(&p)
        .unwrap();
        assert_eq!(cube.array_length, 1);
        assert_eq!(cube.layer_count(), Ok(6));
        let ca = view(
            PixelFormat::RGBA8Unorm,
            TextureType::TypeCubeArray,
            (0, 1),
            (0, 12),
        )
        .validate(&p)
        .unwrap();
        assert_eq!(ca.array_length, 2);
        // Same layout, different numeric type: no usage flag needed.
        assert!(
            view(
                PixelFormat::RGBA8Uint,
                TextureType::Type2DArray,
                (0, 5),
                (0, 12)
            )
            .validate(&p)
            .is_ok()
        );
        // Stencil view of a depth/stencil texture.
        let ds = TextureDesc {
            pixel_format: PixelFormat::Depth32Float_Stencil8,
            storage_mode: StorageMode::Private,
            ..TextureDesc::new_2d(
                PixelFormat::Depth32Float_Stencil8,
                16,
                16,
                TextureUsage::RENDER_TARGET,
            )
        };
        assert!(
            view(
                PixelFormat::X32_Stencil8,
                TextureType::Type2D,
                (0, 1),
                (0, 1)
            )
            .validate(&ds)
            .is_ok()
        );
    }

    #[test]
    fn invalid_views() {
        let p = parent();
        type Case = (TextureViewDesc, fn(&ModelError) -> bool);
        let cases: Vec<Case> = vec![
            (
                view(PixelFormat::R32Uint, TextureType::Type2D, (0, 1), (0, 1)),
                |e| matches!(e, ModelError::UsageNotAllowedForFormat { .. }),
            ),
            (
                view(PixelFormat::RG8Unorm, TextureType::Type2D, (0, 1), (0, 1)),
                |e| matches!(e, ModelError::IncompatibleViewFormat { .. }),
            ),
            (
                view(PixelFormat::RGBA8Unorm, TextureType::Type3D, (0, 1), (0, 1)),
                |e| matches!(e, ModelError::IncompatibleViewType { .. }),
            ),
            (
                view(PixelFormat::RGBA8Unorm, TextureType::Type2D, (4, 2), (0, 1)),
                |e| matches!(e, ModelError::RangeOutOfBounds { .. }),
            ),
            (
                view(PixelFormat::RGBA8Unorm, TextureType::Type2D, (0, 0), (0, 1)),
                |e| matches!(e, ModelError::RangeOutOfBounds { .. }),
            ),
            (
                view(
                    PixelFormat::RGBA8Unorm,
                    TextureType::Type2D,
                    (u32::MAX, 2),
                    (0, 1),
                ),
                |e| matches!(e, ModelError::RangeOutOfBounds { .. }),
            ),
            (
                view(
                    PixelFormat::RGBA8Unorm,
                    TextureType::Type2DArray,
                    (0, 1),
                    (11, u32::MAX),
                ),
                |e| matches!(e, ModelError::RangeOutOfBounds { .. }),
            ),
            (
                view(PixelFormat::RGBA8Unorm, TextureType::Type2D, (0, 1), (0, 2)),
                |e| matches!(e, ModelError::InvalidValue { .. }),
            ),
            (
                view(
                    PixelFormat::RGBA8Unorm,
                    TextureType::TypeCube,
                    (0, 1),
                    (0, 5),
                ),
                |e| matches!(e, ModelError::InvalidValue { .. }),
            ),
            (
                view(
                    PixelFormat::RGBA8Unorm,
                    TextureType::TypeCubeArray,
                    (0, 1),
                    (0, 8),
                ),
                |e| matches!(e, ModelError::InvalidValue { .. }),
            ),
        ];
        for (i, (v, pred)) in cases.iter().enumerate() {
            let e = v.validate(&p).expect_err(&format!("case {i}"));
            assert!(pred(&e), "case {i}: {e:?}");
        }
        // With the usage flag the reinterpretation is fine.
        let pv = TextureDesc {
            usage: p.usage.union(TextureUsage::PIXEL_FORMAT_VIEW),
            ..p
        };
        assert!(
            view(PixelFormat::R32Uint, TextureType::Type2D, (0, 1), (0, 1))
                .validate(&pv)
                .is_ok()
        );
        // Non-square cube views.
        let rect = TextureDesc {
            width: 64,
            height: 32,
            ..p
        };
        assert_eq!(
            view(
                PixelFormat::RGBA8Unorm,
                TextureType::TypeCube,
                (0, 1),
                (0, 6)
            )
            .validate(&rect),
            Err(ModelError::CubeNotSquare)
        );
    }

    #[test]
    fn view_swizzle_is_combined_with_the_texture_swizzle() {
        use TextureSwizzle::{Alpha as A, Blue as B, Green as G, One as I, Red as R, Zero as Z};
        let sw = |red, green, blue, alpha| TextureSwizzleChannels {
            red,
            green,
            blue,
            alpha,
        };
        // Apple's documented example.
        let texture = sw(R, I, I, G);
        let view = sw(R, G, A, B);
        assert_eq!(view.compose(texture), sw(R, I, G, I));
        // Identity is neutral on both sides; constants stay constants.
        assert_eq!(view.compose(TextureSwizzleChannels::IDENTITY), view);
        assert_eq!(TextureSwizzleChannels::IDENTITY.compose(texture), texture);
        assert_eq!(sw(Z, I, A, A).compose(texture), sw(Z, I, G, G));
        // Through `validate`: the derived descriptor carries the combined swizzle.
        let p = TextureDesc {
            swizzle: texture,
            ..parent()
        };
        let v = TextureViewDesc {
            swizzle: view,
            ..view_desc_2d()
        }
        .validate(&p)
        .unwrap();
        assert_eq!(v.swizzle, sw(R, I, G, I));
    }

    fn view_desc_2d() -> TextureViewDesc {
        view(PixelFormat::RGBA8Unorm, TextureType::Type2D, (0, 1), (0, 1))
    }

    #[test]
    fn type_cast_table() {
        use TextureType as T;
        for from in T::ALL {
            for to in T::ALL {
                let ok = TextureViewDesc::type_cast_allowed(*from, *to);
                if from == to {
                    assert!(ok, "{from:?}");
                }
                if ok && from != to {
                    assert!(
                        matches!(
                            from,
                            T::Type2D | T::Type2DArray | T::TypeCube | T::TypeCubeArray
                        ),
                        "{from:?} -> {to:?}"
                    );
                }
            }
        }
        assert!(!TextureViewDesc::type_cast_allowed(T::Type2D, T::TypeCube));
        assert!(TextureViewDesc::type_cast_allowed(T::TypeCube, T::Type2D));
    }
}
