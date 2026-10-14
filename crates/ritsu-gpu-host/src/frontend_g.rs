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
//! * **Validate every payload before the model.** Every guest id is resolved, every raw
//!   Metal value converted with `from_raw`, every descriptor validated against the limits the
//!   guest knows (`ReportedFeatures::guest_limits`) and checked against this GPU (`caps::*_support`,
//!   gaps → `Unimplemented`) before the backend sees it. Encoder commands additionally run
//!   through an [`EncoderValidator`] at record time (binds/draws only inside a render pass,
//!   pipeline matches the pass, deferred store actions set before the end, vertex buffers of
//!   the vertex descriptor bound), so the failing record is reported precisely.
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

use std::collections::{HashMap, HashSet, VecDeque};

use ritsu_gpu_proto::Opcode;
use ritsu_gpu_proto::negotiate::negotiate;
use ritsu_gpu_proto::{
    self as proto, BOOTSTRAP_VERSION, CapabilityBlock, Command, CommandBufferStatus, ErrorCode,
    Features, HelloAck, NO_RECORD, Negotiated, ObjectId, OpcodeClass, ProtoError, RecordError,
    Reply, ReplyEncoder, StreamFault, SubmissionStatus, SupportLevel,
};

use crate::backend::{
    BufferId, DepthStencilStateId, GpuBackend, GpuCommand, GpuError, RenderPipelineId, SamplerId,
    Submission, SubmissionToken, TextureId,
};
use crate::caps::{
    ArgumentBuffersTier, EmulationPolicy, ReportedFeatures, Support, render_pipeline_support,
    reported_feature_set_with, texture_support,
};
use crate::encoder::EncoderValidator;
use crate::handle::HandleError;
use crate::model::vertex::optional_vertex_format;
use crate::model::{
    AttachmentDesc, AttachmentSlot, BlendFactor, BlendOperation, BufferDesc, ClearColor,
    ColorAttachmentDesc, ColorWriteMask, CompareFunction, DepthAttachmentDesc, DepthStencilDesc,
    LoadAction, MAX_SAMPLER_BINDINGS, MAX_TEXTURE_BINDINGS, ModelError,
    MultisampleDepthResolveFilter, MultisampleStencilResolveFilter, PipelineTargets, PixelFormat,
    PrimitiveTopologyClass, PrimitiveType, RenderPassDesc, RenderPipelineColorAttachmentDesc,
    RenderPipelineDesc, ResolveTarget, SamplerAddressMode, SamplerBorderColor, SamplerDesc,
    SamplerMinMagFilter, SamplerMipFilter, SamplerReductionMode, ShaderStage,
    StencilAttachmentDesc, StencilDesc, StorageMode, StoreAction, StoreActionOptions,
    SubresourceRange, TextureDesc, TextureSwizzleChannels, TextureType, TextureUsage,
    TextureViewDesc, VertexAttributeDesc, VertexBufferLayoutDesc, VertexDescriptor,
    VertexStepFunction, VertexStride, optional_pixel_format,
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
            Self::Model(ModelError::EncoderState(_))
            | Self::Gpu(GpuError::Model(ModelError::EncoderState(_))) => ErrorCode::InvalidCommand,
            Self::Model(m) | Self::Gpu(GpuError::Model(m)) if m.is_unsupported() => {
                ErrorCode::Unimplemented
            }
            Self::Model(_) | Self::Gpu(GpuError::Model(_)) => ErrorCode::InvalidArgument,
            Self::Gpu(GpuError::OutOfDeviceMemory { .. }) => ErrorCode::OutOfMemory,
            Self::Gpu(GpuError::Unimplemented(_)) | Self::Unimplemented(_) => {
                ErrorCode::Unimplemented
            }
            Self::Gpu(GpuError::Host(_)) => ErrorCode::HostError,
            Self::Gpu(GpuError::Handle(HandleError::Full { .. })) => ErrorCode::LimitExceeded,
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

/// Object kinds with their own guest namespace and per-session limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Buffer,
    /// Textures and texture views (one namespace, as in Metal).
    Texture,
    Sampler,
    DepthStencil,
    Pipeline,
}

impl Kind {
    const COUNT: usize = 5;

    const fn index(self) -> usize {
        self as usize
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Buffer => "buffer",
            Self::Texture => "texture",
            Self::Sampler => "sampler state",
            Self::DepthStencil => "depth/stencil state",
            Self::Pipeline => "render pipeline state",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Deferred {
    Buffer(BufferId),
    Texture(TextureId),
    Sampler(SamplerId),
    DepthStencil(DepthStencilStateId),
    Pipeline(RenderPipelineId),
}

impl Deferred {
    const fn kind(self) -> Kind {
        match self {
            Self::Buffer(_) => Kind::Buffer,
            Self::Texture(_) => Kind::Texture,
            Self::Sampler(_) => Kind::Sampler,
            Self::DepthStencil(_) => Kind::DepthStencil,
            Self::Pipeline(_) => Kind::Pipeline,
        }
    }
}

/// A guest buffer: backend handle and length (for bind-time offset checks).
#[derive(Debug, Clone, Copy)]
struct BufferEntry {
    id: BufferId,
    length: u64,
}

/// A guest render pipeline: backend handle and what binding it must agree with.
#[derive(Debug, Clone, Copy)]
struct PipelineEntry {
    id: RenderPipelineId,
    targets: PipelineTargets,
}

/// Fails if `key` is taken in `map` or the kind is at its limit (live + deferred).
fn admit<V>(
    map: &HashMap<u32, V>,
    key: ObjectId,
    kind: Kind,
    deferred: usize,
    limit: usize,
) -> Result<u32, FrontendError> {
    let key = key.get();
    if map.contains_key(&key) {
        return Err(FrontendError::DuplicateObject {
            kind: kind.name(),
            id: key,
        });
    }
    if map.len().saturating_add(deferred) >= limit {
        return Err(FrontendError::Limit(kind.name()));
    }
    Ok(key)
}

fn lookup<V: Copy>(map: &HashMap<u32, V>, id: ObjectId, kind: Kind) -> Result<V, FrontendError> {
    map.get(&id.get())
        .copied()
        .ok_or(FrontendError::UnknownObject {
            kind: kind.name(),
            id: id.get(),
        })
}

fn lookup_opt<V: Copy>(
    map: &HashMap<u32, V>,
    id: Option<ObjectId>,
    kind: Kind,
) -> Result<Option<V>, FrontendError> {
    id.map(|id| lookup(map, id, kind)).transpose()
}

/// Removes a guest name, returning what it named.
fn take<V>(map: &mut HashMap<u32, V>, id: ObjectId, kind: Kind) -> Result<V, FrontendError> {
    map.remove(&id.get()).ok_or(FrontendError::UnknownObject {
        kind: kind.name(),
        id: id.get(),
    })
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
    policy: EmulationPolicy,
    reported: ReportedFeatures,
    host_features: Features,
    negotiated: Option<Negotiated>,
    next_stream_seq: u64,
    buffers: HashMap<u32, BufferEntry>,
    textures: HashMap<u32, TextureId>,
    /// Descriptor of every backend texture/view this session created and has not destroyed
    /// yet (including those awaiting deferred destruction), for render pass validation.
    texture_descs: HashMap<TextureId, TextureDesc>,
    /// Parent of every view the backend still holds (mirrors Metal's "a view retains its
    /// parent", so the object limit also covers parents kept alive by views).
    view_parents: HashMap<TextureId, TextureId>,
    /// Number of views the backend still holds per texture (only entries > 0).
    view_counts: HashMap<TextureId, u32>,
    /// Textures whose destruction ran but which views keep alive in the backend; they count
    /// toward the texture limit like deferred destroys.
    retained_textures: HashSet<TextureId>,
    samplers: HashMap<u32, SamplerId>,
    depth_stencils: HashMap<u32, DepthStencilStateId>,
    pipelines: HashMap<u32, PipelineEntry>,
    pending: Vec<GpuCommand>,
    deferred_destroy: Vec<Deferred>,
    /// Objects per [`Kind`] awaiting deferred destruction (count toward the object limit).
    deferred: [usize; Kind::COUNT],
    limits: Limits,
    /// Encoder state of the command buffer being recorded (record-time validation).
    encoder: EncoderValidator<TextureId>,
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
            policy: *policy,
            reported,
            host_features: HOST_FEATURES,
            negotiated: None,
            next_stream_seq: 0,
            buffers: HashMap::new(),
            textures: HashMap::new(),
            texture_descs: HashMap::new(),
            view_parents: HashMap::new(),
            view_counts: HashMap::new(),
            retained_textures: HashSet::new(),
            samplers: HashMap::new(),
            depth_stencils: HashMap::new(),
            pipelines: HashMap::new(),
            pending: Vec::new(),
            deferred_destroy: Vec::new(),
            deferred: [0; Kind::COUNT],
            limits: Limits::DEFAULT,
            encoder: EncoderValidator::new(),
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
        self.deferred = [0; Kind::COUNT];
        self.encoder.reset();
        self.poisoned = None;
        // No statuses for earlier submissions after a reset (documented in the reply).
        self.in_flight.clear();
        let mut doomed: Vec<Deferred> = core::mem::take(&mut self.deferred_destroy);
        doomed.extend(self.buffers.drain().map(|(_, b)| Deferred::Buffer(b.id)));
        doomed.extend(self.textures.drain().map(|(_, t)| Deferred::Texture(t)));
        doomed.extend(self.samplers.drain().map(|(_, v)| Deferred::Sampler(v)));
        doomed.extend(
            self.depth_stencils
                .drain()
                .map(|(_, v)| Deferred::DepthStencil(v)),
        );
        doomed.extend(
            self.pipelines
                .drain()
                .map(|(_, v)| Deferred::Pipeline(v.id)),
        );
        for obj in doomed {
            if let Err(e) = self.destroy_now(obj) {
                out.diagnostic(None, e.into());
            }
        }
        // Every view was destroyed above, so the backend released all retained parents.
        self.view_parents.clear();
        self.view_counts.clear();
        self.retained_textures.clear();
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
        lookup(&self.buffers, id, Kind::Buffer).map(|b| b.id)
    }

    fn texture(&self, id: ObjectId) -> Result<TextureId, FrontendError> {
        lookup(&self.textures, id, Kind::Texture)
    }

    /// Descriptor of a backend texture of this session (also after the guest destroyed its
    /// name, until the deferred destruction ran).
    fn texture_desc(&self, id: TextureId) -> Result<TextureDesc, FrontendError> {
        texture_desc_in(&self.texture_descs, id)
    }

    /// Checks the per-kind object limit and the guest name for a new object.
    fn admit<V>(
        &self,
        map: &HashMap<u32, V>,
        key: ObjectId,
        kind: Kind,
    ) -> Result<u32, FrontendError> {
        let mut held = self.deferred[kind.index()];
        if kind == Kind::Texture {
            // Parents kept alive by views hold backend storage without a guest name.
            held = held.saturating_add(self.retained_textures.len());
        }
        admit(map, key, kind, held, self.limits.objects_per_kind)
    }

    /// Destroys a texture or view in the backend and mirrors the backend's retention rule:
    /// a texture with live views stays (retained, counted toward the limit) until its last
    /// view is gone; releasing a view may release its parent chain.
    fn destroy_texture_now(&mut self, t: TextureId) -> Result<(), GpuError> {
        self.texture_descs.remove(&t);
        self.backend.destroy_texture(t)?;
        if self.view_counts.contains_key(&t) {
            self.retained_textures.insert(t);
            return Ok(());
        }
        // Walk up the chain of parents that only this view kept alive (bounded by the number
        // of textures, iterative).
        let mut child = t;
        while let Some(parent) = self.view_parents.remove(&child) {
            let remaining = match self.view_counts.get_mut(&parent) {
                Some(n) => {
                    *n = n.saturating_sub(1);
                    *n
                }
                None => 0,
            };
            if remaining > 0 {
                break;
            }
            self.view_counts.remove(&parent);
            if !self.retained_textures.remove(&parent) {
                break;
            }
            child = parent;
        }
        Ok(())
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
            Deferred::Texture(t) => self.destroy_texture_now(t),
            Deferred::Sampler(v) => self.backend.destroy_sampler(v),
            Deferred::DepthStencil(v) => self.backend.destroy_depth_stencil_state(v),
            Deferred::Pipeline(v) => self.backend.destroy_render_pipeline(v),
        }
    }

