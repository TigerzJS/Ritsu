//! Handshake and session decoders of `ritsu-gpu-proto` on arbitrary bytes:
//! `decode_hello_stream` and `decode_session_stream` (with and without the shared-events feature).
//!
//! Invariants: never panics; an accepted HELLO survives an encode/decode roundtrip unchanged;
//! a stream accepted by `decode_session_stream` is also accepted by `decode_stream` with the same
//! number of commands (the session rules only add restrictions).
#![no_main]

use libfuzzer_sys::fuzz_target;
use ritsu_gpu_proto::{
    Command, Features, Negotiated, StreamEncoder, decode_hello_stream, decode_session_stream,
    decode_stream,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(hello) = decode_hello_stream(data) {
        let mut enc = StreamEncoder::new();
        enc.push(&Command::Hello(hello))
            .expect("a decoded HELLO must be encodable");
        let again = decode_hello_stream(&enc.finish()).expect("re-encoded HELLO must decode");
        assert_eq!(hello, again);
    }
    for features in [Features::empty(), Features::KNOWN] {
        let session = Negotiated {
            version: 1,
            features,
        };
        if let Ok(cmds) = decode_session_stream(data, &session) {
            let plain = decode_stream(data).expect("session stream must also be a valid stream");
            assert_eq!(plain.len(), cmds.len());
        }
    }
});
