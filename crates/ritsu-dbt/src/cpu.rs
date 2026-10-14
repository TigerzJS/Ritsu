//! Architectural state of one guest vCPU (AArch64, EL0 + EL1).
//!
//! [`CpuState`] is `#[repr(C)]` because translated code (T1/T2) will address its fields
//! through a pinned base register at fixed offsets; the field order is therefore part of
//! the JIT ABI once WP-DBT-12 lands. Every type embedded in it is `#[repr(C)]`,
//! `#[repr(transparent)]` or `#[repr(u8)]` so that the offsets are defined by the
//! language rather than by the compiler's layout choice; `tests::jit_abi_offsets` pins
//! them. See `docs/subsystems/dbt.md` §6.

use core::fmt;

/// Exception level the vCPU executes at. The guest has no EL2/EL3 (vmapple platform,
/// ADR-0004), so only EL0 and EL1 exist.
///
/// `#[repr(u8)]` with the architectural `PSTATE.EL` value as discriminant, because it is
/// part of [`CpuState`]'s JIT ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ExceptionLevel {
    /// User mode.
    El0 = 0,
    /// Kernel mode (XNU).
    El1 = 1,
}

/// A general-purpose register number `0..=30` (`X0`–`X30`).
///
/// Register number 31 is not representable: depending on the instruction it means
/// `SP` or `XZR`, which [`GpReg`] makes explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct XReg(u8);

impl XReg {
    /// The link register `X30`.
    pub const LR: Self = Self(30);

    /// Returns `Some` for `n <= 30`.
    #[must_use]
    pub const fn new(n: u8) -> Option<Self> {
        if n <= 30 { Some(Self(n)) } else { None }
    }

    /// Register number `0..=30`.
    #[must_use]
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// A general-purpose register operand with the meaning of encoding 31 resolved.
///
/// The decoder decides per operand whether 31 is the stack pointer or the zero register
/// (Arm ARM: `X[n]` vs `SP[]`), so executors never have to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GpReg {
    /// `X0`–`X30` / `W0`–`W30`.
    X(XReg),
    /// The current stack pointer (`SP`/`WSP`), selected by EL and `PSTATE.SP`.
    Sp,
    /// The zero register (`XZR`/`WZR`): reads as 0, writes are discarded.
    Zr,
}

impl GpReg {
    /// Resolves a 5-bit register field where 31 means `XZR`.
    #[must_use]
    pub const fn zr_from_field(field: u32) -> Self {
        // `field & 0x1f` always fits in a u8.
        match XReg::new((field & 0x1f) as u8) {
            Some(x) => Self::X(x),
            None => Self::Zr,
        }
    }

    /// Resolves a 5-bit register field where 31 means `SP`.
    #[must_use]
    pub const fn sp_from_field(field: u32) -> Self {
        match XReg::new((field & 0x1f) as u8) {
            Some(x) => Self::X(x),
            None => Self::Sp,
        }
    }

    /// Shorthand for `GpReg::X` of register `n`; `None` for `n > 30`.
    #[must_use]
    pub const fn x(n: u8) -> Option<Self> {
        match XReg::new(n) {
            Some(x) => Some(Self::X(x)),
            None => None,
        }
    }
}

/// The condition flags `PSTATE.{N,Z,C,V}`.
///
/// Stored in the layout of the `NZCV` system register (N = bit 31, Z = 30, C = 29,
/// V = 28, all other bits zero), so `MRS/MSR NZCV` are plain moves.
///
/// Note for the x86 backends: `C` has Arm semantics. After a subtraction `C == 1`
/// means *no borrow*, the inverse of x86 `CF`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct Nzcv(u32);

impl Nzcv {
    const N: u32 = 1 << 31;
    const Z: u32 = 1 << 30;
    const C: u32 = 1 << 29;
    const V: u32 = 1 << 28;
    const MASK: u32 = Self::N | Self::Z | Self::C | Self::V;

    /// Builds flags from individual bits.
    #[must_use]
    pub const fn new(n: bool, z: bool, c: bool, v: bool) -> Self {
        let mut bits = 0;
        if n {
            bits |= Self::N;
        }
        if z {
            bits |= Self::Z;
        }
        if c {
            bits |= Self::C;
        }
        if v {
            bits |= Self::V;
        }
        Self(bits)
    }

    /// From the `NZCV` system register value; RES0 bits are ignored.
    #[must_use]
    pub const fn from_sysreg(value: u64) -> Self {
        // Only bits 31:28 survive the mask, so the truncation is lossless.
        Self(value as u32 & Self::MASK)
    }

    /// As the `NZCV` system register value.
    #[must_use]
    pub const fn to_sysreg(self) -> u64 {
        self.0 as u64
    }

