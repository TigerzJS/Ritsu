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

use crate::caps::Capabilities;
use crate::handle::{Handle, HandleError, HandleTable, RawHandle};
use crate::model::{
    BufferDesc, LoadAction, ModelError, PrimitiveType, StorageMode, StoreAction, TextureDesc,
    TextureUsage,
};

/// Backend-neutral buffer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferId(pub RawHandle);

/// Backend-neutral texture identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureId(pub RawHandle);

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

impl From<GpuError> for ritsu_core::Error {
    fn from(e: GpuError) -> Self {
        match e {
            GpuError::Unimplemented(what) => ritsu_core::Error::Unimplemented(what),
            GpuError::Host(msg) => ritsu_core::Error::Host(msg),
            other => ritsu_core::Error::Guest(format!("gpu: {other}")),
        }
    }
}

/// A colour attachment of a render pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorAttachment {
    /// Attachment texture.
    pub texture: TextureId,
    /// Load action.
    pub load: LoadAction,
    /// Store action.
    pub store: StoreAction,
    /// Clear colour (used with [`LoadAction::Clear`]).
    pub clear_color: [f32; 4],
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
    /// Start a render encoder.
    BeginRenderPass {
        /// Render area width (0 = from first attachment).
        width: u32,
        /// Render area height (0 = from first attachment).
        height: u32,
        /// Colour attachments.
        color_attachments: Vec<ColorAttachment>,
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

    /// Destroys a texture (deferred like [`GpuBackend::destroy_buffer`]).
    ///
    /// # Errors
    ///
    /// Invalid or stale id.
    fn destroy_texture(&mut self, id: TextureId) -> Result<(), GpuError>;

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
    bytes: u64,
}

/// Backend without a GPU: validates every call, tracks objects and memory, executes nothing.
/// Submissions complete immediately.
#[derive(Debug)]
pub struct NullBackend {
    caps: Capabilities,
    buffers: HandleTable<NullBuffer>,
    textures: HandleTable<NullTexture>,
    allocated: u64,
    last_submitted: u64,
    commands_executed: u64,
}

