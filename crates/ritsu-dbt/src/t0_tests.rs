//! T0 semantics of the WP-DBT-02 classes, against hand-computed Arm results (not
//! against the implementation), plus compiled C programs (memcpy/strlen/memcmp and an
//! arithmetic mix) run to completion in T0.

use crate::asm;
use crate::cpu::{CpuState, ExceptionLevel, GpReg, Nzcv, SysReg};
use crate::decode::Cond;
use crate::interp::{DataFaultKind, ExecError, StepOutcome, step, step_mut};
use crate::sem::Exception;
use ritsu_core::GuestPhysAddr;
use ritsu_mem::{GuestMemory, RamRegion};

const BASE: u64 = 0x4000_0000;
const RAM: usize = 0x10_0000;
const DATA: u64 = BASE + 0x8_0000;

fn ram_with(code: &[u32]) -> RamRegion {
    let mut ram = RamRegion::new(GuestPhysAddr(BASE), RAM);
    for (i, w) in code.iter().enumerate() {
        ram.write_u32(GuestPhysAddr(BASE + 4 * i as u64), *w)
            .unwrap();
    }
    ram
}

/// Runs one instruction at `BASE` on `cpu` (read/write memory), returns the result.
fn exec(word: u32, cpu: &mut CpuState, ram: &mut RamRegion) -> Result<StepOutcome, ExecError> {
    ram.write_u32(GuestPhysAddr(BASE), word).unwrap();
    cpu.pc = BASE;
    step_mut(cpu, ram)
}

/// Runs one instruction with registers `regs` (x0, x1, …) and returns the CPU.
fn run(word: u32, regs: &[u64], nzcv: Nzcv) -> CpuState {
    let mut ram = ram_with(&[]);
    let mut cpu = CpuState::new(BASE);
    for (i, &v) in regs.iter().enumerate() {
        cpu.x[i] = v;
    }
    cpu.nzcv = nzcv;
    exec(word, &mut cpu, &mut ram).unwrap();
    cpu
}

fn r0(word: u32, regs: &[u64]) -> u64 {
    run(word, regs, Nzcv::default()).x[0]
}

const NONE: Nzcv = Nzcv::new(false, false, false, false);

// ---------------------------------------------------------------------------------
// Data processing
// ---------------------------------------------------------------------------------

#[test]
fn adr_and_adrp() {
    // At BASE = 0x4000_0000: adr x0, .+0x10 / adr x0, .-1 / adrp x0, .+0x3000
    assert_eq!(r0(asm::adr(false, 0x10, 0), &[]), BASE + 0x10);
    assert_eq!(r0(asm::adr(false, -1, 0), &[]), BASE - 1);
    assert_eq!(r0(asm::adr(true, 3, 0), &[]), BASE + 0x3000);
    assert_eq!(
        r0(asm::adr(true, -(1 << 20), 0), &[]),
        BASE.wrapping_sub(1 << 32)
    );
}

#[test]
fn logical_immediate_and_flags() {
    // and x0, x1, #0xff ; orr w0, wzr, #0x55555555 ; eor x0, x1, #1 ; ands x0, x1, #1<<63
    assert_eq!(r0(0x9240_1c20, &[0, 0x1234]), 0x34);
    assert_eq!(r0(0x3200_f3e0, &[u64::MAX]), 0x5555_5555);
    assert_eq!(r0(asm::logical_imm(true, 2, 0x1000, 1, 0), &[0, 6]), 7);
    // ands x0, x1, #0x8000000000000000 (N=1, immr=1, imms=0)
    let c = run(
        asm::logical_imm(true, 3, 0x1040, 1, 0),
        &[0, u64::MAX],
        Nzcv::from_nibble(0xf),
    );
    assert_eq!(c.x[0], 1 << 63);
    assert_eq!(c.nzcv, Nzcv::new(true, false, false, false));
    let c = run(
        asm::logical_imm(true, 3, 0x1040, 1, 0),
        &[0, 1],
        Nzcv::from_nibble(0xf),
    );
    assert_eq!(c.nzcv, Nzcv::new(false, true, false, false));
    // and sp, x1, #~0xf writes SP.
    let c = run(0x927c_ec3f, &[0, 0x1234_5679], NONE);
    assert_eq!(c.sp_el1, 0x1234_5670);
}

#[test]
fn bitfield_aliases() {
    let v = 0xfedc_ba98_7654_3210u64;
    // ubfx w0, w1, #4, #8 → bits 11:4 of the low word
    assert_eq!(r0(asm::bitfield(false, 2, 4, 11, 1, 0), &[0, v]), 0x21);
    // sbfx x0, x1, #60, #4 → sign-extended 0xf
    assert_eq!(r0(asm::bitfield(true, 0, 60, 63, 1, 0), &[0, v]), u64::MAX);
    // lsl x0, x1, #4 (ubfm #60, #59) / lsr x0, x1, #60 / asr w0, w1, #31
    assert_eq!(r0(asm::bitfield(true, 2, 60, 59, 1, 0), &[0, v]), v << 4);
    assert_eq!(r0(asm::bitfield(true, 2, 60, 63, 1, 0), &[0, v]), 0xf);
    assert_eq!(
        r0(asm::bitfield(false, 0, 31, 31, 1, 0), &[0, 0x8000_0000]),
        0xffff_ffff
    );
    // sxtw x0, w1 / sxtb w0, w1 / uxth w0, w1
    assert_eq!(
        r0(asm::bitfield(true, 0, 0, 31, 1, 0), &[0, 0x8000_0000]),
        0xffff_ffff_8000_0000
    );
    assert_eq!(
        r0(asm::bitfield(false, 0, 0, 7, 1, 0), &[0, 0x80]),
        0xffff_ff80
    );
    assert_eq!(r0(asm::bitfield(false, 2, 0, 15, 1, 0), &[0, v]), 0x3210);
    // bfi x0, x1, #8, #16 (bfm #56, #15): insert low 16 bits of x1 at bit 8
    assert_eq!(
        r0(asm::bitfield(true, 1, 56, 15, 1, 0), &[u64::MAX, 0x1234]),
        0xffff_ffff_ff12_34ff
    );
    // bfxil w0, w1, #4, #8 (bfm #4, #11): upper half of x0 cleared (32-bit write)
    assert_eq!(
        r0(asm::bitfield(false, 1, 4, 11, 1, 0), &[u64::MAX, v]),
        0xffff_ff21
    );
    // sbfiz x0, x1, #1, #32 (sbfm #63, #31)
    assert_eq!(r0(0x937f_7c20, &[0, 0x8000_0000]), 0xffff_ffff_0000_0000);
}

