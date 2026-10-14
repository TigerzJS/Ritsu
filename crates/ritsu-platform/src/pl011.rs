//! Minimal ARM PL011 UART model.
//!
//! Register map and reset values follow the public *ARM PrimeCell UART (PL011) Technical
//! Reference Manual* (ARM DDI 0183, "Programmers model", register summary) and the PrimeCell
//! identification scheme (ARM DDI 0183 "Peripheral Identification Registers" /
//! "PrimeCell Identification Registers"). Whether the vmapple guest uses the PL011 at a
//! particular address and interrupt is **Annahme** (QEMU documents a PL011 on `vmapple`
//! as the boot console); it is wired up by the machine model, not here.
//!
//! What is modelled:
//!
//! * `UARTDR` writes go to a [`ByteSink`]; the transmit FIFO is infinitely fast, so
//!   `UARTFR` always reports TX-FIFO empty and receive FIFO empty (no input yet).
//! * `UARTCR`, `UARTLCR_H`, `UARTIBRD`, `UARTFBRD`, `UARTIFLS`, `UARTIMSC` are plain
//!   read/write storage so that a guest driver can program and read them back.
//! * `UARTRIS`, `UARTMIS`, `UARTDMACR`, `UARTRSR`, `UARTILPR` read as 0; writes are ignored.
//! * The ID registers return the PL011 values from the TRM.
//!
//! Not modelled (yet): receive path, interrupts, FIFO timing, baud-rate effects.

use crate::mmio::{AccessSize, MmioDevice, MmioError};

/// Size of the PL011 register window (4 KiB, TRM register summary ends at `0xFFC`).
pub const PL011_REGION_LEN: u64 = 0x1000;

/// `UARTDR`: data register.
pub const UARTDR: u64 = 0x000;
/// `UARTRSR` (read) / `UARTECR` (write): receive status / error clear.
pub const UARTRSR_ECR: u64 = 0x004;
/// `UARTFR`: flag register.
pub const UARTFR: u64 = 0x018;
/// `UARTILPR`: IrDA low-power counter.
pub const UARTILPR: u64 = 0x020;
/// `UARTIBRD`: integer baud rate divisor.
pub const UARTIBRD: u64 = 0x024;
/// `UARTFBRD`: fractional baud rate divisor.
pub const UARTFBRD: u64 = 0x028;
/// `UARTLCR_H`: line control.
pub const UARTLCR_H: u64 = 0x02c;
/// `UARTCR`: control register.
pub const UARTCR: u64 = 0x030;
/// `UARTIFLS`: interrupt FIFO level select.
pub const UARTIFLS: u64 = 0x034;
/// `UARTIMSC`: interrupt mask set/clear.
pub const UARTIMSC: u64 = 0x038;
/// `UARTRIS`: raw interrupt status.
pub const UARTRIS: u64 = 0x03c;
/// `UARTMIS`: masked interrupt status.
pub const UARTMIS: u64 = 0x040;
/// `UARTICR`: interrupt clear.
pub const UARTICR: u64 = 0x044;
/// `UARTDMACR`: DMA control.
pub const UARTDMACR: u64 = 0x048;
/// `UARTPeriphID0..3` occupy `0xFE0..=0xFEC`.
pub const UART_PERIPH_ID0: u64 = 0xfe0;
/// `UARTPCellID0..3` occupy `0xFF0..=0xFFC`.
pub const UART_PCELL_ID0: u64 = 0xff0;

/// `UARTFR` bit 4: receive FIFO empty.
pub const FR_RXFE: u64 = 1 << 4;
/// `UARTFR` bit 7: transmit FIFO empty.
pub const FR_TXFE: u64 = 1 << 7;

/// Reset value of `UARTCR` (TXE and RXE set, UARTEN clear), per the TRM.
pub const CR_RESET: u64 = 0x0300;

/// PL011 peripheral ID bytes `UARTPeriphID0..3`: part number 0x011, designer ARM (0x41).
/// The revision nibble in `PeriphID2` is **Annahme** (guest drivers only check the part number).
const PERIPH_ID: [u64; 4] = [0x11, 0x10, 0x34, 0x00];
/// PrimeCell ID bytes `UARTPCellID0..3` (fixed `0x0D, 0xF0, 0x05, 0xB1` for all PrimeCells).
const PCELL_ID: [u64; 4] = [0x0d, 0xf0, 0x05, 0xb1];

