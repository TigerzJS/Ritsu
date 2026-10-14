//! `MTLSamplerDescriptor` and its enums.
//!
//! Raw values: public SDK header `MTLSampler.h` (`MTLSamplerMinMagFilter`,
//! `MTLSamplerMipFilter`, `MTLSamplerAddressMode`, `MTLSamplerBorderColor`,
//! `MTLSamplerReductionMode`). Rules: Apple documentation of the descriptor properties
//! (`maxAnisotropy` 1…16; non-normalized coordinates only with clamp-to-edge/zero, no mipmaps,
//! equal min/mag filters and anisotropy 1) and the Metal Feature Set Tables (border colour:
//! Apple7; min/max reduction and LOD bias: Apple10 only).
//!
//! Decisions (gpu.md §4.4):
//! * `lodAverage` is accepted and ignored: Apple documents it as a performance hint that "is
//!   ignored on some GPUs", so ignoring it is Metal-conformant (resolves open question G-7).
//! * Reduction modes other than weighted average and a non-zero LOD bias are Apple10 features.
//!   Ritsu reports at most Apple7, so they are rejected as [`ModelError::Unsupported`].
//! * Non-normalized coordinates outside the documented conditions give undefined results in
//!   Metal; Vulkan forbids them outright (`VkSamplerCreateInfo::unnormalizedCoordinates`).
//!   Ritsu rejects them as invalid instead of guessing.
//! * Non-normalized coordinates together with a comparison function are not restricted by
//!   Apple's documentation, but Vulkan forbids `compareEnable` with unnormalized coordinates
//!   (`VUID-VkSamplerCreateInfo-unnormalizedCoordinates-01076`) → [`ModelError::Unsupported`]
//!   until a shader-side emulation exists. `compareFunction = never` is the SDK default and
//!   indistinguishable from "no comparison", so it stays allowed (a `sample_compare` through
//!   such a sampler must then be rejected where the shader binding is known).

use super::{CompareFunction, ModelError, raw_enum};

raw_enum! {
    /// `MTLSamplerMinMagFilter`.
    SamplerMinMagFilter, UnknownSamplerFilter {
        /// Nearest texel.
        Nearest = 0,
        /// Bilinear.
        Linear = 1,
    }
}

raw_enum! {
    /// `MTLSamplerMipFilter`.
    SamplerMipFilter, UnknownSamplerMipFilter {
        /// Only the base level.
        NotMipmapped = 0,
        /// Nearest mip level.
        Nearest = 1,
        /// Blend between two mip levels.
        Linear = 2,
    }
}

raw_enum! {
    /// `MTLSamplerAddressMode`.
    SamplerAddressMode, UnknownSamplerAddressMode {
        /// Clamp to the edge texel.
        ClampToEdge = 0,
        /// Mirror once, then clamp to edge.
        MirrorClampToEdge = 1,
        /// Repeat.
        Repeat = 2,
        /// Mirrored repeat.
        MirrorRepeat = 3,
        /// Out-of-range samples return zero (alpha one for formats without alpha).
        ClampToZero = 4,
        /// Out-of-range samples return the border colour.
        ClampToBorderColor = 5,
    }
}

raw_enum! {
    /// `MTLSamplerBorderColor`.
    SamplerBorderColor, UnknownSamplerBorderColor {
        /// (0, 0, 0, 0).
        TransparentBlack = 0,
        /// (0, 0, 0, 1).
        OpaqueBlack = 1,
        /// (1, 1, 1, 1).
        OpaqueWhite = 2,
    }
}

raw_enum! {
    /// `MTLSamplerReductionMode`.
    SamplerReductionMode, UnknownSamplerReductionMode {
        /// Weighted average (normal filtering).
        WeightedAverage = 0,
        /// Component-wise minimum of the footprint.
        Minimum = 1,
        /// Component-wise maximum of the footprint.
        Maximum = 2,
    }
}

/// Highest `maxAnisotropy` Metal accepts (Apple documentation: 1…16).
pub const MAX_ANISOTROPY: u32 = 16;

/// Guest description of a sampler (`MTLSamplerDescriptor`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerDesc {
    /// Minification filter.
    pub min_filter: SamplerMinMagFilter,
    /// Magnification filter.
    pub mag_filter: SamplerMinMagFilter,
    /// Mip filter.
    pub mip_filter: SamplerMipFilter,
    /// Maximum anisotropy (1…16).
    pub max_anisotropy: u32,
    /// Address mode of the s (width) coordinate.
    pub s_address_mode: SamplerAddressMode,
    /// Address mode of the t (height) coordinate.
    pub t_address_mode: SamplerAddressMode,
    /// Address mode of the r (depth) coordinate.
    pub r_address_mode: SamplerAddressMode,
    /// Border colour for [`SamplerAddressMode::ClampToBorderColor`].
    pub border_color: SamplerBorderColor,
    /// Reduction mode.
    pub reduction_mode: SamplerReductionMode,
    /// Normalized (`[0, 1]`) or pixel coordinates.
    pub normalized_coordinates: bool,
    /// Minimum LOD clamp.
    pub lod_min_clamp: f32,
    /// Maximum LOD clamp.
    pub lod_max_clamp: f32,
    /// LOD averaging hint (ignored, see module docs).
    pub lod_average: bool,
    /// LOD bias.
    pub lod_bias: f32,
    /// Comparison for `sample_compare` (`Never` is the SDK default).
    pub compare_function: CompareFunction,
    /// The sampler may be referenced from argument buffers (`gpuResourceID`).
    pub support_argument_buffers: bool,
}

