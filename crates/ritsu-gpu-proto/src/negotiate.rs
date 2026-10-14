//! Session handshake: version and feature negotiation (HELLO / HELLO_ACK).
//!
//! Life cycle of a session (gpu.md §9.3):
//!
//! 1. The guest sends a *bootstrap stream* (header version [`BOOTSTRAP_VERSION`]) containing
//!    exactly one [`Opcode::Hello`] record with its version range and feature bits (plus
//!    optional `Nop` padding).
//! 2. The host answers with `HelloAck` (chosen version, enabled features, capability block)
//!    or `HelloReject` (no common version). See [`negotiate`].
//! 3. Every later stream must carry the negotiated version in its header and may only use
//!    opcodes whose [`Opcode::required_feature`] was negotiated. A second HELLO is a protocol
//!    violation. See [`decode_session_stream`].
//!
//! Before step 2 succeeded, any other stream is rejected ([`ProtoError::HelloRequired`]).
//!
//! **Forward compatibility of the frozen HELLO_ACK** (decision): the host only puts values
//! into replies that exist in the negotiated version and feature set (family bits, support
//! levels, error codes, reply opcodes such as `EventSignaled` only with
//! [`Features::SHARED_EVENTS`]). The reference guest decoder additionally ignores unknown
//! family bits, so a newer host can add families without breaking older guests.

use crate::command::Command;
use crate::error::ProtoError;
use crate::wire::{BOOTSTRAP_VERSION, MIN_PROTOCOL_VERSION, Opcode, PROTOCOL_VERSION, RecordIter};

/// Optional protocol features, negotiated per session as a 64-bit set.
///
/// Bits are part of the wire ABI and never reused. Unknown bits offered by a guest are never
/// enabled (the host only acknowledges bits it knows *and* implements).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Features(u64);

impl Features {
    /// Shared events: opcodes `SignalEvent`/`WaitEvent` and the `EventSignaled` reply.
    pub const SHARED_EVENTS: Self = Self(1 << 0);

    /// Every feature bit defined by this protocol version.
    pub const KNOWN: Self = Self::SHARED_EVENTS;

    /// The empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Raw bits.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Wraps raw bits, keeping unknown ones (as received on the wire).
    #[must_use]
    pub const fn from_bits_retain(bits: u64) -> Self {
        Self(bits)
    }

    /// Whether every bit of `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Bits set in both.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Bits set in either.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether no bit is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// `Hello` payload (16 bytes, frozen layout):
/// `min_version u16 | max_version u16 | reserved u32 | features u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello {
    /// Lowest protocol version the guest speaks (≥ 1).
    pub min_version: u16,
    /// Highest protocol version the guest speaks (≥ `min_version`).
    pub max_version: u16,
    /// Features the guest would like to use (may contain bits unknown to the host).
    pub features: Features,
}

/// Result of a successful handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    /// Protocol version of all further streams of the session.
    pub version: u16,
    /// Enabled features (subset of guest offer, host offer and [`Features::KNOWN`]).
    pub features: Features,
}

/// `HelloReject` payload (8 bytes, frozen layout):
/// `host_min_version u16 | host_max_version u16 | reserved u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelloReject {
    /// Lowest version the host speaks.
    pub host_min_version: u16,
    /// Highest version the host speaks.
    pub host_max_version: u16,
}

/// Chooses the session version and feature set.
///
/// Version: the highest version in both the guest's range and
/// `MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION`. Features: guest offer ∩ `host_features` ∩
/// [`Features::KNOWN`]. `host_features` must only contain what the host actually implements
/// (never silently wrong, CLAUDE.md §3.5).
///
/// # Errors
///
/// [`HelloReject`] with the host's version range if the ranges do not overlap (or the guest
/// range is empty).
pub fn negotiate(hello: &Hello, host_features: Features) -> Result<Negotiated, HelloReject> {
    let lo = hello.min_version.max(MIN_PROTOCOL_VERSION);
    let hi = hello.max_version.min(PROTOCOL_VERSION);
    if lo > hi {
        return Err(HelloReject {
            host_min_version: MIN_PROTOCOL_VERSION,
            host_max_version: PROTOCOL_VERSION,
        });
    }
    Ok(Negotiated {
        version: hi,
        features: hello
            .features
            .intersection(host_features)
            .intersection(Features::KNOWN),
    })
}

/// Decodes the bootstrap stream of a not yet negotiated session.
///
/// The stream must carry header version [`BOOTSTRAP_VERSION`] and consist of exactly one
/// wire-valid HELLO record, optionally surrounded by `Nop` records (ring padding, gpu.md
/// §9.1). Panic-free on every input (fuzz target).
///
/// # Errors
///
/// Any framing error; [`ProtoError::HelloRequired`] if a record other than `Nop` precedes
/// HELLO or the stream holds no HELLO (offset = end of stream); [`ProtoError::HelloNotAlone`]
/// for a non-`Nop` record (including a second HELLO) after HELLO.
pub fn decode_hello_stream(bytes: &[u8]) -> Result<Hello, ProtoError> {
    let records = RecordIter::new(bytes)?;
    let header = records.header();
    if header.version != BOOTSTRAP_VERSION {
        return Err(ProtoError::UnsupportedVersion {
            found: header.version,
            supported: BOOTSTRAP_VERSION,
        });
    }
    let mut hello = None;
    for record in records {
        let record = record?;
        // Decode every record, `Nop` included, so wire errors are never skipped.
        let cmd = Command::decode(&record)?;
        match (cmd, hello) {
            (Command::Nop, _) => {}
            (Command::Hello(h), None) => hello = Some(h),
            (_, None) => {
                return Err(ProtoError::HelloRequired {
                    offset: record.offset,
                    raw: record.opcode.raw(),
                });
            }
            (_, Some(_)) => {
                return Err(ProtoError::HelloNotAlone {
                    offset: record.offset,
                });
            }
        }
    }
    hello.ok_or(ProtoError::HelloRequired {
        offset: bytes.len(),
        raw: 0,
    })
}

/// Decodes and fully validates a stream of an established session.
///
/// In addition to [`crate::decode_stream`]: the header version must equal the negotiated
/// version, HELLO is rejected, and feature-gated opcodes need their negotiated feature. The
/// whole stream is validated before anything is returned (validate, then execute).
/// Panic-free on every input (fuzz target).
///
/// # Errors
///
/// Framing errors, [`ProtoError::VersionMismatch`], [`ProtoError::UnexpectedHello`],
/// [`ProtoError::FeatureNotNegotiated`].
pub fn decode_session_stream<'a>(
    bytes: &'a [u8],
    session: &Negotiated,
) -> Result<Vec<Command<'a>>, ProtoError> {
    let records = RecordIter::new(bytes)?;
    let found = records.header().version;
    if found != session.version {
        return Err(ProtoError::VersionMismatch {
            found,
            negotiated: session.version,
        });
    }
    let mut out = Vec::new();
    for record in records {
        let record = record?;
        if record.opcode == Opcode::Hello {
            return Err(ProtoError::UnexpectedHello {
                offset: record.offset,
            });
        }
        if let Some(needed) = record.opcode.required_feature()
            && !session.features.contains(needed)
        {
            return Err(ProtoError::FeatureNotNegotiated {
                offset: record.offset,
                raw: record.opcode.raw(),
            });
        }
        out.push(Command::decode(&record)?);
    }
    Ok(out)
}
