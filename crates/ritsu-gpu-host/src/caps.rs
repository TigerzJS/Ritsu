//! Device capabilities and the feature set Ritsu reports to the guest.
//!
//! A backend describes what its Vulkan device can do in [`Capabilities`]. From that (and an
//! explicit [`EmulationPolicy`]), [`reported_feature_set`] derives [`ReportedFeatures`]: the
//! Metal GPU families and optional features the guest is told about. Rule: a feature is only
//! reported if it is either native or *implemented* as an emulation and enabled by policy —
//! never silently wrong (CLAUDE.md §3.5). See `docs/subsystems/gpu.md` §5.
//!
//! Per-format capabilities are kept apart from the formats themselves: the model says what
//! Metal guarantees for a format ([`PixelFormat::metal_caps`]), [`Capabilities::formats`] says
//! what the GPU offers natively, and [`format_support`] / [`texture_support`] compare the two
//! (plus enabled emulations). The same split exists for vertex input
//! ([`vertex_input_support`]), render passes ([`render_pass_support`]) and render pipelines
//! ([`render_pipeline_support`]).

use crate::model::{
    Compression, FormatCaps, ModelError, PixelFormat, RenderPipelineDesc, RenderTargetLayout,
    ResourceLimits, TextureDesc, TextureType, TextureUsage, VertexDescriptor, VertexStepFunction,
    VertexStride,
};

/// GPU vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GpuVendor {
    /// NVIDIA.
    Nvidia,
    /// Intel.
    Intel,
    /// Software rasterizer or other vendor.
    Other,
}

/// Vulkan driver family; capabilities differ per driver for the same GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DriverKind {
    /// NVIDIA proprietary driver.
    NvidiaProprietary,
    /// Mesa NVK.
    Nvk,
    /// Mesa ANV (Intel).
    Anv,
    /// Mesa lavapipe (software, CI).
    Lavapipe,
    /// Anything else.
    Other,
}

/// Vulkan API version (major.minor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VulkanVersion {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
}

impl VulkanVersion {
    /// Vulkan 1.3, Ritsu's minimum (ADR-0002).
    pub const V1_3: Self = Self { major: 1, minor: 3 };
}

/// What a Vulkan device (driver + GPU) offers, as relevant for Metal translation.
///
/// Field names follow the Vulkan feature/extension they come from (see gpu.md §3).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // a capability matrix is a set of flags by nature
pub struct Capabilities {
    /// Human-readable device name.
    pub device_name: String,
    /// Vendor.
    pub vendor: GpuVendor,
    /// Driver family.
    pub driver: DriverKind,
    /// Supported Vulkan API version.
    pub api_version: VulkanVersion,
    /// Timeline semaphores (core 1.2).
    pub timeline_semaphore: bool,
    /// `bufferDeviceAddress` (core 1.2).
    pub buffer_device_address: bool,
    /// Descriptor indexing incl. update-after-bind and partially bound (core 1.2).
    pub descriptor_indexing: bool,
    /// Dynamic rendering (core 1.3).
    pub dynamic_rendering: bool,
    /// `synchronization2` (core 1.3).
    pub synchronization2: bool,
    /// `VK_EXT_descriptor_buffer`.
    pub descriptor_buffer: bool,
    /// `VK_KHR_dynamic_rendering_local_read` (framebuffer fetch).
    pub dynamic_rendering_local_read: bool,
    /// `VK_EXT_fragment_shader_interlock` (raster order groups).
    pub fragment_shader_interlock: bool,
    /// `VK_EXT_mesh_shader`.
    pub mesh_shader: bool,
    /// `VK_KHR_acceleration_structure`.
    pub acceleration_structure: bool,
    /// `VK_KHR_ray_query`.
    pub ray_query: bool,
    /// `VK_KHR_ray_tracing_pipeline`.
    pub ray_tracing_pipeline: bool,
    /// `subgroupSizeControl` (core 1.3 feature).
    pub subgroup_size_control: bool,
    /// Smallest selectable subgroup size.
    pub min_subgroup_size: u32,
    /// Largest selectable subgroup size.
    pub max_subgroup_size: u32,
    /// `shaderFloat16`.
    pub shader_float16: bool,
    /// `shaderInt8`.
    pub shader_int8: bool,
    /// `VK_EXT_shader_atomic_float`: `shaderBufferFloat32AtomicAdd`.
    pub buffer_float32_atomic_add: bool,
    /// `textureCompressionBC`.
    pub texture_compression_bc: bool,
    /// `textureCompressionASTC_LDR`.
    pub texture_compression_astc_ldr: bool,
    /// `VK_EXT_custom_border_color`.
    pub custom_border_color: bool,
    /// `VK_EXT_device_generated_commands` (indirect command buffers).
    pub device_generated_commands: bool,
    /// GPU and CPU share physical memory (integrated GPU).
    pub unified_memory: bool,
    /// Resizable BAR: (almost) all VRAM is host-visible.
    pub resizable_bar: bool,
    /// Size of the device-local heap(s) usable by Ritsu, in bytes.
    pub device_local_bytes: u64,
    /// Bytes of memory that are both device-local and host-visible.
    pub host_visible_device_local_bytes: u64,
    /// Resource limits of the device (before clamping to the reported Metal family).
    pub limits: ResourceLimits,
    /// Native capabilities per Metal pixel format (Vulkan format features of the format the
    /// backend maps it to).
    pub formats: FormatCapsTable,
    /// Vertex input limits and relaxations.
    pub vertex_input: VertexInputCaps,
    /// `VK_RESOLVE_MODE_MIN_BIT`/`MAX_BIT` for depth resolves (`supportedDepthResolveModes`).
    pub depth_resolve_min_max: bool,
    /// `independentResolveNone`: one aspect of a depth/stencil attachment may resolve while the
    /// other does not.
    pub independent_resolve_none: bool,
    /// `independentResolve`: depth and stencil may resolve with different modes.
    pub independent_resolve: bool,
}

/// Native capabilities of a device per [`PixelFormat`].
#[derive(Clone, PartialEq, Eq)]
pub struct FormatCapsTable([FormatCaps; PixelFormat::COUNT]);

impl FormatCapsTable {
    /// Builds the table from a function.
    pub fn from_fn(mut f: impl FnMut(PixelFormat) -> FormatCaps) -> Self {
        let mut t = [FormatCaps::NONE; PixelFormat::COUNT];
        for p in PixelFormat::ALL {
            t[p.index()] = f(*p);
        }
        Self(t)
    }

    /// Capabilities of `format`.
    #[must_use]
    pub fn get(&self, format: PixelFormat) -> FormatCaps {
        self.0[format.index()]
    }

    /// Replaces the capabilities of `format` (used when filling from a real device).
    pub fn set(&mut self, format: PixelFormat, caps: FormatCaps) {
        self.0[format.index()] = caps;
    }

