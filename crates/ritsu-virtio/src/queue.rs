//! Split virtqueue: descriptor table, available ring, used ring.
//!
//! Layout and semantics follow the OASIS *Virtual I/O Device (VIRTIO) Specification
//! Version 1.2*, §2.7 "Split Virtqueues":
//!
//! | Area | Size | Alignment |
//! |---|---|---|
//! | Descriptor table | `16 * queue_size` | 16 |
//! | Available ring (`flags`, `idx`, `ring[]`, `used_event`) | `6 + 2 * queue_size` | 2 |
//! | Used ring (`flags`, `idx`, `ring[]`, `avail_event`) | `6 + 8 * queue_size` | 4 |
//!
//! A descriptor is `{ addr: le64, len: le32, flags: le16, next: le16 }` (§2.7.5). Flags are
//! `NEXT = 1`, `WRITE = 2`, `INDIRECT = 4` (§2.7.5, §2.7.7).
//!
//! # Threat model
//!
//! The guest owns the rings. Every index, address and length read from them is validated:
//!
//! * the queue size must be a power of two in `1..=32768` (§2.7),
//! * ring addresses must be aligned as above,
//! * an available head or `next` index `>= queue_size` is an error,
//! * a chain with more descriptors than the queue has entries must contain a loop and is an
//!   error (every descriptor can appear in a chain at most once),
//! * `addr + len` of a buffer must not overflow,
//! * device-readable descriptors must precede device-writable ones (§2.7.4.2),
//! * the guest advertising more than `queue_size` new entries is an error.
//!
//! After the first protocol violation the [`Queue`] is *broken* (the device must set
//! `DEVICE_NEEDS_RESET`, §2.1.2) and every further call fails with [`QueueError::Broken`]
//! until the queue is replaced. This prevents a hostile guest from driving the device into
//! an endless retry loop.
//!
//! # Not implemented
//!
//! `VIRTIO_F_EVENT_IDX`, packed rings and indirect descriptors (the latter are reported as
//! [`QueueError::Unsupported`]).
//!
//! # Memory ordering
//!
//! [`GuestMemory`] has no ordering guarantees. The production memory backend must make
//! `pop` observe ring contents written before the guest's `avail.idx` update (acquire) and
//! publish used entries before the `used.idx` update (release). That is a requirement on the
//! backend, not on this module.

use ritsu_core::GuestPhysAddr;
use ritsu_mem::{GuestMemory, MemError};

/// Largest queue size allowed by the specification.
pub const MAX_QUEUE_SIZE: u16 = 32768;

/// Descriptor flag: the chain continues in `next`.
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
/// Descriptor flag: the buffer is device-writable (otherwise device-readable).
pub const VIRTQ_DESC_F_WRITE: u16 = 2;
/// Descriptor flag: the buffer contains an indirect descriptor table.
pub const VIRTQ_DESC_F_INDIRECT: u16 = 4;
/// Available-ring flag: the driver does not want an interrupt after the device uses buffers.
pub const VIRTQ_AVAIL_F_NO_INTERRUPT: u16 = 1;

const DESC_SIZE: u64 = 16;

