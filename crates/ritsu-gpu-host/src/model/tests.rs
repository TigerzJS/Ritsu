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
    for (i, s) in StoreAction::ALL.iter().enumerate() {
        assert_eq!(s.raw() as usize, i);
    }
    assert_eq!(
        StoreAction::from_raw(2),
        Ok(StoreAction::MultisampleResolve)
    );
    assert_eq!(StoreAction::from_raw(4), Ok(StoreAction::Unknown));
    assert_eq!(
        StoreAction::from_raw(6),
        Err(ModelError::UnknownStoreAction(6))
    );
    assert!(StoreAction::StoreAndMultisampleResolve.resolves());
    assert!(StoreAction::StoreAndMultisampleResolve.stores());
    assert!(!StoreAction::MultisampleResolve.stores());
    assert!(PrimitiveType::from_raw(5).is_err());
    assert!(TextureUsage::from_raw(0x8).is_err());
    assert!(TextureUsage::from_raw(0x37).is_ok());
    for (i, t) in TextureType::ALL.iter().enumerate() {
        assert_eq!(t.raw() as usize, i);
    }
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
    d.length = L.max_buffer_length;
    assert!(d.validate(&L).is_ok(), "limit itself is allowed");
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
    // Compressed volume textures (Apple3+), not for PVRTC.
    let astc3d = TextureDesc {
        texture_type: TextureType::Type3D,
        depth: 4,
        usage: TextureUsage::SHADER_READ,
        ..TextureDesc::new_2d(PixelFormat::ASTC_4x4_LDR, 64, 64, TextureUsage::SHADER_READ)
    };
    assert!(astc3d.validate(&L).is_ok());
    // Atomics on R32Sint (Apple6+), writes on RGB9E5 (Apple3+), stencil-only textures.
    for (fmt, usage) in [
        (
            PixelFormat::R32Sint,
            TextureUsage::SHADER_ATOMIC.union(TextureUsage::SHADER_WRITE),
        ),
        (PixelFormat::RGB9E5Float, TextureUsage::SHADER_WRITE),
        (PixelFormat::Stencil8, TextureUsage::RENDER_TARGET),
        (PixelFormat::GBGR422, TextureUsage::SHADER_READ),
        (PixelFormat::BGRA10_XR, RT_READ),
    ] {
        let t = TextureDesc::new_2d(fmt, 64, 64, usage);
        assert!(t.validate(&L).is_ok(), "{fmt:?}");
    }
    // Texture buffers with writable formats.
    let tb = TextureDesc {
        texture_type: TextureType::TypeTextureBuffer,
        height: 1,
        usage: TextureUsage::SHADER_WRITE.union(TextureUsage::SHADER_READ),
        ..TextureDesc::new_2d(PixelFormat::RG16Float, 4096, 1, TextureUsage::SHADER_READ)
    };
    assert!(tb.validate(&L).is_ok());
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
            // 8 samples exceed Apple7's limit of 4.
            TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 8,
                ..tex2d()
            },
            |e| *e == ModelError::InvalidSampleCount(8),
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
                pixel_format: PixelFormat::PVRTC_RGBA_4BPP,
                usage: TextureUsage::SHADER_READ,
                width: 8,
                height: 8,
                depth: 8,
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
            // RG32Uint atomics need Apple8.
            TextureDesc {
                pixel_format: PixelFormat::RG32Uint,
                usage: TextureUsage::SHADER_ATOMIC,
                ..tex2d()
            },
            |e| matches!(e, ModelError::UsageNotAllowedForFormat { .. }),
        ),
        (
            // Packed 16-bit formats are not writable.
            TextureDesc {
                pixel_format: PixelFormat::B5G6R5Unorm,
                usage: TextureUsage::SHADER_WRITE,
                ..tex2d()
            },
            |e| matches!(e, ModelError::UsageNotAllowedForFormat { .. }),
        ),
        (
            // Integer formats are not multisample-resolvable but can be MSAA; 4:2:2 cannot.
            TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 4,
                pixel_format: PixelFormat::GBGR422,
                usage: TextureUsage::SHADER_READ,
                ..tex2d()
            },
            |e| matches!(e, ModelError::FormatNotAllowedForType { .. }),
        ),
        (
            TextureDesc {
                pixel_format: PixelFormat::Depth24Unorm_Stencil8,
                ..tex2d()
            },
            |e| matches!(e, ModelError::FormatNotUsable { .. }),
        ),
        (
            TextureDesc {
                pixel_format: PixelFormat::X32_Stencil8,
                usage: TextureUsage::SHADER_READ,
                ..tex2d()
            },
            |e| matches!(e, ModelError::FormatNotUsable { .. }),
        ),
        (
            TextureDesc {
                pixel_format: PixelFormat::GBGR422,
                usage: TextureUsage::SHADER_READ,
                mip_level_count: 2,
                ..tex2d()
            },
            |e| matches!(e, ModelError::FormatNotUsable { .. }),
        ),
        (
            // Texture buffers: BGRA8Unorm is read-only, sRGB formats are not allowed at all.
            TextureDesc {
                texture_type: TextureType::TypeTextureBuffer,
                height: 1,
                usage: TextureUsage::SHADER_WRITE,
                ..tex2d()
            },
            |e| matches!(e, ModelError::UsageNotAllowedForFormat { .. }),
        ),
        (
            TextureDesc {
                texture_type: TextureType::TypeTextureBuffer,
                height: 1,
                pixel_format: PixelFormat::RGBA8Unorm_sRGB,
                usage: TextureUsage::SHADER_READ,
                ..tex2d()
            },
            |e| matches!(e, ModelError::FormatNotAllowedForType { .. }),
        ),
        (
            TextureDesc {
                texture_type: TextureType::TypeTextureBuffer,
                height: 1,
                pixel_format: PixelFormat::R32Float,
                usage: TextureUsage::RENDER_TARGET,
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
        (
            // Cube arrays count 6 layers per cube: 342 × 6 = 2052 > 2048.
            TextureDesc {
                texture_type: TextureType::TypeCubeArray,
                width: 16,
                height: 16,
                array_length: 342,
                ..tex2d()
            },
            |e| matches!(e, ModelError::DimensionTooLarge { .. }),
        ),
        (
            // Guest-controlled array length must not overflow the layer count.
            TextureDesc {
                texture_type: TextureType::TypeCubeArray,
                width: 16,
                height: 16,
                array_length: u32::MAX,
                ..tex2d()
            },
            |e| *e == ModelError::SizeOverflow,
        ),
    ];
    for (i, (desc, pred)) in cases.iter().enumerate() {
        let err = desc
            .validate(&L)
            .expect_err(&format!("case {i} should fail"));
        assert!(pred(&err), "case {i}: unexpected error {err:?}");
    }
}

