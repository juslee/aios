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
    use crate::arch::aarch64::exceptions;

    println!("AIOS kernel booting...");

    // Install the Rust-owned exception vector table, replacing the boot.S stub.
    let vbar = exceptions::install_vector_table();

    // Boot diagnostics — verify CPU state matches expectations.
    println!("[boot] EL:       {}", exceptions::current_el());
    println!("[boot] Core ID:  {}", exceptions::core_id());
    println!("[boot] VBAR_EL1: {:#018x}", vbar);

    // Verify VBAR_EL1 was written correctly by reading it back.
    debug_assert_eq!(vbar, exceptions::read_vbar_el1());

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
    let mut w = crate::arch::aarch64::uart::UartWriter;
    let _ = writeln!(&mut w, "PANIC: {}", info);

    loop {
        // SAFETY: wfe is a hint instruction, safe at any EL.
        unsafe { core::arch::asm!("wfe") }
    }
}
