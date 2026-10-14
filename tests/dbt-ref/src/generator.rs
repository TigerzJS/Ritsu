//! Table-driven vector generator.
//!
//! A *class* is one row of [`CLASSES`]: a name, a stable id and a function that builds a short
//! instruction sequence (a list of [`Item`]s) with random operands. Adding a class for a new
//! instruction group (WP-DBT-02 and later) means adding one function and one table row; existing
//! vectors do not change because every vector's seed is `hash(master seed, class name, index)`
//! (testing.md section 4.5).
//!
//! Execution model (see README.md): the words are placed at `code_base`, followed by `BRK`
//! sentinels; the block ends when `PC` reaches the sentinels (or a fault/`BRK` stops it earlier).
//! All generated branches go **forward** to the start of a later item or to one of the first two
//! sentinel slots, so every vector terminates, and never into the middle of a multi-word item.

use crate::enc::{self, MoveWide};
use crate::format::{Config, MAX_WORDS, Vector};
use crate::handmade;
use crate::rng::{Rng, vector_seed};

/// Version of the generator. Bump on every change that alters generated vectors; golden files
/// record it and are rejected (not silently compared) when it differs.
pub const GEN_VERSION: u32 = 1;
/// Address of the first test word (`code_base`): 256 bytes behind a 64 KiB boundary.
pub const CODE_BASE: u64 = 0x10_0000_0100;
/// Address of the data window.
pub const MEM_BASE: u64 = 0x20_0000_0000;
/// Length of the data window.
pub const MEM_LEN: u32 = 256;
/// Default `--units`: vectors per class = `weight * units`.
pub const DEFAULT_UNITS: u32 = 1000;

/// Config of the standard vector set for `seed`.
#[must_use]
pub fn default_config(seed: u64) -> Config {
    Config {
        seed,
        gen_version: GEN_VERSION,
        code_base: CODE_BASE,
        mem_base: MEM_BASE,
        mem_len: MEM_LEN,
        units: DEFAULT_UNITS,
    }
}

/// How a class produces its vectors.
#[derive(Clone, Copy)]
pub enum Kind {
    /// `weight * units` random vectors built by the function.
    Random {
        /// Vectors per unit.
        weight: u32,
        /// Builds one vector's items and seeds.
        build: fn(&mut Ctx),
    },
    /// Hand-written vectors with hand-computed expectations ([`handmade`]).
    Handmade,
}

/// One row of the class table.
#[derive(Clone, Copy)]
pub struct Class {
    /// Stable id (stored in vector files; never reuse or renumber).
    pub id: u16,
    /// Name; part of every vector's seed.
    pub name: &'static str,
    /// What the class contains.
    pub kind: Kind,
}

impl std::fmt::Debug for Class {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Class({} {})", self.id, self.name)
    }
}

const fn random(id: u16, name: &'static str, weight: u32, build: fn(&mut Ctx)) -> Class {
    Class {
        id,
        name,
        kind: Kind::Random { weight, build },
    }
}

