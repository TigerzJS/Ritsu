//! Operation conformance tests (dbt.md §3.2, level 2): every [`Sem`] operation of the
//! [`Concrete`] backend, driven through the trait, against an independent oracle.
//!
//! The oracles are deliberately written differently from `prim.rs`: 128-bit
//! arithmetic, explicit modulo and two's-complement conversion, bit-by-bit loops for
//! logic and shifts, and the condition table spelled out from the Arm ARM condition
//! code descriptions. A shared mistake would have to be made twice in different ways.

use super::concrete::Concrete;
use super::{
    Align, BinOp, BranchKind, CmpOp, Ctx, DataIss, Exception, MemAttrs, MemOrder, Sem, Ty, UnOp,
};
use crate::cpu::{CpuState, ExceptionLevel, GpReg, Nzcv, SysReg};
use crate::decode::Cond;
use crate::interp::{DataAbort, DataFaultKind, ExecError, StepOutcome};
use crate::mem::{DataBus, MemBus, NoMemory};
use crate::tier::TbFlags;
use ritsu_core::{GuestPhysAddr, GuestVirtAddr};
use ritsu_mem::mmu::{Access, AccessKind, FaultKind, Privilege, Translate, TranslationFault};
use ritsu_mem::{GuestMemory, RamRegion};

// ---------------------------------------------------------------------------------
// Oracles
// ---------------------------------------------------------------------------------

fn modulus(ty: Ty) -> u128 {
    1u128 << ty.bits()
}

/// `v MOD 2^N`.
fn o_trunc(ty: Ty, v: u64) -> u64 {
    (u128::from(v) % modulus(ty)) as u64
}

/// `SInt(v<N-1:0>)`.
fn o_sint(ty: Ty, v: u64) -> i128 {
    let u = i128::from(o_trunc(ty, v));
    if u >= (1i128 << (ty.bits() - 1)) {
        u - (1i128 << ty.bits())
    } else {
        u
    }
}

fn bit(v: u64, i: u32) -> bool {
    (v >> i) & 1 == 1
}

fn from_bits(ty: Ty, f: impl Fn(u32) -> bool) -> u64 {
    (0..ty.bits()).filter(|&i| f(i)).map(|i| 1u64 << i).sum()
}

/// Arm ARM `AddWithCarry()`, literally: unsigned and signed sums in wide arithmetic,
/// `C` and `V` from comparing the truncated result against them.
fn o_add_with_carry(ty: Ty, x: u64, y: u64, cin: bool) -> (u64, Nzcv) {
    let unsigned_sum = u128::from(o_trunc(ty, x)) + u128::from(o_trunc(ty, y)) + u128::from(cin);
    let signed_sum = o_sint(ty, x) + o_sint(ty, y) + i128::from(cin);
    let result = (unsigned_sum % modulus(ty)) as u64;
    let n = bit(result, ty.bits() - 1);
    let z = result == 0;
    let c = u128::from(result) != unsigned_sum;
    let v = o_sint(ty, result) != signed_sum;
    (result, Nzcv::new(n, z, c, v))
}

// The oracle spells division out on purpose (independent of `checked_div`).
#[allow(clippy::manual_checked_ops)]
fn o_binop(op: BinOp, ty: Ty, a: u64, b: u64) -> u64 {
    let w = ty.bits();
    let m = modulus(ty);
    let (a, b) = (o_trunc(ty, a), o_trunc(ty, b));
    // ShiftReg(): UInt(operand2) MOD datasize.
    let n = (u128::from(b) % u128::from(w)) as u32;
    match op {
        BinOp::Add => ((u128::from(a) + u128::from(b)) % m) as u64,
        BinOp::Sub => ((u128::from(a) + m - u128::from(b)) % m) as u64,
        BinOp::And => from_bits(ty, |i| bit(a, i) && bit(b, i)),
        BinOp::Or => from_bits(ty, |i| bit(a, i) || bit(b, i)),
        BinOp::Xor => from_bits(ty, |i| bit(a, i) != bit(b, i)),
        BinOp::Shl => from_bits(ty, |i| i >= n && bit(a, i - n)),
        BinOp::Lshr => from_bits(ty, |i| i + n < w && bit(a, i + n)),
        BinOp::Ashr => from_bits(ty, |i| bit(a, (i + n).min(w - 1))),
        BinOp::Ror => from_bits(ty, |i| bit(a, (i + n) % w)),
        BinOp::Mul => ((u128::from(a) * u128::from(b)) % m) as u64,
        BinOp::UMulHi => ((u128::from(a) * u128::from(b)) / m) as u64,
        BinOp::SMulHi => {
            // floor(product / 2^N), as a two's-complement N-bit pattern.
            let p = o_sint(ty, a) * o_sint(ty, b);
            let hi = p.div_euclid(m as i128);
            hi.rem_euclid(m as i128) as u64
        }
        BinOp::UDiv => {
            if b == 0 {
                0
            } else {
                a / b
            }
        }
        BinOp::SDiv => {
            let (sa, sb) = (o_sint(ty, a), o_sint(ty, b));
            if sb == 0 {
                0
            } else {
                // Real quotient rounded towards zero, then reduced modulo 2^N (the
                // single overflow case MIN / -1 = 2^(N-1) wraps to MIN).
                let q = sa.abs() / sb.abs();
                let q = if (sa < 0) != (sb < 0) { -q } else { q };
                q.rem_euclid(m as i128) as u64
            }
        }
    }
}