impl NullBackend {
    /// Creates a null backend that behaves like a device with `caps`.
    #[must_use]
    pub fn new(caps: Capabilities) -> Self {
        Self {
            caps,
            buffers: HandleTable::new(),
            textures: HandleTable::new(),
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

    /// Live texture count.
    #[must_use]
    pub fn live_textures(&self) -> usize {
        self.textures.len()
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

    fn texture(&self, id: TextureId) -> Result<&NullTexture, GpuError> {
        Ok(self.textures.get(Handle::from_raw(id.0))?)
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

    fn validate_render_pass(
        &self,
        width: u32,
        height: u32,
        attachments: &[ColorAttachment],
    ) -> Result<(), GpuError> {
        let max = self.caps.limits.max_color_attachments as usize;
        if attachments.len() > max {
            return Err(GpuError::Guest(format!(
                "{} colour attachments exceed limit {max}",
                attachments.len()
            )));
        }
        if attachments.is_empty() && (width == 0 || height == 0) {
            return Err(GpuError::Guest(
                "render pass without attachments needs an explicit render area".into(),
            ));
        }
        for (i, a) in attachments.iter().enumerate() {
            if attachments.iter().take(i).any(|b| b.texture == a.texture) {
                return Err(GpuError::Guest(format!("attachment {i} bound twice")));
            }
            let t = self.texture(a.texture)?;
            let d = &t.desc;
            if !d.usage.contains(TextureUsage::RENDER_TARGET) {
                return Err(GpuError::Guest(format!(
                    "attachment {i} lacks renderTarget usage"
                )));
            }
            if d.pixel_format.is_compressed() || d.pixel_format.is_depth_stencil() {
                return Err(GpuError::Guest(format!(
                    "attachment {i} has non-colour-renderable format {:?}",
                    d.pixel_format
                )));
            }
            if d.storage_mode == StorageMode::Memoryless
                && (a.load == LoadAction::Load || a.store == StoreAction::Store)
            {
                return Err(GpuError::Guest(format!(
                    "memoryless attachment {i} cannot be loaded or stored"
                )));
            }
            if width > d.width || height > d.height {
                return Err(GpuError::Guest(format!(
                    "render area {width}x{height} exceeds attachment {i}"
                )));
            }
        }
        Ok(())
    }

    fn validate_submission(&self, s: &Submission) -> Result<(), GpuError> {
        #[derive(PartialEq)]
        enum Enc {
            None,
            Render,
            Compute,
        }
        let mut enc = Enc::None;
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
                    if enc != Enc::None {
                        return Err(wrong("blit inside another encoder"));
                    }
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
                GpuCommand::BeginRenderPass {
                    width,
                    height,
                    color_attachments,
                } => {
                    if enc != Enc::None {
                        return Err(wrong("nested encoder"));
                    }
                    self.validate_render_pass(*width, *height, color_attachments)?;
                    enc = Enc::Render;
                }
                GpuCommand::EndRenderPass => {
                    if enc != Enc::Render {
                        return Err(wrong("EndRenderPass without render encoder"));
                    }
                    enc = Enc::None;
                }
                GpuCommand::Draw { .. } => {
                    if enc != Enc::Render {
                        return Err(wrong("draw outside render encoder"));
                    }
                }
                GpuCommand::BeginComputePass => {
                    if enc != Enc::None {
                        return Err(wrong("nested encoder"));
                    }
                    enc = Enc::Compute;
                }
                GpuCommand::EndComputePass => {
                    if enc != Enc::Compute {
                        return Err(wrong("EndComputePass without compute encoder"));
                    }
                    enc = Enc::None;
                }
                GpuCommand::Dispatch {
                    threads_per_threadgroup,
                    ..
                } => {
                    if enc != Enc::Compute {
                        return Err(wrong("dispatch outside compute encoder"));
                    }
                    let total = threads_per_threadgroup
                        .iter()
                        .try_fold(1u64, |acc, &v| acc.checked_mul(u64::from(v)));
                    let max = u64::from(self.caps.limits.max_threads_per_threadgroup);
                    if !matches!(total, Some(t) if (1..=max).contains(&t)) {
                        return Err(wrong("threads per threadgroup out of range"));
                    }
                }
            }
        }
        if enc != Enc::None {
            return Err(GpuError::Guest(
                "command buffer ends inside an encoder".into(),
            ));
        }
        Ok(())
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
        // Memoryless textures live in tile memory on Apple GPUs; on IMR GPUs they become
        // transient attachments that a real backend allocates per pass. No budget charge here.
        let bytes = if desc.storage_mode == StorageMode::Memoryless {
            0
        } else {
            desc.size_in_bytes()?
        };
        self.charge(bytes)?;
        match self.textures.insert(NullTexture { desc: *desc, bytes }) {
            Ok(h) => Ok(TextureId(h.raw())),
            Err(e) => {
                self.allocated -= bytes;
                Err(e.into())
            }
        }
    }

    fn destroy_texture(&mut self, id: TextureId) -> Result<(), GpuError> {
        let t = self.textures.remove(Handle::from_raw(id.0))?;
        self.allocated = self.allocated.saturating_sub(t.bytes);
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
    use crate::model::{CpuCacheMode, HazardTrackingMode, PixelFormat};

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

    fn color(texture: TextureId) -> ColorAttachment {
        ColorAttachment {
            texture,
            load: LoadAction::Clear,
            store: StoreAction::Store,
            clear_color: [0.0, 0.0, 0.0, 1.0],
        }
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
            commands: vec![
                GpuCommand::BeginRenderPass {
                    width: 0,
                    height: 0,
                    color_attachments: vec![color(t)],
                },
                GpuCommand::EndRenderPass,
            ],
        };
        assert!(matches!(b.submit(&s), Err(GpuError::Handle(_))));
        assert_eq!(b.completed(), SubmissionToken(0));
    }

    #[test]
    fn valid_triangle_submission() {
        let mut b = backend();
        let t = b.create_texture(&rt(640, 480)).unwrap();
        let s = Submission {
            commands: vec![
                GpuCommand::BeginRenderPass {
                    width: 640,
                    height: 480,
                    color_attachments: vec![color(t)],
                },
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
        assert_eq!(b.commands_executed(), 6);
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
        let pass = |atts: Vec<ColorAttachment>, w, h| Submission {
            commands: vec![
                GpuCommand::BeginRenderPass {
                    width: w,
                    height: h,
                    color_attachments: atts,
                },
                GpuCommand::EndRenderPass,
            ],
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
        m.store = StoreAction::DontCare;
        assert!(b.submit(&pass(vec![m], 0, 0)).is_ok());
        assert!(b.submit(&pass(vec![color(memoryless)], 0, 0)).is_err());
        assert_eq!(
            b.allocated_bytes(),
            2 * 64 * 64 * 4,
            "memoryless not charged"
        );
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