/// Texture types per compression family (Apple's format pages) and the 4:2:2 rules.
#[test]
fn compressed_and_422_type_rules() {
    use TextureType as T;
    let read = TextureUsage::SHADER_READ;
    let desc = |fmt, ty: TextureType| {
        let (depth, array_length) = match ty {
            T::Type3D => (4, 1),
            T::Type2DArray | T::TypeCubeArray => (1, 2),
            _ => (1, 1),
        };
        TextureDesc {
            texture_type: ty,
            depth,
            array_length,
            ..TextureDesc::new_2d(fmt, 64, 64, read)
        }
    };
    let all = [
        T::Type2D,
        T::Type2DArray,
        T::TypeCube,
        T::TypeCubeArray,
        T::Type3D,
    ];
    for fmt in [
        PixelFormat::ETC2_RGB8,
        PixelFormat::EAC_R11Unorm,
        PixelFormat::EAC_RGBA8_sRGB,
        PixelFormat::PVRTC_RGBA_4BPP,
        PixelFormat::PVRTC_RGB_2BPP_sRGB,
    ] {
        for ty in all {
            let ok = matches!(ty, T::Type2D | T::Type2DArray | T::TypeCube);
            let r = desc(fmt, ty).validate(&L);
            assert_eq!(r.is_ok(), ok, "{fmt:?} {ty:?}: {r:?}");
            if !ok {
                assert!(matches!(r, Err(ModelError::FormatNotAllowedForType { .. })));
            }
        }
    }
    for fmt in [
        PixelFormat::BC1_RGBA,
        PixelFormat::BC6H_RGBUfloat,
        PixelFormat::ASTC_4x4_LDR,
        PixelFormat::ASTC_8x8_HDR,
    ] {
        for ty in all {
            assert!(desc(fmt, ty).validate(&L).is_ok(), "{fmt:?} {ty:?}");
        }
    }
    // 4:2:2: 2D only, one mip, even width.
    for ty in [T::Type2DArray, T::TypeCube, T::Type3D] {
        assert!(
            desc(PixelFormat::BGRG422, ty).validate(&L).is_err(),
            "{ty:?}"
        );
    }
    let odd = TextureDesc::new_2d(PixelFormat::GBGR422, 63, 64, read);
    assert!(matches!(
        odd.validate(&L),
        Err(ModelError::InvalidValue { field: "width", .. })
    ));
    let mipped = TextureDesc {
        mip_level_count: 2,
        ..TextureDesc::new_2d(PixelFormat::GBGR422, 64, 64, read)
    };
    assert!(matches!(
        mipped.validate(&L),
        Err(ModelError::FormatNotUsable { .. })
    ));
}

