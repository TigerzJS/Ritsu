//! IR tests: builder output verifies, and replaying it gives exactly the T0 result.
//!
//! The evaluator below interprets a [`Region`] by replaying each IR operation on the
//! [`Concrete`] backend, so it adds no semantics of its own; what it checks is that the
//! [`Builder`] records the descriptions faithfully (operands, results, block structure,
//! exits, relative offsets). Together with the operation conformance tests this
//! covers the semantic layer in both backends.

use super::*;
use crate::asm;
use crate::cpu::{CpuState, ExceptionLevel, Nzcv};
use crate::decode::{Inst, decode};
use crate::interp::{self, ExecError};
use crate::mem::{DataBus, MemBus, NoMemory};
use crate::sem::Ctx;
use crate::sem::Sem;
use crate::sem::concrete::Concrete;
use crate::testutil::{CLASSES, Rng};
use crate::tier::TbFlags;
use ritsu_core::GuestPhysAddr;
use ritsu_mem::mmu::IdentityTranslate;
use ritsu_mem::{GuestMemory, RamRegion};

#[derive(Clone, Copy, Debug)]
enum Cv {
    Int(u64),
    Flags(Nzcv),
}

fn int(vals: &[Option<Cv>], v: Value) -> u64 {
    match vals[v.index()] {
        Some(Cv::Int(x)) => x,
        other => panic!("{v} is {other:?}, expected an integer"),
    }
}

fn flags(vals: &[Option<Cv>], v: Value) -> Nzcv {
    match vals[v.index()] {
        Some(Cv::Flags(f)) => f,
        other => panic!("{v} is {other:?}, expected flags"),
    }
}

