//! Virtqueue workloads (Q-PERF-2, virtio dominates boot I/O): the device side of a split
//! virtqueue, `Queue::pop` followed by `Queue::push_used`, over a queue of 256 descriptors.
//!
//! The guest side is set up once: descriptor chains, and an available ring whose every slot
//! names a valid chain head. A round then only advances `avail.idx` (one 2-byte write) and the
//! device pops and completes every pending chain. One work unit is one pop plus one push.

use ritsu_core::GuestPhysAddr;
use ritsu_mem::{GuestMemory, RamRegion};
use ritsu_virtio::queue::{Queue, QueueConfig, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};

const BASE: u64 = 0x1000_0000;
const DESC: u64 = BASE;
const AVAIL: u64 = BASE + 0x1000;
const USED: u64 = BASE + 0x2000;
const BUFFERS: u64 = BASE + 0x3000;
const RAM_LEN: usize = 0x8000;
/// Queue size.
pub const QUEUE_SIZE: u16 = 256;
const BUFFER_LEN: u32 = 64;

/// A queue with a prepared guest side.
#[derive(Debug)]
pub struct Fixture {
    ram: RamRegion,
    queue: Queue,
    chains: u16,
    published: u16,
    completed: u64,
}

impl Fixture {
    /// Queue of [`QUEUE_SIZE`] descriptors in chains of `desc_per_chain` (1 or 3) descriptors:
    /// 1 = a single device-readable buffer, 3 = header (R), data (R), status (W), like a virtio-blk
    /// request. `QUEUE_SIZE / desc_per_chain` chains exist.
    #[must_use]
    pub fn new(desc_per_chain: u16) -> Self {
        assert!(desc_per_chain == 1 || desc_per_chain == 3);
        let mut ram = RamRegion::new(GuestPhysAddr(BASE), RAM_LEN);
        let chains = QUEUE_SIZE / desc_per_chain;
        for c in 0..chains {
            for j in 0..desc_per_chain {
                let index = c * desc_per_chain + j;
                let last = j + 1 == desc_per_chain;
                let mut flags = if last { 0 } else { VIRTQ_DESC_F_NEXT };
                if last && desc_per_chain == 3 {
                    flags |= VIRTQ_DESC_F_WRITE;
                }
                let next = if last { 0 } else { index + 1 };
                let mut d = [0u8; 16];
                d[..8].copy_from_slice(
                    &(BUFFERS + u64::from(index) * u64::from(BUFFER_LEN)).to_le_bytes(),
                );
                d[8..12].copy_from_slice(&BUFFER_LEN.to_le_bytes());
                d[12..14].copy_from_slice(&flags.to_le_bytes());
                d[14..16].copy_from_slice(&next.to_le_bytes());
                ram.write(GuestPhysAddr(DESC + 16 * u64::from(index)), &d)
                    .expect("descriptor in RAM");
            }
        }
        for slot in 0..QUEUE_SIZE {
            let head = (slot % chains) * desc_per_chain;
            ram.write(
                GuestPhysAddr(AVAIL + 4 + 2 * u64::from(slot)),
                &head.to_le_bytes(),
            )
            .expect("avail ring in RAM");
        }
        let queue = Queue::new(QueueConfig {
            size: QUEUE_SIZE,
            desc_table: GuestPhysAddr(DESC),
            avail_ring: GuestPhysAddr(AVAIL),
            used_ring: GuestPhysAddr(USED),
        })
        .expect("valid queue");
        Self {
            ram,
            queue,
            chains,
            published: 0,
            completed: 0,
        }
    }

    /// Chains made available per round.
    #[must_use]
    pub fn chains(&self) -> u16 {
        self.chains
    }

    /// Total pop + push pairs done so far.
    #[must_use]
    pub fn completed(&self) -> u64 {
        self.completed
    }

    /// Guest publishes every chain once, then the device pops and completes all of them.
    /// Returns the number of chains handled.
    pub fn round(&mut self) -> u64 {
        self.published = self.published.wrapping_add(self.chains);
        self.ram
            .write(GuestPhysAddr(AVAIL + 2), &self.published.to_le_bytes())
            .expect("avail.idx in RAM");
        let mut n = 0u64;
        while let Some(chain) = self.queue.pop(&self.ram).expect("valid queue state") {
            let written = u32::try_from(std::hint::black_box(chain.writable_len())).unwrap_or(0);
            self.queue
                .push_used(&mut self.ram, chain.head, written)
                .expect("used ring in RAM");
            n += 1;
        }
        self.completed += n;
        n
    }

    /// Reads `used.idx` from guest memory.
    #[must_use]
    pub fn used_idx(&self) -> u16 {
        let mut b = [0u8; 2];
        self.ram
            .read(GuestPhysAddr(USED + 2), &mut b)
            .expect("used.idx in RAM");
        u16::from_le_bytes(b)
    }

    /// Reads used-ring entry `slot` as `(id, len)`.
    #[must_use]
    pub fn used_entry(&self, slot: u16) -> (u32, u32) {
        let mut b = [0u8; 8];
        self.ram
            .read(
                GuestPhysAddr(USED + 4 + 8 * u64::from(slot % QUEUE_SIZE)),
                &mut b,
            )
            .expect("used entry in RAM");
        (
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_descriptor_chains_complete_in_order() {
        let mut f = Fixture::new(1);
        assert_eq!(f.chains(), 256);
        assert_eq!(f.round(), 256);
        assert_eq!(f.used_idx(), 256);
        for slot in 0..256u16 {
            // Chain `slot` has head `slot` and no writable buffer (len 0 written).
            assert_eq!(f.used_entry(slot), (u32::from(slot), 0), "slot {slot}");
        }
    }

    #[test]
    fn three_descriptor_chains_report_the_writable_length() {
        let mut f = Fixture::new(3);
        assert_eq!(f.chains(), 85);
        assert_eq!(f.round(), 85);
        assert_eq!(f.used_idx(), 85);
        for slot in 0..85u16 {
            // Head of chain c is 3c; the third descriptor is the 64-byte device-writable status.
            assert_eq!(f.used_entry(slot), (3 * u32::from(slot), 64), "slot {slot}");
        }
    }

    #[test]
    fn rounds_wrap_the_ring_and_index_correctly() {
        let mut f = Fixture::new(3);
        // 85 chains per round: after 4 rounds the 16-bit index has not wrapped but the ring has.
        for _ in 0..4 {
            assert_eq!(f.round(), 85);
        }
        assert_eq!(f.used_idx(), 340);
        assert_eq!(f.completed(), 340);
        // Slot 340 % 256 = 84 holds the entry of completion 340 - 1 = 339: the head of the
        // chain in avail slot 339 % 256 = 83, i.e. chain 83 % 85 = 83, head 249.
        assert_eq!(f.used_entry(83), (3 * 83, 64));
        // 4 + 800 rounds = 804 * 85 = 68340 completions = 65536 + 2804: the u16 index wrapped.
        for _ in 0..800 {
            f.round();
        }
        assert_eq!(f.used_idx(), 2804);
    }
}