/// The class table. Append new rows; never change the id or name of an existing row.
pub const CLASSES: &[Class] = &[
    Class {
        id: 0,
        name: "handmade",
        kind: Kind::Handmade,
    },
    random(1, "movz", 6, |c| move_wide_single(c, MoveWide::Z)),
    random(2, "movn", 4, |c| move_wide_single(c, MoveWide::N)),
    random(3, "movk", 8, |c| move_wide_single(c, MoveWide::K)),
    random(4, "mov_const_seq", 4, mov_const_seq),
    random(5, "add_imm", 8, |c| add_sub_single(c, false, false)),
    random(6, "adds_imm", 8, |c| add_sub_single(c, false, true)),
    random(7, "sub_imm", 8, |c| add_sub_single(c, true, false)),
    random(8, "subs_imm", 10, |c| add_sub_single(c, true, true)),
    random(9, "b", 3, |c| direct_branch_vector(c, Direct::B)),
    random(10, "bl", 3, |c| direct_branch_vector(c, Direct::Bl)),
    random(11, "br", 4, |c| indirect_branch_vector(c, Indirect::Br)),
    random(12, "blr", 4, |c| indirect_branch_vector(c, Indirect::Blr)),
    random(13, "ret", 3, |c| indirect_branch_vector(c, Indirect::Ret)),
    random(14, "cbz", 4, |c| {
        direct_branch_vector(c, Direct::Cbz(false))
    }),
    random(15, "cbnz", 4, |c| {
        direct_branch_vector(c, Direct::Cbz(true))
    }),
    random(16, "b_cond", 8, |c| direct_branch_vector(c, Direct::Cond)),
    random(17, "cmp_b_cond", 8, cmp_b_cond),
    random(18, "nop", 1, nop_vector),
    random(19, "udf", 1, udf_vector),
    random(20, "brk", 1, brk_vector),
    random(21, "mix_dp", 8, mix_dp),
    random(22, "mix_flow", 10, mix_flow),
];

/// Looks a class up by name.
#[must_use]
pub fn class_by_name(name: &str) -> Option<&'static Class> {
    CLASSES.iter().find(|c| c.name == name)
}

/// Looks a class up by id.
#[must_use]
pub fn class_by_id(id: u16) -> Option<&'static Class> {
    CLASSES.iter().find(|c| c.id == id)
}

/// Number of vectors of `class` for `units`.
#[must_use]
pub fn class_count(class: &Class, units: u32) -> u32 {
    match class.kind {
        Kind::Random { weight, .. } => weight * units,
        Kind::Handmade => handmade::cases().len() as u32,
    }
}

/// Vector `index` of `class` (a pure function of `cfg.seed`, the class name and the index).
///
/// # Panics
///
/// If `index` is out of range for a handmade class.
#[must_use]
pub fn vector_at(cfg: &Config, class: &Class, index: u32) -> Vector {
    match class.kind {
        Kind::Handmade => handmade::cases()[index as usize].to_vector(cfg, class.id),
        Kind::Random { build, .. } => {
            let mut rng = Rng(vector_seed(cfg.seed, class.name, u64::from(index)));
            let mem_seed = rng.next_u64();
            let mut ctx = Ctx::new(rng, cfg);
            build(&mut ctx);
            ctx.finish(class.id, mem_seed)
        }
    }
}