/// Runs `region` once from its entry at `cpu.pc` until it exits, on memory `bus`.
///
/// Restriction: an [`Exit::Raise`] must come from the instruction at offset 0 (the
/// `Concrete` backend reports the region entry as the faulting PC); the same holds for
/// data aborts.
fn eval(region: &Region, cpu: &mut CpuState, bus: &mut dyn DataBus) -> Result<(), ExecError> {
    region.verify().expect("builder output must verify");
    let mut c = Concrete::new(cpu, bus);
    let mut vals: Vec<Option<Cv>> = vec![None; region.num_values()];
    let mut block = Region::ENTRY;
    for _ in 0..10_000 {
        let data = region.block(block).expect("valid block");
        for inst in &data.insts {
            let i = |v: Value| int(&vals, v);
            let fl = |v: Value| flags(&vals, v);
            let r: [Option<Cv>; 2] = match inst.op {
                Op::Iconst { ty, imm } => [Some(Cv::Int(c.imm(ty, imm))), None],
                Op::PcRel { offset } => [Some(Cv::Int(c.pc_rel(offset))), None],
                Op::GetGpr { reg, ty } => [Some(Cv::Int(c.gpr(reg, ty))), None],
                Op::SetGpr { reg, ty, value } => {
                    c.set_gpr(reg, ty, i(value));
                    [None, None]
                }
                Op::GetNzcv => [Some(Cv::Flags(c.nzcv())), None],
                Op::SetNzcv { flags: f } => {
                    c.set_nzcv(fl(f));
                    [None, None]
                }
                Op::GetSysreg { reg } => [Some(Cv::Int(c.sysreg(reg))), None],
                Op::SetSysreg { reg, value } => {
                    c.set_sysreg(reg, i(value));
                    [None, None]
                }
                Op::AddWithCarry { ty, a, b, carry_in } => {
                    let (r, f) = c.add_with_carry(ty, i(a), i(b), i(carry_in));
                    [Some(Cv::Int(r)), Some(Cv::Flags(f))]
                }
                Op::SubWithCarry { ty, a, b, carry_in } => {
                    let (r, f) = c.sub_with_carry(ty, i(a), i(b), i(carry_in));
                    [Some(Cv::Int(r)), Some(Cv::Flags(f))]
                }
                Op::FlagsLogic { ty, a } => [Some(Cv::Flags(c.flags_logic(ty, i(a)))), None],
                Op::FlagC { flags: f } => [Some(Cv::Int(c.flag_c(fl(f)))), None],
                Op::SelectFlags { cond, a, b } => {
                    [Some(Cv::Flags(c.select_flags(i(cond), fl(a), fl(b)))), None]
                }
                Op::FlagsToBits { flags: f } => [Some(Cv::Int(c.flags_to_bits(fl(f)))), None],
                Op::BitsToFlags { a } => [Some(Cv::Flags(c.bits_to_flags(i(a)))), None],
                Op::Binary { op, ty, a, b } => [Some(Cv::Int(c.binop(op, ty, i(a), i(b)))), None],
                Op::Unary { op, ty, a } => [Some(Cv::Int(c.unop(op, ty, i(a)))), None],
                Op::Not { ty, a } => [Some(Cv::Int(c.not(ty, i(a)))), None],
                Op::Ext {
                    signed: false,
                    from,
                    to,
                    a,
                } => [Some(Cv::Int(c.zext(from, to, i(a)))), None],
                Op::Ext {
                    signed: true,
                    from,
                    to,
                    a,
                } => [Some(Cv::Int(c.sext(from, to, i(a)))), None],
                Op::Trunc { from, to, a } => [Some(Cv::Int(c.trunc(from, to, i(a)))), None],
                Op::Icmp { cc, ty, a, b } => [Some(Cv::Int(c.icmp(cc, ty, i(a), i(b)))), None],
                Op::Select { ty, cond, a, b } => {
                    [Some(Cv::Int(c.select(ty, i(cond), i(a), i(b)))), None]
                }
                Op::CondHolds { cond, flags: f } => {
                    [Some(Cv::Int(c.cond_holds(cond, fl(f)))), None]
                }
                Op::Load { ty, attrs, addr } => {
                    assert_eq!(inst.pc_offset, 0, "eval restriction");
                    [Some(Cv::Int(c.load(ty, i(addr), attrs)?)), None]
                }
                Op::Store {
                    ty,
                    attrs,
                    addr,
                    value,
                } => {
                    assert_eq!(inst.pc_offset, 0, "eval restriction");
                    c.store(ty, i(addr), i(value), attrs)?;
                    [None, None]
                }
                Op::Probe {
                    ty,
                    count,
                    attrs,
                    addr,
                } => {
                    assert_eq!(inst.pc_offset, 0, "eval restriction");
                    c.probe(ty, count, i(addr), attrs)?;
                    [None, None]
                }
            };
            for (slot, value) in inst.results.iter().zip(r) {
                if let Some(v) = slot {
                    vals[v.index()] = value;
                }
            }
        }
        let (dest, args) = match data.term.as_ref().expect("terminated") {
            Terminator::Jump { dest, args } => (*dest, args.clone()),
            Terminator::Branch {
                cond,
                then_dest,
                then_args,
                else_dest,
                else_args,
            } => {
                if int(&vals, *cond) != 0 {
                    (*then_dest, then_args.clone())
                } else {
                    (*else_dest, else_args.clone())
                }
            }
            Terminator::Exit(exit) => {
                match *exit {
                    Exit::Direct { offset, kind } => c.branch(None, offset, kind),
                    Exit::Indirect { target, kind } => c.branch_indirect(int(&vals, target), kind),
                    Exit::Raise {
                        exception,
                        pc_offset,
                    } => {
                        assert_eq!(pc_offset, 0, "eval restriction");
                        return c.raise(exception);
                    }
                }
                c.finish();
                return Ok(());
            }
        };
        let params = &region.block(dest).expect("valid block").params;
        let incoming: Vec<_> = args.iter().map(|a| vals[a.index()]).collect();
        for (p, v) in params.iter().zip(incoming) {
            vals[p.index()] = v;
        }
        block = dest;
    }
    panic!("region did not exit");
}

const RAM_BASE: u64 = 0x4000_0000;
const RAM_LEN: usize = 0x2000;