fn o_unop(op: UnOp, ty: Ty, a: u64) -> u64 {
    let w = ty.bits();
    let a = o_trunc(ty, a);
    match op {
        UnOp::Clz => u64::from((0..w).rev().take_while(|&i| !bit(a, i)).count() as u32),
        UnOp::Rbit => from_bits(ty, |i| bit(a, w - 1 - i)),
        UnOp::Bswap => {
            if w < 16 {
                a
            } else {
                // Byte k of the result is byte (w/8 - 1 - k) of the operand.
                let n = w / 8;
                (0..n)
                    .map(|k| ((a >> (8 * (n - 1 - k))) & 0xff) << (8 * k))
                    .sum()
            }
        }
    }
}

fn o_icmp(cc: CmpOp, ty: Ty, a: u64, b: u64) -> bool {
    let (ua, ub) = (o_trunc(ty, a), o_trunc(ty, b));
    let (sa, sb) = (o_sint(ty, a), o_sint(ty, b));
    match cc {
        CmpOp::Eq => ua == ub,
        CmpOp::Ne => ua != ub,
        CmpOp::Ult => ua < ub,
        CmpOp::Ule => ua <= ub,
        CmpOp::Ugt => ua > ub,
        CmpOp::Uge => ua >= ub,
        CmpOp::Slt => sa < sb,
        CmpOp::Sle => sa <= sb,
        CmpOp::Sgt => sa > sb,
        CmpOp::Sge => sa >= sb,
    }
}

/// The condition table from the Arm ARM ("Condition code" descriptions).
fn o_cond(c: Cond, n: bool, z: bool, cf: bool, v: bool) -> bool {
    match c {
        Cond::Eq => z,
        Cond::Ne => !z,
        Cond::Cs => cf,
        Cond::Cc => !cf,
        Cond::Mi => n,
        Cond::Pl => !n,
        Cond::Vs => v,
        Cond::Vc => !v,
        Cond::Hi => cf && !z,
        Cond::Ls => !cf || z,
        Cond::Ge => n == v,
        Cond::Lt => n != v,
        Cond::Gt => !z && n == v,
        Cond::Le => z || n != v,
        Cond::Al | Cond::Nv => true,
    }
}

// ---------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Canonical edge values of `ty` plus random ones.
fn samples(ty: Ty, rng: &mut Rng) -> Vec<u64> {
    let max = o_trunc(ty, u64::MAX);
    let smin = 1u64 << (ty.bits() - 1);
    let mut v = vec![
        0,
        1,
        2,
        3,
        max,
        max.wrapping_sub(1),
        smin,
        smin.wrapping_sub(1),
        smin.wrapping_add(1),
        u64::from(ty.bits()),
        u64::from(ty.bits()) + 1,
    ];
    v.extend((0..12).map(|_| rng.next()));
    let mut v: Vec<u64> = v.into_iter().map(|x| o_trunc(ty, x)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn run<R>(cpu: &mut CpuState, f: impl FnOnce(&mut Concrete<'_>) -> R) -> R {
    let mut bus = NoMemory;
    let bus: &mut dyn DataBus = &mut bus;
    let mut c = Concrete::new(cpu, bus);
    f(&mut c)
}

fn gpregs() -> impl Iterator<Item = GpReg> {
    (0..=30).filter_map(GpReg::x).chain([GpReg::Sp, GpReg::Zr])
}

// ---------------------------------------------------------------------------------
// Value operations
// ---------------------------------------------------------------------------------

#[test]
fn imm_truncates_to_type() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        for raw in (0..64).map(|_| rng.next()).chain([0, u64::MAX]) {
            assert_eq!(run(&mut cpu, |c| c.imm(ty, raw)), o_trunc(ty, raw));
        }
    }
}

#[test]
fn pc_rel_is_wrapping_offset_from_current_instruction() {
    for pc in [0u64, 4, 0x4000_0000, u64::MAX - 3, 1 << 63] {
        for off in [0i64, 4, -4, 1 << 20, -(1 << 27), i64::MAX, i64::MIN] {
            let mut cpu = CpuState::new(pc);
            let expect = ((i128::from(pc) + i128::from(off)).rem_euclid(1i128 << 64)) as u64;
            assert_eq!(run(&mut cpu, |c| c.pc_rel(off)), expect, "{pc:#x}{off:+}");
        }
    }
}

#[test]
fn add_with_carry_matches_pseudocode() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        let s = samples(ty, &mut rng);
        for &a in &s {
            for &b in &s {
                for cin in [0u64, 1] {
                    let got = run(&mut cpu, |c| c.add_with_carry(ty, a, b, cin));
                    assert_eq!(
                        got,
                        o_add_with_carry(ty, a, b, cin == 1),
                        "{ty:?} {a:#x} {b:#x} {cin}"
                    );
                }
            }
        }
    }
}

