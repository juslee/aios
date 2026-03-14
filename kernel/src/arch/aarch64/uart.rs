//! PL011 UART driver.
//!
//! Supports two modes:
//! 1. Early boot: hardcoded base 0x0900_0000 (QEMU pre-initialized).
//! 2. Post-DTB: full PL011 initialization from DTB-sourced base address
//!    with baud rate programming (required on real hardware).

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

// NOTE: UART locking not yet implemented. WB cacheable memory (Phase 2 M8+) makes
// spin::Mutex safe, but UART output is currently unlocked — interleaved output
// from multiple cores is possible. Acceptable for kernel diagnostics; formal
// locking will be added when contention becomes a problem.

/// PL011 UART physical base address on QEMU virt (hal.md §4.3).
pub const UART_PHYS: usize = 0x0900_0000;

/// Current UART base address — starts with QEMU default, updated after DTB parse.
static UART_BASE_ADDR: AtomicUsize = AtomicUsize::new(UART_PHYS);

// PL011 register offsets (hal.md §4.3)
const UARTDR: usize = 0x000;
const UARTFR: usize = 0x018;
const UARTIBRD: usize = 0x024;
const UARTFBRD: usize = 0x028;
const UARTLCR_H: usize = 0x02C;
const UARTCR: usize = 0x030;

// Flag register bits
const FR_TXFF: u32 = 1 << 5;
const FR_BUSY: u32 = 1 << 3;

/// PL011 UART handle returned by init_pl011().
#[allow(dead_code)]
pub struct Uart {
    base: usize,
}

/// Perform full PL011 initialization at the given base address.
///
/// Programs baud rate to 115200 with 24 MHz UART clock (IBRD=13, FBRD=1),
/// 8N1 with FIFO enabled. Updates the global UART base so print!/println!
/// continue to work.
pub fn init_pl011(base: usize) -> Uart {
    // SAFETY: All writes are to PL011 MMIO registers at the DTB-provided
    // base address. The initialization sequence follows the PL011 TRM.
    unsafe {
        // 1. Disable UART: clear CR.UARTEN (bit 0)
        mmio_write32(base + UARTCR, 0);

        // 2. Wait for any in-progress transmission (poll FR.BUSY)
        let mut timeout = 100_000u32;
        while mmio_read32(base + UARTFR) & FR_BUSY != 0 {
            timeout -= 1;
            if timeout == 0 {
                break;
            }
        }

        // 3. Flush FIFO: clear LCR_H.FEN (bit 4) — writing 0 flushes
        mmio_write32(base + UARTLCR_H, 0);

        // 4. Program baud rate: 24 MHz / (16 * 115200) = 13.0208...
        //    IBRD = 13, FBRD = round(0.0208 * 64) = 1
        mmio_write32(base + UARTIBRD, 13);
        mmio_write32(base + UARTFBRD, 1);

        // 5. Line control: 8-bit, 1 stop, no parity, FIFO enabled
        //    LCR_H = 0x70 (WLEN=0b11 [8-bit] | FEN [FIFO enable])
        mmio_write32(base + UARTLCR_H, 0x70);

        // 6. Re-enable UART: CR = 0x301 (UARTEN | TXE | RXE)
        mmio_write32(base + UARTCR, 0x301);
    }

    // Update global base for print!/println! macros
    UART_BASE_ADDR.store(base, Ordering::Relaxed);

    Uart { base }
}

/// Update the UART base address (e.g., after MMU maps MMIO to virtual addresses).
pub fn update_base(new_base: usize) {
    UART_BASE_ADDR.store(new_base, Ordering::Relaxed);
}

/// Write a single byte to the PL011 UART.
pub fn putc(byte: u8) {
    let base = UART_BASE_ADDR.load(Ordering::Relaxed);
    // SAFETY: UART base is a valid PL011 MMIO address (set by init or default).
    unsafe {
        while mmio_read32(base + UARTFR) & FR_TXFF != 0 {}
        mmio_write32(base + UARTDR, byte as u32);
    }
}

#[inline(always)]
unsafe fn mmio_read32(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

#[inline(always)]
unsafe fn mmio_write32(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}

#[allow(dead_code)]
impl Uart {
    /// Get the base address of this UART.
    pub fn base(&self) -> usize {
        self.base
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

/// Write formatted arguments to the UART.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let mut writer = UartWriter;
    fmt::Write::write_fmt(&mut writer, args).unwrap();
}

/// Print to the UART without a newline.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::arch::aarch64::uart::_print(format_args!($($arg)*))
    };
}

/// Print to the UART with a trailing newline.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {
        $crate::arch::aarch64::uart::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
