//! Tests of the opcodes added by WP-GPU-24: texture views, samplers, depth/stencil states,
//! render pipelines, the full render pass, deferred store actions and binds. Per opcode:
//! round trip, length rule, every payload truncation, every single bit flip, wire-level field
//! rules.

use super::*;

fn id(v: u32) -> ObjectId {
    ObjectId::new(v).unwrap()
}

/// Record lists borrow their bytes; tests leak small buffers to get `'static` commands.
fn list<R: WireRecord>(records: &[R]) -> RecordList<'static, R> {
    RecordList::parse(encode_records(records).leak()).unwrap()
}

fn color2(index: u8, tex: u32, resolve: Option<u32>) -> ColorAttachment2 {
    ColorAttachment2 {
        index,
        attachment: AttachmentRecord {
            store_action: if resolve.is_some() { 2 } else { 1 },
            resolve: resolve.map(|r| ResolveRecord {
                texture_id: id(r),
                level: 0,
                slice: 0,
                depth_plane: 0,
            }),
            ..AttachmentRecord::new(id(tex), 2, 1)
        },
        clear_color: [0.25, 0.5, 0.75, 1.0],
    }
}

fn pass2() -> BeginRenderPass2<'static> {
    BeginRenderPass2 {
        render_target_width: 640,
        render_target_height: 480,
        render_target_array_length: 0,
        default_raster_sample_count: 0,
        color_attachments: list(&[color2(0, 7, Some(8)), color2(2, 9, None)]),
        depth_attachment: Some(DepthAttachment2 {
            attachment: AttachmentRecord::new(id(10), 2, 0),
            resolve_filter: 0,
            clear_depth: 1.0,
        }),
        stencil_attachment: Some(StencilAttachment2 {
            attachment: AttachmentRecord::new(id(10), 2, 0),
            resolve_filter: 0,
            clear_stencil: 0x80,
        }),
    }
}

fn pipeline() -> CreateRenderPipelineState<'static> {
    let mut blend = PipelineColorRecord::with_format(80);
    blend.blending_enabled = true;
    blend.source_rgb_blend_factor = 4;
    blend.destination_rgb_blend_factor = 5;
    CreateRenderPipelineState {
        pipeline_id: id(30),
        vertex_function: None,
        fragment_function: None,
        depth_pixel_format: 260,
        stencil_pixel_format: 260,
        raster_sample_count: 4,
        input_primitive_topology: 3,
        flags: CreateRenderPipelineState::RASTERIZATION_ENABLED
            | CreateRenderPipelineState::VERTEX_DESCRIPTOR,
        max_vertex_amplification_count: 1,
        color_attachments: list(&[blend, PipelineColorRecord::with_format(0)]),
        vertex_attributes: list(&[
            VertexAttributeRecord {
                index: 0,
                format: 30,
                buffer_index: 0,
                offset: 0,
            },
            VertexAttributeRecord {
                index: 3,
                format: 42,
                buffer_index: 1,
                offset: 12,
            },
        ]),
        vertex_layouts: list(&[VertexLayoutRecord {
            index: 1,
            step_function: 2,
            stride: 16,
            step_rate: 1,
        }]),
    }
}

fn sampler() -> CreateSampler {
    CreateSampler {
        sampler_id: id(20),
        min_filter: 1,
        mag_filter: 1,
        mip_filter: 2,
        address_modes: [2, 2, 0],
        border_color: 1,
        reduction_mode: 0,
        compare_function: 0,
        flags: CreateSampler::NORMALIZED_COORDINATES,
        max_anisotropy: 8,
        lod_min_clamp: 0.0,
        lod_max_clamp: f32::MAX,
        lod_bias: 0.0,
    }
}

fn depth_stencil() -> CreateDepthStencilState {
    CreateDepthStencilState {
        dss_id: id(21),
        depth_compare: 1,
        depth_write_enabled: true,
        front_face_stencil: Some(StencilRecord {
            compare: 7,
            stencil_failure: 0,
            depth_failure: 0,
            depth_stencil_pass: 2,
            read_mask: 0xff,
            write_mask: 0xff,
        }),
        back_face_stencil: None,
    }
}