fn random_state(rng: &mut Rng) -> CpuState {
    let mut cpu = CpuState::new(rng.next() & !3);
    let value = |rng: &mut Rng| match rng.next() % 6 {
        0 => rng.next() % 8,
        1 => rng.next() & 0xffff_ffff,
        // Addresses into the test RAM (any alignment), so loads and stores succeed.
        2 | 3 => RAM_BASE + 0x100 + rng.next() % (RAM_LEN as u64 - 0x200),
        _ => rng.next(),
    };
    for i in 0..31 {
        cpu.x[i] = value(rng);
    }
    cpu.sp_el0 = value(rng);
    cpu.sp_el1 = value(rng);
    cpu.nzcv = Nzcv::from_nibble((rng.next() & 0xf) as u8);
    cpu.el = if rng.next() & 1 == 0 {
        ExceptionLevel::El0
    } else {
        ExceptionLevel::El1
    };
    cpu.sp_sel = rng.next() & 1 == 0;
    // SCTLR_EL1.{A, SA, SA0, EnIB, EnIA}: every context-dependent path is exercised.
    let bits = rng.next();
    cpu.sys.sctlr_el1 = (bits & 0b11010) | (bits & (3 << 30));
    cpu.sys.fpcr = rng.next();
    cpu.sys.fpsr = rng.next();
    cpu.sys.tpidr_el0 = rng.next();
    cpu
}

fn test_ram(rng: &mut Rng) -> RamRegion {
    let mut ram = RamRegion::new(GuestPhysAddr(RAM_BASE), RAM_LEN);
    // Eight bytes per RNG step keeps this cheap in debug builds.
    let bytes: Vec<u8> = (0..RAM_LEN / 8)
        .flat_map(|_| rng.next().to_le_bytes())
        .collect();
    ram.write(GuestPhysAddr(RAM_BASE), &bytes).unwrap();
    ram
}

fn ram_bytes(ram: &RamRegion) -> Vec<u8> {
    let mut b = vec![0; RAM_LEN];
    ram.read(GuestPhysAddr(RAM_BASE), &mut b).unwrap();
    b
}

/// Every decodable instruction class, plus unknown words: the region built from the
/// single instruction (specialised on the start state's context), replayed, yields
/// exactly the state, memory and error T0 produces, and the static rule check finds no
/// violation.
#[test]
fn single_instruction_regions_match_t0() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut seen_unknown = false;
    let mut mem_ok = 0;
    for _ in 0..1_500 {
        for (name, class) in CLASSES {
            let word = rng.word_in(class);
            let inst = decode(word);
            seen_unknown |= matches!(inst, Inst::Unknown(_));
            let start = random_state(&mut rng);
            let (region, violations) = Builder::lower_sequence_checked(
                Builder::with_ctx(Ctx::from_state(&start)),
                &[inst],
            );
            assert_eq!(violations, vec![], "{name} {word:#010x} {inst:?}");

            let ram = test_ram(&mut rng);
            let mut ram0 = test_ram(&mut Rng(1));
            ram0.write(GuestPhysAddr(RAM_BASE), &ram_bytes(&ram))
                .unwrap();
            let mut ram1 = test_ram(&mut Rng(1));
            ram1.write(GuestPhysAddr(RAM_BASE), &ram_bytes(&ram))
                .unwrap();

            let mut t0 = start.clone();
            let mut mmu = IdentityTranslate::default();
            let r0 = interp::execute_with(&mut t0, inst, &mut MemBus::new(&mut ram0, &mut mmu))
                .map(|_| ());
            let mut ir = start.clone();
            let mut mmu = IdentityTranslate::default();
            let r1 = eval(&region, &mut ir, &mut MemBus::new(&mut ram1, &mut mmu));

            assert_eq!(r1, r0, "{word:#010x} {inst:?}\n{region:#?}");
            assert_eq!(ir, t0, "{word:#010x} {inst:?}\n{region:#?}");
            assert!(
                ram_bytes(&ram0) == ram_bytes(&ram1),
                "{word:#010x} memory differs"
            );
            if r0.is_err() {
                assert_eq!(t0, start, "{word:#010x}: failed instruction changed state");
                assert!(
                    ram_bytes(&ram0) == ram_bytes(&ram),
                    "{word:#010x}: failed instruction wrote memory"
                );
            }
            if r0.is_ok() && name.starts_with("ldst") {
                mem_ok += 1;
            }
        }
    }
    assert!(seen_unknown);
    assert!(mem_ok > 500, "only {mem_ok} successful memory instructions");
}