#[test]
fn binop_matches_reference() {
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        let s = samples(ty, &mut rng);
        // Shift amounts: every in-range amount and some beyond (taken MOD width).
        let mut amounts: Vec<u64> = (0..=2 * u64::from(ty.bits()) + 1)
            .map(|x| o_trunc(ty, x))
            .collect();
        amounts.extend(&s);
        for op in BinOp::ALL {
            let rhs = match op {
                BinOp::Shl | BinOp::Lshr | BinOp::Ashr | BinOp::Ror => &amounts,
                _ => &s,
            };
            for &a in &s {
                for &b in rhs {
                    let got = run(&mut cpu, |c| c.binop(op, ty, a, b));
                    assert_eq!(got, o_binop(op, ty, a, b), "{op:?} {ty:?} {a:#x} {b:#x}");
                }
            }
        }
    }
}

#[test]
fn shift_spot_checks() {
    // Hand-computed values, independent of both implementations.
    let mut cpu = CpuState::new(0);
    let mut f = |op, ty, a, b| run(&mut cpu, |c| c.binop(op, ty, a, b));
    assert_eq!(f(BinOp::Shl, Ty::I64, 1, 63), 1 << 63);
    assert_eq!(f(BinOp::Shl, Ty::I64, 1, 64), 1); // LSLV: 64 MOD 64 = 0
    assert_eq!(f(BinOp::Shl, Ty::I32, 0x8000_0001, 1), 2);
    assert_eq!(f(BinOp::Lshr, Ty::I32, 0x8000_0000, 31), 1);
    assert_eq!(f(BinOp::Lshr, Ty::I32, 0x8000_0000, 33), 0x4000_0000);
    assert_eq!(f(BinOp::Ashr, Ty::I32, 0x8000_0000, 31), 0xffff_ffff);
    assert_eq!(f(BinOp::Ashr, Ty::I64, 0x8000_0000, 31), 1);
    assert_eq!(f(BinOp::Ashr, Ty::I8, 0x80, 3), 0xf0);
    assert_eq!(f(BinOp::Ror, Ty::I64, 1, 1), 1 << 63);
    assert_eq!(f(BinOp::Ror, Ty::I32, 0x1234_5678, 8), 0x7812_3456);
    assert_eq!(f(BinOp::Ror, Ty::I16, 0x00ff, 20), 0xf00f); // 20 MOD 16 = 4
    assert_eq!(f(BinOp::Sub, Ty::I32, 0, 1), 0xffff_ffff);
    assert_eq!(f(BinOp::Add, Ty::I8, 0xff, 1), 0);
}

#[test]
fn not_matches_reference() {
    let mut rng = Rng(0x0bad_cafe_0bad_cafe);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        for a in samples(ty, &mut rng) {
            let expect = (modulus(ty) - 1 - u128::from(a)) as u64;
            assert_eq!(run(&mut cpu, |c| c.not(ty, a)), expect);
        }
    }
}

#[test]
fn icmp_matches_reference() {
    let mut rng = Rng(0x5555_aaaa_3333_cccc);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        let s = samples(ty, &mut rng);
        for cc in CmpOp::ALL {
            for &a in &s {
                for &b in &s {
                    let got = run(&mut cpu, |c| c.icmp(cc, ty, a, b));
                    assert_eq!(
                        got,
                        u64::from(o_icmp(cc, ty, a, b)),
                        "{cc:?} {ty:?} {a:#x} {b:#x}"
                    );
                }
            }
        }
    }
}

#[test]
fn select_picks_by_condition() {
    let mut rng = Rng(0x7777_1111_2222_9999);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        let s = samples(ty, &mut rng);
        for (&a, &b) in s.iter().zip(s.iter().rev()) {
            assert_eq!(run(&mut cpu, |c| c.select(ty, 1, a, b)), a);
            assert_eq!(run(&mut cpu, |c| c.select(ty, 0, a, b)), b);
        }
    }
}

#[test]
fn cond_holds_matches_arm_table() {
    let mut cpu = CpuState::new(0);
    for bits in 0..16u32 {
        let (n, z, c, v) = (bits & 8 != 0, bits & 4 != 0, bits & 2 != 0, bits & 1 != 0);
        let f = Nzcv::new(n, z, c, v);
        for cond in Cond::ALL {
            let got = run(&mut cpu, |s| s.cond_holds(cond, f));
            assert_eq!(got, u64::from(o_cond(cond, n, z, c, v)), "{cond:?} {f:?}");
        }
    }
}

// ---------------------------------------------------------------------------------
// Guest state
// ---------------------------------------------------------------------------------

/// A state with every register holding a distinct, recognisable value.
fn painted(el: ExceptionLevel, sp_sel: bool) -> CpuState {
    let mut cpu = CpuState::new(0x4000_0000);
    for (i, x) in cpu.x.iter_mut().enumerate() {
        *x = 0xa5a5_0000_0000_0000 | (i as u64) << 32 | 0x8000_0000 | i as u64;
    }
    cpu.sp_el0 = 0x5a5a_5a5a_dead_0000;
    cpu.sp_el1 = 0x6b6b_6b6b_beef_0000;
    cpu.el = el;
    cpu.sp_sel = sp_sel;
    cpu
}

/// Which 64-bit storage cell `r` names in `cpu`, per Arm ARM `X[]`/`SP[]`.
fn o_cell(cpu: &mut CpuState, r: GpReg) -> Option<&mut u64> {
    match r {
        GpReg::X(x) => cpu.x.get_mut(usize::from(x.index())),
        GpReg::Sp if cpu.el == ExceptionLevel::El1 && cpu.sp_sel => Some(&mut cpu.sp_el1),
        GpReg::Sp => Some(&mut cpu.sp_el0),
        GpReg::Zr => None,
    }
}

