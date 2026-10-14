//! `ritsu_gpu_proto::decode_stream` on arbitrary bytes.
//!
//! Invariants: never panics; for accepted streams, re-encoding is stable
//! (`encode(decode(encode(decode(x)))) == encode(decode(x))`) and decodes to the same number of
//! commands.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ritsu_gpu_proto::{StreamEncoder, decode_stream};

fn encode(cmds: &[ritsu_gpu_proto::Command<'_>]) -> Vec<u8> {
    let mut enc = StreamEncoder::new();
    for c in cmds {
        enc.push(c).expect("a decoded command must be encodable");
    }
    enc.finish()
}

fuzz_target!(|data: &[u8]| {
    let Ok(cmds) = decode_stream(data) else {
        return;
    };
    let e1 = encode(&cmds);
    let again = decode_stream(&e1).expect("re-encoded stream must decode");
    assert_eq!(
        again.len(),
        cmds.len(),
        "command count changed by roundtrip"
    );
    let e2 = encode(&again);
    assert_eq!(e1, e2, "encode(decode(x)) is not a fixed point");
});
