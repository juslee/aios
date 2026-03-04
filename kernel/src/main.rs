#![no_std]
#![no_main]

mod arch {
    pub mod aarch64;
}

use core::fmt::Write;
use core::panic::PanicInfo;
use shared::{BootInfo, BOOTINFO_MAGIC};

// Include the assembly boot code (entry point + exception vector stubs).
core::arch::global_asm!(include_str!("arch/aarch64/boot.S"));

#[no_mangle]
pub extern "C" fn kernel_main(boot_info_ptr: u64) -> ! {
    use crate::arch::aarch64::exceptions;

    println!("AIOS kernel booting...");

    // Validate BootInfo if a non-zero pointer was passed (Phase 1+ UEFI boot).
    if boot_info_ptr != 0 {
        // SAFETY: The UEFI stub allocates a page-aligned, fully-initialized BootInfo
        // struct and passes its physical address in x0. We validated the pointer is
        // non-zero; the magic check below confirms the struct is intact.
        let boot_info = unsafe { &*(boot_info_ptr as *const BootInfo) };
        if boot_info.magic == BOOTINFO_MAGIC {
            println!("[boot] BootInfo at {:#x}, magic OK", boot_info_ptr);
        } else {
            println!(
                "[boot] BootInfo at {:#x}, BAD magic {:#x}",
                boot_info_ptr, boot_info.magic
            );
        }
    }

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
