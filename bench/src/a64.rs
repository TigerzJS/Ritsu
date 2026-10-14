//! Tiny A64 assembler for the benchmark programs (the subset `ritsu-dbt` decodes at M0).
//!
//! Encodings are transcribed from the Arm Architecture Reference Manual field layouts, not
//! taken from `ritsu-dbt`: the unit tests below check them against hand-verified words, and
//! `t0::tests` decodes every program word with `ritsu_dbt::decode`.

/// Condition codes used by the programs.
#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum Cond {
    /// Not equal (`Z == 0`).
    Ne = 1,
    /// Unsigned lower (`C == 0`).
    Lo = 3,
}

/// A program under construction: words plus labels with pending references.
#[derive(Debug, Default)]
pub struct Asm {
    words: Vec<u32>,
    labels: Vec<Option<usize>>,
    fixups: Vec<(usize, usize, FixKind)>,
}

#[derive(Clone, Copy, Debug)]
enum FixKind {
    B26,
    Imm19,
}

/// Handle of a label.
#[derive(Clone, Copy, Debug)]
pub struct Label(usize);

impl Asm {
    /// Empty program.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Byte offset of the next instruction.
    #[must_use]
    pub fn here(&self) -> u64 {
        (self.words.len() * 4) as u64
    }

    /// Creates an unbound label.
    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() - 1)
    }

    /// Binds `l` to the next instruction.
    pub fn bind(&mut self, l: Label) {
        assert!(self.labels[l.0].is_none(), "label bound twice");
        self.labels[l.0] = Some(self.words.len());
    }

    /// Byte offset of the bound label `l`.
    #[must_use]
    pub fn offset_of(&self, l: Label) -> u64 {
        (self.labels[l.0].expect("unbound label") * 4) as u64
    }

    fn emit(&mut self, w: u32) {
        self.words.push(w);
    }

    fn emit_ref(&mut self, w: u32, l: Label, kind: FixKind) {
        self.fixups.push((self.words.len(), l.0, kind));
        self.words.push(w);
    }

    /// `MOVZ Xd, #imm16, LSL #(16*hw)`
    pub fn movz(&mut self, rd: u32, imm16: u32, hw: u32) {
        self.emit(0xd280_0000 | (hw << 21) | (imm16 << 5) | rd);
    }

    /// `MOVK Xd, #imm16, LSL #(16*hw)`
    pub fn movk(&mut self, rd: u32, imm16: u32, hw: u32) {
        self.emit(0xf280_0000 | (hw << 21) | (imm16 << 5) | rd);
    }

    /// Loads a 32-bit constant with MOVZ + MOVK (always two instructions).
    pub fn mov32(&mut self, rd: u32, value: u32) {
        self.movz(rd, value & 0xffff, 0);
        self.movk(rd, value >> 16, 1);
    }

    /// `ADD Xd, Xn, #imm12` (`shift12`: `LSL #12`)
    pub fn add_imm(&mut self, rd: u32, rn: u32, imm12: u32, shift12: bool) {
        self.emit(0x9100_0000 | (u32::from(shift12) << 22) | (imm12 << 10) | (rn << 5) | rd);
    }

    /// `SUB Xd, Xn, #imm12`
    pub fn sub_imm(&mut self, rd: u32, rn: u32, imm12: u32) {
        self.emit(0xd100_0000 | (imm12 << 10) | (rn << 5) | rd);
    }

    /// `SUBS Xd, Xn, #imm12` (`rd = 31` is XZR, i.e. `CMP Xn, #imm12`)
    pub fn subs_imm(&mut self, rd: u32, rn: u32, imm12: u32) {
        self.emit(0xf100_0000 | (imm12 << 10) | (rn << 5) | rd);
    }

    /// `B label`
    pub fn b(&mut self, l: Label) {
        self.emit_ref(0x1400_0000, l, FixKind::B26);
    }

    /// `BL label`
    pub fn bl(&mut self, l: Label) {
        self.emit_ref(0x9400_0000, l, FixKind::B26);
    }

    /// `B.cond label`
    pub fn b_cond(&mut self, c: Cond, l: Label) {
        self.emit_ref(0x5400_0000 | c as u32, l, FixKind::Imm19);
    }

    /// `CBZ Xt, label`
    pub fn cbz(&mut self, rt: u32, l: Label) {
        self.emit_ref(0xb400_0000 | rt, l, FixKind::Imm19);
    }

    /// `CBNZ Xt, label`
    pub fn cbnz(&mut self, rt: u32, l: Label) {
        self.emit_ref(0xb500_0000 | rt, l, FixKind::Imm19);
    }

    /// `BR Xn`
    pub fn br(&mut self, rn: u32) {
        self.emit(0xd61f_0000 | (rn << 5));
    }

    /// `RET` (through X30)
    pub fn ret(&mut self) {
        self.emit(0xd65f_03c0);
    }

    /// `NOP`
    pub fn nop(&mut self) {
        self.emit(0xd503_201f);
    }

    /// Resolves all label references and returns the program words.
    ///
    /// Panics on unbound labels or out-of-range displacements (programs are built at start-up
    /// from constants, so this is a bug in the benchmark, not guest input).
    #[must_use]
    pub fn finish(mut self) -> Vec<u32> {
        for &(at, label, kind) in &self.fixups {
            let target = self.labels[label].expect("unbound label");
            let delta = target as i64 - at as i64; // in instructions
            match kind {
                FixKind::B26 => {
                    assert!((-(1 << 25)..(1 << 25)).contains(&delta));
                    self.words[at] |= (delta as u32) & 0x03ff_ffff;
                }
                FixKind::Imm19 => {
                    assert!((-(1 << 18)..(1 << 18)).contains(&delta));
                    self.words[at] |= ((delta as u32) & 0x7_ffff) << 5;
                }
            }
        }
        self.words
    }
}