    /// From a 4-bit `nzcv` immediate as used by `CCMP`/`CCMN` (bit 3 = N … bit 0 = V).
    #[must_use]
    pub const fn from_nibble(nibble: u8) -> Self {
        Self(((nibble as u32) & 0xf) << 28)
    }

    /// Negative flag.
    #[must_use]
    pub const fn n(self) -> bool {
        self.0 & Self::N != 0
    }

    /// Zero flag.
    #[must_use]
    pub const fn z(self) -> bool {
        self.0 & Self::Z != 0
    }

    /// Carry flag (Arm semantics: set = no borrow on subtraction).
    #[must_use]
    pub const fn c(self) -> bool {
        self.0 & Self::C != 0
    }

    /// Overflow flag.
    #[must_use]
    pub const fn v(self) -> bool {
        self.0 & Self::V != 0
    }
}

impl fmt::Debug for Nzcv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ch = |set: bool, c: char| if set { c } else { '-' };
        write!(
            f,
            "Nzcv({}{}{}{})",
            ch(self.n(), 'N'),
            ch(self.z(), 'Z'),
            ch(self.c(), 'C'),
            ch(self.v(), 'V')
        )
    }
}

/// One 128-bit pointer-authentication key (`AP*KeyHi_EL1:AP*KeyLo_EL1`).
///
/// `Debug` is redacted: keys are guest secrets and must not end up in host logs.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct PacKey {
    /// `AP*KeyLo_EL1`.
    pub lo: u64,
    /// `AP*KeyHi_EL1`.
    pub hi: u64,
}

impl fmt::Debug for PacKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PacKey(<redacted>)")
    }
}

/// The five arm64e pointer-authentication keys. Placeholder storage: the PAC algorithm
/// (implementation-defined, reported via `ID_AA64ISAR1_EL1.API/GPI`) lands in WP-DBT-11.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(C)]
pub struct PacKeys {
    /// Instruction key A (`APIAKey`).
    pub ia: PacKey,
    /// Instruction key B (`APIBKey`).
    pub ib: PacKey,
    /// Data key A (`APDAKey`).
    pub da: PacKey,
    /// Data key B (`APDBKey`).
    pub db: PacKey,
    /// Generic key (`APGAKey`, used by `PACGA`).
    pub ga: PacKey,
}

/// EL1/EL0 system registers needed early. Placeholder: the final register file is
/// generated from a table (encoding, access rules, reset value) in WP-DBT-07.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(C)]
pub struct SysRegs {
    /// `SCTLR_EL1`: MMU enable, alignment checks, PAC enables, `SPAN`, `UCI`, …
    pub sctlr_el1: u64,
    /// `TCR_EL1`: `T0SZ/T1SZ`, granule sizes, `AS` (16-bit ASIDs), `TBI0/1`, `IPS`.
    pub tcr_el1: u64,
    /// `TTBR0_EL1`: lower-half table base + ASID.
    pub ttbr0_el1: u64,
    /// `TTBR1_EL1`: upper-half (kernel) table base.
    pub ttbr1_el1: u64,
    /// `MAIR_EL1`: memory attribute encodings.
    pub mair_el1: u64,
    /// `VBAR_EL1`: exception vector base.
    pub vbar_el1: u64,
    /// `ELR_EL1`: exception return address.
    pub elr_el1: u64,
    /// `SPSR_EL1`: saved `PSTATE` on exception entry.
    pub spsr_el1: u64,
    /// `ESR_EL1`: exception syndrome.
    pub esr_el1: u64,
    /// `FAR_EL1`: faulting virtual address.
    pub far_el1: u64,
    /// `TPIDR_EL0`: EL0 read/write thread pointer.
    pub tpidr_el0: u64,
    /// `TPIDRRO_EL0`: EL0 read-only thread pointer.
    pub tpidrro_el0: u64,
    /// `TPIDR_EL1`: EL1 thread pointer.
    pub tpidr_el1: u64,
    /// `FPCR`: rounding mode, `FZ`, `DN`, `AH`, …
    pub fpcr: u64,
    /// `FPSR`: cumulative FP exception flags.
    pub fpsr: u64,
}

