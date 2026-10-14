//! `DecodeBitMasks()` from the Arm ARM shared pseudocode (logical immediates and
//! bitfield masks). Tested exhaustively against a bit-level transcription in
//! `decode/tests.rs` (all `N:immr:imms` for both widths, both modes).

/// `Ones(k)` for `k <= 64`.
const fn ones(k: u32) -> u64 {
    if k >= 64 { u64::MAX } else { (1u64 << k) - 1 }
}

/// Rotates the low `esize` bits of `x` right by `r` (`r < esize`).
const fn ror_elem(x: u64, r: u32, esize: u32) -> u64 {
    let mask = ones(esize);
    let x = x & mask;
    if r == 0 {
        x
    } else {
        ((x >> r) | (x << (esize - r))) & mask
    }
}

/// `Replicate(x, datasize / esize)` for an `esize`-bit element.
const fn replicate(x: u64, esize: u32, datasize: u32) -> u64 {
    let mut out = 0u64;
    let mut pos = 0;
    while pos < datasize {
        out |= x << pos;
        pos += esize;
    }
    out & ones(datasize)
}

/// `DecodeBitMasks(immN, imms, immr, immediate, datasize)`: returns `(wmask, tmask)`,
/// or `None` where the pseudocode is UNDEFINED (`len < 1`, `esize > datasize`, or an
/// all-ones element for logical immediates). `datasize` is 32 or 64.
#[must_use]
pub const fn decode_bit_masks(
    n: bool,
    imms: u32,
    immr: u32,
    immediate: bool,
    datasize: u32,
) -> Option<(u64, u64)> {
    let (imms, immr) = (imms & 0x3f, immr & 0x3f);
    // len = HighestSetBit(immN:NOT(imms))
    let combined = ((n as u32) << 6) | (!imms & 0x3f);
    if combined == 0 {
        return None;
    }
    let len = 31 - combined.leading_zeros();
    if len < 1 {
        return None;
    }
    let esize = 1u32 << len;
    if esize > datasize {
        return None;
    }
    let levels = esize - 1;
    if immediate && (imms & levels) == levels {
        return None;
    }
    let s = imms & levels;
    let r = immr & levels;
    // diff = S - R (6-bit, with borrow); d = UInt(diff<len-1:0>)
    let d = s.wrapping_sub(r) & levels;
    let welem = ones(s + 1);
    let telem = ones(d + 1);
    Some((
        replicate(ror_elem(welem, r, esize), esize, datasize),
        replicate(telem, esize, datasize),
    ))
}