    /// **Assumed** native format capabilities of the desktop target GPUs.
    ///
    /// \[Annahme\] Not measured: a reading of the Vulkan required-format tables plus
    /// general knowledge of the NVIDIA (Ada) and Intel (Xe-LP, ANV) drivers. WP-GPU-03
    /// replaces it with `vkGetPhysicalDeviceFormatProperties` results. Notable gaps assumed
    /// on both GPUs: no ETC2/EAC, PVRTC or ASTC sampling; no storage writes to sRGB formats;
    /// `RGB9E5Float` only sampled (no render target); `R8Unorm_sRGB`/`RG8Unorm_sRGB`,
    /// `A8Unorm`, `A1BGR5Unorm`, `ABGR4Unorm` only sampled; no counterpart for the
    /// extended-range `*_XR` formats; no sparse residency reported.
    #[must_use]
    pub fn assumed_desktop(vendor: GpuVendor, bc: bool, astc_ldr: bool) -> Self {
        use PixelFormat as P;
        let c = |list: &[FormatCaps]| list.iter().fold(FormatCaps::NONE, |a, b| a.union(*b));
        let sf = c(&[FormatCaps::SAMPLE, FormatCaps::FILTER]);
        let render = c(&[
            sf,
            FormatCaps::COLOR,
            FormatCaps::BLEND,
            FormatCaps::MSAA,
            FormatCaps::RESOLVE,
        ]);
        let full = render.union(FormatCaps::WRITE);
        let int = c(&[
            FormatCaps::SAMPLE,
            FormatCaps::COLOR,
            FormatCaps::MSAA,
            FormatCaps::WRITE,
        ]);
        let ds = c(&[FormatCaps::SAMPLE, FormatCaps::MSAA, FormatCaps::RESOLVE]);
        let nvidia = vendor == GpuVendor::Nvidia;
        Self::from_fn(|f| match f.compression() {
            Compression::Bc if bc => sf,
            Compression::Astc if astc_ldr => sf,
            Compression::None => match f {
                P::Depth16Unorm | P::Depth32Float => {
                    c(&[ds, FormatCaps::FILTER, FormatCaps::DEPTH])
                }
                P::Stencil8 => ds.union(FormatCaps::STENCIL),
                P::Depth32Float_Stencil8 => c(&[
                    ds,
                    FormatCaps::FILTER,
                    FormatCaps::DEPTH,
                    FormatCaps::STENCIL,
                ]),
                P::X32_Stencil8 => c(&[FormatCaps::SAMPLE, FormatCaps::MSAA]),
                // D24S8 exists on NVIDIA, not on Intel; Metal never asks for it on Apple GPUs.
                P::Depth24Unorm_Stencil8 if nvidia => c(&[
                    ds,
                    FormatCaps::FILTER,
                    FormatCaps::DEPTH,
                    FormatCaps::STENCIL,
                ]),
                P::X24_Stencil8 if nvidia => c(&[FormatCaps::SAMPLE, FormatCaps::MSAA]),
                P::Depth24Unorm_Stencil8 | P::X24_Stencil8 => FormatCaps::NONE,
                P::BGR10_XR | P::BGR10_XR_sRGB | P::BGRA10_XR | P::BGRA10_XR_sRGB => {
                    FormatCaps::NONE
                }
                P::GBGR422
                | P::BGRG422
                | P::RGB9E5Float
                | P::R8Unorm_sRGB
                | P::RG8Unorm_sRGB
                | P::A8Unorm
                | P::A1BGR5Unorm
                | P::ABGR4Unorm => sf,
                P::B5G6R5Unorm | P::BGR5A1Unorm => render,
                P::R32Uint | P::R32Sint => int.union(FormatCaps::ATOMIC),
                _ if f.numeric_type().is_integer() => int,
                _ if f.is_srgb() => render,
                _ => full,
            },
            _ => FormatCaps::NONE,
        })
    }
}

impl core::fmt::Debug for FormatCapsTable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map()
            .entries(PixelFormat::ALL.iter().map(|p| (p, self.get(*p))))
            .finish()
    }
}

/// Vertex-input properties of a device that Metal's vertex descriptors can exceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct VertexInputCaps {
    /// `maxVertexInputBindingStride` (Metal has no stride limit).
    pub max_binding_stride: u32,
    /// `maxVertexInputAttributeOffset`.
    pub max_attribute_offset: u32,
    /// Unaligned attribute offsets/strides (`VK_EXT_legacy_vertex_attributes` with dynamic
    /// vertex input; core Vulkan needs component alignment, Metal on Apple7 needs 1 byte).
    pub unaligned_fetch: bool,
    /// Instance step rates other than 1 (`vertexAttributeInstanceRateDivisor`).
    pub instance_divisor: bool,
    /// Constant step function (`vertexAttributeInstanceRateZeroDivisor`).
    pub zero_divisor: bool,
}

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

impl Capabilities {
    /// Assumed capabilities of the RTX 4050 Laptop (AD107) with the NVIDIA proprietary
    /// driver (≥ 560).
    ///
    /// **Assumption, to verify on hardware (WP-GPU-03, `vulkaninfo` on the G5 MF):** every
    /// value here comes from public driver documentation and general knowledge of the Ada
    /// generation, not from a measurement. ReBAR depends on BIOS settings. NVK exposes fewer
    /// features (no ray tracing at the time of writing). Format table:
    /// [`FormatCapsTable::assumed_desktop`]; vertex input (2048-byte stride, unaligned fetch via
    /// `VK_EXT_legacy_vertex_attributes`, divisors), min/max depth resolve and
    /// independent depth/stencil resolve are assumed too; `maxTexelBufferElements` = 2^27.
    #[must_use]
    pub fn rtx4050_assumed() -> Self {
        Self {
            device_name: "NVIDIA GeForce RTX 4050 Laptop GPU (assumed)".to_owned(),
            vendor: GpuVendor::Nvidia,
            driver: DriverKind::NvidiaProprietary,
            api_version: VulkanVersion { major: 1, minor: 3 },
            timeline_semaphore: true,
            buffer_device_address: true,
            descriptor_indexing: true,
            dynamic_rendering: true,
            synchronization2: true,
            descriptor_buffer: true,
            dynamic_rendering_local_read: true,
            fragment_shader_interlock: true,
            mesh_shader: true,
            acceleration_structure: true,
            ray_query: true,
            ray_tracing_pipeline: true,
            subgroup_size_control: true,
            min_subgroup_size: 32,
            max_subgroup_size: 32,
            shader_float16: true,
            shader_int8: true,
            buffer_float32_atomic_add: true,
            texture_compression_bc: true,
            texture_compression_astc_ldr: false,
            custom_border_color: true,
            device_generated_commands: true,
            unified_memory: false,
            resizable_bar: true,
            device_local_bytes: 6 * GIB,
            host_visible_device_local_bytes: 6 * GIB,
            limits: ResourceLimits {
                max_buffer_length: 4 * GIB,
                max_texture_dimension_1d: 32768,
                max_texture_dimension_2d: 32768,
                max_texture_dimension_3d: 16384,
                max_texture_array_layers: 2048,
                max_threads_per_threadgroup: 1024,
                max_color_attachments: 8,
                max_sample_count: 8,
                max_vertex_attributes: 32,
                max_vertex_buffers: 32,
                // `maxTexelBufferElements` [Annahme: 2^27].
                max_texture_buffer_width: 1 << 27,
            },
            formats: FormatCapsTable::assumed_desktop(GpuVendor::Nvidia, true, false),
            vertex_input: VertexInputCaps {
                max_binding_stride: 2048,
                max_attribute_offset: 2047,
                unaligned_fetch: true,
                instance_divisor: true,
                zero_divisor: true,
            },
            depth_resolve_min_max: true,
            independent_resolve_none: true,
            independent_resolve: true,
        }
    }

