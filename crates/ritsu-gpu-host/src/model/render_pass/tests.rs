use std::collections::HashMap;

use super::*;

const L: ResourceLimits = ResourceLimits::APPLE7;
const RT: TextureUsage = TextureUsage::RENDER_TARGET;

/// Texture table for the lookup closure: id → descriptor.
struct Textures(HashMap<u32, TextureDesc>);

impl Textures {
    fn new() -> Self {
        let mut m = HashMap::new();
        let rt = |fmt, w, h| TextureDesc::new_2d(fmt, w, h, RT.union(TextureUsage::SHADER_READ));
        m.insert(1, rt(PixelFormat::BGRA8Unorm, 64, 64));
        m.insert(2, rt(PixelFormat::RGBA16Float, 64, 64));
        m.insert(
            3,
            TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 4,
                usage: RT,
                storage_mode: StorageMode::Memoryless,
                ..rt(PixelFormat::BGRA8Unorm, 64, 64)
            },
        );
        m.insert(4, rt(PixelFormat::Depth32Float_Stencil8, 64, 64));
        m.insert(5, rt(PixelFormat::Depth32Float, 64, 64));
        m.insert(6, rt(PixelFormat::Stencil8, 64, 64));
        m.insert(
            7,
            TextureDesc::new_2d(PixelFormat::BGRA8Unorm, 64, 64, TextureUsage::SHADER_READ),
        );
        m.insert(
            8,
            TextureDesc {
                texture_type: TextureType::Type2DArray,
                array_length: 4,
                mip_level_count: 3,
                ..rt(PixelFormat::RGBA8Unorm, 64, 64)
            },
        );
        m.insert(9, rt(PixelFormat::BGRA8Unorm, 32, 32));
        m.insert(
            10,
            TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 4,
                ..rt(PixelFormat::Depth32Float, 64, 64)
            },
        );
        m.insert(11, rt(PixelFormat::RGBA8Uint, 64, 64));
        m.insert(
            12,
            TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 4,
                ..rt(PixelFormat::RGBA8Uint, 64, 64)
            },
        );
        m.insert(
            13,
            TextureDesc {
                texture_type: TextureType::Type3D,
                depth: 8,
                ..rt(PixelFormat::RGBA8Unorm, 64, 64)
            },
        );
        m.insert(
            14,
            TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 2,
                ..rt(PixelFormat::BGRA8Unorm, 64, 64)
            },
        );
        m.insert(
            15,
            TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 4,
                ..rt(PixelFormat::Depth32Float_Stencil8, 64, 64)
            },
        );
        m.insert(
            16,
            TextureDesc {
                texture_type: TextureType::Type2DArray,
                array_length: 4,
                ..rt(PixelFormat::BGRA8Unorm, 64, 64)
            },
        );
        m.insert(
            17,
            TextureDesc {
                mip_level_count: 2,
                ..rt(PixelFormat::Depth32Float_Stencil8, 128, 128)
            },
        );
        Self(m)
    }

    fn validate(&self, rp: &RenderPassDesc<u32>) -> Result<RenderTargetLayout, ModelError> {
        rp.validate(&L, |id| {
            self.0.get(&id).copied().ok_or(ModelError::InvalidValue {
                field: "texture",
                reason: "unknown",
            })
        })
    }
}

fn color(tex: u32) -> ColorAttachmentDesc<u32> {
    ColorAttachmentDesc {
        attachment: AttachmentDesc::new(tex, LoadAction::Clear, StoreAction::Store),
        clear_color: ClearColor::default(),
    }
}

fn depth(tex: u32) -> DepthAttachmentDesc<u32> {
    DepthAttachmentDesc {
        attachment: AttachmentDesc::new(tex, LoadAction::Clear, StoreAction::DontCare),
        clear_depth: 1.0,
        resolve_filter: MultisampleDepthResolveFilter::Sample0,
    }
}

fn stencil(tex: u32) -> StencilAttachmentDesc<u32> {
    StencilAttachmentDesc {
        attachment: AttachmentDesc::new(tex, LoadAction::Clear, StoreAction::DontCare),
        clear_stencil: 0,
        resolve_filter: MultisampleStencilResolveFilter::Sample0,
    }
}

fn pass(colors: &[(usize, ColorAttachmentDesc<u32>)]) -> RenderPassDesc<u32> {
    let mut rp = RenderPassDesc::empty();
    for (i, c) in colors {
        rp.color_attachments[*i] = Some(*c);
    }
    rp
}