/// All vectors of one class.
#[must_use]
pub fn generate_class(cfg: &Config, class: &Class, units: u32) -> Vec<Vector> {
    (0..class_count(class, units))
        .map(|i| vector_at(cfg, class, i))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Operand generation.

const EDGE: [u64; 14] = [
    0,
    1,
    2,
    u64::MAX,
    1 << 63,
    (1 << 63) - 1,
    0xffff_ffff,
    0x8000_0000,
    0x7fff_ffff,
    0x1_0000_0000,
    0xff,
    0xfff,
    0x5555_5555_5555_5555,
    0xaaaa_aaaa_aaaa_aaaa,
];

/// A 64-bit operand value with edge-case bias (testing.md section 4.2 item 3).
pub fn rand_value(rng: &mut Rng) -> u64 {
    match rng.below(100) {
        0..=9 => *rng.pick(&EDGE),
        10..=19 => (1u64 << rng.below(64)).wrapping_add_signed(rng.below(3) as i64 - 1),
        20..=27 => {
            let v = rng.below(256);
            if rng.chance(50) { v.wrapping_neg() } else { v }
        }
        28..=33 => rng.next_u64() & 0xffff_ffff,
        34..=39 => i64::from(rng.next_u64() as u32 as i32) as u64,
        _ => rng.next_u64(),
    }
}

fn rand_imm16(rng: &mut Rng) -> u32 {
    match rng.below(10) {
        0 => 0,
        1 => 0xffff,
        2 => 1,
        3 => 0x8000,
        4 => 0x7fff,
        _ => rng.below(0x1_0000) as u32,
    }
}

fn rand_imm12(rng: &mut Rng) -> u32 {
    match rng.below(10) {
        0 => 0,
        1 => 0xfff,
        2 => 1,
        3 => 0x800,
        4 => rng.below(16) as u32,
        _ => rng.below(0x1000) as u32,
    }
}

// ---------------------------------------------------------------------------------------------
// Sequence construction.

/// Direct branch kinds (target given at layout time).
#[derive(Clone, Copy, Debug)]
pub enum Direct {
    /// `B`
    B,
    /// `BL`
    Bl,
    /// `CBZ` (`false`) / `CBNZ` (`true`)
    Cbz(bool),
    /// `B.cond` with a random condition
    Cond,
}

/// Indirect branch kinds.
#[derive(Clone, Copy, Debug)]
pub enum Indirect {
    /// `BR`
    Br,
    /// `BLR`
    Blr,
    /// `RET`
    Ret,
}

/// One element of a sequence.
#[derive(Clone, Debug)]
pub enum Item {
    /// Straight-line words.
    Plain(Vec<u32>),
    /// A one-word direct branch; the target is chosen at layout.
    Branch(BranchSpec),
    /// An indirect branch. With `setup` it is preceded by `MOVZ`/`MOVK` loading the target; without,
    /// the register is seeded in the initial state (only allowed as the first item).
    Indirect {
        /// Branch kind.
        kind: Indirect,
        /// Register holding the target.
        reg: u32,
        /// Load the target with preceding instructions.
        setup: bool,
    },
}

/// A direct branch whose offset is filled in at layout.
#[derive(Clone, Copy, Debug)]
pub struct BranchSpec {
    /// Kind (for `Cbz`, `rt` and `sf` below apply; for `Cond`, `cond`).
    pub kind: Direct,
    /// `CBZ` register field.
    pub rt: u32,
    /// `CBZ` width.
    pub sf: bool,
    /// `B.cond` condition.
    pub cond: u32,
}

#[derive(Clone, Copy, Debug)]
enum Seed {
    Reg(u32, u64),
    Sp(u64),
}

/// State of one vector under construction.
#[derive(Debug)]
pub struct Ctx<'a> {
    rng: Rng,
    cfg: &'a Config,
    items: Vec<Item>,
    seeds: Vec<Seed>,
    /// Registers possibly written by items so far (bit 31 = `SP`).
    written: u32,
    pool: Option<Vec<u32>>,
}

impl<'a> Ctx<'a> {
    fn new(rng: Rng, cfg: &'a Config) -> Self {
        Self {
            rng,
            cfg,
            items: Vec::new(),
            seeds: Vec::new(),
            written: 0,
            pool: None,
        }
    }

    /// Restricts register choices to a small pool so that items in a sequence depend on each other.
    fn use_pool(&mut self) {
        let size = self.rng.range(3, 6) as usize;
        let mut pool = Vec::new();
        while pool.len() < size {
            let r = if self.rng.chance(10) {
                30
            } else {
                self.rng.below(20) as u32
            };
            if !pool.contains(&r) {
                pool.push(r);
            }
        }
        self.pool = Some(pool);
    }

    /// A register 0..=30 (from the pool if there is one).
    fn reg(&mut self) -> u32 {
        match &self.pool {
            Some(p) => *self.rng.pick(p),
            None => self.rng.below(31) as u32,
        }
    }

    /// A register field 0..=31 (31 = `SP`/`XZR`).
    fn reg31(&mut self) -> u32 {
        if self.rng.chance(12) { 31 } else { self.reg() }
    }

    fn can_seed(&self, reg: u32) -> bool {
        self.written & (1 << reg) == 0
    }

