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
use crate::sem::Sem;
use crate::sem::concrete::Concrete;

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

/// Runs `region` once from its entry at `cpu.pc` until it exits.
///
/// Restriction: an [`Exit::Raise`] must come from the instruction at offset 0 (the
/// `Concrete` backend reports the region entry as the faulting PC).
fn eval(region: &Region, cpu: &mut CpuState) -> Result<(), ExecError> {
    region.verify().expect("builder output must verify");
    let mut c = Concrete::new(cpu);
    let mut vals: Vec<Option<Cv>> = vec![None; region.num_values()];
    let mut block = Region::ENTRY;
    for _ in 0..10_000 {
        let data = region.block(block).expect("valid block");
        for inst in &data.insts {
            let r: [Option<Cv>; 2] = match inst.op {
                Op::Iconst { ty, imm } => [Some(Cv::Int(c.imm(ty, imm))), None],
                Op::PcRel { offset } => [Some(Cv::Int(c.pc_rel(offset))), None],
                Op::GetGpr { reg, ty } => [Some(Cv::Int(c.gpr(reg, ty))), None],
                Op::SetGpr { reg, ty, value } => {
                    c.set_gpr(reg, ty, int(&vals, value));
                    [None, None]
                }
                Op::GetNzcv => [Some(Cv::Flags(c.nzcv())), None],
                Op::SetNzcv { flags: f } => {
                    c.set_nzcv(flags(&vals, f));
                    [None, None]
                }
                Op::AddWithCarry { ty, a, b, carry_in } => {
                    let (r, f) =
                        c.add_with_carry(ty, int(&vals, a), int(&vals, b), int(&vals, carry_in));
                    [Some(Cv::Int(r)), Some(Cv::Flags(f))]
                }
                Op::Binary { op, ty, a, b } => [
                    Some(Cv::Int(c.binop(op, ty, int(&vals, a), int(&vals, b)))),
                    None,
                ],
                Op::Not { ty, a } => [Some(Cv::Int(c.not(ty, int(&vals, a)))), None],
                Op::Icmp { cc, ty, a, b } => [
                    Some(Cv::Int(c.icmp(cc, ty, int(&vals, a), int(&vals, b)))),
                    None,
                ],
                Op::Select { ty, cond, a, b } => {
                    let v = c.select(ty, int(&vals, cond), int(&vals, a), int(&vals, b));
                    [Some(Cv::Int(v)), None]
                }
                Op::CondHolds { cond, flags: f } => {
                    [Some(Cv::Int(c.cond_holds(cond, flags(&vals, f)))), None]
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

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn random_state(rng: &mut Rng) -> CpuState {
    let mut cpu = CpuState::new(rng.next() & !3);
    for x in &mut cpu.x {
        // Mix of small, large and 32-bit-boundary values.
        *x = match rng.next() % 4 {
            0 => rng.next() % 8,
            1 => rng.next() & 0xffff_ffff,
            _ => rng.next(),
        };
    }
    cpu.sp_el0 = rng.next();
    cpu.sp_el1 = rng.next();
    cpu.nzcv = Nzcv::from_nibble((rng.next() & 0xf) as u8);
    cpu.el = if rng.next() & 1 == 0 {
        ExceptionLevel::El0
    } else {
        ExceptionLevel::El1
    };
    cpu.sp_sel = rng.next() & 1 == 0;
    cpu
}

/// Every decodable M0 instruction class, plus unknown words: the region built from the
/// single instruction, replayed, yields exactly the state (and error) T0 produces.
#[test]
fn single_instruction_regions_match_t0() {
    const CLASSES: [(u32, u32); 8] = [
        (0x1f80_0000, 0x1100_0000), // add/sub immediate
        (0x1f80_0000, 0x1280_0000), // move wide
        (0x7c00_0000, 0x1400_0000), // b/bl
        (0x7e00_0000, 0x3400_0000), // cbz/cbnz
        (0xff00_0010, 0x5400_0000), // b.cond
        (0xfe9f_fc1f, 0xd61f_0000), // br/blr/ret
        (0xffff_ffff, 0xd503_201f), // nop
        (0x0000_0000, 0x0000_0000), // anything (mostly unknown)
    ];
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut seen_unknown = false;
    for _ in 0..3_000 {
        for (mask, pattern) in CLASSES {
            let word = (rng.next() as u32 & !mask) | pattern;
            let inst = decode(word);
            seen_unknown |= matches!(inst, Inst::Unknown(_));
            let region = Builder::lower_sequence(&[inst]);
            let start = random_state(&mut rng);

            let mut t0 = start.clone();
            let r0 = interp::execute(&mut t0, inst).map(|_| ());
            let mut ir = start.clone();
            let r1 = eval(&region, &mut ir);

            assert_eq!(r1, r0, "{word:#010x} {inst:?}\n{region:#?}");
            assert_eq!(ir, t0, "{word:#010x} {inst:?}\n{region:#?}");
        }
    }
    assert!(seen_unknown);
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
        eval(&region, &mut ir).unwrap();
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
