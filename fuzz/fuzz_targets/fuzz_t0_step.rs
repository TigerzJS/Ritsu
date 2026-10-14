//! T0 interpreter (`ritsu_dbt::step`) on arbitrary instruction words in RAM.
//!
//! Input: 8 header bytes, then instruction words (little-endian) placed at the start of a 4 KiB RAM
//! region. Header: byte 0 bit 0 = EL (0 EL1, 1 EL0), bit 1 = SP select, bits 4..7 = NZCV;
//! bytes 1..8 seed the register file (xorshift; every third register gets a small value).
//! The vCPU runs at most STEP_LIMIT steps; an undefined instruction or an abort ends the run.
//!
//! Invariants (ritsu-dbt interp.rs module docs): no panic on any word, address or register value;
//! `Err` leaves the state unchanged; a sequential step advances `PC` by exactly 4; the RAM is not
//! modified by instruction fetch.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ritsu_core::GuestPhysAddr;
use ritsu_dbt::{CpuState, ExceptionLevel, Nzcv, StepOutcome, step};
use ritsu_fuzz::Reader;
use ritsu_mem::{GuestMemory, RamRegion};

const BASE: u64 = 0x4000_0000;
const RAM_LEN: usize = 0x1000;
const STEP_LIMIT: usize = 512;

fn xorshift(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fuzz_target!(|data: &[u8]| {
    let mut r = Reader::new(data);
    let flags = r.u8();
    let mut seed =
        u64::from_le_bytes([r.u8(), r.u8(), r.u8(), r.u8(), r.u8(), r.u8(), r.u8(), 0x5a]) | 1;

    let mut cpu = CpuState::new(BASE);
    cpu.el = if flags & 1 == 0 {
        ExceptionLevel::El1
    } else {
        ExceptionLevel::El0
    };
    cpu.sp_sel = flags & 2 != 0;
    cpu.nzcv = Nzcv::from_nibble(flags >> 4);
    for (i, x) in cpu.x.iter_mut().enumerate() {
        let v = xorshift(&mut seed);
        *x = if i % 3 == 0 { v & 0xff } else { v };
    }
    cpu.sp_el0 = xorshift(&mut seed);
    cpu.sp_el1 = xorshift(&mut seed);

    let mut ram = RamRegion::new(GuestPhysAddr(BASE), RAM_LEN);
    let code = r.bytes(RAM_LEN);
    ram.write(GuestPhysAddr(BASE), code).expect("code fits RAM");
    let mut ram_before = vec![0u8; RAM_LEN];
    ram.read(GuestPhysAddr(BASE), &mut ram_before)
        .expect("RAM readable");

    for _ in 0..STEP_LIMIT {
        let before = cpu.clone();
        match step(&mut cpu, &ram) {
            Ok(StepOutcome::Sequential) => {
                assert_eq!(
                    cpu.pc,
                    before.pc.wrapping_add(4),
                    "sequential step moved PC"
                );
            }
            Ok(StepOutcome::Branch { .. }) => {}
            Err(_) => {
                assert_eq!(cpu, before, "failed step changed the state");
                break;
            }
        }
    }

    let mut ram_after = vec![0u8; RAM_LEN];
    ram.read(GuestPhysAddr(BASE), &mut ram_after)
        .expect("RAM readable");
    assert_eq!(ram_before, ram_after);
});