    fn seed_reg(&mut self, reg: u32, value: u64) {
        if reg == 31 {
            if self.can_seed(31) {
                self.seeds.push(Seed::Sp(value));
            }
        } else if self.can_seed(reg) {
            self.seeds.push(Seed::Reg(reg, value));
        }
    }

    fn wrote(&mut self, reg: u32) {
        self.written |= 1 << reg;
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
    }

    fn plain(&mut self, words: Vec<u32>) {
        self.push(Item::Plain(words));
    }

    /// `n` marker instructions (`MOVZ X20+k, #imm`): they show which instructions after a branch
    /// were executed.
    fn markers(&mut self, n: u32) {
        for k in 0..n {
            let imm = 0xa500 + k * 0x11 + 1;
            self.plain(vec![enc::move_wide(MoveWide::Z, true, 0, imm, 20 + k)]);
            self.wrote(20 + k);
        }
    }

    fn finish(mut self, class_id: u16, mem_seed: u64) -> Vector {
        let (words, layout_seeds) = layout(&mut self.rng, self.cfg, &self.items);
        assert!(words.len() <= MAX_WORDS, "sequence too long");
        let cfg = self.cfg;
        let mut x = [0u64; 31];
        for v in &mut x {
            *v = if self.rng.chance(8) {
                cfg.mem_base + self.rng.below(u64::from(cfg.mem_len) / 8) * 8
            } else {
                rand_value(&mut self.rng)
            };
        }
        let mut sp = match self.rng.below(5) {
            0 | 1 => cfg.mem_base + 16 * self.rng.below(u64::from(cfg.mem_len) / 16),
            2 => rand_value(&mut self.rng) & !0xf,
            _ => rand_value(&mut self.rng),
        };
        let nzcv = self.rng.below(16) << 28;
        for s in self.seeds.iter().chain(layout_seeds.iter()) {
            match *s {
                Seed::Reg(r, v) => x[r as usize] = v,
                Seed::Sp(v) => sp = v,
            }
        }
        Vector {
            class_id,
            mem_seed,
            x,
            sp,
            nzcv,
            words,
        }
    }
}

/// Resolves targets and encodes the items. Returns the words and the seeds required by items
/// without setup code.
fn layout(rng: &mut Rng, cfg: &Config, items: &[Item]) -> (Vec<u32>, Vec<Seed>) {
    let lens: Vec<usize> = items
        .iter()
        .map(|it| match it {
            Item::Plain(w) => w.len(),
            Item::Branch(_) => 1,
            Item::Indirect { setup, .. } => {
                if *setup {
                    enc::load_const(0, cfg.code_base).len() + 1
                } else {
                    1
                }
            }
        })
        .collect();
    let mut starts = Vec::with_capacity(items.len());
    let mut total = 0usize;
    for l in &lens {
        starts.push(total);
        total += l;
    }
    let mut words = Vec::with_capacity(total);
    let mut seeds = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let here = starts[i];
        let pick_target = |rng: &mut Rng| -> usize {
            // Later item starts, then the first two sentinel slots.
            let later = &starts[i + 1..];
            let k = rng.below(later.len() as u64 + 2) as usize;
            later
                .get(k)
                .copied()
                .unwrap_or_else(|| total + (k - later.len()))
        };
        match item {
            Item::Plain(w) => words.extend_from_slice(w),
            Item::Branch(b) => {
                let t = pick_target(rng);
                let off = t as i32 - here as i32;
                words.push(match b.kind {
                    Direct::B => enc::b(off),
                    Direct::Bl => enc::bl(off),
                    Direct::Cbz(nz) => enc::cbz(b.sf, nz, off, b.rt),
                    Direct::Cond => enc::b_cond(b.cond, off),
                });
            }
            Item::Indirect { kind, reg, setup } => {
                let t = pick_target(rng);
                let target = cfg.code_base + 4 * t as u64;
                if *setup {
                    let w = enc::load_const(*reg, target);
                    assert_eq!(
                        w.len() + 1,
                        lens[i],
                        "constant length changed with the target"
                    );
                    words.extend(w);
                } else {
                    assert_eq!(i, 0, "seeded indirect branches must be the first item");
                    seeds.push(Seed::Reg(*reg, target));
                }
                words.push(match kind {
                    Indirect::Br => enc::br(*reg),
                    Indirect::Blr => enc::blr(*reg),
                    Indirect::Ret => enc::ret(*reg),
                });
            }
        }
    }
    (words, seeds)
}