    /// Assumed capabilities of the Iris Xe (Alder Lake, Xe-LP Gen12) with Mesa ANV (≥ 24.x).
    ///
    /// **Assumption, to verify on hardware (WP-GPU-03):** no hardware ray tracing, no mesh
    /// shaders, no ASTC (removed in Gen12), no float32 atomic add on buffers (needs the LSC
    /// of Xe-HPG). Descriptor buffer and local read depend on the Mesa version. Device
    /// memory is system RAM; the budget below assumes 16 GiB RAM in the G5 MF.
    #[must_use]
    pub fn iris_xe_assumed() -> Self {
        Self {
            device_name: "Intel Iris Xe Graphics (ADL GT2) (assumed)".to_owned(),
            vendor: GpuVendor::Intel,
            driver: DriverKind::Anv,
            api_version: VulkanVersion { major: 1, minor: 3 },
            timeline_semaphore: true,
            buffer_device_address: true,
            descriptor_indexing: true,
            dynamic_rendering: true,
            synchronization2: true,
            descriptor_buffer: true,
            dynamic_rendering_local_read: true,
            fragment_shader_interlock: true,
            mesh_shader: false,
            acceleration_structure: false,
            ray_query: false,
            ray_tracing_pipeline: false,
            subgroup_size_control: true,
            min_subgroup_size: 8,
            max_subgroup_size: 32,
            shader_float16: true,
            shader_int8: true,
            buffer_float32_atomic_add: false,
            texture_compression_bc: true,
            texture_compression_astc_ldr: false,
            custom_border_color: true,
            device_generated_commands: false,
            unified_memory: true,
            resizable_bar: false,
            device_local_bytes: 8 * GIB,
            host_visible_device_local_bytes: 8 * GIB,
            limits: ResourceLimits {
                max_buffer_length: 2 * GIB,
                max_texture_dimension_1d: 16384,
                max_texture_dimension_2d: 16384,
                max_texture_dimension_3d: 2048,
                max_texture_array_layers: 2048,
                max_threads_per_threadgroup: 1024,
                max_color_attachments: 8,
                max_sample_count: 8,
                // ANV exposes fewer vertex elements/buffers than NVIDIA [Annahme: 29/28].
                max_vertex_attributes: 29,
                max_vertex_buffers: 28,
                // `maxTexelBufferElements` [Annahme: 2^27].
                max_texture_buffer_width: 1 << 27,
            },
            formats: FormatCapsTable::assumed_desktop(GpuVendor::Intel, true, false),
            vertex_input: VertexInputCaps {
                max_binding_stride: 2048,
                max_attribute_offset: 2047,
                unaligned_fetch: true,
                instance_divisor: true,
                zero_divisor: true,
            },
            depth_resolve_min_max: true,
            independent_resolve_none: true,
            independent_resolve: true,
        }
    }

    /// Vulkan baseline items this device lacks (empty = usable by Ritsu).
    ///
    /// The baseline is Vulkan 1.3 with timeline semaphores, buffer device address,
    /// descriptor indexing, dynamic rendering and synchronization2 (gpu.md §3.1). Devices
    /// that miss any of these are rejected with `Error::Host`, never half-used.
    #[must_use]
    pub fn missing_baseline(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.api_version < VulkanVersion::V1_3 {
            missing.push("Vulkan 1.3");
        }
        for (ok, name) in [
            (self.timeline_semaphore, "timelineSemaphore"),
            (self.buffer_device_address, "bufferDeviceAddress"),
            (self.descriptor_indexing, "descriptorIndexing"),
            (self.dynamic_rendering, "dynamicRendering"),
            (self.synchronization2, "synchronization2"),
        ] {
            if !ok {
                missing.push(name);
            }
        }
        missing
    }

    /// Whether host-visible device-local memory is large enough to place all `Shared`
    /// buffers there directly (ReBAR or integrated GPU), see gpu.md §6.
    #[must_use]
    pub fn shared_buffers_direct(&self) -> bool {
        self.unified_memory
            || (self.resizable_bar && self.host_visible_device_local_bytes > 256 * MIB)
    }
}

/// How a Metal feature reaches the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Support {
    /// Mapped to a native Vulkan feature.
    Native,
    /// Implemented by Ritsu on top of other features (slower, but correct).
    Emulated,
    /// Not reported to the guest.
    Unsupported,
}

impl Support {
    /// Whether the guest is told the feature exists.
    #[must_use]
    pub const fn is_reported(self) -> bool {
        !matches!(self, Self::Unsupported)
    }

    const fn native_or(native: bool, emulated: bool) -> Self {
        if native {
            Self::Native
        } else if emulated {
            Self::Emulated
        } else {
            Self::Unsupported
        }
    }
}

/// Which emulations are enabled. An emulation may only be enabled once it is implemented
/// and covered by image-comparison tests; the default enables none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct EmulationPolicy {
    /// Compute-based BVH traversal when `ray_query` is missing.
    pub ray_tracing: bool,
    /// Object/mesh stages via compute + indirect draw when `mesh_shader` is missing.
    pub mesh_shaders: bool,
    /// ASTC (LDR → RGBA8, HDR → RGBA16Float) transcoding at upload time.
    pub astc_transcode: bool,
    /// ETC2/EAC → RGBA8/R16/RG16 transcoding at upload time.
    pub etc_eac_transcode: bool,
    /// PVRTC → RGBA8 transcoding at upload time.
    pub pvrtc_transcode: bool,
    /// Tile shaders / imageblocks via pass splitting and storage images.
    pub tile_shaders: bool,
    /// `atomic<float>` via compare-exchange loops in translated shaders.
    pub float_atomics_cas: bool,
    /// Expose the Metal 4 API family (MTL4 command allocators, argument tables, …).
    pub metal4: bool,
}

impl EmulationPolicy {
    /// No emulation at all: only native features are reported.
    pub const NATIVE_ONLY: Self = Self {
        ray_tracing: false,
        mesh_shaders: false,
        astc_transcode: false,
        etc_eac_transcode: false,
        pvrtc_transcode: false,
        tile_shaders: false,
        float_atomics_cas: false,
        metal4: false,
    };

    /// Every emulation enabled (target state of M7; used in tests and experiments).
    pub const ALL: Self = Self {
        ray_tracing: true,
        mesh_shaders: true,
        astc_transcode: true,
        etc_eac_transcode: true,
        pvrtc_transcode: true,
        tile_shaders: true,
        float_atomics_cas: true,
        metal4: true,
    };
}

/// `MTLArgumentBuffersTier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArgumentBuffersTier {
    /// Tier 1 (fixed-size, no GPU addresses in argument buffers).
    Tier1,
    /// Tier 2 (`gpuAddress`, `gpuResourceID`, bindless).
    Tier2,
}

/// Metal GPU families reported via `supportsFamily:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct GpuFamilies {
    /// `MTLGPUFamilyApple7` (M1 class; implies tile shaders, framebuffer fetch, ASTC, …).
    pub apple7: bool,
    /// `MTLGPUFamilyMac2`.
    pub mac2: bool,
    /// `MTLGPUFamilyCommon3`.
    pub common3: bool,
    /// `MTLGPUFamilyMetal3`.
    pub metal3: bool,
    /// `MTLGPUFamilyMetal4` (introduced with macOS 26).
    pub metal4: bool,
}

/// What Ritsu advertises to the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportedFeatures {
    /// Families answered `YES` by `supportsFamily:`.
    pub families: GpuFamilies,
    /// Argument buffer tier.
    pub argument_buffers_tier: ArgumentBuffersTier,
    /// Ray tracing (acceleration structures + intersection in shaders).
    pub ray_tracing: Support,
    /// Mesh and object shaders.
    pub mesh_shaders: Support,
    /// ASTC pixel formats (LDR and HDR).
    pub astc: Support,
    /// ETC2/EAC pixel formats (not on the wire in protocol v1; part of the Apple7 decision).
    pub etc_eac: Support,
    /// PVRTC pixel formats (not on the wire in protocol v1; part of the Apple7 decision).
    pub pvrtc: Support,
    /// BC pixel formats.
    pub bc: Support,
    /// `atomic<float>` in device memory.
    pub float32_atomics: Support,
    /// Programmable blending / framebuffer fetch.
    pub framebuffer_fetch: Support,
    /// Raster order groups.
    pub raster_order_groups: Support,
    /// Tile shaders and imageblocks.
    pub tile_shaders: Support,
    /// Memoryless render targets (always emulated on IMR GPUs: transient attachments).
    pub memoryless: Support,
    /// `hasUnifiedMemory`: native on integrated GPUs, emulated (coherence tracking) on
    /// discrete ones. The guest always sees `YES` (Apple Silicon).
    pub unified_memory: Support,
    /// Effective limits: device limits clamped to Apple7.
    pub limits: ResourceLimits,
    /// Limits the guest can know. Fields carried in the HELLO_ACK capability block (buffer
    /// length, texture dimensions, array layers, threads, colour attachments) equal
    /// [`ReportedFeatures::limits`]; all others (sample count, vertex attributes and buffers,
    /// texture buffer width) are not on the wire, so the guest assumes Apple7. Descriptors are
    /// validated against these (violation = guest error); exceeding a smaller device limit is
    /// a capability gap (`Unsupported`, see [`texture_support`], [`vertex_input_support`]).
    pub guest_limits: ResourceLimits,
}

