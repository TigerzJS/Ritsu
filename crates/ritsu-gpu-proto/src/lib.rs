//! Guest-host GPU command stream definitions and codecs (frontend G, ADR-0005).
//!
//! This crate defines Ritsu's own, documented command stream that a Ritsu guest GPU driver
//! (variant G of ADR-0005, still `Proposed`) sends to the host. The design is described in
//! `docs/subsystems/gpu.md` §9. The host core (`ritsu-gpu-host`) does not depend on the wire
//! format; only its frontend G does.
//!
//! # Wire format (version [`PROTOCOL_VERSION`])
//!
//! All integers are little-endian. A *stream* is:
//!
//! ```text
//! +--------------------------- Header (16 bytes) ----------------------------+
//! | magic u32 = "RTSG" | version u16 | flags u16 (=0) | record_count u32 | reserved u32 (=0) |
//! +--------------------------------------------------------------------------+
//! | Record 0 | Record 1 | ... | Record record_count-1 |   (no trailing bytes)
//!
//! Record:
//! | opcode u16 | flags u16 (=0) | payload_len u32 | payload | zero padding to 8 bytes |
//! ```
//!
//! Every record occupies a multiple of [`RECORD_ALIGN`] bytes, so every record header sits at
//! an 8-byte aligned offset relative to the stream start. The decoder does not require the
//! *buffer* to be aligned in host memory (it reads bytes, never casts pointers).
//!
//! # Session
//!
//! A session starts with a bootstrap stream holding a single HELLO record; the host answers
//! on the reply channel (same framing, magic `"RTSR"`, [`ReplyOpcode`]) with `HelloAck` or
//! `HelloReject`. Afterwards streams carry the negotiated version and may use feature-gated
//! opcodes only if negotiated ([`negotiate`] module). The reply channel also carries
//! per-record errors, session resets and command buffer completion ([`Reply`]).
//!
//! # Hostile input
//!
//! Everything here decodes guest-controlled bytes. Every malformed input is reported as a
//! [`ProtoError`]; nothing panics, indexes unchecked, or allocates proportionally to a
//! guest-declared count. Fuzz targets (all panic-free on every input):
//!
//! * [`decode_stream`]: command stream framing + payloads (session-independent).
//! * [`decode_hello_stream`] / [`decode_session_stream`]: the same plus handshake rules.
//! * [`decode_reply_stream`]: reply channel (host → guest).

mod command;
mod encode;
mod error;
pub mod negotiate;
mod reader;
mod reply;
mod wire;

pub use command::{
    BeginRenderPass, ColorAttachmentRecord, Command, CopyBuffer, CreateBuffer, CreateTexture,
    Dispatch, Draw, EventValue, MAX_COLOR_ATTACHMENTS, ObjectId, WriteBuffer,
};
pub use encode::{ReplyEncoder, StreamEncoder};
pub use error::ProtoError;
pub use negotiate::{
    Features, Hello, HelloReject, Negotiated, decode_hello_stream, decode_session_stream,
};
pub use reply::{
    CapabilityBlock, CommandBufferStatus, ErrorCode, HelloAck, NO_RECORD, RecordError, Reply,
    StreamFault, SubmissionStatus, SupportLevel, decode_reply_stream,
};
pub use wire::{
    BOOTSTRAP_VERSION, HEADER_LEN, Header, MAGIC, MAX_OUTSTANDING_SUBMISSIONS, MAX_PAYLOAD_LEN,
    MIN_PROTOCOL_VERSION, Opcode, OpcodeClass, PROTOCOL_VERSION, PayloadLen, RECORD_ALIGN,
    RECORD_HEADER_LEN, REPLY_MAGIC, Record, RecordIter, ReplyOpcode, version_parseable,
};

/// Decodes and fully validates a complete stream into typed [`Command`]s.
///
/// This is the convenience entry point used by the host frontend and the intended fuzz target:
/// it must return `Ok` or `Err` for *every* input and never panic. The whole stream is validated
/// before anything is returned, so callers can implement validate-then-execute semantics
/// (a malformed stream has no partial side effects).
///
/// Memory use is bounded by the input length: the result vector grows per decoded record,
/// never by the guest-declared record count.
///
/// # Errors
///
/// Returns the first [`ProtoError`] encountered (framing, unknown opcode, bad payload).
pub fn decode_stream(bytes: &[u8]) -> Result<Vec<Command<'_>>, ProtoError> {
    let mut out = Vec::new();
    for record in RecordIter::new(bytes)? {
        out.push(Command::decode(&record?)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