fn resolve_to(tex: u32) -> Option<ResolveTarget<u32>> {
    Some(ResolveTarget {
        texture: tex,
        level: 0,
        slice: 0,
        depth_plane: 0,
    })
}

#[test]
fn raw_values() {
    assert_eq!(
        MultisampleDepthResolveFilter::from_raw(2),
        Ok(MultisampleDepthResolveFilter::Max)
    );
    assert!(MultisampleDepthResolveFilter::from_raw(3).is_err());
    assert_eq!(
        MultisampleStencilResolveFilter::from_raw(1),
        Ok(MultisampleStencilResolveFilter::DepthResolvedSample)
    );
    assert!(MultisampleStencilResolveFilter::from_raw(2).is_err());
    assert_eq!(
        StoreActionOptions::from_raw(1),
        Ok(StoreActionOptions::CUSTOM_SAMPLE_POSITIONS)
    );
    assert_eq!(
        StoreActionOptions::from_raw(2),
        Err(ModelError::UnknownStoreActionOptions(2))
    );
    assert_eq!(
        format!("{}", AttachmentSlot::Color(3)),
        "colour attachment 3"
    );
}

#[test]
fn simple_and_mrt_passes() {
    let t = Textures::new();
    let l = t.validate(&pass(&[(0, color(1))])).unwrap();
    assert_eq!(l.color_formats[0], Some(PixelFormat::BGRA8Unorm));
    assert_eq!(
        (l.width, l.height, l.sample_count, l.layers),
        (64, 64, 1, 0)
    );
    // Sparse colour slots, depth+stencil in one combined texture, render area from the
    // smallest attachment.
    let mut rp = pass(&[(0, color(1)), (5, color(2)), (7, color(9))]);
    rp.depth_attachment = Some(depth(4));
    rp.stencil_attachment = Some(stencil(4));
    let l = t.validate(&rp).unwrap();
    assert_eq!(l.color_formats[5], Some(PixelFormat::RGBA16Float));
    assert_eq!((l.width, l.height), (32, 32));
    assert!(!l.separate_depth_stencil);
    assert_eq!(l.depth_format, Some(PixelFormat::Depth32Float_Stencil8));
    // Separate depth and stencil textures are valid Metal; the layout reports it.
    let mut rp = pass(&[(0, color(1))]);
    rp.depth_attachment = Some(depth(5));
    rp.stencil_attachment = Some(stencil(6));
    assert!(t.validate(&rp).unwrap().separate_depth_stencil);
    // Explicit smaller render area.
    let mut rp = pass(&[(0, color(1))]);
    rp.render_target_width = 16;
    let l = t.validate(&rp).unwrap();
    assert_eq!((l.width, l.height), (16, 64));
    // Attachment-less pass.
    let mut rp = RenderPassDesc::empty();
    rp.render_target_width = 128;
    rp.render_target_height = 128;
    rp.default_raster_sample_count = 4;
    let l = t.validate(&rp).unwrap();
    assert_eq!((l.width, l.sample_count), (128, 4));
}

#[test]
fn msaa_resolve() {
    let t = Textures::new();
    // Memoryless MSAA colour resolved into a single-sample texture.
    let mut c = color(3);
    c.attachment.store = StoreAction::MultisampleResolve;
    c.attachment.resolve = resolve_to(1);
    let l = t.validate(&pass(&[(0, c)])).unwrap();
    assert_eq!(l.sample_count, 4);
    // Depth resolve with min filter: valid, flagged for the backend.
    let mut d = depth(15);
    d.attachment.store = StoreAction::StoreAndMultisampleResolve;
    d.attachment.resolve = resolve_to(4);
    d.resolve_filter = MultisampleDepthResolveFilter::Min;
    let mut rp = pass(&[(0, c)]);
    rp.depth_attachment = Some(d);
    let l = t.validate(&rp).unwrap();
    assert!(l.depth_resolve_min_max);
    // Stencil following the depth-resolved sample with a min/max depth filter: no Vulkan
    // equivalent → unsupported.
    let mut s = stencil(15);
    s.attachment.store = StoreAction::MultisampleResolve;
    s.attachment.resolve = resolve_to(4);
    s.resolve_filter = MultisampleStencilResolveFilter::DepthResolvedSample;
    rp.stencil_attachment = Some(s);
    assert!(t.validate(&rp).unwrap_err().is_unsupported());
    // …but fine with a sample-0 depth resolve.
    if let Some(d) = rp.depth_attachment.as_mut() {
        d.resolve_filter = MultisampleDepthResolveFilter::Sample0;
    }
    assert!(t.validate(&rp).is_ok());
}

