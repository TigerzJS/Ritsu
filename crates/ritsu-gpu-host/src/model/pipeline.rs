//! `MTLRenderPipelineDescriptor` without shader contents: colour attachments with blending
//! and write masks, depth/stencil formats, sample count, alpha-to-coverage, topology class and
//! the vertex descriptor.
//!
//! Raw values: public SDK header `MTLRenderPipeline.h` (`MTLBlendFactor`, `MTLBlendOperation`,
//! `MTLColorWriteMask`, `MTLPrimitiveTopologyClass`). The Metal 4 "unspecialized" values
//! (blend factor 19, blend operation 5, write mask bit `0x10`, pixel format 263) belong to
//! flexible render pipeline states, which are not modelled → [`ModelError::Unsupported`].
//!
//! Shader functions, linked functions, binary archives and tessellation state are outside
//! this descriptor (shader side: `compiler/`, tessellation: later WP).

use super::{
    FormatCaps, ModelError, PixelFormat, PrimitiveType, ResourceLimits, VertexDescriptor, raw_enum,
    render_pass::{MAX_COLOR_ATTACHMENTS, RenderTargetLayout},
};

raw_enum! {
    /// `MTLBlendFactor`.
    BlendFactor, UnknownBlendFactor {
        /// 0.
        Zero = 0,
        /// 1.
        One = 1,
        /// Source colour.
        SourceColor = 2,
        /// 1 − source colour.
        OneMinusSourceColor = 3,
        /// Source alpha.
        SourceAlpha = 4,
        /// 1 − source alpha.
        OneMinusSourceAlpha = 5,
        /// Destination colour.
        DestinationColor = 6,
        /// 1 − destination colour.
        OneMinusDestinationColor = 7,
        /// Destination alpha.
        DestinationAlpha = 8,
        /// 1 − destination alpha.
        OneMinusDestinationAlpha = 9,
        /// min(source alpha, 1 − destination alpha).
        SourceAlphaSaturated = 10,
        /// Constant blend colour.
        BlendColor = 11,
        /// 1 − constant blend colour.
        OneMinusBlendColor = 12,
        /// Constant blend alpha.
        BlendAlpha = 13,
        /// 1 − constant blend alpha.
        OneMinusBlendAlpha = 14,
        /// Second fragment output colour (dual-source blending).
        Source1Color = 15,
        /// 1 − second output colour.
        OneMinusSource1Color = 16,
        /// Second fragment output alpha.
        Source1Alpha = 17,
        /// 1 − second output alpha.
        OneMinusSource1Alpha = 18,
        /// Metal 4: decided when the pipeline is specialized.
        Unspecialized = 19,
    }
}

impl BlendFactor {
    /// Whether the factor reads the second fragment output (dual-source blending).
    #[must_use]
    pub const fn is_dual_source(self) -> bool {
        matches!(
            self,
            Self::Source1Color
                | Self::OneMinusSource1Color
                | Self::Source1Alpha
                | Self::OneMinusSource1Alpha
        )
    }
}

raw_enum! {
    /// `MTLBlendOperation`.
    BlendOperation, UnknownBlendOperation {
        /// src·Fs + dst·Fd.
        Add = 0,
        /// src·Fs − dst·Fd.
        Subtract = 1,
        /// dst·Fd − src·Fs.
        ReverseSubtract = 2,
        /// min(src, dst).
        Min = 3,
        /// max(src, dst).
        Max = 4,
        /// Metal 4: decided when the pipeline is specialized.
        Unspecialized = 5,
    }
}

raw_enum! {
    /// `MTLPrimitiveTopologyClass`.
    PrimitiveTopologyClass, UnknownPrimitiveTopologyClass {
        /// Any primitive type may be drawn.
        Unspecified = 0,
        /// Points.
        Point = 1,
        /// Lines and line strips.
        Line = 2,
        /// Triangles and triangle strips.
        Triangle = 3,
    }
}

impl PrimitiveTopologyClass {
    /// Whether a draw with `primitive` is allowed with a pipeline of this class.
    #[must_use]
    pub const fn accepts(self, primitive: PrimitiveType) -> bool {
        match self {
            Self::Unspecified => true,
            _ => self as u8 == primitive.topology_class() as u8,
        }
    }
}