/// Errors from queue setup and processing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum QueueError {
    /// The queue size is zero, not a power of two or larger than [`MAX_QUEUE_SIZE`].
    #[error("invalid queue size {0}")]
    InvalidSize(u32),
    /// A ring address is not aligned as the specification requires.
    #[error("{what} address {addr:#x} is not {align}-byte aligned")]
    Misaligned {
        /// Which structure.
        what: &'static str,
        /// The offending address.
        addr: u64,
        /// Required alignment in bytes.
        align: u64,
    },
    /// Ring address arithmetic overflowed the 64-bit address space.
    #[error("address overflow while locating {0}")]
    AddressOverflow(&'static str),
    /// Guest memory access failed (ring or descriptor outside RAM).
    #[error("queue memory access failed: {0}")]
    Memory(#[from] MemError),
    /// A head or `next` index is not smaller than the queue size.
    #[error("descriptor index {index} out of range (queue size {size})")]
    IndexOutOfRange {
        /// The offending index.
        index: u16,
        /// The queue size.
        size: u16,
    },
    /// The chain has more descriptors than the queue has entries, i.e. it loops.
    #[error("descriptor chain longer than queue size {size} (loop)")]
    ChainLoop {
        /// The queue size.
        size: u16,
    },
    /// A device-readable descriptor follows a device-writable one.
    #[error("device-readable descriptor after device-writable descriptor")]
    ReadableAfterWritable,
    /// A descriptor buffer `addr + len` overflows the address space.
    #[error("descriptor buffer {addr:#x}+{len:#x} overflows")]
    BufferOverflow {
        /// Buffer address.
        addr: u64,
        /// Buffer length.
        len: u32,
    },
    /// The guest made more entries available than the queue can hold.
    #[error("available index jumped by {pending} entries (queue size {size})")]
    AvailOverrun {
        /// Number of entries the guest claims are pending.
        pending: u16,
        /// The queue size.
        size: u16,
    },
    /// A feature that Ritsu does not implement yet.
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    /// The queue saw a protocol violation earlier and must be reset.
    #[error("queue is broken after an earlier protocol violation")]
    Broken,
}

impl From<QueueError> for ritsu_core::Error {
    fn from(e: QueueError) -> Self {
        match e {
            QueueError::Unsupported(what) => ritsu_core::Error::Unimplemented(what),
            other => ritsu_core::Error::Guest(other.to_string()),
        }
    }
}

/// Guest-programmed location and size of a virtqueue (the `QueueSize`, `QueueDesc`,
/// `QueueDriver`, `QueueDevice` registers of the virtio transport).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueConfig {
    /// Number of descriptors; a power of two in `1..=32768`.
    pub size: u16,
    /// Guest physical address of the descriptor table.
    pub desc_table: GuestPhysAddr,
    /// Guest physical address of the available ring ("driver area").
    pub avail_ring: GuestPhysAddr,
    /// Guest physical address of the used ring ("device area").
    pub used_ring: GuestPhysAddr,
}

/// One descriptor of a chain, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Descriptor {
    /// Guest physical address of the buffer.
    pub addr: GuestPhysAddr,
    /// Buffer length in bytes.
    pub len: u32,
    /// `true` if the device may write to the buffer, `false` if it may only read it.
    pub write: bool,
}

/// A validated descriptor chain taken from the available ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorChain {
    /// Index of the head descriptor; pass it back to [`Queue::push_used`].
    pub head: u16,
    /// Descriptors in chain order. Device-readable ones come first.
    pub descriptors: Vec<Descriptor>,
}

impl DescriptorChain {
    /// Total length of device-readable buffers.
    #[must_use]
    pub fn readable_len(&self) -> u64 {
        self.descriptors
            .iter()
            .filter(|d| !d.write)
            .map(|d| u64::from(d.len))
            .sum()
    }

    /// Total length of device-writable buffers.
    #[must_use]
    pub fn writable_len(&self) -> u64 {
        self.descriptors
            .iter()
            .filter(|d| d.write)
            .map(|d| u64::from(d.len))
            .sum()
    }
}

/// Device-side state of one split virtqueue.
#[derive(Debug)]
pub struct Queue {
    cfg: QueueConfig,
    next_avail: u16,
    next_used: u16,
    broken: bool,
}

fn offset(base: GuestPhysAddr, off: u64, what: &'static str) -> Result<GuestPhysAddr, QueueError> {
    base.checked_add(off)
        .ok_or(QueueError::AddressOverflow(what))
}

fn check_align(what: &'static str, addr: GuestPhysAddr, align: u64) -> Result<(), QueueError> {
    if addr.get().is_multiple_of(align) {
        Ok(())
    } else {
        Err(QueueError::Misaligned {
            what,
            addr: addr.get(),
            align,
        })
    }
}