/// Every opcode added by WP-GPU-24, wire-valid (semantics irrelevant here).
pub(super) fn sample_object_commands() -> Vec<Command<'static>> {
    vec![
        Command::CreateTexture(CreateTexture {
            texture_id: id(9),
            pixel_format: 80,
            usage: 0x5,
            texture_type: 2,
            storage_mode: 2,
            sample_count: 1,
            width: 64,
            height: 64,
            depth: 1,
            array_length: 1,
            mip_level_count: 1,
            swizzle: [2, 1, 0, 5],
        }),
        Command::CreateTextureView(CreateTextureView {
            texture_id: id(11),
            parent_id: id(9),
            pixel_format: 81,
            texture_type: 2,
            swizzle: [5, 4, 3, 2],
            level_start: 0,
            level_count: 1,
            slice_start: 0,
            slice_count: 1,
        }),
        Command::CreateSampler(sampler()),
        Command::CreateDepthStencilState(depth_stencil()),
        Command::CreateRenderPipelineState(pipeline()),
        Command::BeginRenderPass2(pass2()),
        Command::SetRenderPipelineState {
            pipeline_id: id(30),
        },
        Command::SetDepthStencilState {
            dss_id: Some(id(21)),
        },
        Command::SetDepthStencilState { dss_id: None },
        Command::SetVertexBuffer(SetVertexBuffer {
            buffer_id: Some(id(1)),
            index: 0,
            offset: 16,
            attribute_stride: SetVertexBuffer::STRIDE_STATIC,
        }),
        Command::SetVertexTexture(Binding {
            object_id: Some(id(9)),
            index: 1,
        }),
        Command::SetFragmentTexture(Binding {
            object_id: None,
            index: 127,
        }),
        Command::SetVertexSamplerState(Binding {
            object_id: Some(id(20)),
            index: 0,
        }),
        Command::SetFragmentSamplerState(Binding {
            object_id: Some(id(20)),
            index: 15,
        }),
        Command::SetStoreAction(SetStoreAction {
            attachment: AttachmentKind::Color(2),
            store_action: 1,
        }),
        Command::SetStoreAction(SetStoreAction {
            attachment: AttachmentKind::Stencil,
            store_action: 0,
        }),
        Command::EndRenderPass,
        Command::DestroyRenderPipelineState {
            pipeline_id: id(30),
        },
        Command::DestroyDepthStencilState { dss_id: id(21) },
        Command::DestroySampler { sampler_id: id(20) },
        Command::DestroyTexture { texture_id: id(11) },
    ]
}

fn payload(cmd: &Command<'_>) -> Vec<u8> {
    let mut p = Vec::new();
    cmd.encode_payload(&mut p);
    p
}

fn decode_one(op: Opcode, payload: &[u8]) -> Result<Vec<Command<'_>>, ProtoError> {
    // The stream is leaked so the borrowed commands can be returned.
    let mut e = StreamEncoder::new();
    e.push_raw(op.raw(), 0, payload).unwrap();
    decode_stream(e.finish().leak())
}

