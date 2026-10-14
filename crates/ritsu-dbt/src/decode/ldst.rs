//! Loads and stores (`op0 = x1x0`), integer registers only (`V = 0`).
//!
//! FP/SIMD forms (`V = 1`, WP-DBT-10), exclusives and acquire/release (`LDXR`,
//! `LDAR`, `LDAPR`, `STLUR`, …), LSE atomics (WP-DBT-09), `LDRAA/LDRAB` (WP-DBT-11),
//! memory tagging and FEAT_LS64/MOPS/RCpc3 stay [`Inst::Unknown`].

use super::{
    AddrMode, Extend, Inst, LdStOp, LiteralOp, LoadStore, LoadStorePair, PairMode, PairOp, bit,
    bits, branch_offset, sext, sp, zr,
};
use crate::cpu::GpReg;
use crate::sem::Ty;

#[inline(never)]
pub(super) fn decode(word: u32) -> Inst {
    if bit(word, 26) {
        return Inst::Unknown(word); // V = 1: SIMD&FP
    }
    match bits(word, 27, 3) {
        // Load register (literal): opc:2 011 V 00 imm19 Rt
        0b011 if bits(word, 24, 2) == 0b00 => decode_literal(word),
        // Load/store pair: opc:2 101 V type:3 L imm7 Rt2 Rn Rt
        0b101 => decode_pair(word),
        // Load/store register: size:2 111 V op:2 ...
        0b111 => match bits(word, 24, 2) {
            0b01 => decode_unsigned(word),
            0b00 if !bit(word, 21) => decode_imm9(word),
            0b00 if bits(word, 10, 2) == 0b10 => decode_register(word),
            _ => Inst::Unknown(word),
        },
        _ => Inst::Unknown(word),
    }
}

/// `opc:2 011 0 00 imm19:19 Rt:5`
fn decode_literal(word: u32) -> Inst {
    let op = match bits(word, 30, 2) {
        0b00 => LiteralOp::Ldr32,
        0b01 => LiteralOp::Ldr64,
        0b10 => LiteralOp::Ldrsw,
        _ => LiteralOp::Prfm,
    };
    Inst::LoadLiteral {
        op,
        rt: zr(word, 0),
        offset: branch_offset(bits(word, 5, 19), 19),
    }
}

/// Which operation `size:opc` encodes (shared by all single-register forms).
/// `prefetch_ok`: whether `size = 11, opc = 10` is `PRFM/PRFUM` in this form.
fn ldst_op(size: u32, opc: u32, prefetch_ok: bool) -> Option<LdStOp> {
    match (size, opc) {
        (_, 0b00) => Some(LdStOp::Store),
        (_, 0b01) => Some(LdStOp::Load),
        (0b11, 0b10) if prefetch_ok => Some(LdStOp::Prefetch),
        (0b00..=0b10, 0b10) => Some(LdStOp::LoadSigned64),
        (0b00 | 0b01, 0b11) => Some(LdStOp::LoadSigned32),
        _ => None,
    }
}

/// Writeback with the base register also being the transfer register is CONSTRAINED
/// UNPREDICTABLE; Ritsu chooses UNDEFINED (module documentation of `decode`).
fn wback_overlap(rn: GpReg, rt: GpReg) -> bool {
    matches!((rn, rt), (GpReg::X(n), GpReg::X(t)) if n == t)
}

/// `size:2 111 0 01 opc:2 imm12:12 Rn:5 Rt:5`
fn decode_unsigned(word: u32) -> Inst {
    let (size, opc) = (bits(word, 30, 2), bits(word, 22, 2));
    let Some(op) = ldst_op(size, opc, true) else {
        return Inst::Unknown(word);
    };
    Inst::LoadStore(LoadStore {
        op,
        size: Ty::from_size(size),
        rt: zr(word, 0),
        rn: sp(word, 5),
        mode: AddrMode::Unsigned(u64::from(bits(word, 10, 12)) << size),
    })
}

