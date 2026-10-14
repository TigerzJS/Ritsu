//! Handshake and reply-channel tests: negotiation rules, session stream checks, reply round
//! trips, truncation/bit-flip/garbage robustness of the reply decoder.

use super::*;
use crate::negotiate::negotiate;

fn hello(min: u16, max: u16, features: u64) -> Hello {
    Hello {
        min_version: min,
        max_version: max,
        features: Features::from_bits_retain(features),
    }
}

fn hello_stream(h: Hello) -> Vec<u8> {
    let mut e = StreamEncoder::new();
    e.push(&Command::Hello(h)).unwrap();
    e.finish()
}

fn v1(features: Features) -> Negotiated {
    Negotiated {
        version: 1,
        features,
    }
}

// ---- negotiation ------------------------------------------------------------------------

#[test]
fn matching_version_is_chosen_highest_common() {
    let n = negotiate(&hello(1, 1, 0), Features::KNOWN).unwrap();
    assert_eq!(n.version, PROTOCOL_VERSION);
    // A newer guest that still speaks v1 gets v1.
    let n = negotiate(&hello(1, 9, 0), Features::KNOWN).unwrap();
    assert_eq!(n.version, PROTOCOL_VERSION);
}

#[test]
fn non_overlapping_versions_are_rejected_with_host_range() {
    let reject = HelloReject {
        host_min_version: MIN_PROTOCOL_VERSION,
        host_max_version: PROTOCOL_VERSION,
    };
    assert_eq!(negotiate(&hello(2, 5, 0), Features::KNOWN), Err(reject));
    // Hand-built invalid ranges never panic and are rejected too.
    assert_eq!(negotiate(&hello(0, 0, 0), Features::KNOWN), Err(reject));
    assert_eq!(negotiate(&hello(3, 1, 0), Features::KNOWN), Err(reject));
    assert_eq!(
        negotiate(&hello(u16::MAX, u16::MAX, 0), Features::KNOWN),
        Err(reject)
    );
}

#[test]
fn features_are_the_intersection_of_guest_host_and_known() {
    let unknown = 1u64 << 40;
    // Guest offers a known and an unknown bit; host offers the known bit.
    let n = negotiate(
        &hello(1, 1, Features::SHARED_EVENTS.bits() | unknown),
        Features::SHARED_EVENTS,
    )
    .unwrap();
    assert_eq!(n.features, Features::SHARED_EVENTS);
    // Host does not offer it -> not enabled.
    let n = negotiate(
        &hello(1, 1, Features::SHARED_EVENTS.bits()),
        Features::empty(),
    )
    .unwrap();
    assert!(n.features.is_empty());
    // Even if a (buggy) host offers unknown bits, they are never enabled.
    let n = negotiate(&hello(1, 1, u64::MAX), Features::from_bits_retain(u64::MAX)).unwrap();
    assert_eq!(n.features, Features::KNOWN);
    // Guest offers nothing -> nothing.
    let n = negotiate(&hello(1, 1, 0), Features::KNOWN).unwrap();
    assert!(n.features.is_empty());
}

#[test]
fn features_set_operations() {
    let a = Features::from_bits_retain(0b1010);
    let b = Features::from_bits_retain(0b0110);
    assert_eq!(a.intersection(b).bits(), 0b0010);
    assert_eq!(a.union(b).bits(), 0b1110);
    assert!(a.contains(Features::from_bits_retain(0b1000)));
    assert!(!a.contains(b));
    assert!(a.contains(Features::empty()));
    assert!(Features::default().is_empty());
}

// ---- hello stream -----------------------------------------------------------------------

#[test]
fn hello_stream_round_trip() {
    let h = hello(1, 2, Features::SHARED_EVENTS.bits() | 1 << 63);
    assert_eq!(decode_hello_stream(&hello_stream(h)), Ok(h));
}