/// The static rule check (R-DBT-01 P1-4) over many words of every class, in every
/// combination of the context bits descriptions branch on.
#[test]
fn every_description_obeys_the_rules_statically() {
    let mut rng = Rng(0x0dd_ba11_c0ff_ee00);
    let ctxs: Vec<Ctx> = (0..32u32)
        .map(|bits| Ctx::from_tb_flags(TbFlags(bits)))
        .collect();
    for (name, class) in CLASSES {
        for _ in 0..400 {
            let word = rng.word_in(class);
            let inst = decode(word);
            for &ctx in &ctxs {
                let (region, violations) =
                    Builder::lower_sequence_checked(Builder::with_ctx(ctx), &[inst]);
                assert_eq!(violations, vec![], "{name} {word:#010x} {inst:?} {ctx:?}");
                assert_eq!(region.verify(), Ok(()), "{name} {word:#010x} {inst:?}");
            }
        }
    }
}

#[test]
fn rule_check_reports_violations() {
    use crate::sem::{BranchKind as K, MemAttrs};
    let x0 = GpReg::x(0).unwrap();
    let check = |f: &dyn Fn(&mut Builder)| -> Vec<RuleViolationKind> {
        let mut b = Builder::new();
        b.set_pc_offset(8);
        f(&mut b);
        let v: Vec<_> = b.violations().iter().map(|v| v.kind).collect();
        assert!(b.violations().iter().all(|v| v.pc_offset == 8));
        v
    };
    // Two branches.
    assert_eq!(
        check(&|b| {
            b.branch(None, 4, K::Jump);
            b.branch(None, 8, K::Jump);
        }),
        vec![RuleViolationKind::AfterControlTransfer]
    );
    // A write after the branch.
    assert_eq!(
        check(&|b| {
            let v = b.imm(Ty::I64, 1);
            b.branch_indirect(v, K::Jump);
            b.set_gpr(x0, Ty::I64, v);
        }),
        vec![RuleViolationKind::AfterControlTransfer]
    );
    // Raise after a register write, load after a write, store after a write.
    assert_eq!(
        check(&|b| {
            let v = b.imm(Ty::I64, 1);
            b.set_gpr(x0, Ty::I64, v);
            let _ = b.load(Ty::I8, v, MemAttrs::PLAIN);
            let _ = b.store(Ty::I8, v, v, MemAttrs::PLAIN);
            let c = b.imm(Ty::I1, 1);
            let _ = b.raise_if(c, Exception::SpAlignment);
            let _ = b.raise(Exception::SpAlignment);
        }),
        vec![
            RuleViolationKind::RaiseAfterWrite,
            RuleViolationKind::StoreAfterWrite,
            RuleViolationKind::RaiseAfterWrite,
            RuleViolationKind::RaiseAfterWrite,
        ]
    );
    // Second store without probe; load after store.
    assert_eq!(
        check(&|b| {
            let a = b.imm(Ty::I64, 0x1000);
            let v = b.imm(Ty::I8, 1);
            let _ = b.store(Ty::I8, a, v, MemAttrs::PLAIN);
            let _ = b.store(Ty::I8, a, v, MemAttrs::PLAIN);
            let _ = b.load(Ty::I8, a, MemAttrs::PLAIN);
        }),
        vec![
            RuleViolationKind::UnprobedSecondStore,
            RuleViolationKind::RaiseAfterWrite
        ]
    );
    // Correct multi-store: probe first.
    assert_eq!(
        check(&|b| {
            let a = b.imm(Ty::I64, 0x1000);
            let v = b.imm(Ty::I8, 1);
            let _ = b.probe(Ty::I8, 2, a, MemAttrs::PLAIN);
            let _ = b.store(Ty::I8, a, v, MemAttrs::PLAIN);
            let _ = b.store(Ty::I8, a, v, MemAttrs::PLAIN);
            b.set_gpr(x0, Ty::I64, a);
        }),
        vec![]
    );
    // The rule state resets at the next instruction.
    let mut b = Builder::new();
    b.branch(None, 4, K::Jump);
    b.set_pc_offset(4);
    let v = b.imm(Ty::I64, 0);
    b.set_gpr(x0, Ty::I64, v);
    assert_eq!(b.violations(), &[]);
}