/// [`reported_feature_set_with`] using [`EmulationPolicy::NATIVE_ONLY`].
#[must_use]
pub fn reported_feature_set(caps: &Capabilities) -> ReportedFeatures {
    reported_feature_set_with(caps, &EmulationPolicy::NATIVE_ONLY)
}

/// Derives the guest-visible feature set from device capabilities and policy.
///
/// **Assumption:** the family requirements below are a reading of Apple's public Metal
/// feature set tables and must be re-checked for macOS 27 (gpu.md §5, open question G-3).
#[must_use]
pub fn reported_feature_set_with(
    caps: &Capabilities,
    policy: &EmulationPolicy,
) -> ReportedFeatures {
    use Support::{Emulated, Native, Unsupported};

    let bda = caps.buffer_device_address;
    let tier = if bda && (caps.descriptor_buffer || caps.descriptor_indexing) {
        ArgumentBuffersTier::Tier2
    } else {
        ArgumentBuffersTier::Tier1
    };
    let tier2 = tier == ArgumentBuffersTier::Tier2;

    let ray_tracing = Support::native_or(
        bda && caps.acceleration_structure && caps.ray_query,
        bda && policy.ray_tracing,
    );
    let mesh_shaders = Support::native_or(caps.mesh_shader, policy.mesh_shaders);
    let astc = Support::native_or(caps.texture_compression_astc_ldr, policy.astc_transcode);
    // No Vulkan driver of the target GPUs exposes ETC2 or PVRTC [Annahme]; both need
    // transcoding.
    let etc_eac = Support::native_or(false, policy.etc_eac_transcode);
    let pvrtc = Support::native_or(false, policy.pvrtc_transcode);
    let bc = Support::native_or(caps.texture_compression_bc, false);
    let float32_atomics =
        Support::native_or(caps.buffer_float32_atomic_add, policy.float_atomics_cas);
    let framebuffer_fetch = Support::native_or(caps.dynamic_rendering_local_read, false);
    let raster_order_groups = Support::native_or(caps.fragment_shader_interlock, false);
    // Tile shaders have no Vulkan counterpart on IMR GPUs; emulation builds on framebuffer
    // fetch for imageblock reads.
    let tile_shaders = if policy.tile_shaders && framebuffer_fetch.is_reported() {
        Emulated
    } else {
        Unsupported
    };
    let memoryless = Emulated;
    let unified_memory = if caps.unified_memory {
        Native
    } else {
        Emulated
    };

    // Apple2+ features of every Apple family: ASTC, ETC2/EAC and PVRTC formats.
    let apple7 = tier2
        && astc.is_reported()
        && etc_eac.is_reported()
        && pvrtc.is_reported()
        && framebuffer_fetch.is_reported()
        && raster_order_groups.is_reported()
        && tile_shaders.is_reported()
        && float32_atomics.is_reported()
        && memoryless.is_reported();
    let mac2 = tier2 && bc.is_reported();
    let metal3 = (apple7 || mac2) && mesh_shaders.is_reported();
    let metal4 = metal3 && apple7 && policy.metal4;

    let limits = caps.limits.min(&ResourceLimits::APPLE7);
    let a7 = ResourceLimits::APPLE7;
    let guest_limits = ResourceLimits {
        max_sample_count: a7.max_sample_count,
        max_vertex_attributes: a7.max_vertex_attributes,
        max_vertex_buffers: a7.max_vertex_buffers,
        max_texture_buffer_width: a7.max_texture_buffer_width,
        ..limits
    };

    ReportedFeatures {
        families: GpuFamilies {
            apple7,
            mac2,
            common3: tier2,
            metal3,
            metal4,
        },
        argument_buffers_tier: tier,
        ray_tracing,
        mesh_shaders,
        astc,
        etc_eac,
        pvrtc,
        bc,
        float32_atomics,
        framebuffer_fetch,
        raster_order_groups,
        tile_shaders,
        memoryless,
        unified_memory,
        limits,
        guest_limits,
    }
}

/// How a device covers the capabilities Metal guarantees for one pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSupport {
    /// What Metal guarantees ([`PixelFormat::metal_caps`]).
    pub required: FormatCaps,
    /// Required capabilities the device has natively.
    pub native: FormatCaps,
    /// Required capabilities provided by an enabled emulation (transcoding).
    pub emulated: FormatCaps,
}

impl FormatSupport {
    /// Required capabilities that are neither native nor emulated.
    #[must_use]
    pub const fn missing(&self) -> FormatCaps {
        self.required.difference(self.native.union(self.emulated))
    }
}

/// Compares what Metal guarantees for `format` with what the device and the enabled
/// emulations provide.
///
/// Emulation exists only for sampling compressed formats without hardware support
/// (transcoding at upload, gpu.md §4.3), and only if the transcoding target is natively
/// sampleable and filterable.
#[must_use]
pub fn format_support(
    caps: &Capabilities,
    policy: &EmulationPolicy,
    format: PixelFormat,
) -> FormatSupport {
    let required = format.metal_caps();
    let native = caps.formats.get(format).intersection(required);
    let sf = FormatCaps::SAMPLE.union(FormatCaps::FILTER);
    let target_ok = |target: PixelFormat| caps.formats.get(target).contains(sf);
    let rgba8 = if format.is_srgb() {
        PixelFormat::RGBA8Unorm_sRGB
    } else {
        PixelFormat::RGBA8Unorm
    };
    let transcode = match format.compression() {
        Compression::Astc => policy.astc_transcode && target_ok(rgba8),
        Compression::AstcHdr => policy.astc_transcode && target_ok(PixelFormat::RGBA16Float),
        Compression::Etc2Eac => {
            // EAC R11/RG11 keep 11 bits → 16-bit targets [Entscheidung].
            policy.etc_eac_transcode
                && target_ok(rgba8)
                && target_ok(PixelFormat::R16Snorm)
                && target_ok(PixelFormat::RG16Snorm)
        }
        Compression::Pvrtc => policy.pvrtc_transcode && target_ok(rgba8),
        _ => false,
    };
    let emulated = if transcode {
        sf.intersection(required).difference(native)
    } else {
        FormatCaps::NONE
    };
    FormatSupport {
        required,
        native,
        emulated,
    }
}