/// Receiver of bytes the guest transmits on the UART.
pub trait ByteSink: Send {
    /// Called once per byte written to `UARTDR`.
    fn put(&mut self, byte: u8);
}

impl<F: FnMut(u8) + Send> ByteSink for F {
    fn put(&mut self, byte: u8) {
        self(byte);
    }
}

/// A minimal PL011 UART. See the [module documentation](self) for what is modelled.
pub struct Pl011 {
    sink: Box<dyn ByteSink>,
    cr: u64,
    lcr_h: u64,
    ibrd: u64,
    fbrd: u64,
    ifls: u64,
    imsc: u64,
}

impl std::fmt::Debug for Pl011 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pl011")
            .field("cr", &self.cr)
            .field("imsc", &self.imsc)
            .finish_non_exhaustive()
    }
}

impl Pl011 {
    /// Creates a UART in its reset state that forwards transmitted bytes to `sink`.
    #[must_use]
    pub fn new(sink: Box<dyn ByteSink>) -> Self {
        Self {
            sink,
            cr: CR_RESET,
            lcr_h: 0,
            ibrd: 0,
            fbrd: 0,
            ifls: 0x12,
            imsc: 0,
        }
    }

    fn check(offset: u64, size: AccessSize) -> Result<(), MmioError> {
        // Registers are 32 bits wide at 4-byte pitch; 8-byte accesses are not defined.
        if size == AccessSize::B8 {
            return Err(MmioError::BadSize {
                device: "pl011".into(),
                offset,
                size: size.bytes(),
            });
        }
        if offset >= PL011_REGION_LEN || !offset.is_multiple_of(4) {
            return Err(MmioError::BadOffset {
                device: "pl011".into(),
                offset,
            });
        }
        Ok(())
    }
}

impl MmioDevice for Pl011 {
    fn name(&self) -> &str {
        "pl011"
    }

    fn read(&mut self, offset: u64, size: AccessSize) -> Result<u64, MmioError> {
        Self::check(offset, size)?;
        Ok(match offset {
            // No receive path yet: reading the data register yields 0.
            UARTDR => 0,
            UARTFR => FR_TXFE | FR_RXFE,
            UARTIBRD => self.ibrd,
            UARTFBRD => self.fbrd,
            UARTLCR_H => self.lcr_h,
            UARTCR => self.cr,
            UARTIFLS => self.ifls,
            UARTIMSC => self.imsc,
            UART_PERIPH_ID0..=0xfec => PERIPH_ID[((offset - UART_PERIPH_ID0) / 4) as usize],
            UART_PCELL_ID0..=0xffc => PCELL_ID[((offset - UART_PCELL_ID0) / 4) as usize],
            // UARTRSR, UARTILPR, UARTRIS, UARTMIS, UARTDMACR and unimplemented/reserved
            // offsets read as zero (read-as-zero is the documented behaviour of this model).
            _ => 0,
        })
    }

