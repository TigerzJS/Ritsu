//! `ritsu_gpu_host::frontend_g::GuestSession<NullBackend>`: a valid HELLO, then hostile streams.
//!
//! Input format (little-endian, reads past the end yield zeros; seeds: `fuzz/tools/gen_seeds.py`):
//!
//! ```text
//! u8   options: bit 0 = offer SHARED_EVENTS in HELLO, bit 1 = also offer unknown feature bits,
//!               bit 2 = emulate an Iris Xe instead of an RTX 4050
//! then chunks until the input ends (at most MAX_STREAMS): u16 len (mod 4096), len bytes of stream
//! ```
//!
//! Every chunk is passed to `execute_stream`; chunks are usually (parts of) command streams in
//! the wire format, so seeds are real streams.
//!
//! Invariants: no panic; the reply stream of every outcome encodes and decodes with
//! `decode_reply_stream` to exactly the replies of the outcome (the guest can always read what the
//! host says); completions polled later decode too; the pending-command and in-flight counts stay
//! within their documented limits.

use crate::Reader;
use ritsu_gpu_host::backend::NullBackend;
use ritsu_gpu_host::caps::Capabilities;
use ritsu_gpu_host::frontend_g::{
    GuestSession, MAX_IN_FLIGHT_SUBMISSIONS, MAX_PENDING_COMMANDS, StreamOutcome,
};
use ritsu_gpu_proto::{
    Command, Features, Hello, Reply, ReplyEncoder, StreamEncoder, decode_reply_stream,
};

const MAX_STREAMS: usize = 64;

fn check_outcome(out: &StreamOutcome) {
    let bytes = out.reply_stream().expect("reply stream must encode");
    let decoded = decode_reply_stream(&bytes).expect("reply stream must decode");
    assert_eq!(decoded, out.replies, "replies changed on the wire");
}

fn check_replies(replies: &[Reply]) {
    let mut enc = ReplyEncoder::new();
    for r in replies {
        enc.push(r).expect("reply must encode");
    }
    let decoded = decode_reply_stream(&enc.finish()).expect("replies must decode");
    assert_eq!(decoded, replies);
}

/// One fuzz iteration; panics on any violated invariant.
pub fn run(data: &[u8]) {
    let mut r = Reader::new(data);
    let options = r.u8();
    let caps = if options & 4 == 0 {
        Capabilities::rtx4050_assumed()
    } else {
        Capabilities::iris_xe_assumed()
    };
    let mut offered = Features::empty();
    if options & 1 != 0 {
        offered = offered.union(Features::SHARED_EVENTS);
    }
    if options & 2 != 0 {
        offered = offered.union(Features::from_bits_retain(!1));
    }

    let mut session = GuestSession::new(NullBackend::new(caps));
    let mut enc = StreamEncoder::new();
    enc.push(&Command::Hello(Hello {
        min_version: 1,
        max_version: 1,
        features: offered,
    }))
    .expect("HELLO encodes");
    let out = session.execute_stream(&enc.finish());
    assert!(
        matches!(out.replies.as_slice(), [Reply::HelloAck(_)]),
        "valid HELLO not acknowledged"
    );
    check_outcome(&out);

    for _ in 0..MAX_STREAMS {
        if r.exhausted() {
            break;
        }
        let len = usize::from(r.u16() % 4096);
        let out = session.execute_stream(r.bytes(len));
        check_outcome(&out);
        assert!(session.pending_commands() <= MAX_PENDING_COMMANDS);
        assert!(session.in_flight_submissions() <= MAX_IN_FLIGHT_SUBMISSIONS);
        check_replies(&session.poll_completions());
    }
}
