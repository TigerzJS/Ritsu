//! Backend abstraction and a validating null backend.
//!
//! A [`GpuBackend`] executes the protocol-neutral object model on a concrete device. The
//! Vulkan backends (`nvidia`, `intel`, built on `ash`) arrive in WP-GPU-03/04; until then
//! [`NullBackend`] implements the full validation and object tracking without a GPU, so that
//! frontends and the core can be tested in CI.
//!
//! Contract for all backends:
//! * Inputs are already validated against the object model ([`BufferDesc::validate`] etc.),
//!   but backends re-check everything that depends on *their* state (handles, bounds, pass
//!   nesting). A backend never trusts a handle.
//! * Resources referenced by a submission stay alive until that submission completes, even
//!   if destroyed earlier (deferred destruction keyed on the queue timeline).
//! * Errors caused by guest input are [`GpuError::Guest`]; they never poison the backend.

use crate::caps::{Capabilities, EmulationPolicy, render_pipeline_support, texture_support};
use crate::encoder::EncoderValidator;
use crate::handle::{Handle, HandleError, HandleTable, RawHandle};
use crate::model::{
    AttachmentSlot, BufferDesc, DepthStencilDesc, MAX_BUFFER_BINDINGS, MAX_SAMPLER_BINDINGS,
    MAX_TEXTURE_BINDINGS, ModelError, PipelineTargets, PrimitiveType, RenderPassDesc,
    RenderPipelineDesc, SamplerDesc, ShaderStage, StorageMode, StoreAction, TextureDesc,
    TextureViewDesc,
};

/// Backend-neutral buffer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferId(pub RawHandle);

/// Backend-neutral texture identifier (textures and texture views).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureId(pub RawHandle);

/// Backend-neutral sampler state identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SamplerId(pub RawHandle);

/// Backend-neutral depth/stencil state identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DepthStencilStateId(pub RawHandle);

/// Backend-neutral render pipeline state identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderPipelineId(pub RawHandle);

/// Position on a queue's completion timeline (maps to a Vulkan timeline semaphore value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubmissionToken(pub u64);

/// Backend error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GpuError {
    /// Invalid object description from the guest.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// Invalid or stale handle.
    #[error("invalid handle: {0}")]
    Handle(#[from] HandleError),
    /// Guest-invalid command or argument.
    #[error("invalid command: {0}")]
    Guest(String),
    /// Device memory budget exhausted (reported to the guest as an allocation failure).
    #[error("out of device memory: requested {requested} bytes, {available} available")]
    OutOfDeviceMemory {
        /// Requested bytes.
        requested: u64,
        /// Remaining budget.
        available: u64,
    },
    /// The feature is valid Metal but not implemented by this backend.
    #[error("not implemented: {0}")]
    Unimplemented(&'static str),
    /// Host/driver failure (device lost, driver error).
    #[error("host: {0}")]
    Host(String),
}

impl GpuError {
    /// Whether the error means "valid Metal, not supported here" (reported as
    /// `Unimplemented`), as opposed to invalid guest input or a host failure.
    #[must_use]
    pub const fn is_unsupported(&self) -> bool {
        match self {
            Self::Unimplemented(_) => true,
            Self::Model(m) => m.is_unsupported(),
            _ => false,
        }
    }
}

impl From<GpuError> for ritsu_core::Error {
    fn from(e: GpuError) -> Self {
        match e {
            GpuError::Unimplemented(what) => ritsu_core::Error::Unimplemented(what),
            GpuError::Model(m) => m.into(),
            GpuError::Host(msg) => ritsu_core::Error::Host(msg),
            other => ritsu_core::Error::Guest(format!("gpu: {other}")),
        }
    }
}

/// Encoded GPU work, in Metal encoder order. Backends translate it to Vulkan commands and
/// insert barriers (gpu.md §7).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GpuCommand {
    /// Blit copy between buffers.
    CopyBuffer {
        /// Source.
        src: BufferId,
        /// Source offset.
        src_offset: u64,
        /// Destination.
        dst: BufferId,
        /// Destination offset.
        dst_offset: u64,
        /// Byte count.
        size: u64,
    },
    /// Start a render encoder (attachments, resolve targets, clear values; validated with
    /// [`RenderPassDesc::validate`] against the backend's textures).
    BeginRenderPass(Box<RenderPassDesc<TextureId>>),
    /// Replace a deferred (`MTLStoreActionUnknown`) store action of the open render pass.
    SetStoreAction {
        /// Attachment.
        attachment: AttachmentSlot,
        /// New, known store action.
        action: StoreAction,
    },
    /// Bind a render pipeline state (must match the open pass).
    SetRenderPipelineState(RenderPipelineId),
    /// Bind a depth/stencil state (`None` = Metal's default state).
    SetDepthStencilState(Option<DepthStencilStateId>),
    /// Bind a buffer to the vertex stage's buffer argument table.
    SetVertexBuffer {
        /// Argument table index.
        index: u32,
        /// Buffer (`None` = unbind).
        buffer: Option<BufferId>,
        /// Byte offset (< buffer length).
        offset: u64,
        /// Stride for layouts with dynamic stride (`None` = the layout's stride).
        attribute_stride: Option<u64>,
    },
    /// Bind a texture to a stage's texture argument table.
    SetTexture {
        /// Stage.
        stage: ShaderStage,
        /// Argument table index.
        index: u32,
        /// Texture (`None` = unbind).
        texture: Option<TextureId>,
    },
    /// Bind a sampler state to a stage's sampler argument table.
    SetSamplerState {
        /// Stage.
        stage: ShaderStage,
        /// Argument table index.
        index: u32,
        /// Sampler (`None` = unbind).
        sampler: Option<SamplerId>,
    },
    /// End the render encoder.
    EndRenderPass,
    /// Non-indexed draw.
    Draw {
        /// Primitive topology.
        primitive: PrimitiveType,
        /// First vertex.
        vertex_start: u32,
        /// Vertex count.
        vertex_count: u32,
        /// Instance count.
        instance_count: u32,
        /// First instance.
        base_instance: u32,
    },
    /// Start a compute encoder.
    BeginComputePass,
    /// End the compute encoder.
    EndComputePass,
    /// Dispatch threadgroups.
    Dispatch {
        /// Threadgroups per grid.
        threadgroups: [u32; 3],
        /// Threads per threadgroup.
        threads_per_threadgroup: [u32; 3],
    },
}

