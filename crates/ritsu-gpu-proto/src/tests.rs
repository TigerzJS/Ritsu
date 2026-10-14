//! Crate-level tests: round trips, malformed input, exhaustive truncation and mutation loops.

use super::*;

fn id(v: u32) -> ObjectId {
    ObjectId::new(v).unwrap()
}

fn attachment(tex: u32) -> ColorAttachmentRecord {
    ColorAttachmentRecord {
        texture_id: id(tex),
        load_action: 2,
        store_action: 1,
        clear_color: [0.0, 0.25, 0.5, 1.0],
    }
}

/// A stream exercising every opcode.
fn sample_commands() -> Vec<Command<'static>> {
    vec![
        Command::Nop,
        Command::Hello(Hello {
            min_version: 1,
            max_version: 3,
            features: Features::SHARED_EVENTS,
        }),
        Command::CreateBuffer(CreateBuffer {
            buffer_id: id(1),
            storage_mode: 0,
            cpu_cache_mode: 0,
            hazard_tracking: 0,
            length: 4096,
        }),
        Command::WriteBuffer(WriteBuffer {
            buffer_id: id(1),
            offset: 16,
            data: b"hello",
        }),
        Command::CreateBuffer(CreateBuffer {
            buffer_id: id(2),
            storage_mode: 2,
            cpu_cache_mode: 1,
            hazard_tracking: 2,
            length: 4096,
        }),
        Command::CopyBuffer(CopyBuffer {
            src: id(1),
            dst: id(2),
            src_offset: 0,
            dst_offset: 64,
            size: 128,
        }),
        Command::CreateTexture(CreateTexture {
            texture_id: id(7),
            pixel_format: 80,
            usage: 0x5,
            texture_type: 2,
            storage_mode: 2,
            sample_count: 1,
            width: 640,
            height: 480,
            depth: 1,
            array_length: 1,
            mip_level_count: 1,
        }),
        Command::BeginRenderPass(BeginRenderPass::new(640, 480, &[attachment(7)]).unwrap()),
        Command::Draw(Draw {
            primitive_type: 3,
            vertex_start: 0,
            vertex_count: 3,
            instance_count: 1,
            base_instance: 0,
        }),
        Command::EndRenderPass,
        Command::BeginComputePass,
        Command::Dispatch(Dispatch {
            threadgroups: [4, 2, 1],
            threads_per_threadgroup: [64, 1, 1],
        }),
        Command::EndComputePass,
        Command::SignalEvent(EventValue {
            event_id: id(3),
            value: 42,
        }),
        Command::WaitEvent(EventValue {
            event_id: id(3),
            value: 41,
        }),
        Command::Commit { submission_id: 9 },
        Command::DestroyTexture { texture_id: id(7) },
        Command::DestroyBuffer { buffer_id: id(2) },
    ]
}

fn encode(cmds: &[Command<'_>]) -> Vec<u8> {
    let mut enc = StreamEncoder::new();
    for c in cmds {
        enc.push(c).unwrap();
    }
    enc.finish()
}

#[test]
fn round_trip_all_opcodes() {
    let cmds = sample_commands();
    let bytes = encode(&cmds);
    assert_eq!(bytes.len() % RECORD_ALIGN, 0);
    let decoded = decode_stream(&bytes).unwrap();
    assert_eq!(decoded, cmds);
    // Every opcode is covered by the sample.
    for op in Opcode::ALL {
        assert!(cmds.iter().any(|c| c.opcode() == *op), "{op:?} not covered");
    }
}

#[test]
fn opcode_raw_round_trip_and_zero_invalid() {
    for op in Opcode::ALL {
        assert_eq!(Opcode::from_raw(op.raw()), Some(*op));
    }
    assert_eq!(Opcode::from_raw(0), None);
    assert_eq!(Opcode::from_raw(0xFFFF), None);
}

#[test]
fn write_buffer_is_zero_copy() {
    let bytes = encode(&[Command::WriteBuffer(WriteBuffer {
        buffer_id: id(1),
        offset: 0,
        data: b"abcdef",
    })]);
    let mut it = RecordIter::new(&bytes).unwrap();
    let rec = it.next().unwrap().unwrap();
    let Command::WriteBuffer(w) = Command::decode(&rec).unwrap() else {
        panic!("wrong command");
    };
    let base = bytes.as_ptr() as usize;
    let p = w.data.as_ptr() as usize;
    assert!(p >= base && p + w.data.len() <= base + bytes.len());
    assert_eq!(w.data, b"abcdef");
    assert!(it.next().is_none());
}

#[test]
fn empty_stream_is_valid() {
    let bytes = StreamEncoder::new().finish();
    assert_eq!(bytes.len(), HEADER_LEN);
    assert!(decode_stream(&bytes).unwrap().is_empty());
}

#[test]
fn header_errors() {
    assert_eq!(
        decode_stream(&[]),
        Err(ProtoError::TruncatedHeader { len: 0 })
    );
    let good = StreamEncoder::new().finish();

    let mut b = good.clone();
    b[0] ^= 1;
    assert!(matches!(
        decode_stream(&b),
        Err(ProtoError::BadMagic { .. })
    ));

    let mut b = good.clone();
    b[4] = 2;
    assert_eq!(
        decode_stream(&b),
        Err(ProtoError::UnsupportedVersion {
            found: 2,
            supported: PROTOCOL_VERSION
        })
    );

    let mut b = good.clone();
    b[6] = 1;
    assert_eq!(
        decode_stream(&b),
        Err(ProtoError::ReservedNonZero { offset: 6 })
    );

    let mut b = good;
    b[15] = 1;
    assert_eq!(
        decode_stream(&b),
        Err(ProtoError::ReservedNonZero { offset: 12 })
    );
}

#[test]
fn record_count_larger_than_buffer_rejected_early() {
    let mut b = StreamEncoder::new().finish();
    b[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        decode_stream(&b),
        Err(ProtoError::RecordCountTooLarge { .. })
    ));
}

