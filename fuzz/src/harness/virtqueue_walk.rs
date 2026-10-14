//! Split virtqueue (`ritsu_virtio::queue::Queue`) against hostile guest memory.
//!
//! Input format (little-endian, reads past the end yield zeros; seeds: `fuzz/tools/gen_seeds.py`):
//!
//! ```text
//! u8   size selector: 0..=9 -> size 1<<n; 10..=14 -> raw u16 follows (may be invalid);
//!                     15 -> 32768
//! u8   layout: 0 = standard layout inside RAM, 1 = three u32 RAM offsets follow
//!              (desc, avail, used; may be misaligned), 2+ = three raw u64 addresses follow
//! ops  until the input ends (at most MAX_OPS):
//!        op % 7 = 0 Poke  { off: u16, len: u8, data[len] }  guest writes into RAM
//!                 1 Pop
//!                 2 PushUsed { head: u16, written: u32 }
//!                 3 ShouldNotify
//!                 4 Reset                                    re-create the queue (device reset)
//!                 5 Publish { head: u16 }                    well-behaved driver adds a chain
//!                 6 PokeAvailIdx { idx: u16 }                guest jumps avail.idx
//! ```
//!
//! Invariants checked against the virtio 1.2 layout (section 2.7), not against the
//! implementation: no panic; `Ok(None)` iff `avail.idx == consumed`; the returned `head` is the
//! one stored in `avail.ring[consumed % size]`; chains hold `1..=size` descriptors; the first
//! descriptor equals the raw table entry; every error breaks the queue and all further calls fail
//! with `Broken`; a successful `push_used` leaves `used.idx` and the entry as specified.

use crate::Reader;
use ritsu_core::GuestPhysAddr;
use ritsu_mem::{GuestMemory, RamRegion};
use ritsu_virtio::queue::{Queue, QueueConfig, QueueError};

const BASE: u64 = 0x1000_0000;
const RAM_LEN: usize = 0x1_0000;
const MAX_OPS: usize = 512;

fn align_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

fn rd16(ram: &RamRegion, addr: u64) -> Option<u16> {
    let mut b = [0u8; 2];
    ram.read(GuestPhysAddr(addr), &mut b).ok()?;
    Some(u16::from_le_bytes(b))
}

fn is_broken_for_good(q: &mut Queue, ram: &mut RamRegion) {
    assert!(q.is_broken());
    assert_eq!(q.pop(ram).unwrap_err(), QueueError::Broken);
    assert_eq!(q.push_used(ram, 0, 0).unwrap_err(), QueueError::Broken);
    assert_eq!(q.should_notify(ram).unwrap_err(), QueueError::Broken);
}