/// One committed command buffer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Submission {
    /// Commands in order.
    pub commands: Vec<GpuCommand>,
}

/// A device that executes the Metal object model.
///
/// Methods take `&mut self`: each backend instance is driven by one device thread; queue
/// submission threads are internal to the backend (gpu.md §8).
pub trait GpuBackend {
    /// Device capabilities.
    fn capabilities(&self) -> &Capabilities;

    /// Creates a buffer.
    ///
    /// # Errors
    ///
    /// Invalid descriptor, memory budget exhausted, host failure.
    fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId, GpuError>;

    /// Destroys a buffer (deferred until in-flight work referencing it completes).
    ///
    /// # Errors
    ///
    /// Invalid or stale id.
    fn destroy_buffer(&mut self, id: BufferId) -> Result<(), GpuError>;

    /// Writes CPU data into a buffer at `offset` (models CPU stores to `contents`).
    ///
    /// # Errors
    ///
    /// Invalid id, out-of-bounds range, non-CPU-accessible storage mode.
    fn write_buffer(&mut self, id: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError>;

    /// Creates a texture.
    ///
    /// # Errors
    ///
    /// Invalid descriptor, memory budget exhausted, host failure.
    fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId, GpuError>;

    /// Destroys a texture or texture view (deferred like [`GpuBackend::destroy_buffer`]). A
    /// texture keeps its storage while views of it exist (Metal views retain their parent).
    ///
    /// # Errors
    ///
    /// Invalid or stale id.
    fn destroy_texture(&mut self, id: TextureId) -> Result<(), GpuError>;

    /// Creates a view of a texture or of another view; returns a texture id.
    ///
    /// # Errors
    ///
    /// Invalid parent, view not allowed for the parent ([`TextureViewDesc::validate`]).
    fn create_texture_view(
        &mut self,
        parent: TextureId,
        desc: &TextureViewDesc,
    ) -> Result<TextureId, GpuError>;

    /// Creates a sampler state.
    ///
    /// # Errors
    ///
    /// Invalid descriptor, host failure.
    fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId, GpuError>;

    /// Destroys a sampler state (deferred like [`GpuBackend::destroy_buffer`]).
    ///
    /// # Errors
    ///
    /// Invalid or stale id.
    fn destroy_sampler(&mut self, id: SamplerId) -> Result<(), GpuError>;

    /// Creates a depth/stencil state.
    ///
    /// # Errors
    ///
    /// Invalid descriptor, host failure.
    fn create_depth_stencil_state(
        &mut self,
        desc: &DepthStencilDesc,
    ) -> Result<DepthStencilStateId, GpuError>;

    /// Destroys a depth/stencil state (deferred like [`GpuBackend::destroy_buffer`]).
    ///
    /// # Errors
    ///
    /// Invalid or stale id.
    fn destroy_depth_stencil_state(&mut self, id: DepthStencilStateId) -> Result<(), GpuError>;

    /// Creates a render pipeline state. Shader functions are not part of the descriptor yet
    /// (WP-SC-09/12); a Vulkan backend must reject draws with such a pipeline as unimplemented
    /// instead of drawing without shaders.
    ///
    /// # Errors
    ///
    /// Invalid descriptor, device gaps, host failure.
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDesc,
    ) -> Result<RenderPipelineId, GpuError>;

    /// Destroys a render pipeline state (deferred like [`GpuBackend::destroy_buffer`]).
    ///
    /// # Errors
    ///
    /// Invalid or stale id.
    fn destroy_render_pipeline(&mut self, id: RenderPipelineId) -> Result<(), GpuError>;

    /// Validates and submits a command buffer; returns its timeline position.
    ///
    /// Validation is complete before anything is executed: an invalid submission has no
    /// effect.
    ///
    /// # Errors
    ///
    /// Any invalid command (bad handle, out-of-bounds copy, encoder nesting) or host failure.
    fn submit(&mut self, submission: &Submission) -> Result<SubmissionToken, GpuError>;

    /// Highest completed submission (monotonic).
    fn completed(&self) -> SubmissionToken;
}

#[derive(Debug)]
struct NullBuffer {
    desc: BufferDesc,
}

#[derive(Debug)]
struct NullTexture {
    desc: TextureDesc,
    /// Bytes charged against the budget (0 for views and memoryless textures).
    bytes: u64,
    /// Texture this view was made of (`None` for textures).
    parent: Option<TextureId>,
    /// Number of live views made of this texture.
    views: u32,
    /// The client destroyed it, but views still keep it alive (no longer usable).
    released: bool,
}

/// Backend without a GPU: validates every call, tracks objects and memory, executes nothing.
/// Submissions complete immediately.
#[derive(Debug)]
pub struct NullBackend {
    caps: Capabilities,
    /// Emulations this backend claims to implement (device gaps beyond them are rejected).
    policy: EmulationPolicy,
    buffers: HandleTable<NullBuffer>,
    textures: HandleTable<NullTexture>,
    samplers: HandleTable<SamplerDesc>,
    depth_stencil_states: HandleTable<DepthStencilDesc>,
    /// Only what validation needs (the full descriptor is about 1.7 KiB).
    pipelines: HandleTable<PipelineTargets>,
    allocated: u64,
    last_submitted: u64,
    commands_executed: u64,
}

impl NullBackend {
    /// Creates a null backend that behaves like a device with `caps` and implements every
    /// emulation ([`EmulationPolicy::ALL`]: it executes nothing, so any emulation is
    /// trivially "implemented"). Device gaps that no emulation covers (e.g. sRGB storage
    /// writes, render targets in sample-only formats) are still rejected. Frontends apply
    /// their own, usually stricter, policy before the backend.
    #[must_use]
    pub fn new(caps: Capabilities) -> Self {
        Self::with_policy(caps, EmulationPolicy::ALL)
    }