/// `MTLColorWriteMask` bit set (red is the high bit in the SDK).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColorWriteMask(u32);

impl ColorWriteMask {
    /// Nothing written.
    pub const NONE: Self = Self(0);
    /// Red.
    pub const RED: Self = Self(0x8);
    /// Green.
    pub const GREEN: Self = Self(0x4);
    /// Blue.
    pub const BLUE: Self = Self(0x2);
    /// Alpha.
    pub const ALPHA: Self = Self(0x1);
    /// All channels.
    pub const ALL: Self = Self(0xf);
    /// Raw Metal 4 "unspecialized" bit.
    pub const UNSPECIALIZED_BIT: u32 = 0x10;

    /// Parses raw mask bits.
    ///
    /// # Errors
    ///
    /// [`ModelError::Unsupported`] for the Metal 4 unspecialized bit,
    /// [`ModelError::UnknownColorWriteMask`] for other undefined bits.
    pub const fn from_raw(raw: u32) -> Result<Self, ModelError> {
        if raw == Self::UNSPECIALIZED_BIT {
            return Err(ModelError::Unsupported(
                "MTLColorWriteMaskUnspecialized (Metal 4 flexible render pipelines)",
            ));
        }
        if raw & !0xf != 0 {
            return Err(ModelError::UnknownColorWriteMask(raw));
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
}

/// `MTLRenderPipelineColorAttachmentDescriptor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderPipelineColorAttachmentDesc {
    /// Pixel format; `None` = `MTLPixelFormatInvalid` (slot unused).
    pub pixel_format: Option<PixelFormat>,
    /// Blending on.
    pub blending_enabled: bool,
    /// Source RGB factor.
    pub source_rgb_blend_factor: BlendFactor,
    /// Destination RGB factor.
    pub destination_rgb_blend_factor: BlendFactor,
    /// RGB operation.
    pub rgb_blend_operation: BlendOperation,
    /// Source alpha factor.
    pub source_alpha_blend_factor: BlendFactor,
    /// Destination alpha factor.
    pub destination_alpha_blend_factor: BlendFactor,
    /// Alpha operation.
    pub alpha_blend_operation: BlendOperation,
    /// Write mask.
    pub write_mask: ColorWriteMask,
}

impl Default for RenderPipelineColorAttachmentDesc {
    /// SDK defaults: no format, blending off, `one`/`zero`/`add`, all channels written.
    fn default() -> Self {
        Self {
            pixel_format: None,
            blending_enabled: false,
            source_rgb_blend_factor: BlendFactor::One,
            destination_rgb_blend_factor: BlendFactor::Zero,
            rgb_blend_operation: BlendOperation::Add,
            source_alpha_blend_factor: BlendFactor::One,
            destination_alpha_blend_factor: BlendFactor::Zero,
            alpha_blend_operation: BlendOperation::Add,
            write_mask: ColorWriteMask::ALL,
        }
    }
}

impl RenderPipelineColorAttachmentDesc {
    fn factors(&self) -> [BlendFactor; 4] {
        [
            self.source_rgb_blend_factor,
            self.destination_rgb_blend_factor,
            self.source_alpha_blend_factor,
            self.destination_alpha_blend_factor,
        ]
    }

    /// Whether blending reads the second fragment output.
    #[must_use]
    pub fn uses_dual_source(&self) -> bool {
        self.blending_enabled && self.factors().iter().any(|f| f.is_dual_source())
    }
}

/// `MTLRenderPipelineDescriptor` without shader functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderPipelineDesc {
    /// Colour attachments by index.
    pub color_attachments: [RenderPipelineColorAttachmentDesc; MAX_COLOR_ATTACHMENTS],
    /// Depth attachment format.
    pub depth_attachment_pixel_format: Option<PixelFormat>,
    /// Stencil attachment format.
    pub stencil_attachment_pixel_format: Option<PixelFormat>,
    /// `rasterSampleCount`.
    pub raster_sample_count: u32,
    /// Alpha-to-coverage.
    pub alpha_to_coverage_enabled: bool,
    /// Alpha-to-one.
    pub alpha_to_one_enabled: bool,
    /// Rasterization on (off: vertex-only pipeline).
    pub rasterization_enabled: bool,
    /// Topology class (required by Metal for layered rendering).
    pub input_primitive_topology: PrimitiveTopologyClass,
    /// Vertex descriptor (`None`: vertex shader fetches its data itself).
    pub vertex_descriptor: Option<VertexDescriptor>,
    /// `maxVertexAmplificationCount`.
    pub max_vertex_amplification_count: u32,
}