// ---------------------------------------------------------------------------------------------
// Item builders.

fn move_wide_item(c: &mut Ctx, op: MoveWide) {
    let sf = c.rng.chance(60);
    let hw = c.rng.below(if sf { 4 } else { 2 }) as u32;
    let imm = rand_imm16(&mut c.rng);
    let rd = c.reg31();
    c.plain(vec![enc::move_wide(op, sf, hw, imm, rd)]);
    if rd != 31 {
        c.wrote(rd);
    }
}

/// `ADD`/`SUB` immediate item; flag-setting forms get operand relations that provoke the
/// interesting flag outcomes (equal operands, borrow, signed overflow, ...).
fn add_sub_item(c: &mut Ctx, sub: bool, flags: bool) {
    let sf = c.rng.chance(60);
    let shift12 = c.rng.chance(25);
    let imm = rand_imm12(&mut c.rng);
    let rn = c.reg31();
    let rd = c.reg31(); // 31 = SP (no flags) or XZR (flags: CMP/CMN)
    if flags {
        seed_relation(c, rn, sf, u64::from(imm) << (12 * u32::from(shift12)));
    }
    c.plain(vec![enc::add_sub_imm(sf, sub, flags, shift12, imm, rn, rd)]);
    if rd != 31 || !flags {
        c.wrote(rd);
    }
}

/// Seeds `rn` with a value related to the immediate operand.
fn seed_relation(c: &mut Ctx, rn: u32, sf: bool, imm: u64) {
    let mask = if sf { u64::MAX } else { 0xffff_ffff };
    let smax = mask >> 1;
    let v = match c.rng.below(11) {
        0 | 1 => imm,
        2 => imm.wrapping_sub(1),
        3 => imm.wrapping_add(1),
        4 => imm.wrapping_neg(),
        5 => smax,
        6 => smax + 1,
        7 => mask,
        8 => 0,
        _ => return,
    } & mask;
    let upper = if !sf && c.rng.chance(50) {
        c.rng.next_u64() & !0xffff_ffff
    } else {
        0
    };
    c.seed_reg(rn, v | upper);
}

fn nop_item(c: &mut Ctx) {
    c.plain(vec![enc::NOP]);
}

/// A random straight-line item (data processing or NOP).
fn dp_item(c: &mut Ctx) {
    match c.rng.below(10) {
        0 => move_wide_item(c, MoveWide::Z),
        1 => move_wide_item(c, MoveWide::N),
        2 | 3 => move_wide_item(c, MoveWide::K),
        4 => {
            let sub = c.rng.chance(50);
            add_sub_item(c, sub, false);
        }
        5..=8 => {
            let sub = c.rng.chance(50);
            add_sub_item(c, sub, true);
        }
        _ => nop_item(c),
    }
}

fn direct_item(c: &mut Ctx, kind: Direct) {
    let sf = c.rng.chance(50);
    let rt = c.reg31();
    let cond = c.rng.below(16) as u32;
    if let Direct::Cbz(_) = kind {
        // Operand: zero, "low half zero, high half set" (W form only sees the low half), or other.
        let v = match c.rng.below(10) {
            0..=3 => 0,
            4 | 5 => (rand_value(&mut c.rng) | 1) << 32,
            6 => 1,
            _ => rand_value(&mut c.rng),
        };
        if rt != 31 {
            // Register field 31 is XZR here (always zero), not SP.
            c.seed_reg(rt, v);
        }
    }
    c.push(Item::Branch(BranchSpec { kind, rt, sf, cond }));
    if matches!(kind, Direct::Bl) {
        c.wrote(30);
    }
}