const SP_MODES: [(ExceptionLevel, bool); 4] = [
    (ExceptionLevel::El0, false),
    (ExceptionLevel::El0, true),
    (ExceptionLevel::El1, false),
    (ExceptionLevel::El1, true),
];

#[test]
fn gpr_reads_truncate_and_resolve_sp_zr() {
    for (el, sp_sel) in SP_MODES {
        for r in gpregs() {
            for ty in Ty::ALL {
                let mut cpu = painted(el, sp_sel);
                let mut oracle = cpu.clone();
                let expect = o_cell(&mut oracle, r).map_or(0, |c| o_trunc(ty, *c));
                assert_eq!(
                    run(&mut cpu, |c| c.gpr(r, ty)),
                    expect,
                    "{r:?} {ty:?} {el:?}"
                );
            }
        }
    }
}

#[test]
fn gpr_writes_zero_extend_and_touch_only_their_cell() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    for (el, sp_sel) in SP_MODES {
        for r in gpregs() {
            for ty in Ty::ALL {
                let v = o_trunc(ty, rng.next());
                let mut cpu = painted(el, sp_sel);
                let mut expect = cpu.clone();
                if let Some(cell) = o_cell(&mut expect, r) {
                    *cell = v; // canonical value = zero-extended to 64 bits
                }
                run(&mut cpu, |c| c.set_gpr(r, ty, v));
                assert_eq!(cpu, expect, "{r:?} {ty:?} {el:?} {sp_sel}");
            }
        }
    }
}

#[test]
fn nzcv_read_and_write() {
    for bits in 0..16u8 {
        let f = Nzcv::from_nibble(bits);
        let mut cpu = CpuState::new(0);
        cpu.nzcv = f;
        assert_eq!(run(&mut cpu, |c| c.nzcv()), f);
        let mut cpu = CpuState::new(0);
        let before = cpu.clone();
        run(&mut cpu, |c| c.set_nzcv(f));
        assert_eq!(cpu.nzcv, f);
        assert_eq!(
            CpuState {
                nzcv: before.nzcv,
                ..cpu
            },
            before
        );
    }
}

// ---------------------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------------------

const KINDS: [BranchKind; 3] = [BranchKind::Jump, BranchKind::Call, BranchKind::Return];

#[test]
fn no_control_transfer_falls_through() {
    for pc in [0u64, 0x4000_0000, u64::MAX - 3] {
        let mut cpu = CpuState::new(pc);
        let outcome = Concrete::new(&mut cpu, &mut NoMemory).finish();
        assert_eq!(outcome, StepOutcome::Sequential);
        assert_eq!(cpu.pc, pc.wrapping_add(4));
    }
}

#[test]
fn direct_branch_semantics() {
    for pc in [0u64, 0x4000_0000, u64::MAX - 3] {
        for off in [0i64, 8, -8, (1 << 27) - 4, -(1 << 27)] {
            let target = ((i128::from(pc) + i128::from(off)).rem_euclid(1i128 << 64)) as u64;
            for kind in KINDS {
                for (cond, taken) in [(None, true), (Some(1), true), (Some(0), false)] {
                    let mut cpu = CpuState::new(pc);
                    let mut bus = NoMemory;
                    let mut c = Concrete::new(&mut cpu, &mut bus);
                    c.branch(cond, off, kind);
                    assert_eq!(c.finish(), StepOutcome::Branch { taken });
                    let expect = if taken { target } else { pc.wrapping_add(4) };
                    assert_eq!(cpu.pc, expect, "{pc:#x}{off:+} {cond:?} {kind:?}");
                }
            }
        }
    }
}

#[test]
fn indirect_branch_goes_anywhere() {
    for target in [0u64, 2, 0xffff_0000_0000_0001, u64::MAX] {
        for kind in KINDS {
            let mut cpu = CpuState::new(0x4000_0000);
            let mut bus = NoMemory;
            let mut c = Concrete::new(&mut cpu, &mut bus);
            c.branch_indirect(target, kind);
            assert_eq!(c.finish(), StepOutcome::Branch { taken: true });
            // Misaligned/non-canonical targets are taken as-is and fault on fetch.
            assert_eq!(cpu.pc, target);
        }
    }
}

#[test]
fn raise_undefined_reports_pc_and_leaves_state() {
    let mut cpu = painted(ExceptionLevel::El1, true);
    let before = cpu.clone();
    let r = run(&mut cpu, |c| {
        c.raise(Exception::Undefined { word: 0xdead_beef })
    });
    assert_eq!(
        r,
        Err(ExecError::Undefined {
            pc: 0x4000_0000,
            word: 0xdead_beef
        })
    );
    assert_eq!(cpu, before);
}

// ---------------------------------------------------------------------------------
// Description-bug detection (debug builds only)
// ---------------------------------------------------------------------------------

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "not a canonical")]
fn non_canonical_operand_is_caught_in_debug() {
    let mut cpu = CpuState::new(0);
    run(&mut cpu, |c| c.binop(BinOp::Add, Ty::I32, 1 << 32, 0));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "raise after an architectural write")]
