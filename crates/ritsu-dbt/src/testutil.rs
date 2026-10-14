//! Shared test helpers: deterministic RNG and the instruction-class table used by the
//! decoder roundtrip, the IR/T0 equivalence tests, the static rule check and the LLVM
//! differential test.

/// xorshift64 (deterministic, seedable).
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A random word forced into a class: `(random & !mask) | pattern`.
    pub(crate) fn word_in(&mut self, (mask, pattern): (u32, u32)) -> u32 {
        (self.next() as u32 & !mask) | pattern
    }
}

/// `(mask, pattern)` per decoded instruction class: random words with
/// `word & mask == pattern` cover the class (plus its unallocated neighbours).
pub(crate) const CLASSES: [(&str, (u32, u32)); 34] = [
    ("adr/adrp", (0x1f00_0000, 0x1000_0000)),
    ("add/sub imm", (0x1f80_0000, 0x1100_0000)),
    ("logical imm", (0x1f80_0000, 0x1200_0000)),
    ("move wide", (0x1f80_0000, 0x1280_0000)),
    ("bitfield", (0x1f80_0000, 0x1300_0000)),
    ("extract", (0x1f80_0000, 0x1380_0000)),
    ("logical reg", (0x1f00_0000, 0x0a00_0000)),
    ("add/sub shifted", (0x1f20_0000, 0x0b00_0000)),
    ("add/sub extended", (0x1f20_0000, 0x0b20_0000)),
    ("adc/sbc", (0x1fe0_fc00, 0x1a00_0000)),
    ("ccmp/ccmn", (0x1fe0_0000, 0x1a40_0000)),
    ("csel", (0x1fe0_0000, 0x1a80_0000)),
    ("dp 1/2 source", (0x1fe0_0000, 0x1ac0_0000)),
    ("dp 1/2 source (low opcodes)", (0x1fe0_e000, 0x1ac0_0000)),
    ("dp 3 source", (0x1f00_0000, 0x1b00_0000)),
    ("b/bl", (0x7c00_0000, 0x1400_0000)),
    ("cbz/cbnz", (0x7e00_0000, 0x3400_0000)),
    ("tbz/tbnz", (0x7e00_0000, 0x3600_0000)),
    ("b.cond", (0xff00_0010, 0x5400_0000)),
    ("exception", (0xff00_0000, 0xd400_0000)),
    ("exception (svc/brk shape)", (0xff80_001c, 0xd400_0000)),
    ("hint", (0xffff_f01f, 0xd503_201f)),
    ("mrs/msr nzcv", (0xffdf_ffe0, 0xd51b_4200)),
    ("mrs/msr fpcr", (0xffdf_ffe0, 0xd51b_4400)),
    ("mrs/msr fpsr", (0xffdf_ffe0, 0xd51b_4420)),
    ("mrs/msr tpidr_el0", (0xffdf_ffe0, 0xd51b_d040)),
    ("br/blr/ret", (0xfe9f_fc1f, 0xd61f_0000)),
    ("load literal", (0x3f00_0000, 0x1800_0000)),
    ("ldst unsigned imm", (0x3f00_0000, 0x3900_0000)),
    ("ldst imm9", (0x3f20_0000, 0x3800_0000)),
    ("ldst register", (0x3f20_0c00, 0x3820_0800)),
    ("ldst pair", (0x3e00_0000, 0x2800_0000)),
    ("nop", (0xffff_ffff, 0xd503_201f)),
    ("anything", (0, 0)),
];
