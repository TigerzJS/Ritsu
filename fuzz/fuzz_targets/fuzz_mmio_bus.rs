//! `ritsu_platform::mmio::MmioBus`: random registrations and accesses.
//!
//! Input format (little-endian, reads past the end yield zeros; seeds: `fuzz/tools/gen_seeds.py`):
//!
//! ```text
//! ops until the input ends (at most MAX_OPS):
//!   op % 3 = 0 Register { device: u8 (even = PL011, odd = probe), base: Addr, len: Len }
//!            1 Read     { addr: Addr, size: u8 }
//!            2 Write    { addr: Addr, size: u8, value: u64 }
//! Addr: u8 sel; 0 -> raw u64 follows; n -> anchor[(n-1) % anchors.len()] + i16 delta
//!       (anchors: 0, u64::MAX, and base and end of every registration attempt)
//! Len:  u8 sel; 0 -> 0, 1 -> 0x1000 (PL011 size), 2 -> u64::MAX, 3 -> u16, else u64
//! size: 1 << (size % 4) bytes
//! ```
//!
//! Invariants, checked against an independent model of the bus contract: registration fails
//! exactly for zero length, `base + len` overflow and overlap; accesses fail with `Unmapped`,
//! `Misaligned` or `CrossesRegionEnd` exactly as the model predicts; the probe device asserts what
//! the `MmioDevice` documentation guarantees (offset inside the region and naturally aligned);
//! results are masked to the access width.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ritsu_fuzz::Reader;
use ritsu_platform::mmio::{AccessSize, BusError, MmioBus, MmioDevice, MmioError};
use ritsu_platform::pl011::{PL011_REGION_LEN, Pl011};
use std::collections::BTreeMap;

const MAX_OPS: usize = 1024;
const MAX_DEVICES: usize = 64;

/// Device that asserts the bus guarantees from the `MmioDevice` documentation.
struct Probe {
    len: u64,
    regs: BTreeMap<u64, u64>,
}

impl MmioDevice for Probe {
    fn name(&self) -> &str {
        "probe"
    }

    fn read(&mut self, offset: u64, size: AccessSize) -> Result<u64, MmioError> {
        assert!(
            offset
                .checked_add(size.bytes())
                .is_some_and(|e| e <= self.len),
            "bus passed offset {offset:#x}+{} outside region of {:#x}",
            size.bytes(),
            self.len
        );
        assert_eq!(
            offset % size.bytes(),
            0,
            "bus passed misaligned offset {offset:#x}"
        );
        // Deliberately not masked: the bus must mask.
        Ok(self.regs.get(&offset).copied().unwrap_or(0) ^ 0xdead_beef_cafe_f00d)
    }

    fn write(&mut self, offset: u64, size: AccessSize, value: u64) -> Result<(), MmioError> {
        assert!(
            offset
                .checked_add(size.bytes())
                .is_some_and(|e| e <= self.len),
            "bus passed offset {offset:#x}+{} outside region of {:#x}",
            size.bytes(),
            self.len
        );
        assert_eq!(
            offset % size.bytes(),
            0,
            "bus passed misaligned offset {offset:#x}"
        );
        assert_eq!(value & !size.mask(), 0, "bus passed unmasked write value");
        if self.regs.len() < 256 {
            self.regs.insert(offset, value);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Model {
    base: u64,
    len: u64,
}

fn addr(r: &mut Reader<'_>, anchors: &[u64]) -> u64 {
    match r.u8() {
        0 => r.u64(),
        n => {
            let a = anchors[usize::from(n - 1) % anchors.len()];
            a.wrapping_add_signed(i64::from(r.u16() as i16))
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut r = Reader::new(data);
    let mut bus = MmioBus::new();
    let mut model: Vec<Model> = Vec::new();
    let mut anchors: Vec<u64> = vec![0, u64::MAX];

    for _ in 0..MAX_OPS {
        if r.exhausted() {
            break;
        }
        match r.u8() % 3 {
            0 => {
                let kind = r.u8();
                let base = addr(&mut r, &anchors);
                let len = match r.u8() {
                    0 => 0,
                    1 => PL011_REGION_LEN,
                    2 => u64::MAX,
                    3 => u64::from(r.u16()),
                    _ => r.u64(),
                };
                if model.len() >= MAX_DEVICES {
                    continue;
                }
                anchors.push(base);
                anchors.push(base.wrapping_add(len));
                let dev: Box<dyn MmioDevice> = if kind.is_multiple_of(2) {
                    Box::new(Pl011::new(Box::new(|_b: u8| {})))
                } else {
                    Box::new(Probe {
                        len,
                        regs: BTreeMap::new(),
                    })
                };
                let res = bus.register(base, len, dev);
                let expected = if len == 0 {
                    Some("zero")
                } else if base.checked_add(len).is_none() {
                    Some("overflow")
                } else if !base.is_multiple_of(8) {
                    Some("misaligned_base")
                } else if model
                    .iter()
                    .any(|m| base < m.base + m.len && m.base < base + len)
                {
                    Some("overlap")
                } else {
                    None
                };
                match (expected, &res) {
                    (None, Ok(())) => model.push(Model { base, len }),
                    (Some("zero"), Err(BusError::ZeroLength { .. }))
                    | (Some("overflow"), Err(BusError::Overflow { .. }))
                    | (Some("misaligned_base"), Err(BusError::MisalignedBase { .. }))
                    | (Some("overlap"), Err(BusError::Overlap { .. })) => {}
                    _ => panic!(
                        "register({base:#x}, {len:#x}) = {res:?}, model expected {expected:?}"
                    ),
                }
                assert_eq!(bus.len(), model.len());
            }
            op => {
                let a = addr(&mut r, &anchors);
                let size = AccessSize::from_bytes(1 << (r.u8() % 4)).expect("1,2,4,8");
                let bytes = size.bytes();
                let value = if op == 2 { r.u64() } else { 0 };
                let hit = model.iter().find(|m| a >= m.base && a - m.base < m.len);
                let expected: Option<MmioError> = match hit {
                    None => Some(MmioError::Unmapped {
                        addr: a,
                        size: bytes,
                    }),
                    Some(_) if !a.is_multiple_of(bytes) => Some(MmioError::Misaligned {
                        addr: a,
                        size: bytes,
                    }),
                    Some(m) if (a - m.base).checked_add(bytes).is_none_or(|e| e > m.len) => {
                        Some(MmioError::CrossesRegionEnd {
                            addr: a,
                            size: bytes,
                        })
                    }
                    Some(_) => None,
                };
                if op == 1 {
                    match (bus.read(a, size), expected) {
                        (Ok(v), None) => assert_eq!(v & !size.mask(), 0, "read not masked"),
                        (Err(e), Some(x)) => assert_eq!(e, x),
                        // Device-level errors are legitimate for accesses the bus accepted.
                        (Err(MmioError::BadOffset { .. } | MmioError::BadSize { .. }), None) => {}
                        (got, want) => panic!("read({a:#x}, {bytes}) = {got:?}, model {want:?}"),
                    }
                } else {
                    match (bus.write(a, size, value), expected) {
                        (Ok(()), None) => {}
                        (Err(e), Some(x)) => assert_eq!(e, x),
                        (Err(MmioError::BadOffset { .. } | MmioError::BadSize { .. }), None) => {}
                        (got, want) => panic!("write({a:#x}, {bytes}) = {got:?}, model {want:?}"),
                    }
                }
            }
        }
    }
});