#[test]
fn hello_wire_validation() {
    for (min, max, field) in [(0u16, 1u16, "min_version"), (2, 1, "max_version")] {
        let mut payload = Vec::new();
        Command::Hello(hello(1, 1, 0)).encode_payload(&mut payload);
        payload[0..2].copy_from_slice(&min.to_le_bytes());
        payload[2..4].copy_from_slice(&max.to_le_bytes());
        let mut e = StreamEncoder::new();
        e.push_raw(Opcode::Hello.raw(), 0, &payload).unwrap();
        assert!(
            matches!(
                decode_hello_stream(&e.finish()),
                Err(ProtoError::InvalidField { field: f, .. }) if f == field
            ),
            "{min}..{max}"
        );
    }
    // Reserved bytes must be zero.
    let mut b = hello_stream(hello(1, 1, 0));
    b[HEADER_LEN + 8 + 5] = 1;
    assert_eq!(
        decode_hello_stream(&b),
        Err(ProtoError::ReservedNonZero {
            offset: HEADER_LEN + 8 + 5
        })
    );
}

#[test]
fn commands_before_hello_are_rejected() {
    let mut e = StreamEncoder::new();
    e.push(&Command::DestroyBuffer {
        buffer_id: ObjectId::new(1).unwrap(),
    })
    .unwrap();
    let err = decode_hello_stream(&e.finish()).unwrap_err();
    assert_eq!(
        err,
        ProtoError::HelloRequired {
            offset: HEADER_LEN,
            raw: Opcode::DestroyBuffer.raw()
        }
    );
    assert_eq!(err.reply_code(), ErrorCode::HelloRequired);
    // An empty stream is not a HELLO either.
    assert_eq!(
        decode_hello_stream(&StreamEncoder::new().finish()),
        Err(ProtoError::HelloRequired {
            offset: HEADER_LEN,
            raw: 0
        })
    );
}

#[test]
fn unknown_family_bits_are_ignored_by_the_guest_decoder() {
    let ack = HelloAck {
        version: 1,
        features: Features::empty(),
        caps: caps(),
    };
    let mut p = Vec::new();
    Reply::HelloAck(ack).encode_payload(&mut p);
    // A newer host announces family bit 31.
    p[16 + 3] |= 0x80;
    let mut e = ReplyEncoder::new();
    e.push_raw(ReplyOpcode::HelloAck.raw(), 0, &p).unwrap();
    assert_eq!(
        decode_reply_stream(&e.finish()).unwrap(),
        vec![Reply::HelloAck(ack)]
    );
}

#[test]
fn nop_padding_is_allowed_around_hello() {
    let h = hello(1, 1, 0);
    let mut e = StreamEncoder::new();
    e.push(&Command::Nop).unwrap();
    e.push_raw(Opcode::Nop.raw(), 0, &[0xAB; 13]).unwrap();
    e.push(&Command::Hello(h)).unwrap();
    e.push(&Command::Nop).unwrap();
    assert_eq!(decode_hello_stream(&e.finish()), Ok(h));
    // Only Nops: no HELLO, offset = end of stream.
    let mut e = StreamEncoder::new();
    e.push(&Command::Nop).unwrap();
    let b = e.finish();
    assert_eq!(
        decode_hello_stream(&b),
        Err(ProtoError::HelloRequired {
            offset: b.len(),
            raw: 0
        })
    );
    // A second HELLO after Nop padding is still not alone.
    let mut e = StreamEncoder::new();
    e.push(&Command::Hello(h)).unwrap();
    e.push(&Command::Nop).unwrap();
    e.push(&Command::Hello(h)).unwrap();
    assert_eq!(
        decode_hello_stream(&e.finish()),
        Err(ProtoError::HelloNotAlone {
            offset: HEADER_LEN + 8 + 16 + 8
        })
    );
    // A command between Nops before HELLO is rejected at its offset.
    let mut e = StreamEncoder::new();
    e.push(&Command::Nop).unwrap();
    e.push(&Command::EndRenderPass).unwrap();
    e.push(&Command::Hello(h)).unwrap();
    assert_eq!(
        decode_hello_stream(&e.finish()),
        Err(ProtoError::HelloRequired {
            offset: HEADER_LEN + 8,
            raw: Opcode::EndRenderPass.raw()
        })
    );
}

