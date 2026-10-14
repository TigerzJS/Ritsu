//! Stream encoder.
//!
//! The production encoder lives in the guest driver (C/ObjC, `guest/`). This Rust encoder
//! is the reference implementation of the wire format, used by host tests, fuzz seed
//! generation and documentation examples.

use crate::command::Command;
use crate::error::ProtoError;
use crate::wire::{HEADER_LEN, Header, MAX_PAYLOAD_LEN, PROTOCOL_VERSION, RECORD_ALIGN};

/// Builds a version-[`PROTOCOL_VERSION`] stream record by record.
#[derive(Debug, Clone)]
pub struct StreamEncoder {
    buf: Vec<u8>,
    records: u32,
}

impl Default for StreamEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamEncoder {
    /// Starts an empty stream.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: vec![0; HEADER_LEN],
            records: 0,
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
        self.records
    }

    /// Finalizes the header and returns the stream bytes.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        let header = Header {
            version: PROTOCOL_VERSION,
            record_count: self.records,
        };
        // `buf` always starts with HEADER_LEN placeholder bytes (see `new`).
        if let Some(h) = self.buf.get_mut(..HEADER_LEN) {
            h.copy_from_slice(&header.to_bytes());
        }
        self.buf
    }
}