/// Architectural state of one vCPU.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct CpuState {
    /// `X0`–`X30`. `X31` does not exist as storage (it is `SP` or `XZR`).
    pub x: [u64; 31],
    /// `SP_EL0`.
    pub sp_el0: u64,
    /// `SP_EL1`.
    pub sp_el1: u64,
    /// Program counter of the next instruction to execute.
    pub pc: u64,
    /// `PSTATE.{N,Z,C,V}`.
    pub nzcv: Nzcv,
    /// `PSTATE.EL`.
    pub el: ExceptionLevel,
    /// `PSTATE.SP`: at EL1, `true` selects `SP_EL1`, `false` selects `SP_EL0`.
    /// Ignored at EL0 (always `SP_EL0`).
    pub sp_sel: bool,
    /// `PSTATE.{D,A,I,F}` as a 4-bit value (bit 3 = D … bit 0 = F).
    pub daif: u8,
    /// System registers (placeholder subset).
    pub sys: SysRegs,
    /// Pointer-authentication keys.
    pub pac: PacKeys,
}

impl CpuState {
    /// Architectural reset state for a guest without EL2/EL3: EL1, `SP_EL1` selected,
    /// all of `DAIF` masked, registers zero, `PC = reset_pc`.
    #[must_use]
    pub fn new(reset_pc: u64) -> Self {
        Self {
            x: [0; 31],
            sp_el0: 0,
            sp_el1: 0,
            pc: reset_pc,
            nzcv: Nzcv::default(),
            el: ExceptionLevel::El1,
            sp_sel: true,
            daif: 0b1111,
            sys: SysRegs::default(),
            pac: PacKeys::default(),
        }
    }

    /// The currently selected stack pointer value.
    #[must_use]
    pub fn sp(&self) -> u64 {
        if self.uses_sp_el1() {
            self.sp_el1
        } else {
            self.sp_el0
        }
    }

    /// Writes the currently selected stack pointer.
    pub fn set_sp(&mut self, value: u64) {
        if self.uses_sp_el1() {
            self.sp_el1 = value;
        } else {
            self.sp_el0 = value;
        }
    }

    fn uses_sp_el1(&self) -> bool {
        self.el == ExceptionLevel::El1 && self.sp_sel
    }

    /// Reads a 64-bit register (`XZR` reads as 0).
    #[must_use]
    pub fn read(&self, r: GpReg) -> u64 {
        match r {
            GpReg::X(x) => self.x.get(usize::from(x.index())).copied().unwrap_or(0),
            GpReg::Sp => self.sp(),
            GpReg::Zr => 0,
        }
    }

    /// Writes a 64-bit register (writes to `XZR` are discarded).
    pub fn write(&mut self, r: GpReg, value: u64) {
        match r {
            GpReg::X(x) => {
                if let Some(slot) = self.x.get_mut(usize::from(x.index())) {
                    *slot = value;
                }
            }
            GpReg::Sp => self.set_sp(value),
            GpReg::Zr => {}
        }
    }

    /// Reads a register at operand width: the low 32 bits if `!sf` (`Wn`), else 64.
    #[must_use]
    pub fn read_sized(&self, r: GpReg, sf: bool) -> u64 {
        let v = self.read(r);
        if sf { v } else { v & 0xffff_ffff }
    }