    fn destroy_or_defer(&mut self, obj: Deferred) -> Result<(), FrontendError> {
        if self.pending.is_empty() {
            self.destroy_now(obj)?;
        } else {
            // Counted against the per-kind object limit until the commit releases it.
            let n = &mut self.deferred[obj.kind().index()];
            *n = n.saturating_add(1);
            self.deferred_destroy.push(obj);
        }
        Ok(())
    }

    fn commit(&mut self, submission_id: u64, index: u32, out: &mut StreamOutcome) {
        let submission = Submission {
            commands: core::mem::take(&mut self.pending),
        };
        let open_encoder = self.encoder.finish();
        self.encoder.reset();
        let state = if let Some(code) = self.poisoned.take() {
            // Never submit a partial command buffer (CLAUDE.md §3.5).
            out.diagnostic(Some(index), FrontendError::PoisonedCommandBuffer(code));
            Err(code)
        } else if let Err(e) = open_encoder {
            let e = FrontendError::from(e);
            let code = e.code();
            out.diagnostic(Some(index), e);
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
        self.deferred = [0; Kind::COUNT];
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
                let key = self.admit(&self.buffers, c.buffer_id, Kind::Buffer)?;
                let desc = BufferDesc::from_raw(
                    c.length,
                    u32::from(c.storage_mode),
                    u32::from(c.cpu_cache_mode),
                    u32::from(c.hazard_tracking),
                )?;
                let id = self.backend.create_buffer(&desc)?;
                self.buffers.insert(
                    key,
                    BufferEntry {
                        id,
                        length: desc.length,
                    },
                );
            }
            Command::DestroyBuffer { buffer_id } => {
                let b = take(&mut self.buffers, *buffer_id, Kind::Buffer)?;
                self.destroy_or_defer(Deferred::Buffer(b.id))?;
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
                self.encoder.blit()?;
                self.record(cmd)?;
            }
            Command::CreateTexture(c) => {
                let key = self.admit(&self.textures, c.texture_id, Kind::Texture)?;
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
                    swizzle: TextureSwizzleChannels::from_raw(c.swizzle)?,
                };
                // Metal rules first (guest error), then what this GPU can honour for the
                // format with the session's emulations (unimplemented), then the backend.
                desc.validate(&self.reported.guest_limits)?;
                texture_support(self.backend.capabilities(), &self.policy, &desc)?;
                let id = self.backend.create_texture(&desc)?;
                self.textures.insert(key, id);
                self.texture_descs.insert(id, desc);
            }
            Command::CreateTextureView(c) => {
                let key = self.admit(&self.textures, c.texture_id, Kind::Texture)?;
                let parent = self.texture(c.parent_id)?;
                let view = TextureViewDesc {
                    pixel_format: PixelFormat::from_raw(u32::from(c.pixel_format))?,
                    texture_type: TextureType::from_raw(u32::from(c.texture_type))?,
                    levels: SubresourceRange::new(c.level_start, c.level_count),
                    slices: SubresourceRange::new(c.slice_start, c.slice_count),
                    swizzle: TextureSwizzleChannels::from_raw(c.swizzle)?,
                };
                // The view's own descriptor (base level extent, composed swizzle, parent
                // usage) must be honoured by the GPU like a texture's.
                let desc = view.validate(&self.texture_desc(parent)?)?;
                texture_support(self.backend.capabilities(), &self.policy, &desc)?;
                let id = self.backend.create_texture_view(parent, &view)?;
                self.textures.insert(key, id);
                self.texture_descs.insert(id, desc);
                self.view_parents.insert(id, parent);
                let n = self.view_counts.entry(parent).or_insert(0);
                *n = n.saturating_add(1);
            }
            Command::DestroyTexture { texture_id } => {
                let id = take(&mut self.textures, *texture_id, Kind::Texture)?;
                self.destroy_or_defer(Deferred::Texture(id))?;
            }
            Command::CreateSampler(c) => {
                let key = self.admit(&self.samplers, c.sampler_id, Kind::Sampler)?;
                let desc = sampler_desc(c)?;
                desc.validate()?;
                let id = self.backend.create_sampler(&desc)?;
                self.samplers.insert(key, id);
            }
            Command::DestroySampler { sampler_id } => {
                let id = take(&mut self.samplers, *sampler_id, Kind::Sampler)?;
                self.destroy_or_defer(Deferred::Sampler(id))?;
            }
            Command::CreateDepthStencilState(c) => {
                let key = self.admit(&self.depth_stencils, c.dss_id, Kind::DepthStencil)?;
                let desc = depth_stencil_desc(c)?;
                desc.validate()?;
                let id = self.backend.create_depth_stencil_state(&desc)?;
                self.depth_stencils.insert(key, id);
            }
            Command::DestroyDepthStencilState { dss_id } => {
                let id = take(&mut self.depth_stencils, *dss_id, Kind::DepthStencil)?;
                self.destroy_or_defer(Deferred::DepthStencil(id))?;
            }
            Command::CreateRenderPipelineState(c) => {
                let key = self.admit(&self.pipelines, c.pipeline_id, Kind::Pipeline)?;
                if c.vertex_function.is_some() || c.fragment_function.is_some() {
                    // Placeholder fields; function objects arrive with WP-SC-09/12.
                    return Err(FrontendError::Unimplemented(
                        "shader functions in render pipelines (library/function opcodes)",
                    ));
                }
                let desc = pipeline_desc(c)?;
                desc.validate(&self.reported.guest_limits)?;
                render_pipeline_support(self.backend.capabilities(), &self.policy, &desc)?;
                let id = self.backend.create_render_pipeline(&desc)?;
                self.pipelines.insert(
                    key,
                    PipelineEntry {
                        id,
                        targets: desc.targets(),
                    },
                );
            }
            Command::DestroyRenderPipelineState { pipeline_id } => {
                let p = take(&mut self.pipelines, *pipeline_id, Kind::Pipeline)?;
                self.destroy_or_defer(Deferred::Pipeline(p.id))?;
            }
            Command::BeginRenderPass(rp) => {
                // Compact form: dense colour attachments at level/slice 0, no resolve
                // textures (a resolving store action therefore fails validation instead of
                // silently dropping the resolve), `float` clear colours.
                let mut pass = Box::new(RenderPassDesc::empty());
                pass.render_target_width = rp.width;
                pass.render_target_height = rp.height;
                for (slot, a) in pass
                    .color_attachments
                    .iter_mut()
                    .zip(rp.color_attachments())
                {
                    let [red, green, blue, alpha] = a.clear_color.map(f64::from);
                    *slot = Some(ColorAttachmentDesc {
                        attachment: AttachmentDesc::new(
                            self.texture(a.texture_id)?,
                            LoadAction::from_raw(u32::from(a.load_action))?,
                            StoreAction::from_raw(u32::from(a.store_action))?,
                        ),
                        clear_color: ClearColor {
                            red,
                            green,
                            blue,
                            alpha,
                        },
                    });
                }
                self.begin_render_pass(pass)?;
            }
            Command::BeginRenderPass2(rp) => {
                let pass = self.render_pass_desc(rp)?;
                self.begin_render_pass(pass)?;
            }
            Command::SetStoreAction(s) => {
                let attachment = match s.attachment {
                    proto::AttachmentKind::Color(i) => AttachmentSlot::Color(i),
                    proto::AttachmentKind::Depth => AttachmentSlot::Depth,
                    proto::AttachmentKind::Stencil => AttachmentSlot::Stencil,
                };
                let action = StoreAction::from_raw(u32::from(s.store_action))?;
                let descs = &self.texture_descs;
                self.encoder.set_store_action(
                    attachment,
                    action,
                    &self.reported.guest_limits,
                    self.backend.capabilities(),
                    |id| texture_desc_in(descs, id),
                )?;
                self.record(GpuCommand::SetStoreAction { attachment, action })?;
            }
            Command::SetRenderPipelineState { pipeline_id } => {
                let p = lookup(&self.pipelines, *pipeline_id, Kind::Pipeline)?;
                self.encoder.set_pipeline(&p.targets)?;
                self.record(GpuCommand::SetRenderPipelineState(p.id))?;
            }
            Command::SetDepthStencilState { dss_id } => {
                let id = lookup_opt(&self.depth_stencils, *dss_id, Kind::DepthStencil)?;
                self.encoder.render_command()?;
                self.record(GpuCommand::SetDepthStencilState(id))?;
            }
            Command::SetVertexBuffer(b) => {
                let buffer = lookup_opt(&self.buffers, b.buffer_id, Kind::Buffer)?;
                if b.index >= self.reported.guest_limits.max_vertex_buffers {
                    return Err(ModelError::InvalidValue {
                        field: "vertex buffer index",
                        reason: "exceeds the buffer argument table",
                    }
                    .into());
                }
                if let Some(entry) = buffer
                    && b.offset >= entry.length
                {
                    return Err(ModelError::InvalidValue {
                        field: "vertex buffer offset",
                        reason: "not inside the buffer",
                    }
                    .into());
                }
                let attribute_stride = (b.attribute_stride
                    != proto::SetVertexBuffer::STRIDE_STATIC)
                    .then_some(b.attribute_stride);
                self.encoder.set_vertex_buffer(
                    b.index,
                    buffer.is_some(),
                    attribute_stride.is_some(),
                )?;
                self.record(GpuCommand::SetVertexBuffer {
                    index: b.index,
                    buffer: buffer.map(|e| e.id),
                    offset: b.offset,
                    attribute_stride,
                })?;
            }
            Command::SetVertexTexture(b) => self.bind_texture(ShaderStage::Vertex, b)?,
            Command::SetFragmentTexture(b) => self.bind_texture(ShaderStage::Fragment, b)?,
            Command::SetVertexSamplerState(b) => self.bind_sampler(ShaderStage::Vertex, b)?,
            Command::SetFragmentSamplerState(b) => self.bind_sampler(ShaderStage::Fragment, b)?,
            Command::EndRenderPass => {
                self.encoder.end_render()?;
                self.record(GpuCommand::EndRenderPass)?;
            }
            Command::Draw(d) => {
                let primitive = PrimitiveType::from_raw(u32::from(d.primitive_type))?;
                self.encoder.draw(primitive)?;
                self.record(GpuCommand::Draw {
                    primitive,
                    vertex_start: d.vertex_start,
                    vertex_count: d.vertex_count,
                    instance_count: d.instance_count,
                    base_instance: d.base_instance,
                })?;
            }
            Command::BeginComputePass => {
                self.encoder.begin_compute()?;
                self.record(GpuCommand::BeginComputePass)?;
            }
            Command::EndComputePass => {
                self.encoder.end_compute()?;
                self.record(GpuCommand::EndComputePass)?;
            }
            Command::Dispatch(d) => {
                self.encoder
                    .dispatch(d.threads_per_threadgroup, &self.reported.guest_limits)?;
                self.record(GpuCommand::Dispatch {
                    threadgroups: d.threadgroups,
                    threads_per_threadgroup: d.threads_per_threadgroup,
                })?;
            }
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