/// Capabilities a texture with this descriptor relies on (only those Metal promises for its
/// format; sparse residency is never requested here).
#[must_use]
pub fn texture_requirements(desc: &TextureDesc) -> FormatCaps {
    let mut needed = FormatCaps::SAMPLE;
    if desc.usage.contains(TextureUsage::SHADER_READ) {
        needed = needed.union(FormatCaps::FILTER);
    }
    if desc.usage.contains(TextureUsage::SHADER_WRITE) {
        needed = needed.union(FormatCaps::WRITE);
    }
    if desc.usage.contains(TextureUsage::RENDER_TARGET) {
        needed = needed
            .union(FormatCaps::COLOR)
            .union(FormatCaps::BLEND)
            .union(FormatCaps::DEPTH)
            .union(FormatCaps::STENCIL);
    }
    if desc.usage.contains(TextureUsage::SHADER_ATOMIC) {
        needed = needed.union(FormatCaps::ATOMIC);
    }
    if desc.texture_type.is_multisample() {
        needed = needed.union(FormatCaps::MSAA).union(FormatCaps::RESOLVE);
    }
    needed.intersection(desc.pixel_format.metal_caps())
}

/// Checks that the device (with the enabled emulations) can honour everything Metal
/// promises for a texture with this (already validated) descriptor. Call it before creating
/// the texture; a failure is reported to the guest as "not implemented", never approximated.
///
/// # Errors
///
/// [`ModelError::FormatUnsupportedOnDevice`] listing the missing capabilities;
/// [`ModelError::Unsupported`] when a limit the guest cannot know (texture buffer width,
/// sample count) is lower on this GPU than on Apple7.
pub fn texture_support(
    caps: &Capabilities,
    policy: &EmulationPolicy,
    desc: &TextureDesc,
) -> Result<(), ModelError> {
    let l = &caps.limits;
    if desc.texture_type == TextureType::TypeTextureBuffer
        && desc.width > l.max_texture_buffer_width
    {
        return Err(ModelError::Unsupported(
            "texture buffer wider than maxTexelBufferElements of this GPU",
        ));
    }
    if desc.sample_count > l.max_sample_count {
        return Err(ModelError::Unsupported(
            "sample count above the limit of this GPU",
        ));
    }
    let support = format_support(caps, policy, desc.pixel_format);
    let missing = texture_requirements(desc)
        .difference(support.native)
        .difference(support.emulated);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ModelError::FormatUnsupportedOnDevice {
            format: desc.pixel_format,
            missing,
        })
    }
}

/// Checks a (validated) vertex descriptor against the device's vertex-input capabilities.
/// Metal allows more than core Vulkan; until vertex pulling exists in the shader compiler,
/// the excess is reported as unsupported.
///
/// # Errors
///
/// [`ModelError::Unsupported`] naming the first gap.
pub fn vertex_input_support(
    caps: &Capabilities,
    desc: &VertexDescriptor,
) -> Result<(), ModelError> {
    let v = &caps.vertex_input;
    for (i, a) in desc.used_attributes() {
        // Indices up to Apple7's 31 are valid for the guest (checked by the model); a device
        // with fewer attributes or buffers has a gap, not the guest an error.
        if i as u64 >= u64::from(caps.limits.max_vertex_attributes) {
            return Err(ModelError::Unsupported(
                "vertex attribute index above the limit of this GPU",
            ));
        }
        if a.buffer_index >= caps.limits.max_vertex_buffers {
            return Err(ModelError::Unsupported(
                "vertex buffer index above the limit of this GPU",
            ));
        }
        if a.offset > u64::from(v.max_attribute_offset) {
            return Err(ModelError::Unsupported(
                "vertex attribute offset above the device limit (needs vertex pulling)",
            ));
        }
        let Some(layout) = desc.layout_of(a) else {
            return Err(ModelError::InvalidValue {
                field: "vertex attribute bufferIndex",
                reason: "exceeds the layout table",
            });
        };
        if let VertexStride::Fixed(stride) = layout.stride
            && stride > u64::from(v.max_binding_stride)
        {
            return Err(ModelError::Unsupported(
                "vertex buffer stride above the device limit (needs vertex pulling)",
            ));
        }
        match layout.step_function {
            VertexStepFunction::PerInstance if layout.step_rate > 1 => {
                if !v.instance_divisor || layout.step_rate > u64::from(u32::MAX) {
                    return Err(ModelError::Unsupported("vertex instance step rate > 1"));
                }
            }
            VertexStepFunction::Constant if !v.zero_divisor => {
                return Err(ModelError::Unsupported("constant vertex step function"));
            }
            _ => {}
        }
    }
    if !v.unaligned_fetch && !desc.is_naturally_aligned() {
        return Err(ModelError::Unsupported(
            "unaligned vertex attribute offset or stride",
        ));
    }
    Ok(())
}

/// Checks a validated render pipeline against the device: every attachment format must be
/// natively renderable in its role with the pipeline's blending and sample count (render
/// targets are never emulated by transcoding), the sample count must not exceed the device
/// limit, and the vertex descriptor must fit [`vertex_input_support`].
///
/// # Errors
///
/// [`ModelError::FormatUnsupportedOnDevice`] naming the missing capabilities,
/// [`ModelError::Unsupported`] for limits and vertex input gaps.
pub fn render_pipeline_support(
    caps: &Capabilities,
    policy: &EmulationPolicy,
    desc: &RenderPipelineDesc,
) -> Result<(), ModelError> {
    if desc.raster_sample_count > caps.limits.max_sample_count {
        return Err(ModelError::Unsupported(
            "raster sample count above the limit of this GPU",
        ));
    }
    let msaa = if desc.raster_sample_count > 1 {
        FormatCaps::MSAA
    } else {
        FormatCaps::NONE
    };
    let check = |format: PixelFormat, needed: FormatCaps| {
        let needed = needed.union(msaa).intersection(format.metal_caps());
        let missing = needed.difference(format_support(caps, policy, format).native);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(ModelError::FormatUnsupportedOnDevice { format, missing })
        }
    };
    for c in &desc.color_attachments {
        if let Some(format) = c.pixel_format {
            let blend = if c.blending_enabled {
                FormatCaps::BLEND
            } else {
                FormatCaps::NONE
            };
            check(format, FormatCaps::COLOR.union(blend))?;
        }
    }
    if let Some(format) = desc.depth_attachment_pixel_format {
        check(format, FormatCaps::DEPTH)?;
    }
    if let Some(format) = desc.stencil_attachment_pixel_format {
        check(format, FormatCaps::STENCIL)?;
    }
    if let Some(v) = &desc.vertex_descriptor {
        vertex_input_support(caps, v)?;
    }
    Ok(())
}

