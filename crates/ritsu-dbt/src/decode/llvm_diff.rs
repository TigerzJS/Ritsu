//! Decoder differential test against LLVM's AArch64 disassembler (`docs/process/testing.md`
//! §4.4a): `llvm-mc` assembles random words of every decoded class as `.inst`
//! directives into an ELF object, `llvm-objdump -d --mattr=+all` disassembles them, and
//! every word is compared with [`print`], a printer of decoded [`Inst`]s in LLVM syntax
//! (including LLVM's/Arm's preferred aliases).
//!
//! Rules:
//!
//! * LLVM says `<unknown>` → the decoder must return [`Inst::Unknown`].
//! * The decoder decodes a word → LLVM's text must match [`print`] after normalisation
//!   (case, whitespace, comments, numeric immediates compared by value).
//! * LLVM decodes, the decoder does not → a *gap* (not yet implemented or deliberately
//!   UNDEFINED); every gap mnemonic must be on the explained list [`GAPS`].
//!
//! LLVM is an oracle for decoding only. The test is skipped (with a message) when the
//! LLVM tools are not installed; CI has llvm-18.

use super::*;
use crate::testutil::{CLASSES, Rng};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::Command;

/// Mnemonics LLVM decodes in the tested classes that Ritsu deliberately leaves
/// [`Inst::Unknown`], with the reason.
const GAPS: &[(&str, &str)] = &[
    ("udf", "permanently undefined"),
    ("hvc", "UNDEFINED without EL2"),
    ("smc", "UNDEFINED without EL3"),
    ("hlt", "UNDEFINED without halting debug"),
    ("dcps1", "UNDEFINED outside Debug state"),
    ("dcps2", "UNDEFINED outside Debug state"),
    ("dcps3", "UNDEFINED outside Debug state"),
    ("tcancel", "FEAT_TME not reported"),
    ("addg", "FEAT_MTE not reported"),
    ("subg", "FEAT_MTE not reported"),
    ("stgp", "FEAT_MTE not reported"),
    ("subp", "FEAT_MTE not reported"),
    ("subps", "FEAT_MTE not reported"),
    ("cmpp", "FEAT_MTE not reported"),
    ("irg", "FEAT_MTE not reported"),
    ("gmi", "FEAT_MTE not reported"),
    ("smax", "FEAT_CSSC not reported"),
    ("smin", "FEAT_CSSC not reported"),
    ("umax", "FEAT_CSSC not reported"),
    ("umin", "FEAT_CSSC not reported"),
    ("abs", "FEAT_CSSC not reported"),
    ("cnt", "FEAT_CSSC not reported"),
    ("ctz", "FEAT_CSSC not reported"),
    ("rmif", "FEAT_FlagM: not in WP-DBT-02"),
    ("setf8", "FEAT_FlagM: not in WP-DBT-02"),
    ("setf16", "FEAT_FlagM: not in WP-DBT-02"),
    ("crc32b", "CRC32: not in WP-DBT-02"),
    ("crc32h", "CRC32: not in WP-DBT-02"),
    ("crc32w", "CRC32: not in WP-DBT-02"),
    ("crc32x", "CRC32: not in WP-DBT-02"),
    ("crc32cb", "CRC32: not in WP-DBT-02"),
    ("crc32ch", "CRC32: not in WP-DBT-02"),
    ("crc32cw", "CRC32: not in WP-DBT-02"),
    ("crc32cx", "CRC32: not in WP-DBT-02"),
    ("pacga", "PAC: WP-DBT-11"),
    ("pacia", "PAC: WP-DBT-11"),
    ("pacib", "PAC: WP-DBT-11"),
    ("pacda", "PAC: WP-DBT-11"),
    ("pacdb", "PAC: WP-DBT-11"),
    ("autia", "PAC: WP-DBT-11"),
    ("autib", "PAC: WP-DBT-11"),
    ("autda", "PAC: WP-DBT-11"),
    ("autdb", "PAC: WP-DBT-11"),
    ("autiasppc", "FEAT_PAuth_LR not reported"),
    ("autibsppc", "FEAT_PAuth_LR not reported"),
    ("maddpt", "FEAT_CPA not reported"),
    ("msubpt", "FEAT_CPA not reported"),
    (
        "smulh",
        "Ra != 31 (should-be-one field): UNDEFINED by choice",
    ),
    (
        "umulh",
        "Ra != 31 (should-be-one field): UNDEFINED by choice",
    ),
    ("ldr", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("ldrb", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("ldrh", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("ldrsb", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("ldrsh", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("ldrsw", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("str", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("strb", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("strh", "writeback with Rn == Rt: UNDEFINED by choice"),
    ("ldp", "Rt == Rt2 or writeback overlap: UNDEFINED by choice"),
    (
        "ldpsw",
        "Rt == Rt2 or writeback overlap: UNDEFINED by choice",
    ),
    ("ldnp", "Rt == Rt2: UNDEFINED by choice"),
    ("stp", "writeback overlap: UNDEFINED by choice"),
    ("mrs", "system register outside the WP-DBT-02 subset"),
    ("msr", "system register outside the WP-DBT-02 subset"),
    ("sys", "SYS: WP-DBT-07"),
    ("sysl", "SYSL: WP-DBT-07"),
    ("sysp", "FEAT_SYSINSTR128 not reported"),
    ("msrr", "FEAT_SYSREG128 not reported"),
    ("mrrs", "FEAT_SYSREG128 not reported"),
    ("tstart", "FEAT_TME not reported"),
    ("ttest", "FEAT_TME not reported"),
    ("braa", "PAC: WP-DBT-11"),
    ("brab", "PAC: WP-DBT-11"),
    ("blraa", "PAC: WP-DBT-11"),
    ("blrab", "PAC: WP-DBT-11"),
    ("braaz", "PAC: WP-DBT-11"),
    ("brabz", "PAC: WP-DBT-11"),
    ("blraaz", "PAC: WP-DBT-11"),
    ("blrabz", "PAC: WP-DBT-11"),
    ("retaa", "PAC: WP-DBT-11"),
    ("retab", "PAC: WP-DBT-11"),
    ("eretaa", "PAC: WP-DBT-11"),
    ("eretab", "PAC: WP-DBT-11"),
    ("eret", "WP-DBT-07"),
    ("drps", "UNDEFINED outside Debug state"),
    ("bc", "FEAT_HBC not reported"),
];

// ---------------------------------------------------------------------------------
// Printer (LLVM syntax)
// ---------------------------------------------------------------------------------

/// `xN`/`wN`/`sp`/`wsp`/`xzr`/`wzr`.
fn reg(r: GpReg, sf: bool) -> String {
    match (r, sf) {
        (GpReg::X(x), true) => format!("x{}", x.index()),
        (GpReg::X(x), false) => format!("w{}", x.index()),
        (GpReg::Sp, true) => "sp".into(),
        (GpReg::Sp, false) => "wsp".into(),
        (GpReg::Zr, true) => "xzr".into(),
        (GpReg::Zr, false) => "wzr".into(),
    }
}

fn field(r: GpReg) -> u32 {
    match r {
        GpReg::X(x) => u32::from(x.index()),
        GpReg::Sp | GpReg::Zr => 31,
    }
}

fn cond_name(c: Cond) -> &'static str {
    [
        "eq", "ne", "hs", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "al",
        "nv",
    ][c.bits() as usize]
}

fn invert(c: Cond) -> Cond {
    Cond::from_bits(c.bits() ^ 1)
}

fn shift_name(s: Shift) -> &'static str {
    match s {
        Shift::Lsl => "lsl",
        Shift::Lsr => "lsr",
        Shift::Asr => "asr",
        Shift::Ror => "ror",
    }
}

fn extend_name(e: Extend) -> &'static str {
    [
        "uxtb", "uxth", "uxtw", "uxtx", "sxtb", "sxth", "sxtw", "sxtx",
    ][e.bits() as usize]
}

fn target(addr: u64, offset: i64) -> String {
    format!("{:#x}", addr.wrapping_add_signed(offset))
}

fn hint_name(imm: u8) -> String {
    let named = match imm {
        0 => "nop",
        1 => "yield",
        2 => "wfe",
        3 => "wfi",
        4 => "sev",
        5 => "sevl",
        6 => "dgh",
        7 => "xpaclri",
        8 => "pacia1716",
        10 => "pacib1716",
        12 => "autia1716",
        14 => "autib1716",
        16 => "esb",
        17 => "psb csync",
        18 => "tsb csync",
        19 => "gcsb dsync",
        20 => "csdb",
        22 => "clrbhb",
        24 => "paciaz",
        25 => "paciasp",
        26 => "pacibz",
        27 => "pacibsp",
        28 => "autiaz",
        29 => "autiasp",
        30 => "autibz",
        31 => "autibsp",
        32 => "bti",
        34 => "bti c",
        36 => "bti j",
        38 => "bti jc",
        39 => "pacm",
        40 => "chkfeat x16",
        _ => return format!("hint #{imm}"),
    };
    named.into()
}

fn prfop(rt: GpReg) -> String {
    let v = field(rt);
    let (kind, tgt, policy) = (v >> 3, (v >> 1) & 3, v & 1);
    if kind == 3 {
        return format!("#{v}");
    }
    format!(
        "{}{}{}",
        ["pld", "pli", "pst"][kind as usize],
        ["l1", "l2", "l3", "slc"][tgt as usize],
        ["keep", "strm"][policy as usize]
    )
}

fn sysreg_name(r: SysReg) -> &'static str {
    match r {
        SysReg::Nzcv => "nzcv",
        SysReg::Fpcr => "fpcr",
        SysReg::Fpsr => "fpsr",
        SysReg::TpidrEl0 => "tpidr_el0",
    }
}

/// Arm `MoveWidePreferred(sf, immN, imms, immr)`.
fn move_wide_preferred(sf: bool, bitmask: u16) -> bool {
    let n = bitmask >> 12 & 1 == 1;
    let immr = i32::from(bitmask >> 6 & 63);
    let imms = i32::from(bitmask & 63);
    let width = if sf { 64 } else { 32 };
    if sf && !n {
        return false;
    }
    if !sf && (n || imms & 0x20 != 0) {
        return false;
    }
    if imms < 16 {
        return (-immr).rem_euclid(16) <= 15 - imms;
    }
    if imms >= width - 15 {
        return immr % 16 <= imms - (width - 15);
    }
    false
}

/// Arm `BFXPreferred(sf, uns, imms, immr)`.
fn bfx_preferred(sf: bool, uns: bool, imms: u8, immr: u8) -> bool {
    if imms < immr {
        return false;
    }
    if imms == if sf { 63 } else { 31 } {
        return false;
    }
    if immr == 0 {
        if !sf && (imms == 7 || imms == 15) {
            return false;
        }
        if sf && !uns && (imms == 7 || imms == 15 || imms == 31) {
            return false;
        }
    }
    true
}

fn bitfield_text(b: Bitfield) -> String {
    let sf = b.sf;
    let width: u8 = if sf { 64 } else { 32 };
    let (rd, rn) = (reg(b.rd, sf), reg(b.rn, sf));
    let (immr, imms) = (b.immr, b.imms);
    match b.op {
        BitfieldOp::Sbfm => {
            if imms == width - 1 {
                format!("asr {rd}, {rn}, #{immr}")
            } else if imms < immr {
                format!("sbfiz {rd}, {rn}, #{}, #{}", width - immr, imms + 1)
            } else if bfx_preferred(sf, false, imms, immr) {
                format!("sbfx {rd}, {rn}, #{immr}, #{}", imms - immr + 1)
            } else if immr == 0 && (imms == 7 || imms == 15 || imms == 31) {
                let name = match imms {
                    7 => "sxtb",
                    15 => "sxth",
                    _ => "sxtw",
                };
                format!("{name} {rd}, {}", reg(b.rn, false))
            } else {
                format!("sbfm {rd}, {rn}, #{immr}, #{imms}")
            }
        }
        BitfieldOp::Ubfm => {
            if imms != width - 1 && imms + 1 == immr {
                format!("lsl {rd}, {rn}, #{}", width - 1 - imms)
            } else if imms == width - 1 {
                format!("lsr {rd}, {rn}, #{immr}")
            } else if imms < immr {
                format!("ubfiz {rd}, {rn}, #{}, #{}", width - immr, imms + 1)
            } else if bfx_preferred(sf, true, imms, immr) {
                format!("ubfx {rd}, {rn}, #{immr}, #{}", imms - immr + 1)
            } else if immr == 0 && !sf && (imms == 7 || imms == 15) {
                let name = if imms == 7 { "uxtb" } else { "uxth" };
                format!("{name} {rd}, {rn}")
            } else {
                format!("ubfm {rd}, {rn}, #{immr}, #{imms}")
            }
        }
        BitfieldOp::Bfm => {
            // `BFC` exists as an alias (FEAT_ASMv8p2), but llvm-objdump prints `BFI`
            // with `xzr`, so the printer does too.
            if imms < immr {
                format!("bfi {rd}, {rn}, #{}, #{}", width - immr, imms + 1)
            } else {
                format!("bfxil {rd}, {rn}, #{immr}, #{}", imms - immr + 1)
            }
        }
    }
}

/// LLVM prints the `MOV` aliases' immediates as signed values of the register width.
fn signed(v: u64, sf: bool) -> i64 {
    if sf {
        v as i64
    } else {
        i64::from(v as u32 as i32)
    }
}

fn shifted(shift: Shift, amount: u8) -> String {
    if shift == Shift::Lsl && amount == 0 {
        String::new()
    } else {
        format!(", {} #{amount}", shift_name(shift))
    }
}

fn add_sub_reg_text(i: AddSubReg) -> String {
    let sf = i.sf;
    let sub = i.op == AddSubOp::Sub;
    let base = match (sub, i.set_flags) {
        (false, false) => "add",
        (false, true) => "adds",
        (true, false) => "sub",
        (true, true) => "subs",
    };
    match i.operand {
        RegOperand::Shifted { shift, amount } => {
            let rm = format!("{}{}", reg(i.rm, sf), shifted(shift, amount));
            if i.set_flags && i.rd == GpReg::Zr {
                let name = if sub { "cmp" } else { "cmn" };
                return format!("{name} {}, {rm}", reg(i.rn, sf));
            }
            if sub && i.rn == GpReg::Zr {
                let name = if i.set_flags { "negs" } else { "neg" };
                return format!("{name} {}, {rm}", reg(i.rd, sf));
            }
            format!("{base} {}, {}, {rm}", reg(i.rd, sf), reg(i.rn, sf))
        }
        RegOperand::Extended { extend, amount } => {
            let wide_rm = sf && matches!(extend, Extend::Uxtx | Extend::Sxtx);
            let uses_sp = i.rd == GpReg::Sp || i.rn == GpReg::Sp;
            let lsl_ext = if sf { Extend::Uxtx } else { Extend::Uxtw };
            let ext = if uses_sp && extend == lsl_ext {
                if amount == 0 {
                    String::new()
                } else {
                    format!(", lsl #{amount}")
                }
            } else if amount == 0 {
                format!(", {}", extend_name(extend))
            } else {
                format!(", {} #{amount}", extend_name(extend))
            };
            let rm = format!("{}{ext}", reg(i.rm, wide_rm));
            if i.set_flags && i.rd == GpReg::Zr {
                let name = if sub { "cmp" } else { "cmn" };
                return format!("{name} {}, {rm}", reg(i.rn, sf));
            }
            format!("{base} {}, {}, {rm}", reg(i.rd, sf), reg(i.rn, sf))
        }
    }
}

fn logical_reg_text(i: LogicalReg) -> String {
    let sf = i.sf;
    let rm = format!("{}{}", reg(i.rm, sf), shifted(i.shift, i.amount));
    let (rd, rn) = (reg(i.rd, sf), reg(i.rn, sf));
    match (i.op, i.invert) {
        (LogicOp::Orr, false) if i.rn == GpReg::Zr && i.shift == Shift::Lsl && i.amount == 0 => {
            format!("mov {rd}, {rm}")
        }
        (LogicOp::Orr, true) if i.rn == GpReg::Zr => format!("mvn {rd}, {rm}"),
        (LogicOp::Ands, false) if i.rd == GpReg::Zr => format!("tst {rn}, {rm}"),
        (op, inv) => {
            let name = match (op, inv) {
                (LogicOp::And, false) => "and",
                (LogicOp::And, true) => "bic",
                (LogicOp::Orr, false) => "orr",
                (LogicOp::Orr, true) => "orn",
                (LogicOp::Eor, false) => "eor",
                (LogicOp::Eor, true) => "eon",
                (LogicOp::Ands, false) => "ands",
                (LogicOp::Ands, true) => "bics",
            };
            format!("{name} {rd}, {rn}, {rm}")
        }
    }
}

fn mem_offset(base: &str, off: i64) -> String {
    if off == 0 {
        format!("[{base}]")
    } else {
        format!("[{base}, #{off}]")
    }
}

fn load_store_text(i: LoadStore) -> String {
    let base = reg(i.rn, true);
    let reg_wide = match i.op {
        LdStOp::LoadSigned64 => true,
        LdStOp::LoadSigned32 => false,
        _ => i.size == Ty::I64,
    };
    let rt = reg(i.rt, reg_wide);
    let suffix = match (i.op, i.size) {
        (LdStOp::LoadSigned32 | LdStOp::LoadSigned64, Ty::I8) => "sb",
        (LdStOp::LoadSigned32 | LdStOp::LoadSigned64, Ty::I16) => "sh",
        (LdStOp::LoadSigned64, _) => "sw",
        (_, Ty::I8) => "b",
        (_, Ty::I16) => "h",
        _ => "",
    };
    let (ld, st) = match i.mode {
        AddrMode::Unscaled(_) => ("ldur", "stur"),
        AddrMode::Unprivileged(_) => ("ldtr", "sttr"),
        _ => ("ldr", "str"),
    };
    let name = match i.op {
        LdStOp::Store => format!("{st}{suffix}"),
        LdStOp::Prefetch => {
            if matches!(i.mode, AddrMode::Unscaled(_)) {
                "prfum".into()
            } else {
                "prfm".into()
            }
        }
        _ => format!("{ld}{suffix}"),
    };
    let first = if i.op == LdStOp::Prefetch {
        prfop(i.rt)
    } else {
        rt
    };
    let addr = match i.mode {
        AddrMode::Unsigned(off) => mem_offset(&base, off as i64),
        AddrMode::Unscaled(off) | AddrMode::Unprivileged(off) => mem_offset(&base, off),
        AddrMode::PreIndex(off) => format!("[{base}, #{off}]!"),
        AddrMode::PostIndex(off) => format!("[{base}], #{off}"),
        AddrMode::Register { rm, extend, shift } => {
            let amount = i.size.bytes().trailing_zeros();
            let rm_wide = matches!(extend, Extend::Uxtx | Extend::Sxtx);
            let rm = reg(rm, rm_wide);
            let ext = match (extend, shift) {
                (Extend::Uxtx, false) => String::new(),
                (Extend::Uxtx, true) => format!(", lsl #{amount}"),
                (e, false) => format!(", {}", extend_name(e)),
                (e, true) => format!(", {} #{amount}", extend_name(e)),
            };
            format!("[{base}, {rm}{ext}]")
        }
    };
    format!("{name} {first}, {addr}")
}

fn pair_text(p: LoadStorePair) -> String {
    let name = match (p.op, p.mode) {
        (PairOp::Store, PairMode::NonTemporal) => "stnp",
        (PairOp::Load, PairMode::NonTemporal) => "ldnp",
        (PairOp::Store, _) => "stp",
        (PairOp::Load, _) => "ldp",
        (PairOp::LoadSw, _) => "ldpsw",
    };
    let base = reg(p.rn, true);
    let addr = match p.mode {
        PairMode::Offset | PairMode::NonTemporal => mem_offset(&base, p.offset),
        PairMode::PreIndex => format!("[{base}, #{}]!", p.offset),
        PairMode::PostIndex => format!("[{base}], #{}", p.offset),
    };
    format!("{name} {}, {}, {addr}", reg(p.rt, p.sf), reg(p.rt2, p.sf))
}

/// Prints `inst` (located at `addr`) as `llvm-objdump --mattr=+all` would.
#[allow(clippy::too_many_lines)]
fn print(inst: Inst, addr: u64) -> String {
    match inst {
        Inst::MoveWide {
            op,
            sf,
            shift,
            imm16,
            rd,
        } => {
            let rd = reg(rd, sf);
            let hw_zero_imm = imm16 == 0 && shift != 0;
            match op {
                MoveWideOp::Movz if !hw_zero_imm => {
                    format!("mov {rd}, #{}", signed(u64::from(imm16) << shift, sf))
                }
                MoveWideOp::Movn if !hw_zero_imm && !(!sf && imm16 == 0xffff) => {
                    format!("mov {rd}, #{}", signed(!(u64::from(imm16) << shift), sf))
                }
                _ => {
                    let name = match op {
                        MoveWideOp::Movn => "movn",
                        MoveWideOp::Movz => "movz",
                        MoveWideOp::Movk => "movk",
                    };
                    if shift == 0 {
                        format!("{name} {rd}, #{imm16}")
                    } else {
                        format!("{name} {rd}, #{imm16}, lsl #{shift}")
                    }
                }
            }
        }
        Inst::AddSubImm {
            op,
            sf,
            set_flags,
            imm12,
            shift12,
            rd,
            rn,
        } => {
            let imm = if shift12 {
                format!("#{imm12}, lsl #12")
            } else {
                format!("#{imm12}")
            };
            if set_flags && rd == GpReg::Zr {
                let name = if op == AddSubOp::Sub { "cmp" } else { "cmn" };
                return format!("{name} {}, {imm}", reg(rn, sf));
            }
            if op == AddSubOp::Add
                && !set_flags
                && imm12 == 0
                && !shift12
                && (rd == GpReg::Sp || rn == GpReg::Sp)
            {
                return format!("mov {}, {}", reg(rd, sf), reg(rn, sf));
            }
            let name = match (op, set_flags) {
                (AddSubOp::Add, false) => "add",
                (AddSubOp::Add, true) => "adds",
                (AddSubOp::Sub, false) => "sub",
                (AddSubOp::Sub, true) => "subs",
            };
            format!("{name} {}, {}, {imm}", reg(rd, sf), reg(rn, sf))
        }
        Inst::Adr { page, imm, rd } => {
            if page {
                format!(
                    "adrp {}, {:#x}",
                    reg(rd, true),
                    (addr & !0xfff).wrapping_add_signed(imm)
                )
            } else {
                format!("adr {}, {}", reg(rd, true), target(addr, imm))
            }
        }
        Inst::LogicalImm(i) => {
            let (rd, rn) = (reg(i.rd, i.sf), reg(i.rn, i.sf));
            match i.op {
                LogicOp::Orr if i.rn == GpReg::Zr && !move_wide_preferred(i.sf, i.bitmask) => {
                    format!("mov {rd}, #{}", signed(i.imm, i.sf))
                }
                LogicOp::Ands if i.rd == GpReg::Zr => format!("tst {rn}, #{}", i.imm),
                op => {
                    let name = ["and", "orr", "eor", "ands"][op as usize];
                    format!("{name} {rd}, {rn}, #{}", i.imm)
                }
            }
        }
        Inst::Bitfield(b) => bitfield_text(b),
        Inst::Extract(e) => {
            let (rd, rn, rm) = (reg(e.rd, e.sf), reg(e.rn, e.sf), reg(e.rm, e.sf));
            if e.rn == e.rm {
                format!("ror {rd}, {rn}, #{}", e.lsb)
            } else {
                format!("extr {rd}, {rn}, {rm}, #{}", e.lsb)
            }
        }
        Inst::LogicalReg(i) => logical_reg_text(i),
        Inst::AddSubReg(i) => add_sub_reg_text(i),
        Inst::AddSubCarry(i) => {
            let (rd, rn, rm) = (reg(i.rd, i.sf), reg(i.rn, i.sf), reg(i.rm, i.sf));
            match (i.op, i.set_flags) {
                (AddSubOp::Sub, s) if i.rn == GpReg::Zr => {
                    format!("{} {rd}, {rm}", if s { "ngcs" } else { "ngc" })
                }
                (op, s) => {
                    let name = match (op, s) {
                        (AddSubOp::Add, false) => "adc",
                        (AddSubOp::Add, true) => "adcs",
                        (AddSubOp::Sub, false) => "sbc",
                        (AddSubOp::Sub, true) => "sbcs",
                    };
                    format!("{name} {rd}, {rn}, {rm}")
                }
            }
        }
        Inst::CondCompare(c) => {
            let name = if c.op == AddSubOp::Sub {
                "ccmp"
            } else {
                "ccmn"
            };
            let rhs = match c.rhs {
                CondCmpRhs::Reg(r) => reg(r, c.sf),
                CondCmpRhs::Imm(v) => format!("#{v}"),
            };
            format!(
                "{name} {}, {rhs}, #{}, {}",
                reg(c.rn, c.sf),
                c.nzcv,
                cond_name(c.cond)
            )
        }
        Inst::CondSelect(c) => {
            let (rd, rn, rm) = (reg(c.rd, c.sf), reg(c.rn, c.sf), reg(c.rm, c.sf));
            let cc = cond_name(c.cond);
            let inv = cond_name(invert(c.cond));
            let not_al = !c.cond.is_always();
            match c.op {
                CondSelOp::Csinc if not_al && c.rn == GpReg::Zr && c.rm == GpReg::Zr => {
                    format!("cset {rd}, {inv}")
                }
                CondSelOp::Csinc if not_al && c.rn == c.rm && c.rn != GpReg::Zr => {
                    format!("cinc {rd}, {rn}, {inv}")
                }
                CondSelOp::Csinv if not_al && c.rn == GpReg::Zr && c.rm == GpReg::Zr => {
                    format!("csetm {rd}, {inv}")
                }
                CondSelOp::Csinv if not_al && c.rn == c.rm && c.rn != GpReg::Zr => {
                    format!("cinv {rd}, {rn}, {inv}")
                }
                CondSelOp::Csneg if not_al && c.rn == c.rm => format!("cneg {rd}, {rn}, {inv}"),
                op => {
                    let name = ["csel", "csinc", "csinv", "csneg"][op as usize];
                    format!("{name} {rd}, {rn}, {rm}, {cc}")
                }
            }
        }
        Inst::MulAdd(m) => {
            let long = !matches!(m.op, MulOp::Madd | MulOp::Msub);
            let (rd, ra) = (reg(m.rd, true), reg(m.ra, true));
            let (rn, rm) = match m.op {
                MulOp::Madd | MulOp::Msub => (reg(m.rn, m.sf), reg(m.rm, m.sf)),
                MulOp::Smulh | MulOp::Umulh => (reg(m.rn, true), reg(m.rm, true)),
                _ => (reg(m.rn, false), reg(m.rm, false)),
            };
            let rd = if long { rd } else { reg(m.rd, m.sf) };
            let ra = if long { ra } else { reg(m.ra, m.sf) };
            let (full, short) = match m.op {
                MulOp::Madd => ("madd", "mul"),
                MulOp::Msub => ("msub", "mneg"),
                MulOp::Smaddl => ("smaddl", "smull"),
                MulOp::Smsubl => ("smsubl", "smnegl"),
                MulOp::Umaddl => ("umaddl", "umull"),
                MulOp::Umsubl => ("umsubl", "umnegl"),
                MulOp::Smulh => return format!("smulh {rd}, {rn}, {rm}"),
                MulOp::Umulh => return format!("umulh {rd}, {rn}, {rm}"),
            };
            if m.ra == GpReg::Zr {
                format!("{short} {rd}, {rn}, {rm}")
            } else {
                format!("{full} {rd}, {rn}, {rm}, {ra}")
            }
        }
        Inst::DataProc2(d) => {
            let name = ["udiv", "sdiv", "lsl", "lsr", "asr", "ror"][d.op as usize];
            format!(
                "{name} {}, {}, {}",
                reg(d.rd, d.sf),
                reg(d.rn, d.sf),
                reg(d.rm, d.sf)
            )
        }
        Inst::DataProc1(d) => {
            let name = ["rbit", "rev16", "rev32", "rev", "clz", "cls"][d.op as usize];
            format!("{name} {}, {}", reg(d.rd, d.sf), reg(d.rn, d.sf))
        }
        Inst::B { offset } => format!("b {}", target(addr, offset)),
        Inst::Bl { offset } => format!("bl {}", target(addr, offset)),
        Inst::Br { rn } => format!("br {}", reg(rn, true)),
        Inst::Blr { rn } => format!("blr {}", reg(rn, true)),
        Inst::Ret { rn } => {
            if field(rn) == 30 {
                "ret".into()
            } else {
                format!("ret {}", reg(rn, true))
            }
        }
        Inst::CompareBranch {
            nonzero,
            sf,
            rt,
            offset,
        } => format!(
            "{} {}, {}",
            if nonzero { "cbnz" } else { "cbz" },
            reg(rt, sf),
            target(addr, offset)
        ),
        Inst::TestBranch {
            nonzero,
            bit,
            rt,
            offset,
        } => format!(
            "{} {}, #{bit}, {}",
            if nonzero { "tbnz" } else { "tbz" },
            reg(rt, bit >= 32),
            target(addr, offset)
        ),
        Inst::BCond { cond, offset } => format!("b.{} {}", cond_name(cond), target(addr, offset)),
        Inst::Svc { imm } => format!("svc #{imm}"),
        Inst::Brk { imm } => format!("brk #{imm}"),
        Inst::Nop => "nop".into(),
        Inst::Hint { imm } => hint_name(imm),
        Inst::Mrs { reg: r, rt } => format!("mrs {}, {}", reg(rt, true), sysreg_name(r)),
        Inst::Msr { reg: r, rt } => format!("msr {}, {}", sysreg_name(r), reg(rt, true)),
        Inst::LoadLiteral { op, rt, offset } => {
            let t = target(addr, offset);
            match op {
                LiteralOp::Ldr32 => format!("ldr {}, {t}", reg(rt, false)),
                LiteralOp::Ldr64 => format!("ldr {}, {t}", reg(rt, true)),
                LiteralOp::Ldrsw => format!("ldrsw {}, {t}", reg(rt, true)),
                LiteralOp::Prfm => format!("prfm {}, {t}", prfop(rt)),
            }
        }
        Inst::LoadStore(i) => load_store_text(i),
        Inst::LoadStorePair(p) => pair_text(p),
        Inst::Unknown(w) => format!("<unknown {w:#x}>"),
    }
}

// ---------------------------------------------------------------------------------
// Normalisation and the LLVM side
// ---------------------------------------------------------------------------------

/// Lower case, comment and `<symbol>` suffixes removed, whitespace collapsed, every
/// `#number` immediate rewritten in decimal (LLVM prints hex, the printer decimal; a
/// value is compared, not its spelling).
fn normalise(s: &str) -> String {
    let s = s.split("//").next().unwrap_or("").to_lowercase();
    let mut s = match s.find(" <") {
        Some(i) => s[..i].to_string(),
        None => s,
    };
    s = s.replace(['\t', ','], " ");
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        out.push(normalise_token(tok));
    }
    out.join(" ")
}

fn normalise_token(tok: &str) -> String {
    // Split off brackets and '!' around the token.
    let start = tok.find(|c: char| c != '[').unwrap_or(tok.len());
    let end = tok
        .rfind(|c: char| c != ']' && c != '!')
        .map_or(start, |i| i + 1);
    let (pre, core, post) = (
        &tok[..start],
        &tok[start..end.max(start)],
        &tok[end.max(start)..],
    );
    let parse = |t: &str| -> Option<i128> {
        let (neg, t) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t),
        };
        let v = match t.strip_prefix("0x") {
            Some(h) => i128::from_str_radix(h, 16).ok()?,
            None => t.parse::<i128>().ok()?,
        };
        Some(if neg { -v } else { v })
    };
    let core = if let Some(num) = core.strip_prefix('#') {
        parse(num).map_or_else(|| core.to_string(), |v| format!("#{v}"))
    } else if core.starts_with("0x") {
        // Branch/literal targets (no '#').
        parse(core).map_or_else(|| core.to_string(), |v| format!("{v}"))
    } else {
        core.to_string()
    };
    format!("{pre}{core}{post}")
}

fn tool(names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| {
        Command::new(n)
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| (*n).to_string())
    })
}