fn raise_after_write_is_caught_in_debug() {
    let mut cpu = CpuState::new(0);
    let _ = run(&mut cpu, |c| {
        c.set_gpr(GpReg::Zr, Ty::I64, 0);
        c.raise(Exception::Undefined { word: 0 })
    });
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "two control transfers")]
fn double_branch_is_caught_in_debug() {
    let mut cpu = CpuState::new(0);
    run(&mut cpu, |c| {
        c.branch(None, 4, BranchKind::Jump);
        c.branch_indirect(0, BranchKind::Jump);
    });
}

// ---------------------------------------------------------------------------------
// WP-DBT-02 operations
// ---------------------------------------------------------------------------------

#[test]
fn arithmetic_spot_checks() {
    // Hand-computed Arm results, independent of both implementations.
    let mut cpu = CpuState::new(0);
    let mut f = |op, ty, a, b| run(&mut cpu, |c| c.binop(op, ty, a, b));
    let min64 = 1u64 << 63;
    assert_eq!(f(BinOp::SDiv, Ty::I64, min64, u64::MAX), min64); // INT64_MIN / -1
    assert_eq!(
        f(BinOp::SDiv, Ty::I32, 0x8000_0000, 0xffff_ffff),
        0x8000_0000
    );
    assert_eq!(f(BinOp::SDiv, Ty::I64, 7, 0), 0);
    assert_eq!(f(BinOp::UDiv, Ty::I64, u64::MAX, 0), 0);
    assert_eq!(f(BinOp::SDiv, Ty::I64, -7i64 as u64, 2), -3i64 as u64); // towards zero
    assert_eq!(f(BinOp::SDiv, Ty::I64, 7, -2i64 as u64), -3i64 as u64);
    assert_eq!(f(BinOp::SDiv, Ty::I32, 0xffff_fff9, 2), 0xffff_fffd);
    assert_eq!(f(BinOp::UDiv, Ty::I32, 0xffff_ffff, 2), 0x7fff_ffff);
    assert_eq!(f(BinOp::UMulHi, Ty::I64, u64::MAX, u64::MAX), u64::MAX - 1);
    assert_eq!(f(BinOp::SMulHi, Ty::I64, u64::MAX, u64::MAX), 0); // -1 * -1 = 1
    assert_eq!(f(BinOp::SMulHi, Ty::I64, u64::MAX, 2), u64::MAX); // -2 → high = -1
    assert_eq!(f(BinOp::SMulHi, Ty::I64, min64, min64), 1 << 62);
    assert_eq!(f(BinOp::Mul, Ty::I32, 0x1_0000, 0x1_0000), 0);
}

#[test]
fn unop_matches_reference() {
    let mut rng = Rng(0x3141_5926_5358_9793);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        let mut s = samples(ty, &mut rng);
        s.extend((0..ty.bits()).map(|i| 1u64 << i));
        for op in UnOp::ALL {
            for &a in &s {
                let got = run(&mut cpu, |c| c.unop(op, ty, a));
                assert_eq!(got, o_unop(op, ty, a), "{op:?} {ty:?} {a:#x}");
            }
        }
    }
    let mut f = |op, ty, a| run(&mut cpu, |c| c.unop(op, ty, a));
    assert_eq!(f(UnOp::Clz, Ty::I64, 0), 64);
    assert_eq!(f(UnOp::Clz, Ty::I32, 0), 32);
    assert_eq!(f(UnOp::Clz, Ty::I32, 1), 31);
    assert_eq!(f(UnOp::Rbit, Ty::I32, 1), 0x8000_0000);
    assert_eq!(f(UnOp::Bswap, Ty::I32, 0x1122_3344), 0x4433_2211);
    assert_eq!(f(UnOp::Bswap, Ty::I16, 0x1122), 0x2211);
}

#[test]
fn width_conversions_match_reference() {
    let mut rng = Rng(0x2718_2818_2845_9045);
    let mut cpu = CpuState::new(0);
    for from in Ty::ALL {
        for to in Ty::ALL {
            for v in samples(from, &mut rng) {
                if from.bits() <= to.bits() {
                    assert_eq!(run(&mut cpu, |c| c.zext(from, to, v)), v);
                    let expect = o_sint(from, v).rem_euclid(modulus(to) as i128) as u64;
                    assert_eq!(
                        run(&mut cpu, |c| c.sext(from, to, v)),
                        expect,
                        "sext {from:?}->{to:?} {v:#x}"
                    );
                }
                if from.bits() >= to.bits() {
                    assert_eq!(run(&mut cpu, |c| c.trunc(from, to, v)), o_trunc(to, v));
                }
            }
        }
    }
}

#[test]
fn sub_with_carry_is_add_with_carry_of_not() {
    let mut rng = Rng(0x1618_0339_8874_9894);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        let s = samples(ty, &mut rng);
        for &a in &s {
            for &b in &s {
                for cin in [0u64, 1] {
                    let not_b = (modulus(ty) - 1 - u128::from(b)) as u64;
                    assert_eq!(
                        run(&mut cpu, |c| c.sub_with_carry(ty, a, b, cin)),
                        o_add_with_carry(ty, a, not_b, cin == 1),
                        "{ty:?} {a:#x} {b:#x} {cin}"
                    );
                }
            }
        }
    }
}