#[test]
fn extract_and_ror() {
    // extr x0, x1, x2, #12 / ror w0, w1, #8 / extr with lsb 0 = Rm
    assert_eq!(
        r0(asm::extr(true, 2, 12, 1, 0), &[0, 0xabc, 0x1000]),
        0xabc0_0000_0000_0001
    );
    assert_eq!(
        r0(asm::extr(false, 1, 8, 1, 0), &[0, 0x1234_5678]),
        0x7812_3456
    );
    assert_eq!(r0(asm::extr(true, 2, 0, 1, 0), &[0, 1, 2]), 2);
}

#[test]
fn add_sub_register_forms() {
    // add x0, x1, x2, lsl #4 / sub w0, w1, w2, lsr #1 / adds x0, x1, x2, asr #63
    assert_eq!(
        r0(
            asm::add_sub_shifted(true, false, false, 0, 2, 4, 1, 0),
            &[0, 1, 2]
        ),
        33
    );
    assert_eq!(
        r0(
            asm::add_sub_shifted(false, true, false, 1, 2, 1, 1, 0),
            &[0, 0, 4]
        ),
        0xffff_fffe
    );
    let c = run(
        asm::add_sub_shifted(true, false, true, 2, 2, 63, 1, 0),
        &[0, 1, 1 << 63],
        NONE,
    );
    assert_eq!((c.x[0], c.nzcv), (0, Nzcv::new(false, true, true, false)));
    // neg x0, x1 / cmp x1, x2 (32-bit, wraps)
    assert_eq!(
        r0(
            asm::add_sub_shifted(true, true, false, 0, 1, 0, 31, 0),
            &[0, 5]
        ),
        5u64.wrapping_neg()
    );
    let c = run(
        asm::add_sub_shifted(false, true, true, 0, 2, 0, 1, 31),
        &[0, 1, 2],
        NONE,
    );
    assert_eq!(c.nzcv, Nzcv::new(true, false, false, false));
    // add x0, sp, w1, sxtw #2 with negative w1 / sub sp, sp, x1, uxtx
    let mut cpu = CpuState::new(BASE);
    let mut ram = ram_with(&[]);
    cpu.sp_el1 = 0x1000;
    cpu.x[1] = 0xdead_beef_ffff_fffe; // w1 = -2
    exec(
        asm::add_sub_ext(true, false, false, 1, 6, 2, 31, 0),
        &mut cpu,
        &mut ram,
    )
    .unwrap();
    assert_eq!(cpu.x[0], 0x1000 - 8);
    cpu.x[1] = 0x10;
    exec(
        asm::add_sub_ext(true, true, false, 1, 3, 0, 31, 31),
        &mut cpu,
        &mut ram,
    )
    .unwrap();
    assert_eq!(cpu.sp_el1, 0xff0);
    // add w0, w1, w2, uxtb #4 / adds x0, x1, w2, uxth (flags)
    assert_eq!(
        r0(
            asm::add_sub_ext(false, false, false, 2, 0, 4, 1, 0),
            &[0, 1, 0x1ff]
        ),
        1 + 0xff0
    );
    let c = run(
        asm::add_sub_ext(true, false, true, 2, 1, 0, 1, 0),
        &[0, u64::MAX, 0x1_0001],
        NONE,
    );
    assert_eq!((c.x[0], c.nzcv), (0, Nzcv::new(false, true, true, false)));
    // cmp sp, x1 (subs xzr, sp, x1, uxtx): Rn = SP, Rd = ZR.
    let mut cpu = CpuState::new(BASE);
    cpu.sp_el1 = 5;
    cpu.x[1] = 5;
    exec(
        asm::add_sub_ext(true, true, true, 1, 3, 0, 31, 31),
        &mut cpu,
        &mut ram,
    )
    .unwrap();
    assert_eq!(
        (cpu.sp_el1, cpu.nzcv),
        (5, Nzcv::new(false, true, true, false))
    );
}

#[test]
fn add_sub_with_carry() {
    let c_set = Nzcv::new(false, false, true, false);
    // adc x0, x1, x2 with C = 1 / C = 0
    assert_eq!(
        run(asm::adc(true, false, false, 2, 1, 0), &[0, 1, 2], c_set).x[0],
        4
    );
    assert_eq!(
        run(asm::adc(true, false, false, 2, 1, 0), &[0, 1, 2], NONE).x[0],
        3
    );
    // adcs x0, x1, x2: MAX + 0 + 1 → 0, C = 1, Z = 1
    let c = run(
        asm::adc(true, false, true, 2, 1, 0),
        &[0, u64::MAX, 0],
        c_set,
    );
    assert_eq!((c.x[0], c.nzcv), (0, Nzcv::new(false, true, true, false)));
    // sbcs w0, w1, w2: 5 - 3 - (1 - C): C = 0 → 1; C = 1 → 2
    assert_eq!(
        run(asm::adc(false, true, true, 2, 1, 0), &[0, 5, 3], NONE).x[0],
        1
    );
    assert_eq!(
        run(asm::adc(false, true, true, 2, 1, 0), &[0, 5, 3], c_set).x[0],
        2
    );
    // sbcs x0, x1, x2 = 0 - 0 - 1 with C = 0: borrow, N set, C clear
    let c = run(asm::adc(true, true, true, 2, 1, 0), &[0, 0, 0], NONE);
    assert_eq!(
        (c.x[0], c.nzcv),
        (u64::MAX, Nzcv::new(true, false, false, false))
    );
    // ngc x0, x1 (sbc x0, xzr, x1) with C = 1 → -x1
    assert_eq!(
        run(asm::adc(true, true, false, 1, 31, 0), &[0, 7], c_set).x[0],
        7u64.wrapping_neg()
    );
}