impl Default for RenderPipelineDesc {
    /// SDK defaults.
    fn default() -> Self {
        Self {
            color_attachments: [RenderPipelineColorAttachmentDesc::default();
                MAX_COLOR_ATTACHMENTS],
            depth_attachment_pixel_format: None,
            stencil_attachment_pixel_format: None,
            raster_sample_count: 1,
            alpha_to_coverage_enabled: false,
            alpha_to_one_enabled: false,
            rasterization_enabled: true,
            input_primitive_topology: PrimitiveTopologyClass::Unspecified,
            vertex_descriptor: None,
            max_vertex_amplification_count: 1,
        }
    }
}

impl RenderPipelineDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidValue`] / [`ModelError::UsageNotAllowedForFormat`] for invalid
    /// combinations; [`ModelError::Unsupported`] for Metal 4 unspecialized values and vertex
    /// amplification.
    pub fn validate(&self, limits: &ResourceLimits) -> Result<(), ModelError> {
        let invalid = |field, reason| Err(ModelError::InvalidValue { field, reason });
        if !limits.sample_count_ok(self.raster_sample_count) {
            return Err(ModelError::InvalidSampleCount(self.raster_sample_count));
        }
        match self.max_vertex_amplification_count {
            0 | 1 => {}
            _ => return Err(ModelError::Unsupported("vertex amplification")),
        }
        let msaa = self.raster_sample_count > 1;
        let mut dual_source_at: Option<usize> = None;
        let mut used = 0usize;
        for (i, c) in self.color_attachments.iter().enumerate() {
            let unspecialized = c.factors().contains(&BlendFactor::Unspecialized)
                || c.rgb_blend_operation == BlendOperation::Unspecialized
                || c.alpha_blend_operation == BlendOperation::Unspecialized;
            if unspecialized {
                return Err(ModelError::Unsupported(
                    "unspecialized blend state (Metal 4 flexible render pipelines)",
                ));
            }
            let Some(fmt) = c.pixel_format else {
                continue;
            };
            used += 1;
            if i as u64 >= u64::from(limits.max_color_attachments) {
                return invalid(
                    "colorAttachments",
                    "index exceeds the colour attachment limit",
                );
            }
            let caps = fmt.metal_caps();
            if !caps.contains(FormatCaps::COLOR) {
                return Err(ModelError::FormatNotUsable {
                    format: fmt,
                    reason: "not colour-renderable",
                });
            }
            if msaa && !caps.contains(FormatCaps::MSAA) {
                return Err(ModelError::FormatNotUsable {
                    format: fmt,
                    reason: "no multisample support",
                });
            }
            if c.blending_enabled && !caps.contains(FormatCaps::BLEND) {
                return Err(ModelError::FormatNotUsable {
                    format: fmt,
                    reason: "not blendable",
                });
            }
            if c.uses_dual_source() {
                dual_source_at = Some(i);
            }
        }
        // [Annahme: conservative, matches Vulkan's maxFragmentDualSrcAttachments = 1 on both
        // target GPUs] dual-source blending only on attachment 0 and with no other colour
        // attachment.
        if let Some(i) = dual_source_at
            && (i != 0 || used > 1)
        {
            return invalid(
                "blend factors",
                "dual-source blending needs colour attachment 0 as the only colour attachment",
            );
        }

        if let Some(d) = self.depth_attachment_pixel_format
            && !d.metal_caps().contains(FormatCaps::DEPTH)
        {
            return Err(ModelError::FormatNotUsable {
                format: d,
                reason: "not a depth attachment format",
            });
        }
        if let Some(s) = self.stencil_attachment_pixel_format
            && !s.metal_caps().contains(FormatCaps::STENCIL)
        {
            return Err(ModelError::FormatNotUsable {
                format: s,
                reason: "not a stencil attachment format",
            });
        }
        if let (Some(d), Some(s)) = (
            self.depth_attachment_pixel_format,
            self.stencil_attachment_pixel_format,
        ) {
            let combined = d.aspects().has_stencil() || s.aspects().has_depth();
            if combined && d != s {
                return invalid(
                    "depth/stencil pixel formats",
                    "a combined depth/stencil format must be used for both",
                );
            }
        }
        for f in [
            self.depth_attachment_pixel_format,
            self.stencil_attachment_pixel_format,
        ]
        .into_iter()
        .flatten()
        {
            if msaa && !f.metal_caps().contains(FormatCaps::MSAA) {
                return Err(ModelError::FormatNotUsable {
                    format: f,
                    reason: "no multisample support",
                });
            }
        }
        if let Some(v) = &self.vertex_descriptor {
            v.validate(limits)?;
        }
        Ok(())
    }

    /// Whether depth and stencil are two different formats (two textures at draw time).
    #[must_use]
    pub fn separate_depth_stencil(&self) -> bool {
        matches!(
            (self.depth_attachment_pixel_format, self.stencil_attachment_pixel_format),
            (Some(d), Some(s)) if d != s
        )
    }

    /// Checks that the pipeline matches the render pass it is used in: same colour formats
    /// in every slot, same depth and stencil formats, same sample count (Metal validation
    /// rule; Vulkan dynamic rendering needs the same).
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidValue`] naming the mismatch.
    pub fn check_compatible(&self, pass: &RenderTargetLayout) -> Result<(), ModelError> {
        let mismatch = |reason| {
            Err(ModelError::InvalidValue {
                field: "render pipeline state",
                reason,
            })
        };
        for (p, r) in self.color_attachments.iter().zip(pass.color_formats.iter()) {
            if p.pixel_format != *r {
                return mismatch("colour attachment format differs from the render pass");
            }
        }
        if self.depth_attachment_pixel_format != pass.depth_format {
            return mismatch("depth format differs from the render pass");
        }
        if self.stencil_attachment_pixel_format != pass.stencil_format {
            return mismatch("stencil format differs from the render pass");
        }
        if self.raster_sample_count != pass.sample_count {
            return mismatch("sample count differs from the render pass");
        }
        if pass.layers > 0 && self.input_primitive_topology == PrimitiveTopologyClass::Unspecified {
            return mismatch("layered rendering needs an input primitive topology");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::vertex::{VertexAttributeDesc, VertexFormat, VertexStepFunction};

    const L: ResourceLimits = ResourceLimits::APPLE7;

    fn simple() -> RenderPipelineDesc {
        let mut p = RenderPipelineDesc::default();
        p.color_attachments[0].pixel_format = Some(PixelFormat::BGRA8Unorm);
        p
    }

    fn alpha_blend(fmt: PixelFormat) -> RenderPipelineColorAttachmentDesc {
        RenderPipelineColorAttachmentDesc {
            pixel_format: Some(fmt),
            blending_enabled: true,
            source_rgb_blend_factor: BlendFactor::SourceAlpha,
            destination_rgb_blend_factor: BlendFactor::OneMinusSourceAlpha,
            ..RenderPipelineColorAttachmentDesc::default()
        }
    }

    #[test]
    fn raw_values() {
        assert_eq!(BlendFactor::ALL.len(), 20);
        for (i, f) in BlendFactor::ALL.iter().enumerate() {
            assert_eq!(f.raw() as usize, i);
        }
        assert_eq!(
            BlendFactor::from_raw(20),
            Err(ModelError::UnknownBlendFactor(20))
        );
        assert_eq!(BlendOperation::from_raw(4), Ok(BlendOperation::Max));
        assert!(BlendOperation::from_raw(6).is_err());
        assert_eq!(
            PrimitiveTopologyClass::from_raw(3),
            Ok(PrimitiveTopologyClass::Triangle)
        );
        assert!(PrimitiveTopologyClass::from_raw(4).is_err());
        assert_eq!(ColorWriteMask::from_raw(0xf), Ok(ColorWriteMask::ALL));
        assert_eq!(ColorWriteMask::from_raw(0x8), Ok(ColorWriteMask::RED));
        assert!(ColorWriteMask::from_raw(0x10).unwrap_err().is_unsupported());
        assert_eq!(
            ColorWriteMask::from_raw(0x20),
            Err(ModelError::UnknownColorWriteMask(0x20))
        );
        assert!(ColorWriteMask::ALL.contains(ColorWriteMask::BLUE));
    }

    #[test]
    fn topology_classes() {
        use PrimitiveTopologyClass as C;
        use PrimitiveType as P;
        assert!(C::Unspecified.accepts(P::TriangleStrip));
        assert!(C::Triangle.accepts(P::TriangleStrip));
        assert!(C::Line.accepts(P::LineStrip));
        assert!(!C::Line.accepts(P::Point));
        assert!(C::Point.accepts(P::Point));
        assert!(!C::Point.accepts(P::Triangle));
    }

    #[test]
    fn valid_pipelines() {
        assert!(simple().validate(&L).is_ok());
        let mut p = simple();
        p.color_attachments[0] = alpha_blend(PixelFormat::RGBA16Float);
        p.color_attachments[3] = alpha_blend(PixelFormat::RGBA8Unorm_sRGB);
        p.color_attachments[4].pixel_format = Some(PixelFormat::R32Uint); // no blending
        p.depth_attachment_pixel_format = Some(PixelFormat::Depth32Float_Stencil8);
        p.stencil_attachment_pixel_format = Some(PixelFormat::Depth32Float_Stencil8);
        p.raster_sample_count = 4;
        p.alpha_to_coverage_enabled = true;
        p.input_primitive_topology = PrimitiveTopologyClass::Triangle;
        let mut v = VertexDescriptor::default();
        v.attributes[0] = Some(VertexAttributeDesc {
            format: VertexFormat::Float2,
            offset: 0,
            buffer_index: 0,
        });
        v.layouts[0].stride = crate::model::VertexStride::Fixed(8);
        p.vertex_descriptor = Some(v);
        assert!(p.validate(&L).is_ok());
        assert!(!p.separate_depth_stencil());
        // Dual-source blending on attachment 0 alone.
        let mut d = simple();
        d.color_attachments[0] = RenderPipelineColorAttachmentDesc {
            destination_rgb_blend_factor: BlendFactor::OneMinusSource1Color,
            ..alpha_blend(PixelFormat::RGBA8Unorm)
        };
        assert!(d.validate(&L).is_ok());
        // Separate depth and stencil formats are valid Metal.
        let mut s = simple();
        s.depth_attachment_pixel_format = Some(PixelFormat::Depth32Float);
        s.stencil_attachment_pixel_format = Some(PixelFormat::Stencil8);
        assert!(s.validate(&L).is_ok());
        assert!(s.separate_depth_stencil());
        // No colour attachment at all (depth-only pass).
        let z = RenderPipelineDesc {
            depth_attachment_pixel_format: Some(PixelFormat::Depth16Unorm),
            ..RenderPipelineDesc::default()
        };
        assert!(z.validate(&L).is_ok());
    }

    #[test]
    fn invalid_pipelines() {
        let mut cases: Vec<(RenderPipelineDesc, bool)> = Vec::new();
        let mut p = simple();
        p.raster_sample_count = 3;
        cases.push((p, false));
        let mut p = simple();
        p.raster_sample_count = 8; // above Apple7's 4
        cases.push((p, false));
        let mut p = simple();
        p.color_attachments[0] = alpha_blend(PixelFormat::RGBA8Uint); // integer blend
        cases.push((p, false));
        let mut p = simple();
        p.color_attachments[0] = alpha_blend(PixelFormat::R32Float); // blendable, fine
        p.color_attachments[1].pixel_format = Some(PixelFormat::BC1_RGBA); // not renderable
        cases.push((p, false));
        let mut p = simple();
        p.color_attachments[0].pixel_format = Some(PixelFormat::Depth32Float);
        cases.push((p, false));
        let mut p = simple();
        p.raster_sample_count = 4;
        p.color_attachments[0].pixel_format = Some(PixelFormat::RGB9E5Float); // MSAA ok
        p.depth_attachment_pixel_format = Some(PixelFormat::Stencil8); // not depth
        cases.push((p, false));
        let mut p = simple();
        p.depth_attachment_pixel_format = Some(PixelFormat::Depth32Float_Stencil8);
        p.stencil_attachment_pixel_format = Some(PixelFormat::Stencil8);
        cases.push((p, false));
        let mut p = simple();
        p.stencil_attachment_pixel_format = Some(PixelFormat::X32_Stencil8);
        cases.push((p, false));
        let mut p = simple();
        p.color_attachments[1] = RenderPipelineColorAttachmentDesc {
            source_rgb_blend_factor: BlendFactor::Source1Alpha,
            ..alpha_blend(PixelFormat::RGBA8Unorm)
        };
        cases.push((p, false));
        let mut p = simple();
        p.color_attachments[0] = RenderPipelineColorAttachmentDesc {
            source_alpha_blend_factor: BlendFactor::Source1Alpha,
            ..alpha_blend(PixelFormat::RGBA8Unorm)
        };
        p.color_attachments[1].pixel_format = Some(PixelFormat::RGBA8Unorm);
        cases.push((p, false));
        let mut p = simple();
        p.color_attachments[2].rgb_blend_operation = BlendOperation::Unspecialized;
        cases.push((p, true));
        let mut p = simple();
        p.max_vertex_amplification_count = 2;
        cases.push((p, true));
        let mut p = simple();
        let mut v = VertexDescriptor::default();
        v.attributes[0] = Some(VertexAttributeDesc {
            format: VertexFormat::Float,
            offset: 0,
            buffer_index: 0,
        });
        v.layouts[0].step_function = VertexStepFunction::PerInstance;
        v.layouts[0].step_rate = 0;
        p.vertex_descriptor = Some(v);
        cases.push((p, false));
        for (i, (p, unsupported)) in cases.iter().enumerate() {
            let e = p.validate(&L).expect_err(&format!("case {i}"));
            assert_eq!(e.is_unsupported(), *unsupported, "case {i}: {e:?}");
        }
        let small = ResourceLimits {
            max_color_attachments: 2,
            ..L
        };
        let mut p = simple();
        p.color_attachments[2].pixel_format = Some(PixelFormat::RGBA8Unorm);
        assert!(p.validate(&small).is_err());
    }

    #[test]
    fn compatibility_with_render_pass() {
        let p = simple();
        let mut layout = RenderTargetLayout {
            color_formats: [None; MAX_COLOR_ATTACHMENTS],
            depth_format: None,
            stencil_format: None,
            sample_count: 1,
            width: 64,
            height: 64,
            layers: 0,
            separate_depth_stencil: false,
            deferred_store_action: false,
            depth_resolve_min_max: false,
            ds_resolve_targets_differ: false,
            ds_resolve_one_aspect: false,
            ds_resolve_modes_differ: false,
        };
        layout.color_formats[0] = Some(PixelFormat::BGRA8Unorm);
        assert!(p.check_compatible(&layout).is_ok());
        let wrong_fmt = RenderTargetLayout {
            color_formats: {
                let mut c = layout.color_formats;
                c[0] = Some(PixelFormat::BGRA8Unorm_sRGB);
                c
            },
            ..layout
        };
        assert!(p.check_compatible(&wrong_fmt).is_err());
        assert!(
            p.check_compatible(&RenderTargetLayout {
                sample_count: 4,
                ..layout
            })
            .is_err()
        );
        assert!(
            p.check_compatible(&RenderTargetLayout {
                depth_format: Some(PixelFormat::Depth32Float),
                ..layout
            })
            .is_err()
        );
        assert!(
            p.check_compatible(&RenderTargetLayout {
                layers: 2,
                ..layout
            })
            .is_err()
        );
        let layered = RenderPipelineDesc {
            input_primitive_topology: PrimitiveTopologyClass::Triangle,
            ..p
        };
        assert!(
            layered
                .check_compatible(&RenderTargetLayout {
                    layers: 2,
                    ..layout
                })
                .is_ok()
        );
    }
}