impl Queue {
    /// Creates a queue from guest-programmed registers.
    ///
    /// Only size and alignment are validated here; whether the rings lie in RAM is checked on
    /// each access (the guest can reprogram RAM mappings, and a failed access breaks the queue).
    ///
    /// # Errors
    ///
    /// [`QueueError::InvalidSize`], [`QueueError::Misaligned`] or
    /// [`QueueError::AddressOverflow`].
    pub fn new(cfg: QueueConfig) -> Result<Self, QueueError> {
        let size = cfg.size;
        if size == 0 || !size.is_power_of_two() || size > MAX_QUEUE_SIZE {
            return Err(QueueError::InvalidSize(u32::from(size)));
        }
        check_align("descriptor table", cfg.desc_table, 16)?;
        check_align("available ring", cfg.avail_ring, 2)?;
        check_align("used ring", cfg.used_ring, 4)?;
        let n = u64::from(size);
        offset(cfg.desc_table, DESC_SIZE * n, "descriptor table")?;
        offset(cfg.avail_ring, 6 + 2 * n, "available ring")?;
        offset(cfg.used_ring, 6 + 8 * n, "used ring")?;
        Ok(Self {
            cfg,
            next_avail: 0,
            next_used: 0,
            broken: false,
        })
    }

    /// The configuration this queue was created with.
    #[must_use]
    pub fn config(&self) -> &QueueConfig {
        &self.cfg
    }

