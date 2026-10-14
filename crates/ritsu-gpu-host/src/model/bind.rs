//! Argument tables of the render encoder (`setVertexBuffer:offset:atIndex:`,
//! `setFragmentTexture:atIndex:`, `setVertexSamplerState:atIndex:`, …).
//!
//! Limits: Metal Feature Set Tables (2026-05-21), "GPU implementation limits by family",
//! column Apple7 (the floor of every Metal 3/4 device): buffer argument table 31 entries,
//! texture argument table 128 entries, sampler state argument table 16 entries (inline
//! `constexpr` samplers count toward the 16, which only the shader compiler can check) — per
//! graphics function, i.e. per stage. The tables are not part of HELLO_ACK, so the guest
//! assumes these values; an index beyond them is a guest error.
//!
//! On the Vulkan side the tables map to Ritsu's bindless heaps (gpu.md §4.5), not to
//! per-stage descriptor limits, so no device gap exists below these sizes \[Annahme: holds for
//! the descriptor-indexing/descriptor-buffer limits of both target GPUs, verified in
//! WP-GPU-03/13\].

/// Entries of the buffer argument table per stage (Apple7: 31; equals
/// `ResourceLimits::max_vertex_buffers` of `ResourceLimits::APPLE7`).
pub const MAX_BUFFER_BINDINGS: u32 = 31;
/// Entries of the texture argument table per stage (Apple6 and later: 128).
pub const MAX_TEXTURE_BINDINGS: u32 = 128;
/// Entries of the sampler state argument table per stage (all families: 16).
pub const MAX_SAMPLER_BINDINGS: u32 = 16;

/// Shader stage of a render-encoder binding (tile, object and mesh stages are not modelled
/// yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShaderStage {
    /// Vertex function arguments (`setVertex…`).
    Vertex,
    /// Fragment function arguments (`setFragment…`).
    Fragment,
}