    /// Validates a render pass against the guest-visible limits and this GPU (record time),
    /// opens the render encoder and records the pass.
    fn begin_render_pass(
        &mut self,
        pass: Box<RenderPassDesc<TextureId>>,
    ) -> Result<(), FrontendError> {
        let descs = &self.texture_descs;
        self.encoder.begin_render(
            &pass,
            &self.reported.guest_limits,
            self.backend.capabilities(),
            |id| texture_desc_in(descs, id),
        )?;
        self.record(GpuCommand::BeginRenderPass(pass))
    }

    /// `BeginRenderPass2` → model (every guest id resolved, every enum validated).
    fn render_pass_desc(
        &self,
        rp: &proto::BeginRenderPass2<'_>,
    ) -> Result<Box<RenderPassDesc<TextureId>>, FrontendError> {
        let mut pass = Box::new(RenderPassDesc::empty());
        pass.render_target_width = rp.render_target_width;
        pass.render_target_height = rp.render_target_height;
        pass.render_target_array_length = rp.render_target_array_length;
        pass.default_raster_sample_count = rp.default_raster_sample_count;
        for c in rp.color_attachments.iter() {
            // The wire guarantees index < 8 and no duplicates.
            let slot = pass.color_attachments.get_mut(usize::from(c.index)).ok_or(
                ModelError::InvalidValue {
                    field: "colour attachment index",
                    reason: "out of range",
                },
            )?;
            let [red, green, blue, alpha] = c.clear_color;
            *slot = Some(ColorAttachmentDesc {
                attachment: self.attachment_desc(&c.attachment)?,
                clear_color: ClearColor {
                    red,
                    green,
                    blue,
                    alpha,
                },
            });
        }
        if let Some(d) = &rp.depth_attachment {
            pass.depth_attachment = Some(DepthAttachmentDesc {
                attachment: self.attachment_desc(&d.attachment)?,
                clear_depth: d.clear_depth,
                resolve_filter: MultisampleDepthResolveFilter::from_raw(u32::from(
                    d.resolve_filter,
                ))?,
            });
        }
        if let Some(s) = &rp.stencil_attachment {
            pass.stencil_attachment = Some(StencilAttachmentDesc {
                attachment: self.attachment_desc(&s.attachment)?,
                clear_stencil: s.clear_stencil,
                resolve_filter: MultisampleStencilResolveFilter::from_raw(u32::from(
                    s.resolve_filter,
                ))?,
            });
        }
        Ok(pass)
    }

    fn attachment_desc(
        &self,
        a: &proto::AttachmentRecord,
    ) -> Result<AttachmentDesc<TextureId>, FrontendError> {
        let resolve = match &a.resolve {
            Some(r) => Some(ResolveTarget {
                texture: self.texture(r.texture_id)?,
                level: r.level,
                slice: r.slice,
                depth_plane: r.depth_plane,
            }),
            None => None,
        };
        Ok(AttachmentDesc {
            texture: self.texture(a.texture_id)?,
            level: a.level,
            slice: a.slice,
            depth_plane: a.depth_plane,
            load: LoadAction::from_raw(u32::from(a.load_action))?,
            store: StoreAction::from_raw(u32::from(a.store_action))?,
            store_options: StoreActionOptions::from_raw(a.store_action_options)?,
            resolve,
        })
    }

    fn bind_texture(
        &mut self,
        stage: ShaderStage,
        b: &proto::Binding,
    ) -> Result<(), FrontendError> {
        let texture = lookup_opt(&self.textures, b.object_id, Kind::Texture)?;
        if b.index >= MAX_TEXTURE_BINDINGS {
            return Err(ModelError::InvalidValue {
                field: "texture index",
                reason: "exceeds the texture argument table",
            }
            .into());
        }
        self.encoder.render_command()?;
        self.record(GpuCommand::SetTexture {
            stage,
            index: b.index,
            texture,
        })
    }

    fn bind_sampler(
        &mut self,
        stage: ShaderStage,
        b: &proto::Binding,
    ) -> Result<(), FrontendError> {
        let sampler = lookup_opt(&self.samplers, b.object_id, Kind::Sampler)?;
        if b.index >= MAX_SAMPLER_BINDINGS {
            return Err(ModelError::InvalidValue {
                field: "sampler index",
                reason: "exceeds the sampler state argument table",
            }
            .into());
        }
        self.encoder.render_command()?;
        self.record(GpuCommand::SetSamplerState {
            stage,
            index: b.index,
            sampler,
        })
    }
}

fn texture_desc_in(
    descs: &HashMap<TextureId, TextureDesc>,
    id: TextureId,
) -> Result<TextureDesc, FrontendError> {
    descs
        .get(&id)
        .copied()
        .ok_or(FrontendError::Gpu(GpuError::Host(
            "texture descriptor missing".into(),
        )))
}

/// `CreateSampler` → model (enums validated, not yet the descriptor rules).
fn sampler_desc(c: &proto::CreateSampler) -> Result<SamplerDesc, ModelError> {
    let [s, t, r] = c.address_modes;
    let raw = |v: u8| u32::from(v);
    Ok(SamplerDesc {
        min_filter: SamplerMinMagFilter::from_raw(raw(c.min_filter))?,
        mag_filter: SamplerMinMagFilter::from_raw(raw(c.mag_filter))?,
        mip_filter: SamplerMipFilter::from_raw(raw(c.mip_filter))?,
        max_anisotropy: c.max_anisotropy,
        s_address_mode: SamplerAddressMode::from_raw(raw(s))?,
        t_address_mode: SamplerAddressMode::from_raw(raw(t))?,
        r_address_mode: SamplerAddressMode::from_raw(raw(r))?,
        border_color: SamplerBorderColor::from_raw(raw(c.border_color))?,
        reduction_mode: SamplerReductionMode::from_raw(raw(c.reduction_mode))?,
        normalized_coordinates: c.flags & proto::CreateSampler::NORMALIZED_COORDINATES != 0,
        lod_min_clamp: c.lod_min_clamp,
        lod_max_clamp: c.lod_max_clamp,
        lod_average: c.flags & proto::CreateSampler::LOD_AVERAGE != 0,
        lod_bias: c.lod_bias,
        compare_function: CompareFunction::from_raw(raw(c.compare_function))?,
        support_argument_buffers: c.flags & proto::CreateSampler::SUPPORT_ARGUMENT_BUFFERS != 0,
    })
}

/// `CreateDepthStencilState` → model.
fn depth_stencil_desc(c: &proto::CreateDepthStencilState) -> Result<DepthStencilDesc, ModelError> {
    let face = |f: Option<proto::StencilRecord>| {
        f.map(|f| {
            StencilDesc::from_raw(
                u32::from(f.compare),
                u32::from(f.stencil_failure),
                u32::from(f.depth_failure),
                u32::from(f.depth_stencil_pass),
                f.read_mask,
                f.write_mask,
            )
        })
        .transpose()
    };
    Ok(DepthStencilDesc {
        depth_compare: CompareFunction::from_raw(u32::from(c.depth_compare))?,
        depth_write_enabled: c.depth_write_enabled,
        front_face_stencil: face(c.front_face_stencil)?,
        back_face_stencil: face(c.back_face_stencil)?,
    })
}