#[test]
fn raise_if_splits_the_block() {
    let insts = [decode(asm::ldst_uimm(3, 1, 0, 31, 0))]; // ldr x0, [sp]
    let ctx = Ctx::from_tb_flags(TbFlags(TbFlags::EL1 | TbFlags::SP_ALIGN));
    let (region, v) = Builder::lower_sequence_checked(Builder::with_ctx(ctx), &insts);
    assert_eq!(v, vec![]);
    assert_eq!(region.verify(), Ok(()));
    let raises = region
        .blocks()
        .iter()
        .filter(|b| {
            matches!(
                b.term,
                Some(Terminator::Exit(Exit::Raise {
                    exception: Exception::SpAlignment,
                    ..
                }))
            )
        })
        .count();
    assert_eq!(raises, 1);
    // Without the SA check there is no conditional exit at all.
    let region = Builder::lower_sequence(&insts);
    assert_eq!(region.blocks().len(), 1);
    // Effects classes of the memory ops.
    let load = region.blocks()[0]
        .insts
        .iter()
        .find(|i| matches!(i.op, Op::Load { .. }))
        .expect("a load");
    assert!(load.op.effects().may_raise && load.op.effects().reads_mem);
    assert!(Op::GetNzcv.effects().reads_state);
}

#[test]
fn verifier_rejects_wrong_conversion_direction() {
    let r = Region {
        blocks: vec![BlockData {
            params: vec![],
            insts: vec![
                inst(
                    Op::Iconst {
                        ty: Ty::I64,
                        imm: 1,
                    },
                    [Some(Value(0)), None],
                ),
                inst(
                    Op::Ext {
                        signed: false,
                        from: Ty::I64,
                        to: Ty::I32,
                        a: Value(0),
                    },
                    [Some(Value(1)), None],
                ),
            ],
            term: exit(),
        }],
        value_types: vec![Type::I64, Type::I32],
    };
    assert_eq!(r.verify(), Err(VerifyError::BadConversion(Block(0))));
}

/// A whole basic block (the countdown loop body) as one region, iterated to the end.
#[test]
fn loop_body_region_matches_t0() {
    // loop: add x1, x1, #3 ; subs x0, x0, #1 ; b.ne loop
    let words = [
        asm::add_sub_imm(true, false, false, false, 3, 1, 1),
        asm::add_sub_imm(true, true, true, false, 1, 0, 0),
        asm::b_cond(Cond::Ne, -8),
    ];
    let insts: Vec<Inst> = words.iter().map(|&w| decode(w)).collect();
    let region = Builder::lower_sequence(&insts);
    assert_eq!(region.verify(), Ok(()));

    let base = 0x4000_0000u64;
    let mut t0 = CpuState::new(base);
    t0.x[0] = 5;
    let mut ir = t0.clone();
    for _ in 0..5 {
        for &inst in &insts {
            interp::execute(&mut t0, inst).unwrap();
        }
        eval(&region, &mut ir, &mut NoMemory).unwrap();
        assert_eq!(ir, t0);
    }
    assert_eq!(ir.pc, base + 12);
    assert_eq!(ir.x[1], 15);
    assert_eq!(ir.nzcv, Nzcv::new(false, true, true, false));
}

#[test]
fn conditional_branch_shape() {
    // b.eq +16 at offset 4 inside a region: taken exit at offset 20, fall-through 8.
    let insts = [Inst::Nop, decode(asm::b_cond(Cond::Eq, 16))];
    let region = Builder::lower_sequence(&insts);
    assert_eq!(region.verify(), Ok(()));
    let blocks = region.blocks();
    assert_eq!(blocks.len(), 3);
    let entry = &blocks[0];
    assert!(matches!(
        entry.insts.as_slice(),
        [
            InstData {
                op: Op::GetNzcv,
                pc_offset: 4,
                ..
            },
            InstData {
                op: Op::CondHolds { cond: Cond::Eq, .. },
                pc_offset: 4,
                ..
            }
        ]
    ));
    let Some(Terminator::Branch {
        then_dest,
        else_dest,
        ..
    }) = &entry.term
    else {
        panic!("expected a branch: {entry:?}");
    };
    assert_eq!(
        region.block(*then_dest).unwrap().term,
        Some(Terminator::Exit(Exit::Direct {
            offset: 20,
            kind: BranchKind::Jump
        }))
    );
    assert_eq!(
        region.block(*else_dest).unwrap().term,
        Some(Terminator::Exit(Exit::Direct {
            offset: 8,
            kind: BranchKind::Jump
        }))
    );
}

