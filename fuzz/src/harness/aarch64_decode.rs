//! `ritsu_dbt::decode` on arbitrary 32-bit words (the input is read as little-endian words).
//!
//! Invariants: the decoder is total and deterministic; `Inst::Unknown(w)` carries the word it was
//! given; an unknown word executes as `ExecError::Undefined` and leaves the state unchanged.

use ritsu_dbt::interp::execute;
use ritsu_dbt::{CpuState, ExecError, Inst, decode};

/// One fuzz iteration; panics on any violated invariant.
pub fn run(data: &[u8]) {
    for chunk in data.chunks(4).take(1024) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        let word = u32::from_le_bytes(b);
        let inst = decode(word);
        assert_eq!(
            inst,
            decode(word),
            "decode is not deterministic for {word:#010x}"
        );
        if let Inst::Unknown(w) = inst {
            assert_eq!(w, word, "Unknown carries a different word");
            let mut cpu = CpuState::new(0x1000);
            let before = cpu.clone();
            match execute(&mut cpu, inst) {
                Err(ExecError::Undefined { word: w, .. }) => assert_eq!(w, word),
                other => panic!("unknown word {word:#010x} executed as {other:?}"),
            }
            assert_eq!(cpu, before, "undefined instruction changed the state");
        }
    }
}