impl Default for SamplerDesc {
    /// The SDK defaults of `MTLSamplerDescriptor`.
    fn default() -> Self {
        Self {
            min_filter: SamplerMinMagFilter::Nearest,
            mag_filter: SamplerMinMagFilter::Nearest,
            mip_filter: SamplerMipFilter::NotMipmapped,
            max_anisotropy: 1,
            s_address_mode: SamplerAddressMode::ClampToEdge,
            t_address_mode: SamplerAddressMode::ClampToEdge,
            r_address_mode: SamplerAddressMode::ClampToEdge,
            border_color: SamplerBorderColor::TransparentBlack,
            reduction_mode: SamplerReductionMode::WeightedAverage,
            normalized_coordinates: true,
            lod_min_clamp: 0.0,
            lod_max_clamp: f32::MAX,
            lod_average: false,
            lod_bias: 0.0,
            compare_function: CompareFunction::Never,
            support_argument_buffers: false,
        }
    }
}

impl SamplerDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidValue`] for out-of-range or undefined combinations,
    /// [`ModelError::Unsupported`] for Apple10-only features.
    pub fn validate(&self) -> Result<(), ModelError> {
        let invalid = |field, reason| Err(ModelError::InvalidValue { field, reason });
        if !(1..=MAX_ANISOTROPY).contains(&self.max_anisotropy) {
            return invalid("maxAnisotropy", "must be between 1 and 16");
        }
        if self.lod_min_clamp.is_nan() || self.lod_max_clamp.is_nan() {
            return invalid("lodMinClamp/lodMaxClamp", "NaN");
        }
        if self.lod_min_clamp > self.lod_max_clamp {
            return invalid("lodMinClamp", "greater than lodMaxClamp");
        }
        if self.lod_bias.is_nan() {
            return invalid("lodBias", "NaN");
        }
        if self.lod_bias != 0.0 {
            return Err(ModelError::Unsupported("sampler LOD bias (Apple10 family)"));
        }
        if self.reduction_mode != SamplerReductionMode::WeightedAverage {
            return Err(ModelError::Unsupported(
                "sampler min/max reduction (Apple10 family)",
            ));
        }
        if !self.normalized_coordinates {
            let clamp = |m| {
                matches!(
                    m,
                    SamplerAddressMode::ClampToEdge | SamplerAddressMode::ClampToZero
                )
            };
            if !clamp(self.s_address_mode) || !clamp(self.t_address_mode) {
                return invalid(
                    "normalizedCoordinates",
                    "pixel coordinates need clampToEdge or clampToZero",
                );
            }
            if self.mip_filter != SamplerMipFilter::NotMipmapped {
                return invalid(
                    "normalizedCoordinates",
                    "pixel coordinates need notMipmapped",
                );
            }
            if self.min_filter != self.mag_filter {
                return invalid(
                    "normalizedCoordinates",
                    "pixel coordinates need minFilter == magFilter",
                );
            }
            if self.max_anisotropy != 1 {
                return invalid(
                    "normalizedCoordinates",
                    "pixel coordinates need maxAnisotropy 1",
                );
            }
            if self.compare_function != CompareFunction::Never {
                return Err(ModelError::Unsupported(
                    "comparison sampler with pixel (non-normalized) coordinates",
                ));
            }
        }
        Ok(())
    }

    /// Whether any coordinate uses [`SamplerAddressMode::ClampToBorderColor`] (the border
    /// colour then matters; a Vulkan backend needs float and integer border variants).
    #[must_use]
    pub fn uses_border_color(&self) -> bool {
        [
            self.s_address_mode,
            self.t_address_mode,
            self.r_address_mode,
        ]
        .contains(&SamplerAddressMode::ClampToBorderColor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_values() {
        assert_eq!(SamplerMinMagFilter::ALL.len(), 2);
        assert_eq!(SamplerMipFilter::from_raw(2), Ok(SamplerMipFilter::Linear));
        assert_eq!(
            SamplerAddressMode::from_raw(1),
            Ok(SamplerAddressMode::MirrorClampToEdge)
        );
        assert_eq!(
            SamplerAddressMode::from_raw(5),
            Ok(SamplerAddressMode::ClampToBorderColor)
        );
        assert_eq!(
            SamplerAddressMode::from_raw(6),
            Err(ModelError::UnknownSamplerAddressMode(6))
        );
        assert_eq!(
            SamplerBorderColor::from_raw(3),
            Err(ModelError::UnknownSamplerBorderColor(3))
        );
        assert_eq!(
            SamplerReductionMode::from_raw(2),
            Ok(SamplerReductionMode::Maximum)
        );
        assert!(SamplerMipFilter::from_raw(3).is_err());
        assert!(SamplerMinMagFilter::from_raw(2).is_err());
        for (i, m) in SamplerAddressMode::ALL.iter().enumerate() {
            assert_eq!(m.raw() as usize, i);
        }
    }

    #[test]
    fn defaults_are_valid() {
        let d = SamplerDesc::default();
        assert!(d.validate().is_ok());
        assert!(!d.uses_border_color());
    }

    #[test]
    fn valid_samplers() {
        let trilinear = SamplerDesc {
            min_filter: SamplerMinMagFilter::Linear,
            mag_filter: SamplerMinMagFilter::Linear,
            mip_filter: SamplerMipFilter::Linear,
            max_anisotropy: 16,
            s_address_mode: SamplerAddressMode::Repeat,
            t_address_mode: SamplerAddressMode::MirrorRepeat,
            r_address_mode: SamplerAddressMode::ClampToBorderColor,
            border_color: SamplerBorderColor::OpaqueWhite,
            lod_min_clamp: 1.0,
            lod_max_clamp: 1.0,
            lod_average: true,
            compare_function: CompareFunction::LessEqual,
            ..SamplerDesc::default()
        };
        assert!(trilinear.validate().is_ok());
        assert!(trilinear.uses_border_color());
        let pixel = SamplerDesc {
            normalized_coordinates: false,
            s_address_mode: SamplerAddressMode::ClampToZero,
            r_address_mode: SamplerAddressMode::Repeat, // r is irrelevant for 1D/2D
            min_filter: SamplerMinMagFilter::Linear,
            mag_filter: SamplerMinMagFilter::Linear,
            ..SamplerDesc::default()
        };
        assert!(pixel.validate().is_ok());
        // `never` is the SDK default and therefore allowed with pixel coordinates.
        assert_eq!(pixel.compare_function, CompareFunction::Never);
    }

    #[test]
    fn pixel_coordinates_with_comparison_are_unsupported() {
        let pixel = SamplerDesc {
            normalized_coordinates: false,
            ..SamplerDesc::default()
        };
        for c in CompareFunction::ALL
            .iter()
            .filter(|c| **c != CompareFunction::Never)
        {
            let e = SamplerDesc {
                compare_function: *c,
                ..pixel
            }
            .validate()
            .unwrap_err();
            assert!(e.is_unsupported(), "{c:?}: {e:?}");
        }
        // With normalized coordinates every comparison is fine.
        for c in CompareFunction::ALL {
            let s = SamplerDesc {
                compare_function: *c,
                ..SamplerDesc::default()
            };
            assert!(s.validate().is_ok(), "{c:?}");
        }
    }

    #[test]
    fn invalid_samplers() {
        let d = SamplerDesc::default();
        let bad = [
            SamplerDesc {
                max_anisotropy: 0,
                ..d
            },
            SamplerDesc {
                max_anisotropy: 17,
                ..d
            },
            SamplerDesc {
                max_anisotropy: u32::MAX,
                ..d
            },
            SamplerDesc {
                lod_min_clamp: f32::NAN,
                ..d
            },
            SamplerDesc {
                lod_max_clamp: f32::NAN,
                ..d
            },
            SamplerDesc {
                lod_min_clamp: 2.0,
                lod_max_clamp: 1.0,
                ..d
            },
            SamplerDesc {
                lod_bias: f32::NAN,
                ..d
            },
            SamplerDesc {
                normalized_coordinates: false,
                s_address_mode: SamplerAddressMode::Repeat,
                ..d
            },
            SamplerDesc {
                normalized_coordinates: false,
                t_address_mode: SamplerAddressMode::ClampToBorderColor,
                ..d
            },
            SamplerDesc {
                normalized_coordinates: false,
                mip_filter: SamplerMipFilter::Nearest,
                ..d
            },
            SamplerDesc {
                normalized_coordinates: false,
                min_filter: SamplerMinMagFilter::Linear,
                ..d
            },
            SamplerDesc {
                normalized_coordinates: false,
                max_anisotropy: 2,
                ..d
            },
        ];
        for (i, s) in bad.iter().enumerate() {
            assert!(
                matches!(s.validate(), Err(ModelError::InvalidValue { .. })),
                "case {i}: {:?}",
                s.validate()
            );
        }
        // Apple10 features are valid Metal, but not for the families Ritsu reports.
        for s in [
            SamplerDesc { lod_bias: 0.5, ..d },
            SamplerDesc {
                reduction_mode: SamplerReductionMode::Minimum,
                ..d
            },
        ] {
            let e = s.validate().unwrap_err();
            assert!(e.is_unsupported(), "{e:?}");
        }
    }
}
