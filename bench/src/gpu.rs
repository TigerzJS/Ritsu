//! GPU protocol workloads (Q-PERF-3): throughput of `ritsu_gpu_proto::decode_stream` and
//! `decode_reply_stream` on streams built with the public encoders.
//!
//! * `mixed`: many small records, as a render/compute frame produces them (per-record overhead).
//! * `bulk`: `WriteBuffer` records with 4 KiB of data (payload handling).
//!
//! Stream layout is fixed; only the payload bytes depend on the seed, so the byte counts used as
//! work units do not.

use ritsu_gpu_proto::{
    BeginRenderPass, ColorAttachmentRecord, Command, CommandBufferStatus, CopyBuffer, CreateBuffer,
    Dispatch, Draw, EventValue, ObjectId, Reply, ReplyEncoder, StreamEncoder, SubmissionStatus,
    WriteBuffer, decode_reply_stream, decode_stream,
};

/// Target size of the generated streams (1 MiB).
pub const STREAM_TARGET_BYTES: usize = 1 << 20;

/// Frames in [`mixed_stream`] (one frame encodes to 688 bytes), chosen to reach the target size.
const MIXED_FRAMES: u32 = 1524;

fn id(raw: u32) -> ObjectId {
    ObjectId::new(raw).expect("non-zero id")
}

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn payload(seed: &mut u64, len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len + 8);
    while v.len() < len {
        v.extend_from_slice(&xorshift(seed).to_le_bytes());
    }
    v.truncate(len);
    v
}

/// A stream of about [`STREAM_TARGET_BYTES`] made of [`MIXED_FRAMES`] 23-record "frames". Returns the encoded
/// bytes and the number of records.
#[must_use]
pub fn mixed_stream(seed: u64) -> (Vec<u8>, usize) {
    let mut s = seed | 1;
    let mut enc = StreamEncoder::new();
    let mut records = 0usize;
    for frame in 1..=MIXED_FRAMES {
        let data = payload(&mut s, 64);
        let cmds = [
            Command::CreateBuffer(CreateBuffer {
                buffer_id: id(1),
                storage_mode: 0,
                cpu_cache_mode: 0,
                hazard_tracking: 0,
                length: 4096,
            }),
            Command::WriteBuffer(WriteBuffer {
                buffer_id: id(1),
                offset: 0,
                data: &data,
            }),
            Command::CopyBuffer(CopyBuffer {
                src: id(1),
                dst: id(2),
                src_offset: 0,
                dst_offset: 16,
                size: 64,
            }),
            Command::BeginRenderPass(
                BeginRenderPass::new(
                    256,
                    256,
                    &[ColorAttachmentRecord {
                        texture_id: id(7),
                        load_action: 2,
                        store_action: 1,
                        clear_color: [0.0, 0.0, 0.0, 1.0],
                    }],
                )
                .expect("one attachment"),
            ),
        ];
        for c in &cmds {
            enc.push(c).expect("encodes");
            records += 1;
        }
        for i in 0..8u32 {
            enc.push(&Command::Draw(Draw {
                primitive_type: 3,
                vertex_start: i * 3,
                vertex_count: 3,
                instance_count: 1,
                base_instance: 0,
            }))
            .expect("encodes");
            records += 1;
        }
        let mid = [Command::EndRenderPass, Command::BeginComputePass];
        for c in &mid {
            enc.push(c).expect("encodes");
            records += 1;
        }
        for i in 0..4u32 {
            enc.push(&Command::Dispatch(Dispatch {
                threadgroups: [8 + i, 8, 1],
                threads_per_threadgroup: [16, 16, 1],
            }))
            .expect("encodes");
            records += 1;
        }
        let tail = [
            Command::EndComputePass,
            Command::SignalEvent(EventValue {
                event_id: id(9),
                value: u64::from(frame),
            }),
            Command::WaitEvent(EventValue {
                event_id: id(9),
                value: u64::from(frame),
            }),
            Command::Commit {
                submission_id: u64::from(frame),
            },
            Command::DestroyBuffer { buffer_id: id(1) },
        ];
        for c in &tail {
            enc.push(c).expect("encodes");
            records += 1;
        }
    }
    (enc.finish(), records)
}

/// A stream of `WriteBuffer` records with 4 KiB of data each, about [`STREAM_TARGET_BYTES`] long.
#[must_use]
pub fn bulk_stream(seed: u64) -> (Vec<u8>, usize) {
    let mut s = seed | 1;
    let mut enc = StreamEncoder::new();
    let n = STREAM_TARGET_BYTES / 4096;
    for i in 0..n {
        let data = payload(&mut s, 4096);
        enc.push(&Command::WriteBuffer(WriteBuffer {
            buffer_id: id(1),
            offset: (i as u64) * 4096,
            data: &data,
        }))
        .expect("encodes");
    }
    (enc.finish(), n)
}

/// A reply stream alternating command buffer completions and event notifications.
#[must_use]
pub fn reply_stream() -> (Vec<u8>, usize) {
    let mut enc = ReplyEncoder::new();
    let n = 16 * 1024;
    for i in 0..n {
        let r = if i % 2 == 0 {
            Reply::CommandBufferStatus(SubmissionStatus {
                submission_id: i as u64,
                status: CommandBufferStatus::Completed,
            })
        } else {
            Reply::EventSignaled(EventValue {
                event_id: id(9),
                value: i as u64,
            })
        };
        enc.push(&r).expect("encodes");
    }
    (enc.finish(), n)
}

/// Decodes `bytes` and returns the number of records (the benchmark's checked result).
#[must_use]
pub fn decode_count(bytes: &[u8]) -> usize {
    decode_stream(bytes)
        .expect("benchmark stream is valid")
        .len()
}

/// Decodes a reply stream and returns the number of replies.
#[must_use]
pub fn decode_reply_count(bytes: &[u8]) -> usize {
    decode_reply_stream(bytes)
        .expect("benchmark reply stream is valid")
        .len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_stream_decodes_to_the_encoded_records() {
        let (bytes, records) = mixed_stream(1);
        assert!(records % 23 == 0, "whole frames");
        assert_eq!(decode_count(&bytes), records);
        // Within 5 % of the 1 MiB target.
        let target = STREAM_TARGET_BYTES as f64;
        assert!(
            (bytes.len() as f64 - target).abs() < 0.05 * target,
            "{} bytes",
            bytes.len()
        );
    }

    #[test]
    fn stream_bytes_are_independent_of_the_seed_in_length() {
        assert_eq!(mixed_stream(1).0.len(), mixed_stream(99).0.len());
        assert_eq!(bulk_stream(1).0.len(), bulk_stream(99).0.len());
        assert_ne!(
            bulk_stream(1).0,
            bulk_stream(99).0,
            "payload follows the seed"
        );
    }

    #[test]
    fn bulk_stream_has_the_hand_computed_size() {
        // 16-byte stream header + 256 * (8-byte record header + 16-byte WriteBuffer header + 4096).
        let (bytes, records) = bulk_stream(1);
        assert_eq!(records, 256);
        assert_eq!(bytes.len(), 16 + 256 * (8 + 16 + 4096));
        assert_eq!(decode_count(&bytes), 256);
    }

    #[test]
    fn reply_stream_decodes() {
        let (bytes, n) = reply_stream();
        assert_eq!(decode_reply_count(&bytes), n);
    }
}