    fn write(&mut self, offset: u64, size: AccessSize, value: u64) -> Result<(), MmioError> {
        Self::check(offset, size)?;
        match offset {
            // Only the low 8 bits of UARTDR carry data (TRM: DATA[7:0]).
            UARTDR => self.sink.put((value & 0xff) as u8),
            UARTIBRD => self.ibrd = value & 0xffff,
            UARTFBRD => self.fbrd = value & 0x3f,
            UARTLCR_H => self.lcr_h = value & 0xff,
            UARTCR => self.cr = value & 0xffff,
            UARTIFLS => self.ifls = value & 0x3f,
            UARTIMSC => self.imsc = value & 0x7ff,
            // Everything else (ECR, ILPR, ICR, DMACR, read-only or reserved): write ignored.
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmio::MmioBus;
    use std::sync::{Arc, Mutex};

    fn uart() -> (Pl011, Arc<Mutex<Vec<u8>>>) {
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&out);
        let u = Pl011::new(Box::new(move |b| sink.lock().unwrap().push(b)));
        (u, out)
    }

    #[test]
    fn dr_write_reaches_sink_low_byte_only() {
        let (mut u, out) = uart();
        for c in b"hi" {
            u.write(UARTDR, AccessSize::B4, u64::from(*c)).unwrap();
        }
        u.write(UARTDR, AccessSize::B4, 0xffff_ff21).unwrap();
        assert_eq!(*out.lock().unwrap(), b"hi!");
    }

    #[test]
    fn flag_register_reports_tx_and_rx_empty() {
        let (mut u, _) = uart();
        let fr = u.read(UARTFR, AccessSize::B4).unwrap();
        assert_eq!(fr & FR_TXFE, FR_TXFE);
        assert_eq!(fr & FR_RXFE, FR_RXFE);
        assert_eq!(fr & (1 << 5), 0, "TX FIFO must never report full");
        assert_eq!(fr & (1 << 3), 0, "never busy");
    }

    #[test]
    fn id_registers_match_trm() {
        let (mut u, _) = uart();
        let periph: Vec<u64> = (0..4)
            .map(|i| u.read(0xfe0 + 4 * i, AccessSize::B4).unwrap())
            .collect();
        assert_eq!(&periph[..2], &[0x11, 0x10]);
        let cell: Vec<u64> = (0..4)
            .map(|i| u.read(0xff0 + 4 * i, AccessSize::B4).unwrap())
            .collect();
        assert_eq!(cell, [0x0d, 0xf0, 0x05, 0xb1]);
    }

    #[test]
    fn control_registers_read_back_and_reset_value() {
        let (mut u, _) = uart();
        assert_eq!(u.read(UARTCR, AccessSize::B4).unwrap(), CR_RESET);
        u.write(UARTCR, AccessSize::B4, 0x301).unwrap();
        u.write(UARTLCR_H, AccessSize::B4, 0x70).unwrap();
        u.write(UARTIBRD, AccessSize::B4, 13).unwrap();
        u.write(UARTFBRD, AccessSize::B4, 1).unwrap();
        assert_eq!(u.read(UARTCR, AccessSize::B4).unwrap(), 0x301);
        assert_eq!(u.read(UARTLCR_H, AccessSize::B4).unwrap(), 0x70);
        assert_eq!(u.read(UARTIBRD, AccessSize::B4).unwrap(), 13);
        assert_eq!(u.read(UARTFBRD, AccessSize::B4).unwrap(), 1);
    }

    #[test]
    fn unmodelled_registers_read_zero_and_ignore_writes() {
        let (mut u, out) = uart();
        for off in [
            UARTRIS,
            UARTMIS,
            UARTDMACR,
            UARTRSR_ECR,
            UARTILPR,
            0x100,
            0xf00,
        ] {
            assert_eq!(u.read(off, AccessSize::B4).unwrap(), 0, "{off:#x}");
            u.write(off, AccessSize::B4, 0xffff_ffff).unwrap();
            assert_eq!(u.read(off, AccessSize::B4).unwrap(), 0, "{off:#x}");
        }
        assert!(out.lock().unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_offsets_and_sizes_without_panicking() {
        let (mut u, _) = uart();
        assert!(matches!(
            u.read(UARTFR, AccessSize::B8),
            Err(MmioError::BadSize { .. })
        ));
        assert!(matches!(
            u.write(UARTDR, AccessSize::B8, 1),
            Err(MmioError::BadSize { .. })
        ));
        assert!(matches!(
            u.read(0x1000, AccessSize::B4),
            Err(MmioError::BadOffset { .. })
        ));
        assert!(matches!(
            u.read(u64::MAX, AccessSize::B1),
            Err(MmioError::BadOffset { .. })
        ));
        assert!(matches!(
            u.read(0x19, AccessSize::B1),
            Err(MmioError::BadOffset { .. })
        ));
    }

    #[test]
    fn works_behind_the_bus() {
        let (u, out) = uart();
        let mut bus = MmioBus::new();
        bus.register(0x0900_0000, PL011_REGION_LEN, Box::new(u))
            .unwrap();
        bus.write(0x0900_0000, AccessSize::B1, u64::from(b'x'))
            .unwrap();
        assert_eq!(*out.lock().unwrap(), b"x");
        assert_eq!(
            bus.read(0x0900_0018, AccessSize::B4).unwrap() & FR_TXFE,
            FR_TXFE
        );
        assert!(bus.read(0x0900_1000, AccessSize::B4).is_err());
    }
}