#[test]
fn flag_operations() {
    let mut rng = Rng(0x0577_2156_6490_1532);
    let mut cpu = CpuState::new(0);
    for ty in Ty::ALL {
        for r in samples(ty, &mut rng) {
            let expect = Nzcv::new(bit(r, ty.bits() - 1), r == 0, false, false);
            assert_eq!(run(&mut cpu, |c| c.flags_logic(ty, r)), expect);
        }
    }
    for bits in 0..16u8 {
        let f = Nzcv::from_nibble(bits);
        assert_eq!(run(&mut cpu, |c| c.flag_c(f)), u64::from(bits >> 1 & 1));
        let packed = u64::from(bits) << 28;
        assert_eq!(run(&mut cpu, |c| c.flags_to_bits(f)), packed);
        // Bits outside 31:28 are ignored.
        let noisy = packed | 0xffff_ffff_0fff_ffff;
        assert_eq!(run(&mut cpu, |c| c.bits_to_flags(noisy)), f);
        let g = Nzcv::from_nibble(!bits & 0xf);
        assert_eq!(run(&mut cpu, |c| c.select_flags(1, f, g)), f);
        assert_eq!(run(&mut cpu, |c| c.select_flags(0, f, g)), g);
    }
}

#[test]
fn sysreg_read_write() {
    for r in SysReg::ALL {
        let mut cpu = CpuState::new(0);
        let v = 0xa5a5_5a5a_f00d_cafe;
        run(&mut cpu, |c| c.set_sysreg(r, v));
        let mut expect = CpuState::new(0);
        match r {
            SysReg::Nzcv => expect.nzcv = Nzcv::from_sysreg(v),
            SysReg::Fpcr => expect.sys.fpcr = v,
            SysReg::Fpsr => expect.sys.fpsr = v,
            SysReg::TpidrEl0 => expect.sys.tpidr_el0 = v,
        }
        assert_eq!(cpu, expect, "{r:?}");
        let back = run(&mut cpu, |c| c.sysreg(r));
        let want = if r == SysReg::Nzcv {
            v & 0xf000_0000
        } else {
            v
        };
        assert_eq!(back, want);
        assert_eq!(SysReg::from_encoding(r.encoding()), Some(r));
    }
}

#[test]
fn ctx_follows_state_through_tb_flags() {
    const A: u64 = 1 << 1;
    const SA: u64 = 1 << 3;
    const SA0: u64 = 1 << 4;
    const ENIB: u64 = 1 << 30;
    const ENIA: u64 = 1 << 31;
    for el in [ExceptionLevel::El0, ExceptionLevel::El1] {
        for sctlr in 0..32u64 {
            let pick = |i: u64, bit: u64| if sctlr >> i & 1 == 1 { bit } else { 0 };
            let sctlr = pick(0, A) | pick(1, SA) | pick(2, SA0) | pick(3, ENIA) | pick(4, ENIB);
            let mut cpu = CpuState::new(0);
            cpu.el = el;
            cpu.sys.sctlr_el1 = sctlr;
            let expect = Ctx {
                el,
                sp_align_check: sctlr & if el == ExceptionLevel::El1 { SA } else { SA0 } != 0,
                align_check: sctlr & A != 0,
                pac_ia: sctlr & ENIA != 0,
                pac_ib: sctlr & ENIB != 0,
            };
            assert_eq!(run(&mut cpu, |c| c.ctx()), expect);
            assert_eq!(Ctx::from_tb_flags(TbFlags::from_state(&cpu)), expect);
        }
    }
}

#[test]
fn raise_variants_and_raise_if() {
    let cases = [
        (
            Exception::SupervisorCall { imm: 7 },
            ExecError::SupervisorCall {
                pc: 0x4000_0000,
                imm: 7,
            },
        ),
        (
            Exception::Breakpoint { imm: 9 },
            ExecError::Breakpoint {
                pc: 0x4000_0000,
                imm: 9,
            },
        ),
        (
            Exception::SpAlignment,
            ExecError::SpAlignment { pc: 0x4000_0000 },
        ),
        (
            Exception::Unimplemented { word: 1 },
            ExecError::Unimplemented {
                pc: 0x4000_0000,
                word: 1,
            },
        ),
    ];
    for (e, expect) in cases {
        let mut cpu = painted(ExceptionLevel::El1, true);
        let before = cpu.clone();
        assert_eq!(run(&mut cpu, |c| c.raise(e)), Err(expect.clone()));
        assert_eq!(run(&mut cpu, |c| c.raise_if(1, e)), Err(expect));
        assert_eq!(run(&mut cpu, |c| c.raise_if(0, e)), Ok(()));
        assert_eq!(cpu, before);
    }
}

// --- memory -----------------------------------------------------------------------

const RAM_BASE: u64 = 0x1_0000;
const RAM_LEN: usize = 0x4000;

/// Test MMU: identity, but one 4 KiB page faults (for all or only write accesses), and
/// every translation is logged.
struct TestMmu {
    fault_page: Option<u64>,
    write_only: bool,
    log: Vec<(u64, Access)>,
}

impl Translate for TestMmu {
    fn translate(
        &mut self,
        _mem: &dyn GuestMemory,
        va: GuestVirtAddr,
        access: Access,
    ) -> Result<GuestPhysAddr, TranslationFault> {
        self.log.push((va.get(), access));
        let faults = self.fault_page == Some(va.get() >> 12)
            && (!self.write_only || access.kind == AccessKind::Write);
        if faults {
            return Err(TranslationFault {
                va,
                access,
                kind: FaultKind::Translation { level: 3 },
            });
        }
        Ok(GuestPhysAddr(va.get()))
    }
}

