//! PL011 UART driver for QEMU virt machine.
//!
//! Phase 0: hardcoded MMIO at 0x0900_0000. QEMU pre-initializes the PL011,
//! so no baud rate configuration is needed. Phase 1+ uses the HAL Platform
//! trait and reads the base address from the device tree.

use core::fmt;

/// PL011 UART base address on QEMU virt (UART0).
const UART_BASE: usize = 0x0900_0000;

/// UART Data Register offset.
const UARTDR: usize = 0x000;
/// UART Flag Register offset.
const UARTFR: usize = 0x018;
/// TXFF (Transmit FIFO Full) flag — bit 5 of UARTFR.
const TXFF: u32 = 1 << 5;

/// Write a single byte to the PL011 UART.
///
/// Spins until the transmit FIFO has space, then writes the byte.
pub fn putc(byte: u8) {
    // SAFETY: UART base 0x0900_0000 is valid MMIO on QEMU virt. QEMU maps
    // this region unconditionally. Writing to DR after checking TXFF ensures
    // the FIFO is not full. On non-QEMU hardware this address may not be
    // mapped, but Phase 0 only targets QEMU.
    unsafe {
        let fr = (UART_BASE + UARTFR) as *const u32;
        let dr = (UART_BASE + UARTDR) as *mut u32;

        while core::ptr::read_volatile(fr) & TXFF != 0 {}
        core::ptr::write_volatile(dr, byte as u32);
    }
}

/// A writer that outputs to the PL011 UART, implementing `core::fmt::Write`.
pub struct UartWriter;

impl fmt::Write for UartWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                putc(b'\r');
            }
            putc(byte);
        }
        Ok(())
    }
}

/// Print to the UART without a newline.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::arch::aarch64::uart::UartWriter, $($arg)*);
    }};
}

/// Print to the UART with a trailing newline.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::arch::aarch64::uart::UartWriter, $($arg)*);
    }};
}