/// `CreateRenderPipelineState` → model (shader function references are handled by the
/// caller).
fn pipeline_desc(
    c: &proto::CreateRenderPipelineState<'_>,
) -> Result<RenderPipelineDesc, ModelError> {
    use proto::CreateRenderPipelineState as P;
    let raw = |v: u8| u32::from(v);
    let mut desc = RenderPipelineDesc {
        depth_attachment_pixel_format: optional_pixel_format(u32::from(c.depth_pixel_format))?,
        stencil_attachment_pixel_format: optional_pixel_format(u32::from(c.stencil_pixel_format))?,
        raster_sample_count: raw(c.raster_sample_count),
        alpha_to_coverage_enabled: c.flags & P::ALPHA_TO_COVERAGE != 0,
        alpha_to_one_enabled: c.flags & P::ALPHA_TO_ONE != 0,
        rasterization_enabled: c.flags & P::RASTERIZATION_ENABLED != 0,
        input_primitive_topology: PrimitiveTopologyClass::from_raw(
            raw(c.input_primitive_topology),
        )?,
        max_vertex_amplification_count: c.max_vertex_amplification_count,
        ..RenderPipelineDesc::default()
    };
    // The wire guarantees at most MAX_COLOR_ATTACHMENTS entries.
    for (slot, a) in desc
        .color_attachments
        .iter_mut()
        .zip(c.color_attachments.iter())
    {
        *slot = RenderPipelineColorAttachmentDesc {
            pixel_format: optional_pixel_format(u32::from(a.pixel_format))?,
            blending_enabled: a.blending_enabled,
            source_rgb_blend_factor: BlendFactor::from_raw(raw(a.source_rgb_blend_factor))?,
            destination_rgb_blend_factor: BlendFactor::from_raw(raw(
                a.destination_rgb_blend_factor
            ))?,
            rgb_blend_operation: BlendOperation::from_raw(raw(a.rgb_blend_operation))?,
            source_alpha_blend_factor: BlendFactor::from_raw(raw(a.source_alpha_blend_factor))?,
            destination_alpha_blend_factor: BlendFactor::from_raw(raw(
                a.destination_alpha_blend_factor
            ))?,
            alpha_blend_operation: BlendOperation::from_raw(raw(a.alpha_blend_operation))?,
            write_mask: ColorWriteMask::from_raw(raw(a.write_mask))?,
        };
    }
    if c.flags & P::VERTEX_DESCRIPTOR != 0 {
        let mut v = VertexDescriptor::default();
        // The wire guarantees indices < 31 (the size of both tables), strictly increasing.
        for a in c.vertex_attributes.iter() {
            if let Some(slot) = v.attributes.get_mut(usize::from(a.index)) {
                *slot = optional_vertex_format(u32::from(a.format))?.map(|format| {
                    VertexAttributeDesc {
                        format,
                        offset: a.offset,
                        buffer_index: a.buffer_index,
                    }
                });
            }
        }
        for l in c.vertex_layouts.iter() {
            if let Some(slot) = v.layouts.get_mut(usize::from(l.index)) {
                *slot = VertexBufferLayoutDesc {
                    stride: VertexStride::from_raw(l.stride),
                    step_function: VertexStepFunction::from_raw(raw(l.step_function))?,
                    step_rate: l.step_rate,
                };
            }
        }
        desc.vertex_descriptor = Some(v);
    }
    Ok(desc)
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

    /// Guest id of the render pipeline every test session creates (BGRA8Unorm, one sample,
    /// no depth, any topology): draws need a bound pipeline.
    const PIPE: u32 = 900;

    /// Record lists borrow their bytes; tests leak small buffers to get `'static` commands.
    fn list<R: proto::WireRecord>(records: &[R]) -> proto::RecordList<'static, R> {
        proto::RecordList::parse(proto::encode_records(records).leak()).unwrap()
    }

    /// `CreateRenderPipelineState` with one colour format, optional depth format, sample
    /// count, topology class and an optional one-attribute vertex descriptor.
    fn create_pipeline(
        id: u32,
        color: PixelFormat,
        depth: Option<PixelFormat>,
        samples: u8,
        topology: PrimitiveTopologyClass,
    ) -> Command<'static> {
        Command::CreateRenderPipelineState(proto::CreateRenderPipelineState {
            pipeline_id: oid(id),
            vertex_function: None,
            fragment_function: None,
            depth_pixel_format: depth.map_or(0, |f| f.raw() as u16),
            stencil_pixel_format: 0,
            raster_sample_count: samples,
            input_primitive_topology: topology.raw() as u8,
            flags: proto::CreateRenderPipelineState::RASTERIZATION_ENABLED,
            max_vertex_amplification_count: 1,
            color_attachments: list(&[proto::PipelineColorRecord::with_format(color.raw() as u16)]),
            vertex_attributes: proto::RecordList::empty(),
            vertex_layouts: proto::RecordList::empty(),
        })
    }

    fn bind_pipe() -> Command<'static> {
        Command::SetRenderPipelineState {
            pipeline_id: oid(PIPE),
        }
    }

    /// HELLO plus the default pipeline [`PIPE`].
    fn handshake<B: GpuBackend>(s: &mut GuestSession<B>) {
        let out = s.execute_stream(&hello(1, 1, Features::empty()));
        assert!(matches!(out.replies.as_slice(), [Reply::HelloAck(_)]));
        let out = s.execute_stream(&stream(&[create_pipeline(
            PIPE,
            PixelFormat::BGRA8Unorm,
            None,
            1,
            PrimitiveTopologyClass::Unspecified,
        )]));
        assert!(out.replies.is_empty(), "{:?}", out.replies);
    }

    /// Session that has completed the handshake (and created [`PIPE`]).
    fn session() -> GuestSession<NullBackend> {
        let mut s = GuestSession::new(NullBackend::new(Capabilities::rtx4050_assumed()));
        handshake(&mut s);
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
            swizzle: proto::SWIZZLE_IDENTITY,
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

    fn triangle_pass(tex: u32) -> [Command<'static>; 4] {
        [
            begin_pass(tex),
            bind_pipe(),
            draw(3),
            Command::EndRenderPass,
        ]
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
                stream_seq: 3,
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
        assert_eq!(out.stream_seq, 2);
        assert_eq!(out.records_executed, cmds.len());
        assert_eq!(out.submissions, vec![(77, SubmissionToken(1))]);
        assert_eq!(wire(&out), vec![Reply::CommandBufferStatus(completed(77))]);
        assert_eq!(s.backend().live_buffers(), 1);
        assert_eq!(s.backend().live_textures(), 1);
        assert_eq!(s.backend().commands_executed(), 4);
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
                stream_seq: 2,
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
                stream_seq: 2,
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
        assert_eq!(s.backend().commands_executed(), 4);
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
    fn invalid_command_order_fails_at_record_time() {
        let mut s = session();
        let out = s.execute_stream(&stream(&[draw(3), Command::Commit { submission_id: 1 }]));
        assert_eq!(statuses(&out), vec![failed(1, ErrorCode::InvalidCommand)]);
        assert_eq!(
            record_errors(&out)
                .iter()
                .map(|e| (e.record_index, e.code))
                .collect::<Vec<_>>(),
            vec![(0, ErrorCode::InvalidCommand)]
        );
        assert!(matches!(
            out.errors[..],
            [
                StreamError {
                    record_index: Some(0),
                    error: FrontendError::Model(ModelError::EncoderState(_))
                },
                StreamError {
                    record_index: Some(1),
                    error: FrontendError::PoisonedCommandBuffer(ErrorCode::InvalidCommand)
                }
            ]
        ));
        assert_eq!(s.pending_commands(), 0, "invalid command buffer is dropped");
        // An encoder left open is only detectable at the commit.
        let out = s.execute_stream(&stream(&[
            create_rt(2),
            begin_pass(2),
            Command::Commit { submission_id: 2 },
        ]));
        assert!(record_errors(&out).is_empty());
        assert_eq!(statuses(&out), vec![failed(2, ErrorCode::InvalidCommand)]);
        assert!(out.submissions.is_empty());
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
                stream_seq: 2,
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
        fail_submit: bool,
    }

    fn slow() -> SlowBackend {
        SlowBackend {
            inner: NullBackend::new(Capabilities::rtx4050_assumed()),
            done: SubmissionToken(0),
            fail_submit: false,
        }
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
        fn create_texture_view(
            &mut self,
            parent: TextureId,
            desc: &TextureViewDesc,
        ) -> Result<TextureId, GpuError> {
            self.inner.create_texture_view(parent, desc)
        }
        fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId, GpuError> {
            self.inner.create_sampler(desc)
        }
        fn destroy_sampler(&mut self, id: SamplerId) -> Result<(), GpuError> {
            self.inner.destroy_sampler(id)
        }
        fn create_depth_stencil_state(
            &mut self,
            desc: &DepthStencilDesc,
        ) -> Result<DepthStencilStateId, GpuError> {
            self.inner.create_depth_stencil_state(desc)
        }
        fn destroy_depth_stencil_state(&mut self, id: DepthStencilStateId) -> Result<(), GpuError> {
            self.inner.destroy_depth_stencil_state(id)
        }
        fn create_render_pipeline(
            &mut self,
            desc: &RenderPipelineDesc,
        ) -> Result<RenderPipelineId, GpuError> {
            self.inner.create_render_pipeline(desc)
        }
        fn destroy_render_pipeline(&mut self, id: RenderPipelineId) -> Result<(), GpuError> {
            self.inner.destroy_render_pipeline(id)
        }
        fn submit(&mut self, submission: &Submission) -> Result<SubmissionToken, GpuError> {
            if self.fail_submit {
                return Err(GpuError::Host("device lost (test)".into()));
            }
            self.inner.submit(submission)
        }
        fn completed(&self) -> SubmissionToken {
            self.done
        }
    }

    #[test]
    fn completions_are_reported_in_commit_order_including_errors() {
        let mut s = GuestSession::new(slow());
        handshake(&mut s);
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
        let mut s = GuestSession::new(slow());
        handshake(&mut s);
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
        let legacy = stream(&cmds);
        // The object-model commands of WP-GPU-24: resources, a full frame, teardown.
        let mut cmds = triangle_resources();
        cmds.extend(triangle_frame(2));
        cmds.push(Command::SetStoreAction(proto::SetStoreAction {
            attachment: proto::AttachmentKind::Color(0),
            store_action: 1,
        }));
        cmds.push(Command::DestroyTexture {
            texture_id: oid(13),
        });
        cmds.push(Command::DestroyRenderPipelineState {
            pipeline_id: oid(30),
        });
        let objects = stream(&cmds);
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
            for round in 0..3 {
                let mut b = if round == 0 {
                    legacy.clone()
                } else {
                    objects.clone()
                };
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
        let mut s = GuestSession::new(slow()).with_limits(limits);
        handshake(&mut s);
        s
    }

    #[test]
    fn command_buffer_membership_follows_opcode_class() {
        let legacy = [
            Opcode::CopyBuffer,
            Opcode::BeginRenderPass,
            Opcode::EndRenderPass,
            Opcode::Draw,
            Opcode::BeginRenderPass2,
            Opcode::SetStoreAction,
            Opcode::SetRenderPipelineState,
            Opcode::SetDepthStencilState,
            Opcode::SetVertexBuffer,
            Opcode::SetVertexTexture,
            Opcode::SetFragmentTexture,
            Opcode::SetVertexSamplerState,
            Opcode::SetFragmentSamplerState,
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
            bind_pipe(),
            draw(3), // third pending command: limit
            Command::EndRenderPass,
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
        let mut s = slow_session(Limits::DEFAULT);
        let mut cmds = vec![create_rt(2)];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::DestroyTexture { texture_id: oid(2) });
        let out = s.execute_stream(&stream(&cmds));
        assert!(out.replies.is_empty());
        assert_eq!(s.backend().inner.live_textures(), 1, "deferred");
        s.backend_mut().fail_submit = true;
        let out = s.execute_stream(&stream(&[Command::Commit { submission_id: 1 }]));
        assert_eq!(statuses(&out), vec![failed(1, ErrorCode::HostError)]);
        assert_eq!(
            s.backend().inner.live_textures(),
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
        handshake(&mut s);
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
                stream_seq: 3,
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

    // ---- object model (WP-GPU-02) -------------------------------------------------------

    fn create_tex(id: u32, format: PixelFormat, usage: TextureUsage, w: u32) -> Command<'static> {
        Command::CreateTexture(CreateTexture {
            texture_id: oid(id),
            pixel_format: format.raw() as u16,
            usage: usage.bits() as u16,
            texture_type: TextureType::Type2D.raw() as u8,
            storage_mode: StorageMode::Private.raw() as u8,
            sample_count: 1,
            width: w,
            height: 64,
            depth: 1,
            array_length: 1,
            mip_level_count: 1,
            swizzle: proto::SWIZZLE_IDENTITY,
        })
    }

    #[test]
    fn formats_without_gpu_support_are_unimplemented_not_misrendered() {
        let astc = create_tex(1, PixelFormat::ASTC_4x4_LDR, TextureUsage::SHADER_READ, 64);
        // Native only: no ASTC hardware → not implemented, nothing created.
        let mut s = session();
        let out = s.execute_stream(&stream(&[astc]));
        let errs = record_errors(&out);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code, ErrorCode::Unimplemented);
        assert_eq!(s.backend().live_textures(), 0);
        // With transcoding enabled the texture exists.
        let mut s = GuestSession::with_policy(
            NullBackend::new(Capabilities::rtx4050_assumed()),
            &EmulationPolicy::ALL,
        );
        s.execute_stream(&hello(1, 1, Features::empty()));
        let out = s.execute_stream(&stream(&[astc]));
        assert!(record_errors(&out).is_empty(), "{:?}", out.replies);
        assert_eq!(s.backend().live_textures(), 1);
        // sRGB storage writes have no emulation yet: unimplemented even with ALL.
        let srgb_write = create_tex(
            2,
            PixelFormat::RGBA8Unorm_sRGB,
            TextureUsage::SHADER_WRITE,
            64,
        );
        let out = s.execute_stream(&stream(&[srgb_write]));
        assert_eq!(record_errors(&out)[0].code, ErrorCode::Unimplemented);
        // Invalid Metal stays an invalid argument (compressed render target).
        let bad = create_tex(3, PixelFormat::BC1_RGBA, TextureUsage::RENDER_TARGET, 64);
        let out = s.execute_stream(&stream(&[bad]));
        assert_eq!(record_errors(&out)[0].code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn textures_are_checked_against_reported_limits() {
        // The RTX allows 32768-wide textures, Apple7 (and thus the guest) only 16384.
        let mut s = session();
        let out = s.execute_stream(&stream(&[create_tex(
            1,
            PixelFormat::RGBA8Unorm,
            TextureUsage::SHADER_READ,
            16385,
        )]));
        assert_eq!(record_errors(&out)[0].code, ErrorCode::InvalidArgument);
        let out = s.execute_stream(&stream(&[create_tex(
            1,
            PixelFormat::RGBA8Unorm,
            TextureUsage::SHADER_READ,
            16384,
        )]));
        assert!(record_errors(&out).is_empty());
    }

    #[test]
    fn resolve_store_action_without_resolve_target_poisons_the_command_buffer() {
        let mut s = session();
        let resolve_pass = Command::BeginRenderPass(
            BeginRenderPass::new(
                640,
                480,
                &[ColorAttachmentRecord {
                    texture_id: oid(1),
                    load_action: 2,
                    store_action: StoreAction::MultisampleResolve.raw() as u8,
                    clear_color: [0.0; 4],
                }],
            )
            .unwrap(),
        );
        let out = s.execute_stream(&stream(&[
            create_rt(1),
            resolve_pass,
            draw(3),
            Command::EndRenderPass,
            Command::Commit { submission_id: 5 },
        ]));
        // The compact pass has no resolve texture, so the pass is rejected when recorded and
        // the whole command buffer fails instead of rendering without the resolve.
        wire(&out);
        assert_eq!(
            record_errors(&out)
                .iter()
                .map(|e| (e.record_index, e.code))
                .collect::<Vec<_>>(),
            vec![(1, ErrorCode::InvalidArgument)]
        );
        assert_eq!(statuses(&out), vec![failed(5, ErrorCode::InvalidArgument)]);
        // Unknown store actions are still rejected at decode-to-model time.
        let mut bad = begin_pass(1);
        if let Command::BeginRenderPass(rp) = &mut bad {
            *rp = BeginRenderPass::new(
                640,
                480,
                &[ColorAttachmentRecord {
                    texture_id: oid(1),
                    load_action: 2,
                    store_action: 6,
                    clear_color: [0.0; 4],
                }],
            )
            .unwrap();
        }
        let out = s.execute_stream(&stream(&[bad]));
        assert_eq!(record_errors(&out)[0].code, ErrorCode::InvalidArgument);
    }

    // ---- protocol commands for the object model (WP-GPU-24) -------------------------------

    #[allow(clippy::too_many_arguments)]
    fn create_texture(
        id: u32,
        format: PixelFormat,
        texture_type: TextureType,
        samples: u8,
        storage: StorageMode,
        usage: TextureUsage,
        w: u32,
        h: u32,
    ) -> Command<'static> {
        Command::CreateTexture(CreateTexture {
            texture_id: oid(id),
            pixel_format: format.raw() as u16,
            usage: usage.bits() as u16,
            texture_type: texture_type.raw() as u8,
            storage_mode: storage.raw() as u8,
            sample_count: samples,
            width: w,
            height: h,
            depth: 1,
            array_length: 1,
            mip_level_count: 1,
            swizzle: proto::SWIZZLE_IDENTITY,
        })
    }

    fn rt_usage() -> TextureUsage {
        TextureUsage::RENDER_TARGET
    }

    fn msaa(id: u32, format: PixelFormat) -> Command<'static> {
        create_texture(
            id,
            format,
            TextureType::Type2DMultisample,
            4,
            StorageMode::Memoryless,
            rt_usage(),
            640,
            480,
        )
    }

    fn single(id: u32, format: PixelFormat, usage: TextureUsage) -> Command<'static> {
        create_texture(
            id,
            format,
            TextureType::Type2D,
            1,
            StorageMode::Private,
            usage,
            640,
            480,
        )
    }

    fn att(tex: u32, load: LoadAction, store: StoreAction) -> proto::AttachmentRecord {
        proto::AttachmentRecord::new(oid(tex), load.raw() as u8, store.raw() as u8)
    }

    fn resolving(mut a: proto::AttachmentRecord, target: u32) -> proto::AttachmentRecord {
        a.resolve = Some(proto::ResolveRecord {
            texture_id: oid(target),
            level: 0,
            slice: 0,
            depth_plane: 0,
        });
        a
    }

    fn color_att(index: u8, a: proto::AttachmentRecord) -> proto::ColorAttachment2 {
        proto::ColorAttachment2 {
            index,
            attachment: a,
            clear_color: [0.0, 0.0, 0.0, 1.0],
        }
    }

    fn depth_att(a: proto::AttachmentRecord) -> proto::DepthAttachment2 {
        proto::DepthAttachment2 {
            attachment: a,
            resolve_filter: 0,
            clear_depth: 1.0,
        }
    }

    fn stencil_att(a: proto::AttachmentRecord) -> proto::StencilAttachment2 {
        proto::StencilAttachment2 {
            attachment: a,
            resolve_filter: 0,
            clear_stencil: 0,
        }
    }

    fn pass2(
        colors: &[proto::ColorAttachment2],
        depth: Option<proto::DepthAttachment2>,
        stencil: Option<proto::StencilAttachment2>,
    ) -> Command<'static> {
        Command::BeginRenderPass2(proto::BeginRenderPass2 {
            render_target_width: 0,
            render_target_height: 0,
            render_target_array_length: 0,
            default_raster_sample_count: 0,
            color_attachments: list(colors),
            depth_attachment: depth,
            stencil_attachment: stencil,
        })
    }

    fn binding(id: Option<u32>, index: u32) -> proto::Binding {
        proto::Binding {
            object_id: id.map(oid),
            index,
        }
    }

    fn sampler_cmd(id: u32) -> proto::CreateSampler {
        proto::CreateSampler {
            sampler_id: oid(id),
            min_filter: 1,
            mag_filter: 1,
            mip_filter: 0,
            address_modes: [2, 2, 2],
            border_color: 0,
            reduction_mode: 0,
            compare_function: 0,
            flags: proto::CreateSampler::NORMALIZED_COORDINATES,
            max_anisotropy: 1,
            lod_min_clamp: 0.0,
            lod_max_clamp: f32::MAX,
            lod_bias: 0.0,
        }
    }

    fn dss_cmd(id: u32) -> proto::CreateDepthStencilState {
        proto::CreateDepthStencilState {
            dss_id: oid(id),
            depth_compare: CompareFunction::Less.raw() as u8,
            depth_write_enabled: true,
            front_face_stencil: None,
            back_face_stencil: None,
        }
    }

    /// The pipeline of the triangle scene: blended BGRA8, Depth32Float, 4× MSAA, triangles,
    /// position (float3) + colour (uchar4 BGRA) in one interleaved buffer.
    fn triangle_pipeline(id: u32) -> Command<'static> {
        let mut blend =
            proto::PipelineColorRecord::with_format(PixelFormat::BGRA8Unorm.raw() as u16);
        blend.blending_enabled = true;
        blend.source_rgb_blend_factor = BlendFactor::SourceAlpha.raw() as u8;
        blend.destination_rgb_blend_factor = BlendFactor::OneMinusSourceAlpha.raw() as u8;
        Command::CreateRenderPipelineState(proto::CreateRenderPipelineState {
            pipeline_id: oid(id),
            vertex_function: None,
            fragment_function: None,
            depth_pixel_format: PixelFormat::Depth32Float.raw() as u16,
            stencil_pixel_format: 0,
            raster_sample_count: 4,
            input_primitive_topology: PrimitiveTopologyClass::Triangle.raw() as u8,
            flags: proto::CreateRenderPipelineState::RASTERIZATION_ENABLED
                | proto::CreateRenderPipelineState::VERTEX_DESCRIPTOR,
            max_vertex_amplification_count: 1,
            color_attachments: list(&[blend]),
            vertex_attributes: list(&[
                proto::VertexAttributeRecord {
                    index: 0,
                    format: crate::model::VertexFormat::Float3.raw() as u16,
                    buffer_index: 0,
                    offset: 0,
                },
                proto::VertexAttributeRecord {
                    index: 1,
                    format: crate::model::VertexFormat::UChar4Normalized_BGRA.raw() as u16,
                    buffer_index: 0,
                    offset: 12,
                },
            ]),
            vertex_layouts: list(&[proto::VertexLayoutRecord {
                index: 0,
                step_function: VertexStepFunction::PerVertex.raw() as u8,
                stride: 16,
                step_rate: 1,
            }]),
        })
    }

    /// Resources of the triangle scene (ids: 1 vertex buffer, 10 MSAA colour, 11 resolve
    /// target, 12 MSAA depth, 13 sampled texture, 14 swizzled sRGB view of 13, 20 sampler,
    /// 21 depth/stencil state, 30 pipeline).
    fn triangle_resources() -> Vec<Command<'static>> {
        vec![
            create_buf(1, 3 * 16),
            Command::WriteBuffer(WriteBuffer {
                buffer_id: oid(1),
                offset: 0,
                data: &[0x3f; 48],
            }),
            msaa(10, PixelFormat::BGRA8Unorm),
            single(
                11,
                PixelFormat::BGRA8Unorm,
                TextureUsage::RENDER_TARGET.union(TextureUsage::SHADER_READ),
            ),
            msaa(12, PixelFormat::Depth32Float),
            single(13, PixelFormat::RGBA8Unorm, TextureUsage::SHADER_READ),
            Command::CreateTextureView(proto::CreateTextureView {
                texture_id: oid(14),
                parent_id: oid(13),
                pixel_format: PixelFormat::RGBA8Unorm_sRGB.raw() as u16,
                texture_type: TextureType::Type2D.raw() as u8,
                swizzle: [4, 3, 2, 5], // BGRA
                level_start: 0,
                level_count: 1,
                slice_start: 0,
                slice_count: 1,
            }),
            Command::CreateSampler(sampler_cmd(20)),
            Command::CreateDepthStencilState(dss_cmd(21)),
            triangle_pipeline(30),
        ]
    }

    /// One frame of the triangle scene: MSAA colour resolved into 11, MSAA depth, binds,
    /// draw.
    fn triangle_frame(submission_id: u64) -> Vec<Command<'static>> {
        let color = resolving(
            att(10, LoadAction::Clear, StoreAction::MultisampleResolve),
            11,
        );
        vec![
            pass2(
                &[color_att(0, color)],
                Some(depth_att(att(12, LoadAction::Clear, StoreAction::DontCare))),
                None,
            ),
            Command::SetRenderPipelineState {
                pipeline_id: oid(30),
            },
            Command::SetDepthStencilState {
                dss_id: Some(oid(21)),
            },
            Command::SetVertexBuffer(proto::SetVertexBuffer {
                buffer_id: Some(oid(1)),
                index: 0,
                offset: 0,
                attribute_stride: proto::SetVertexBuffer::STRIDE_STATIC,
            }),
            Command::SetFragmentTexture(binding(Some(14), 0)),
            Command::SetFragmentSamplerState(binding(Some(20), 0)),
            Command::SetVertexTexture(binding(None, 0)),
            Command::SetVertexSamplerState(binding(None, 0)),
            draw(PrimitiveType::Triangle.raw() as u8),
            Command::EndRenderPass,
            Command::Commit { submission_id },
        ]
    }

    /// Acceptance test WP-GPU-24: HELLO → textures (colour, depth, resolve) → sampler →
    /// depth/stencil state → render pipeline → BeginRenderPass2 (colour + depth + MSAA
    /// resolve) → binds → draw → end → commit → `Completed`, on both target profiles, every
    /// reply through the wire.
    #[test]
    fn triangle_setup_end_to_end() {
        for caps in [
            Capabilities::rtx4050_assumed(),
            Capabilities::iris_xe_assumed(),
        ] {
            let mut s = GuestSession::new(NullBackend::new(caps));
            let out = s.execute_stream(&hello(1, 1, Features::empty()));
            assert!(matches!(wire(&out)[..], [Reply::HelloAck(_)]));
            let resources = triangle_resources();
            let out = s.execute_stream(&stream(&resources));
            assert_eq!(wire(&out), vec![], "{:?}", out.errors);
            assert_eq!(out.records_executed, resources.len());
            let b = s.backend();
            assert_eq!(
                (
                    b.live_buffers(),
                    b.live_textures(),
                    b.live_samplers(),
                    b.live_depth_stencil_states(),
                    b.live_render_pipelines()
                ),
                (1, 5, 1, 1, 1)
            );
            // The view's swizzle is composed with the (identity) parent swizzle.
            let view = s.texture_desc(s.texture(oid(14)).unwrap()).unwrap();
            assert_eq!(view.swizzle.raw(), [4, 3, 2, 5]);
            assert_eq!(view.pixel_format, PixelFormat::RGBA8Unorm_sRGB);

            let frame = triangle_frame(77);
            let out = s.execute_stream(&stream(&frame));
            assert_eq!(
                wire(&out),
                vec![Reply::CommandBufferStatus(completed(77))],
                "{:?}",
                out.errors
            );
            assert_eq!(out.records_executed, frame.len());
            assert_eq!(out.submissions, vec![(77, SubmissionToken(1))]);
            assert_eq!(s.backend().commands_executed(), (frame.len() - 1) as u64);
            assert!(!s.command_buffer_poisoned());

            // Tear down: destroys inside a recorded frame are deferred to its commit.
            let mut teardown = triangle_frame(78);
            teardown.insert(
                teardown.len() - 1,
                Command::DestroyRenderPipelineState {
                    pipeline_id: oid(30),
                },
            );
            for (i, d) in [
                Command::DestroyDepthStencilState { dss_id: oid(21) },
                Command::DestroySampler {
                    sampler_id: oid(20),
                },
                Command::DestroyTexture {
                    texture_id: oid(13),
                },
                Command::DestroyTexture {
                    texture_id: oid(14),
                },
                Command::DestroyTexture {
                    texture_id: oid(10),
                },
                Command::DestroyTexture {
                    texture_id: oid(11),
                },
                Command::DestroyTexture {
                    texture_id: oid(12),
                },
                Command::DestroyBuffer { buffer_id: oid(1) },
            ]
            .into_iter()
            .enumerate()
            {
                teardown.insert(teardown.len() - 1 - i, d);
            }
            let out = s.execute_stream(&stream(&teardown));
            assert_eq!(wire(&out), vec![Reply::CommandBufferStatus(completed(78))]);
            let b = s.backend();
            assert_eq!(
                (
                    b.live_buffers(),
                    b.live_textures(),
                    b.live_samplers(),
                    b.live_depth_stencil_states(),
                    b.live_render_pipelines()
                ),
                (0, 0, 0, 0, 0)
            );
            assert_eq!(b.allocated_bytes(), 0);
        }
    }

    /// Every bind (and the other render-encoder commands) outside a render pass poisons the
    /// command buffer: one `RecordError` (`InvalidCommand`), later encoder records skipped,
    /// the commit fails without reaching the backend.
    #[test]
    fn bind_outside_a_render_pass_poisons() {
        let outside: Vec<Command<'static>> = vec![
            bind_pipe(),
            Command::SetDepthStencilState {
                dss_id: Some(oid(21)),
            },
            Command::SetVertexBuffer(proto::SetVertexBuffer {
                buffer_id: Some(oid(1)),
                index: 0,
                offset: 0,
                attribute_stride: proto::SetVertexBuffer::STRIDE_STATIC,
            }),
            Command::SetVertexTexture(binding(Some(13), 0)),
            Command::SetFragmentTexture(binding(Some(13), 0)),
            Command::SetVertexSamplerState(binding(Some(20), 0)),
            Command::SetFragmentSamplerState(binding(None, 0)),
            Command::SetStoreAction(proto::SetStoreAction {
                attachment: proto::AttachmentKind::Color(0),
                store_action: StoreAction::Store.raw() as u8,
            }),
            Command::EndRenderPass,
        ];
        for bad in outside {
            let mut s = session();
            assert!(record_errors(&s.execute_stream(&stream(&triangle_resources()))).is_empty());
            let mut cmds = vec![bad];
            // A complete, valid frame after the bad record is skipped as a whole.
            cmds.extend(triangle_frame(5));
            let out = s.execute_stream(&stream(&cmds));
            let errs = record_errors(&out);
            assert_eq!(
                errs.iter()
                    .map(|e| (e.record_index, e.opcode, e.code))
                    .collect::<Vec<_>>(),
                vec![(0, bad.opcode().raw(), ErrorCode::InvalidCommand)],
                "{bad:?}"
            );
            assert_eq!(statuses(&out), vec![failed(5, ErrorCode::InvalidCommand)]);
            assert_eq!(out.records_executed, 1, "only the commit ran");
            assert!(out.submissions.is_empty());
            assert_eq!(s.backend().commands_executed(), 0);
            // The next command buffer is clean.
            let out = s.execute_stream(&stream(&triangle_frame(6)));
            assert_eq!(statuses(&out), vec![completed(6)], "{bad:?}");
        }
    }

    /// Depth and stencil of one combined texture resolving into different textures is valid
    /// Metal that Vulkan dynamic rendering cannot express: `Unimplemented`, never a partial
    /// resolve.
    #[test]
    fn depth_stencil_resolve_conflict_is_unimplemented() {
        for caps in [
            Capabilities::rtx4050_assumed(),
            Capabilities::iris_xe_assumed(),
        ] {
            let mut s = GuestSession::new(NullBackend::new(caps));
            handshake(&mut s);
            let d32s8 = PixelFormat::Depth32Float_Stencil8;
            let mut cmds = vec![
                msaa(10, PixelFormat::BGRA8Unorm),
                single(11, PixelFormat::BGRA8Unorm, rt_usage()),
                create_texture(
                    40,
                    d32s8,
                    TextureType::Type2DMultisample,
                    4,
                    StorageMode::Private,
                    rt_usage(),
                    640,
                    480,
                ),
                single(41, d32s8, rt_usage()),
                single(42, d32s8, rt_usage()),
            ];
            let color = color_att(
                0,
                resolving(
                    att(10, LoadAction::Clear, StoreAction::MultisampleResolve),
                    11,
                ),
            );
            let ds = |target| {
                resolving(
                    att(40, LoadAction::Clear, StoreAction::MultisampleResolve),
                    target,
                )
            };
            cmds.push(pass2(
                &[color],
                Some(depth_att(ds(41))),
                Some(stencil_att(ds(42))),
            ));
            cmds.push(Command::EndRenderPass);
            cmds.push(Command::Commit { submission_id: 3 });
            let out = s.execute_stream(&stream(&cmds));
            assert_eq!(
                record_errors(&out)
                    .iter()
                    .map(|e| (e.record_index, e.code))
                    .collect::<Vec<_>>(),
                vec![(5, ErrorCode::Unimplemented)],
                "{:?}",
                out.errors
            );
            assert_eq!(statuses(&out), vec![failed(3, ErrorCode::Unimplemented)]);
            assert_eq!(s.backend().commands_executed(), 0);
            // The same resolve into one target is expressible.
            let ok = vec![
                pass2(&[color], Some(depth_att(ds(41))), Some(stencil_att(ds(41)))),
                Command::EndRenderPass,
                Command::Commit { submission_id: 4 },
            ];
            let out = s.execute_stream(&stream(&ok));
            assert_eq!(wire(&out), vec![Reply::CommandBufferStatus(completed(4))]);
        }
    }

    #[test]
    fn deferred_store_action_is_set_by_the_protocol() {
        let mut s = session();
        assert!(record_errors(&s.execute_stream(&stream(&triangle_resources()))).is_empty());
        let unknown = |submission_id, extra: Vec<Command<'static>>| {
            let mut frame = triangle_frame(submission_id);
            frame[0] = pass2(
                &[color_att(
                    0,
                    resolving(att(10, LoadAction::Clear, StoreAction::Unknown), 11),
                )],
                Some(depth_att(att(12, LoadAction::Clear, StoreAction::DontCare))),
                None,
            );
            let end = frame.len() - 2;
            for (i, c) in extra.into_iter().enumerate() {
                frame.insert(end + i, c);
            }
            frame
        };
        let set = |kind, action: StoreAction| {
            Command::SetStoreAction(proto::SetStoreAction {
                attachment: kind,
                store_action: action.raw() as u8,
            })
        };
        let color0 = proto::AttachmentKind::Color(0);
        // Set before the end: the command buffer completes.
        let out = s.execute_stream(&stream(&unknown(
            1,
            vec![set(color0, StoreAction::MultisampleResolve)],
        )));
        assert_eq!(wire(&out), vec![Reply::CommandBufferStatus(completed(1))]);
        // Never set: the end of the encoder is the failing record.
        let frame = unknown(2, vec![]);
        let end = frame.len() - 2;
        let out = s.execute_stream(&stream(&frame));
        assert_eq!(
            record_errors(&out)
                .iter()
                .map(|e| (e.record_index as usize, e.code))
                .collect::<Vec<_>>(),
            vec![(end, ErrorCode::InvalidCommand)]
        );
        assert_eq!(statuses(&out), vec![failed(2, ErrorCode::InvalidCommand)]);
        // Wrong uses: known action, empty slot, unknown raw value, unsupported action.
        for (cmd, code) in [
            // The depth store action is known (DontCare): only `Unknown` may be replaced.
            (
                set(proto::AttachmentKind::Depth, StoreAction::Store),
                ErrorCode::InvalidCommand,
            ),
            (
                set(proto::AttachmentKind::Color(1), StoreAction::Store),
                ErrorCode::InvalidArgument,
            ),
            (
                set(proto::AttachmentKind::Stencil, StoreAction::Store),
                ErrorCode::InvalidArgument,
            ),
            (
                Command::SetStoreAction(proto::SetStoreAction {
                    attachment: color0,
                    store_action: 9,
                }),
                ErrorCode::InvalidArgument,
            ),
            // Stores into the memoryless MSAA texture: invalid before it is unsupported.
            (
                set(color0, StoreAction::CustomSampleDepthStore),
                ErrorCode::InvalidArgument,
            ),
            (
                set(color0, StoreAction::Unknown),
                ErrorCode::InvalidArgument,
            ),
        ] {
            let out = s.execute_stream(&stream(&unknown(3, vec![cmd])));
            assert_eq!(
                record_errors(&out).first().map(|e| (e.opcode, e.code)),
                Some((Opcode::SetStoreAction.raw(), code)),
                "{cmd:?}"
            );
            assert_eq!(statuses(&out), vec![failed(3, code)], "{cmd:?}");
        }
        // A memoryless attachment cannot be stored, even when the action is set late.
        let mut frame = unknown(4, vec![]);
        frame[0] = pass2(
            &[color_att(
                0,
                resolving(
                    att(10, LoadAction::Clear, StoreAction::MultisampleResolve),
                    11,
                ),
            )],
            Some(depth_att(att(12, LoadAction::Clear, StoreAction::Unknown))),
            None,
        );
        let end = frame.len() - 2;
        frame.insert(end, set(proto::AttachmentKind::Depth, StoreAction::Store));
        let out = s.execute_stream(&stream(&frame));
        assert_eq!(
            record_errors(&out)
                .first()
                .map(|e| (e.record_index as usize, e.code)),
            Some((end, ErrorCode::InvalidArgument))
        );
    }

    #[test]
    fn pipeline_must_match_the_pass_and_the_draw() {
        let mut s = session();
        assert!(record_errors(&s.execute_stream(&stream(&triangle_resources()))).is_empty());
        // Default pipeline: BGRA8, one sample, no depth -> mismatch with the MSAA pass.
        let mut frame = triangle_frame(1);
        frame[1] = bind_pipe();
        let out = s.execute_stream(&stream(&frame));
        assert_eq!(
            record_errors(&out)
                .first()
                .map(|e| (e.record_index, e.code)),
            Some((1, ErrorCode::InvalidArgument))
        );
        // Lines with a triangle pipeline.
        let mut frame = triangle_frame(2);
        let draw_at = frame.len() - 3;
        frame[draw_at] = draw(PrimitiveType::LineStrip.raw() as u8);
        let out = s.execute_stream(&stream(&frame));
        assert_eq!(
            record_errors(&out)
                .first()
                .map(|e| (e.record_index as usize, e.code)),
            Some((draw_at, ErrorCode::InvalidArgument))
        );
        // No pipeline at all.
        let mut frame = triangle_frame(3);
        frame.remove(1);
        let out = s.execute_stream(&stream(&frame));
        assert_eq!(
            record_errors(&out).first().map(|e| e.code),
            Some(ErrorCode::InvalidCommand)
        );
        // The vertex descriptor fetches from buffer 0, which is not bound.
        let mut frame = triangle_frame(5);
        assert!(matches!(frame.remove(3), Command::SetVertexBuffer(_)));
        let draw_at = frame.len() - 3;
        let out = s.execute_stream(&stream(&frame));
        assert_eq!(
            record_errors(&out)
                .first()
                .map(|e| (e.record_index as usize, e.code)),
            Some((draw_at, ErrorCode::InvalidCommand))
        );
        // Unknown pipeline id.
        let mut frame = triangle_frame(4);
        frame[1] = Command::SetRenderPipelineState {
            pipeline_id: oid(31),
        };
        let out = s.execute_stream(&stream(&frame));
        let e = record_errors(&out)[0];
        assert_eq!(
            (e.code, e.object_id),
            (ErrorCode::UnknownObject, Some(oid(31)))
        );
    }

    #[test]
    fn bind_arguments_are_validated() {
        let mut s = session();
        assert!(record_errors(&s.execute_stream(&stream(&triangle_resources()))).is_empty());
        let vb = |id, index, offset| {
            Command::SetVertexBuffer(proto::SetVertexBuffer {
                buffer_id: Some(oid(id)),
                index,
                offset,
                attribute_stride: 4,
            })
        };
        for (cmd, code) in [
            (
                Command::SetFragmentTexture(binding(Some(13), 128)),
                ErrorCode::InvalidArgument,
            ),
            (
                Command::SetVertexTexture(binding(Some(99), 0)),
                ErrorCode::UnknownObject,
            ),
            (
                Command::SetFragmentSamplerState(binding(Some(20), 16)),
                ErrorCode::InvalidArgument,
            ),
            (
                Command::SetVertexSamplerState(binding(Some(13), 0)),
                ErrorCode::UnknownObject,
            ),
            (vb(1, 31, 0), ErrorCode::InvalidArgument),
            (vb(1, 0, 48), ErrorCode::InvalidArgument),
            (vb(2, 0, 0), ErrorCode::UnknownObject),
            (
                Command::SetDepthStencilState {
                    dss_id: Some(oid(20)),
                },
                ErrorCode::UnknownObject,
            ),
        ] {
            let mut frame = triangle_frame(1);
            frame.insert(2, cmd);
            let out = s.execute_stream(&stream(&frame));
            assert_eq!(
                record_errors(&out)
                    .iter()
                    .map(|e| (e.record_index, e.code))
                    .collect::<Vec<_>>(),
                vec![(2, code)],
                "{cmd:?}"
            );
            assert_eq!(statuses(&out), vec![failed(1, code)]);
        }
        // Highest valid indices and an explicit stride pass.
        let mut frame = triangle_frame(2);
        frame.insert(2, Command::SetFragmentTexture(binding(Some(11), 127)));
        frame.insert(2, Command::SetVertexSamplerState(binding(Some(20), 15)));
        frame.insert(2, vb(1, 30, 47));
        let out = s.execute_stream(&stream(&frame));
        assert_eq!(statuses(&out), vec![completed(2)], "{:?}", out.errors);
    }

    #[test]
    fn texture_views_compose_swizzles_and_keep_their_parent() {
        let mut s = session();
        let mut parent = single(
            50,
            PixelFormat::RGBA8Unorm,
            TextureUsage::SHADER_READ.union(TextureUsage::RENDER_TARGET),
        );
        if let Command::CreateTexture(t) = &mut parent {
            t.swizzle = [2, 1, 1, 3]; // [R, 1, 1, G]
        }
        let view = |id, parent, format: PixelFormat, swizzle| {
            Command::CreateTextureView(proto::CreateTextureView {
                texture_id: oid(id),
                parent_id: oid(parent),
                pixel_format: format.raw() as u16,
                texture_type: TextureType::Type2D.raw() as u8,
                swizzle,
                level_start: 0,
                level_count: 1,
                slice_start: 0,
                slice_count: 1,
            })
        };
        let out = s.execute_stream(&stream(&[
            parent,
            view(51, 50, PixelFormat::RGBA8Unorm, [2, 3, 5, 4]), // [R, G, A, B]
            view(52, 51, PixelFormat::RGBA8Unorm_sRGB, [2, 3, 4, 5]), // view of a view
        ]));
        assert!(record_errors(&out).is_empty(), "{:?}", out.errors);
        let desc = |s: &GuestSession<NullBackend>, id| {
            s.texture_desc(s.texture(oid(id)).unwrap()).unwrap()
        };
        // Apple's example: [R,G,A,B] on [R,1,1,G] gives [R,1,G,1].
        assert_eq!(desc(&s, 51).swizzle.raw(), [2, 1, 3, 1]);
        assert_eq!(desc(&s, 52).swizzle.raw(), [2, 1, 3, 1]);
        assert_eq!(s.backend().live_textures(), 3);
        assert_eq!(
            s.backend().allocated_bytes(),
            640 * 480 * 4,
            "views share storage"
        );

        // Errors: unknown parent, incompatible format, bad swizzle, out-of-range levels,
        // duplicate id.
        let mut out_of_range = view(60, 50, PixelFormat::RGBA8Unorm, [2, 3, 4, 5]);
        if let Command::CreateTextureView(v) = &mut out_of_range {
            v.level_start = 1;
        }
        for (cmd, code) in [
            (
                view(60, 49, PixelFormat::RGBA8Unorm, [2, 3, 4, 5]),
                ErrorCode::UnknownObject,
            ),
            (
                view(60, 50, PixelFormat::RG16Float, [2, 3, 4, 5]),
                ErrorCode::InvalidArgument,
            ),
            (
                view(60, 50, PixelFormat::RGBA8Unorm, [2, 3, 4, 6]),
                ErrorCode::InvalidArgument,
            ),
            (out_of_range, ErrorCode::InvalidArgument),
            (
                view(51, 50, PixelFormat::RGBA8Unorm, [2, 3, 4, 5]),
                ErrorCode::DuplicateObject,
            ),
        ] {
            let out = s.execute_stream(&stream(&[cmd]));
            assert_eq!(record_errors(&out)[0].code, code, "{cmd:?}");
            assert!(
                !s.command_buffer_poisoned(),
                "resource errors do not poison"
            );
        }

        // Destroying the parent keeps the storage alive for the views; the views stay
        // usable as render targets and the parent's name is free again.
        s.execute_stream(&stream(&[Command::DestroyTexture {
            texture_id: oid(50),
        }]));
        assert_eq!(s.backend().live_textures(), 3);
        assert_eq!(s.backend().allocated_bytes(), 640 * 480 * 4);
        let pipe = create_pipeline(
            70,
            PixelFormat::RGBA8Unorm_sRGB,
            None,
            1,
            PrimitiveTopologyClass::Unspecified,
        );
        let out = s.execute_stream(&stream(&[
            pipe,
            pass2(
                &[color_att(0, att(52, LoadAction::Clear, StoreAction::Store))],
                None,
                None,
            ),
            Command::SetRenderPipelineState {
                pipeline_id: oid(70),
            },
            draw(3),
            Command::EndRenderPass,
            Command::Commit { submission_id: 1 },
        ]));
        assert_eq!(wire(&out), vec![Reply::CommandBufferStatus(completed(1))]);
        s.execute_stream(&stream(&[
            Command::DestroyTexture {
                texture_id: oid(52),
            },
            Command::DestroyTexture {
                texture_id: oid(51),
            },
        ]));
        assert_eq!(
            s.backend().live_textures(),
            0,
            "last view released the parent"
        );
        assert_eq!(s.backend().allocated_bytes(), 0);
    }

    #[test]
    fn new_object_kinds_respect_limits_including_deferred_destroys() {
        type Make = fn(u32) -> Command<'static>;
        type Kill = fn(u32) -> Command<'static>;
        let kinds: [(Make, Kill); 4] = [
            (
                |id| Command::CreateSampler(sampler_cmd(id)),
                |id| Command::DestroySampler {
                    sampler_id: oid(id),
                },
            ),
            (
                |id| Command::CreateDepthStencilState(dss_cmd(id)),
                |id| Command::DestroyDepthStencilState { dss_id: oid(id) },
            ),
            (
                |id| {
                    create_pipeline(
                        id,
                        PixelFormat::RGBA8Unorm,
                        None,
                        1,
                        PrimitiveTopologyClass::Unspecified,
                    )
                },
                |id| Command::DestroyRenderPipelineState {
                    pipeline_id: oid(id),
                },
            ),
            (
                |id| {
                    Command::CreateTextureView(proto::CreateTextureView {
                        texture_id: oid(id),
                        parent_id: oid(2),
                        pixel_format: PixelFormat::BGRA8Unorm_sRGB.raw() as u16,
                        texture_type: TextureType::Type2D.raw() as u8,
                        swizzle: proto::SWIZZLE_IDENTITY,
                        level_start: 0,
                        level_count: 1,
                        slice_start: 0,
                        slice_count: 1,
                    })
                },
                |id| Command::DestroyTexture {
                    texture_id: oid(id),
                },
            ),
        ];
        for (make, kill) in kinds {
            // Limit 3 per kind. The default pipeline (PIPE) and texture 2 (parent of the views)
            // already take one slot of their kinds; samplers and depth/stencil states get one
            // object (101) so that every kind starts with one live object.
            let mut s = small(Limits {
                objects_per_kind: 3,
                ..Limits::DEFAULT
            });
            let mut setup = vec![create_rt(2)];
            if matches!(
                make(101),
                Command::CreateSampler(_) | Command::CreateDepthStencilState(_)
            ) {
                setup.push(make(101));
            }
            assert!(record_errors(&s.execute_stream(&stream(&setup))).is_empty());
            let mut cmds = vec![make(100)]; // 2 live
            cmds.extend(triangle_pass(2)); // pending work: destroys are deferred now
            cmds.push(kill(100)); // 1 live + 1 deferred
            cmds.push(make(102)); // 2 live + 1 deferred
            cmds.push(make(103)); // over the limit
            let out = s.execute_stream(&stream(&cmds));
            assert_eq!(
                record_errors(&out)
                    .iter()
                    .map(|e| (e.record_index as usize, e.code))
                    .collect::<Vec<_>>(),
                vec![(cmds.len() - 1, ErrorCode::LimitExceeded)],
                "{:?}",
                out.errors
            );
            assert!(
                !s.command_buffer_poisoned(),
                "resource errors do not poison"
            );
            // The commit releases the deferred object; the slot can be used again.
            let out = s.execute_stream(&stream(&[
                Command::Commit { submission_id: 1 },
                make(103),
                make(103),
            ]));
            assert_eq!(statuses(&out), vec![completed(1)]);
            assert_eq!(
                record_errors(&out)
                    .iter()
                    .map(|e| e.code)
                    .collect::<Vec<_>>(),
                vec![ErrorCode::DuplicateObject]
            );
        }
    }

    #[test]
    fn object_descriptors_are_validated_before_the_backend() {
        let mut s = session();
        let mut bad_aniso = sampler_cmd(1);
        bad_aniso.max_anisotropy = 0;
        let mut bias = sampler_cmd(2);
        bias.lod_bias = 1.0;
        let mut bad_mode = sampler_cmd(3);
        bad_mode.address_modes[2] = 6;
        let mut bad_compare = dss_cmd(4);
        bad_compare.depth_compare = 8;
        let mut bad_stencil = dss_cmd(5);
        bad_stencil.front_face_stencil = Some(proto::StencilRecord {
            compare: 7,
            stencil_failure: 8,
            depth_failure: 0,
            depth_stencil_pass: 0,
            read_mask: 0xff,
            write_mask: 0xff,
        });
        let mut with_fn = triangle_pipeline(6);
        if let Command::CreateRenderPipelineState(p) = &mut with_fn {
            p.fragment_function = Some(oid(1));
        }
        let mut msaa8 = triangle_pipeline(7);
        if let Command::CreateRenderPipelineState(p) = &mut msaa8 {
            p.raster_sample_count = 8;
        }
        let mut blend_int = triangle_pipeline(8);
        if let Command::CreateRenderPipelineState(p) = &mut blend_int {
            let mut c = proto::PipelineColorRecord::with_format(PixelFormat::R32Uint.raw() as u16);
            c.blending_enabled = true;
            p.color_attachments = list(&[c]);
        }
        let cases: Vec<(Command<'static>, ErrorCode)> = vec![
            (
                Command::CreateSampler(bad_aniso),
                ErrorCode::InvalidArgument,
            ),
            (Command::CreateSampler(bias), ErrorCode::Unimplemented),
            (Command::CreateSampler(bad_mode), ErrorCode::InvalidArgument),
            (
                Command::CreateDepthStencilState(bad_compare),
                ErrorCode::InvalidArgument,
            ),
            (
                Command::CreateDepthStencilState(bad_stencil),
                ErrorCode::InvalidArgument,
            ),
            (with_fn, ErrorCode::Unimplemented),
            (msaa8, ErrorCode::InvalidArgument),
            (blend_int, ErrorCode::InvalidArgument),
            (
                create_texture(
                    9,
                    PixelFormat::BGRA8Unorm,
                    TextureType::Type2DMultisample,
                    8,
                    StorageMode::Private,
                    rt_usage(),
                    64,
                    64,
                ),
                ErrorCode::InvalidArgument,
            ),
        ];
        for (cmd, code) in cases {
            let out = s.execute_stream(&stream(&[cmd]));
            assert_eq!(
                record_errors(&out)[0].code,
                code,
                "{cmd:?} {:?}",
                out.errors
            );
        }
        let b = s.backend();
        assert_eq!(
            (
                b.live_samplers(),
                b.live_depth_stencil_states(),
                b.live_render_pipelines()
            ),
            (0, 0, 1)
        );
    }

    /// Iris Xe (assumed): 29 vertex attributes. Index 30 is valid for the guest (Apple7: 31),
    /// so it is a device gap (`Unimplemented`), not a guest error.
    #[test]
    fn vertex_input_gaps_are_unimplemented() {
        let mut p = triangle_pipeline(30);
        if let Command::CreateRenderPipelineState(c) = &mut p {
            c.vertex_attributes = list(&[proto::VertexAttributeRecord {
                index: 30,
                format: crate::model::VertexFormat::Float.raw() as u16,
                buffer_index: 0,
                offset: 0,
            }]);
        }
        for (caps, code) in [
            (Capabilities::rtx4050_assumed(), None),
            (
                Capabilities::iris_xe_assumed(),
                Some(ErrorCode::Unimplemented),
            ),
        ] {
            let mut s = GuestSession::new(NullBackend::new(caps));
            handshake(&mut s);
            let out = s.execute_stream(&stream(&[p]));
            assert_eq!(record_errors(&out).first().map(|e| e.code), code);
        }
    }

    #[test]
    fn compact_pass_store_action_can_be_deferred_too() {
        let mut s = session();
        let pass = Command::BeginRenderPass(
            BeginRenderPass::new(
                0,
                0,
                &[ColorAttachmentRecord {
                    texture_id: oid(2),
                    load_action: 2,
                    store_action: StoreAction::Unknown.raw() as u8,
                    clear_color: [0.0; 4],
                }],
            )
            .unwrap(),
        );
        let frame = |action: StoreAction, id| {
            stream(&[
                pass,
                bind_pipe(),
                draw(3),
                Command::SetStoreAction(proto::SetStoreAction {
                    attachment: proto::AttachmentKind::Color(0),
                    store_action: action.raw() as u8,
                }),
                Command::EndRenderPass,
                Command::Commit { submission_id: id },
            ])
        };
        s.execute_stream(&stream(&[create_rt(2)]));
        let out = s.execute_stream(&frame(StoreAction::DontCare, 1));
        assert_eq!(wire(&out), vec![Reply::CommandBufferStatus(completed(1))]);
        // Valid Metal without a Vulkan mapping yet: unimplemented, the buffer fails.
        let out = s.execute_stream(&frame(StoreAction::CustomSampleDepthStore, 2));
        assert_eq!(
            record_errors(&out)
                .iter()
                .map(|e| (e.record_index, e.code))
                .collect::<Vec<_>>(),
            vec![(3, ErrorCode::Unimplemented)]
        );
        assert_eq!(statuses(&out), vec![failed(2, ErrorCode::Unimplemented)]);
    }

    /// Regression for review finding WP-GPU-24 (major): parents kept alive by views must
    /// count toward the texture limit, otherwise a chain `V1 = view(T); destroy T;
    /// V2 = view(V1); destroy V1; …` holds unbounded backend textures with one guest name.
    #[test]
    fn regression_view_chain_with_one_name_is_bounded() {
        let limit = 4;
        let mut s = small(Limits {
            objects_per_kind: limit,
            ..Limits::DEFAULT
        });
        let view = |id, parent| {
            Command::CreateTextureView(proto::CreateTextureView {
                texture_id: oid(id),
                parent_id: oid(parent),
                pixel_format: PixelFormat::BGRA8Unorm_sRGB.raw() as u16,
                texture_type: TextureType::Type2D.raw() as u8,
                swizzle: proto::SWIZZLE_IDENTITY,
                level_start: 0,
                level_count: 1,
                slice_start: 0,
                slice_count: 1,
            })
        };
        assert!(record_errors(&s.execute_stream(&stream(&[create_rt(1)]))).is_empty());
        let mut name = 1;
        let mut created = 0;
        for next in 2..100 {
            let out = s.execute_stream(&stream(&[view(next, name)]));
            let errs = record_errors(&out);
            if errs.is_empty() {
                let out = s.execute_stream(&stream(&[Command::DestroyTexture {
                    texture_id: oid(name),
                }]));
                assert!(record_errors(&out).is_empty());
                created += 1;
                name = next;
            } else {
                assert_eq!(errs[0].code, ErrorCode::LimitExceeded, "{:?}", out.errors);
                break;
            }
            assert!(
                s.backend().live_textures() <= limit,
                "{}",
                s.backend().live_textures()
            );
        }
        assert_eq!(
            created,
            limit - 1,
            "one live name + retained parents fill the limit"
        );
        assert_eq!(s.backend().live_textures(), limit);
        // Destroying the last view releases the whole chain and the budget.
        let out = s.execute_stream(&stream(&[Command::DestroyTexture {
            texture_id: oid(name),
        }]));
        assert!(record_errors(&out).is_empty());
        assert_eq!(s.backend().live_textures(), 0);
        assert_eq!(s.backend().allocated_bytes(), 0);
        let fill: Vec<_> = (10..10 + limit as u32).map(create_rt).collect();
        assert!(record_errors(&s.execute_stream(&stream(&fill))).is_empty());
        // The backend's own handle-table limit is a limit, not an invalid command.
        assert_eq!(
            FrontendError::Gpu(GpuError::Handle(HandleError::Full { max_slots: 1 })).code(),
            ErrorCode::LimitExceeded
        );
    }

    /// Retention also survives deferred destruction and session resets.
    #[test]
    fn retained_parents_are_released_by_reset_and_deferred_commit() {
        let mut s = small(Limits {
            objects_per_kind: 3,
            ..Limits::DEFAULT
        });
        let view = Command::CreateTextureView(proto::CreateTextureView {
            texture_id: oid(5),
            parent_id: oid(2),
            pixel_format: PixelFormat::BGRA8Unorm_sRGB.raw() as u16,
            texture_type: TextureType::Type2D.raw() as u8,
            swizzle: proto::SWIZZLE_IDENTITY,
            level_start: 0,
            level_count: 1,
            slice_start: 0,
            slice_count: 1,
        });
        let mut cmds = vec![create_rt(2), view];
        cmds.extend(triangle_pass(2));
        cmds.push(Command::DestroyTexture { texture_id: oid(2) }); // deferred
        cmds.push(create_rt(3)); // 2 live names + 1 deferred = limit
        cmds.push(create_rt(4));
        let out = s.execute_stream(&stream(&cmds));
        assert_eq!(
            record_errors(&out)
                .iter()
                .map(|e| e.code)
                .collect::<Vec<_>>(),
            vec![ErrorCode::LimitExceeded]
        );
        // Commit: the deferred destroy runs, the parent is retained by view 5 → still full.
        let out = s.execute_stream(&stream(&[
            Command::Commit { submission_id: 1 },
            create_rt(4),
        ]));
        assert_eq!(statuses(&out), vec![completed(1)]);
        assert_eq!(record_errors(&out)[0].code, ErrorCode::LimitExceeded);
        assert_eq!(s.backend().live_textures(), 3);
        // A reset destroys everything and forgets the retention bookkeeping.
        s.execute_stream(b"garbage");
        assert_eq!(s.backend().live_textures(), 0);
        handshake(&mut s);
        let fill: Vec<_> = (10..13).map(create_rt).collect();
        assert!(record_errors(&s.execute_stream(&stream(&fill))).is_empty());
    }
}