#[test]
fn bootstrap_version_is_always_parseable() {
    assert!(version_parseable(BOOTSTRAP_VERSION));
    for v in MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION {
        assert!(version_parseable(v));
    }
    assert!(!version_parseable(0));
    assert!(!version_parseable(PROTOCOL_VERSION + 1));
    let cmd = StreamEncoder::with_version(BOOTSTRAP_VERSION).finish();
    assert_eq!(
        Header::parse(&cmd).map(|h| h.version),
        Ok(BOOTSTRAP_VERSION)
    );
    let rep = ReplyEncoder::with_version(BOOTSTRAP_VERSION).finish();
    assert_eq!(
        Header::parse_reply(&rep).map(|h| h.version),
        Ok(BOOTSTRAP_VERSION)
    );
}

#[test]
fn every_opcode_has_a_class() {
    for op in Opcode::ALL {
        let expected = match op.raw() >> 8 {
            0x00 => OpcodeClass::Control,
            0x01 if *op == Opcode::CopyBuffer => OpcodeClass::Encoder, // blit, recorded
            0x01 => OpcodeClass::Resource,
            0x02 | 0x03 => OpcodeClass::Encoder,
            0x04 => OpcodeClass::Sync,
            0x05 => OpcodeClass::Submission,
            _ => panic!("{op:?} outside the documented ranges"),
        };
        assert_eq!(op.class(), expected, "{op:?}");
    }
}

#[test]
fn hello_must_be_alone_in_its_stream() {
    let mut e = StreamEncoder::new();
    e.push(&Command::Hello(hello(1, 1, 0))).unwrap();
    e.push(&Command::EndRenderPass).unwrap();
    assert_eq!(
        decode_hello_stream(&e.finish()),
        Err(ProtoError::HelloNotAlone {
            offset: HEADER_LEN + 8 + 16
        })
    );
}

#[test]
fn hello_stream_framing_errors_propagate() {
    let mut b = hello_stream(hello(1, 1, 0));
    b[0] ^= 0xFF;
    assert!(matches!(
        decode_hello_stream(&b),
        Err(ProtoError::BadMagic { .. })
    ));
    let b = hello_stream(hello(1, 1, 0));
    for cut in 0..b.len() {
        assert!(decode_hello_stream(&b[..cut]).is_err());
    }
}

// ---- session streams --------------------------------------------------------------------

#[test]
fn duplicate_hello_in_session_is_rejected() {
    let err =
        decode_session_stream(&hello_stream(hello(1, 1, 0)), &v1(Features::empty())).unwrap_err();
    assert_eq!(err, ProtoError::UnexpectedHello { offset: HEADER_LEN });
    assert_eq!(err.reply_code(), ErrorCode::DuplicateHello);
    assert_eq!(err.raw_opcode(), Some(Opcode::Hello.raw()));
}

#[test]
fn feature_gated_opcodes_need_negotiation() {
    let ev = Command::SignalEvent(EventValue {
        event_id: ObjectId::new(3).unwrap(),
        value: 1,
    });
    let mut e = StreamEncoder::new();
    e.push(&Command::Nop).unwrap();
    e.push(&ev).unwrap();
    let bytes = e.finish();
    let err = decode_session_stream(&bytes, &v1(Features::empty())).unwrap_err();
    assert_eq!(
        err,
        ProtoError::FeatureNotNegotiated {
            offset: HEADER_LEN + 8,
            raw: Opcode::SignalEvent.raw()
        }
    );
    assert_eq!(err.reply_code(), ErrorCode::FeatureNotNegotiated);
    let ok = decode_session_stream(&bytes, &v1(Features::SHARED_EVENTS)).unwrap();
    assert_eq!(ok, vec![Command::Nop, ev]);
    assert_eq!(
        Opcode::WaitEvent.required_feature(),
        Some(Features::SHARED_EVENTS)
    );
    assert_eq!(Opcode::Draw.required_feature(), None);
}