#[test]
fn unknown_opcode_and_flags() {
    let mut enc = StreamEncoder::new();
    enc.push_raw(0x7777, 0, &[]).unwrap();
    assert_eq!(
        decode_stream(&enc.finish()),
        Err(ProtoError::UnknownOpcode {
            offset: HEADER_LEN,
            raw: 0x7777
        })
    );

    let mut enc = StreamEncoder::new();
    enc.push_raw(Opcode::Nop.raw(), 0x8000, &[]).unwrap();
    assert_eq!(
        decode_stream(&enc.finish()),
        Err(ProtoError::UnknownFlags {
            offset: HEADER_LEN,
            flags: 0x8000
        })
    );
}

#[test]
fn wrong_payload_length_rejected() {
    let mut enc = StreamEncoder::new();
    enc.push_raw(Opcode::DestroyBuffer.raw(), 0, &[1, 0, 0, 0, 0])
        .unwrap();
    assert!(matches!(
        decode_stream(&enc.finish()),
        Err(ProtoError::BadPayloadLength { len: 5, .. })
    ));

    let mut enc = StreamEncoder::new();
    enc.push_raw(Opcode::WriteBuffer.raw(), 0, &[1; 15])
        .unwrap();
    assert!(matches!(
        decode_stream(&enc.finish()),
        Err(ProtoError::BadPayloadLength { len: 15, .. })
    ));
}

#[test]
fn payload_length_overflow_and_too_large() {
    // Declared length larger than the buffer.
    let mut b = encode(&[Command::Nop]);
    b[HEADER_LEN + 4..HEADER_LEN + 8].copy_from_slice(&1024u32.to_le_bytes());
    assert!(matches!(
        decode_stream(&b),
        Err(ProtoError::TruncatedPayload { .. })
    ));
    // Declared length beyond the protocol maximum (would overflow naive u32 padding math).
    b[HEADER_LEN + 4..HEADER_LEN + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        decode_stream(&b),
        Err(ProtoError::PayloadTooLarge { len: u32::MAX, .. })
    ));
    // Encoder refuses to produce oversized payloads.
    let big = vec![0u8; MAX_PAYLOAD_LEN as usize + 1];
    assert!(StreamEncoder::new().push_raw(1, 0, &big).is_err());
}

#[test]
fn nonzero_padding_and_reserved_rejected() {
    let mut b = encode(&[Command::DestroyBuffer { buffer_id: id(1) }]);
    // Payload is 4 bytes, padding occupies bytes 4..8 of the payload area.
    b[HEADER_LEN + 8 + 5] = 0xAA;
    assert_eq!(
        decode_stream(&b),
        Err(ProtoError::ReservedNonZero {
            offset: HEADER_LEN + 8 + 5
        })
    );

    let mut b = encode(&[Command::CreateBuffer(CreateBuffer {
        buffer_id: id(1),
        storage_mode: 0,
        cpu_cache_mode: 0,
        hazard_tracking: 0,
        length: 1,
    })]);
    b[HEADER_LEN + 8 + 7] = 1; // reserved byte after hazard_tracking
    assert_eq!(
        decode_stream(&b),
        Err(ProtoError::ReservedNonZero {
            offset: HEADER_LEN + 8 + 7
        })
    );
}