fn invalid_field(r: Result<Vec<Command<'_>>, ProtoError>) -> &'static str {
    match r {
        Err(ProtoError::InvalidField { field, .. }) => field,
        other => panic!("expected InvalidField, got {other:?}"),
    }
}

#[test]
fn new_opcodes_round_trip_and_have_classes() {
    let cmds = sample_object_commands();
    for c in &cmds {
        let p = payload(c);
        assert!(
            c.opcode().payload_len().accepts(p.len() as u32),
            "{:?}: {} bytes",
            c.opcode(),
            p.len()
        );
        assert_eq!(decode_one(c.opcode(), &p).unwrap(), vec![*c]);
    }
    use OpcodeClass::{Encoder, Resource};
    for (op, class) in [
        (Opcode::CreateTextureView, Resource),
        (Opcode::CreateSampler, Resource),
        (Opcode::DestroySampler, Resource),
        (Opcode::CreateDepthStencilState, Resource),
        (Opcode::DestroyDepthStencilState, Resource),
        (Opcode::CreateRenderPipelineState, Resource),
        (Opcode::DestroyRenderPipelineState, Resource),
        (Opcode::BeginRenderPass2, Encoder),
        (Opcode::SetStoreAction, Encoder),
        (Opcode::SetRenderPipelineState, Encoder),
        (Opcode::SetDepthStencilState, Encoder),
        (Opcode::SetVertexBuffer, Encoder),
        (Opcode::SetVertexTexture, Encoder),
        (Opcode::SetFragmentTexture, Encoder),
        (Opcode::SetVertexSamplerState, Encoder),
        (Opcode::SetFragmentSamplerState, Encoder),
    ] {
        assert_eq!(op.class(), class, "{op:?}");
        assert_eq!(op.required_feature(), None, "{op:?} is part of base v1");
    }
}

#[test]
fn record_list_accessors() {
    let p = pipeline();
    assert_eq!(p.color_attachments.len(), 2);
    assert_eq!(
        p.vertex_attributes
            .iter()
            .map(|a| a.index)
            .collect::<Vec<_>>(),
        [0, 3]
    );
    assert_eq!(p.vertex_layouts.iter().next().unwrap().stride, 16);
    assert!(RecordList::<VertexLayoutRecord>::empty().is_empty());
    assert!(format!("{:?}", p.vertex_attributes).contains("format: 42"));
    // Standalone parsing checks lengths, indices and entry counts.
    let one = encode_records(&[VertexLayoutRecord {
        index: 0,
        step_function: 1,
        stride: 4,
        step_rate: 1,
    }]);
    assert!(RecordList::<VertexLayoutRecord>::parse(&one[..23]).is_err());
    let too_many =
        encode_records(&[PipelineColorRecord::with_format(80); MAX_COLOR_ATTACHMENTS + 1]);
    assert!(RecordList::<PipelineColorRecord>::parse(&too_many).is_err());
}

#[test]
fn create_texture_swizzle_extension() {
    let Command::CreateTexture(base) = sample_commands()[6] else {
        panic!("sample layout changed");
    };
    assert_eq!(base.swizzle, SWIZZLE_IDENTITY);
    let plain = payload(&Command::CreateTexture(base));
    assert_eq!(plain.len(), 32, "identity keeps the 32-byte layout");
    let swz = CreateTexture {
        swizzle: [5, 1, 0, 2],
        ..base
    };
    let ext = payload(&Command::CreateTexture(swz));
    assert_eq!(ext.len(), 40);
    assert_eq!(&ext[..32], &plain[..]);
    assert_eq!(
        decode_one(Opcode::CreateTexture, &ext).unwrap(),
        vec![Command::CreateTexture(swz)]
    );
    // Only 32 and 40 bytes are valid.
    for len in [31usize, 33, 36, 39, 41, 48] {
        let mut p = ext.clone();
        p.resize(len, 0);
        assert!(
            matches!(
                decode_one(Opcode::CreateTexture, &p),
                Err(ProtoError::BadPayloadLength { .. })
            ),
            "{len}"
        );
    }
    // Reserved bytes of the extension must be zero.
    let mut p = ext.clone();
    p[39] = 1;
    assert!(matches!(
        decode_one(Opcode::CreateTexture, &p),
        Err(ProtoError::ReservedNonZero { offset }) if offset == HEADER_LEN + 8 + 39
    ));
    // An explicit identity extension decodes to the same command as the short form, which
    // the encoder then emits (canonical form).
    let mut p = plain.clone();
    p.extend_from_slice(&[2, 3, 4, 5, 0, 0, 0, 0]);
    assert_eq!(
        decode_one(Opcode::CreateTexture, &p).unwrap(),
        vec![Command::CreateTexture(base)]
    );
}

#[test]
fn every_payload_truncation_of_new_opcodes_is_rejected() {
    for c in sample_object_commands() {
        let p = payload(&c);
        let op = c.opcode();
        for cut in 0..p.len() {
            let r = decode_one(op, &p[..cut]);
            let valid_short = op == Opcode::CreateTexture && cut == 32;
            assert_eq!(r.is_ok(), valid_short, "{op:?} cut to {cut}: {r:?}");
        }
        // One byte too many is rejected as well.
        let mut long = p.clone();
        long.push(0);
        assert!(decode_one(op, &long).is_err(), "{op:?} + 1 byte");
    }
}

#[test]
fn every_bit_flip_of_new_opcodes_never_panics_and_reencodes_stably() {
    for c in sample_object_commands() {
        let p = payload(&c);
        let op = c.opcode();
        let mut b = p.clone();
        for i in 0..b.len() {
            for bit in 0..8 {
                b[i] ^= 1 << bit;
                if let Ok(cmds) = decode_one(op, &b) {
                    // Accepted payloads re-encode to a fixed point.
                    let mut e1 = Vec::new();
                    cmds[0].encode_payload(&mut e1);
                    let again = decode_one(op, &e1).unwrap();
                    let mut e2 = Vec::new();
                    again[0].encode_payload(&mut e2);
                    assert_eq!(e1, e2, "{op:?} byte {i} bit {bit}");
                }
                b[i] ^= 1 << bit;
            }
        }
        assert_eq!(b, p);
    }
}

#[test]
fn zero_ids_are_rejected_where_an_object_is_required() {
    let cases: [(Command<'static>, usize, &str); 6] = [
        (
            Command::CreateTextureView(match sample_object_commands()[1] {
                Command::CreateTextureView(v) => v,
                _ => unreachable!(),
            }),
            4,
            "parent_id",
        ),
        (Command::CreateSampler(sampler()), 0, "sampler_id"),
        (
            Command::CreateDepthStencilState(depth_stencil()),
            0,
            "dss_id",
        ),
        (
            Command::CreateRenderPipelineState(pipeline()),
            0,
            "pipeline_id",
        ),
        (
            Command::SetRenderPipelineState {
                pipeline_id: id(30),
            },
            0,
            "pipeline_id",
        ),
        (Command::BeginRenderPass2(pass2()), 24, "texture_id"),
    ];
    for (cmd, at, field) in cases {
        let mut p = payload(&cmd);
        p[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(invalid_field(decode_one(cmd.opcode(), &p)), field);
    }
    // Optional ids: 0 is nil.
    let p = 0u32.to_le_bytes();
    assert_eq!(
        decode_one(Opcode::SetDepthStencilState, &p).unwrap(),
        vec![Command::SetDepthStencilState { dss_id: None }]
    );
}

#[test]
fn render_pass2_wire_rules() {
    let good = payload(&Command::BeginRenderPass2(pass2()));
    assert_eq!(good.len(), 24 + 2 * 72 + 48 + 48);
    let op = Opcode::BeginRenderPass2;
    // Colour indices must be strictly increasing and below 8.
    let mut p = good.clone();
    p[24 + 72 + 6] = 0; // second attachment index 2 -> 0
    assert_eq!(invalid_field(decode_one(op, &p)), "color_attachments");
    let mut p = good.clone();
    p[24 + 72 + 6] = 8;
    assert_eq!(invalid_field(decode_one(op, &p)), "color_attachments");
    // Too many colour attachments, tail mismatch, unknown flags.
    let mut p = good.clone();
    p[16] = 9;
    assert_eq!(invalid_field(decode_one(op, &p)), "color_count");
    let mut p = good.clone();
    p[16] = 1;
    assert_eq!(invalid_field(decode_one(op, &p)), "render_pass_tail");
    let mut p = good.clone();
    p[17] |= 4;
    assert_eq!(invalid_field(decode_one(op, &p)), "flags");
    // A resolve subresource without a resolve texture is rejected, not ignored.
    let mut p = good.clone();
    p[24 + 72 + 28] = 1; // resolve_level of the attachment without resolve texture
    assert_eq!(invalid_field(decode_one(op, &p)), "resolve_texture_id");
    // Reserved bytes (attachment reserved byte, header reserved, stencil tail).
    for at in [24 + 7, 18, good.len() - 1] {
        let mut p = good.clone();
        p[at] = 1;
        assert!(
            matches!(decode_one(op, &p), Err(ProtoError::ReservedNonZero { offset }) if offset == HEADER_LEN + 8 + at),
            "{at}"
        );
    }
    // A pass without attachments is just the header.
    let empty = BeginRenderPass2 {
        color_attachments: RecordList::empty(),
        depth_attachment: None,
        stencil_attachment: None,
        default_raster_sample_count: 4,
        ..pass2()
    };
    assert_eq!(payload(&Command::BeginRenderPass2(empty)).len(), 24);
}

#[test]
fn pipeline_wire_rules() {
    let good = payload(&Command::CreateRenderPipelineState(pipeline()));
    assert_eq!(good.len(), 32 + 2 * 16 + 2 * 16 + 24);
    let op = Opcode::CreateRenderPipelineState;
    let mut p = good.clone();
    p[19] = 9; // colour count
    assert_eq!(invalid_field(decode_one(op, &p)), "color_count");
    let mut p = good.clone();
    p[24] = 32; // attribute count
    assert_eq!(invalid_field(decode_one(op, &p)), "attribute_count");
    let mut p = good.clone();
    p[18] &= !CreateRenderPipelineState::VERTEX_DESCRIPTOR;
    assert_eq!(invalid_field(decode_one(op, &p)), "vertex_descriptor");
    let mut p = good.clone();
    p[18] |= 0x80;
    assert_eq!(invalid_field(decode_one(op, &p)), "flags");
    let mut p = good.clone();
    p[25] = 0; // layout count 1 -> 0: tail too long
    assert_eq!(invalid_field(decode_one(op, &p)), "pipeline_tail");
    // Attribute indices: duplicate / out of range.
    let mut p = good.clone();
    p[64 + 16] = 0;
    assert_eq!(invalid_field(decode_one(op, &p)), "vertex_attributes");
    let mut p = good.clone();
    p[64 + 16] = 31;
    assert_eq!(invalid_field(decode_one(op, &p)), "vertex_attributes");
    // Unknown blend flag bits, reserved bytes of a colour entry and of a layout.
    let mut p = good.clone();
    p[32 + 2] = 2;
    assert_eq!(invalid_field(decode_one(op, &p)), "blend_flags");
    for at in [32 + 15, 96 + 2, 26, 31] {
        let mut p = good.clone();
        p[at] = 1;
        assert!(
            matches!(decode_one(op, &p), Err(ProtoError::ReservedNonZero { offset }) if offset == HEADER_LEN + 8 + at),
            "{at}"
        );
    }
    // The reference encoder derives the vertex-descriptor flag from the lists, so a
    // hand-built command without it still encodes to a decodable stream.
    let mut no_flag = pipeline();
    no_flag.flags &= !CreateRenderPipelineState::VERTEX_DESCRIPTOR;
    let bytes = payload(&Command::CreateRenderPipelineState(no_flag));
    let decoded = decode_one(op, &bytes).unwrap();
    assert_eq!(
        decoded,
        vec![Command::CreateRenderPipelineState(pipeline())]
    );
    // Function references are carried as given (the host decides; 0 = none).
    let mut with_fn = pipeline();
    with_fn.vertex_function = Some(id(5));
    let cmd = Command::CreateRenderPipelineState(with_fn);
    assert_eq!(decode_one(op, &payload(&cmd)).unwrap(), vec![cmd]);
}

#[test]
fn small_object_wire_rules() {
    // Sampler: unknown flags, reserved bytes.
    let s = payload(&Command::CreateSampler(sampler()));
    assert_eq!(s.len(), 32);
    let mut p = s.clone();
    p[14] = 0x08;
    assert_eq!(
        invalid_field(decode_one(Opcode::CreateSampler, &p)),
        "flags"
    );
    for at in [7usize, 15] {
        let mut p = s.clone();
        p[at] = 1;
        assert!(matches!(
            decode_one(Opcode::CreateSampler, &p),
            Err(ProtoError::ReservedNonZero { .. })
        ));
    }
    // Depth/stencil: an absent face must be all zero; unknown flags.
    let d = payload(&Command::CreateDepthStencilState(depth_stencil()));
    assert_eq!(d.len(), 40);
    let mut p = d.clone();
    p[24 + 4] = 1; // back face absent but read mask set
    assert!(matches!(
        decode_one(Opcode::CreateDepthStencilState, &p),
        Err(ProtoError::ReservedNonZero { .. })
    ));
    let mut p = d.clone();
    p[5] |= 0x10;
    assert_eq!(
        invalid_field(decode_one(Opcode::CreateDepthStencilState, &p)),
        "flags"
    );
    // Store action: attachment kind and index are protocol enums.
    for (kind, index) in [(3u8, 0u8), (1, 1), (2, 7), (0, 8)] {
        let p = [kind, index, 1, 0, 0, 0, 0, 0];
        assert_eq!(
            invalid_field(decode_one(Opcode::SetStoreAction, &p)),
            "attachment",
            "{kind}/{index}"
        );
    }
    let p = [1u8, 0, 1, 0, 0, 0, 0, 0];
    assert_eq!(
        decode_one(Opcode::SetStoreAction, &p).unwrap(),
        vec![Command::SetStoreAction(SetStoreAction {
            attachment: AttachmentKind::Depth,
            store_action: 1
        })]
    );
    // Texture view reserved byte.
    let v = payload(&sample_object_commands()[1]);
    let mut p = v.clone();
    p[11] = 1;
    assert!(matches!(
        decode_one(Opcode::CreateTextureView, &p),
        Err(ProtoError::ReservedNonZero { offset }) if offset == HEADER_LEN + 8 + 11
    ));
}

#[test]
fn decoded_commands_stay_small() {
    // Variable-length parts are borrowed, so a decoded stream costs a bounded amount of host
    // memory per record (no fixed arrays of 31 vertex attributes in every `Command`).
    assert!(
        core::mem::size_of::<Command<'static>>() <= 256,
        "{} bytes",
        core::mem::size_of::<Command<'static>>()
    );
}