/// One fuzz iteration; panics on any violated invariant.
pub fn run(data: &[u8]) {
    let mut r = Reader::new(data);
    let sel = r.u8() & 0xf;
    let size: u16 = match sel {
        0..=9 => 1 << sel,
        10..=14 => r.u16(),
        _ => 32768,
    };
    let layout = r.u8();
    let n = u64::from(size);
    let cfg = match layout {
        0 => {
            let desc = BASE;
            let avail = align_up(desc + 16 * n, 2);
            let used = align_up(avail + 6 + 2 * n, 4);
            (desc, avail, used)
        }
        1 => (
            BASE + u64::from(r.u32() % RAM_LEN as u32),
            BASE + u64::from(r.u32() % RAM_LEN as u32),
            BASE + u64::from(r.u32() % RAM_LEN as u32),
        ),
        _ => (r.u64(), r.u64(), r.u64()),
    };
    let cfg = QueueConfig {
        size,
        desc_table: GuestPhysAddr(cfg.0),
        avail_ring: GuestPhysAddr(cfg.1),
        used_ring: GuestPhysAddr(cfg.2),
    };
    let mut ram = RamRegion::new(GuestPhysAddr(BASE), RAM_LEN);

    let valid_size = size != 0 && size.is_power_of_two();
    let new_queue = |cfg: QueueConfig| -> Option<Queue> {
        match Queue::new(cfg) {
            Ok(q) => {
                assert!(valid_size, "accepted invalid size {size}");
                assert_eq!(cfg.desc_table.get() % 16, 0);
                assert_eq!(cfg.avail_ring.get() % 2, 0);
                assert_eq!(cfg.used_ring.get() % 4, 0);
                Some(q)
            }
            Err(_) => None,
        }
    };
    let Some(mut q) = new_queue(cfg) else {
        return;
    };
    // Model state: entries consumed from avail and published to used since the last reset.
    let mut consumed: u16 = 0;
    let mut used: u16 = 0;

    for _ in 0..MAX_OPS {
        if r.exhausted() {
            break;
        }
        match r.u8() % 7 {
            0 => {
                let off = u64::from(r.u16());
                let len = usize::from(r.u8());
                let bytes = r.bytes(len);
                let _ = ram.write(GuestPhysAddr(BASE + off), bytes);
            }
            1 => {
                let was_broken = q.is_broken();
                let avail_idx = rd16(&ram, cfg.avail_ring.get().wrapping_add(2));
                let slot_head = rd16(
                    &ram,
                    cfg.avail_ring
                        .get()
                        .wrapping_add(4 + 2 * u64::from(consumed % size)),
                );
                match q.pop(&ram) {
                    Ok(None) => {
                        assert!(!was_broken);
                        assert_eq!(avail_idx, Some(consumed), "None although entries pending");
                        assert!(!q.is_broken());
                    }
                    Ok(Some(chain)) => {
                        assert!(!was_broken);
                        assert_ne!(avail_idx, Some(consumed), "chain although none pending");
                        assert_eq!(Some(chain.head), slot_head, "head differs from avail ring");
                        assert!(chain.head < size);
                        let n = chain.descriptors.len();
                        assert!((1..=usize::from(size)).contains(&n));
                        let sum: u64 = chain.descriptors.iter().map(|d| u64::from(d.len)).sum();
                        assert_eq!(sum, chain.readable_len() + chain.writable_len());
                        let first_writable = chain.descriptors.iter().position(|d| d.write);
                        if let Some(p) = first_writable {
                            assert!(chain.descriptors[p..].iter().all(|d| d.write));
                        }
                        // First descriptor against the raw table entry.
                        let at = cfg.desc_table.get() + 16 * u64::from(chain.head);
                        let mut raw = [0u8; 16];
                        ram.read(GuestPhysAddr(at), &mut raw)
                            .expect("descriptor was readable");
                        let d0 = chain.descriptors[0];
                        assert_eq!(d0.addr.get().to_le_bytes(), raw[..8]);
                        assert_eq!(d0.len.to_le_bytes(), raw[8..12]);
                        assert_eq!(d0.write, raw[12] & 2 != 0);
                        assert!(!q.is_broken());
                        consumed = consumed.wrapping_add(1);
                    }
                    Err(e) => {
                        assert!(q.is_broken());
                        if was_broken {
                            assert_eq!(e, QueueError::Broken);
                        }
                        is_broken_for_good(&mut q, &mut ram);
                    }
                }
            }
            2 => {
                let head = r.u16();
                let written = r.u32();
                let was_broken = q.is_broken();
                match q.push_used(&mut ram, head, written) {
                    Ok(()) => {
                        assert!(!was_broken && head < size);
                        used = used.wrapping_add(1);
                        let base = cfg.used_ring.get();
                        assert_eq!(rd16(&ram, base + 2), Some(used), "used.idx");
                        let entry = base + 4 + 8 * u64::from(used.wrapping_sub(1) % size);
                        let mut e = [0u8; 8];
                        ram.read(GuestPhysAddr(entry), &mut e)
                            .expect("entry in RAM");
                        assert_eq!(e[..4], u32::from(head).to_le_bytes());
                        assert_eq!(e[4..], written.to_le_bytes());
                    }
                    Err(_) => {
                        assert!(q.is_broken());
                        is_broken_for_good(&mut q, &mut ram);
                    }
                }
            }
            3 => {
                let was_broken = q.is_broken();
                match q.should_notify(&ram) {
                    Ok(_) => assert!(!was_broken),
                    Err(_) => {
                        assert!(q.is_broken());
                        is_broken_for_good(&mut q, &mut ram);
                    }
                }
            }
            4 => {
                let Some(fresh) = new_queue(cfg) else {
                    return;
                };
                q = fresh;
                consumed = 0;
                used = 0;
            }
            5 => {
                let head = r.u16();
                let avail = cfg.avail_ring.get();
                if let Some(idx) = rd16(&ram, avail.wrapping_add(2)) {
                    let slot = avail.wrapping_add(4 + 2 * u64::from(idx % size));
                    if ram.write(GuestPhysAddr(slot), &head.to_le_bytes()).is_ok() {
                        let _ = ram.write(
                            GuestPhysAddr(avail.wrapping_add(2)),
                            &idx.wrapping_add(1).to_le_bytes(),
                        );
                    }
                }
            }
            _ => {
                let idx = r.u16();
                let _ = ram.write(
                    GuestPhysAddr(cfg.avail_ring.get().wrapping_add(2)),
                    &idx.to_le_bytes(),
                );
            }
        }
    }
}