#[test]
fn session_stream_version_must_match_negotiated() {
    let bytes = StreamEncoder::new().finish();
    let err = decode_session_stream(
        &bytes,
        &Negotiated {
            version: 7,
            features: Features::empty(),
        },
    )
    .unwrap_err();
    assert_eq!(
        err,
        ProtoError::VersionMismatch {
            found: 1,
            negotiated: 7
        }
    );
    assert_eq!(err.reply_code(), ErrorCode::UnsupportedVersion);
    // Versions outside the supported range fail at the header already.
    let bytes = StreamEncoder::with_version(PROTOCOL_VERSION + 1).finish();
    assert!(matches!(
        decode_session_stream(&bytes, &v1(Features::empty())),
        Err(ProtoError::UnsupportedVersion { .. })
    ));
    assert!(matches!(
        decode_hello_stream(&bytes),
        Err(ProtoError::UnsupportedVersion { .. })
    ));
}

#[test]
fn proto_error_offsets_and_codes() {
    assert_eq!(ProtoError::BadMagic { found: 0 }.offset(), Some(0));
    assert_eq!(
        ProtoError::BadMagic { found: 0 }.reply_code(),
        ErrorCode::Malformed
    );
    assert_eq!(
        ProtoError::UnknownOpcode {
            offset: 24,
            raw: 0x7777
        }
        .raw_opcode(),
        Some(0x7777)
    );
    assert_eq!(ProtoError::EncoderLimit("x").offset(), None);
}

// ---- reply channel ----------------------------------------------------------------------

fn caps() -> CapabilityBlock {
    CapabilityBlock {
        families: CapabilityBlock::FAMILY_MAC2
            | CapabilityBlock::FAMILY_COMMON3
            | CapabilityBlock::FAMILY_METAL3,
        argument_buffers_tier: 2,
        ray_tracing: SupportLevel::Native,
        mesh_shaders: SupportLevel::Native,
        astc: SupportLevel::Unsupported,
        bc: SupportLevel::Native,
        float32_atomics: SupportLevel::Native,
        framebuffer_fetch: SupportLevel::Native,
        raster_order_groups: SupportLevel::Native,
        tile_shaders: SupportLevel::Unsupported,
        memoryless: SupportLevel::Emulated,
        unified_memory: SupportLevel::Emulated,
        max_buffer_length: 4 << 30,
        max_texture_dimension_1d: 16384,
        max_texture_dimension_2d: 16384,
        max_texture_dimension_3d: 2048,
        max_texture_array_layers: 2048,
        max_threads_per_threadgroup: 1024,
        max_color_attachments: 8,
    }
}

fn sample_replies() -> Vec<Reply> {
    vec![
        Reply::HelloAck(HelloAck {
            version: 1,
            features: Features::SHARED_EVENTS,
            caps: caps(),
        }),
        Reply::HelloReject(HelloReject {
            host_min_version: 1,
            host_max_version: 2,
        }),
        Reply::SessionReset(StreamFault {
            stream_seq: 5,
            offset: 40,
            opcode: 0x7777,
            code: ErrorCode::Malformed,
        }),
        Reply::RecordError(RecordError {
            stream_seq: 6,
            record_index: 3,
            opcode: Opcode::Draw.raw(),
            code: ErrorCode::InvalidArgument,
            object_id: None,
            detail: 0,
        }),
        Reply::RecordError(RecordError {
            stream_seq: 6,
            record_index: 4,
            opcode: Opcode::DestroyBuffer.raw(),
            code: ErrorCode::UnknownObject,
            object_id: ObjectId::new(9),
            detail: u64::MAX,
        }),
        Reply::CommandBufferStatus(SubmissionStatus {
            submission_id: 77,
            status: CommandBufferStatus::Completed,
        }),
        Reply::CommandBufferStatus(SubmissionStatus {
            submission_id: 78,
            status: CommandBufferStatus::Error(ErrorCode::InvalidArgument),
        }),
        Reply::EventSignaled(EventValue {
            event_id: ObjectId::new(3).unwrap(),
            value: 42,
        }),
    ]
}

