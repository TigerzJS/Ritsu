//! Frontend G: executes `ritsu-gpu-proto` command streams on a [`GpuBackend`].
//!
//! Variant G of ADR-0005 (still `Proposed`). This is the only module of the crate that knows
//! the wire format; it translates guest object ids into backend handles and protocol records
//! into the protocol-neutral model. Frontend P will be a sibling module with the same shape.
//!
//! Session semantics (gpu.md §9.3–§9.4):
//! * **Handshake.** The first stream must hold exactly one HELLO (plus optional `Nop`
//!   padding). The host answers `HelloAck`
//!   (version, enabled features, capability block derived from [`ReportedFeatures`]) or
//!   `HelloReject`. Only features the host implements ([`HOST_FEATURES`]) are enabled.
//! * **Validate, then execute.** A stream is decoded completely before any record runs.
//!   Protocol violations (framing, wrong version, non-negotiated opcode, duplicate HELLO,
//!   commands before HELLO) reject the whole stream and reset the session: every object is
//!   destroyed, pending work is dropped and the guest must send HELLO again (`SessionReset`
//!   reply; the guest sees a GPU reset).
//! * **Errors per record.** A semantically invalid record is not executed, answered with a
//!   `RecordError` and execution continues. If the record belongs to the command buffer being
//!   recorded (decided by [`proto::OpcodeClass`]: `Encoder`, `Sync` and any class this
//!   frontend does not know), that command buffer is *poisoned*: later encoder commands are
//!   skipped until `Commit`, and the commit reports
//!   `CommandBufferStatus::Error` instead of submitting a partial command buffer. Poisoning
//!   survives stream boundaries (a command buffer may span several streams). A command this
//!   frontend cannot handle at all always poisons (in doubt, never a partial commit).
//! * **Completion.** Every `Commit` yields exactly one `CommandBufferStatus`, strictly in
//!   commit order (errors included), once the backend timeline reached it
//!   ([`GuestSession::poll_completions`]). A stream whose commits would exceed
//!   [`proto::MAX_OUTSTANDING_SUBMISSIONS`] unreported command buffers is a protocol
//!   violation (session reset), so the status queue never overflows.
//! * **Lifetime.** Destroying an object frees the guest name at once; the backend object is
//!   released after the pending command buffer that may reference it was committed (or
//!   dropped).
//! * **Bounded memory.** Objects (live plus awaiting deferred destruction), pending commands,
//!   unreported submissions and replies per stream all have fixed upper bounds; nothing
//!   grows with a guest-declared count.

use std::collections::{HashMap, VecDeque};

use ritsu_gpu_proto::Opcode;
use ritsu_gpu_proto::negotiate::negotiate;
use ritsu_gpu_proto::{
    self as proto, BOOTSTRAP_VERSION, CapabilityBlock, Command, CommandBufferStatus, ErrorCode,
    Features, HelloAck, NO_RECORD, Negotiated, ObjectId, OpcodeClass, ProtoError, RecordError,
    Reply, ReplyEncoder, StreamFault, SubmissionStatus, SupportLevel,
};

use crate::backend::{
    BufferId, ColorAttachment, GpuBackend, GpuCommand, GpuError, Submission, SubmissionToken,
    TextureId,
};
use crate::caps::{
    ArgumentBuffersTier, EmulationPolicy, ReportedFeatures, Support, reported_feature_set_with,
};
use crate::model::{
    BufferDesc, LoadAction, ModelError, PixelFormat, PrimitiveType, StorageMode, StoreAction,
    TextureDesc, TextureType, TextureUsage,
};

/// Upper bound of recorded-but-uncommitted commands per session (guards host memory).
pub const MAX_PENDING_COMMANDS: usize = 1 << 20;

/// Upper bound of objects per kind and session: live guest names plus objects whose
/// destruction is deferred until the next commit. A create beyond it fails with
/// [`ErrorCode::LimitExceeded`].
pub const MAX_OBJECTS_PER_KIND: usize = 1 << 20;

/// Upper bound of committed command buffers whose status has not been reported yet (the
/// protocol rule [`proto::MAX_OUTSTANDING_SUBMISSIONS`]).
pub const MAX_IN_FLIGHT_SUBMISSIONS: usize = proto::MAX_OUTSTANDING_SUBMISSIONS as usize;

/// Session limits (constants in production, smaller in tests).
#[derive(Debug, Clone, Copy)]
struct Limits {
    objects_per_kind: usize,
    pending_commands: usize,
    in_flight: usize,
}

impl Limits {
    const DEFAULT: Self = Self {
        objects_per_kind: MAX_OBJECTS_PER_KIND,
        pending_commands: MAX_PENDING_COMMANDS,
        in_flight: MAX_IN_FLIGHT_SUBMISSIONS,
    };
}

/// Upper bound of `RecordError` replies (and host-side error entries) per stream. Further
/// errors are counted and reported once as [`ErrorCode::TooManyErrors`].
pub const MAX_ERRORS_PER_STREAM: usize = 64;

/// Optional protocol features this host implements and offers in HELLO_ACK.
///
/// Empty for now: shared events arrive with WP-GPU-08. A guest that asks for
/// [`Features::SHARED_EVENTS`] is told honestly that they are unavailable.
pub const HOST_FEATURES: Features = Features::empty();

/// Errors of the G frontend.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum FrontendError {
    /// The stream is malformed or violates the session protocol.
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
    /// The command buffer was already poisoned by an earlier record (commit status only).
    #[error("command buffer invalidated by an earlier error ({0:?})")]
    PoisonedCommandBuffer(ErrorCode),
}

impl FrontendError {
    /// Reply-channel error code.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Proto(p) => p.reply_code(),
            Self::Model(_) | Self::Gpu(GpuError::Model(_)) => ErrorCode::InvalidArgument,
            Self::Gpu(GpuError::OutOfDeviceMemory { .. }) => ErrorCode::OutOfMemory,
            Self::Gpu(GpuError::Unimplemented(_)) | Self::Unimplemented(_) => {
                ErrorCode::Unimplemented
            }
            Self::Gpu(GpuError::Host(_)) => ErrorCode::HostError,
            Self::Gpu(_) => ErrorCode::InvalidCommand,
            Self::UnknownObject { .. } => ErrorCode::UnknownObject,
            Self::DuplicateObject { .. } => ErrorCode::DuplicateObject,
            Self::Limit(_) => ErrorCode::LimitExceeded,
            Self::PoisonedCommandBuffer(c) => *c,
        }
    }

    fn object_id(&self) -> Option<ObjectId> {
        match self {
            Self::UnknownObject { id, .. } | Self::DuplicateObject { id, .. } => ObjectId::new(*id),
            _ => None,
        }
    }

    fn detail(&self) -> u64 {
        match self {
            Self::Gpu(GpuError::OutOfDeviceMemory { requested, .. }) => *requested,
            _ => 0,
        }
    }
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

/// Host-side diagnostic for one failure (for logs; the guest sees [`Reply`]s).
#[derive(Debug, Clone, PartialEq)]
pub struct StreamError {
    /// Record index within the stream, `None` for stream-level or housekeeping failures.
    pub record_index: Option<u32>,
    /// What went wrong.
    pub error: FrontendError,
}