    /// Creates a null backend that implements exactly the emulations of `policy`.
    #[must_use]
    pub fn with_policy(caps: Capabilities, policy: EmulationPolicy) -> Self {
        Self {
            caps,
            policy,
            buffers: HandleTable::new(),
            textures: HandleTable::new(),
            samplers: HandleTable::new(),
            depth_stencil_states: HandleTable::new(),
            pipelines: HandleTable::new(),
            allocated: 0,
            last_submitted: 0,
            commands_executed: 0,
        }
    }

    /// Live buffer count.
    #[must_use]
    pub fn live_buffers(&self) -> usize {
        self.buffers.len()
    }

    /// Live texture count: textures and views not yet destroyed, plus destroyed textures
    /// that views still keep alive.
    #[must_use]
    pub fn live_textures(&self) -> usize {
        self.textures.len()
    }

    /// Live sampler state count.
    #[must_use]
    pub fn live_samplers(&self) -> usize {
        self.samplers.len()
    }

    /// Live depth/stencil state count.
    #[must_use]
    pub fn live_depth_stencil_states(&self) -> usize {
        self.depth_stencil_states.len()
    }

    /// Live render pipeline state count.
    #[must_use]
    pub fn live_render_pipelines(&self) -> usize {
        self.pipelines.len()
    }

    /// Bytes currently charged against the device memory budget.
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        self.allocated
    }

    /// Total number of commands accepted in submissions.
    #[must_use]
    pub fn commands_executed(&self) -> u64 {
        self.commands_executed
    }

    /// Length of a live buffer.
    ///
    /// # Errors
    ///
    /// Invalid or stale id.
    pub fn buffer_length(&self, id: BufferId) -> Result<u64, GpuError> {
        Ok(self.buffer(id)?.desc.length)
    }

    fn buffer(&self, id: BufferId) -> Result<&NullBuffer, GpuError> {
        Ok(self.buffers.get(Handle::from_raw(id.0))?)
    }

    /// A usable texture (not destroyed by the client).
    fn texture(&self, id: TextureId) -> Result<&NullTexture, GpuError> {
        let t = self.textures.get(Handle::from_raw(id.0))?;
        if t.released {
            return Err(stale(id.0).into());
        }
        Ok(t)
    }

    fn pipeline(&self, id: RenderPipelineId) -> Result<&PipelineTargets, GpuError> {
        Ok(self.pipelines.get(Handle::from_raw(id.0))?)
    }

    /// Removes a texture whose client handle is gone and that no view needs any more; then
    /// releases its parent chain the same way. Bounded by the view chain length, iterative.
    fn drop_texture(&mut self, id: TextureId) -> Result<(), GpuError> {
        let mut next = Some(id);
        while let Some(id) = next.take() {
            let t = self.textures.remove(Handle::from_raw(id.0))?;
            self.allocated = self.allocated.saturating_sub(t.bytes);
            if let Some(parent) = t.parent {
                let p = self.textures.get_mut(Handle::from_raw(parent.0))?;
                p.views = p.views.saturating_sub(1);
                if p.released && p.views == 0 {
                    next = Some(parent);
                }
            }
        }
        Ok(())
    }

    fn charge(&mut self, bytes: u64) -> Result<(), GpuError> {
        let available = self.caps.device_local_bytes.saturating_sub(self.allocated);
        if bytes > available {
            return Err(GpuError::OutOfDeviceMemory {
                requested: bytes,
                available,
            });
        }
        self.allocated += bytes;
        Ok(())
    }

    fn check_range(&self, id: BufferId, offset: u64, size: u64) -> Result<(), GpuError> {
        let len = self.buffer(id)?.desc.length;
        match offset.checked_add(size) {
            Some(end) if end <= len => Ok(()),
            _ => Err(GpuError::Guest(format!(
                "range {offset}+{size} out of bounds for buffer of length {len}"
            ))),
        }
    }

    fn validate_submission(&self, s: &Submission) -> Result<(), GpuError> {
        let limits = &self.caps.limits;
        let lookup = |id| Ok::<_, GpuError>(self.texture(id)?.desc);
        let mut v = EncoderValidator::new();
        for (i, cmd) in s.commands.iter().enumerate() {
            let wrong = |what: &str| GpuError::Guest(format!("command {i}: {what}"));
            match cmd {
                GpuCommand::CopyBuffer {
                    src,
                    src_offset,
                    dst,
                    dst_offset,
                    size,
                } => {
                    v.blit()?;
                    self.check_range(*src, *src_offset, *size)?;
                    self.check_range(*dst, *dst_offset, *size)?;
                    // Metal forbids overlapping source and destination ranges.
                    if src == dst
                        && *size > 0
                        && *src_offset < dst_offset.saturating_add(*size)
                        && *dst_offset < src_offset.saturating_add(*size)
                    {
                        return Err(wrong("overlapping copy within one buffer"));
                    }
                }
                GpuCommand::BeginRenderPass(pass) => {
                    v.begin_render(pass, limits, &self.caps, lookup)?;
                }
                GpuCommand::SetStoreAction { attachment, action } => {
                    v.set_store_action(*attachment, *action, limits, &self.caps, lookup)?;
                }
                GpuCommand::SetRenderPipelineState(id) => v.set_pipeline(self.pipeline(*id)?)?,
                GpuCommand::SetDepthStencilState(id) => {
                    v.render_command()?;
                    if let Some(id) = id {
                        self.depth_stencil_states.get(Handle::from_raw(id.0))?;
                    }
                }
                GpuCommand::SetVertexBuffer {
                    index,
                    buffer,
                    offset,
                    attribute_stride,
                } => {
                    v.set_vertex_buffer(*index, buffer.is_some(), attribute_stride.is_some())?;
                    if *index >= MAX_BUFFER_BINDINGS {
                        return Err(wrong("buffer index out of range"));
                    }
                    if let Some(b) = buffer
                        && *offset >= self.buffer(*b)?.desc.length
                    {
                        return Err(wrong("vertex buffer offset beyond the buffer"));
                    }
                }
                GpuCommand::SetTexture { index, texture, .. } => {
                    v.render_command()?;
                    if *index >= MAX_TEXTURE_BINDINGS {
                        return Err(wrong("texture index out of range"));
                    }
                    if let Some(t) = texture {
                        self.texture(*t)?;
                    }
                }
                GpuCommand::SetSamplerState { index, sampler, .. } => {
                    v.render_command()?;
                    if *index >= MAX_SAMPLER_BINDINGS {
                        return Err(wrong("sampler index out of range"));
                    }
                    if let Some(id) = sampler {
                        self.samplers.get(Handle::from_raw(id.0))?;
                    }
                }
                GpuCommand::EndRenderPass => v.end_render()?,
                GpuCommand::Draw { primitive, .. } => v.draw(*primitive)?,
                GpuCommand::BeginComputePass => v.begin_compute()?,
                GpuCommand::EndComputePass => v.end_compute()?,
                GpuCommand::Dispatch {
                    threads_per_threadgroup,
                    ..
                } => v.dispatch(*threads_per_threadgroup, limits)?,
            }
        }
        v.finish()?;
        Ok(())
    }
}