#[test]
fn unconditional_exit_leaves_no_dead_block() {
    for (word, kind) in [
        (asm::b(true, 0x40), BranchKind::Call),
        (asm::br(2, 30), BranchKind::Return),
    ] {
        let region = Builder::lower_sequence(&[decode(word)]);
        assert_eq!(region.verify(), Ok(()));
        assert_eq!(region.blocks().len(), 1);
        let got = match region.blocks()[0].term {
            Some(Terminator::Exit(Exit::Direct { kind, .. } | Exit::Indirect { kind, .. })) => kind,
            ref other => panic!("{other:?}"),
        };
        assert_eq!(got, kind);
    }
    let region = Builder::lower_sequence(&[Inst::Unknown(0)]);
    assert_eq!(
        region.blocks()[0].term,
        Some(Terminator::Exit(Exit::Raise {
            exception: Exception::Undefined { word: 0 },
            pc_offset: 0
        }))
    );
}

#[test]
fn pc_rel_is_relative_to_region_entry() {
    // bl at offset 8: link value = entry + 12.
    let region = Builder::lower_sequence(&[Inst::Nop, Inst::Nop, Inst::Bl { offset: -8 }]);
    let ops: Vec<Op> = region.blocks()[0].insts.iter().map(|i| i.op).collect();
    assert!(ops.contains(&Op::PcRel { offset: 12 }));
    assert_eq!(
        region.blocks()[0].term,
        Some(Terminator::Exit(Exit::Direct {
            offset: 0,
            kind: BranchKind::Call
        }))
    );
}

// ---------------------------------------------------------------------------------
// Verifier
// ---------------------------------------------------------------------------------

fn inst(op: Op, results: [Option<Value>; 2]) -> InstData {
    InstData {
        op,
        results,
        pc_offset: 0,
    }
}

fn exit() -> Option<Terminator> {
    Some(Terminator::Exit(Exit::Direct {
        offset: 4,
        kind: BranchKind::Jump,
    }))
}

#[test]
fn verifier_rejects_malformed_regions() {
    let x0 = GpReg::x(0).unwrap();
    assert_eq!(Region::default().verify(), Err(VerifyError::Empty));

    // Unterminated block.
    let r = Region {
        blocks: vec![BlockData::default()],
        value_types: vec![],
    };
    assert_eq!(r.verify(), Err(VerifyError::Unterminated(Block(0))));

    // Type mismatch: I32 value written as I64.
    let r = Region {
        blocks: vec![BlockData {
            params: vec![],
            insts: vec![
                inst(
                    Op::Iconst {
                        ty: Ty::I32,
                        imm: 1,
                    },
                    [Some(Value(0)), None],
                ),
                inst(
                    Op::SetGpr {
                        reg: x0,
                        ty: Ty::I64,
                        value: Value(0),
                    },
                    [None, None],
                ),
            ],
            term: exit(),
        }],
        value_types: vec![Type::I32],
    };
    assert_eq!(
        r.verify(),
        Err(VerifyError::Type {
            value: Value(0),
            expected: Type::I64,
            found: Type::I32
        })
    );

    // Use before definition in the same block.
    let r = Region {
        blocks: vec![BlockData {
            params: vec![],
            insts: vec![
                inst(Op::SetNzcv { flags: Value(0) }, [None, None]),
                inst(Op::GetNzcv, [Some(Value(0)), None]),
            ],
            term: exit(),
        }],
        value_types: vec![Type::Flags],
    };
    assert_eq!(
        r.verify(),
        Err(VerifyError::Undefined {
            value: Value(0),
            block: Block(0)
        })
    );

    // Value defined on only one of two paths into a join block.
    let cond = Value(0);
    let r = Region {
        blocks: vec![
            BlockData {
                params: vec![],
                insts: vec![inst(Op::Iconst { ty: Ty::I1, imm: 1 }, [Some(cond), None])],
                term: Some(Terminator::Branch {
                    cond,
                    then_dest: Block(1),
                    then_args: vec![],
                    else_dest: Block(2),
                    else_args: vec![],
                }),
            },
            BlockData {
                params: vec![],
                insts: vec![inst(Op::GetNzcv, [Some(Value(1)), None])],
                term: Some(Terminator::Jump {
                    dest: Block(2),
                    args: vec![],
                }),
            },
            BlockData {
                params: vec![],
                insts: vec![inst(Op::SetNzcv { flags: Value(1) }, [None, None])],
                term: exit(),
            },
        ],
        value_types: vec![Type::I1, Type::Flags],
    };
    assert_eq!(
        r.verify(),
        Err(VerifyError::Undefined {
            value: Value(1),
            block: Block(2)
        })
    );

    // Block arguments: wrong count, wrong target.
    let mut bad = r.clone();
    bad.blocks[1].term = Some(Terminator::Jump {
        dest: Block(2),
        args: vec![Value(1)],
    });
    assert!(matches!(bad.verify(), Err(VerifyError::ArgCount { .. })));
    bad.blocks[1].term = Some(Terminator::Jump {
        dest: Block(9),
        args: vec![],
    });
    assert_eq!(
        bad.verify(),
        Err(VerifyError::NoSuchBlock {
            from: Block(1),
            to: Block(9)
        })
    );

    // Passing the value as a block parameter fixes the join.
    let mut good = r.clone();
    good.value_types.push(Type::Flags); // v2: parameter of block 2
    good.blocks[2].params = vec![Value(2)];
    good.blocks[2].insts = vec![inst(Op::SetNzcv { flags: Value(2) }, [None, None])];
    good.blocks[0]
        .insts
        .push(inst(Op::GetNzcv, [Some(Value(3)), None]));
    good.value_types.push(Type::Flags); // v3
    good.blocks[0].term = Some(Terminator::Branch {
        cond,
        then_dest: Block(1),
        then_args: vec![],
        else_dest: Block(2),
        else_args: vec![Value(3)],
    });
    good.blocks[1].term = Some(Terminator::Jump {
        dest: Block(2),
        args: vec![Value(1)],
    });
    assert_eq!(good.verify(), Ok(()));

    // Double definition.
    let r = Region {
        blocks: vec![BlockData {
            params: vec![],
            insts: vec![
                inst(Op::GetNzcv, [Some(Value(0)), None]),
                inst(Op::GetNzcv, [Some(Value(0)), None]),
            ],
            term: exit(),
        }],
        value_types: vec![Type::Flags],
    };
    assert_eq!(r.verify(), Err(VerifyError::BadDefinition(Value(0))));

    // Entry block with parameters.
    let r = Region {
        blocks: vec![BlockData {
            params: vec![Value(0)],
            insts: vec![],
            term: exit(),
        }],
        value_types: vec![Type::I64],
    };
    assert_eq!(r.verify(), Err(VerifyError::EntryParams));
}