    /// Whether a protocol violation was detected and the queue needs a reset.
    #[must_use]
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// Runs `f`; any error marks the queue broken.
    fn guarded<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, QueueError>,
    ) -> Result<T, QueueError> {
        if self.broken {
            return Err(QueueError::Broken);
        }
        let r = f(self);
        if r.is_err() {
            self.broken = true;
        }
        r
    }

    /// Takes the next available descriptor chain, or `None` if the guest has none pending.
    ///
    /// # Errors
    ///
    /// Any [`QueueError`]; the queue is broken afterwards (see module docs).
    pub fn pop<M: GuestMemory + ?Sized>(
        &mut self,
        mem: &M,
    ) -> Result<Option<DescriptorChain>, QueueError> {
        self.guarded(|q| q.pop_inner(mem))
    }

    fn pop_inner<M: GuestMemory + ?Sized>(
        &mut self,
        mem: &M,
    ) -> Result<Option<DescriptorChain>, QueueError> {
        let size = self.cfg.size;
        // avail.idx at offset 2
        let idx_addr = offset(self.cfg.avail_ring, 2, "available ring")?;
        let mut b = [0u8; 2];
        mem.read(idx_addr, &mut b)?;
        let avail_idx = u16::from_le_bytes(b);
        let pending = avail_idx.wrapping_sub(self.next_avail);
        if pending == 0 {
            return Ok(None);
        }
        if pending > size {
            return Err(QueueError::AvailOverrun { pending, size });
        }
        // avail.ring[next_avail % size] at offset 4 + 2 * slot
        let slot = u64::from(self.next_avail % size);
        let head_addr = offset(self.cfg.avail_ring, 4 + 2 * slot, "available ring entry")?;
        mem.read(head_addr, &mut b)?;
        let head = u16::from_le_bytes(b);
        let chain = self.walk(mem, head)?;
        self.next_avail = self.next_avail.wrapping_add(1);
        Ok(Some(chain))
    }

    /// Reads the chain starting at `head` with full validation.
    fn walk<M: GuestMemory + ?Sized>(
        &self,
        mem: &M,
        head: u16,
    ) -> Result<DescriptorChain, QueueError> {
        let size = self.cfg.size;
        let mut descriptors = Vec::new();
        let mut index = head;
        let mut seen_write = false;
        loop {
            if index >= size {
                return Err(QueueError::IndexOutOfRange { index, size });
            }
            if descriptors.len() >= usize::from(size) {
                return Err(QueueError::ChainLoop { size });
            }
            let at = offset(
                self.cfg.desc_table,
                DESC_SIZE * u64::from(index),
                "descriptor",
            )?;
            let mut raw = [0u8; DESC_SIZE as usize];
            mem.read(at, &mut raw)?;
            let addr = u64::from_le_bytes([
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
            ]);
            let len = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
            let flags = u16::from_le_bytes([raw[12], raw[13]]);
            let next = u16::from_le_bytes([raw[14], raw[15]]);

            if flags & VIRTQ_DESC_F_INDIRECT != 0 {
                return Err(QueueError::Unsupported("indirect descriptors"));
            }
            if addr.checked_add(u64::from(len)).is_none() {
                return Err(QueueError::BufferOverflow { addr, len });
            }
            let write = flags & VIRTQ_DESC_F_WRITE != 0;
            if write {
                seen_write = true;
            } else if seen_write {
                return Err(QueueError::ReadableAfterWritable);
            }
            descriptors.push(Descriptor {
                addr: GuestPhysAddr(addr),
                len,
                write,
            });

            if flags & VIRTQ_DESC_F_NEXT == 0 {
                return Ok(DescriptorChain { head, descriptors });
            }
            index = next;
        }
    }

    /// Returns a processed chain to the guest by appending `{ id: head, len: written }` to the
    /// used ring and advancing `used.idx`.
    ///
    /// `written` is the number of bytes the device wrote into the chain's writable buffers
    /// (§2.7.8.2).
    ///
    /// # Errors
    ///
    /// Any [`QueueError`]; the queue is broken afterwards.
    pub fn push_used<M: GuestMemory + ?Sized>(
        &mut self,
        mem: &mut M,
        head: u16,
        written: u32,
    ) -> Result<(), QueueError> {
        self.guarded(|q| {
            let size = q.cfg.size;
            if head >= size {
                return Err(QueueError::IndexOutOfRange { index: head, size });
            }
            let slot = u64::from(q.next_used % size);
            let entry = offset(q.cfg.used_ring, 4 + 8 * slot, "used ring entry")?;
            let mut raw = [0u8; 8];
            raw[..4].copy_from_slice(&u32::from(head).to_le_bytes());
            raw[4..].copy_from_slice(&written.to_le_bytes());
            mem.write(entry, &raw)?;
            let new_idx = q.next_used.wrapping_add(1);
            // Publish the entry only after it is fully written.
            let idx_addr = offset(q.cfg.used_ring, 2, "used ring")?;
            mem.write(idx_addr, &new_idx.to_le_bytes())?;
            q.next_used = new_idx;
            Ok(())
        })
    }

    /// Whether the guest wants an interrupt for newly used buffers (§2.7.7: the driver sets
    /// `VIRTQ_AVAIL_F_NO_INTERRUPT` to suppress them). Without `EVENT_IDX` this is the only
    /// suppression mechanism.
    ///
    /// # Errors
    ///
    /// Any [`QueueError`]; the queue is broken afterwards.
    pub fn should_notify<M: GuestMemory + ?Sized>(&mut self, mem: &M) -> Result<bool, QueueError> {
        self.guarded(|q| {
            let mut b = [0u8; 2];
            mem.read(q.cfg.avail_ring, &mut b)?;
            Ok(u16::from_le_bytes(b) & VIRTQ_AVAIL_F_NO_INTERRUPT == 0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ritsu_mem::RamRegion;

    const BASE: u64 = 0x1000_0000;
    const DESC: u64 = BASE;
    const AVAIL: u64 = BASE + 0x1000;
    const USED: u64 = BASE + 0x2000;
    const BUF: u64 = BASE + 0x3000;

    fn cfg(size: u16) -> QueueConfig {
        QueueConfig {
            size,
            desc_table: GuestPhysAddr(DESC),
            avail_ring: GuestPhysAddr(AVAIL),
            used_ring: GuestPhysAddr(USED),
        }
    }

    fn ram() -> RamRegion {
        RamRegion::new(GuestPhysAddr(BASE), 0x1_0000)
    }

    fn set_desc(ram: &mut RamRegion, index: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let at = GuestPhysAddr(DESC + 16 * u64::from(index));
        ram.write_u64(at, addr).unwrap();
        ram.write_u32(GuestPhysAddr(at.get() + 8), len).unwrap();
        ram.write(GuestPhysAddr(at.get() + 12), &flags.to_le_bytes())
            .unwrap();
        ram.write(GuestPhysAddr(at.get() + 14), &next.to_le_bytes())
            .unwrap();
    }

    /// Makes `heads` available (writes ring entries and bumps idx).
    fn publish(ram: &mut RamRegion, first_slot: u16, heads: &[u16], size: u16, new_idx: u16) {
        for (i, h) in heads.iter().enumerate() {
            let slot = (u64::from(first_slot) + i as u64) % u64::from(size);
            ram.write(GuestPhysAddr(AVAIL + 4 + 2 * slot), &h.to_le_bytes())
                .unwrap();
        }
        ram.write(GuestPhysAddr(AVAIL + 2), &new_idx.to_le_bytes())
            .unwrap();
    }

    #[test]
    fn rejects_bad_sizes_and_alignment() {
        for s in [0u16, 3, 6, 100, 0x8001, 0xffff] {
            assert!(
                matches!(Queue::new(cfg(s)), Err(QueueError::InvalidSize(_))),
                "{s}"
            );
        }
        for s in [1u16, 2, 256, 32768] {
            assert!(Queue::new(cfg(s)).is_ok(), "{s}");
        }
        let mut c = cfg(8);
        c.desc_table = GuestPhysAddr(DESC + 8);
        assert!(matches!(
            Queue::new(c),
            Err(QueueError::Misaligned { align: 16, .. })
        ));
        let mut c = cfg(8);
        c.avail_ring = GuestPhysAddr(AVAIL + 1);
        assert!(matches!(
            Queue::new(c),
            Err(QueueError::Misaligned { align: 2, .. })
        ));
        let mut c = cfg(8);
        c.used_ring = GuestPhysAddr(USED + 2);
        assert!(matches!(
            Queue::new(c),
            Err(QueueError::Misaligned { align: 4, .. })
        ));
    }

    #[test]
    fn rejects_ring_address_overflow() {
        let mut c = cfg(8);
        c.used_ring = GuestPhysAddr(u64::MAX - 3);
        assert!(matches!(Queue::new(c), Err(QueueError::AddressOverflow(_))));
    }

    #[test]
    fn empty_queue_yields_none() {
        let ram = ram();
        let mut q = Queue::new(cfg(8)).unwrap();
        assert_eq!(q.pop(&ram).unwrap(), None);
        assert!(!q.is_broken());
    }

    #[test]
    fn single_and_chained_descriptors() {
        let mut ram = ram();
        // chain 2 -> 5 -> 1: read(0x10), read(0x20), write(0x30)
        set_desc(&mut ram, 2, BUF, 0x10, VIRTQ_DESC_F_NEXT, 5);
        set_desc(&mut ram, 5, BUF + 0x100, 0x20, VIRTQ_DESC_F_NEXT, 1);
        set_desc(&mut ram, 1, BUF + 0x200, 0x30, VIRTQ_DESC_F_WRITE, 0);
        // a second, single-descriptor chain
        set_desc(&mut ram, 7, BUF + 0x300, 4, 0, 0);
        publish(&mut ram, 0, &[2, 7], 8, 2);

        let mut q = Queue::new(cfg(8)).unwrap();
        let c = q.pop(&ram).unwrap().unwrap();
        assert_eq!(c.head, 2);
        assert_eq!(c.descriptors.len(), 3);
        assert!(!c.descriptors[0].write && !c.descriptors[1].write && c.descriptors[2].write);
        assert_eq!(c.descriptors[1].addr, GuestPhysAddr(BUF + 0x100));
        assert_eq!(c.readable_len(), 0x30);
        assert_eq!(c.writable_len(), 0x30);

        let c = q.pop(&ram).unwrap().unwrap();
        assert_eq!((c.head, c.descriptors.len()), (7, 1));
        assert_eq!(q.pop(&ram).unwrap(), None);
    }

    #[test]
    fn chain_may_use_every_descriptor_once() {
        let mut ram = ram();
        // 0 -> 1 -> 2 -> 3, queue size 4: exactly size descriptors is legal.
        for i in 0..3 {
            set_desc(&mut ram, i, BUF, 1, VIRTQ_DESC_F_NEXT, i + 1);
        }
        set_desc(&mut ram, 3, BUF, 1, 0, 0);
        publish(&mut ram, 0, &[0], 4, 1);
        let mut q = Queue::new(cfg(4)).unwrap();
        assert_eq!(q.pop(&ram).unwrap().unwrap().descriptors.len(), 4);
    }

    #[test]
    fn detects_two_descriptor_loop_and_self_loop() {
        let mut ram = ram();
        set_desc(&mut ram, 0, BUF, 1, VIRTQ_DESC_F_NEXT, 1);
        set_desc(&mut ram, 1, BUF, 1, VIRTQ_DESC_F_NEXT, 0);
        publish(&mut ram, 0, &[0], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert_eq!(q.pop(&ram), Err(QueueError::ChainLoop { size: 8 }));
        assert!(q.is_broken());

        let mut ram = self::ram();
        set_desc(&mut ram, 3, BUF, 1, VIRTQ_DESC_F_NEXT, 3);
        publish(&mut ram, 0, &[3], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert!(matches!(q.pop(&ram), Err(QueueError::ChainLoop { .. })));
    }

    #[test]
    fn loop_in_largest_queue_terminates() {
        // All-zero descriptor table except 0 -> 0 self-loop; must stop after 32768 steps.
        let mut ram = RamRegion::new(GuestPhysAddr(BASE), 0x10_0000);
        let size = MAX_QUEUE_SIZE;
        let c = QueueConfig {
            size,
            desc_table: GuestPhysAddr(BASE),
            avail_ring: GuestPhysAddr(BASE + 0x8_0000),
            used_ring: GuestPhysAddr(BASE + 0x9_0000),
        };
        ram.write(GuestPhysAddr(BASE + 12), &VIRTQ_DESC_F_NEXT.to_le_bytes())
            .unwrap();
        ram.write(GuestPhysAddr(BASE + 0x8_0000 + 2), &1u16.to_le_bytes())
            .unwrap();
        let mut q = Queue::new(c).unwrap();
        assert!(matches!(q.pop(&ram), Err(QueueError::ChainLoop { .. })));
    }

    #[test]
    fn out_of_range_indices() {
        // head out of range
        let mut ram = ram();
        publish(&mut ram, 0, &[8], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert_eq!(
            q.pop(&ram),
            Err(QueueError::IndexOutOfRange { index: 8, size: 8 })
        );

        // next out of range
        let mut ram = self::ram();
        set_desc(&mut ram, 0, BUF, 1, VIRTQ_DESC_F_NEXT, 0xffff);
        publish(&mut ram, 0, &[0], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert_eq!(
            q.pop(&ram),
            Err(QueueError::IndexOutOfRange {
                index: 0xffff,
                size: 8
            })
        );
    }

    #[test]
    fn avail_idx_jump_is_rejected() {
        let mut ram = ram();
        publish(&mut ram, 0, &[], 8, 9); // 9 pending > size 8
        let mut q = Queue::new(cfg(8)).unwrap();
        assert_eq!(
            q.pop(&ram),
            Err(QueueError::AvailOverrun {
                pending: 9,
                size: 8
            })
        );
        // wrapped "negative" distance (idx behind next_avail) is also an overrun
        let mut ram = self::ram();
        set_desc(&mut ram, 0, BUF, 1, 0, 0);
        publish(&mut ram, 0, &[0], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        q.pop(&ram).unwrap().unwrap();
        ram.write(GuestPhysAddr(AVAIL + 2), &0u16.to_le_bytes())
            .unwrap();
        assert!(matches!(q.pop(&ram), Err(QueueError::AvailOverrun { .. })));
    }

    #[test]
    fn indirect_is_unsupported() {
        let mut ram = ram();
        set_desc(&mut ram, 0, BUF, 16, VIRTQ_DESC_F_INDIRECT, 0);
        publish(&mut ram, 0, &[0], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert_eq!(
            q.pop(&ram),
            Err(QueueError::Unsupported("indirect descriptors"))
        );
        let e: ritsu_core::Error = QueueError::Unsupported("indirect descriptors").into();
        assert!(matches!(e, ritsu_core::Error::Unimplemented(_)));
    }

    #[test]
    fn readable_after_writable_is_rejected() {
        let mut ram = ram();
        set_desc(
            &mut ram,
            0,
            BUF,
            1,
            VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE,
            1,
        );
        set_desc(&mut ram, 1, BUF, 1, 0, 0);
        publish(&mut ram, 0, &[0], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert_eq!(q.pop(&ram), Err(QueueError::ReadableAfterWritable));
    }

    #[test]
    fn buffer_address_overflow_is_rejected_but_unmapped_is_not_checked_here() {
        let mut ram = ram();
        set_desc(&mut ram, 0, u64::MAX - 3, 8, 0, 0);
        publish(&mut ram, 0, &[0], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert!(matches!(
            q.pop(&ram),
            Err(QueueError::BufferOverflow { .. })
        ));

        // A buffer pointing outside RAM is accepted by the walker; the device's data access
        // through GuestMemory reports MemError later.
        let mut ram = self::ram();
        set_desc(&mut ram, 0, 0xdead_0000_0000, 8, 0, 0);
        publish(&mut ram, 0, &[0], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert!(q.pop(&ram).unwrap().is_some());
    }

    #[test]
    fn rings_outside_memory_fail_without_panic() {
        let ram = RamRegion::new(GuestPhysAddr(BASE), 0x100); // too small for the rings
        let mut q = Queue::new(cfg(8)).unwrap();
        assert!(matches!(q.pop(&ram), Err(QueueError::Memory(_))));
        assert!(q.is_broken());
    }

    #[test]
    fn broken_queue_stays_broken() {
        let mut ram = ram();
        publish(&mut ram, 0, &[99], 8, 1);
        let mut q = Queue::new(cfg(8)).unwrap();
        assert!(q.pop(&ram).is_err());
        assert_eq!(q.pop(&ram), Err(QueueError::Broken));
        assert_eq!(q.push_used(&mut ram, 0, 0), Err(QueueError::Broken));
    }

    #[test]
    fn push_used_writes_entry_then_index_and_wraps() {
        let mut ram = ram();
        let mut q = Queue::new(cfg(2)).unwrap();
        q.push_used(&mut ram, 1, 0x40).unwrap();
        assert_eq!(ram.read_u32(GuestPhysAddr(USED + 4)).unwrap(), 1);
        assert_eq!(ram.read_u32(GuestPhysAddr(USED + 8)).unwrap(), 0x40);
        let mut b = [0u8; 2];
        ram.read(GuestPhysAddr(USED + 2), &mut b).unwrap();
        assert_eq!(u16::from_le_bytes(b), 1);

        q.push_used(&mut ram, 0, 7).unwrap();
        q.push_used(&mut ram, 1, 9).unwrap(); // wraps to slot 0
        assert_eq!(ram.read_u32(GuestPhysAddr(USED + 4)).unwrap(), 1);
        assert_eq!(ram.read_u32(GuestPhysAddr(USED + 8)).unwrap(), 9);
        ram.read(GuestPhysAddr(USED + 2), &mut b).unwrap();
        assert_eq!(u16::from_le_bytes(b), 3);
    }

    #[test]
    fn push_used_validates_head() {
        let mut ram = ram();
        let mut q = Queue::new(cfg(4)).unwrap();
        assert_eq!(
            q.push_used(&mut ram, 4, 0),
            Err(QueueError::IndexOutOfRange { index: 4, size: 4 })
        );
    }

    #[test]
    fn index_wraparound_across_u16() {
        let mut ram = ram();
        set_desc(&mut ram, 0, BUF, 1, 0, 0);
        let mut q = Queue::new(cfg(4)).unwrap();
        // Walk next_avail up to 0xfffe by faking consumption of 65534 entries.
        q.next_avail = 0xfffe;
        // slot = 0xfffe % 4 = 2; idx 0x0001 => pending = 3
        publish(&mut ram, 0xfffe, &[0, 0, 0], 4, 0x0001);
        for _ in 0..3 {
            assert!(q.pop(&ram).unwrap().is_some());
        }
        assert_eq!(q.pop(&ram).unwrap(), None);
    }

    #[test]
    fn notification_suppression_flag() {
        let mut ram = ram();
        let mut q = Queue::new(cfg(4)).unwrap();
        assert!(q.should_notify(&ram).unwrap());
        ram.write(
            GuestPhysAddr(AVAIL),
            &VIRTQ_AVAIL_F_NO_INTERRUPT.to_le_bytes(),
        )
        .unwrap();
        assert!(!q.should_notify(&ram).unwrap());
    }

    #[test]
    fn works_through_dyn_guest_memory() {
        let mut ram = ram();
        set_desc(&mut ram, 0, BUF, 1, 0, 0);
        publish(&mut ram, 0, &[0], 4, 1);
        let mem: &mut dyn GuestMemory = &mut ram;
        let mut q = Queue::new(cfg(4)).unwrap();
        let c = q.pop(mem).unwrap().unwrap();
        q.push_used(mem, c.head, 0).unwrap();
    }
}