/// Resolve of one combined depth/stencil texture: the layout reports what Vulkan dynamic
/// rendering can only express with one resolve target and (in)dependent resolve modes.
#[test]
fn combined_depth_stencil_resolve_flags() {
    let t = Textures::new();
    let target = |tex, level| {
        Some(ResolveTarget {
            texture: tex,
            level,
            slice: 0,
            depth_plane: 0,
        })
    };
    let ds_pass = |d_store, d_target, d_filter, s_store, s_target| {
        let mut rp = RenderPassDesc::empty();
        let mut d = depth(15);
        d.attachment.store = d_store;
        d.attachment.resolve = d_target;
        d.resolve_filter = d_filter;
        let mut s = stencil(15);
        s.attachment.store = s_store;
        s.attachment.resolve = s_target;
        rp.depth_attachment = Some(d);
        rp.stencil_attachment = Some(s);
        rp
    };
    let flags = |l: RenderTargetLayout| {
        (
            l.ds_resolve_targets_differ,
            l.ds_resolve_one_aspect,
            l.ds_resolve_modes_differ,
        )
    };
    let r = StoreAction::MultisampleResolve;
    let keep = StoreAction::DontCare;
    let s0 = MultisampleDepthResolveFilter::Sample0;
    // Same target, same mode: expressible everywhere.
    let l = t
        .validate(&ds_pass(r, target(4, 0), s0, r, target(4, 0)))
        .unwrap();
    assert_eq!(flags(l), (false, false, false));
    // Different resolve textures, or different subresources of one texture.
    let l = t
        .validate(&ds_pass(r, target(4, 0), s0, r, target(17, 0)))
        .unwrap();
    assert_eq!(flags(l), (true, false, false));
    let l = t
        .validate(&ds_pass(r, target(17, 0), s0, r, target(17, 1)))
        .unwrap();
    assert_eq!(flags(l), (true, false, false));
    // Only one aspect resolves.
    let l = t
        .validate(&ds_pass(r, target(4, 0), s0, keep, None))
        .unwrap();
    assert_eq!(flags(l), (false, true, false));
    let l = t
        .validate(&ds_pass(keep, None, s0, r, target(4, 0)))
        .unwrap();
    assert_eq!(flags(l), (false, true, false));
    // Depth min, stencil sample 0: different modes.
    let l = t
        .validate(&ds_pass(
            r,
            target(4, 0),
            MultisampleDepthResolveFilter::Min,
            r,
            target(4, 0),
        ))
        .unwrap();
    assert_eq!(flags(l), (false, false, true));
    assert!(l.depth_resolve_min_max);
    // Same target subresource for both aspects is not a duplicate binding.
    let l = t
        .validate(&ds_pass(r, target(17, 1), s0, r, target(17, 1)))
        .unwrap();
    assert_eq!(flags(l), (false, false, false));
}

#[test]
fn invalid_resolves() {
    let t = Textures::new();
    let resolving = |src: u32, dst: Option<u32>| {
        let mut c = color(src);
        c.attachment.store = StoreAction::MultisampleResolve;
        c.attachment.resolve = dst.and_then(resolve_to);
        pass(&[(0, c)])
    };
    let cases = [
        resolving(3, None),      // no resolve texture
        resolving(1, Some(9)),   // single-sample source
        resolving(3, Some(2)),   // different format
        resolving(3, Some(14)),  // multisampled destination
        resolving(3, Some(7)),   // destination without renderTarget usage
        resolving(3, Some(9)),   // destination smaller than the render area
        resolving(12, Some(11)), // integer formats are not resolvable
        resolving(3, Some(3)),   // memoryless destination
    ];
    for (i, rp) in cases.iter().enumerate() {
        let e = t.validate(rp).expect_err(&format!("case {i}"));
        assert!(
            matches!(e, ModelError::InvalidAttachment { .. }),
            "case {i}: {e:?}"
        );
    }
    // Resolve target out of range (guest-controlled level/slice/plane).
    for (level, slice, depth_plane) in [(0, u32::MAX, 0), (0, 4, 0), (u32::MAX, 0, 0), (0, 0, 1)] {
        let mut c = color(3);
        c.attachment.store = StoreAction::MultisampleResolve;
        c.attachment.resolve = Some(ResolveTarget {
            texture: 16,
            level,
            slice,
            depth_plane,
        });
        let e = t.validate(&pass(&[(0, c)])).unwrap_err();
        assert!(matches!(e, ModelError::InvalidAttachment { .. }), "{e:?}");
        c.attachment.resolve = Some(ResolveTarget {
            texture: 16,
            level: 0,
            slice: 3,
            depth_plane: 0,
        });
        assert!(t.validate(&pass(&[(0, c)])).is_ok());
    }
    // Resolve texture set but store action does not resolve: ignored (Metal semantics).
    let mut c = color(1);
    c.attachment.resolve = resolve_to(2);
    assert!(t.validate(&pass(&[(0, c)])).is_ok());
}