fn ram() -> RamRegion {
    let mut ram = RamRegion::new(GuestPhysAddr(RAM_BASE), RAM_LEN);
    let bytes: Vec<u8> = (0..RAM_LEN).map(|i| (i * 7 + 3) as u8).collect();
    ram.write(GuestPhysAddr(RAM_BASE), &bytes).unwrap();
    ram
}

fn snapshot(ram: &RamRegion) -> Vec<u8> {
    let mut b = vec![0; RAM_LEN];
    ram.read(GuestPhysAddr(RAM_BASE), &mut b).unwrap();
    b
}

/// Oracle: little-endian value of `n` bytes at `va` from the byte array.
fn o_read(mem: &[u8], va: u64, n: u32) -> u64 {
    (0..n)
        .map(|i| u64::from(mem[(va - RAM_BASE) as usize + i as usize]) << (8 * i))
        .sum()
}

fn mem_run<R>(
    cpu: &mut CpuState,
    ram: &mut RamRegion,
    mmu: &mut TestMmu,
    f: impl FnOnce(&mut Concrete<'_>) -> R,
) -> R {
    let mut bus = MemBus::new(ram, mmu);
    let bus: &mut dyn DataBus = &mut bus;
    let mut c = Concrete::new(cpu, bus);
    f(&mut c)
}

fn mmu(fault_page: Option<u64>, write_only: bool) -> TestMmu {
    TestMmu {
        fault_page,
        write_only,
        log: Vec::new(),
    }
}

fn abort(va: u64, write: bool, size: u8, kind: DataFaultKind) -> ExecError {
    ExecError::DataAbort {
        pc: 0,
        abort: DataAbort {
            va,
            write,
            size,
            kind,
            iss: None,
        },
    }
}

#[test]
fn load_and_store_little_endian_any_alignment() {
    let page_end = RAM_BASE + 0x1000;
    for ty in Ty::MEM {
        let n = ty.bytes();
        // Aligned, misaligned and straddling the 4 KiB boundary.
        for va in [
            RAM_BASE,
            RAM_BASE + 1,
            page_end - u64::from(n) + 1,
            page_end - 1,
            page_end,
        ] {
            let mut ram = ram();
            let before = snapshot(&ram);
            let mut cpu = CpuState::new(0);
            let mut m = mmu(None, false);
            let got = mem_run(&mut cpu, &mut ram, &mut m, |c| {
                c.load(ty, va, MemAttrs::PLAIN)
            });
            assert_eq!(got, Ok(o_read(&before, va, n)), "{ty:?} {va:#x}");
            // A straddling access translates each page separately.
            let pages = (va + u64::from(n) - 1) / 0x1000 - va / 0x1000 + 1;
            assert_eq!(m.log.len() as u64, pages);

            let value = 0x8877_6655_4433_2211 & ty.mask();
            let r = mem_run(&mut cpu, &mut ram, &mut m, |c| {
                c.store(ty, va, value, MemAttrs::PLAIN)
            });
            assert_eq!(r, Ok(()));
            let mut expect = before.clone();
            for i in 0..n as usize {
                expect[(va - RAM_BASE) as usize + i] = (value >> (8 * i)) as u8;
            }
            assert_eq!(snapshot(&ram), expect, "{ty:?} {va:#x}");
            assert_eq!(cpu, CpuState::new(0), "memory ops touch no register");
        }
    }
}

#[test]
fn faulting_store_writes_nothing() {
    let page_end = RAM_BASE + 0x1000;
    for ty in [Ty::I16, Ty::I32, Ty::I64] {
        // Second page faults on write: the straddling store must not write its first part.
        let va = page_end - 1;
        let mut ram = ram();
        let before = snapshot(&ram);
        let mut cpu = CpuState::new(0);
        let mut m = mmu(Some(page_end >> 12), true);
        let r = mem_run(&mut cpu, &mut ram, &mut m, |c| {
            c.store(ty, va, ty.mask(), MemAttrs::PLAIN)
        });
        assert_eq!(
            r,
            Err(abort(
                page_end,
                true,
                ty.bytes() as u8,
                DataFaultKind::Translation(FaultKind::Translation { level: 3 })
            ))
        );
        assert_eq!(snapshot(&ram), before);
        // Reads of the same page still work (write-only fault).
        let mut m = mmu(Some(page_end >> 12), true);
        assert!(
            mem_run(&mut cpu, &mut ram, &mut m, |c| c.load(
                ty,
                va,
                MemAttrs::PLAIN
            ))
            .is_ok()
        );

        // Past the end of RAM: external abort, again without a partial write.
        let end = RAM_BASE + RAM_LEN as u64;
        let va = end - 1;
        let mut m = mmu(None, false);
        let r = mem_run(&mut cpu, &mut ram, &mut m, |c| {
            c.store(ty, va, 0, MemAttrs::PLAIN)
        });
        assert!(matches!(
            r,
            Err(ExecError::DataAbort {
                abort: DataAbort {
                    kind: DataFaultKind::External(_),
                    write: true,
                    ..
                },
                ..
            })
        ));
        assert_eq!(snapshot(&ram), before);
        let r = mem_run(&mut cpu, &mut ram, &mut m, |c| {
            c.load(ty, va, MemAttrs::PLAIN)
        });
        assert!(matches!(
            r,
            Err(ExecError::DataAbort {
                abort: DataAbort {
                    kind: DataFaultKind::External(_),
                    write: false,
                    ..
                },
                ..
            })
        ));
    }
}

#[test]
fn probe_checks_without_writing() {
    let page_end = RAM_BASE + 0x1000;
    let mut ram = ram();
    let before = snapshot(&ram);
    let mut cpu = CpuState::new(0);
    // 2 x 8 bytes ending exactly at the page end: fine; one byte later: fault.
    let mut m = mmu(Some(page_end >> 12), true);
    let ok = mem_run(&mut cpu, &mut ram, &mut m, |c| {
        c.probe(Ty::I64, 2, page_end - 16, MemAttrs::PLAIN)
    });
    assert_eq!(ok, Ok(()));
    let r = mem_run(&mut cpu, &mut ram, &mut m, |c| {
        c.probe(Ty::I64, 2, page_end - 15, MemAttrs::PLAIN)
    });
    assert_eq!(
        r,
        Err(abort(
            page_end,
            true,
            8,
            DataFaultKind::Translation(FaultKind::Translation { level: 3 })
        ))
    );
    // Probes use write permission.
    assert!(m.log.iter().all(|(_, a)| a.kind == AccessKind::Write));
    assert_eq!(snapshot(&ram), before);
}

#[test]
fn alignment_checks_follow_sctlr_a_and_required() {
    for (sctlr_a, align, misaligned_ok) in [
        (false, Align::Normal, true),
        (true, Align::Normal, false),
        (false, Align::Required, false),
    ] {
        for ty in Ty::MEM {
            for off in 0..8u64 {
                let va = RAM_BASE + 0x100 + off;
                let aligned = va.is_multiple_of(u64::from(ty.bytes()));
                let mut ram = ram();
                let mut cpu = CpuState::new(0);
                cpu.sys.sctlr_el1 = if sctlr_a { 1 << 1 } else { 0 };
                let attrs = MemAttrs {
                    align,
                    ..MemAttrs::PLAIN
                };
                let mut m = mmu(None, false);
                let l = mem_run(&mut cpu, &mut ram, &mut m, |c| c.load(ty, va, attrs));
                let s = mem_run(&mut cpu, &mut ram, &mut m, |c| c.store(ty, va, 0, attrs));
                let p = mem_run(&mut cpu, &mut ram, &mut m, |c| c.probe(ty, 2, va, attrs));
                if aligned || misaligned_ok {
                    assert!(l.is_ok() && s.is_ok() && p.is_ok());
                } else {
                    let size = ty.bytes() as u8;
                    assert_eq!(l, Err(abort(va, false, size, DataFaultKind::Alignment)));
                    assert_eq!(s, Err(abort(va, true, size, DataFaultKind::Alignment)));
                    assert_eq!(p, Err(abort(va, true, size, DataFaultKind::Alignment)));
                    assert!(m.log.is_empty(), "alignment is checked before translation");
                }
            }
        }
    }
}

#[test]
fn privilege_and_syndrome_of_accesses() {
    let iss = DataIss {
        srt: 5,
        sign_extend: true,
        sixty_four: false,
    };
    for el in [ExceptionLevel::El0, ExceptionLevel::El1] {
        for unprivileged in [false, true] {
            let mut ram = ram();
            let mut cpu = CpuState::new(0);
            cpu.el = el;
            let attrs = MemAttrs {
                unprivileged,
                order: MemOrder::Plain,
                align: Align::Normal,
                iss: Some(iss),
            };
            let mut m = mmu(Some(RAM_BASE >> 12), false);
            let r = mem_run(&mut cpu, &mut ram, &mut m, |c| {
                c.load(Ty::I8, RAM_BASE, attrs)
            });
            let expect_priv = if unprivileged || el == ExceptionLevel::El0 {
                Privilege::Unprivileged
            } else {
                Privilege::Privileged
            };
            assert_eq!(
                m.log,
                vec![(RAM_BASE, Access::new(AccessKind::Read, expect_priv))]
            );
            match r {
                Err(ExecError::DataAbort { abort, .. }) => assert_eq!(abort.iss, Some(iss)),
                other => panic!("{other:?}"),
            }
        }
    }
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "second store without probe")]
fn unprobed_second_store_is_caught_in_debug() {
    let mut ram = ram();
    let mut cpu = CpuState::new(0);
    let mut m = mmu(None, false);
    let _ = mem_run(&mut cpu, &mut ram, &mut m, |c| {
        let _ = c.store(Ty::I8, RAM_BASE, 0, MemAttrs::PLAIN);
        c.store(Ty::I8, RAM_BASE + 1, 0, MemAttrs::PLAIN)
    });
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "raise after an architectural write")]
fn load_after_store_is_caught_in_debug() {
    let mut ram = ram();
    let mut cpu = CpuState::new(0);
    let mut m = mmu(None, false);
    let _ = mem_run(&mut cpu, &mut ram, &mut m, |c| {
        let _ = c.store(Ty::I8, RAM_BASE, 0, MemAttrs::PLAIN);
        c.load(Ty::I8, RAM_BASE, MemAttrs::PLAIN)
    });
}