/// Result of executing one stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StreamOutcome {
    /// Per-session sequence number of this stream (0 = first stream of the session).
    pub stream_seq: u64,
    /// Header version for the reply stream ([`BOOTSTRAP_VERSION`] before negotiation).
    pub reply_version: u16,
    /// Records that ran. A `Commit` counts even if its command buffer failed (the failure is
    /// in its `CommandBufferStatus`); records answered with `RecordError` and encoder records
    /// skipped in a poisoned command buffer do not count.
    pub records_executed: usize,
    /// Replies for the guest, in order.
    pub replies: Vec<Reply>,
    /// `(guest submission id, backend token)` for every command buffer submitted.
    pub submissions: Vec<(u64, SubmissionToken)>,
    /// Host-side diagnostics (at most [`MAX_ERRORS_PER_STREAM`]).
    pub errors: Vec<StreamError>,
    reported_errors: usize,
    suppressed: u64,
}

impl StreamOutcome {
    /// Encodes [`StreamOutcome::replies`] as a reply stream for the guest.
    ///
    /// # Errors
    ///
    /// Only encoder limits, which bounded reply counts cannot reach.
    pub fn reply_stream(&self) -> Result<Vec<u8>, ProtoError> {
        let mut e = ReplyEncoder::with_version(self.reply_version);
        for r in &self.replies {
            e.push(r)?;
        }
        Ok(e.finish())
    }

    fn diagnostic(&mut self, record_index: Option<u32>, error: FrontendError) {
        if self.errors.len() < MAX_ERRORS_PER_STREAM {
            self.errors.push(StreamError {
                record_index,
                error,
            });
        }
    }

    fn record_error(&mut self, index: u32, opcode: u16, error: FrontendError) {
        if self.reported_errors >= MAX_ERRORS_PER_STREAM {
            self.suppressed = self.suppressed.saturating_add(1);
            return;
        }
        self.reported_errors += 1;
        self.replies.push(Reply::RecordError(RecordError {
            stream_seq: self.stream_seq,
            record_index: index,
            opcode,
            code: error.code(),
            object_id: error.object_id(),
            detail: error.detail(),
        }));
        self.diagnostic(Some(index), error);
    }
}

#[derive(Debug, Clone, Copy)]
enum Deferred {
    Buffer(BufferId),
    Texture(TextureId),
}

/// A committed command buffer whose status has not been reported yet.
#[derive(Debug, Clone, Copy)]
struct InFlight {
    submission_id: u64,
    state: Result<SubmissionToken, ErrorCode>,
}

/// Whether a command belongs to the command buffer being recorded (and is therefore skipped
/// in, and poisons, a failed command buffer).
///
/// Decided by the opcode class, a mandatory column of the opcode table. Unknown future
/// classes count as part of the command buffer: in doubt, poison rather than commit a partial
/// command buffer (CLAUDE.md §3.5).
fn belongs_to_command_buffer(cmd: &Command<'_>) -> bool {
    opcode_belongs_to_command_buffer(cmd.opcode())
}

fn opcode_belongs_to_command_buffer(op: Opcode) -> bool {
    // Negative list on purpose: any class added later counts as part of the command buffer.
    !matches!(
        op.class(),
        OpcodeClass::Control | OpcodeClass::Resource | OpcodeClass::Submission
    )
}

fn support_level(s: Support) -> SupportLevel {
    match s {
        Support::Native => SupportLevel::Native,
        Support::Emulated => SupportLevel::Emulated,
        Support::Unsupported => SupportLevel::Unsupported,
    }
}