#[test]
fn invalid_attachments() {
    let t = Textures::new();
    let mut cases: Vec<RenderPassDesc<u32>> = Vec::new();
    cases.push(pass(&[(0, color(7))])); // no renderTarget usage
    cases.push(pass(&[(0, color(5))])); // depth format in a colour slot
    cases.push({
        let mut rp = pass(&[(0, color(1))]);
        rp.depth_attachment = Some(depth(1)); // colour format as depth
        rp
    });
    cases.push({
        let mut rp = pass(&[(0, color(1))]);
        rp.stencil_attachment = Some(stencil(5)); // depth-only format as stencil
        rp
    });
    cases.push(pass(&[(0, color(1)), (1, color(1))])); // bound twice
    cases.push(pass(&[(0, color(1)), (1, color(3))])); // sample counts differ
    cases.push({
        let mut c = color(1);
        c.attachment.level = 1; // texture has one level
        pass(&[(0, c)])
    });
    cases.push({
        let mut c = color(8);
        c.attachment.slice = 4; // array has 4 slices
        pass(&[(0, c)])
    });
    cases.push({
        let mut c = color(1);
        c.attachment.depth_plane = 1; // not a 3D texture
        pass(&[(0, c)])
    });
    cases.push({
        let mut c = color(13);
        c.attachment.depth_plane = 8; // 3D depth is 8
        pass(&[(0, c)])
    });
    cases.push({
        let mut c = color(13);
        c.attachment.slice = 1; // 3D textures have no slices
        pass(&[(0, c)])
    });
    cases.push({
        let mut c = color(3);
        c.attachment.load = LoadAction::Load; // memoryless
        pass(&[(0, c)])
    });
    cases.push(pass(&[(0, color(3))])); // memoryless + store
    cases.push({
        let mut rp = pass(&[(0, color(1))]);
        rp.render_target_width = 65; // larger than the attachment
        rp
    });
    for (i, rp) in cases.iter().enumerate() {
        let e = t.validate(rp).expect_err(&format!("case {i}"));
        assert!(!e.is_unsupported(), "case {i}: {e:?}");
    }
    // Same texture, different mip levels: allowed (different subresources).
    let mut a = color(8);
    a.attachment.level = 1;
    let mut b = color(8);
    b.attachment.level = 2;
    assert!(
        t.validate(&pass(&[(0, a), (1, b)]))
            .is_ok_and(|l| l.width == 16)
    );
    // Unknown texture: the lookup error is passed through.
    assert!(t.validate(&pass(&[(0, color(99))])).is_err());
    // Colour slot beyond the reported limit.
    let small = ResourceLimits {
        max_color_attachments: 4,
        ..L
    };
    let rp = pass(&[(4, color(1))]);
    let e = rp.validate(&small, |id| {
        t.0.get(&id).copied().ok_or(ModelError::ZeroExtent)
    });
    assert!(matches!(e, Err(ModelError::InvalidAttachment { .. })));
}

#[test]
fn attachment_less_passes() {
    let t = Textures::new();
    let mut rp: RenderPassDesc<u32> = RenderPassDesc::empty();
    assert!(t.validate(&rp).is_err(), "no render area");
    rp.render_target_width = 16;
    assert!(t.validate(&rp).is_err(), "height missing");
    rp.render_target_height = 16;
    assert!(t.validate(&rp).is_ok());
    rp.default_raster_sample_count = 3;
    assert!(matches!(
        t.validate(&rp),
        Err(ModelError::InvalidSampleCount(3))
    ));
    rp.default_raster_sample_count = 8;
    assert!(t.validate(&rp).is_err(), "above Apple7's 4 samples");
    rp.default_raster_sample_count = 1;
    rp.render_target_width = 16385;
    assert!(t.validate(&rp).is_err());
}