fn indirect_item(c: &mut Ctx, kind: Indirect, allow_seeded: bool) {
    let reg = match kind {
        Indirect::Ret if c.rng.chance(50) => 30,
        _ => c.reg(),
    };
    let setup = !(allow_seeded && c.rng.chance(50));
    c.push(Item::Indirect { kind, reg, setup });
    c.wrote(reg);
    if matches!(kind, Indirect::Blr) {
        c.wrote(30);
    }
}

// ---------------------------------------------------------------------------------------------
// Class builders.

fn move_wide_single(c: &mut Ctx, op: MoveWide) {
    move_wide_item(c, op);
}

fn mov_const_seq(c: &mut Ctx) {
    let sf = c.rng.chance(70);
    let rd = c.rng.below(31) as u32;
    let halves = if sf { 4 } else { 2 };
    let mut words = Vec::new();
    let first = c.rng.below(halves) as u32;
    let op = if c.rng.chance(30) {
        MoveWide::N
    } else {
        MoveWide::Z
    };
    words.push(enc::move_wide(op, sf, first, rand_imm16(&mut c.rng), rd));
    for hw in 0..halves as u32 {
        if hw != first && c.rng.chance(70) {
            words.push(enc::move_wide(
                MoveWide::K,
                sf,
                hw,
                rand_imm16(&mut c.rng),
                rd,
            ));
        }
    }
    c.plain(words);
}

fn add_sub_single(c: &mut Ctx, sub: bool, flags: bool) {
    add_sub_item(c, sub, flags);
}

fn direct_branch_vector(c: &mut Ctx, kind: Direct) {
    direct_item(c, kind);
    let n = c.rng.range(1, 4) as u32;
    c.markers(n);
}

fn indirect_branch_vector(c: &mut Ctx, kind: Indirect) {
    indirect_item(c, kind, true);
    let n = c.rng.range(1, 4) as u32;
    c.markers(n);
}

fn cmp_b_cond(c: &mut Ctx) {
    let sub = c.rng.chance(70);
    add_sub_item_cmp(c, sub);
    direct_item(c, Direct::Cond);
    let n = c.rng.range(1, 3) as u32;
    c.markers(n);
}

/// A flag-setting compare (`CMP`/`CMN`, or with a destination register).
fn add_sub_item_cmp(c: &mut Ctx, sub: bool) {
    let sf = c.rng.chance(60);
    let shift12 = c.rng.chance(15);
    let imm = rand_imm12(&mut c.rng);
    let rn = c.reg31();
    let rd = if c.rng.chance(80) { 31 } else { c.reg() };
    seed_relation(c, rn, sf, u64::from(imm) << (12 * u32::from(shift12)));
    c.plain(vec![enc::add_sub_imm(sf, sub, true, shift12, imm, rn, rd)]);
    if rd != 31 {
        c.wrote(rd);
    }
}

fn nop_vector(c: &mut Ctx) {
    let n = c.rng.range(1, 3);
    for _ in 0..n {
        nop_item(c);
    }
    let m = c.rng.range(0, 1) as u32;
    c.markers(m);
}

fn udf_vector(c: &mut Ctx) {
    let imm = rand_imm16(&mut c.rng);
    c.plain(vec![enc::udf(imm)]);
    c.markers(1);
}

fn brk_vector(c: &mut Ctx) {
    // Avoid the immediates that Linux itself uses for kernel breakpoints (kprobes 0x004-0x006,
    // fault 0x100, KGDB 0x400, BUG 0x800, KASAN 0x900, UBSAN 0x5500, CFI 0x8000): a user-mode
    // BRK should only raise SIGTRAP, and this keeps it independent of kernel debug hooks.
    let imm = if c.rng.chance(40) {
        *c.rng.pick(&[0, 1, 2, 3, 0x10, 0xff])
    } else {
        c.rng.range(0x1000, 0x4fff) as u32
    };
    c.plain(vec![enc::brk(imm)]);
    c.markers(1);
}