/// Texture buffers: own width limit (Apple7: 256 Mi pixels) and `maxBufferLength / pixel size`.
#[test]
fn texture_buffer_width_limits() {
    let tb = |fmt, width| TextureDesc {
        texture_type: TextureType::TypeTextureBuffer,
        ..TextureDesc::new_2d(fmt, width, 1, TextureUsage::SHADER_READ)
    };
    assert_eq!(L.max_texture_buffer_width, 256 << 20);
    // Far above the 1D texture limit (16384), which does not apply to texture buffers.
    assert!(tb(PixelFormat::R8Unorm, 16385).validate(&L).is_ok());
    assert!(tb(PixelFormat::R8Unorm, 256 << 20).validate(&L).is_ok());
    assert!(matches!(
        tb(PixelFormat::R8Unorm, (256 << 20) + 1).validate(&L),
        Err(ModelError::DimensionTooLarge { field: "width", .. })
    ));
    // 16-byte pixels: 4 GiB / 16 = 2^28 pixels, which equals the family limit.
    assert!(tb(PixelFormat::RGBA32Float, 1 << 28).validate(&L).is_ok());
    let small = ResourceLimits {
        max_buffer_length: 1 << 20,
        ..L
    };
    assert!(
        tb(PixelFormat::RGBA32Float, 1 << 16)
            .validate(&small)
            .is_ok()
    );
    assert!(
        tb(PixelFormat::RGBA32Float, (1 << 16) + 1)
            .validate(&small)
            .is_err()
    );
    assert!(tb(PixelFormat::RGBA32Float, u32::MAX).validate(&L).is_err());
}