fn encode_replies(replies: &[Reply]) -> Vec<u8> {
    let mut e = ReplyEncoder::new();
    for r in replies {
        e.push(r).unwrap();
    }
    e.finish()
}

#[test]
fn reply_round_trip_all_opcodes() {
    let replies = sample_replies();
    let bytes = encode_replies(&replies);
    assert_eq!(bytes.len() % RECORD_ALIGN, 0);
    assert_eq!(decode_reply_stream(&bytes).unwrap(), replies);
    for op in ReplyOpcode::ALL {
        assert!(
            replies.iter().any(|r| r.opcode() == *op),
            "{op:?} not covered"
        );
        assert_eq!(ReplyOpcode::from_raw(op.raw()), Some(*op));
    }
    // Payload sizes match the length rules.
    for r in &replies {
        let mut p = Vec::new();
        r.encode_payload(&mut p);
        assert!(r.opcode().payload_len().accepts(p.len() as u32), "{r:?}");
    }
    let mut p = Vec::new();
    caps().encode(&mut p);
    assert_eq!(p.len(), CapabilityBlock::LEN);
}

#[test]
fn error_codes_round_trip() {
    for c in ErrorCode::ALL {
        assert_eq!(ErrorCode::from_raw(c.raw()), Some(*c));
    }
    assert_eq!(ErrorCode::from_raw(0), None);
    assert_eq!(ErrorCode::from_raw(0xFFFF), None);
}

#[test]
fn reply_and_command_streams_are_not_interchangeable() {
    let replies = encode_replies(&sample_replies());
    assert!(matches!(
        decode_stream(&replies),
        Err(ProtoError::BadMagic { .. })
    ));
    let cmds = StreamEncoder::new().finish();
    assert!(matches!(
        decode_reply_stream(&cmds),
        Err(ProtoError::BadMagic { .. })
    ));
}

