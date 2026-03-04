#![no_std]
#![no_main]

mod arch {
    pub mod aarch64;
}

use core::fmt::Write;
use core::panic::PanicInfo;

// Include the assembly boot code (entry point + exception vector stubs).
core::arch::global_asm!(include_str!("arch/aarch64/boot.S"));

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    println!("AIOS kernel booting...");

    loop {
        // SAFETY: wfe is a hint instruction that puts the core in low-power
        // state until an event occurs. Safe to execute at EL1.
        unsafe { core::arch::asm!("wfe") }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // SAFETY: UartWriter accesses PL011 MMIO at 0x0900_0000, which is valid
    // on QEMU virt. In the panic path, correctness of output is best-effort.
    let _ = writeln!(crate::arch::aarch64::uart::UartWriter, "PANIC: {}", info);

    loop {
        // SAFETY: wfe is a hint instruction, safe at any EL.
        unsafe { core::arch::asm!("wfe") }
    }
}