#[test]
fn boundary_values() {
    // Exactly at the limits.
    let max2d = TextureDesc::new_2d(PixelFormat::R8Unorm, 16384, 16384, RT_READ);
    assert!(max2d.validate(&L).is_ok());
    let max3d = TextureDesc {
        texture_type: TextureType::Type3D,
        width: 2048,
        height: 2048,
        depth: 2048,
        usage: TextureUsage::SHADER_READ,
        ..max2d
    };
    assert!(max3d.validate(&L).is_ok());
    assert!(
        TextureDesc {
            depth: 2049,
            ..max3d
        }
        .validate(&L)
        .is_err()
    );
    let layers = TextureDesc {
        texture_type: TextureType::TypeCubeArray,
        width: 16,
        height: 16,
        array_length: 341, // 2046 layers
        ..max2d
    };
    assert!(layers.validate(&L).is_ok());
    // Full mip chain of a 1×1 texture is one level.
    let one = TextureDesc::new_2d(PixelFormat::R8Unorm, 1, 1, RT_READ);
    assert!(one.validate(&L).is_ok());
    assert!(
        TextureDesc {
            mip_level_count: 2,
            ..one
        }
        .validate(&L)
        .is_err()
    );
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
    // ASTC 10×8: 33×17 → 4×3 blocks of 16 bytes.
    let astc = TextureDesc::new_2d(
        PixelFormat::ASTC_10x8_LDR,
        33,
        17,
        TextureUsage::SHADER_READ,
    );
    assert_eq!(astc.size_in_bytes(), Ok(4 * 3 * 16));
    // PVRTC 2 bpp: at least 2×2 blocks of 8×4 pixels (16×8 pixels = 32 bytes).
    let pvrtc = TextureDesc::new_2d(
        PixelFormat::PVRTC_RGBA_2BPP,
        4,
        4,
        TextureUsage::SHADER_READ,
    );
    assert_eq!(pvrtc.size_in_bytes(), Ok(32));
    // 4:2:2: 6 pixels wide (even, as Apple requires) → 3 blocks of 4 bytes per row.
    let yuv = TextureDesc::new_2d(PixelFormat::GBGR422, 6, 2, TextureUsage::SHADER_READ);
    assert!(yuv.validate(&L).is_ok());
    assert_eq!(yuv.size_in_bytes(), Ok(24));
    // 3D mips shrink in depth too: 4×4×4 R8 with 3 levels = 64 + 8 + 1.
    let vol = TextureDesc {
        texture_type: TextureType::Type3D,
        depth: 4,
        mip_level_count: 3,
        ..TextureDesc::new_2d(PixelFormat::R8Unorm, 4, 4, RT_READ)
    };
    assert_eq!(vol.size_in_bytes(), Ok(73));
    // Multisample: × samples.
    let ms = TextureDesc {
        texture_type: TextureType::Type2DMultisample,
        sample_count: 4,
        ..TextureDesc::new_2d(PixelFormat::RGBA8Unorm, 2, 2, RT_READ)
    };
    assert_eq!(ms.size_in_bytes(), Ok(64));
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
fn level_extent_never_zero() {
    let t = TextureDesc::new_2d(PixelFormat::R8Unorm, 5, 3, RT_READ);
    assert_eq!(t.level_extent(0), (5, 3, 1));
    assert_eq!(t.level_extent(1), (2, 1, 1));
    assert_eq!(t.level_extent(2), (1, 1, 1));
    assert_eq!(t.level_extent(40), (1, 1, 1));
    assert_eq!(mip_extent(u32::MAX, 31), 1);
}

#[test]
fn limits_min_and_sample_counts() {
    let other = ResourceLimits {
        max_sample_count: 8,
        max_vertex_attributes: 29,
        ..ResourceLimits::APPLE7
    };
    let m = L.min(&other);
    assert_eq!(m.max_sample_count, 4);
    assert_eq!(m.max_vertex_attributes, 29);
    assert!(L.sample_count_ok(4));
    assert!(!L.sample_count_ok(8));
    assert!(!L.sample_count_ok(0));
    assert!(!other.sample_count_ok(16));
}

#[test]
fn model_error_converts_to_core_error() {
    let e: ritsu_core::Error = ModelError::ZeroLength.into();
    assert!(matches!(e, ritsu_core::Error::Guest(_)));
    let u: ritsu_core::Error = ModelError::Unsupported("x").into();
    assert!(matches!(u, ritsu_core::Error::Unimplemented("x")));
    let f: ritsu_core::Error = ModelError::FormatUnsupportedOnDevice {
        format: PixelFormat::ASTC_4x4_LDR,
        missing: FormatCaps::SAMPLE,
    }
    .into();
    assert!(matches!(f, ritsu_core::Error::Unimplemented(_)));
    assert!(!ModelError::ZeroLength.is_unsupported());
}