/// Wire form of the reported feature set (HELLO_ACK capability block).
#[must_use]
pub fn capability_block(r: &ReportedFeatures) -> CapabilityBlock {
    let f = &r.families;
    let families = [
        (f.apple7, CapabilityBlock::FAMILY_APPLE7),
        (f.mac2, CapabilityBlock::FAMILY_MAC2),
        (f.common3, CapabilityBlock::FAMILY_COMMON3),
        (f.metal3, CapabilityBlock::FAMILY_METAL3),
        (f.metal4, CapabilityBlock::FAMILY_METAL4),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(0, |acc, (_, bit)| acc | bit);
    CapabilityBlock {
        families,
        argument_buffers_tier: match r.argument_buffers_tier {
            ArgumentBuffersTier::Tier1 => 1,
            ArgumentBuffersTier::Tier2 => 2,
        },
        ray_tracing: support_level(r.ray_tracing),
        mesh_shaders: support_level(r.mesh_shaders),
        astc: support_level(r.astc),
        bc: support_level(r.bc),
        float32_atomics: support_level(r.float32_atomics),
        framebuffer_fetch: support_level(r.framebuffer_fetch),
        raster_order_groups: support_level(r.raster_order_groups),
        tile_shaders: support_level(r.tile_shaders),
        memoryless: support_level(r.memoryless),
        unified_memory: support_level(r.unified_memory),
        max_buffer_length: r.limits.max_buffer_length,
        max_texture_dimension_1d: r.limits.max_texture_dimension_1d,
        max_texture_dimension_2d: r.limits.max_texture_dimension_2d,
        max_texture_dimension_3d: r.limits.max_texture_dimension_3d,
        max_texture_array_layers: r.limits.max_texture_array_layers,
        max_threads_per_threadgroup: r.limits.max_threads_per_threadgroup,
        max_color_attachments: r.limits.max_color_attachments,
    }
}

/// One guest connection (one guest device context) on top of a backend.
#[derive(Debug)]
pub struct GuestSession<B: GpuBackend> {
    backend: B,
    reported: ReportedFeatures,
    host_features: Features,
    negotiated: Option<Negotiated>,
    next_stream_seq: u64,
    buffers: HashMap<u32, BufferId>,
    textures: HashMap<u32, TextureId>,
    pending: Vec<GpuCommand>,
    deferred_destroy: Vec<Deferred>,
    deferred_buffers: usize,
    deferred_textures: usize,
    limits: Limits,
    /// First error of the command buffer being recorded, if any.
    poisoned: Option<ErrorCode>,
    in_flight: VecDeque<InFlight>,
}

impl<B: GpuBackend> GuestSession<B> {
    /// New session on `backend`, reporting only native features
    /// ([`EmulationPolicy::NATIVE_ONLY`]).
    pub fn new(backend: B) -> Self {
        Self::with_policy(backend, &EmulationPolicy::NATIVE_ONLY)
    }

    /// New session whose capability block follows `policy`.
    pub fn with_policy(backend: B, policy: &EmulationPolicy) -> Self {
        let reported = reported_feature_set_with(backend.capabilities(), policy);
        Self {
            backend,
            reported,
            host_features: HOST_FEATURES,
            negotiated: None,
            next_stream_seq: 0,
            buffers: HashMap::new(),
            textures: HashMap::new(),
            pending: Vec::new(),
            deferred_destroy: Vec::new(),
            deferred_buffers: 0,
            deferred_textures: 0,
            limits: Limits::DEFAULT,
            poisoned: None,
            in_flight: VecDeque::new(),
        }
    }

    #[cfg(test)]
    fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The backend (for inspection in tests).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Mutable backend access (tests driving completion).
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Feature set advertised in HELLO_ACK.
    pub fn reported_features(&self) -> &ReportedFeatures {
        &self.reported
    }

    /// Negotiated version and features, `None` before HELLO or after a reset.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.negotiated
    }

    /// Number of recorded, uncommitted commands.
    pub fn pending_commands(&self) -> usize {
        self.pending.len()
    }

    /// Whether the command buffer being recorded has failed (its commit will report an error).
    pub fn command_buffer_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }

    /// Committed command buffers whose status has not been reported yet.
    pub fn in_flight_submissions(&self) -> usize {
        self.in_flight.len()
    }

    /// Decodes and executes one stream; never fails as a whole; everything the guest needs to
    /// know is in [`StreamOutcome::replies`].
    pub fn execute_stream(&mut self, bytes: &[u8]) -> StreamOutcome {
        let stream_seq = self.next_stream_seq;
        self.next_stream_seq = self.next_stream_seq.wrapping_add(1);
        let mut out = StreamOutcome {
            stream_seq,
            reply_version: self.negotiated.map_or(BOOTSTRAP_VERSION, |n| n.version),
            ..StreamOutcome::default()
        };
        match self.negotiated {
            None => self.handshake(bytes, &mut out),
            Some(n) => match proto::decode_session_stream(bytes, &n) {
                Ok(commands) => {
                    if self.would_exceed_outstanding(&commands, &mut out.replies) {
                        self.fault_with(
                            ErrorCode::LimitExceeded,
                            0,
                            Opcode::Commit.raw(),
                            FrontendError::Limit("outstanding submissions"),
                            &mut out,
                        );
                    } else {
                        self.run(&commands, &mut out);
                    }
                }
                Err(e) => self.fault(e, &mut out),
            },
        }
        if out.suppressed > 0 {
            out.replies.push(Reply::RecordError(RecordError {
                stream_seq,
                record_index: NO_RECORD,
                opcode: 0,
                code: ErrorCode::TooManyErrors,
                object_id: None,
                detail: out.suppressed,
            }));
        }
        self.drain_completions(&mut out.replies);
        out
    }

    /// Status replies for command buffers that completed since the last call, in commit
    /// order. Real backends complete asynchronously; the transport calls this when the
    /// completion thread signals progress.
    pub fn poll_completions(&mut self) -> Vec<Reply> {
        let mut replies = Vec::new();
        self.drain_completions(&mut replies);
        replies
    }

    fn drain_completions(&mut self, replies: &mut Vec<Reply>) {
        let done = self.backend.completed();
        while let Some(front) = self.in_flight.front() {
            let status = match front.state {
                Err(code) => CommandBufferStatus::Error(code),
                Ok(token) if token <= done => CommandBufferStatus::Completed,
                Ok(_) => break,
            };
            replies.push(Reply::CommandBufferStatus(SubmissionStatus {
                submission_id: front.submission_id,
                status,
            }));
            self.in_flight.pop_front();
        }
    }

    fn handshake(&mut self, bytes: &[u8], out: &mut StreamOutcome) {
        let hello = match proto::decode_hello_stream(bytes) {
            Ok(h) => h,
            Err(e) => return self.fault(e, out),
        };
        match negotiate(&hello, self.host_features) {
            Ok(n) => {
                self.negotiated = Some(n);
                // HELLO plus any `Nop` padding (the header was validated by the decoder).
                out.records_executed = proto::Header::parse(bytes)
                    .map_or(1, |h| usize::try_from(h.record_count).unwrap_or(usize::MAX));
                out.replies.push(Reply::HelloAck(HelloAck {
                    version: n.version,
                    features: n.features,
                    caps: capability_block(&self.reported),
                }));
            }
            Err(reject) => out.replies.push(Reply::HelloReject(reject)),
        }
    }

    /// Whether executing `commands` could leave more unreported command buffers than the
    /// protocol allows. Checked before execution, so the status queue keeps commit order and
    /// never overflows. Completed statuses are drained into `replies` first.
    fn would_exceed_outstanding(
        &mut self,
        commands: &[Command<'_>],
        replies: &mut Vec<Reply>,
    ) -> bool {
        let commits = commands
            .iter()
            .filter(|c| matches!(c, Command::Commit { .. }))
            .count();
        if commits == 0 {
            return false;
        }
        self.drain_completions(replies);
        self.in_flight
            .len()
            .checked_add(commits)
            .is_none_or(|n| n > self.limits.in_flight)
    }

    /// Protocol violation: reject the stream, reset the session, tell the guest.
    fn fault(&mut self, e: ProtoError, out: &mut StreamOutcome) {
        self.fault_with(
            e.reply_code(),
            e.offset().map_or(0, |o| o as u64),
            e.raw_opcode().unwrap_or(0),
            e.into(),
            out,
        );
    }

    fn fault_with(
        &mut self,
        code: ErrorCode,
        offset: u64,
        opcode: u16,
        diagnostic: FrontendError,
        out: &mut StreamOutcome,
    ) {
        out.replies.push(Reply::SessionReset(StreamFault {
            stream_seq: out.stream_seq,
            offset,
            opcode,
            code,
        }));
        out.diagnostic(None, diagnostic);
        self.reset(out);
    }

    /// Drops all session state; the guest must negotiate again.
    fn reset(&mut self, out: &mut StreamOutcome) {
        self.negotiated = None;
        self.pending.clear();
        self.deferred_buffers = 0;
        self.deferred_textures = 0;
        self.poisoned = None;
        // No statuses for earlier submissions after a reset (documented in the reply).
        self.in_flight.clear();
        let mut doomed: Vec<Deferred> = core::mem::take(&mut self.deferred_destroy);
        doomed.extend(self.buffers.drain().map(|(_, b)| Deferred::Buffer(b)));
        doomed.extend(self.textures.drain().map(|(_, t)| Deferred::Texture(t)));
        for obj in doomed {
            if let Err(e) = self.destroy_now(obj) {
                out.diagnostic(None, e.into());
            }
        }
    }

    fn run(&mut self, commands: &[Command<'_>], out: &mut StreamOutcome) {
        for (i, cmd) in commands.iter().enumerate() {
            // Streams hold at most u32::MAX records (header field), so this never saturates.
            let index = u32::try_from(i).unwrap_or(NO_RECORD);
            let in_cb = belongs_to_command_buffer(cmd);
            if in_cb && self.poisoned.is_some() {
                // Part of a command buffer that already failed: skipped, reported at commit.
                continue;
            }
            match self.execute(cmd, index, out) {
                Ok(()) => out.records_executed += 1,
                Err(e) => {
                    if in_cb {
                        self.poisoned.get_or_insert(e.code());
                    }
                    out.record_error(index, cmd.opcode().raw(), e);
                }
            }
        }
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
        if self.pending.len() >= self.limits.pending_commands {
            return Err(FrontendError::Limit("pending commands"));
        }
        self.pending.push(cmd);
        Ok(())
    }

    fn destroy_now(&mut self, obj: Deferred) -> Result<(), GpuError> {
        match obj {
            Deferred::Buffer(b) => self.backend.destroy_buffer(b),
            Deferred::Texture(t) => self.backend.destroy_texture(t),
        }
    }

    fn destroy_or_defer(&mut self, obj: Deferred) -> Result<(), FrontendError> {
        if self.pending.is_empty() {
            self.destroy_now(obj)?;
        } else {
            // Counted against the per-kind object limit until the commit releases it.
            match obj {
                Deferred::Buffer(_) => self.deferred_buffers += 1,
                Deferred::Texture(_) => self.deferred_textures += 1,
            }
            self.deferred_destroy.push(obj);
        }
        Ok(())
    }

    fn commit(&mut self, submission_id: u64, index: u32, out: &mut StreamOutcome) {
        let submission = Submission {
            commands: core::mem::take(&mut self.pending),
        };
        let state = if let Some(code) = self.poisoned.take() {
            // Never submit a partial command buffer (CLAUDE.md §3.5).
            out.diagnostic(Some(index), FrontendError::PoisonedCommandBuffer(code));
            Err(code)
        } else {
            match self.backend.submit(&submission) {
                Ok(token) => {
                    out.submissions.push((submission_id, token));
                    Ok(token)
                }
                Err(e) => {
                    let e = FrontendError::from(e);
                    let code = e.code();
                    out.diagnostic(Some(index), e);
                    Err(code)
                }
            }
        };
        // Deferred destroys run whatever happened to the command buffer.
        self.deferred_buffers = 0;
        self.deferred_textures = 0;
        for obj in core::mem::take(&mut self.deferred_destroy) {
            if let Err(e) = self.destroy_now(obj) {
                out.diagnostic(Some(index), e.into());
            }
        }
        // Bounded: `would_exceed_outstanding` admitted this stream's commits beforehand.
        // Errors wait in the same queue so statuses keep commit order.
        self.in_flight.push_back(InFlight {
            submission_id,
            state,
        });
    }

    fn execute(
        &mut self,
        cmd: &Command<'_>,
        index: u32,
        out: &mut StreamOutcome,
    ) -> Result<(), FrontendError> {
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
                if self.buffers.len() + self.deferred_buffers >= self.limits.objects_per_kind {
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
                if self.textures.len() + self.deferred_textures >= self.limits.objects_per_kind {
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
            // Only reachable once SHARED_EVENTS is in HOST_FEATURES.
            // TODO(WP-GPU-08): shared events → timeline semaphores.
            Command::SignalEvent(_) | Command::WaitEvent(_) => {
                return Err(FrontendError::Unimplemented("shared events"));
            }
            Command::Commit { submission_id } => self.commit(*submission_id, index, out),
            // Rejected by `decode_session_stream`; kept total for defence in depth.
            Command::Hello(_) => {
                return Err(ProtoError::UnexpectedHello { offset: 0 }.into());
            }
            // `Command` is non_exhaustive; new opcodes must be handled explicitly. Until then a
            // command this frontend cannot execute poisons the command buffer whatever its
            // class: in doubt, never commit a partial command buffer.
            _ => {
                self.poisoned.get_or_insert(ErrorCode::Unimplemented);
                return Err(FrontendError::Unimplemented("unknown command"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::NullBackend;
    use crate::caps::{Capabilities, reported_feature_set};
    use crate::model::{BufferDesc, TextureDesc};
    use proto::{
        BeginRenderPass, ColorAttachmentRecord, CreateBuffer, CreateTexture, Draw, EventValue,
        Hello, HelloReject, Opcode, StreamEncoder, WriteBuffer, decode_reply_stream,
    };

    fn oid(v: u32) -> ObjectId {
        ObjectId::new(v).unwrap()
    }

    fn stream(cmds: &[Command<'_>]) -> Vec<u8> {
        let mut e = StreamEncoder::new();
        for c in cmds {
            e.push(c).unwrap();
        }
        e.finish()
    }

    fn hello(min: u16, max: u16, features: Features) -> Vec<u8> {
        stream(&[Command::Hello(Hello {
            min_version: min,
            max_version: max,
            features,
        })])
    }

    /// Session that has completed the handshake.
    fn session() -> GuestSession<NullBackend> {
        let mut s = GuestSession::new(NullBackend::new(Capabilities::rtx4050_assumed()));
        let out = s.execute_stream(&hello(1, 1, Features::empty()));
        assert!(matches!(out.replies.as_slice(), [Reply::HelloAck(_)]));
        s
    }

    /// Replies survive the wire: every outcome encodes to a reply stream that decodes back.
    fn wire(out: &StreamOutcome) -> Vec<Reply> {
        let bytes = out.reply_stream().unwrap();
        let decoded = decode_reply_stream(&bytes).unwrap();
        assert_eq!(decoded, out.replies);
        decoded
    }

    fn statuses(out: &StreamOutcome) -> Vec<SubmissionStatus> {
        out.replies
            .iter()
            .filter_map(|r| match r {
                Reply::CommandBufferStatus(s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    fn record_errors(out: &StreamOutcome) -> Vec<RecordError> {
        out.replies
            .iter()
            .filter_map(|r| match r {
                Reply::RecordError(e) => Some(*e),
                _ => None,
            })
            .collect()
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

    fn begin_pass(tex: u32) -> Command<'static> {
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
        )
    }

    fn draw(primitive_type: u8) -> Command<'static> {
        Command::Draw(Draw {
            primitive_type,
            vertex_start: 0,
            vertex_count: 3,
            instance_count: 1,
            base_instance: 0,
        })
    }

    fn triangle_pass(tex: u32) -> [Command<'static>; 3] {
        [begin_pass(tex), draw(3), Command::EndRenderPass]
    }

    fn completed(id: u64) -> SubmissionStatus {
        SubmissionStatus {
            submission_id: id,
            status: CommandBufferStatus::Completed,
        }
    }

    fn failed(id: u64, code: ErrorCode) -> SubmissionStatus {
        SubmissionStatus {
            submission_id: id,
            status: CommandBufferStatus::Error(code),
        }
    }

    // ---- handshake ------------------------------------------------------------------------

    #[test]
    fn hello_ack_carries_version_features_and_capabilities() {
        for caps in [
            Capabilities::rtx4050_assumed(),
            Capabilities::iris_xe_assumed(),
        ] {
            let expected = capability_block(&reported_feature_set(&caps));
            let mut s = GuestSession::new(NullBackend::new(caps));
            assert_eq!(s.negotiated(), None);
            // Guest offers shared events and an unknown bit; the host implements neither.
            let offer = Features::SHARED_EVENTS.union(Features::from_bits_retain(1 << 50));
            let out = s.execute_stream(&hello(1, 4, offer));
            assert_eq!(out.stream_seq, 0);
            assert_eq!(out.reply_version, BOOTSTRAP_VERSION);
            let [Reply::HelloAck(ack)] = wire(&out)[..] else {
                panic!("expected HelloAck, got {:?}", out.replies);
            };
            assert_eq!(ack.version, 1);
            assert_eq!(
                ack.features,
                Features::empty(),
                "never ack unimplemented features"
            );
            assert_eq!(ack.caps, expected);
            assert_eq!(
                ack.caps.families & !CapabilityBlock::FAMILY_KNOWN,
                0,
                "host only sends family bits defined in the negotiated version"
            );
            assert_eq!(
                s.negotiated(),
                Some(Negotiated {
                    version: 1,
                    features: Features::empty()
                })
            );
        }
    }

    #[test]
    fn capability_block_reflects_reported_features() {
        let rtx = capability_block(&reported_feature_set(&Capabilities::rtx4050_assumed()));
        assert_eq!(
            rtx.families,
            CapabilityBlock::FAMILY_MAC2
                | CapabilityBlock::FAMILY_COMMON3
                | CapabilityBlock::FAMILY_METAL3
        );
        assert_eq!(rtx.argument_buffers_tier, 2);
        assert_eq!(rtx.ray_tracing, SupportLevel::Native);
        assert_eq!(rtx.astc, SupportLevel::Unsupported);
        assert_eq!(rtx.unified_memory, SupportLevel::Emulated);
        let iris = capability_block(&reported_feature_set(&Capabilities::iris_xe_assumed()));
        assert_eq!(iris.ray_tracing, SupportLevel::Unsupported);
        assert_eq!(iris.unified_memory, SupportLevel::Native);
        assert_eq!(iris.families & CapabilityBlock::FAMILY_METAL3, 0);
        assert_eq!(iris.max_buffer_length, 2 << 30);
        let all = GuestSession::with_policy(
            NullBackend::new(Capabilities::iris_xe_assumed()),
            &EmulationPolicy::ALL,
        );
        let block = capability_block(all.reported_features());
        assert_ne!(block.families & CapabilityBlock::FAMILY_APPLE7, 0);
        assert_eq!(block.ray_tracing, SupportLevel::Emulated);
    }

    #[test]
    fn version_mismatch_is_rejected_and_session_stays_open_for_hello() {
        let mut s = GuestSession::new(NullBackend::new(Capabilities::rtx4050_assumed()));
        let out = s.execute_stream(&hello(2, 3, Features::empty()));
        assert_eq!(
            wire(&out),
            vec![Reply::HelloReject(HelloReject {
                host_min_version: 1,
                host_max_version: 1
            })]
        );
        assert_eq!(s.negotiated(), None);
        let out = s.execute_stream(&hello(1, 3, Features::empty()));
        assert_eq!(out.stream_seq, 1);
        assert!(matches!(wire(&out)[..], [Reply::HelloAck(_)]));
    }

    #[test]
    fn commands_before_hello_are_rejected_without_effect() {
        let mut s = GuestSession::new(NullBackend::new(Capabilities::rtx4050_assumed()));
        let out = s.execute_stream(&stream(&[create_buf(1, 64)]));
        assert_eq!(
            wire(&out),
            vec![Reply::SessionReset(StreamFault {
                stream_seq: 0,
                offset: proto::HEADER_LEN as u64,
                opcode: Opcode::CreateBuffer.raw(),
                code: ErrorCode::HelloRequired,
            })]
        );
        assert_eq!(out.records_executed, 0);
        assert_eq!(s.backend().live_buffers(), 0);
        assert_eq!(s.negotiated(), None);
        // HELLO mixed with commands is not a handshake either.
        let mut cmds = vec![Command::Hello(Hello {
            min_version: 1,
            max_version: 1,
            features: Features::empty(),
        })];
        cmds.push(create_buf(1, 64));
        let out = s.execute_stream(&stream(&cmds));
        assert!(matches!(
            wire(&out)[..],
            [Reply::SessionReset(StreamFault {
                code: ErrorCode::Malformed,
                ..
            })]
        ));
        assert_eq!(s.negotiated(), None);
        assert_eq!(s.backend().live_buffers(), 0);
    }

    #[test]
    fn duplicate_hello_is_an_error_and_resets_the_session() {
        let mut s = session();
        s.execute_stream(&stream(&[create_buf(1, 64), create_rt(2)]));
        assert_eq!(s.backend().live_buffers(), 1);
        let out = s.execute_stream(&hello(1, 1, Features::empty()));
        assert!(matches!(
            wire(&out)[..],
            [Reply::SessionReset(StreamFault {
                code: ErrorCode::DuplicateHello,
                stream_seq: 2,
                ..
            })]
        ));
        assert_eq!(s.negotiated(), None);
        assert_eq!(s.backend().live_buffers(), 0, "reset destroys all objects");
        assert_eq!(s.backend().live_textures(), 0);
        // Commands are refused until the guest negotiates again.
        let out = s.execute_stream(&stream(&[create_buf(1, 64)]));
        assert!(matches!(
            out.replies[..],
            [Reply::SessionReset(StreamFault {
                code: ErrorCode::HelloRequired,
                ..
            })]
        ));
        assert!(matches!(
            s.execute_stream(&hello(1, 1, Features::empty())).replies[..],
            [Reply::HelloAck(_)]
        ));
        assert!(
            s.execute_stream(&stream(&[create_buf(1, 64)]))
                .replies
                .is_empty()
        );
        assert_eq!(
            s.backend().live_buffers(),
            1,
            "old name 1 is free after reset"
        );
    }

    #[test]
    fn non_negotiated_events_reset_the_session() {
        let mut s = session();
        let ev = Command::SignalEvent(EventValue {
            event_id: oid(1),
            value: 1,
        });
        let out = s.execute_stream(&stream(&[create_buf(1, 64), ev]));
        assert!(matches!(
            wire(&out)[..],
            [Reply::SessionReset(StreamFault {
                code: ErrorCode::FeatureNotNegotiated,
                opcode,
                ..
            })] if opcode == Opcode::SignalEvent.raw()
        ));
        assert_eq!(s.backend().live_buffers(), 0, "validate, then execute");
    }

    #[test]
    fn malformed_stream_has_no_side_effects_and_resets() {
        let mut s = session();
        s.execute_stream(&stream(&[create_buf(7, 64)]));
        let mut bytes = stream(&[create_buf(1, 64), create_buf(2, 64)]);
        bytes.truncate(bytes.len() - 1);
        let out = s.execute_stream(&bytes);
        assert!(matches!(
            wire(&out)[..],
            [Reply::SessionReset(StreamFault {
                code: ErrorCode::Malformed,
                ..
            })]
        ));
        assert!(matches!(
            out.errors[..],
            [StreamError {
                record_index: None,
                error: FrontendError::Proto(_)
            }]
        ));
        assert_eq!(s.backend().live_buffers(), 0);
        let mut bad_version = StreamEncoder::with_version(9);
        bad_version.push(&Command::Nop).unwrap();
        let mut s = session();
        assert!(matches!(
            s.execute_stream(&bad_version.finish()).replies[..],
            [Reply::SessionReset(StreamFault {
                code: ErrorCode::UnsupportedVersion,
                offset: 0,
                ..
            })]
        ));
    }

    // ---- execution and replies --------------------------------------------------------------

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
        let out = s.execute_stream(&stream(&cmds));
        assert_eq!(out.stream_seq, 1);
        assert_eq!(out.records_executed, cmds.len());
        assert_eq!(out.submissions, vec![(77, SubmissionToken(1))]);
        assert_eq!(wire(&out), vec![Reply::CommandBufferStatus(completed(77))]);
        assert_eq!(s.backend().live_buffers(), 1);
        assert_eq!(s.backend().live_textures(), 1);
        assert_eq!(s.backend().commands_executed(), 3);
        assert_eq!(s.pending_commands(), 0);
        assert_eq!(s.in_flight_submissions(), 0);
    }

    #[test]
    fn record_errors_are_reported_and_execution_continues() {
        let mut s = session();
        let out = s.execute_stream(&stream(&[
            Command::DestroyBuffer { buffer_id: oid(5) },
            create_buf(1, 64),
            create_buf(1, 64),
            create_buf(3, 0),
        ]));
        let errs = record_errors(&out);
        assert_eq!(errs.len(), 3);
        assert_eq!(
            errs[0],
            RecordError {
                stream_seq: 1,
                record_index: 0,
                opcode: Opcode::DestroyBuffer.raw(),
                code: ErrorCode::UnknownObject,
                object_id: Some(oid(5)),
                detail: 0,
            }
        );
        assert_eq!(
            (errs[1].record_index, errs[1].code),
            (2, ErrorCode::DuplicateObject)
        );
        assert_eq!(errs[1].object_id, Some(oid(1)));
        assert_eq!(
            (errs[2].record_index, errs[2].code),
            (3, ErrorCode::InvalidArgument)
        );
        assert_eq!(out.records_executed, 1);
        assert_eq!(s.backend().live_buffers(), 1, "valid record took effect");
        assert!(
            !s.command_buffer_poisoned(),
            "resource errors do not poison"
        );
        wire(&out);
        // Bad enum value from the guest -> InvalidArgument.
        let bad_mode = Command::CreateBuffer(CreateBuffer {
            buffer_id: oid(9),
            storage_mode: 9,
            cpu_cache_mode: 0,
            hazard_tracking: 0,
            length: 64,
        });
        let out = s.execute_stream(&stream(&[bad_mode]));
        assert!(matches!(
            out.errors[..],
            [StreamError {
                record_index: Some(0),
                error: FrontendError::Model(ModelError::UnknownStorageMode(9))
            }]
        ));
        assert_eq!(record_errors(&out)[0].code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn out_of_memory_reports_requested_bytes() {
        let mut s = session();
        let max = s.reported_features().limits.max_buffer_length;
        let mut cmds = Vec::new();
        for id in 1..=4 {
            cmds.push(create_buf(id, max));
        }
        let out = s.execute_stream(&stream(&cmds));
        let oom: Vec<_> = record_errors(&out)
            .into_iter()
            .filter(|e| e.code == ErrorCode::OutOfMemory)
            .collect();
        assert!(!oom.is_empty(), "6 GiB budget cannot hold 4 x 4 GiB");
        assert!(oom.iter().all(|e| e.detail == max));
    }

    /// Regression for review finding WP-M0-REV (major): a semantic error in the middle of a
    /// command buffer must not let the remaining commands be submitted as "successful".
    #[test]
    fn regression_failed_record_poisons_command_buffer_across_streams() {
        let mut s = session();
        // Stream 1: CreateTexture, BeginRenderPass, Draw(primitive_type = 99).
        let out = s.execute_stream(&stream(&[create_rt(2), begin_pass(2), draw(99)]));
        assert_eq!(
            record_errors(&out),
            vec![RecordError {
                stream_seq: 1,
                record_index: 2,
                opcode: Opcode::Draw.raw(),
                code: ErrorCode::InvalidArgument,
                object_id: None,
                detail: 0,
            }]
        );
        assert!(matches!(
            out.errors[..],
            [StreamError {
                error: FrontendError::Model(ModelError::UnknownPrimitiveType(99)),
                ..
            }]
        ));
        assert_eq!(s.pending_commands(), 1);
        assert!(s.command_buffer_poisoned());
        // Stream 2: EndRenderPass, DestroyTexture, Commit{1}.
        let out = s.execute_stream(&stream(&[
            Command::EndRenderPass,
            Command::DestroyTexture { texture_id: oid(2) },
            Command::Commit { submission_id: 1 },
        ]));
        assert_eq!(
            wire(&out),
            vec![Reply::CommandBufferStatus(failed(
                1,
                ErrorCode::InvalidArgument
            ))]
        );
        assert!(out.submissions.is_empty(), "nothing reached the backend");
        assert_eq!(s.backend().commands_executed(), 0);
        assert_eq!(s.backend().completed(), SubmissionToken(0));
        assert_eq!(s.pending_commands(), 0, "pending dropped");
        assert!(
            !s.command_buffer_poisoned(),
            "next command buffer starts clean"
        );
        assert_eq!(s.backend().live_textures(), 0, "deferred destroy still ran");
        // The next command buffer is independent and succeeds.
        let mut cmds = vec![create_rt(3)];
        cmds.extend(triangle_pass(3));
        cmds.push(Command::Commit { submission_id: 2 });
        let out = s.execute_stream(&stream(&cmds));
        assert_eq!(statuses(&out), vec![completed(2)]);
        assert_eq!(s.backend().commands_executed(), 3);
    }

    #[test]
    fn records_after_poisoning_are_skipped_but_resource_commands_run() {
        let mut s = session();
        let out = s.execute_stream(&stream(&[
            begin_pass(42), // unknown texture -> poison
            draw(3),
            Command::EndRenderPass,
            create_buf(1, 64),
            Command::Commit { submission_id: 5 },
        ]));
        let errs = record_errors(&out);
        assert_eq!(errs.len(), 1, "skipped records are not reported again");
        assert_eq!(
            (errs[0].code, errs[0].object_id),
            (ErrorCode::UnknownObject, Some(oid(42)))
        );
        assert_eq!(statuses(&out), vec![failed(5, ErrorCode::UnknownObject)]);
        assert_eq!(s.backend().live_buffers(), 1);
        assert_eq!(out.records_executed, 2, "CreateBuffer and Commit");
    }

    #[test]
    fn destroy_is_deferred_until_commit() {
        let mut s = session();
        let mut cmds = vec![create_rt(2)];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::DestroyTexture { texture_id: oid(2) });
        assert!(s.execute_stream(&stream(&cmds)).replies.is_empty());
        assert_eq!(
            s.backend().live_textures(),
            1,
            "retained by pending commands"
        );
        // The guest name is free again immediately.
        s.execute_stream(&stream(&[create_rt(2)]));
        assert_eq!(s.backend().live_textures(), 2);
        let out = s.execute_stream(&stream(&[Command::Commit { submission_id: 1 }]));
        assert_eq!(statuses(&out), vec![completed(1)]);
        assert_eq!(s.backend().live_textures(), 1, "released after commit");
    }

    #[test]
    fn invalid_command_buffer_fails_at_commit() {
        let mut s = session();
        let out = s.execute_stream(&stream(&[draw(3), Command::Commit { submission_id: 1 }]));
        assert_eq!(statuses(&out), vec![failed(1, ErrorCode::InvalidCommand)]);
        assert!(record_errors(&out).is_empty());
        assert!(matches!(
            out.errors[..],
            [StreamError {
                record_index: Some(1),
                error: FrontendError::Gpu(GpuError::Guest(_))
            }]
        ));
        assert_eq!(s.pending_commands(), 0, "invalid command buffer is dropped");
    }

    #[test]
    fn record_errors_per_stream_are_bounded() {
        let mut s = session();
        let n = MAX_ERRORS_PER_STREAM + 36;
        let cmds: Vec<_> = (0..n)
            .map(|_| Command::DestroyBuffer { buffer_id: oid(5) })
            .collect();
        let out = s.execute_stream(&stream(&cmds));
        let errs = record_errors(&out);
        assert_eq!(errs.len(), MAX_ERRORS_PER_STREAM + 1);
        assert_eq!(
            errs.last().copied(),
            Some(RecordError {
                stream_seq: 1,
                record_index: NO_RECORD,
                opcode: 0,
                code: ErrorCode::TooManyErrors,
                object_id: None,
                detail: 36,
            })
        );
        assert_eq!(out.errors.len(), MAX_ERRORS_PER_STREAM);
        wire(&out);
    }

    /// Backend whose timeline only advances when the test says so.
    #[derive(Debug)]
    struct SlowBackend {
        inner: NullBackend,
        done: SubmissionToken,
    }

    impl GpuBackend for SlowBackend {
        fn capabilities(&self) -> &Capabilities {
            self.inner.capabilities()
        }
        fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId, GpuError> {
            self.inner.create_buffer(desc)
        }
        fn destroy_buffer(&mut self, id: BufferId) -> Result<(), GpuError> {
            self.inner.destroy_buffer(id)
        }
        fn write_buffer(&mut self, id: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
            self.inner.write_buffer(id, offset, data)
        }
        fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId, GpuError> {
            self.inner.create_texture(desc)
        }
        fn destroy_texture(&mut self, id: TextureId) -> Result<(), GpuError> {
            self.inner.destroy_texture(id)
        }
        fn submit(&mut self, submission: &Submission) -> Result<SubmissionToken, GpuError> {
            self.inner.submit(submission)
        }
        fn completed(&self) -> SubmissionToken {
            self.done
        }
    }

    #[test]
    fn completions_are_reported_in_commit_order_including_errors() {
        let mut s = GuestSession::new(SlowBackend {
            inner: NullBackend::new(Capabilities::rtx4050_assumed()),
            done: SubmissionToken(0),
        });
        assert!(matches!(
            s.execute_stream(&hello(1, 1, Features::empty())).replies[..],
            [Reply::HelloAck(_)]
        ));
        let mut cmds = vec![create_rt(2)];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::Commit { submission_id: 10 });
        cmds.push(draw(99)); // poisons command buffer 11
        cmds.push(Command::Commit { submission_id: 11 });
        cmds.extend(triangle_pass(2));
        cmds.push(Command::Commit { submission_id: 12 });
        let out = s.execute_stream(&stream(&cmds));
        assert!(
            statuses(&out).is_empty(),
            "nothing completed yet; error waits its turn"
        );
        assert_eq!(s.in_flight_submissions(), 3);
        s.backend_mut().done = SubmissionToken(1);
        assert_eq!(
            s.poll_completions(),
            vec![
                Reply::CommandBufferStatus(completed(10)),
                Reply::CommandBufferStatus(failed(11, ErrorCode::InvalidArgument)),
            ]
        );
        assert!(s.poll_completions().is_empty());
        s.backend_mut().done = SubmissionToken(2);
        assert_eq!(
            s.poll_completions(),
            vec![Reply::CommandBufferStatus(completed(12))]
        );
        assert_eq!(s.in_flight_submissions(), 0);
    }

    #[test]
    fn reset_drops_in_flight_statuses() {
        let mut s = GuestSession::new(SlowBackend {
            inner: NullBackend::new(Capabilities::rtx4050_assumed()),
            done: SubmissionToken(0),
        });
        s.execute_stream(&hello(1, 1, Features::empty()));
        let mut cmds = vec![create_rt(2)];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::Commit { submission_id: 1 });
        s.execute_stream(&stream(&cmds));
        assert_eq!(s.in_flight_submissions(), 1);
        s.execute_stream(&[0u8; 3]);
        assert_eq!(s.in_flight_submissions(), 0);
        s.backend_mut().done = SubmissionToken(1);
        assert!(s.poll_completions().is_empty());
    }

    #[test]
    fn random_streams_never_panic_and_replies_stay_valid() {
        // Mutated valid streams through handshake, frontend and backend.
        let mut cmds = vec![create_buf(1, 64), create_rt(2)];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::Commit { submission_id: 1 });
        cmds.push(Command::DestroyBuffer { buffer_id: oid(1) });
        let base = stream(&cmds);
        let hello_base = hello(1, 1, Features::SHARED_EVENTS);
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..5_000 {
            let mut s = GuestSession::new(NullBackend::new(Capabilities::rtx4050_assumed()));
            let mut h = hello_base.clone();
            if next() % 4 == 0 {
                let pos = (next() as usize) % h.len();
                h[pos] = (next() >> 32) as u8;
            }
            wire(&s.execute_stream(&h));
            for _ in 0..3 {
                let mut b = base.clone();
                let pos = (next() as usize) % b.len();
                b[pos] = (next() >> 32) as u8;
                wire(&s.execute_stream(&b));
            }
        }
    }

    // ---- review findings WP-GPU-01 ------------------------------------------------------

    fn small(limits: Limits) -> GuestSession<NullBackend> {
        session().with_limits(limits)
    }

    fn slow_session(limits: Limits) -> GuestSession<SlowBackend> {
        let mut s = GuestSession::new(SlowBackend {
            inner: NullBackend::new(Capabilities::rtx4050_assumed()),
            done: SubmissionToken(0),
        })
        .with_limits(limits);
        assert!(matches!(
            s.execute_stream(&hello(1, 1, Features::empty())).replies[..],
            [Reply::HelloAck(_)]
        ));
        s
    }

    #[test]
    fn command_buffer_membership_follows_opcode_class() {
        let legacy = [
            Opcode::CopyBuffer,
            Opcode::BeginRenderPass,
            Opcode::EndRenderPass,
            Opcode::Draw,
            Opcode::BeginComputePass,
            Opcode::EndComputePass,
            Opcode::Dispatch,
            Opcode::SignalEvent,
            Opcode::WaitEvent,
        ];
        for op in Opcode::ALL {
            let by_class = matches!(op.class(), OpcodeClass::Encoder | OpcodeClass::Sync);
            assert_eq!(opcode_belongs_to_command_buffer(*op), by_class, "{op:?}");
            assert_eq!(by_class, legacy.contains(op), "{op:?}");
        }
    }

    #[test]
    fn every_failing_encoder_command_poisons() {
        let failing: [&[Command<'static>]; 3] = [
            &[Command::CopyBuffer(proto::CopyBuffer {
                src: oid(8),
                dst: oid(9),
                src_offset: 0,
                dst_offset: 0,
                size: 4,
            })],
            &[begin_pass(42)],
            &[begin_pass(2), draw(99)],
        ];
        for cmds in failing {
            let mut s = session();
            let mut all = vec![create_rt(2)];
            all.extend_from_slice(cmds);
            all.push(Command::Commit { submission_id: 1 });
            let out = s.execute_stream(&stream(&all));
            assert_eq!(record_errors(&out).len(), 1, "{cmds:?}");
            assert!(
                matches!(
                    statuses(&out)[..],
                    [SubmissionStatus {
                        status: CommandBufferStatus::Error(_),
                        ..
                    }]
                ),
                "{cmds:?}"
            );
            assert!(out.submissions.is_empty());
            assert_eq!(s.backend().commands_executed(), 0);
        }
    }

    #[test]
    fn pending_limit_poisons_and_commit_fails_without_submit() {
        let mut s = small(Limits {
            pending_commands: 2,
            ..Limits::DEFAULT
        });
        let out = s.execute_stream(&stream(&[
            create_rt(2),
            begin_pass(2),
            draw(3),
            Command::EndRenderPass, // third pending command: limit
            Command::Commit { submission_id: 4 },
        ]));
        let errs = record_errors(&out);
        assert_eq!(errs.len(), 1);
        assert_eq!(
            (errs[0].record_index, errs[0].code),
            (3, ErrorCode::LimitExceeded)
        );
        assert_eq!(statuses(&out), vec![failed(4, ErrorCode::LimitExceeded)]);
        assert!(out.submissions.is_empty());
        assert_eq!(s.backend().commands_executed(), 0);
        assert_eq!(s.pending_commands(), 0);
    }

    #[test]
    fn failed_submit_still_runs_deferred_destroys() {
        let mut s = session();
        let out = s.execute_stream(&stream(&[
            create_rt(2),
            draw(3), // outside a render pass: rejected by the backend at submit
            Command::DestroyTexture { texture_id: oid(2) },
        ]));
        assert!(out.replies.is_empty());
        assert_eq!(s.backend().live_textures(), 1, "deferred");
        let out = s.execute_stream(&stream(&[Command::Commit { submission_id: 1 }]));
        assert_eq!(statuses(&out), vec![failed(1, ErrorCode::InvalidCommand)]);
        assert_eq!(
            s.backend().live_textures(),
            0,
            "released despite failed submit"
        );
    }

    #[test]
    fn poisoned_command_buffer_does_not_survive_a_reset() {
        let mut s = session();
        s.execute_stream(&stream(&[create_rt(2), begin_pass(2), draw(99)]));
        assert!(s.command_buffer_poisoned());
        let out = s.execute_stream(b"garbage");
        assert!(matches!(out.replies[..], [Reply::SessionReset(_)]));
        assert!(!s.command_buffer_poisoned());
        assert_eq!(s.pending_commands(), 0);
        assert!(matches!(
            s.execute_stream(&hello(1, 1, Features::empty())).replies[..],
            [Reply::HelloAck(_)]
        ));
        let mut cmds = vec![create_rt(2)];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::Commit { submission_id: 9 });
        let out = s.execute_stream(&stream(&cmds));
        assert_eq!(wire(&out), vec![Reply::CommandBufferStatus(completed(9))]);
    }

    #[test]
    fn object_limit_counts_deferred_destroys() {
        let mut s = small(Limits {
            objects_per_kind: 2,
            ..Limits::DEFAULT
        });
        let mut cmds = vec![create_rt(1)];
        cmds.extend(triangle_pass(1));
        cmds.push(Command::DestroyTexture { texture_id: oid(1) }); // deferred
        cmds.push(create_rt(2)); // 1 live + 1 deferred
        cmds.push(create_rt(3)); // would be 3 -> limit
        let out = s.execute_stream(&stream(&cmds));
        let errs = record_errors(&out);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code, ErrorCode::LimitExceeded);
        assert_eq!(s.backend().live_textures(), 2);
        // The commit releases the deferred texture; the name 3 can now be created.
        let out = s.execute_stream(&stream(&[
            Command::Commit { submission_id: 1 },
            create_rt(3),
        ]));
        assert_eq!(statuses(&out), vec![completed(1)]);
        assert!(record_errors(&out).is_empty());
        assert_eq!(s.backend().live_textures(), 2);
    }

    #[test]
    fn statuses_stay_in_commit_order_at_the_outstanding_limit() {
        let limits = Limits {
            in_flight: 3,
            ..Limits::DEFAULT
        };
        let three_commits = || {
            let mut cmds = vec![create_rt(2)];
            cmds.extend(triangle_pass(2));
            cmds.push(Command::Commit { submission_id: 10 });
            cmds.push(draw(99)); // poisons 11
            cmds.push(Command::Commit { submission_id: 11 });
            cmds.extend(triangle_pass(2));
            cmds.push(Command::Commit { submission_id: 12 });
            stream(&cmds)
        };

        // Over the limit: the whole stream is rejected and the session resets.
        let mut s = slow_session(limits);
        assert!(statuses(&s.execute_stream(&three_commits())).is_empty());
        assert_eq!(s.in_flight_submissions(), 3);
        let out = s.execute_stream(&stream(&[
            create_buf(1, 64),
            Command::Commit { submission_id: 13 },
        ]));
        assert_eq!(
            wire(&out),
            vec![Reply::SessionReset(StreamFault {
                stream_seq: 2,
                offset: 0,
                opcode: Opcode::Commit.raw(),
                code: ErrorCode::LimitExceeded,
            })]
        );
        assert_eq!(
            s.backend().inner.live_buffers(),
            0,
            "nothing of the stream ran"
        );
        assert_eq!(s.in_flight_submissions(), 0);

        // Within the limit after progress: earlier statuses first, all in commit order.
        let mut s = slow_session(limits);
        s.execute_stream(&three_commits());
        s.backend_mut().done = SubmissionToken(1);
        let out = s.execute_stream(&stream(&[Command::Commit { submission_id: 13 }]));
        assert_eq!(
            statuses(&out),
            vec![completed(10), failed(11, ErrorCode::InvalidArgument)]
        );
        assert_eq!(s.in_flight_submissions(), 2);
        s.backend_mut().done = SubmissionToken(3);
        assert_eq!(
            s.poll_completions(),
            vec![
                Reply::CommandBufferStatus(completed(12)),
                Reply::CommandBufferStatus(completed(13)),
            ]
        );
    }

    #[test]
    fn hello_with_nop_padding_is_accepted() {
        let mut s = GuestSession::new(NullBackend::new(Capabilities::rtx4050_assumed()));
        let out = s.execute_stream(&stream(&[
            Command::Nop,
            Command::Hello(Hello {
                min_version: 1,
                max_version: 1,
                features: Features::empty(),
            }),
            Command::Nop,
        ]));
        assert!(matches!(wire(&out)[..], [Reply::HelloAck(_)]));
        assert_eq!(out.records_executed, 3);
    }
}
