//! Hand-written vectors with **hand-computed** expected results.
//!
//! These are the independent check of the pipeline itself: the expectation below comes from the
//! Arm ARM pseudocode worked out by hand, not from T0 and not from any recording. A recorded
//! golden file (hardware or qemu) and T0 must both reproduce it.

use crate::enc::{self, MoveWide};
use crate::format::{Config, Outcome, State, Vector, sig};

/// A register in an initial or expected state.
#[derive(Clone, Copy, Debug)]
pub enum Reg {
    /// `X0`-`X30`
    X(usize),
    /// `SP`
    Sp,
}

/// One hand-computed case. The initial state is all zero except `init`; the expected final state
/// is the initial state with `expect` applied.
#[derive(Clone, Debug)]
pub struct Case {
    /// Name for diagnostics.
    pub name: &'static str,
    /// Instruction words.
    pub words: Vec<u32>,
    /// Initial registers.
    pub init: Vec<(Reg, u64)>,
    /// Initial `NZCV` as the 4-bit value `NZCV` (N = bit 3).
    pub init_nzcv: u64,
    /// Expected changes.
    pub expect: Vec<(Reg, u64)>,
    /// Expected `NZCV` nibble.
    pub expect_nzcv: u64,
    /// Expected final `PC` as an offset in bytes from `code_base`.
    pub pc_off: u64,
    /// Expected signal.
    pub signal: u16,
}

impl Case {
    fn new(name: &'static str, words: Vec<u32>) -> Self {
        let pc_off = 4 * words.len() as u64;
        Self {
            name,
            words,
            init: Vec::new(),
            init_nzcv: 0,
            expect: Vec::new(),
            expect_nzcv: 0,
            pc_off,
            signal: sig::TRAP,
        }
    }

    fn init(mut self, r: Reg, v: u64) -> Self {
        self.init.push((r, v));
        self
    }

    fn nzcv(mut self, init: u64, expect: u64) -> Self {
        self.init_nzcv = init;
        self.expect_nzcv = expect;
        self
    }

    fn expect(mut self, r: Reg, v: u64) -> Self {
        self.expect.push((r, v));
        self
    }

    fn ends_at(mut self, pc_off: u64) -> Self {
        self.pc_off = pc_off;
        self
    }

    /// The final PC is an absolute address (outside the block).
    fn set_pc(mut self, pc: u64) -> Self {
        self.pc_off = pc.wrapping_sub(CB);
        self
    }

    fn signal(mut self, s: u16) -> Self {
        self.signal = s;
        self
    }

    /// The vector for this case.
    #[must_use]
    pub fn to_vector(&self, _cfg: &Config, class_id: u16) -> Vector {
        let mut v = Vector {
            class_id,
            mem_seed: 0,
            x: [0; 31],
            sp: 0,
            nzcv: self.init_nzcv << 28,
            words: self.words.clone(),
        };
        apply(&mut v.x, &mut v.sp, &self.init);
        v
    }

    /// The expected outcome, computed by hand.
    #[must_use]
    pub fn expected(&self, cfg: &Config) -> Outcome {
        let mut x = [0u64; 31];
        let mut sp = 0;
        apply(&mut x, &mut sp, &self.init);
        apply(&mut x, &mut sp, &self.expect);
        Outcome {
            signal: self.signal,
            si_code: 0,
            state: State {
                x,
                sp,
                nzcv: self.expect_nzcv << 28,
                pc: cfg.code_base + self.pc_off,
            },
            mem: None,
        }
    }
}

fn apply(x: &mut [u64; 31], sp: &mut u64, list: &[(Reg, u64)]) {
    for &(r, v) in list {
        match r {
            Reg::X(n) => x[n] = v,
            Reg::Sp => *sp = v,
        }
    }
}

const CB: u64 = crate::generator::CODE_BASE;
const ALL: u64 = u64::MAX;
use Reg::{Sp, X};