/// Checks a validated render pass against what the device can express.
///
/// # Errors
///
/// [`ModelError::Unsupported`] for separate depth and stencil textures (Vulkan dynamic
/// rendering needs one image view for both; emulation is a follow-up WP), for min/max depth
/// resolves on devices without them, and for resolves of one combined depth/stencil texture
/// that dynamic rendering cannot express (different resolve targets: never; only one aspect:
/// needs `independentResolveNone`; different modes: needs `independentResolve`).
pub fn render_pass_support(
    caps: &Capabilities,
    layout: &RenderTargetLayout,
) -> Result<(), ModelError> {
    if layout.separate_depth_stencil {
        return Err(ModelError::Unsupported(
            "separate depth and stencil textures in one render pass",
        ));
    }
    if layout.depth_resolve_min_max && !caps.depth_resolve_min_max {
        return Err(ModelError::Unsupported("min/max depth resolve on this GPU"));
    }
    if layout.ds_resolve_targets_differ {
        return Err(ModelError::Unsupported(
            "depth and stencil of one texture resolved into different targets",
        ));
    }
    if layout.ds_resolve_one_aspect && !caps.independent_resolve_none {
        return Err(ModelError::Unsupported(
            "resolving only depth or only stencil of one texture on this GPU",
        ));
    }
    if layout.ds_resolve_modes_differ && !caps.independent_resolve {
        return Err(ModelError::Unsupported(
            "different depth and stencil resolve modes on this GPU",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use Support::{Emulated, Native, Unsupported};

    #[test]
    fn both_targets_meet_the_baseline() {
        assert!(
            Capabilities::rtx4050_assumed()
                .missing_baseline()
                .is_empty()
        );
        assert!(
            Capabilities::iris_xe_assumed()
                .missing_baseline()
                .is_empty()
        );
        let mut old = Capabilities::iris_xe_assumed();
        old.api_version = VulkanVersion { major: 1, minor: 2 };
        old.timeline_semaphore = false;
        assert_eq!(
            old.missing_baseline(),
            vec!["Vulkan 1.3", "timelineSemaphore"]
        );
    }

    #[test]
    fn rtx4050_native_only() {
        let r = reported_feature_set(&Capabilities::rtx4050_assumed());
        assert_eq!(r.ray_tracing, Native);
        assert_eq!(r.mesh_shaders, Native);
        assert_eq!(r.bc, Native);
        assert_eq!(
            r.astc, Unsupported,
            "no ASTC hardware, transcoding disabled"
        );
        assert_eq!(r.unified_memory, Emulated, "discrete GPU");
        assert_eq!(r.argument_buffers_tier, ArgumentBuffersTier::Tier2);
        assert!(r.families.mac2 && r.families.metal3);
        assert!(!r.families.apple7, "Apple7 needs ASTC + tile shaders");
        assert!(!r.families.metal4);
    }

    #[test]
    fn iris_xe_does_not_advertise_ray_tracing_or_mesh_by_default() {
        let caps = Capabilities::iris_xe_assumed();
        let r = reported_feature_set(&caps);
        assert_eq!(r.ray_tracing, Unsupported);
        assert_eq!(r.mesh_shaders, Unsupported);
        assert_eq!(r.float32_atomics, Unsupported);
        assert_eq!(r.unified_memory, Native);
        assert!(r.families.mac2);
        assert!(!r.families.metal3, "Metal 3 requires mesh shaders");

        let rt = EmulationPolicy {
            ray_tracing: true,
            ..EmulationPolicy::NATIVE_ONLY
        };
        assert_eq!(reported_feature_set_with(&caps, &rt).ray_tracing, Emulated);
    }

    #[test]
    fn full_emulation_reaches_apple7_and_metal4_on_both() {
        for caps in [
            Capabilities::rtx4050_assumed(),
            Capabilities::iris_xe_assumed(),
        ] {
            let r = reported_feature_set_with(&caps, &EmulationPolicy::ALL);
            assert!(r.families.apple7, "{}", caps.device_name);
            assert!(r.families.metal3 && r.families.metal4);
            assert_eq!(r.astc, Emulated);
            assert_eq!(r.tile_shaders, Emulated);
        }
    }

    #[test]
    fn missing_capabilities_are_never_reported_native() {
        let base = Capabilities::rtx4050_assumed();
        let check = |mutate: fn(&mut Capabilities), get: fn(&ReportedFeatures) -> Support| {
            let mut c = base.clone();
            mutate(&mut c);
            for policy in [EmulationPolicy::NATIVE_ONLY, EmulationPolicy::ALL] {
                assert_ne!(get(&reported_feature_set_with(&c, &policy)), Native);
            }
        };
        check(|c| c.ray_query = false, |r| r.ray_tracing);
        check(|c| c.acceleration_structure = false, |r| r.ray_tracing);
        check(|c| c.buffer_device_address = false, |r| r.ray_tracing);
        check(|c| c.mesh_shader = false, |r| r.mesh_shaders);
        check(|c| c.texture_compression_bc = false, |r| r.bc);
        check(
            |c| c.buffer_float32_atomic_add = false,
            |r| r.float32_atomics,
        );
        check(
            |c| c.dynamic_rendering_local_read = false,
            |r| r.framebuffer_fetch,
        );
        check(
            |c| c.fragment_shader_interlock = false,
            |r| r.raster_order_groups,
        );
    }

    #[test]
    fn without_bda_no_tier2_no_families_no_rt_emulation() {
        let mut c = Capabilities::iris_xe_assumed();
        c.buffer_device_address = false;
        let r = reported_feature_set_with(&c, &EmulationPolicy::ALL);
        assert_eq!(r.argument_buffers_tier, ArgumentBuffersTier::Tier1);
        assert_eq!(r.ray_tracing, Unsupported);
        assert_eq!(r.families, GpuFamilies::default());
    }

    #[test]
    fn limits_are_clamped_to_device_and_apple7() {
        for caps in [
            Capabilities::rtx4050_assumed(),
            Capabilities::iris_xe_assumed(),
        ] {
            let r = reported_feature_set(&caps).limits;
            assert!(r.max_texture_dimension_2d <= caps.limits.max_texture_dimension_2d);
            assert!(r.max_texture_dimension_2d <= ResourceLimits::APPLE7.max_texture_dimension_2d);
            assert!(r.max_buffer_length <= caps.limits.max_buffer_length);
        }
        let iris = reported_feature_set(&Capabilities::iris_xe_assumed()).limits;
        assert_eq!(iris.max_buffer_length, 2 * GIB);
    }

    /// Format → native capability table for both assumed GPU profiles, kept apart from what
    /// Metal requires, and the resulting gaps.
    #[test]
    fn per_gpu_format_capabilities() {
        use PixelFormat as P;
        let sf = FormatCaps::SAMPLE.union(FormatCaps::FILTER);
        for caps in [
            Capabilities::rtx4050_assumed(),
            Capabilities::iris_xe_assumed(),
        ] {
            let native = |f| format_support(&caps, &EmulationPolicy::NATIVE_ONLY, f);
            // Ordinary formats: everything Metal promises is native.
            for f in [
                P::RGBA8Unorm,
                P::BGRA8Unorm,
                P::RGBA16Float,
                P::RGB10A2Unorm,
                P::RG11B10Float,
                P::R32Float,
                P::RGBA32Uint,
                P::R32Uint,
                P::Depth32Float,
                P::Depth32Float_Stencil8,
                P::Stencil8,
            ] {
                let s = native(f);
                assert_eq!(
                    s.missing().difference(FormatCaps::SPARSE),
                    FormatCaps::NONE,
                    "{f:?} on {}",
                    caps.device_name
                );
            }
            // BC: native sampling and filtering.
            assert_eq!(native(P::BC7_RGBAUnorm).native, sf);
            // sRGB: no shader writes on the GPU although Metal promises them.
            assert!(
                native(P::RGBA8Unorm_sRGB)
                    .missing()
                    .contains(FormatCaps::WRITE)
            );
            // RGB9E5: Metal renders to it, the GPUs only sample it.
            assert!(native(P::RGB9E5Float).missing().contains(FormatCaps::COLOR));
            // Extended range: nothing native at all (open question G-6).
            assert!(native(P::BGRA10_XR).native.is_empty());
            // Compressed formats without hardware: nothing native; emulated only by policy.
            for f in [
                P::ASTC_4x4_LDR,
                P::ASTC_6x6_HDR,
                P::ETC2_RGB8,
                P::EAC_RG11Snorm,
                P::PVRTC_RGBA_4BPP,
            ] {
                assert!(native(f).native.is_empty(), "{f:?}");
                let all = format_support(&caps, &EmulationPolicy::ALL, f);
                assert_eq!(all.emulated, sf, "{f:?}");
                assert_eq!(all.missing(), FormatCaps::SPARSE, "{f:?}");
            }
            // GPU caps never claim more than the format has in Metal terms where Metal
            // forbids it (e.g. no blending of integer formats).
            for f in PixelFormat::ALL {
                let g = caps.formats.get(*f);
                if f.numeric_type().is_integer() {
                    assert!(
                        !g.intersects(FormatCaps::FILTER.union(FormatCaps::BLEND)),
                        "{f:?}"
                    );
                }
                if f.is_compressed() {
                    assert!(
                        !g.intersects(FormatCaps::COLOR.union(FormatCaps::WRITE)),
                        "{f:?}"
                    );
                }
            }
            // The ReportedFeatures summary agrees with the format table.
            let r = reported_feature_set(&caps);
            assert_eq!(
                r.bc == Native,
                native(P::BC1_RGBA).native.contains(FormatCaps::SAMPLE)
            );
            assert_eq!(
                r.astc == Native,
                native(P::ASTC_4x4_LDR).native.contains(FormatCaps::SAMPLE)
            );
        }
        // The profiles differ where the drivers differ (D24S8 exists on NVIDIA only).
        let rtx = Capabilities::rtx4050_assumed();
        let iris = Capabilities::iris_xe_assumed();
        assert!(!rtx.formats.get(P::Depth24Unorm_Stencil8).is_empty());
        assert!(iris.formats.get(P::Depth24Unorm_Stencil8).is_empty());
        // …but Metal never offers it on Apple GPUs, so nothing is required either way.
        assert!(
            format_support(&rtx, &EmulationPolicy::ALL, P::Depth24Unorm_Stencil8)
                .required
                .is_empty()
        );
        assert!(format!("{:?}", rtx.formats).contains("BGRA8Unorm"));
    }

    #[test]
    fn transcoding_needs_a_native_target() {
        let mut caps = Capabilities::iris_xe_assumed();
        caps.formats.set(PixelFormat::RGBA16Float, FormatCaps::NONE);
        let s = format_support(&caps, &EmulationPolicy::ALL, PixelFormat::ASTC_4x4_HDR);
        assert!(s.emulated.is_empty(), "HDR ASTC decodes to RGBA16Float");
        let s = format_support(&caps, &EmulationPolicy::ALL, PixelFormat::ASTC_4x4_LDR);
        assert!(!s.emulated.is_empty());
    }

    #[test]
    fn texture_support_by_usage() {
        use PixelFormat as P;
        let caps = Capabilities::rtx4050_assumed();
        let none = EmulationPolicy::NATIVE_ONLY;
        let all = EmulationPolicy::ALL;
        let tex = |f, usage| TextureDesc::new_2d(f, 64, 64, usage);
        let read = TextureUsage::SHADER_READ;
        let rt = TextureUsage::RENDER_TARGET.union(read);
        assert!(texture_support(&caps, &none, &tex(P::BGRA8Unorm, rt)).is_ok());
        assert!(texture_support(&caps, &none, &tex(P::BC3_RGBA, read)).is_ok());
        // ASTC: unsupported natively, fine with transcoding.
        let e = texture_support(&caps, &none, &tex(P::ASTC_8x8_LDR, read)).unwrap_err();
        assert!(e.is_unsupported());
        assert!(matches!(
            e,
            ModelError::FormatUnsupportedOnDevice { missing, .. } if missing.contains(FormatCaps::SAMPLE)
        ));
        assert!(texture_support(&caps, &all, &tex(P::ASTC_8x8_LDR, read)).is_ok());
        // sRGB is fine to sample and render, not to write in shaders (no emulation yet).
        assert!(texture_support(&caps, &all, &tex(P::RGBA8Unorm_sRGB, rt)).is_ok());
        let w = texture_support(
            &caps,
            &all,
            &tex(P::RGBA8Unorm_sRGB, read.union(TextureUsage::SHADER_WRITE)),
        );
        assert!(
            matches!(w, Err(ModelError::FormatUnsupportedOnDevice { missing, .. }) if missing == FormatCaps::WRITE)
        );
        // RGB9E5 sampled: fine; as render target: unsupported.
        assert!(texture_support(&caps, &all, &tex(P::RGB9E5Float, read)).is_ok());
        assert!(texture_support(&caps, &all, &tex(P::RGB9E5Float, rt)).is_err());
        // Extended range (WindowServer formats, G-6): not yet.
        assert!(texture_support(&caps, &all, &tex(P::BGRA10_XR, read)).is_err());
        // Multisample needs MSAA + resolve.
        let ms = TextureDesc {
            texture_type: crate::model::TextureType::Type2DMultisample,
            sample_count: 4,
            ..tex(P::RGBA16Float, TextureUsage::RENDER_TARGET)
        };
        assert!(texture_support(&caps, &none, &ms).is_ok());
        let mut no_msaa = caps.clone();
        no_msaa.formats.set(
            P::RGBA16Float,
            FormatCaps::SAMPLE
                .union(FormatCaps::COLOR)
                .union(FormatCaps::BLEND),
        );
        assert!(texture_support(&no_msaa, &none, &ms).is_err());
        // Atomics.
        let atomic = tex(P::R32Uint, TextureUsage::SHADER_ATOMIC.union(read));
        assert!(texture_support(&caps, &none, &atomic).is_ok());
    }

    #[test]
    fn vertex_input_gaps() {
        use crate::model::{VertexAttributeDesc, VertexFormat};
        let mut caps = Capabilities::iris_xe_assumed();
        let mut d = VertexDescriptor::default();
        d.attributes[0] = Some(VertexAttributeDesc {
            format: VertexFormat::Float3,
            offset: 2,
            buffer_index: 0,
        });
        d.layouts[0].stride = VertexStride::Fixed(14);
        assert!(
            vertex_input_support(&caps, &d).is_ok(),
            "unaligned fetch assumed available"
        );
        caps.vertex_input.unaligned_fetch = false;
        assert!(
            vertex_input_support(&caps, &d)
                .unwrap_err()
                .is_unsupported()
        );
        d.attributes[0] = Some(VertexAttributeDesc {
            format: VertexFormat::Float3,
            offset: 0,
            buffer_index: 0,
        });
        d.layouts[0].stride = VertexStride::Fixed(16);
        assert!(vertex_input_support(&caps, &d).is_ok());
        d.layouts[0].stride = VertexStride::Fixed(4096);
        assert!(
            vertex_input_support(&caps, &d).is_err(),
            "stride above the device limit"
        );
        d.layouts[0].stride = VertexStride::Dynamic;
        assert!(vertex_input_support(&caps, &d).is_ok());
        d.attributes[0] = Some(VertexAttributeDesc {
            format: VertexFormat::Float,
            offset: 4096,
            buffer_index: 0,
        });
        assert!(
            vertex_input_support(&caps, &d).is_err(),
            "offset above the device limit"
        );
        d.attributes[0] = Some(VertexAttributeDesc {
            format: VertexFormat::Float,
            offset: 0,
            buffer_index: 0,
        });
        d.layouts[0].step_function = VertexStepFunction::PerInstance;
        d.layouts[0].step_rate = 3;
        assert!(vertex_input_support(&caps, &d).is_ok());
        d.layouts[0].step_rate = u64::from(u32::MAX) + 1;
        assert!(vertex_input_support(&caps, &d).is_err());
        caps.vertex_input.instance_divisor = false;
        d.layouts[0].step_rate = 3;
        assert!(vertex_input_support(&caps, &d).is_err());
        d.layouts[0].step_function = VertexStepFunction::Constant;
        d.layouts[0].step_rate = 0;
        assert!(vertex_input_support(&caps, &d).is_ok());
        caps.vertex_input.zero_divisor = false;
        assert!(vertex_input_support(&caps, &d).is_err());
    }

    #[test]
    fn limits_the_guest_cannot_know_are_capability_gaps() {
        use crate::model::{VertexAttributeDesc, VertexFormat};
        let iris = Capabilities::iris_xe_assumed();
        let r = reported_feature_set(&iris);
        // Not on the wire → the guest assumes Apple7.
        assert_eq!(r.guest_limits.max_vertex_attributes, 31);
        assert_eq!(r.guest_limits.max_vertex_buffers, 31);
        assert_eq!(r.guest_limits.max_texture_buffer_width, 256 << 20);
        assert_eq!(r.limits.max_vertex_attributes, 29);
        // On the wire → identical.
        assert_eq!(
            r.guest_limits.max_texture_dimension_2d,
            r.limits.max_texture_dimension_2d
        );
        assert_eq!(r.guest_limits.max_buffer_length, r.limits.max_buffer_length);

        // Vertex attribute 30 / buffer 30: valid for the guest, a gap on the Iris.
        let mut d = VertexDescriptor::default();
        d.attributes[30] = Some(VertexAttributeDesc {
            format: VertexFormat::Float,
            offset: 0,
            buffer_index: 0,
        });
        d.layouts[0].stride = VertexStride::Fixed(4);
        assert!(d.validate(&r.guest_limits).is_ok());
        assert!(
            vertex_input_support(&iris, &d)
                .unwrap_err()
                .is_unsupported()
        );
        assert!(vertex_input_support(&Capabilities::rtx4050_assumed(), &d).is_ok());
        let mut b = VertexDescriptor::default();
        b.attributes[0] = Some(VertexAttributeDesc {
            format: VertexFormat::Float,
            offset: 0,
            buffer_index: 30,
        });
        assert!(b.validate(&r.guest_limits).is_ok());
        assert!(
            vertex_input_support(&iris, &b)
                .unwrap_err()
                .is_unsupported()
        );

        // Texture buffers between maxTexelBufferElements (2^27 assumed) and Apple7's 256 Mi.
        let tb = |w| TextureDesc {
            texture_type: TextureType::TypeTextureBuffer,
            ..TextureDesc::new_2d(PixelFormat::R8Unorm, w, 1, TextureUsage::SHADER_READ)
        };
        let none = EmulationPolicy::NATIVE_ONLY;
        assert!(tb(1 << 28).validate(&r.guest_limits).is_ok());
        assert!(
            texture_support(&iris, &none, &tb(1 << 28))
                .unwrap_err()
                .is_unsupported()
        );
        assert!(texture_support(&iris, &none, &tb(1 << 27)).is_ok());
    }

    #[test]
    fn render_pass_gaps() {
        let caps = Capabilities::rtx4050_assumed();
        let layout = RenderTargetLayout {
            color_formats: [None; crate::model::MAX_COLOR_ATTACHMENTS],
            depth_format: Some(PixelFormat::Depth32Float),
            stencil_format: Some(PixelFormat::Stencil8),
            sample_count: 1,
            width: 1,
            height: 1,
            layers: 0,
            separate_depth_stencil: false,
            deferred_store_action: false,
            depth_resolve_min_max: true,
            ds_resolve_targets_differ: false,
            ds_resolve_one_aspect: false,
            ds_resolve_modes_differ: false,
        };
        assert!(render_pass_support(&caps, &layout).is_ok());
        let mut no_minmax = caps.clone();
        no_minmax.depth_resolve_min_max = false;
        assert!(render_pass_support(&no_minmax, &layout).is_err());
        let separate = RenderTargetLayout {
            separate_depth_stencil: true,
            ..layout
        };
        assert!(
            render_pass_support(&caps, &separate)
                .unwrap_err()
                .is_unsupported()
        );
        // One combined depth/stencil texture resolved into two targets: never expressible.
        let targets = RenderTargetLayout {
            ds_resolve_targets_differ: true,
            ..layout
        };
        assert!(
            render_pass_support(&caps, &targets)
                .unwrap_err()
                .is_unsupported()
        );
        // Only one aspect resolved / different modes: only with independent resolve.
        let one = RenderTargetLayout {
            ds_resolve_one_aspect: true,
            ..layout
        };
        let modes = RenderTargetLayout {
            ds_resolve_modes_differ: true,
            ..layout
        };
        assert!(render_pass_support(&caps, &one).is_ok());
        assert!(render_pass_support(&caps, &modes).is_ok());
        let mut strict = caps.clone();
        strict.independent_resolve_none = false;
        strict.independent_resolve = false;
        assert!(
            render_pass_support(&strict, &one)
                .unwrap_err()
                .is_unsupported()
        );
        assert!(
            render_pass_support(&strict, &modes)
                .unwrap_err()
                .is_unsupported()
        );
    }

    #[test]
    fn apple7_needs_every_compressed_family() {
        let caps = Capabilities::rtx4050_assumed();
        for off in [
            |p: &mut EmulationPolicy| p.astc_transcode = false,
            |p: &mut EmulationPolicy| p.etc_eac_transcode = false,
            |p: &mut EmulationPolicy| p.pvrtc_transcode = false,
        ] {
            let mut policy = EmulationPolicy::ALL;
            off(&mut policy);
            let r = reported_feature_set_with(&caps, &policy);
            assert!(!r.families.apple7);
            assert!(!r.families.metal4);
        }
        let r = reported_feature_set_with(&caps, &EmulationPolicy::ALL);
        assert_eq!((r.etc_eac, r.pvrtc), (Emulated, Emulated));
        let n = reported_feature_set(&caps);
        assert_eq!((n.etc_eac, n.pvrtc), (Unsupported, Unsupported));
    }

    #[test]
    fn reported_limits_include_render_and_vertex_limits() {
        let iris = reported_feature_set(&Capabilities::iris_xe_assumed()).limits;
        assert_eq!(iris.max_sample_count, 4, "Apple7 caps the sample count");
        assert_eq!(iris.max_vertex_attributes, 29, "device below Apple7's 31");
        let rtx = reported_feature_set(&Capabilities::rtx4050_assumed()).limits;
        assert_eq!(rtx.max_vertex_attributes, 31);
        assert_eq!(rtx.max_vertex_buffers, 31);
    }

    #[test]
    fn shared_buffer_placement_policy() {
        assert!(Capabilities::iris_xe_assumed().shared_buffers_direct());
        assert!(Capabilities::rtx4050_assumed().shared_buffers_direct());
        let mut no_rebar = Capabilities::rtx4050_assumed();
        no_rebar.resizable_bar = false;
        no_rebar.host_visible_device_local_bytes = 256 * MIB;
        assert!(!no_rebar.shared_buffers_direct());
    }

    #[test]
    fn render_pipelines_need_native_render_target_formats() {
        use crate::model::{PixelFormat as P, RenderPipelineDesc};
        let caps = Capabilities::rtx4050_assumed();
        let policy = EmulationPolicy::ALL;
        let pipe = |format, blending, samples| {
            let mut p = RenderPipelineDesc::default();
            p.color_attachments[0].pixel_format = Some(format);
            p.color_attachments[0].blending_enabled = blending;
            p.raster_sample_count = samples;
            p
        };
        assert!(render_pipeline_support(&caps, &policy, &pipe(P::BGRA8Unorm, true, 4)).is_ok());
        // RGB9E5Float is colour-renderable in Metal but only sampleable on the GPU [Annahme];
        // transcoding never helps render targets.
        assert!(matches!(
            render_pipeline_support(&caps, &policy, &pipe(P::RGB9E5Float, false, 1)),
            Err(ModelError::FormatUnsupportedOnDevice { .. })
        ));
        let mut depth = pipe(P::BGRA8Unorm, false, 4);
        depth.depth_attachment_pixel_format = Some(P::Depth32Float);
        assert!(render_pipeline_support(&caps, &policy, &depth).is_ok());
        let mut few = caps.clone();
        few.limits.max_sample_count = 2;
        assert!(
            render_pipeline_support(&few, &policy, &pipe(P::BGRA8Unorm, false, 4))
                .unwrap_err()
                .is_unsupported()
        );
    }
}