#[test]
fn zero_object_id_rejected() {
    let mut enc = StreamEncoder::new();
    enc.push_raw(Opcode::DestroyTexture.raw(), 0, &[0; 4])
        .unwrap();
    assert!(matches!(
        decode_stream(&enc.finish()),
        Err(ProtoError::InvalidField {
            field: "texture_id",
            ..
        })
    ));
}

#[test]
fn render_pass_attachment_count_must_match_tail() {
    let rp = BeginRenderPass::new(8, 8, &[attachment(1), attachment(2)]).unwrap();
    let mut payload = Vec::new();
    Command::BeginRenderPass(rp).encode_payload(&mut payload);
    // Claim 3 attachments while only 2 follow.
    payload[8..12].copy_from_slice(&3u32.to_le_bytes());
    let mut enc = StreamEncoder::new();
    enc.push_raw(Opcode::BeginRenderPass.raw(), 0, &payload)
        .unwrap();
    assert!(matches!(
        decode_stream(&enc.finish()),
        Err(ProtoError::InvalidField {
            field: "color_attachments",
            ..
        })
    ));
    // Claim more than the maximum.
    payload[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut enc = StreamEncoder::new();
    enc.push_raw(Opcode::BeginRenderPass.raw(), 0, &payload)
        .unwrap();
    assert!(matches!(
        decode_stream(&enc.finish()),
        Err(ProtoError::InvalidField {
            field: "color_attachment_count",
            ..
        })
    ));
    assert!(BeginRenderPass::new(1, 1, &[attachment(1); MAX_COLOR_ATTACHMENTS + 1]).is_none());
}

#[test]
fn trailing_bytes_rejected() {
    let mut b = encode(&[Command::Nop]);
    b.extend_from_slice(&[0; 8]);
    assert_eq!(
        decode_stream(&b),
        Err(ProtoError::TrailingBytes {
            offset: HEADER_LEN + 8,
            len: 8
        })
    );
}

#[test]
fn iterator_is_fused_after_error() {
    let mut enc = StreamEncoder::new();
    enc.push_raw(0x7777, 0, &[]).unwrap();
    enc.push(&Command::Nop).unwrap();
    let bytes = enc.finish();
    let mut it = RecordIter::new(&bytes).unwrap();
    assert!(it.next().unwrap().is_err());
    assert!(it.next().is_none());
    assert!(it.next().is_none());
}

#[test]
fn every_truncation_is_an_error_never_a_panic() {
    let bytes = encode(&sample_commands());
    for cut in 0..bytes.len() {
        assert!(
            decode_stream(&bytes[..cut]).is_err(),
            "prefix of length {cut} decoded successfully"
        );
    }
    assert!(decode_stream(&bytes).is_ok());
}

#[test]
fn every_single_bit_flip_never_panics() {
    let bytes = encode(&sample_commands());
    let mut b = bytes.clone();
    let mut ok = 0usize;
    for i in 0..b.len() {
        for bit in 0..8 {
            b[i] ^= 1 << bit;
            // Only requirement: no panic. Some flips (e.g. in data/ids) remain valid.
            if decode_stream(&b).is_ok() {
                ok += 1;
            }
            b[i] ^= 1 << bit;
        }
    }
    assert_eq!(b, bytes);
    assert!(ok < bytes.len() * 8, "bit flips must be detected sometimes");
}

#[test]
fn pseudo_random_garbage_never_panics() {
    // xorshift64*; deterministic so failures are reproducible.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for _ in 0..20_000 {
        let len = (next() % 256) as usize;
        let mut buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        // Half the cases get a valid header so the record decoder is exercised too.
        if next() % 2 == 0 && buf.len() >= HEADER_LEN {
            let count = (next() % 8) as u32;
            let h = Header {
                version: PROTOCOL_VERSION,
                record_count: count,
            };
            buf[..HEADER_LEN].copy_from_slice(&h.to_bytes());
            // Bias opcodes towards known values.
            let mut pos = HEADER_LEN;
            while pos + 8 <= buf.len() {
                let op = Opcode::ALL[(next() as usize) % Opcode::ALL.len()];
                buf[pos..pos + 2].copy_from_slice(&op.raw().to_le_bytes());
                buf[pos + 2..pos + 4].copy_from_slice(&[0, 0]);
                let plen = (next() % 48) as u32;
                buf[pos + 4..pos + 8].copy_from_slice(&plen.to_le_bytes());
                pos += 8 + (plen as usize).div_ceil(8) * 8;
            }
        }
        let _ = decode_stream(&buf);
    }
}

#[path = "tests_session.rs"]
mod session;

#[test]
fn proto_error_converts_to_guest_error() {
    let e: ritsu_core::Error = ProtoError::BadMagic { found: 0 }.into();
    assert!(matches!(e, ritsu_core::Error::Guest(_)));
}