fn stale(raw: RawHandle) -> HandleError {
    HandleError::Stale {
        index: raw.index(),
        generation: raw.generation(),
    }
}

impl GpuBackend for NullBackend {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId, GpuError> {
        desc.validate(&self.caps.limits)?;
        self.charge(desc.length)?;
        match self.buffers.insert(NullBuffer { desc: *desc }) {
            Ok(h) => Ok(BufferId(h.raw())),
            Err(e) => {
                self.allocated -= desc.length;
                Err(e.into())
            }
        }
    }

    fn destroy_buffer(&mut self, id: BufferId) -> Result<(), GpuError> {
        let b = self.buffers.remove(Handle::from_raw(id.0))?;
        self.allocated = self.allocated.saturating_sub(b.desc.length);
        Ok(())
    }

    fn write_buffer(&mut self, id: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let mode = self.buffer(id)?.desc.storage_mode;
        if !matches!(mode, StorageMode::Shared | StorageMode::Managed) {
            return Err(GpuError::Guest(format!(
                "CPU write to buffer with storage mode {mode:?}"
            )));
        }
        let size =
            u64::try_from(data.len()).map_err(|_| GpuError::Guest("write too large".into()))?;
        self.check_range(id, offset, size)
    }

    fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId, GpuError> {
        desc.validate(&self.caps.limits)?;
        texture_support(&self.caps, &self.policy, desc)?;
        // Memoryless textures live in tile memory on Apple GPUs; on IMR GPUs they become
        // transient attachments that a real backend allocates per pass. No budget charge here.
        let bytes = if desc.storage_mode == StorageMode::Memoryless {
            0
        } else {
            desc.size_in_bytes()?
        };
        self.charge(bytes)?;
        let texture = NullTexture {
            desc: *desc,
            bytes,
            parent: None,
            views: 0,
            released: false,
        };
        match self.textures.insert(texture) {
            Ok(h) => Ok(TextureId(h.raw())),
            Err(e) => {
                self.allocated -= bytes;
                Err(e.into())
            }
        }
    }

    fn destroy_texture(&mut self, id: TextureId) -> Result<(), GpuError> {
        let t = self.textures.get_mut(Handle::from_raw(id.0))?;
        if t.released {
            return Err(stale(id.0).into());
        }
        if t.views > 0 {
            // Views retain their parent: keep the storage, refuse further client use.
            t.released = true;
            return Ok(());
        }
        self.drop_texture(id)
    }

    fn create_texture_view(
        &mut self,
        parent: TextureId,
        desc: &TextureViewDesc,
    ) -> Result<TextureId, GpuError> {
        let view = desc.validate(&self.texture(parent)?.desc)?;
        texture_support(&self.caps, &self.policy, &view)?;
        let h = self.textures.insert(NullTexture {
            desc: view,
            bytes: 0,
            parent: Some(parent),
            views: 0,
            released: false,
        })?;
        match self.textures.get_mut(Handle::from_raw(parent.0)) {
            Ok(p) => {
                // At most `DEFAULT_MAX_SLOTS` views exist, far below u32::MAX.
                p.views = p.views.saturating_add(1);
                Ok(TextureId(h.raw()))
            }
            Err(e) => {
                // Unreachable: the parent was looked up above. Undo the insert regardless.
                let _ = self.textures.remove(h);
                Err(e.into())
            }
        }
    }

    fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId, GpuError> {
        desc.validate()?;
        Ok(SamplerId(self.samplers.insert(*desc)?.raw()))
    }

    fn destroy_sampler(&mut self, id: SamplerId) -> Result<(), GpuError> {
        self.samplers.remove(Handle::from_raw(id.0))?;
        Ok(())
    }

    fn create_depth_stencil_state(
        &mut self,
        desc: &DepthStencilDesc,
    ) -> Result<DepthStencilStateId, GpuError> {
        desc.validate()?;
        Ok(DepthStencilStateId(
            self.depth_stencil_states.insert(*desc)?.raw(),
        ))
    }

