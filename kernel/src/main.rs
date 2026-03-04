#![no_std]
#![no_main]

mod arch {
    pub mod aarch64;
}

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    loop {
        // SAFETY: wfe is a hint instruction that puts the core in low-power
        // state until an event occurs. Safe to execute at EL1.
        unsafe { core::arch::asm!("wfe") }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // SAFETY: PL011 UART DR is at 0x0900_0000 and FR is at 0x0900_0018.
    // Both are valid MMIO on QEMU virt. TXFF (bit 5 of FR) is polled before
    // each write to avoid overrun. Faulting on other platforms is acceptable
    // in the panic path — the machine is already in an unrecoverable state.
    const UART_BASE: usize = 0x0900_0000;
    for b in b"PANIC\n" {
        unsafe {
            let fr = (UART_BASE + 0x018) as *const u32;
            while core::ptr::read_volatile(fr) & (1 << 5) != 0 {}
            core::ptr::write_volatile(UART_BASE as *mut u32, *b as u32);
        }
    }
    loop {
        // SAFETY: wfe is a hint instruction, safe at any EL.
        unsafe { core::arch::asm!("wfe") }
    }
}