/// The hand-computed cases. Append only (indices select the vector seed-independent content, and
/// golden files refer to them by index).
#[must_use]
pub fn cases() -> Vec<Case> {
    let movz = |sf, hw, imm, rd| enc::move_wide(MoveWide::Z, sf, hw, imm, rd);
    let movn = |sf, hw, imm, rd| enc::move_wide(MoveWide::N, sf, hw, imm, rd);
    let movk = |sf, hw, imm, rd| enc::move_wide(MoveWide::K, sf, hw, imm, rd);
    let add = |sf, sub, s, sh, imm, rn, rd| enc::add_sub_imm(sf, sub, s, sh, imm, rn, rd);
    // Marker pattern for branch tests: X1 = 1 and X2 = 2 show which of the two MOVZ ran.
    let m1 = movz(true, 0, 1, 1);
    let m2 = movz(true, 0, 2, 2);
    vec![
        // --- move wide ---
        Case::new("movz x0,#1", vec![movz(true, 0, 1, 0)]).expect(X(0), 1),
        Case::new("movz x1,#0xabcd,lsl16", vec![movz(true, 1, 0xabcd, 1)])
            .expect(X(1), 0xabcd_0000),
        Case::new(
            "movz w2,#0xffff,lsl16 zero-extends",
            vec![movz(false, 1, 0xffff, 2)],
        )
        .init(X(2), ALL)
        .expect(X(2), 0xffff_0000),
        Case::new("movn x3,#0", vec![movn(true, 0, 0, 3)]).expect(X(3), ALL),
        Case::new(
            "movn w4,#0x1234 zero-extends",
            vec![movn(false, 0, 0x1234, 4)],
        )
        .init(X(4), ALL)
        .expect(X(4), 0xffff_edcb),
        Case::new("movn x1,#0x1234,lsl32", vec![movn(true, 2, 0x1234, 1)])
            .expect(X(1), 0xffff_edcb_ffff_ffff),
        Case::new("movk x5,#0xdead,lsl32", vec![movk(true, 2, 0xdead, 5)])
            .init(X(5), 0x1111_2222_3333_4444)
            .expect(X(5), 0x1111_dead_3333_4444),
        Case::new(
            "movk w6,#0,lsl16 clears the upper word",
            vec![movk(false, 1, 0, 6)],
        )
        .init(X(6), ALL)
        .expect(X(6), 0x0000_ffff),
        Case::new("movz xzr is a no-op", vec![movz(true, 0, 7, 31)])
            .init(X(0), 5)
            .expect(X(0), 5),
        Case::new(
            "movz+movk builds 0x56781234",
            vec![movz(true, 0, 0x1234, 0), movk(true, 1, 0x5678, 0)],
        )
        .expect(X(0), 0x5678_1234),
        // --- add/sub immediate ---
        Case::new(
            "add x0,x1,#1 wraps, flags untouched",
            vec![add(true, false, false, false, 1, 1, 0)],
        )
        .init(X(1), ALL)
        .nzcv(0b1010, 0b1010)
        .expect(X(0), 0),
        Case::new(
            "adds x0,x1,#1: -1+1 = 0, Z and C",
            vec![add(true, false, true, false, 1, 1, 0)],
        )
        .init(X(1), ALL)
        .expect(X(0), 0)
        .nzcv(0, 0b0110),
        Case::new(
            "subs x0,x1,#1: 0-1 = -1, N, borrow (C=0)",
            vec![add(true, true, true, false, 1, 1, 0)],
        )
        .expect(X(0), ALL)
        .nzcv(0, 0b1000),
        Case::new(
            "subs w0,w1,#1: 0x80000000-1 overflows, V=1",
            vec![add(false, true, true, false, 1, 1, 0)],
        )
        .init(X(1), 0xdead_beef_8000_0000)
        .expect(X(0), 0x7fff_ffff)
        .nzcv(0, 0b0011),
        Case::new(
            "cmp x1,#5 with x1=5: Z, C, no register change",
            vec![add(true, true, true, false, 5, 1, 31)],
        )
        .init(X(1), 5)
        .nzcv(0, 0b0110),
        Case::new(
            "add sp,sp,#0x10",
            vec![add(true, false, false, false, 0x10, 31, 31)],
        )
        .init(Sp, 0x1000)
        .expect(Sp, 0x1010),
        Case::new(
            "sub sp,sp,#1,lsl12",
            vec![add(true, true, false, true, 1, 31, 31)],
        )
        .init(Sp, 0x3000)
        .expect(Sp, 0x2000),
        Case::new(
            "adds x0,x1,#0xfff,lsl12: signed overflow, N and V",
            vec![add(true, false, true, true, 0xfff, 1, 0)],
        )
        .init(X(1), 0x7fff_ffff_ffff_ffff)
        .expect(X(0), 0x8000_0000_00ff_efff)
        .nzcv(0, 0b1001),
        Case::new(
            "add x0,sp,#0 (mov x0,sp)",
            vec![add(true, false, false, false, 0, 31, 0)],
        )
        .init(Sp, 0x7000)
        .expect(X(0), 0x7000),
        Case::new(
            "add w0,wsp,#1 zero-extends",
            vec![add(false, false, false, false, 1, 31, 0)],
        )
        .init(Sp, 0xffff_ffff_ffff_ffff)
        .init(X(0), ALL)
        .expect(X(0), 0),
        // --- branches ---
        Case::new("b +8 skips the first marker", vec![enc::b(2), m1, m2]).expect(X(2), 2),
        Case::new("bl +8 sets x30 = pc+4", vec![enc::bl(2), m1, m2])
            .expect(X(30), CB + 4)
            .expect(X(2), 2),
        Case::new("br x5 to the second marker", vec![enc::br(5), m1, m2])
            .init(X(5), CB + 8)
            .expect(X(2), 2),
        Case::new(
            "blr x30 reads the target before writing the link",
            vec![enc::blr(30), m1, m2],
        )
        .init(X(30), CB + 8)
        .expect(X(30), CB + 4)
        .expect(X(2), 2),
        Case::new("ret x30", vec![enc::ret(30), m1, m2])
            .init(X(30), CB + 8)
            .expect(X(2), 2),
        Case::new("ret x7", vec![enc::ret(7), m1, m2])
            .init(X(7), CB + 8)
            .init(X(30), CB + 4)
            .expect(X(2), 2),
        Case::new("b to the second sentinel slot", vec![enc::b(2)]).ends_at(8),
        Case::new(
            "cbz x1 taken (x1 = 0)",
            vec![enc::cbz(true, false, 2, 1), m1, m2],
        )
        .expect(X(2), 2),
        Case::new(
            "cbz w1 taken: low word zero, high word set",
            vec![enc::cbz(false, false, 2, 1), m1, m2],
        )
        .init(X(1), 0x1_0000_0000)
        .expect(X(2), 2),
        Case::new(
            "cbz x1 not taken (x1 = 1)",
            vec![enc::cbz(true, false, 2, 1), m1, m2],
        )
        .init(X(1), 1)
        .expect(X(1), 1)
        .expect(X(2), 2),
        Case::new(
            "cbnz x1 not taken (x1 = 0)",
            vec![enc::cbz(true, true, 2, 1), m1, m2],
        )
        .expect(X(1), 1)
        .expect(X(2), 2),
        Case::new(
            "cbz xzr is always taken",
            vec![enc::cbz(true, false, 2, 31), m1, m2],
        )
        .expect(X(2), 2),
        Case::new("b.eq taken (Z=1)", vec![enc::b_cond(0, 2), m1, m2])
            .nzcv(0b0100, 0b0100)
            .expect(X(2), 2),
        Case::new("b.eq not taken (Z=0)", vec![enc::b_cond(0, 2), m1, m2])
            .nzcv(0b0000, 0b0000)
            .expect(X(1), 1)
            .expect(X(2), 2),
        Case::new("b.ge taken (N=V=1)", vec![enc::b_cond(10, 2), m1, m2])
            .nzcv(0b1001, 0b1001)
            .expect(X(2), 2),
        Case::new("b.lt taken (N=1, V=0)", vec![enc::b_cond(11, 2), m1, m2])
            .nzcv(0b1000, 0b1000)
            .expect(X(2), 2),
        Case::new("b.hi not taken (C=1, Z=1)", vec![enc::b_cond(8, 2), m1, m2])
            .nzcv(0b0110, 0b0110)
            .expect(X(1), 1)
            .expect(X(2), 2),
        Case::new("b.hi taken (C=1, Z=0)", vec![enc::b_cond(8, 2), m1, m2])
            .nzcv(0b0010, 0b0010)
            .expect(X(2), 2),
        Case::new("b.nv behaves like b.al", vec![enc::b_cond(15, 2), m1, m2]).expect(X(2), 2),
        Case::new("b.al", vec![enc::b_cond(14, 2), m1, m2]).expect(X(2), 2),
        // --- faults raised by the branch target (Linux: SIGSEGV / SIGBUS with PC = target) ---
        Case::new(
            "br to an unmapped address: SIGSEGV at the target",
            vec![enc::br(5), m1],
        )
        .init(X(5), 0x4000_0000_0000)
        .signal(sig::SEGV)
        .set_pc(0x4000_0000_0000),
        Case::new(
            "br to a non-executable page: SIGSEGV at the target",
            vec![enc::br(5), m1],
        )
        .init(X(5), crate::generator::MEM_BASE)
        .signal(sig::SEGV)
        .set_pc(crate::generator::MEM_BASE),
        Case::new(
            "br to a misaligned address: SIGBUS at the target",
            vec![enc::br(5), m1],
        )
        .init(X(5), CB + 6)
        .signal(sig::BUS)
        .set_pc(CB + 6),
        // --- others ---
        Case::new("nop", vec![enc::NOP]),
        Case::new(
            "udf #0 raises SIGILL at the instruction",
            vec![enc::udf(0), m1],
        )
        .signal(sig::ILL)
        .ends_at(0),
        Case::new(
            "brk #1 raises SIGTRAP at the instruction",
            vec![enc::brk(1), m1],
        )
        .signal(sig::TRAP)
        .ends_at(0),
        Case::new(
            "cmp then b.lt: 3 < 5 signed",
            vec![
                add(true, true, true, false, 5, 1, 31),
                enc::b_cond(11, 2),
                m1,
                m2,
            ],
        )
        .init(X(1), 3)
        .nzcv(0, 0b1000)
        .expect(X(2), 2),
    ]
}
