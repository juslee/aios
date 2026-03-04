#![no_std]
#![no_main]

mod arch {
    pub mod aarch64;
}

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // SAFETY: `wfe` is a hint that parks the core in low-power state until an
    // event occurs. The branch loops back unconditionally so this asm block
    // truly never returns (`noreturn`), which also prevents LLVM from treating
    // the instruction as removable. Safe to execute at EL1.
    unsafe {
        core::arch::asm!(
            "1: wfe",
            "b 1b",
            options(noreturn, nomem, nostack, preserves_flags)
        )
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // SAFETY: Same as above — infinite wfe loop encoded in asm with
    // `noreturn` so the instruction cannot be elided during a panic halt.
    unsafe {
        core::arch::asm!(
            "1: wfe",
            "b 1b",
            options(noreturn, nomem, nostack, preserves_flags)
        )
    }
}