/// Disassembles `words` (placed at address 0, 4, 8, …) with LLVM; `None` = `<unknown>`.
fn llvm_disassemble(words: &[u32]) -> Option<Vec<Option<String>>> {
    let mc = tool(&["llvm-mc-18", "llvm-mc"])?;
    let objdump = tool(&["llvm-objdump-18", "llvm-objdump"])?;
    let dir = std::env::temp_dir().join(format!("ritsu-llvm-diff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let (src, obj) = (dir.join("words.s"), dir.join("words.o"));
    let mut text = String::with_capacity(words.len() * 18);
    for w in words {
        let _ = writeln!(text, ".inst {w:#010x}");
    }
    std::fs::write(&src, text).ok()?;
    let st = Command::new(&mc)
        .args(["-triple=aarch64", "-mattr=+all", "-filetype=obj", "-o"])
        .arg(&obj)
        .arg(&src)
        .status()
        .ok()?;
    assert!(st.success(), "llvm-mc failed");
    let out = Command::new(&objdump)
        .args(["-d", "--mattr=+all", "--no-show-raw-insn"])
        .arg(&obj)
        .output()
        .ok()?;
    assert!(out.status.success(), "llvm-objdump failed");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut result = vec![None; words.len()];
    let mut seen = vec![false; words.len()];
    for line in stdout.lines() {
        let Some((addr, rest)) = line.split_once(':') else {
            continue;
        };
        let Ok(addr) = usize::from_str_radix(addr.trim(), 16) else {
            continue;
        };
        if addr % 4 != 0 || addr / 4 >= words.len() {
            continue;
        }
        let rest = rest.trim();
        seen[addr / 4] = true;
        if !rest.starts_with("<unknown>") {
            result[addr / 4] = Some(rest.to_string());
        }
    }
    assert!(seen.iter().all(|&s| s), "objdump output incomplete");
    Some(result)
}

#[test]
fn decoder_matches_llvm_objdump() {
    const PER_CLASS: usize = 6_000;
    let mut rng = Rng(0x5eed_0f11_7a5e_d1ff);
    let mut words = Vec::new();
    let mut class_of = Vec::new();
    for (name, class) in CLASSES {
        for _ in 0..PER_CLASS {
            words.push(rng.word_in(class));
            class_of.push(name);
        }
    }
    let Some(llvm) = llvm_disassemble(&words) else {
        eprintln!("SKIPPED decoder_matches_llvm_objdump: llvm-mc/llvm-objdump not found");
        return;
    };
    let mut mismatches = Vec::new();
    let mut gaps: BTreeMap<String, usize> = BTreeMap::new();
    let mut compared = 0usize;
    for (i, (&w, theirs)) in words.iter().zip(&llvm).enumerate() {
        let addr = 4 * i as u64;
        let ours = decode(w);
        match (ours, theirs) {
            (Inst::Unknown(_), None) => {}
            (Inst::Unknown(_), Some(t)) => {
                // Gaps are only accounted for in the decoded classes; the catch-all
                // class reaches SIMD/SVE/SME, which belong to later work packages.
                if class_of[i] != "anything" {
                    let mnemonic = t.split_whitespace().next().unwrap_or("").to_lowercase();
                    let mnemonic = mnemonic.split('.').next().unwrap_or("").to_string();
                    if std::env::var_os("RITSU_LLVM_DIFF_VERBOSE").is_some() {
                        eprintln!("gap {w:#010x} [{}]: {t}", class_of[i]);
                    }
                    *gaps.entry(mnemonic).or_default() += 1;
                }
            }
            (inst, None) => mismatches.push(format!("{w:#010x}: LLVM <unknown>, ours {inst:?}")),
            (inst, Some(t)) => {
                compared += 1;
                let (a, b) = (normalise(&print(inst, addr)), normalise(t));
                // RPRFM (FEAT_RPRFM, not reported) lives in the PRFM hint space: without
                // the feature it is a prefetch hint, which is what Ritsu executes.
                let rprfm = b.starts_with("rprfm")
                    && matches!(
                        inst,
                        Inst::LoadStore(LoadStore {
                            op: LdStOp::Prefetch,
                            ..
                        })
                    );
                if a != b && !rprfm {
                    mismatches.push(format!("{w:#010x}: LLVM `{b}` ours `{a}` ({inst:?})"));
                }
            }
        }
    }
    let unexplained: Vec<_> = gaps
        .iter()
        .filter(|(m, _)| !GAPS.iter().any(|(g, _)| g == m))
        .collect();
    eprintln!(
        "llvm diff: {} words, {compared} decoded and compared, gaps {gaps:?}",
        words.len()
    );
    assert!(
        mismatches.is_empty(),
        "{} mismatches, first 40:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(unexplained.is_empty(), "unexplained gaps: {unexplained:?}");
    assert!(compared >= 100_000, "only {compared} words compared");
}
