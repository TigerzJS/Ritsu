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
//! # Hostile input
//!
//! Everything here decodes guest-controlled bytes. Every malformed input is reported as a
//! [`ProtoError`]; nothing panics, indexes unchecked, or allocates proportionally to a
//! guest-declared count. [`decode_stream`] is the intended fuzz target.

mod command;
mod encode;
mod error;
mod reader;
mod wire;

pub use command::{
    BeginRenderPass, ColorAttachmentRecord, Command, CopyBuffer, CreateBuffer, CreateTexture,
    Dispatch, Draw, EventValue, MAX_COLOR_ATTACHMENTS, ObjectId, WriteBuffer,
};
pub use encode::StreamEncoder;
pub use error::ProtoError;
pub use wire::{
    HEADER_LEN, Header, MAGIC, MAX_PAYLOAD_LEN, Opcode, PROTOCOL_VERSION, PayloadLen, RECORD_ALIGN,
    RECORD_HEADER_LEN, Record, RecordIter,
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