#[test]
fn unreachable_predecessor_does_not_constrain_join() {
    // block0: v0 = nzcv; jump block2
    // block1 (dead): jump block2
    // block2: set_nzcv v0  -- dominated by block0 on every *reachable* path.
    let r = Region {
        blocks: vec![
            BlockData {
                params: vec![],
                insts: vec![inst(Op::GetNzcv, [Some(Value(0)), None])],
                term: Some(Terminator::Jump {
                    dest: Block(2),
                    args: vec![],
                }),
            },
            BlockData {
                params: vec![],
                insts: vec![],
                term: Some(Terminator::Jump {
                    dest: Block(2),
                    args: vec![],
                }),
            },
            BlockData {
                params: vec![],
                insts: vec![inst(Op::SetNzcv { flags: Value(0) }, [None, None])],
                term: exit(),
            },
        ],
        value_types: vec![Type::Flags],
    };
    assert_eq!(r.verify(), Ok(()));

    // The dead block itself may not use values from other blocks (nothing dominates
    // an unreachable block) ...
    let mut dead_use = r.clone();
    dead_use.blocks[1].insts = vec![inst(Op::SetNzcv { flags: Value(0) }, [None, None])];
    assert_eq!(
        dead_use.verify(),
        Err(VerifyError::Undefined {
            value: Value(0),
            block: Block(1)
        })
    );
    // ... but its own earlier definitions are fine.
    let mut dead_local = r;
    dead_local.value_types.push(Type::Flags);
    dead_local.blocks[1].insts = vec![
        inst(Op::GetNzcv, [Some(Value(1)), None]),
        inst(Op::SetNzcv { flags: Value(1) }, [None, None]),
    ];
    assert_eq!(dead_local.verify(), Ok(()));
}