#[test]
fn invalid_reply_fields_rejected() {
    let reply_with = |op: ReplyOpcode, patch: &dyn Fn(&mut Vec<u8>)| {
        let r = sample_replies()
            .into_iter()
            .find(|r| r.opcode() == op)
            .unwrap();
        let mut p = Vec::new();
        r.encode_payload(&mut p);
        patch(&mut p);
        let mut e = ReplyEncoder::new();
        e.push_raw(op.raw(), 0, &p).unwrap();
        decode_reply_stream(&e.finish())
    };
    let field = |r: Result<Vec<Reply>, ProtoError>| match r {
        Err(ProtoError::InvalidField { field, .. }) => field,
        other => panic!("expected InvalidField, got {other:?}"),
    };
    // HelloAck: version 0, unknown family bit, bad tier, bad support level.
    assert_eq!(
        field(reply_with(ReplyOpcode::HelloAck, &|p| p[0..2].fill(0))),
        "version"
    );
    assert_eq!(
        field(reply_with(ReplyOpcode::HelloAck, &|p| p[16 + 4] = 3)),
        "argument_buffers_tier"
    );
    assert_eq!(
        field(reply_with(ReplyOpcode::HelloAck, &|p| p[16 + 5] = 3)),
        "ray_tracing"
    );
    // HelloReject: empty range.
    assert_eq!(
        field(reply_with(ReplyOpcode::HelloReject, &|p| p[0] = 5)),
        "host_version_range"
    );
    // Unknown error code.
    assert_eq!(
        field(reply_with(ReplyOpcode::SessionReset, &|p| p[18..20].fill(0))),
        "code"
    );
    // CommandBufferStatus: completed with code, unknown status.
    assert_eq!(
        field(reply_with(ReplyOpcode::CommandBufferStatus, &|p| p[10] = 1)),
        "code"
    );
    assert_eq!(
        field(reply_with(ReplyOpcode::CommandBufferStatus, &|p| p[8] = 2)),
        "status"
    );
    // EventSignaled: id 0.
    assert_eq!(
        field(reply_with(ReplyOpcode::EventSignaled, &|p| p[0..4].fill(0))),
        "event_id"
    );
    // Reserved bytes.
    assert!(matches!(
        reply_with(ReplyOpcode::RecordError, &|p| p[20] = 1),
        Err(ProtoError::ReservedNonZero { .. })
    ));
    // Wrong length and unknown opcode.
    let mut e = ReplyEncoder::new();
    e.push_raw(ReplyOpcode::RecordError.raw(), 0, &[0; 24])
        .unwrap();
    assert!(matches!(
        decode_reply_stream(&e.finish()),
        Err(ProtoError::BadPayloadLength { len: 24, .. })
    ));
    let mut e = ReplyEncoder::new();
    e.push_raw(Opcode::Nop.raw(), 0, &[]).unwrap();
    assert!(matches!(
        decode_reply_stream(&e.finish()),
        Err(ProtoError::UnknownOpcode { .. })
    ));
}

#[test]
fn every_reply_truncation_is_an_error() {
    let bytes = encode_replies(&sample_replies());
    for cut in 0..bytes.len() {
        assert!(
            decode_reply_stream(&bytes[..cut]).is_err(),
            "prefix of length {cut} decoded successfully"
        );
    }
}

#[test]
fn every_reply_bit_flip_never_panics() {
    let bytes = encode_replies(&sample_replies());
    let mut b = bytes.clone();
    let mut ok = 0usize;
    for i in 0..b.len() {
        for bit in 0..8 {
            b[i] ^= 1 << bit;
            if decode_reply_stream(&b).is_ok() {
                ok += 1;
            }
            b[i] ^= 1 << bit;
        }
    }
    assert_eq!(b, bytes);
    assert!(ok < bytes.len() * 8, "bit flips must be detected sometimes");
}

#[test]
fn pseudo_random_reply_garbage_never_panics() {
    let mut state: u64 = 0xD1B5_4A32_D192_ED03;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for _ in 0..20_000 {
        let len = (next() % 256) as usize;
        let mut buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        if next() % 2 == 0 && buf.len() >= HEADER_LEN {
            let h = Header {
                version: PROTOCOL_VERSION,
                record_count: (next() % 8) as u32,
            };
            buf[..HEADER_LEN].copy_from_slice(&h.to_reply_bytes());
            let mut pos = HEADER_LEN;
            while pos + 8 <= buf.len() {
                let op = ReplyOpcode::ALL[(next() as usize) % ReplyOpcode::ALL.len()];
                buf[pos..pos + 2].copy_from_slice(&op.raw().to_le_bytes());
                buf[pos + 2..pos + 4].copy_from_slice(&[0, 0]);
                // Mostly the right length so payload decoding is exercised.
                let plen = match op.payload_len() {
                    PayloadLen::Exact(n) if next() % 4 != 0 => n,
                    _ => (next() % 72) as u32,
                };
                buf[pos + 4..pos + 8].copy_from_slice(&plen.to_le_bytes());
                pos += 8 + (plen as usize).div_ceil(8) * 8;
            }
        }
        let _ = decode_reply_stream(&buf);
        let _ = decode_hello_stream(&buf);
        let _ = decode_session_stream(&buf, &v1(Features::KNOWN));
    }
}