#[test]
fn conditional_compare_all_conditions() {
    for bits in 0..16u8 {
        let f = Nzcv::from_nibble(bits);
        for cond in Cond::ALL {
            // ccmp x1, x2, #0b0110, cond with x1 == x2 (→ Z, C) / ccmn x1, #3, #0b1001, cond
            let c = run(
                asm::ccmp(true, true, false, 2, cond, 1, 0b0110),
                &[0, 9, 9],
                f,
            );
            let expect = if cond.holds(f) {
                Nzcv::new(false, true, true, false)
            } else {
                Nzcv::from_nibble(0b0110)
            };
            assert_eq!(c.nzcv, expect, "{cond:?} {f:?}");
            let c = run(
                asm::ccmp(true, false, true, 3, cond, 1, 0b1001),
                &[0, (-3i64) as u64],
                f,
            );
            let expect = if cond.holds(f) {
                Nzcv::new(false, true, true, false)
            } else {
                Nzcv::from_nibble(0b1001)
            };
            assert_eq!(c.nzcv, expect, "{cond:?} {f:?}");
        }
    }
    // 32-bit compare ignores the upper half: ccmp w1, w2, #0, al with w1 = 0x8000_0000, w2 = 1 → V
    let c = run(
        asm::ccmp(false, true, false, 2, Cond::Al, 1, 0),
        &[0, 0xffff_ffff_8000_0000, 1],
        NONE,
    );
    assert_eq!(c.nzcv, Nzcv::new(false, false, true, true));
}

#[test]
fn conditional_select_family() {
    let z = Nzcv::new(false, true, false, false);
    for (op, o2, taken, not_taken) in [
        (false, false, 10u64, 20u64),           // csel
        (false, true, 10, 21),                  // csinc
        (true, false, 10, !20u64),              // csinv
        (true, true, 10, 20u64.wrapping_neg()), // csneg
    ] {
        let w = asm::csel(true, op, o2, 2, Cond::Eq, 1, 0);
        assert_eq!(run(w, &[0, 10, 20], z).x[0], taken);
        assert_eq!(run(w, &[0, 10, 20], NONE).x[0], not_taken);
    }
    // cset w0, eq (csinc w0, wzr, wzr, ne) / csetm x0, ne
    assert_eq!(
        run(asm::csel(false, false, true, 31, Cond::Ne, 31, 0), &[7], z).x[0],
        1
    );
    assert_eq!(
        run(
            asm::csel(true, true, false, 31, Cond::Eq, 31, 0),
            &[7],
            NONE
        )
        .x[0],
        u64::MAX
    );
    // 32-bit csinc wraps and clears the upper half; AL/NV always select Rn.
    assert_eq!(
        run(
            asm::csel(false, false, true, 2, Cond::Eq, 1, 0),
            &[0, 1, u64::MAX],
            NONE
        )
        .x[0],
        0
    );
    assert_eq!(
        run(
            asm::csel(true, false, true, 2, Cond::Nv, 1, 0),
            &[0, 1, 5],
            NONE
        )
        .x[0],
        1
    );
}

#[test]
fn multiply_family() {
    let (a, b, c) = (0x1234_5678_9abc_def0u64, 0x0fed_cba9_8765_4321u64, 1000u64);
    // madd / msub (64 and 32 bit)
    assert_eq!(
        r0(asm::dp3(true, 0, false, 2, 3, 1, 0), &[0, a, b, c]),
        c.wrapping_add(a.wrapping_mul(b))
    );
    assert_eq!(
        r0(asm::dp3(true, 0, true, 2, 3, 1, 0), &[0, a, b, c]),
        c.wrapping_sub(a.wrapping_mul(b))
    );
    assert_eq!(
        r0(asm::dp3(false, 0, false, 2, 3, 1, 0), &[0, a, b, c]),
        u64::from((c as u32).wrapping_add((a as u32).wrapping_mul(b as u32)))
    );
    // smaddl x0, w1, w2, x3 with negative words; umaddl the same bits unsigned
    let (wa, wb) = (0xffff_fffe_u64, 3u64); // -2, 3
    assert_eq!(
        r0(
            asm::dp3(true, 1, false, 2, 3, 1, 0),
            &[0, wa | 0xdead << 32, wb, 10]
        ),
        4
    );
    assert_eq!(
        r0(asm::dp3(true, 5, false, 2, 3, 1, 0), &[0, wa, wb, 10]),
        10 + 3 * 0xffff_fffe
    );
    assert_eq!(
        r0(asm::dp3(true, 1, true, 2, 3, 1, 0), &[0, wa, wb, 10]),
        16
    ); // smsubl
    assert_eq!(
        r0(asm::dp3(true, 5, true, 2, 3, 1, 0), &[0, 1, 1, 0]),
        u64::MAX
    ); // umsubl
    // smulh / umulh
    assert_eq!(
        r0(asm::dp3(true, 2, false, 2, 31, 1, 0), &[0, u64::MAX, 2]),
        u64::MAX
    );
    assert_eq!(
        r0(asm::dp3(true, 6, false, 2, 31, 1, 0), &[0, u64::MAX, 2]),
        1
    );
    assert_eq!(
        r0(
            asm::dp3(true, 2, false, 2, 31, 1, 0),
            &[0, 1 << 63, 1 << 63]
        ),
        1 << 62
    );
}

