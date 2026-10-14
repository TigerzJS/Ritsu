//! Operation conformance tests (dbt.md §3.2, level 2): every [`Sem`] operation of the
//! [`Concrete`] backend, driven through the trait, against an independent oracle.
//!
//! The oracles are deliberately written differently from `prim.rs`: 128-bit
//! arithmetic, explicit modulo and two's-complement conversion, bit-by-bit loops for
//! logic and shifts, and the condition table spelled out from the Arm ARM condition
//! code descriptions. A shared mistake would have to be made twice in different ways.

use super::concrete::Concrete;
use super::{BinOp, BranchKind, CmpOp, Exception, Sem, Ty};
use crate::cpu::{CpuState, ExceptionLevel, GpReg, Nzcv};
use crate::decode::Cond;
use crate::interp::{ExecError, StepOutcome};

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
    let mut c = Concrete::new(cpu);
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
        let outcome = Concrete::new(&mut cpu).finish();
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
                    let mut c = Concrete::new(&mut cpu);
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
            let mut c = Concrete::new(&mut cpu);
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
