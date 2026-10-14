//! `ritsu_gpu_proto::decode_reply_stream` (host -> guest channel) on arbitrary bytes.
//!
//! Invariants: never panics; an accepted reply stream re-encodes (with its header version) and
//! decodes to the identical replies.

use ritsu_gpu_proto::{ReplyEncoder, decode_reply_stream};

/// One fuzz iteration; panics on any violated invariant.
pub fn run(data: &[u8]) {
    let Ok(replies) = decode_reply_stream(data) else {
        return;
    };
    let mut enc = ReplyEncoder::new();
    for r in &replies {
        enc.push(r).expect("a decoded reply must be encodable");
    }
    let again = decode_reply_stream(&enc.finish()).expect("re-encoded replies must decode");
    assert_eq!(replies, again);
}