#[test]
fn division_edge_cases() {
    let udiv = |sf| asm::dp2(sf, 0b10, 2, 1, 0);
    let sdiv = |sf| asm::dp2(sf, 0b11, 2, 1, 0);
    // Division by zero gives zero (no trap), signed and unsigned, 32 and 64 bits.
    for sf in [false, true] {
        assert_eq!(r0(udiv(sf), &[9, 123, 0]), 0);
        assert_eq!(r0(sdiv(sf), &[9, 123, 0]), 0);
    }
    // INT_MIN / -1 = INT_MIN.
    assert_eq!(r0(sdiv(true), &[0, 1 << 63, u64::MAX]), 1 << 63);
    assert_eq!(r0(sdiv(false), &[0, 0x8000_0000, 0xffff_ffff]), 0x8000_0000);
    // Rounding towards zero; 32-bit operands ignore the upper half.
    assert_eq!(r0(sdiv(true), &[0, (-7i64) as u64, 2]), (-3i64) as u64);
    assert_eq!(r0(sdiv(false), &[0, 0xffff_ffff_ffff_fff9, 2]), 0xffff_fffd);
    assert_eq!(r0(udiv(false), &[0, 0x1_0000_0010, 4]), 4);
    assert_eq!(r0(udiv(true), &[0, u64::MAX, 3]), u64::MAX / 3);
}

#[test]
fn variable_shifts_take_amount_modulo_width() {
    // lslv/lsrv/asrv/rorv x0, x1, x2 and the 32-bit forms with amounts >= width
    let v = 0x8000_0000_0000_0001u64;
    assert_eq!(r0(asm::dp2(true, 0b1000, 2, 1, 0), &[0, v, 65]), 2);
    assert_eq!(r0(asm::dp2(true, 0b1001, 2, 1, 0), &[0, v, 127]), 1);
    assert_eq!(r0(asm::dp2(true, 0b1010, 2, 1, 0), &[0, v, 63]), u64::MAX);
    assert_eq!(
        r0(asm::dp2(true, 0b1011, 2, 1, 0), &[0, v, 1]),
        0xc000_0000_0000_0000
    );
    assert_eq!(r0(asm::dp2(false, 0b1000, 2, 1, 0), &[0, 1, 32]), 1);
    assert_eq!(
        r0(asm::dp2(false, 0b1010, 2, 1, 0), &[0, 0x8000_0000, 33]),
        0xc000_0000
    );
}

#[test]
fn bit_and_byte_operations() {
    let v = 0x0102_0304_0506_0708u64;
    assert_eq!(r0(asm::dp1(true, 0, 1, 0), &[0, 1]), 1 << 63); // rbit
    assert_eq!(r0(asm::dp1(false, 0, 1, 0), &[0, 1]), 1 << 31);
    assert_eq!(r0(asm::dp1(true, 1, 1, 0), &[0, v]), 0x0201_0403_0605_0807); // rev16
    assert_eq!(r0(asm::dp1(false, 1, 1, 0), &[0, v]), 0x0605_0807);
    assert_eq!(r0(asm::dp1(true, 2, 1, 0), &[0, v]), 0x0403_0201_0807_0605); // rev32
    assert_eq!(r0(asm::dp1(true, 3, 1, 0), &[0, v]), 0x0807_0605_0403_0201); // rev (64)
    assert_eq!(r0(asm::dp1(false, 2, 1, 0), &[0, v]), 0x0807_0605); // rev (32)
    for (val, clz64, cls64) in [
        (0u64, 64, 63),
        (u64::MAX, 0, 63),
        (1, 63, 62),
        (1 << 63, 0, 0),
        ((-2i64) as u64, 0, 62),
        (0x4000_0000_0000_0000, 1, 0),
    ] {
        assert_eq!(
            r0(asm::dp1(true, 4, 1, 0), &[0, val]),
            clz64,
            "clz {val:#x}"
        );
        assert_eq!(
            r0(asm::dp1(true, 5, 1, 0), &[0, val]),
            cls64,
            "cls {val:#x}"
        );
    }
    assert_eq!(
        r0(asm::dp1(false, 4, 1, 0), &[0, 0xffff_ffff_0000_0000]),
        32
    );
    assert_eq!(r0(asm::dp1(false, 5, 1, 0), &[0, 0xffff_fff0]), 27);
}

// ---------------------------------------------------------------------------------
// Branches, exceptions, hints, system registers
// ---------------------------------------------------------------------------------

#[test]
fn test_bit_branches() {
    for bit in [0u32, 31, 32, 63] {
        for (val, set) in [(1u64 << bit, true), (!(1u64 << bit), false)] {
            for nonzero in [false, true] {
                let mut ram = ram_with(&[]);
                let mut cpu = CpuState::new(BASE);
                cpu.x[3] = val;
                let taken = set == nonzero;
                let out = exec(asm::tb(nonzero, bit, -0x8000, 3), &mut cpu, &mut ram).unwrap();
                assert_eq!(out, StepOutcome::Branch { taken });
                assert_eq!(cpu.pc, if taken { BASE - 0x8000 } else { BASE + 4 });
            }
        }
    }
}

#[test]
fn svc_and_brk_are_precise_exceptions() {
    for (word, expect) in [
        (
            asm::svc(0x42),
            ExecError::SupervisorCall {
                pc: BASE,
                imm: 0x42,
            },
        ),
        (
            asm::brk(0xf000),
            ExecError::Breakpoint {
                pc: BASE,
                imm: 0xf000,
            },
        ),
    ] {
        let mut ram = ram_with(&[]);
        let mut cpu = CpuState::new(BASE);
        cpu.x[8] = 93;
        let before = cpu.clone();
        assert_eq!(exec(word, &mut cpu, &mut ram), Err(expect));
        assert_eq!(cpu, before);
    }
}

