//! Device capabilities and the feature set Ritsu reports to the guest.
//!
//! A backend describes what its Vulkan device can do in [`Capabilities`]. From that (and an
//! explicit [`EmulationPolicy`]), [`reported_feature_set`] derives [`ReportedFeatures`]: the
//! Metal GPU families and optional features the guest is told about. Rule: a feature is only
//! reported if it is either native or *implemented* as an emulation and enabled by policy —
//! never silently wrong (CLAUDE.md §3.5). See `docs/subsystems/gpu.md` §5.

use crate::model::ResourceLimits;

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
    /// features (no ray tracing at the time of writing).
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
            },
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
            },
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
    /// ASTC → BC7/RGBA8 transcoding at upload time.
    pub astc_transcode: bool,
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
        tile_shaders: false,
        float_atomics_cas: false,
        metal4: false,
    };

    /// Every emulation enabled (target state of M7; used in tests and experiments).
    pub const ALL: Self = Self {
        ray_tracing: true,
        mesh_shaders: true,
        astc_transcode: true,
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
    /// ASTC pixel formats.
    pub astc: Support,
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
    /// Limits reported to the guest (device limits clamped to Apple7).
    pub limits: ResourceLimits,
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

    let apple7 = tier2
        && astc.is_reported()
        && framebuffer_fetch.is_reported()
        && raster_order_groups.is_reported()
        && tile_shaders.is_reported()
        && float32_atomics.is_reported()
        && memoryless.is_reported();
    let mac2 = tier2 && bc.is_reported();
    let metal3 = (apple7 || mac2) && mesh_shaders.is_reported();
    let metal4 = metal3 && apple7 && policy.metal4;

    let l = &caps.limits;
    let a = &ResourceLimits::APPLE7;
    let limits = ResourceLimits {
        max_buffer_length: l.max_buffer_length.min(a.max_buffer_length),
        max_texture_dimension_1d: l.max_texture_dimension_1d.min(a.max_texture_dimension_1d),
        max_texture_dimension_2d: l.max_texture_dimension_2d.min(a.max_texture_dimension_2d),
        max_texture_dimension_3d: l.max_texture_dimension_3d.min(a.max_texture_dimension_3d),
        max_texture_array_layers: l.max_texture_array_layers.min(a.max_texture_array_layers),
        max_threads_per_threadgroup: l
            .max_threads_per_threadgroup
            .min(a.max_threads_per_threadgroup),
        max_color_attachments: l.max_color_attachments.min(a.max_color_attachments),
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
        bc,
        float32_atomics,
        framebuffer_fetch,
        raster_order_groups,
        tile_shaders,
        memoryless,
        unified_memory,
        limits,
    }
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

    #[test]
    fn shared_buffer_placement_policy() {
        assert!(Capabilities::iris_xe_assumed().shared_buffers_direct());
        assert!(Capabilities::rtx4050_assumed().shared_buffers_direct());
        let mut no_rebar = Capabilities::rtx4050_assumed();
        no_rebar.resizable_bar = false;
        no_rebar.host_visible_device_local_bytes = 256 * MIB;
        assert!(!no_rebar.shared_buffers_direct());
    }
}