/// `size:2 111 0 00 opc:2 0 imm9:9 op:2 Rn:5 Rt:5` (`op`: 00 unscaled, 01 post-index,
/// 10 unprivileged, 11 pre-index).
fn decode_imm9(word: u32) -> Inst {
    let (size, opc) = (bits(word, 30, 2), bits(word, 22, 2));
    let imm = sext(bits(word, 12, 9), 9);
    let kind = bits(word, 10, 2);
    let Some(op) = ldst_op(size, opc, kind == 0b00) else {
        return Inst::Unknown(word);
    };
    let (rt, rn) = (zr(word, 0), sp(word, 5));
    let mode = match kind {
        0b00 => AddrMode::Unscaled(imm),
        0b01 => AddrMode::PostIndex(imm),
        0b10 => AddrMode::Unprivileged(imm),
        _ => AddrMode::PreIndex(imm),
    };
    if matches!(mode, AddrMode::PostIndex(_) | AddrMode::PreIndex(_)) && wback_overlap(rn, rt) {
        return Inst::Unknown(word);
    }
    Inst::LoadStore(LoadStore {
        op,
        size: Ty::from_size(size),
        rt,
        rn,
        mode,
    })
}

/// `size:2 111 0 00 opc:2 1 Rm:5 option:3 S:1 10 Rn:5 Rt:5`
fn decode_register(word: u32) -> Inst {
    let (size, opc) = (bits(word, 30, 2), bits(word, 22, 2));
    let option = bits(word, 13, 3);
    if option & 0b010 == 0 {
        return Inst::Unknown(word);
    }
    let Some(op) = ldst_op(size, opc, true) else {
        return Inst::Unknown(word);
    };
    Inst::LoadStore(LoadStore {
        op,
        size: Ty::from_size(size),
        rt: zr(word, 0),
        rn: sp(word, 5),
        mode: AddrMode::Register {
            rm: zr(word, 16),
            extend: Extend::from_bits(option),
            shift: bit(word, 12),
        },
    })
}

/// `opc:2 101 0 type:3 L:1 imm7:7 Rt2:5 Rn:5 Rt:5`
fn decode_pair(word: u32) -> Inst {
    let load = bit(word, 22);
    let mode = match bits(word, 23, 3) {
        0b000 => PairMode::NonTemporal,
        0b001 => PairMode::PostIndex,
        0b010 => PairMode::Offset,
        0b011 => PairMode::PreIndex,
        // 1xx: RCpc3 / FEAT_LRCPC3 forms, not reported.
        _ => return Inst::Unknown(word),
    };
    let (op, sf) = match (bits(word, 30, 2), load) {
        (0b00, false) => (PairOp::Store, false),
        (0b00, true) => (PairOp::Load, false),
        // opc = 01, L = 1: LDPSW (not in the non-temporal form); L = 0: STGP (MTE).
        (0b01, true) if mode != PairMode::NonTemporal => (PairOp::LoadSw, true),
        (0b10, false) => (PairOp::Store, true),
        (0b10, true) => (PairOp::Load, true),
        _ => return Inst::Unknown(word),
    };
    let (rt, rt2, rn) = (zr(word, 0), zr(word, 10), sp(word, 5));
    let wback = matches!(mode, PairMode::PreIndex | PairMode::PostIndex);
    if (load && rt == rt2) || (wback && (wback_overlap(rn, rt) || wback_overlap(rn, rt2))) {
        return Inst::Unknown(word);
    }
    let scale = if op == PairOp::Store || op == PairOp::Load {
        if sf { 3 } else { 2 }
    } else {
        2
    };
    Inst::LoadStorePair(LoadStorePair {
        op,
        sf,
        rt,
        rt2,
        rn,
        mode,
        offset: sext(bits(word, 15, 7), 7) << scale,
    })
}
