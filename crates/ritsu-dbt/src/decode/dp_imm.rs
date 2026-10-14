//! Data processing — immediate (`op0 = 100x`), sub-class in `word[25:23]`.

use super::{
    AddSubOp, Bitfield, BitfieldOp, Extract, Inst, LogicOp, LogicalImm, MoveWideOp, bit, bits,
    decode_bit_masks, sext, sp, zr,
};

pub(super) fn decode(word: u32) -> Inst {
    match bits(word, 23, 3) {
        0b000 | 0b001 => decode_pc_rel(word),
        0b010 => decode_add_sub_imm(word),
        0b100 => decode_logical_imm(word),
        0b101 => decode_move_wide(word),
        0b110 => decode_bitfield(word),
        0b111 => decode_extract(word),
        // 011: add/subtract immediate with tags (FEAT_MTE) and min/max immediate
        // (FEAT_CSSC): not reported.
        _ => Inst::Unknown(word),
    }
}

/// `op:1 immlo:2 10000 immhi:19 Rd:5`
#[inline(never)]
fn decode_pc_rel(word: u32) -> Inst {
    let imm = sext((bits(word, 5, 19) << 2) | bits(word, 29, 2), 21);
    let page = bit(word, 31);
    Inst::Adr {
        page,
        imm: if page { imm * 4096 } else { imm },
        rd: zr(word, 0),
    }
}

/// `sf:1 op:1 S:1 100010 sh:1 imm12:12 Rn:5 Rd:5`
fn decode_add_sub_imm(word: u32) -> Inst {
    let set_flags = bit(word, 29);
    Inst::AddSubImm {
        op: if bit(word, 30) {
            AddSubOp::Sub
        } else {
            AddSubOp::Add
        },
        sf: bit(word, 31),
        set_flags,
        // 12-bit field always fits.
        imm12: bits(word, 10, 12) as u16,
        shift12: bit(word, 22),
        rd: if set_flags { zr(word, 0) } else { sp(word, 0) },
        rn: sp(word, 5),
    }
}

/// `sf:1 opc:2 100100 N:1 immr:6 imms:6 Rn:5 Rd:5`
#[inline(never)]
fn decode_logical_imm(word: u32) -> Inst {
    let sf = bit(word, 31);
    let n = bit(word, 22);
    if !sf && n {
        return Inst::Unknown(word);
    }
    let datasize = if sf { 64 } else { 32 };
    let Some((imm, _)) = decode_bit_masks(n, bits(word, 10, 6), bits(word, 16, 6), true, datasize)
    else {
        return Inst::Unknown(word);
    };
    let op = match bits(word, 29, 2) {
        0b00 => LogicOp::And,
        0b01 => LogicOp::Orr,
        0b10 => LogicOp::Eor,
        _ => LogicOp::Ands,
    };
    Inst::LogicalImm(LogicalImm {
        op,
        sf,
        // 13-bit field always fits.
        bitmask: bits(word, 10, 13) as u16,
        imm,
        rd: if op == LogicOp::Ands {
            zr(word, 0)
        } else {
            sp(word, 0)
        },
        rn: zr(word, 5),
    })
}

/// `sf:1 opc:2 100101 hw:2 imm16:16 Rd:5`
fn decode_move_wide(word: u32) -> Inst {
    let sf = bit(word, 31);
    let hw = bits(word, 21, 2);
    let op = match bits(word, 29, 2) {
        0b00 => MoveWideOp::Movn,
        0b10 => MoveWideOp::Movz,
        0b11 => MoveWideOp::Movk,
        _ => return Inst::Unknown(word), // opc = 01 is unallocated
    };
    if !sf && hw >= 2 {
        return Inst::Unknown(word); // sf = 0 && hw<1> = 1 is unallocated
    }
    Inst::MoveWide {
        op,
        sf,
        // hw <= 3, so the shift is at most 48.
        shift: (hw * 16) as u8,
        imm16: bits(word, 5, 16) as u16,
        rd: zr(word, 0),
    }
}

/// `sf:1 opc:2 100110 N:1 immr:6 imms:6 Rn:5 Rd:5`
#[inline(never)]
fn decode_bitfield(word: u32) -> Inst {
    let sf = bit(word, 31);
    let n = bit(word, 22);
    let (immr, imms) = (bits(word, 16, 6), bits(word, 10, 6));
    let op = match bits(word, 29, 2) {
        0b00 => BitfieldOp::Sbfm,
        0b01 => BitfieldOp::Bfm,
        0b10 => BitfieldOp::Ubfm,
        _ => return Inst::Unknown(word),
    };
    if sf != n || (!sf && (immr >= 32 || imms >= 32)) {
        return Inst::Unknown(word);
    }
    let datasize = if sf { 64 } else { 32 };
    let Some((wmask, tmask)) = decode_bit_masks(n, imms, immr, false, datasize) else {
        return Inst::Unknown(word);
    };
    Inst::Bitfield(Bitfield {
        op,
        sf,
        // 6-bit fields always fit.
        immr: immr as u8,
        imms: imms as u8,
        wmask,
        tmask,
        rd: zr(word, 0),
        rn: zr(word, 5),
    })
}

/// `sf:1 op21:2 100111 N:1 o0:1 Rm:5 imms:6 Rn:5 Rd:5`
#[inline(never)]
fn decode_extract(word: u32) -> Inst {
    let sf = bit(word, 31);
    let n = bit(word, 22);
    let imms = bits(word, 10, 6);
    if bits(word, 29, 2) != 0 || bit(word, 21) || sf != n || (!sf && imms >= 32) {
        return Inst::Unknown(word);
    }
    Inst::Extract(Extract {
        sf,
        lsb: imms as u8,
        rd: zr(word, 0),
        rn: zr(word, 5),
        rm: zr(word, 16),
    })
}