#[test]
fn verifier_rejects_non_canonical_constant() {
    let r = Region {
        blocks: vec![BlockData {
            params: vec![],
            insts: vec![inst(
                Op::Iconst {
                    ty: Ty::I32,
                    imm: 1 << 32,
                },
                [Some(Value(0)), None],
            )],
            term: exit(),
        }],
        value_types: vec![Type::I32],
    };
    assert_eq!(
        r.verify(),
        Err(VerifyError::NonCanonicalConst {
            block: Block(0),
            ty: Ty::I32,
            imm: 1 << 32
        })
    );
    // I1 accepts only 0 and 1.
    let mut r1 = r.clone();
    r1.value_types = vec![Type::I1];
    r1.blocks[0].insts[0].op = Op::Iconst { ty: Ty::I1, imm: 2 };
    assert!(matches!(
        r1.verify(),
        Err(VerifyError::NonCanonicalConst { .. })
    ));
    r1.blocks[0].insts[0].op = Op::Iconst { ty: Ty::I1, imm: 1 };
    assert_eq!(r1.verify(), Ok(()));
}

#[test]
fn dominance_with_loop() {
    // block0: v0 = 0 (I64); jump block1(v0)
    // block1(v1): v2 = nzcv; v3 = cond_holds(eq, v2); br v3 ? block2 : block3
    // block2: v4 = v1 + v0; jump block1(v4)     (back edge)
    // block3: set x0 = v1; exit
    let x0 = GpReg::x(0).unwrap();
    let r = Region {
        blocks: vec![
            BlockData {
                params: vec![],
                insts: vec![inst(
                    Op::Iconst {
                        ty: Ty::I64,
                        imm: 0,
                    },
                    [Some(Value(0)), None],
                )],
                term: Some(Terminator::Jump {
                    dest: Block(1),
                    args: vec![Value(0)],
                }),
            },
            BlockData {
                params: vec![Value(1)],
                insts: vec![
                    inst(Op::GetNzcv, [Some(Value(2)), None]),
                    inst(
                        Op::CondHolds {
                            cond: Cond::Eq,
                            flags: Value(2),
                        },
                        [Some(Value(3)), None],
                    ),
                ],
                term: Some(Terminator::Branch {
                    cond: Value(3),
                    then_dest: Block(2),
                    then_args: vec![],
                    else_dest: Block(3),
                    else_args: vec![],
                }),
            },
            BlockData {
                params: vec![],
                insts: vec![inst(
                    Op::Binary {
                        op: BinOp::Add,
                        ty: Ty::I64,
                        a: Value(1),
                        b: Value(0),
                    },
                    [Some(Value(4)), None],
                )],
                term: Some(Terminator::Jump {
                    dest: Block(1),
                    args: vec![Value(4)],
                }),
            },
            BlockData {
                params: vec![],
                insts: vec![inst(
                    Op::SetGpr {
                        reg: x0,
                        ty: Ty::I64,
                        value: Value(1),
                    },
                    [None, None],
                )],
                term: exit(),
            },
        ],
        value_types: vec![Type::I64, Type::I64, Type::Flags, Type::I1, Type::I64],
    };
    assert_eq!(r.verify(), Ok(()));

    // Using the latch's value in the exit block is invalid: block2 does not dominate
    // block3.
    let mut bad = r.clone();
    bad.blocks[3].insts[0].op = Op::SetGpr {
        reg: x0,
        ty: Ty::I64,
        value: Value(4),
    };
    assert_eq!(
        bad.verify(),
        Err(VerifyError::Undefined {
            value: Value(4),
            block: Block(3)
        })
    );
    // Neither may the header use it directly (only via the block parameter).
    let mut bad = r;
    bad.blocks[1].insts.push(inst(
        Op::SetGpr {
            reg: x0,
            ty: Ty::I64,
            value: Value(4),
        },
        [None, None],
    ));
    assert_eq!(
        bad.verify(),
        Err(VerifyError::Undefined {
            value: Value(4),
            block: Block(1)
        })
    );
}

/// The review case: a 16 KiB page full of `CBZ` gives a region of 8193 blocks. The
/// verifier must handle it with memory linear in the region size (no per-block value
/// sets); this test only checks the result, not the time.
#[test]
fn full_page_of_conditional_branches_verifies() {
    let cbz = decode(asm::cb(true, false, 8, 0));
    let insts = vec![cbz; 4096];
    let region = Builder::lower_sequence(&insts);
    assert_eq!(region.blocks().len(), 2 * 4096 + 1);
    assert_eq!(region.verify(), Ok(()));
}
