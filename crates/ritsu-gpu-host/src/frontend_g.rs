//! Frontend G: executes `ritsu-gpu-proto` command streams on a [`GpuBackend`].
//!
//! Skeleton for variant G of ADR-0005 (still `Proposed`): it shows that the wire format maps
//! onto the protocol-neutral model and is the place where guest object ids are translated
//! into backend handles. Frontend P will be a sibling module with the same shape.
//!
//! Semantics:
//! * **Validate, then execute.** A stream is decoded completely before any command runs; a
//!   malformed stream has no side effects.
//! * Guest object ids are per-session names. Destroying an object removes the name at once,
//!   but the backend object is released only after the pending command buffer that references
//!   it was committed (Metal command buffers retain their resources).
//! * Commands not yet supported by the model (events) fail loudly with `Unimplemented`.

use std::collections::HashMap;

use ritsu_gpu_proto::{self as proto, Command, ObjectId, ProtoError};

use crate::backend::{
    BufferId, ColorAttachment, GpuBackend, GpuCommand, GpuError, Submission, SubmissionToken,
    TextureId,
};
use crate::model::{
    BufferDesc, LoadAction, ModelError, PixelFormat, PrimitiveType, StorageMode, StoreAction,
    TextureDesc, TextureType, TextureUsage,
};

/// Upper bound of recorded-but-uncommitted commands per session (guards host memory).
pub const MAX_PENDING_COMMANDS: usize = 1 << 20;

/// Upper bound of live guest object names per kind and session.
pub const MAX_OBJECTS_PER_KIND: usize = 1 << 20;