/// Program words as little-endian bytes.
#[must_use]
pub fn to_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference words assembled by hand from the Arm ARM encodings (and cross-checked with
    // `llvm-mc --triple=aarch64` while writing this file).
    #[test]
    fn encodings_match_hand_assembled_words() {
        let mut a = Asm::new();
        let top = a.label();
        a.bind(top);
        a.movz(0, 0x4240, 0); // movz x0, #0x4240
        a.movk(0, 0xf, 1); // movk x0, #0xf, lsl #16
        a.sub_imm(0, 0, 1); // sub x0, x0, #1
        a.subs_imm(31, 1, 100); // cmp x1, #100
        a.add_imm(5, 1, 1, true); // add x5, x1, #1, lsl #12
        a.b_cond(Cond::Ne, top); // b.ne top   (-20 bytes)
        a.cbz(3, top); // cbz x3, top   (-24 bytes)
        a.br(9);
        a.ret();
        a.nop();
        assert_eq!(
            a.finish(),
            [
                0xd288_4800, // movz x0, #0x4240
                0xf2a0_01e0, // movk x0, #0xf, lsl #16
                0xd100_0400, // sub x0, x0, #1
                0xf101_903f, // cmp x1, #100
                0x9140_0425, // add x5, x1, #1, lsl #12
                0x54ff_ff61, // b.ne -20
                0xb4ff_ff43, // cbz x3, -24
                0xd61f_0120, // br x9
                0xd65f_03c0, // ret
                0xd503_201f, // nop
            ]
        );
    }

    #[test]
    fn forward_branches_resolve() {
        let mut a = Asm::new();
        let end = a.label();
        a.b(end); // b +8
        a.nop();
        a.bind(end);
        a.bl(end); // bl +0
        let w = a.finish();
        assert_eq!(w, [0x1400_0002, 0xd503_201f, 0x9400_0000]);
    }
}