#[test]
fn hints_behave_per_class_and_context() {
    let mut ram = ram_with(&[]);
    for imm in 1..128u32 {
        for sctlr in [0u64, 1 << 31, 1 << 30] {
            let mut cpu = CpuState::new(BASE);
            cpu.sys.sctlr_el1 = sctlr;
            let before = cpu.clone();
            let r = exec(asm::hint(imm), &mut cpu, &mut ram);
            let key_a = [8, 12, 24, 25, 28, 29].contains(&imm);
            let key_b = [10, 14, 26, 27, 30, 31].contains(&imm);
            let unimpl = [2, 3, 4, 5, 7].contains(&imm)
                || (key_a && sctlr & (1 << 31) != 0)
                || (key_b && sctlr & (1 << 30) != 0);
            if unimpl {
                assert_eq!(
                    r,
                    Err(ExecError::Unimplemented {
                        pc: BASE,
                        word: asm::hint(imm)
                    }),
                    "{imm}"
                );
                assert_eq!(cpu, before);
            } else {
                assert_eq!(
                    r,
                    Ok(StepOutcome::Sequential),
                    "hint #{imm} sctlr {sctlr:#x}"
                );
                assert_eq!(
                    cpu,
                    CpuState {
                        pc: BASE + 4,
                        ..before
                    }
                );
            }
        }
    }
}

#[test]
fn system_register_moves() {
    let mut ram = ram_with(&[]);
    let mut cpu = CpuState::new(BASE);
    cpu.x[1] = u64::MAX;
    for reg in SysReg::ALL {
        exec(asm::sysreg(false, reg, 1), &mut cpu, &mut ram).unwrap();
        exec(asm::sysreg(true, reg, 2), &mut cpu, &mut ram).unwrap();
        assert_eq!(
            cpu.x[2],
            reg.write_mask(),
            "{reg:?}: RES0 bits read as zero"
        );
    }
    assert_eq!(cpu.nzcv, Nzcv::from_nibble(0xf));
    assert_eq!(cpu.sys.fpcr, 0x07c8_0000);
    assert_eq!(cpu.sys.fpsr, 0x0800_009f);
    assert_eq!(cpu.sys.tpidr_el0, u64::MAX);
    // msr nzcv, xzr clears the flags; mrs xzr discards.
    exec(asm::sysreg(false, SysReg::Nzcv, 31), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.nzcv, Nzcv::default());
    exec(asm::sysreg(true, SysReg::TpidrEl0, 31), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.read(GpReg::Zr), 0);
    // Works at EL0 as well (all four are EL0-accessible).
    cpu.el = ExceptionLevel::El0;
    cpu.x[3] = 0x1234;
    exec(asm::sysreg(false, SysReg::TpidrEl0, 3), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.sys.tpidr_el0, 0x1234);
}

// ---------------------------------------------------------------------------------
// Loads and stores
// ---------------------------------------------------------------------------------

fn mem_cpu() -> (CpuState, RamRegion) {
    let mut ram = ram_with(&[]);
    let bytes: Vec<u8> = (0..64u8).map(|i| 0x80 | i).collect();
    ram.write(GuestPhysAddr(DATA), &bytes).unwrap();
    let mut cpu = CpuState::new(BASE);
    cpu.x[1] = DATA;
    (cpu, ram)
}

#[test]
fn single_register_loads_extend_correctly() {
    // (size, opc, expected x0 from bytes 0x80, 0x81, … at DATA + 8)
    let cases = [
        (0, 1, 0x88u64),               // ldrb
        (0, 3, 0xffff_ff88),           // ldrsb w
        (0, 2, 0xffff_ffff_ffff_ff88), // ldrsb x
        (1, 1, 0x8988),                // ldrh
        (1, 3, 0xffff_8988),           // ldrsh w
        (1, 2, 0xffff_ffff_ffff_8988), // ldrsh x
        (2, 1, 0x8b8a_8988),           // ldr w
        (2, 2, 0xffff_ffff_8b8a_8988), // ldrsw
        (3, 1, 0x8f8e_8d8c_8b8a_8988), // ldr x
    ];
    for (size, opc, expect) in cases {
        // unsigned offset (imm12 scaled), unscaled, register (lsl #size), unprivileged
        for word in [
            asm::ldst_uimm(size, opc, 8 >> size, 1, 0),
            asm::ldst_imm9(size, opc, 8, 0, 1, 0),
            asm::ldst_imm9(size, opc, 8, 2, 1, 0),
            asm::ldst_reg(size, opc, 2, 3, true, 1, 0),
        ] {
            let (mut cpu, mut ram) = mem_cpu();
            cpu.x[0] = 0x5555_5555_5555_5555;
            cpu.x[2] = 8 >> size;
            exec(word, &mut cpu, &mut ram).unwrap();
            assert_eq!(cpu.x[0], expect, "{word:#010x}");
        }
    }
}

