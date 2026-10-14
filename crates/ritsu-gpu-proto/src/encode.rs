//! Stream encoders (commands and replies).
//!
//! The production command encoder lives in the guest driver (C/ObjC, `guest/`). These Rust
//! encoders are the reference implementation of the wire format, used by the host (reply
//! channel), host tests, fuzz seed generation and documentation examples.

use crate::command::Command;
use crate::error::ProtoError;
use crate::reply::Reply;
use crate::wire::{HEADER_LEN, Header, MAX_PAYLOAD_LEN, PROTOCOL_VERSION, RECORD_ALIGN};

/// Direction-independent record buffer (internal).
#[derive(Debug, Clone)]
struct FrameBuf {
    buf: Vec<u8>,
    records: u32,
}

impl FrameBuf {
    fn new() -> Self {
        Self {
            buf: vec![0; HEADER_LEN],
            records: 0,
        }
    }

    fn push_raw(&mut self, opcode: u16, flags: u16, payload: &[u8]) -> Result<(), ProtoError> {
        let len = u32::try_from(payload.len())
            .ok()
            .filter(|&l| l <= MAX_PAYLOAD_LEN)
            .ok_or(ProtoError::EncoderLimit("payload too large"))?;
        let records = self
            .records
            .checked_add(1)
            .ok_or(ProtoError::EncoderLimit("too many records"))?;
        self.buf.extend_from_slice(&opcode.to_le_bytes());
        self.buf.extend_from_slice(&flags.to_le_bytes());
        self.buf.extend_from_slice(&len.to_le_bytes());
        self.buf.extend_from_slice(payload);
        let pad = (RECORD_ALIGN - payload.len() % RECORD_ALIGN) % RECORD_ALIGN;
        self.buf.resize(self.buf.len() + pad, 0);
        self.records = records;
        Ok(())
    }

    fn finish(mut self, header: [u8; HEADER_LEN]) -> Vec<u8> {
        // `buf` always starts with HEADER_LEN placeholder bytes (see `new`).
        if let Some(h) = self.buf.get_mut(..HEADER_LEN) {
            h.copy_from_slice(&header);
        }
        self.buf
    }
}

/// Builds a command stream record by record.
#[derive(Debug, Clone)]
pub struct StreamEncoder {
    frames: FrameBuf,
    version: u16,
}

impl Default for StreamEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamEncoder {
    /// Starts an empty stream with header version [`PROTOCOL_VERSION`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_version(PROTOCOL_VERSION)
    }

    /// Starts an empty stream with an explicit header version (tests: version mismatch).
    #[must_use]
    pub fn with_version(version: u16) -> Self {
        Self {
            frames: FrameBuf::new(),
            version,
        }
    }

    /// Appends a record with arbitrary opcode, flags and payload.
    ///
    /// No semantic checks are done: tests use this to build *invalid* streams (unknown
    /// opcodes, wrong lengths, flags). The record is padded with zeros to [`RECORD_ALIGN`].
    ///
    /// # Errors
    ///
    /// [`ProtoError::EncoderLimit`] if the payload exceeds [`MAX_PAYLOAD_LEN`] or the record
    /// count would overflow `u32`.
    pub fn push_raw(&mut self, opcode: u16, flags: u16, payload: &[u8]) -> Result<(), ProtoError> {
        self.frames.push_raw(opcode, flags, payload)
    }

    /// Appends a typed command.
    ///
    /// # Errors
    ///
    /// See [`StreamEncoder::push_raw`].
    pub fn push(&mut self, cmd: &Command<'_>) -> Result<(), ProtoError> {
        let mut payload = Vec::new();
        cmd.encode_payload(&mut payload);
        self.push_raw(cmd.opcode().raw(), 0, &payload)
    }

    /// Number of records appended so far.
    #[must_use]
    pub fn record_count(&self) -> u32 {
        self.frames.records
    }

    /// Finalizes the header and returns the stream bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        let header = Header {
            version: self.version,
            record_count: self.frames.records,
        };
        self.frames.finish(header.to_bytes())
    }
}

/// Builds a reply stream (host → guest) record by record.
#[derive(Debug, Clone)]
pub struct ReplyEncoder {
    frames: FrameBuf,
    version: u16,
}

impl Default for ReplyEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplyEncoder {
    /// Starts an empty reply stream with header version [`PROTOCOL_VERSION`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_version(PROTOCOL_VERSION)
    }

    /// Starts an empty reply stream with an explicit header version.
    #[must_use]
    pub fn with_version(version: u16) -> Self {
        Self {
            frames: FrameBuf::new(),
            version,
        }
    }

    /// Appends a raw reply record (tests: invalid replies).
    ///
    /// # Errors
    ///
    /// See [`StreamEncoder::push_raw`].
    pub fn push_raw(&mut self, opcode: u16, flags: u16, payload: &[u8]) -> Result<(), ProtoError> {
        self.frames.push_raw(opcode, flags, payload)
    }

    /// Appends a typed reply.
    ///
    /// # Errors
    ///
    /// See [`StreamEncoder::push_raw`].
    pub fn push(&mut self, reply: &Reply) -> Result<(), ProtoError> {
        let mut payload = Vec::new();
        reply.encode_payload(&mut payload);
        self.push_raw(reply.opcode().raw(), 0, &payload)
    }

    /// Number of records appended so far.
    #[must_use]
    pub fn record_count(&self) -> u32 {
        self.frames.records
    }

    /// Finalizes the header and returns the reply stream bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        let header = Header {
            version: self.version,
            record_count: self.frames.records,
        };
        self.frames.finish(header.to_reply_bytes())
    }
}