/// Errors of the G frontend.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum FrontendError {
    /// The stream is malformed.
    #[error(transparent)]
    Proto(#[from] ProtoError),
    /// A descriptor is invalid.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// The backend rejected an operation.
    #[error(transparent)]
    Gpu(#[from] GpuError),
    /// A guest id does not name a live object of that kind.
    #[error("unknown {kind} id {id}")]
    UnknownObject {
        /// Object kind.
        kind: &'static str,
        /// Guest id.
        id: u32,
    },
    /// A guest id is already in use.
    #[error("{kind} id {id} already in use")]
    DuplicateObject {
        /// Object kind.
        kind: &'static str,
        /// Guest id.
        id: u32,
    },
    /// A per-session limit was hit.
    #[error("session limit exceeded: {0}")]
    Limit(&'static str),
    /// Valid command that Ritsu does not implement yet.
    #[error("not implemented: {0}")]
    Unimplemented(&'static str),
}

impl From<FrontendError> for ritsu_core::Error {
    fn from(e: FrontendError) -> Self {
        match e {
            FrontendError::Unimplemented(w) => ritsu_core::Error::Unimplemented(w),
            FrontendError::Gpu(g) => g.into(),
            other => ritsu_core::Error::Guest(format!("gpu frontend G: {other}")),
        }
    }
}

/// Result of executing one stream.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamOutcome {
    /// Number of records executed.
    pub records: usize,
    /// `(guest submission id, backend token)` for every `Commit` in the stream.
    pub submissions: Vec<(u64, SubmissionToken)>,
}

#[derive(Debug, Clone, Copy)]
enum Deferred {
    Buffer(BufferId),
    Texture(TextureId),
}

/// One guest connection (one guest device context) on top of a backend.
#[derive(Debug)]
pub struct GuestSession<B: GpuBackend> {
    backend: B,
    buffers: HashMap<u32, BufferId>,
    textures: HashMap<u32, TextureId>,
    pending: Vec<GpuCommand>,
    deferred_destroy: Vec<Deferred>,
}

impl<B: GpuBackend> GuestSession<B> {
    /// New session on `backend`.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            buffers: HashMap::new(),
            textures: HashMap::new(),
            pending: Vec::new(),
            deferred_destroy: Vec::new(),
        }
    }

    /// The backend (for inspection in tests).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Number of recorded, uncommitted commands.
    pub fn pending_commands(&self) -> usize {
        self.pending.len()
    }

    /// Decodes and executes one stream.
    ///
    /// # Errors
    ///
    /// Malformed streams fail before any effect. Semantic errors stop execution at the
    /// offending record; earlier records of the stream have taken effect (like a Metal
    /// command that raises a validation error mid-sequence).
    pub fn execute_stream(&mut self, bytes: &[u8]) -> Result<StreamOutcome, FrontendError> {
        let commands = proto::decode_stream(bytes)?;
        let mut outcome = StreamOutcome::default();
        for cmd in &commands {
            if let Some(sub) = self.execute(cmd)? {
                outcome.submissions.push(sub);
            }
            outcome.records += 1;
        }
        Ok(outcome)
    }

    fn buffer(&self, id: ObjectId) -> Result<BufferId, FrontendError> {
        self.buffers
            .get(&id.get())
            .copied()
            .ok_or(FrontendError::UnknownObject {
                kind: "buffer",
                id: id.get(),
            })
    }

    fn texture(&self, id: ObjectId) -> Result<TextureId, FrontendError> {
        self.textures
            .get(&id.get())
            .copied()
            .ok_or(FrontendError::UnknownObject {
                kind: "texture",
                id: id.get(),
            })
    }

    fn record(&mut self, cmd: GpuCommand) -> Result<(), FrontendError> {
        if self.pending.len() >= MAX_PENDING_COMMANDS {
            return Err(FrontendError::Limit("pending commands"));
        }
        self.pending.push(cmd);
        Ok(())
    }

    fn destroy_or_defer(&mut self, obj: Deferred) -> Result<(), FrontendError> {
        if self.pending.is_empty() {
            match obj {
                Deferred::Buffer(b) => self.backend.destroy_buffer(b)?,
                Deferred::Texture(t) => self.backend.destroy_texture(t)?,
            }
        } else {
            self.deferred_destroy.push(obj);
        }
        Ok(())
    }

    fn execute(
        &mut self,
        cmd: &Command<'_>,
    ) -> Result<Option<(u64, SubmissionToken)>, FrontendError> {
        match cmd {
            Command::Nop => {}
            Command::CreateBuffer(c) => {
                let key = c.buffer_id.get();
                if self.buffers.contains_key(&key) {
                    return Err(FrontendError::DuplicateObject {
                        kind: "buffer",
                        id: key,
                    });
                }
                if self.buffers.len() >= MAX_OBJECTS_PER_KIND {
                    return Err(FrontendError::Limit("buffers"));
                }
                let desc = BufferDesc::from_raw(
                    c.length,
                    u32::from(c.storage_mode),
                    u32::from(c.cpu_cache_mode),
                    u32::from(c.hazard_tracking),
                )?;
                let id = self.backend.create_buffer(&desc)?;
                self.buffers.insert(key, id);
            }
            Command::DestroyBuffer { buffer_id } => {
                let id = self.buffer(*buffer_id)?;
                self.buffers.remove(&buffer_id.get());
                self.destroy_or_defer(Deferred::Buffer(id))?;
            }
            Command::WriteBuffer(w) => {
                let id = self.buffer(w.buffer_id)?;
                self.backend.write_buffer(id, w.offset, w.data)?;
            }
            Command::CopyBuffer(c) => {
                let cmd = GpuCommand::CopyBuffer {
                    src: self.buffer(c.src)?,
                    src_offset: c.src_offset,
                    dst: self.buffer(c.dst)?,
                    dst_offset: c.dst_offset,
                    size: c.size,
                };
                self.record(cmd)?;
            }
            Command::CreateTexture(c) => {
                let key = c.texture_id.get();
                if self.textures.contains_key(&key) {
                    return Err(FrontendError::DuplicateObject {
                        kind: "texture",
                        id: key,
                    });
                }
                if self.textures.len() >= MAX_OBJECTS_PER_KIND {
                    return Err(FrontendError::Limit("textures"));
                }
                let desc = TextureDesc {
                    texture_type: TextureType::from_raw(u32::from(c.texture_type))?,
                    pixel_format: PixelFormat::from_raw(u32::from(c.pixel_format))?,
                    width: c.width,
                    height: c.height,
                    depth: c.depth,
                    array_length: c.array_length,
                    mip_level_count: c.mip_level_count,
                    sample_count: u32::from(c.sample_count),
                    storage_mode: StorageMode::from_raw(u32::from(c.storage_mode))?,
                    usage: TextureUsage::from_raw(u32::from(c.usage))?,
                };
                let id = self.backend.create_texture(&desc)?;
                self.textures.insert(key, id);
            }
            Command::DestroyTexture { texture_id } => {
                let id = self.texture(*texture_id)?;
                self.textures.remove(&texture_id.get());
                self.destroy_or_defer(Deferred::Texture(id))?;
            }
            Command::BeginRenderPass(rp) => {
                let mut color_attachments = Vec::with_capacity(rp.color_attachment_count());
                for a in rp.color_attachments() {
                    color_attachments.push(ColorAttachment {
                        texture: self.texture(a.texture_id)?,
                        load: LoadAction::from_raw(u32::from(a.load_action))?,
                        store: StoreAction::from_raw(u32::from(a.store_action))?,
                        clear_color: a.clear_color,
                    });
                }
                self.record(GpuCommand::BeginRenderPass {
                    width: rp.width,
                    height: rp.height,
                    color_attachments,
                })?;
            }
            Command::EndRenderPass => self.record(GpuCommand::EndRenderPass)?,
            Command::Draw(d) => self.record(GpuCommand::Draw {
                primitive: PrimitiveType::from_raw(u32::from(d.primitive_type))?,
                vertex_start: d.vertex_start,
                vertex_count: d.vertex_count,
                instance_count: d.instance_count,
                base_instance: d.base_instance,
            })?,
            Command::BeginComputePass => self.record(GpuCommand::BeginComputePass)?,
            Command::EndComputePass => self.record(GpuCommand::EndComputePass)?,
            Command::Dispatch(d) => self.record(GpuCommand::Dispatch {
                threadgroups: d.threadgroups,
                threads_per_threadgroup: d.threads_per_threadgroup,
            })?,
            // TODO(WP-GPU-08): shared events → timeline semaphores.
            Command::SignalEvent(_) | Command::WaitEvent(_) => {
                return Err(FrontendError::Unimplemented("shared events"));
            }
            Command::Commit { submission_id } => {
                let submission = Submission {
                    commands: core::mem::take(&mut self.pending),
                };
                // On error the command buffer is dropped (Metal: commit of an invalid command
                // buffer reports an error status); deferred destroys still run below.
                let result = self.backend.submit(&submission);
                for obj in core::mem::take(&mut self.deferred_destroy) {
                    match obj {
                        Deferred::Buffer(b) => self.backend.destroy_buffer(b)?,
                        Deferred::Texture(t) => self.backend.destroy_texture(t)?,
                    }
                }
                return Ok(Some((*submission_id, result?)));
            }
            // `Command` is non_exhaustive; new opcodes must be handled explicitly.
            _ => return Err(FrontendError::Unimplemented("unknown command")),
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::NullBackend;
    use crate::caps::Capabilities;
    use proto::{
        BeginRenderPass, ColorAttachmentRecord, CreateBuffer, CreateTexture, Draw, EventValue,
        StreamEncoder, WriteBuffer,
    };

    fn oid(v: u32) -> ObjectId {
        ObjectId::new(v).unwrap()
    }

    fn session() -> GuestSession<NullBackend> {
        GuestSession::new(NullBackend::new(Capabilities::rtx4050_assumed()))
    }

    fn stream(cmds: &[Command<'_>]) -> Vec<u8> {
        let mut e = StreamEncoder::new();
        for c in cmds {
            e.push(c).unwrap();
        }
        e.finish()
    }

    fn create_rt(id: u32) -> Command<'static> {
        Command::CreateTexture(CreateTexture {
            texture_id: oid(id),
            pixel_format: PixelFormat::BGRA8Unorm.raw() as u16,
            usage: (TextureUsage::RENDER_TARGET.bits() | TextureUsage::SHADER_READ.bits()) as u16,
            texture_type: TextureType::Type2D.raw() as u8,
            storage_mode: StorageMode::Private.raw() as u8,
            sample_count: 1,
            width: 640,
            height: 480,
            depth: 1,
            array_length: 1,
            mip_level_count: 1,
        })
    }

    fn create_buf(id: u32, len: u64) -> Command<'static> {
        Command::CreateBuffer(CreateBuffer {
            buffer_id: oid(id),
            storage_mode: 0,
            cpu_cache_mode: 0,
            hazard_tracking: 0,
            length: len,
        })
    }

    fn triangle_pass(tex: u32) -> [Command<'static>; 3] {
        [
            Command::BeginRenderPass(
                BeginRenderPass::new(
                    640,
                    480,
                    &[ColorAttachmentRecord {
                        texture_id: oid(tex),
                        load_action: 2,
                        store_action: 1,
                        clear_color: [0.1, 0.2, 0.3, 1.0],
                    }],
                )
                .unwrap(),
            ),
            Command::Draw(Draw {
                primitive_type: 3,
                vertex_start: 0,
                vertex_count: 3,
                instance_count: 1,
                base_instance: 0,
            }),
            Command::EndRenderPass,
        ]
    }

    #[test]
    fn first_triangle_end_to_end() {
        let mut s = session();
        let mut cmds = vec![
            create_buf(1, 3 * 16),
            Command::WriteBuffer(WriteBuffer {
                buffer_id: oid(1),
                offset: 0,
                data: &[0u8; 48],
            }),
            create_rt(2),
        ];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::Commit { submission_id: 77 });
        let out = s.execute_stream(&stream(&cmds)).unwrap();
        assert_eq!(out.records, cmds.len());
        assert_eq!(out.submissions, vec![(77, SubmissionToken(1))]);
        assert_eq!(s.backend().live_buffers(), 1);
        assert_eq!(s.backend().live_textures(), 1);
        assert_eq!(s.backend().commands_executed(), 3);
        assert_eq!(s.pending_commands(), 0);
    }

    #[test]
    fn malformed_stream_has_no_side_effects() {
        let mut s = session();
        let mut bytes = stream(&[create_buf(1, 64), create_buf(2, 64)]);
        bytes.truncate(bytes.len() - 1);
        assert!(matches!(
            s.execute_stream(&bytes),
            Err(FrontendError::Proto(_))
        ));
        assert_eq!(s.backend().live_buffers(), 0);
    }

    #[test]
    fn unknown_and_duplicate_ids() {
        let mut s = session();
        assert_eq!(
            s.execute_stream(&stream(&[Command::DestroyBuffer { buffer_id: oid(5) }])),
            Err(FrontendError::UnknownObject {
                kind: "buffer",
                id: 5
            })
        );
        assert_eq!(
            s.execute_stream(&stream(&[create_buf(1, 64), create_buf(1, 64)])),
            Err(FrontendError::DuplicateObject {
                kind: "buffer",
                id: 1
            })
        );
        assert_eq!(s.backend().live_buffers(), 1, "first create took effect");
    }

    #[test]
    fn invalid_descriptor_from_guest_is_rejected() {
        let mut s = session();
        let bad_mode = Command::CreateBuffer(CreateBuffer {
            buffer_id: oid(1),
            storage_mode: 9,
            cpu_cache_mode: 0,
            hazard_tracking: 0,
            length: 64,
        });
        assert!(matches!(
            s.execute_stream(&stream(&[bad_mode])),
            Err(FrontendError::Model(ModelError::UnknownStorageMode(9)))
        ));
        assert!(matches!(
            s.execute_stream(&stream(&[create_buf(1, 0)])),
            Err(FrontendError::Gpu(GpuError::Model(ModelError::ZeroLength)))
        ));
    }

    #[test]
    fn destroy_is_deferred_until_commit() {
        let mut s = session();
        let mut cmds = vec![create_rt(2)];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::DestroyTexture { texture_id: oid(2) });
        s.execute_stream(&stream(&cmds)).unwrap();
        assert_eq!(
            s.backend().live_textures(),
            1,
            "retained by pending commands"
        );
        // The guest name is free again immediately.
        s.execute_stream(&stream(&[create_rt(2)])).unwrap();
        assert_eq!(s.backend().live_textures(), 2);
        let out = s
            .execute_stream(&stream(&[Command::Commit { submission_id: 1 }]))
            .unwrap();
        assert_eq!(out.submissions.len(), 1);
        assert_eq!(s.backend().live_textures(), 1, "released after commit");
    }

    #[test]
    fn invalid_command_buffer_fails_at_commit() {
        let mut s = session();
        let cmds = [
            Command::Draw(Draw {
                primitive_type: 3,
                vertex_start: 0,
                vertex_count: 3,
                instance_count: 1,
                base_instance: 0,
            }),
            Command::Commit { submission_id: 1 },
        ];
        assert!(matches!(
            s.execute_stream(&stream(&cmds)),
            Err(FrontendError::Gpu(GpuError::Guest(_)))
        ));
        assert_eq!(s.pending_commands(), 0, "invalid command buffer is dropped");
    }

    #[test]
    fn events_are_loudly_unimplemented() {
        let mut s = session();
        let ev = Command::SignalEvent(EventValue {
            event_id: oid(1),
            value: 1,
        });
        assert_eq!(
            s.execute_stream(&stream(&[ev])),
            Err(FrontendError::Unimplemented("shared events"))
        );
    }

    #[test]
    fn random_streams_never_panic() {
        // Mutated valid streams through the full frontend + backend path.
        let mut cmds = vec![create_buf(1, 64), create_rt(2)];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::Commit { submission_id: 1 });
        let base = stream(&cmds);
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        for _ in 0..5_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let mut b = base.clone();
            let pos = (state as usize) % b.len();
            b[pos] = (state >> 32) as u8;
            let _ = session().execute_stream(&b);
        }
    }
}