    fn destroy_depth_stencil_state(&mut self, id: DepthStencilStateId) -> Result<(), GpuError> {
        self.depth_stencil_states.remove(Handle::from_raw(id.0))?;
        Ok(())
    }

    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDesc,
    ) -> Result<RenderPipelineId, GpuError> {
        desc.validate(&self.caps.limits)?;
        // Formats, sample count and vertex input (`vertex_input_support`) on this device.
        render_pipeline_support(&self.caps, &self.policy, desc)?;
        Ok(RenderPipelineId(
            self.pipelines.insert(desc.targets())?.raw(),
        ))
    }

    fn destroy_render_pipeline(&mut self, id: RenderPipelineId) -> Result<(), GpuError> {
        self.pipelines.remove(Handle::from_raw(id.0))?;
        Ok(())
    }

    fn submit(&mut self, submission: &Submission) -> Result<SubmissionToken, GpuError> {
        self.validate_submission(submission)?;
        let next = self
            .last_submitted
            .checked_add(1)
            .ok_or_else(|| GpuError::Host("submission timeline exhausted".into()))?;
        self.last_submitted = next;
        self.commands_executed = self
            .commands_executed
            .saturating_add(submission.commands.len() as u64);
        Ok(SubmissionToken(next))
    }

    fn completed(&self) -> SubmissionToken {
        SubmissionToken(self.last_submitted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AttachmentDesc, ClearColor, ColorAttachmentDesc, CpuCacheMode, DepthAttachmentDesc,
        HazardTrackingMode, LoadAction, MultisampleDepthResolveFilter,
        MultisampleStencilResolveFilter, PixelFormat, ResolveTarget, StencilAttachmentDesc,
        TextureType, TextureUsage,
    };

    fn backend() -> NullBackend {
        NullBackend::new(Capabilities::iris_xe_assumed())
    }

    fn buf(len: u64, mode: StorageMode) -> BufferDesc {
        BufferDesc {
            length: len,
            storage_mode: mode,
            cpu_cache_mode: CpuCacheMode::DefaultCache,
            hazard_tracking: HazardTrackingMode::Default,
        }
    }

    fn rt(w: u32, h: u32) -> TextureDesc {
        TextureDesc::new_2d(
            PixelFormat::BGRA8Unorm,
            w,
            h,
            TextureUsage::RENDER_TARGET.union(TextureUsage::SHADER_READ),
        )
    }

    fn color(texture: TextureId) -> ColorAttachmentDesc<TextureId> {
        ColorAttachmentDesc {
            attachment: AttachmentDesc::new(texture, LoadAction::Clear, StoreAction::Store),
            clear_color: ClearColor::default(),
        }
    }

    fn begin(atts: &[ColorAttachmentDesc<TextureId>], width: u32, height: u32) -> GpuCommand {
        let mut pass = RenderPassDesc::empty();
        for (slot, a) in pass.color_attachments.iter_mut().zip(atts) {
            *slot = Some(*a);
        }
        pass.render_target_width = width;
        pass.render_target_height = height;
        GpuCommand::BeginRenderPass(Box::new(pass))
    }

    #[test]
    fn buffer_lifecycle_and_accounting() {
        let mut b = backend();
        let id = b.create_buffer(&buf(4096, StorageMode::Shared)).unwrap();
        assert_eq!(b.live_buffers(), 1);
        assert_eq!(b.allocated_bytes(), 4096);
        assert_eq!(b.buffer_length(id).unwrap(), 4096);
        b.destroy_buffer(id).unwrap();
        assert_eq!(b.allocated_bytes(), 0);
        assert!(matches!(b.destroy_buffer(id), Err(GpuError::Handle(_))));
        assert!(b.buffer_length(id).is_err());
    }

    #[test]
    fn invalid_descriptors_rejected() {
        let mut b = backend();
        assert_eq!(
            b.create_buffer(&buf(0, StorageMode::Shared)),
            Err(GpuError::Model(ModelError::ZeroLength))
        );
        assert!(b.create_texture(&rt(0, 1)).is_err());
        assert_eq!(b.live_buffers() + b.live_textures(), 0);
        assert_eq!(b.allocated_bytes(), 0);
    }

    #[test]
    fn memory_budget_enforced() {
        let mut b = backend();
        let max = b.capabilities().limits.max_buffer_length;
        let budget = b.capabilities().device_local_bytes;
        let mut ids = Vec::new();
        let mut err = None;
        for _ in 0..(budget / max + 2) {
            match b.create_buffer(&buf(max, StorageMode::Private)) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        assert!(matches!(err, Some(GpuError::OutOfDeviceMemory { .. })));
        assert!(b.allocated_bytes() <= budget);
        b.destroy_buffer(ids[0]).unwrap();
        assert!(b.create_buffer(&buf(max, StorageMode::Private)).is_ok());
    }

    #[test]
    fn write_buffer_bounds_and_storage_mode() {
        let mut b = backend();
        let shared = b.create_buffer(&buf(16, StorageMode::Shared)).unwrap();
        let private = b.create_buffer(&buf(16, StorageMode::Private)).unwrap();
        assert!(b.write_buffer(shared, 8, &[0; 8]).is_ok());
        assert!(b.write_buffer(shared, 9, &[0; 8]).is_err());
        assert!(b.write_buffer(shared, u64::MAX, &[0; 1]).is_err());
        assert!(b.write_buffer(private, 0, &[0; 1]).is_err());
    }

    #[test]
    fn stale_texture_handle_in_submission_rejected() {
        let mut b = backend();
        let t = b.create_texture(&rt(64, 64)).unwrap();
        b.destroy_texture(t).unwrap();
        let s = Submission {
            commands: vec![begin(&[color(t)], 0, 0), GpuCommand::EndRenderPass],
        };
        assert!(matches!(b.submit(&s), Err(GpuError::Handle(_))));
        assert_eq!(b.completed(), SubmissionToken(0));
    }

    fn pipeline(b: &mut NullBackend, format: PixelFormat) -> RenderPipelineId {
        let mut p = RenderPipelineDesc::default();
        p.color_attachments[0].pixel_format = Some(format);
        b.create_render_pipeline(&p).unwrap()
    }

    #[test]
    fn valid_triangle_submission() {
        let mut b = backend();
        let t = b.create_texture(&rt(640, 480)).unwrap();
        let p = pipeline(&mut b, PixelFormat::BGRA8Unorm);
        let s = Submission {
            commands: vec![
                begin(&[color(t)], 640, 480),
                GpuCommand::SetRenderPipelineState(p),
                GpuCommand::Draw {
                    primitive: PrimitiveType::Triangle,
                    vertex_start: 0,
                    vertex_count: 3,
                    instance_count: 1,
                    base_instance: 0,
                },
                GpuCommand::EndRenderPass,
            ],
        };
        assert_eq!(b.submit(&s), Ok(SubmissionToken(1)));
        assert_eq!(b.submit(&s), Ok(SubmissionToken(2)));
        assert_eq!(b.completed(), SubmissionToken(2));
        assert_eq!(b.commands_executed(), 8);
        // Without a pipeline the draw is an invalid command.
        let mut no_pipe = s.clone();
        no_pipe.commands.remove(1);
        assert!(matches!(
            b.submit(&no_pipe),
            Err(GpuError::Model(ModelError::EncoderState(_)))
        ));
        // A pipeline for another format does not match the pass.
        let other = pipeline(&mut b, PixelFormat::RGBA16Float);
        let mut wrong = s.clone();
        wrong.commands[1] = GpuCommand::SetRenderPipelineState(other);
        assert!(matches!(
            b.submit(&wrong),
            Err(GpuError::Model(ModelError::InvalidValue { .. }))
        ));
        // Destroyed pipelines are stale.
        b.destroy_render_pipeline(p).unwrap();
        assert!(matches!(b.submit(&s), Err(GpuError::Handle(_))));
        assert_eq!(b.completed(), SubmissionToken(2));
    }

    #[test]
    fn encoder_nesting_rules() {
        let mut b = backend();
        let draw = GpuCommand::Draw {
            primitive: PrimitiveType::Point,
            vertex_start: 0,
            vertex_count: 1,
            instance_count: 1,
            base_instance: 0,
        };
        let dispatch = GpuCommand::Dispatch {
            threadgroups: [1, 1, 1],
            threads_per_threadgroup: [32, 1, 1],
        };
        let bad: Vec<Vec<GpuCommand>> = vec![
            vec![draw.clone()],
            vec![dispatch.clone()],
            vec![GpuCommand::EndRenderPass],
            vec![GpuCommand::EndComputePass],
            vec![GpuCommand::BeginComputePass],
            vec![GpuCommand::BeginComputePass, GpuCommand::BeginComputePass],
            vec![
                GpuCommand::BeginComputePass,
                draw,
                GpuCommand::EndComputePass,
            ],
        ];
        for cmds in bad {
            let s = Submission { commands: cmds };
            assert!(b.submit(&s).is_err(), "{s:?}");
        }
        let ok = Submission {
            commands: vec![
                GpuCommand::BeginComputePass,
                dispatch,
                GpuCommand::EndComputePass,
            ],
        };
        assert!(b.submit(&ok).is_ok());
    }

    #[test]
    fn dispatch_threadgroup_size_limits() {
        let mut b = backend();
        for (tpt, ok) in [
            ([1024, 1, 1], true),
            ([1025, 1, 1], false),
            ([0, 1, 1], false),
            ([u32::MAX, u32::MAX, u32::MAX], false),
            ([32, 32, 1], true),
            ([32, 32, 2], false),
        ] {
            let s = Submission {
                commands: vec![
                    GpuCommand::BeginComputePass,
                    GpuCommand::Dispatch {
                        threadgroups: [u32::MAX, 1, 1],
                        threads_per_threadgroup: tpt,
                    },
                    GpuCommand::EndComputePass,
                ],
            };
            assert_eq!(b.submit(&s).is_ok(), ok, "{tpt:?}");
        }
    }

    #[test]
    fn copy_bounds_and_overlap() {
        let mut b = backend();
        let a = b.create_buffer(&buf(256, StorageMode::Shared)).unwrap();
        let c = b.create_buffer(&buf(128, StorageMode::Private)).unwrap();
        let copy = |src, so, dst, d_o, size| Submission {
            commands: vec![GpuCommand::CopyBuffer {
                src,
                src_offset: so,
                dst,
                dst_offset: d_o,
                size,
            }],
        };
        assert!(b.submit(&copy(a, 0, c, 0, 128)).is_ok());
        assert!(b.submit(&copy(a, 0, c, 1, 128)).is_err(), "dst overflow");
        assert!(
            b.submit(&copy(a, u64::MAX, c, 0, 2)).is_err(),
            "offset overflow"
        );
        assert!(
            b.submit(&copy(a, 0, a, 128, 128)).is_ok(),
            "adjacent ranges"
        );
        assert!(b.submit(&copy(a, 0, a, 64, 128)).is_err(), "overlap");
        let s = Submission {
            commands: vec![
                GpuCommand::BeginComputePass,
                GpuCommand::CopyBuffer {
                    src: a,
                    src_offset: 0,
                    dst: c,
                    dst_offset: 0,
                    size: 1,
                },
                GpuCommand::EndComputePass,
            ],
        };
        assert!(b.submit(&s).is_err(), "blit inside compute encoder");
    }

    #[test]
    fn render_pass_attachment_rules() {
        let mut b = backend();
        let ok_tex = b.create_texture(&rt(64, 64)).unwrap();
        let sampled_only = b
            .create_texture(&TextureDesc::new_2d(
                PixelFormat::RGBA8Unorm,
                64,
                64,
                TextureUsage::SHADER_READ,
            ))
            .unwrap();
        let memoryless = b
            .create_texture(&TextureDesc {
                storage_mode: StorageMode::Memoryless,
                usage: TextureUsage::RENDER_TARGET,
                ..rt(64, 64)
            })
            .unwrap();
        let pass = |atts: Vec<ColorAttachmentDesc<TextureId>>, w, h| Submission {
            commands: vec![begin(&atts, w, h), GpuCommand::EndRenderPass],
        };
        assert!(b.submit(&pass(vec![color(ok_tex)], 64, 64)).is_ok());
        assert!(b.submit(&pass(vec![color(ok_tex)], 65, 64)).is_err());
        assert!(b.submit(&pass(vec![color(sampled_only)], 0, 0)).is_err());
        assert!(
            b.submit(&pass(vec![color(ok_tex), color(ok_tex)], 0, 0))
                .is_err()
        );
        assert!(b.submit(&pass(vec![], 0, 0)).is_err());
        assert!(b.submit(&pass(vec![], 16, 16)).is_ok());
        // Memoryless: clear + don't-care is fine, store is not.
        let mut m = color(memoryless);
        m.attachment.store = StoreAction::DontCare;
        assert!(b.submit(&pass(vec![m], 0, 0)).is_ok());
        assert!(b.submit(&pass(vec![color(memoryless)], 0, 0)).is_err());
        assert_eq!(
            b.allocated_bytes(),
            2 * 64 * 64 * 4,
            "memoryless not charged"
        );
    }

    #[test]
    fn msaa_resolve_depth_stencil_and_deferred_store() {
        let mut b = backend();
        let msaa = b
            .create_texture(&TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 4,
                storage_mode: StorageMode::Memoryless,
                usage: TextureUsage::RENDER_TARGET,
                ..rt(64, 64)
            })
            .unwrap();
        let resolved = b.create_texture(&rt(64, 64)).unwrap();
        let mut c = color(msaa);
        c.attachment.store = StoreAction::MultisampleResolve;
        let one = |cmd| Submission {
            commands: vec![cmd, GpuCommand::EndRenderPass],
        };
        // Resolve store action without a resolve texture: guest error, not a misrender.
        assert!(matches!(
            b.submit(&one(begin(&[c], 0, 0))),
            Err(GpuError::Model(ModelError::InvalidAttachment { .. }))
        ));
        c.attachment.resolve = Some(ResolveTarget {
            texture: resolved,
            level: 0,
            slice: 0,
            depth_plane: 0,
        });
        assert!(b.submit(&one(begin(&[c], 0, 0))).is_ok());

        // Combined depth/stencil texture: fine. Separate textures: valid Metal, but not
        // expressible with dynamic rendering yet → unsupported, never silently dropped.
        let ds = |fmt| TextureDesc::new_2d(fmt, 64, 64, TextureUsage::RENDER_TARGET);
        let combined = b
            .create_texture(&ds(PixelFormat::Depth32Float_Stencil8))
            .unwrap();
        let depth_only = b.create_texture(&ds(PixelFormat::Depth32Float)).unwrap();
        let stencil_only = b.create_texture(&ds(PixelFormat::Stencil8)).unwrap();
        let with_ds = |d, s| {
            let mut pass = RenderPassDesc::empty();
            pass.color_attachments[0] = Some(color(resolved));
            pass.depth_attachment = Some(DepthAttachmentDesc {
                attachment: AttachmentDesc::new(d, LoadAction::Clear, StoreAction::DontCare),
                clear_depth: 1.0,
                resolve_filter: MultisampleDepthResolveFilter::Sample0,
            });
            pass.stencil_attachment = Some(StencilAttachmentDesc {
                attachment: AttachmentDesc::new(s, LoadAction::Clear, StoreAction::DontCare),
                clear_stencil: 0,
                resolve_filter: MultisampleStencilResolveFilter::Sample0,
            });
            one(GpuCommand::BeginRenderPass(Box::new(pass)))
        };
        assert!(b.submit(&with_ds(combined, combined)).is_ok());
        let e = b.submit(&with_ds(depth_only, stencil_only)).unwrap_err();
        assert!(e.is_unsupported(), "{e:?}");
        assert!(matches!(
            ritsu_core::Error::from(e),
            ritsu_core::Error::Unimplemented(_)
        ));

        // A deferred store action that is never set fails at the end of the encoder.
        let mut d = color(resolved);
        d.attachment.store = StoreAction::Unknown;
        let e = b.submit(&one(begin(&[d], 0, 0))).unwrap_err();
        assert!(
            matches!(e, GpuError::Model(ModelError::EncoderState(_))),
            "{e:?}"
        );
        // Setting it before the end makes the pass valid.
        let set = GpuCommand::SetStoreAction {
            attachment: AttachmentSlot::Color(0),
            action: StoreAction::Store,
        };
        let s = Submission {
            commands: vec![begin(&[d], 0, 0), set, GpuCommand::EndRenderPass],
        };
        assert!(b.submit(&s).is_ok());
    }

    #[test]
    fn texture_views_retain_their_parent() {
        let mut b = backend();
        let parent = b.create_texture(&rt(64, 64)).unwrap();
        let view_desc = crate::model::TextureViewDesc {
            pixel_format: PixelFormat::BGRA8Unorm_sRGB,
            texture_type: TextureType::Type2D,
            levels: crate::model::SubresourceRange::new(0, 1),
            slices: crate::model::SubresourceRange::new(0, 1),
            swizzle: crate::model::TextureSwizzleChannels::IDENTITY,
        };
        let view = b.create_texture_view(parent, &view_desc).unwrap();
        let view2 = b.create_texture_view(view, &view_desc).unwrap();
        assert_eq!(b.live_textures(), 3);
        assert_eq!(
            b.allocated_bytes(),
            64 * 64 * 4,
            "views share the parent's storage"
        );
        // Invalid views are rejected by the model.
        let bad = crate::model::TextureViewDesc {
            pixel_format: PixelFormat::RGBA16Float,
            ..view_desc
        };
        assert!(matches!(
            b.create_texture_view(parent, &bad),
            Err(GpuError::Model(ModelError::IncompatibleViewFormat { .. }))
        ));
        b.destroy_texture(parent).unwrap();
        assert_eq!(b.live_textures(), 3, "kept alive by its views");
        assert_eq!(b.allocated_bytes(), 64 * 64 * 4);
        // The destroyed parent is unusable: no new views, no passes, no second destroy.
        assert!(matches!(
            b.create_texture_view(parent, &view_desc),
            Err(GpuError::Handle(_))
        ));
        assert!(matches!(
            b.destroy_texture(parent),
            Err(GpuError::Handle(_))
        ));
        let pass = |t| Submission {
            commands: vec![begin(&[color(t)], 0, 0), GpuCommand::EndRenderPass],
        };
        assert!(matches!(b.submit(&pass(parent)), Err(GpuError::Handle(_))));
        assert!(b.submit(&pass(view2)).is_ok(), "views stay usable");
        b.destroy_texture(view).unwrap();
        assert_eq!(b.live_textures(), 3, "view2 still holds view and parent");
        b.destroy_texture(view2).unwrap();
        assert_eq!(b.live_textures(), 0);
        assert_eq!(b.allocated_bytes(), 0);
    }

    #[test]
    fn binds_are_validated_at_submit() {
        let mut b = backend();
        let t = b.create_texture(&rt(64, 64)).unwrap();
        let buf_id = b.create_buffer(&buf(64, StorageMode::Shared)).unwrap();
        let sampler = b.create_sampler(&SamplerDesc::default()).unwrap();
        let dss = b
            .create_depth_stencil_state(&DepthStencilDesc::default())
            .unwrap();
        let in_pass = |cmd: GpuCommand| Submission {
            commands: vec![begin(&[color(t)], 0, 0), cmd, GpuCommand::EndRenderPass],
        };
        let tex = |index, texture| GpuCommand::SetTexture {
            stage: ShaderStage::Fragment,
            index,
            texture,
        };
        let smp = |index, sampler| GpuCommand::SetSamplerState {
            stage: ShaderStage::Vertex,
            index,
            sampler,
        };
        let vb = |index, offset| GpuCommand::SetVertexBuffer {
            index,
            buffer: Some(buf_id),
            offset,
            attribute_stride: None,
        };
        for ok in [
            tex(127, Some(t)),
            tex(0, None),
            smp(15, Some(sampler)),
            vb(30, 63),
            GpuCommand::SetDepthStencilState(Some(dss)),
            GpuCommand::SetDepthStencilState(None),
        ] {
            assert!(b.submit(&in_pass(ok.clone())).is_ok(), "{ok:?}");
            // Outside a render pass every bind is an order error.
            let outside = Submission {
                commands: vec![ok.clone()],
            };
            assert!(matches!(
                b.submit(&outside),
                Err(GpuError::Model(ModelError::EncoderState(_)))
            ));
        }
        for bad in [tex(128, Some(t)), smp(16, None), vb(31, 0), vb(0, 64)] {
            assert!(
                matches!(b.submit(&in_pass(bad.clone())), Err(GpuError::Guest(_))),
                "{bad:?}"
            );
        }
        b.destroy_sampler(sampler).unwrap();
        b.destroy_depth_stencil_state(dss).unwrap();
        for stale in [
            smp(0, Some(sampler)),
            GpuCommand::SetDepthStencilState(Some(dss)),
        ] {
            assert!(matches!(
                b.submit(&in_pass(stale)),
                Err(GpuError::Handle(_))
            ));
        }
        assert_eq!((b.live_samplers(), b.live_depth_stencil_states()), (0, 0));
        // Descriptors are validated on creation as well.
        let bad_sampler = SamplerDesc {
            max_anisotropy: 17,
            ..SamplerDesc::default()
        };
        assert!(b.create_sampler(&bad_sampler).is_err());
    }

    #[test]
    fn pipelines_are_checked_against_the_device() {
        use crate::model::{VertexAttributeDesc, VertexDescriptor, VertexFormat};
        let mut p = RenderPipelineDesc::default();
        p.color_attachments[0].pixel_format = Some(PixelFormat::BGRA8Unorm);
        let mut v = VertexDescriptor::default();
        v.attributes[30] = Some(VertexAttributeDesc {
            format: VertexFormat::Float,
            offset: 0,
            buffer_index: 0,
        });
        p.vertex_descriptor = Some(v);
        // RTX (assumed): 32 attributes. Iris Xe (assumed): 29 → the model rejects index 30
        // against the device limits the backend validates with.
        let mut rtx = NullBackend::new(Capabilities::rtx4050_assumed());
        assert!(rtx.create_render_pipeline(&p).is_ok());
        assert_eq!(rtx.live_render_pipelines(), 1);
        assert!(backend().create_render_pipeline(&p).is_err());
    }

    #[test]
    fn device_gaps_are_rejected_by_the_backend_too() {
        use crate::model::TextureViewDesc;
        let caps = Capabilities::rtx4050_assumed();
        let astc =
            TextureDesc::new_2d(PixelFormat::ASTC_4x4_LDR, 64, 64, TextureUsage::SHADER_READ);
        // Without transcoding ASTC has no device support; the default implements it.
        let mut native = NullBackend::with_policy(caps.clone(), EmulationPolicy::NATIVE_ONLY);
        assert!(native.create_texture(&astc).unwrap_err().is_unsupported());
        let mut b = NullBackend::new(caps);
        assert!(b.create_texture(&astc).is_ok());
        // sRGB storage writes have no emulation: a writable sRGB view is unsupported.
        let parent = b
            .create_texture(&TextureDesc::new_2d(
                PixelFormat::RGBA8Unorm,
                64,
                64,
                TextureUsage::SHADER_WRITE.union(TextureUsage::SHADER_READ),
            ))
            .unwrap();
        let srgb = TextureViewDesc {
            pixel_format: PixelFormat::RGBA8Unorm_sRGB,
            texture_type: TextureType::Type2D,
            levels: crate::model::SubresourceRange::new(0, 1),
            slices: crate::model::SubresourceRange::new(0, 1),
            swizzle: crate::model::TextureSwizzleChannels::IDENTITY,
        };
        assert!(
            b.create_texture_view(parent, &srgb)
                .unwrap_err()
                .is_unsupported()
        );
        // RGB9E5Float is colour-renderable in Metal but not on the GPU [Annahme].
        let mut p = RenderPipelineDesc::default();
        p.color_attachments[0].pixel_format = Some(PixelFormat::RGB9E5Float);
        assert!(b.create_render_pipeline(&p).unwrap_err().is_unsupported());
        assert_eq!(b.live_render_pipelines(), 0);
    }

    #[test]
    fn gpu_error_maps_to_core_error_categories() {
        assert!(matches!(
            ritsu_core::Error::from(GpuError::Unimplemented("x")),
            ritsu_core::Error::Unimplemented("x")
        ));
        assert!(matches!(
            ritsu_core::Error::from(GpuError::Host("lost".into())),
            ritsu_core::Error::Host(_)
        ));
        assert!(matches!(
            ritsu_core::Error::from(GpuError::Guest("bad".into())),
            ritsu_core::Error::Guest(_)
        ));
    }
}