fn mix_dp(c: &mut Ctx) {
    c.use_pool();
    let n = c.rng.range(2, 6);
    for _ in 0..n {
        dp_item(c);
    }
}

fn mix_flow(c: &mut Ctx) {
    c.use_pool();
    let n = c.rng.range(3, 9);
    for i in 0..n {
        match c.rng.below(10) {
            0..=4 => dp_item(c),
            5 => direct_item(c, Direct::B),
            6 => direct_item(c, Direct::Bl),
            7 => {
                let nz = c.rng.chance(50);
                direct_item(c, Direct::Cbz(nz));
            }
            8 => direct_item(c, Direct::Cond),
            _ => {
                let kind = *c.rng.pick(&[Indirect::Br, Indirect::Blr, Indirect::Ret]);
                indirect_item(c, kind, i == 0);
            }
        }
    }
    if c.rng.chance(50) {
        c.markers(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        default_config(0x1234)
    }

    #[test]
    fn class_ids_and_names_are_unique() {
        for (i, a) in CLASSES.iter().enumerate() {
            for b in &CLASSES[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.name, b.name);
            }
        }
    }

    #[test]
    fn vectors_are_deterministic_and_independent_of_other_classes() {
        let cfg = cfg();
        for class in CLASSES {
            let n = class_count(class, 3).min(5);
            for i in 0..n {
                assert_eq!(
                    vector_at(&cfg, class, i),
                    vector_at(&cfg, class, i),
                    "{}",
                    class.name
                );
            }
        }
        let movz = class_by_name("movz").unwrap();
        assert_ne!(vector_at(&cfg, movz, 0), vector_at(&cfg, movz, 1));
        let other = Config { seed: 99, ..cfg };
        assert_ne!(vector_at(&cfg, movz, 0), vector_at(&other, movz, 0));
    }

    #[test]
    fn default_set_has_at_least_1e5_vectors() {
        let total: u32 = CLASSES.iter().map(|c| class_count(c, DEFAULT_UNITS)).sum();
        assert!(total >= 100_000, "{total}");
    }

    /// Every branch lands on an item start inside the block or on one of the first two sentinel
    /// slots, never backwards, never into the middle of an item: checked by simulating the
    /// control flow of the encoded words with an independent mini decoder.
    #[test]
    fn generated_branches_are_forward_and_in_range() {
        let cfg = cfg();
        for class in CLASSES {
            let Kind::Random { .. } = class.kind else {
                continue;
            };
            for i in 0..400 {
                let v = vector_at(&cfg, class, i);
                let n = v.words.len() as i64;
                for (slot, &w) in v.words.iter().enumerate() {
                    let off = match w {
                        _ if w & 0x7c00_0000 == 0x1400_0000 => sign_extend(w & 0x03ff_ffff, 26),
                        _ if w & 0x7e00_0000 == 0x3400_0000 => sign_extend((w >> 5) & 0x7_ffff, 19),
                        _ if w & 0xff00_0010 == 0x5400_0000 => sign_extend((w >> 5) & 0x7_ffff, 19),
                        _ => continue,
                    };
                    let target = slot as i64 + off;
                    assert!(
                        target > slot as i64,
                        "{} #{i}: backward/zero branch",
                        class.name
                    );
                    assert!(
                        target <= n + 1,
                        "{} #{i}: branch beyond the sentinels",
                        class.name
                    );
                }
            }
        }
    }

    fn sign_extend(v: u32, bits: u32) -> i64 {
        let shift = 64 - bits;
        (i64::from(v) << shift) >> shift
    }
}