    /// Writes a register at operand width. A 32-bit write zero-extends to 64 bits, as
    /// every `Wd` write does in AArch64 (including `WSP`).
    pub fn write_sized(&mut self, r: GpReg, sf: bool, value: u64) {
        self.write(r, if sf { value } else { value & 0xffff_ffff });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg31_resolution() {
        assert_eq!(GpReg::zr_from_field(31), GpReg::Zr);
        assert_eq!(GpReg::sp_from_field(31), GpReg::Sp);
        assert_eq!(GpReg::zr_from_field(30), GpReg::X(XReg::LR));
        assert_eq!(GpReg::sp_from_field(0), GpReg::x(0).unwrap());
        assert_eq!(XReg::new(31), None);
    }

    #[test]
    fn zr_reads_zero_and_discards_writes() {
        let mut cpu = CpuState::new(0);
        cpu.write(GpReg::Zr, 42);
        assert_eq!(cpu.read(GpReg::Zr), 0);
        assert_eq!(cpu.x, [0; 31]);
        assert_eq!(cpu.sp(), 0);
    }

    #[test]
    fn sp_selection_follows_el_and_spsel() {
        let mut cpu = CpuState::new(0);
        cpu.sp_el0 = 0x100;
        cpu.sp_el1 = 0x200;
        assert_eq!(cpu.read(GpReg::Sp), 0x200);
        cpu.sp_sel = false;
        assert_eq!(cpu.read(GpReg::Sp), 0x100);
        cpu.sp_sel = true;
        cpu.el = ExceptionLevel::El0;
        assert_eq!(cpu.read(GpReg::Sp), 0x100);
        cpu.write(GpReg::Sp, 0x300);
        assert_eq!(cpu.sp_el0, 0x300);
        assert_eq!(cpu.sp_el1, 0x200);
    }

    #[test]
    fn sized_write_zero_extends() {
        let mut cpu = CpuState::new(0);
        let x3 = GpReg::x(3).unwrap();
        cpu.write(x3, u64::MAX);
        cpu.write_sized(x3, false, 0xdead_beef_1234_5678);
        assert_eq!(cpu.read(x3), 0x1234_5678);
        assert_eq!(cpu.read_sized(GpReg::Zr, false), 0);
    }

    #[test]
    fn nzcv_layout_matches_sysreg() {
        let f = Nzcv::new(true, false, true, false);
        assert_eq!(f.to_sysreg(), 0xa000_0000);
        assert_eq!(
            Nzcv::from_sysreg(0xffff_ffff_ffff_ffff),
            Nzcv::new(true, true, true, true)
        );
        assert_eq!(
            Nzcv::from_nibble(0b0110),
            Nzcv::new(false, true, true, false)
        );
        assert_eq!(format!("{f:?}"), "Nzcv(N-C-)");
    }

    #[test]
    fn pac_keys_are_redacted_in_debug() {
        let mut cpu = CpuState::new(0);
        cpu.pac.ia = PacKey {
            lo: 0x1122_3344_5566_7788,
            hi: 0x99,
        };
        let s = format!("{cpu:?}");
        assert!(!s.contains("1122334455667788"));
        assert!(!s.contains(&0x1122_3344_5566_7788u64.to_string()));
        assert!(s.contains("redacted"));
    }

    /// Pins the layout translated code will address relative to the state pointer
    /// (`docs/subsystems/dbt.md` §6.1). A change here is a JIT-ABI change: update the
    /// expected values deliberately, together with every emitter that uses them.
    #[test]
    fn jit_abi_offsets() {
        use core::mem::{align_of, offset_of, size_of};

        assert_eq!(offset_of!(CpuState, x), 0);
        assert_eq!(size_of::<[u64; 31]>(), 248);
        assert_eq!(offset_of!(CpuState, sp_el0), 248);
        assert_eq!(offset_of!(CpuState, sp_el1), 256);
        assert_eq!(offset_of!(CpuState, pc), 264);
        assert_eq!(offset_of!(CpuState, nzcv), 272);
        assert_eq!(offset_of!(CpuState, el), 276);
        assert_eq!(offset_of!(CpuState, sp_sel), 277);
        assert_eq!(offset_of!(CpuState, daif), 278);
        assert_eq!(offset_of!(CpuState, sys), 280);
        assert_eq!(offset_of!(CpuState, pac), 280 + 15 * 8);
        assert_eq!(size_of::<CpuState>(), 400 + 5 * 16);
        assert_eq!(align_of::<CpuState>(), 8);

        // Field types whose representation the JIT relies on.
        assert_eq!(size_of::<Nzcv>(), 4);
        assert_eq!(size_of::<ExceptionLevel>(), 1);
        assert_eq!(ExceptionLevel::El0 as u8, 0);
        assert_eq!(ExceptionLevel::El1 as u8, 1);
        assert_eq!(size_of::<bool>(), 1);

        // System registers T1 reads/writes inline (MRS/MSR TPIDR*, FPCR/FPSR, NZCV path).
        assert_eq!(offset_of!(SysRegs, sctlr_el1), 0);
        assert_eq!(offset_of!(SysRegs, tpidr_el0), 10 * 8);
        assert_eq!(offset_of!(SysRegs, tpidrro_el0), 11 * 8);
        assert_eq!(offset_of!(SysRegs, tpidr_el1), 12 * 8);
        assert_eq!(offset_of!(SysRegs, fpcr), 13 * 8);
        assert_eq!(offset_of!(SysRegs, fpsr), 14 * 8);
        assert_eq!(size_of::<SysRegs>(), 15 * 8);

        // PAC keys (inline PAC sequences in T1/T2, WP-DBT-11).
        assert_eq!(offset_of!(PacKey, lo), 0);
        assert_eq!(offset_of!(PacKey, hi), 8);
        assert_eq!(offset_of!(PacKeys, ia), 0);
        assert_eq!(offset_of!(PacKeys, ib), 16);
        assert_eq!(offset_of!(PacKeys, da), 32);
        assert_eq!(offset_of!(PacKeys, db), 48);
        assert_eq!(offset_of!(PacKeys, ga), 64);
    }

    #[test]
    fn reset_state() {
        let cpu = CpuState::new(0x8000);
        assert_eq!(cpu.pc, 0x8000);
        assert_eq!(cpu.el, ExceptionLevel::El1);
        assert!(cpu.sp_sel);
        assert_eq!(cpu.daif, 0b1111);
    }
}