#[test]
fn register_offset_extends() {
    // ldrb w0, [x1, w2, sxtw] with w2 = -1 / uxtw ignores the upper half
    let (mut cpu, mut ram) = mem_cpu();
    cpu.x[1] = DATA + 8;
    cpu.x[2] = 0xffff_ffff_ffff_ffff;
    exec(asm::ldst_reg(0, 1, 2, 6, false, 1, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.x[0], 0x87);
    cpu.x[2] = 0x1_0000_0002;
    exec(asm::ldst_reg(0, 1, 2, 2, false, 1, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.x[0], 0x8a);
    // ldr x0, [x1, w2, sxtw #3] with w2 = -1
    cpu.x[2] = 0xffff_ffff;
    exec(asm::ldst_reg(3, 1, 2, 6, true, 1, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.x[0], 0x87868584_83828180);
}

#[test]
fn stores_and_writeback() {
    let (mut cpu, mut ram) = mem_cpu();
    cpu.x[0] = 0x1122_3344_5566_7788;
    // str x0, [x1, #16]! → writes at DATA+16, x1 = DATA+16
    exec(asm::ldst_imm9(3, 0, 16, 3, 1, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!(
        ram.read_u64(GuestPhysAddr(DATA + 16)).unwrap(),
        0x1122_3344_5566_7788
    );
    assert_eq!(cpu.x[1], DATA + 16);
    // strh w0, [x1], #-16 → writes at DATA+16, x1 = DATA
    cpu.x[0] = 0xabcd;
    exec(asm::ldst_imm9(1, 0, -16, 1, 1, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!(
        ram.read_u64(GuestPhysAddr(DATA + 16)).unwrap(),
        0x1122_3344_5566_abcd
    );
    assert_eq!(cpu.x[1], DATA);
    // strb wzr, [x1, #3] / sturb unscaled negative
    exec(asm::ldst_uimm(0, 0, 3, 1, 31), &mut cpu, &mut ram).unwrap();
    assert_eq!(ram.read_u32(GuestPhysAddr(DATA)).unwrap(), 0x0082_8180);
    // ldr x0, [x1], #8 post-index loads from the old base
    exec(asm::ldst_imm9(3, 1, 8, 1, 1, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!((cpu.x[0], cpu.x[1]), (0x8786_8584_0082_8180, DATA + 8));
}

#[test]
fn literal_loads_and_prefetch() {
    let mut ram = ram_with(&[]);
    ram.write_u64(GuestPhysAddr(BASE + 0x100), 0x8000_0000_ffff_fffe)
        .unwrap();
    let mut cpu = CpuState::new(BASE);
    exec(asm::ldr_lit(0, 0x100, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.x[0], 0xffff_fffe);
    exec(asm::ldr_lit(1, 0x100, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.x[0], 0x8000_0000_ffff_fffe);
    exec(asm::ldr_lit(2, 0x100, 0), &mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.x[0], 0xffff_ffff_ffff_fffe);
    // PRFM never accesses memory: no fault even for an unmapped address.
    let before = {
        let mut c = cpu.clone();
        c.pc = BASE;
        c
    };
    cpu.x[1] = 0xdead_0000_0000;
    let before = CpuState { x: cpu.x, ..before };
    for word in [
        asm::ldr_lit(3, -0x4000_0000, 0),
        asm::ldst_uimm(3, 2, 1, 1, 0),
        asm::ldst_imm9(3, 2, -1, 0, 1, 0),
        asm::ldst_reg(3, 2, 2, 3, true, 1, 0),
    ] {
        let mut c = before.clone();
        assert_eq!(
            exec(word, &mut c, &mut ram),
            Ok(StepOutcome::Sequential),
            "{word:#010x}"
        );
        assert_eq!(
            c,
            CpuState {
                pc: BASE + 4,
                ..before.clone()
            }
        );
    }
}

#[test]
fn pairs_including_ldpsw_and_sp() {
    let (mut cpu, mut ram) = mem_cpu();
    cpu.sp_el1 = DATA + 32;
    cpu.x[2] = 0x1111;
    cpu.x[3] = 0x2222;
    // stp x2, x3, [sp, #-16]!
    exec(
        asm::ldst_pair(2, 3, false, -2, 3, 31, 2),
        &mut cpu,
        &mut ram,
    )
    .unwrap();
    assert_eq!(cpu.sp_el1, DATA + 16);
    assert_eq!(ram.read_u64(GuestPhysAddr(DATA + 16)).unwrap(), 0x1111);
    assert_eq!(ram.read_u64(GuestPhysAddr(DATA + 24)).unwrap(), 0x2222);
    // ldp x4, x5, [sp], #16
    exec(asm::ldst_pair(2, 1, true, 2, 5, 31, 4), &mut cpu, &mut ram).unwrap();
    assert_eq!(
        (cpu.x[4], cpu.x[5], cpu.sp_el1),
        (0x1111, 0x2222, DATA + 32)
    );
    // ldpsw x6, x7, [x1] → sign-extended words of 0x83828180 / 0x87868584
    exec(asm::ldst_pair(1, 2, true, 0, 7, 1, 6), &mut cpu, &mut ram).unwrap();
    assert_eq!(
        (cpu.x[6], cpu.x[7]),
        (0xffff_ffff_8382_8180, 0xffff_ffff_8786_8584)
    );
    // ldp w8, w9, [x1, #4] zero-extends
    cpu.x[8] = u64::MAX;
    exec(asm::ldst_pair(0, 2, true, 1, 9, 1, 8), &mut cpu, &mut ram).unwrap();
    assert_eq!((cpu.x[8], cpu.x[9]), (0x8786_8584, 0x8b8a_8988));
    // stnp wzr, wzr, [x1]
    exec(
        asm::ldst_pair(0, 0, false, 0, 31, 1, 31),
        &mut cpu,
        &mut ram,
    )
    .unwrap();
    assert_eq!(ram.read_u64(GuestPhysAddr(DATA)).unwrap(), 0);
}

#[test]
fn stp_fault_on_second_half_writes_nothing() {
    // stp x2, x3, [x1] with x1 = end of RAM - 8: the second element is outside RAM.
    let (mut cpu, mut ram) = mem_cpu();
    let end = BASE + RAM as u64;
    cpu.x[1] = end - 8;
    cpu.x[2] = 0xaaaa;
    cpu.x[3] = 0xbbbb;
    let mut before = vec![0u8; 8];
    ram.read(GuestPhysAddr(end - 8), &mut before).unwrap();
    let state = CpuState {
        pc: BASE,
        ..cpu.clone()
    };
    let r = exec(asm::ldst_pair(2, 3, false, 0, 3, 1, 2), &mut cpu, &mut ram);
    assert!(
        matches!(r, Err(ExecError::DataAbort { abort, .. }) if abort.write && matches!(abort.kind, DataFaultKind::External(_)))
    );
    let mut after = vec![0u8; 8];
    ram.read(GuestPhysAddr(end - 8), &mut after).unwrap();
    assert_eq!(before, after);
    assert_eq!(cpu, state);
    // Same for a pre-index store: no writeback either.
    let r = exec(asm::ldst_pair(2, 3, false, 0, 3, 1, 2), &mut cpu, &mut ram);
    assert!(r.is_err());
    assert_eq!(cpu.x[1], end - 8);
}

#[test]
fn sp_alignment_check_follows_sctlr() {
    for (el, sctlr, checked) in [
        (ExceptionLevel::El1, 1u64 << 3, true),
        (ExceptionLevel::El1, 1 << 4, false),
        (ExceptionLevel::El0, 1 << 4, true),
        (ExceptionLevel::El0, 1 << 3, false),
    ] {
        let (mut cpu, mut ram) = mem_cpu();
        cpu.el = el;
        cpu.sys.sctlr_el1 = sctlr;
        cpu.sp_el1 = DATA + 8;
        cpu.sp_el0 = DATA + 8;
        // ldr x0, [sp] with SP misaligned by 8 / ldr x0, [x1] (not SP: never checked)
        let r = exec(asm::ldst_uimm(3, 1, 0, 31, 0), &mut cpu, &mut ram);
        if checked {
            assert_eq!(r, Err(ExecError::SpAlignment { pc: BASE }));
        } else {
            assert!(r.is_ok());
        }
        assert!(exec(asm::ldst_uimm(3, 1, 0, 1, 0), &mut cpu, &mut ram).is_ok());
        // An aligned SP passes.
        cpu.sp_el1 = DATA;
        cpu.sp_el0 = DATA;
        assert!(exec(asm::ldst_uimm(3, 1, 0, 31, 0), &mut cpu, &mut ram).is_ok());
    }
    let _ = Exception::SpAlignment;
}

#[test]
fn read_only_step_rejects_stores_without_side_effects() {
    let mut ram = ram_with(&[asm::ldst_uimm(3, 0, 0, 1, 0)]);
    let mut cpu = CpuState::new(BASE);
    cpu.x[1] = DATA;
    let before = cpu.clone();
    let r = step(&mut cpu, &ram);
    assert!(matches!(r, Err(ExecError::DataAbort { abort, .. }) if abort.write));
    assert_eq!(cpu, before);
    assert_eq!(ram.read_u64(GuestPhysAddr(DATA)).unwrap(), 0);
    let _ = &mut ram;
}

// ---------------------------------------------------------------------------------
// Compiled programs
// ---------------------------------------------------------------------------------

/// `clang --target=aarch64-linux-gnu -O2 -ffreestanding -fno-builtin
/// -fno-stack-protector -mgeneral-regs-only -mno-outline-atomics -c prog.c` (clang 18),
/// `.text` as words. Source (no relocations):
///
/// ```c
/// size_t my_strlen(const char *s) { const char *p = s; while (*p) p++; return p - s; }
/// void *my_memcpy(void *dst, const void *src, size_t n) {
///     u8 *d = dst; const u8 *s = src;
///     while (n >= 16) { unsigned long a = ((const unsigned long *)s)[0];
///                       unsigned long b = ((const unsigned long *)s)[1];
///                       ((unsigned long *)d)[0] = a; ((unsigned long *)d)[1] = b;
///                       d += 16; s += 16; n -= 16; }
///     while (n--) *d++ = *s++;
///     return dst; }
/// int my_memcmp(const void *a, const void *b, size_t n) {
///     const u8 *x = a, *y = b;
///     for (size_t i = 0; i < n; i++) if (x[i] != y[i]) return x[i] < y[i] ? -1 : 1;
///     return 0; }
/// unsigned long mix(const unsigned *v, size_t n, long d) {
///     unsigned long h = 0x9e3779b97f4a7c15ul;
///     for (size_t i = 0; i < n; i++) {
///         h ^= v[i]; h = (h << 13) | (h >> 51); h *= 0xff51afd7ed558ccdul;
///         h += (unsigned long)((long)(int)v[i] / (d ? d : 1));
///         h ^= (unsigned long)__builtin_clzl(h | 1) << 7;
///         if ((long)h < 0) h = ~h; else h = h / 3; }
///     return h; }
/// ```
///
/// Uses: `mov` (ORR/ADD aliases), `ldrb` (register offset, post-index), `ldp/stp`
/// post-index, `strb` post-index, `ldrsw` post-index, `cbz/cbnz`, `b.cond`, `subs/cmp`,
/// `cneg`, `csinc`, `csinv`, `movz/movk`, `mov` (bitmask), `sdiv`, `and/eor/orr` imm and
/// shifted, `extr`, `madd`, `clz`, `umulh`, `tst`, `lsr`.
const PROGRAM: [u32; 62] = [
    0xaa1f03e8, 0x38686809, 0x91000508, 0x35ffffc9, 0xd1000500, 0xd65f03c0, 0xaa0003e8, 0xf100405f,
    0x540000c3, 0xa8c12829, 0xd1004042, 0xf1003c5f, 0xa8812909, 0x54ffff88, 0xb40000a2, 0x38401429,
    0xf1000442, 0x38001509, 0x54ffffa1, 0xd65f03c0, 0xb40000e2, 0x38401408, 0x38401429, 0x6b09011f,
    0x540000a1, 0xf1000442, 0x54ffff61, 0x2a1f03e0, 0xd65f03c0, 0x12800008, 0x5a883500, 0xd65f03c0,
    0xd28f82a8, 0xf2afe948, 0xf2cf3728, 0xf2f3c6e8, 0xb4000301, 0xd29199aa, 0xf100045f, 0xb201f3eb,
    0xf2bdaaaa, 0x9a9f8449, 0xf295556b, 0xf2d5faea, 0xf2ffea2a, 0xb880440c, 0x9ac90d8d, 0x92407d8c,
    0xca0c010c, 0x93c8cd88, 0x9b0a3508, 0xb240010c, 0xdac0118c, 0xca0c1d08, 0x9bcb7d0c, 0xf241011f,
    0xd341fd8c, 0xda880188, 0xf1000421, 0x54fffe41, 0xaa0803e0, 0xd65f03c0,
];
const STRLEN: u64 = 0x0;
const MEMCPY: u64 = 0x18;
const MEMCMP: u64 = 0x50;
const MIX: u64 = 0x80;

/// Return address for calls: a `brk` at the end of the code.
const STOP: u64 = BASE + 4 * PROGRAM.len() as u64;

fn program_ram() -> RamRegion {
    let mut code = PROGRAM.to_vec();
    code.push(asm::brk(0xdead));
    ram_with(&code)
}

/// Calls the function at `BASE + entry` with `args`, returns `x0` and the step count.
fn call(ram: &mut RamRegion, entry: u64, args: &[u64]) -> (u64, usize) {
    let mut cpu = CpuState::new(BASE + entry);
    cpu.sp_el1 = BASE + RAM as u64 - 0x100;
    cpu.sys.sctlr_el1 = 1 << 3; // SP alignment checking on, as Linux has it
    for (i, &a) in args.iter().enumerate() {
        cpu.x[i] = a;
    }
    cpu.x[30] = STOP;
    for n in 0..10_000_000 {
        match step_mut(&mut cpu, ram) {
            Ok(_) => {}
            Err(ExecError::Breakpoint { pc, imm: 0xdead }) if pc == STOP => return (cpu.x[0], n),
            Err(e) => panic!("{e} after {n} steps"),
        }
    }
    panic!("no return");
}

#[test]
fn compiled_strlen() {
    let mut ram = program_ram();
    for len in [0usize, 1, 7, 64, 1000] {
        for misalign in 0..3u64 {
            let s = DATA + misalign;
            let bytes: Vec<u8> = (0..len).map(|i| b'a' + (i % 26) as u8).chain([0]).collect();
            ram.write(GuestPhysAddr(s), &bytes).unwrap();
            assert_eq!(call(&mut ram, STRLEN, &[s]).0, len as u64);
        }
    }
}

#[test]
fn compiled_memcpy_and_memcmp() {
    let mut ram = program_ram();
    let src = DATA + 3;
    let pattern: Vec<u8> = (0..300u32).map(|i| (i * 37 + 11) as u8).collect();
    ram.write(GuestPhysAddr(src), &pattern).unwrap();
    for n in [0usize, 1, 15, 16, 17, 31, 32, 33, 100, 255] {
        for dst_off in [0x1000u64, 0x2001, 0x3007] {
            let dst = DATA + dst_off;
            // Guard bytes around the destination must survive.
            ram.write(GuestPhysAddr(dst - 8), &[0xee; 8 + 256 + 8])
                .unwrap();
            let (ret, _) = call(&mut ram, MEMCPY, &[dst, src, n as u64]);
            assert_eq!(ret, dst);
            let mut out = vec![0u8; n + 16];
            ram.read(GuestPhysAddr(dst - 8), &mut out).unwrap();
            assert_eq!(&out[..8], &[0xee; 8]);
            assert_eq!(&out[8..8 + n], &pattern[..n], "n = {n}");
            assert_eq!(&out[8 + n..], &[0xee; 8]);
            assert_eq!(call(&mut ram, MEMCMP, &[dst, src, n as u64]).0, 0);
        }
    }
    // memcmp ordering: first difference decides, as unsigned bytes.
    let (a, b) = (DATA + 0x4000, DATA + 0x5000);
    ram.write(GuestPhysAddr(a), b"hello\x01").unwrap();
    ram.write(GuestPhysAddr(b), b"hello\xff").unwrap();
    assert_eq!(call(&mut ram, MEMCMP, &[a, b, 6]).0 as u32, (-1i32) as u32);
    assert_eq!(call(&mut ram, MEMCMP, &[b, a, 6]).0 as u32, 1);
    assert_eq!(call(&mut ram, MEMCMP, &[a, b, 5]).0 as u32, 0);
}

fn mix_reference(v: &[u32], d: i64) -> u64 {
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15;
    let dd = if d != 0 { d } else { 1 };
    for &x in v {
        h ^= u64::from(x);
        h = h.rotate_left(13);
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h = h.wrapping_add((i64::from(x as i32) / dd) as u64);
        h ^= u64::from((h | 1).leading_zeros()) << 7;
        h = if (h as i64) < 0 { !h } else { h / 3 };
    }
    h
}

#[test]
fn compiled_arithmetic_mix() {
    let mut ram = program_ram();
    let v: Vec<u32> = (0..200u32)
        .map(|i| i.wrapping_mul(0x9e37_79b9) ^ (i << 7))
        .chain([0x8000_0000, 0xffff_ffff, 0, 1, 0x7fff_ffff])
        .collect();
    let base = DATA + 0x100;
    for (i, x) in v.iter().enumerate() {
        ram.write_u32(GuestPhysAddr(base + 4 * i as u64), *x)
            .unwrap();
    }
    for d in [0i64, 1, 3, -7, -1, i64::MAX] {
        let (h, steps) = call(&mut ram, MIX, &[base, v.len() as u64, d as u64]);
        assert_eq!(h, mix_reference(&v, d), "d = {d}");
        assert!(steps > 3000);
    }
}