#[test]
fn layered_rendering() {
    let t = Textures::new();
    let mut rp = pass(&[(0, color(8))]);
    rp.render_target_array_length = 4;
    assert_eq!(t.validate(&rp).unwrap().layers, 4);
    rp.render_target_array_length = 5;
    assert!(t.validate(&rp).is_err(), "more layers than slices");
    rp.render_target_array_length = 2;
    if let Some(c) = rp.color_attachments[0].as_mut() {
        c.attachment.slice = 3;
    }
    assert!(t.validate(&rp).is_err(), "slice + layers out of range");
    // 3D textures layer over depth planes.
    let mut rp = pass(&[(0, color(13))]);
    rp.render_target_array_length = 8;
    assert!(t.validate(&rp).is_ok());
    // Plain 2D textures cannot be layered.
    let mut rp = pass(&[(0, color(1))]);
    rp.render_target_array_length = 1;
    assert!(t.validate(&rp).is_err());
    // Guest-controlled huge values do not overflow.
    let mut rp = pass(&[(0, color(8))]);
    rp.render_target_array_length = 2048;
    if let Some(c) = rp.color_attachments[0].as_mut() {
        c.attachment.slice = u32::MAX;
    }
    assert!(t.validate(&rp).is_err());
    rp.render_target_array_length = 2049;
    assert!(t.validate(&rp).is_err(), "above the layer limit");
}

#[test]
fn store_actions_and_options() {
    let t = Textures::new();
    let mut c = color(1);
    c.attachment.store = StoreAction::Unknown;
    let rp = pass(&[(0, c)]);
    assert!(t.validate(&rp).unwrap().deferred_store_action);
    assert!(rp.has_deferred_store_action());
    assert!(!pass(&[(0, color(1))]).has_deferred_store_action());
    let mut c = color(1);
    c.attachment.store_options = StoreActionOptions::CUSTOM_SAMPLE_POSITIONS;
    assert!(t.validate(&pass(&[(0, c)])).unwrap_err().is_unsupported());
    let mut rp = pass(&[(0, color(14))]);
    let mut d = depth(5);
    d.attachment.store = StoreAction::CustomSampleDepthStore;
    rp.depth_attachment = Some(d);
    rp.color_attachments[0] = Some(color(1));
    assert!(t.validate(&rp).unwrap_err().is_unsupported());
}

#[test]
fn clear_values() {
    let t = Textures::new();
    let mut rp = pass(&[]);
    rp.depth_attachment = Some(depth(5));
    assert!(t.validate(&rp).is_ok());
    for (v, unsupported) in [(f64::NAN, false), (1.5, true), (-0.1, true)] {
        if let Some(d) = rp.depth_attachment.as_mut() {
            d.clear_depth = v;
        }
        let e = t.validate(&rp).unwrap_err();
        assert_eq!(e.is_unsupported(), unsupported, "{v}: {e:?}");
    }
    // Ignored unless the load action clears.
    if let Some(d) = rp.depth_attachment.as_mut() {
        d.attachment.load = LoadAction::DontCare;
    }
    assert!(t.validate(&rp).is_ok());

    let c = ClearColor {
        red: 0.5,
        green: -1.0,
        blue: 300.7,
        alpha: f64::NAN,
    };
    match c.for_format(PixelFormat::RGBA8Unorm) {
        ClearValue::Float([r, g, b, a]) => {
            assert_eq!((r, g, b), (0.5, -1.0, 300.7_f64 as f32));
            assert!(a.is_nan());
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        c.for_format(PixelFormat::BGRA10_XR),
        ClearValue::Float(_)
    ));
    assert_eq!(
        c.for_format(PixelFormat::RGBA8Uint),
        ClearValue::Uint([0, 0, 300, 0])
    );
    assert_eq!(
        c.for_format(PixelFormat::R32Sint),
        ClearValue::Sint([0, -1, 300, 0])
    );
    let big = ClearColor {
        red: 1e20,
        green: -1e20,
        blue: 0.0,
        alpha: 1.0,
    };
    assert_eq!(
        big.for_format(PixelFormat::RGBA32Sint),
        ClearValue::Sint([i32::MAX, i32::MIN, 0, 1])
    );
    assert_eq!(ClearColor::default().to_array(), [0.0, 0.0, 0.0, 1.0]);
}
