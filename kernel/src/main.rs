#![no_std]
#![no_main]

extern crate alloc;

mod arch {
    pub mod aarch64;
}
mod boot_phase;
mod dtb;
mod mm;
mod platform;

use core::fmt::Write;
use core::panic::PanicInfo;
use shared::{BootInfo, BOOTINFO_MAGIC};

use crate::boot_phase::{advance_boot_phase, EarlyBootPhase};

// Include the assembly boot code (entry point + exception vector stubs).
core::arch::global_asm!(include_str!("arch/aarch64/boot.S"));

#[no_mangle]
pub extern "C" fn kernel_main(boot_info_ptr: u64) -> ! {
    use crate::arch::aarch64::exceptions;

    println!("AIOS kernel booting...");

    // Initialize boot timing from ARM Generic Timer counter.
    boot_phase::init_boot_timing();

    // Validate BootInfo.
    let boot_info = if boot_info_ptr != 0 {
        // SAFETY: The UEFI stub allocates a page-aligned, fully-initialized BootInfo
        // struct and passes its physical address in x0. We validated the pointer is
        // non-zero; the magic check below confirms the struct is intact.
        let bi = unsafe { &*(boot_info_ptr as *const BootInfo) };
        if bi.magic == BOOTINFO_MAGIC {
            println!("[boot] BootInfo at {:#x}, magic OK", boot_info_ptr);
        } else {
            println!(
                "[boot] BootInfo at {:#x}, BAD magic {:#x} — halting",
                boot_info_ptr, bi.magic
            );
            halt();
        }
        bi
    } else {
        println!("[boot] No BootInfo (Phase 0 mode) — halting");
        halt();
    };

    // Install the Rust-owned exception vector table, replacing the boot.S stub.
    let vbar = exceptions::install_vector_table();

    // Boot diagnostics.
    println!("[boot] EL:       {}", exceptions::current_el());
    println!("[boot] Core ID:  {}", exceptions::core_id());
    println!("[boot] VBAR_EL1: {:#018x}", vbar);
    debug_assert_eq!(vbar, exceptions::read_vbar_el1());

    advance_boot_phase(EarlyBootPhase::ExceptionVectors);

    // --- Step 3: DTB Parse and Platform Detection ---
    let dt = if boot_info.device_tree != 0 {
        // SAFETY: device_tree is a valid physical address from the UEFI stub,
        // pointing to a DTB blob provided by QEMU via the EFI config table.
        unsafe { dtb::DeviceTree::parse(boot_info.device_tree) }
    } else {
        None
    };

    let dt = dt.unwrap_or_else(|| {
        println!("[boot] No DTB — using QEMU virt defaults");
        dtb::DeviceTree::qemu_defaults()
    });

    let platform = platform::detect_platform(&dt);
    println!("[boot] DeviceTreeParsed — {}", platform.name());
    println!(
        "[boot]   CPUs: {}, PSCI: {}",
        dt.cpu_count(),
        dt.psci_method().unwrap_or("none")
    );
    advance_boot_phase(EarlyBootPhase::DeviceTreeParsed);

    // --- Step 4: Full PL011 UART Initialization ---
    let _uart = platform.init_uart(&dt);
    advance_boot_phase(EarlyBootPhase::UartReady);

    // --- Step 5: GICv3 + Timer ---
    let ic = platform.init_interrupts(&dt);
    advance_boot_phase(EarlyBootPhase::InterruptsReady);

    let timer = platform.init_timer(&dt, &ic);
    advance_boot_phase(EarlyBootPhase::TimerReady);
    println!("[boot]   CNTFRQ={}Hz", timer.frequency());

    // --- Step 6: MMU, Buddy Allocator, Heap ---

    // SAFETY: Called once from boot CPU. Page table statics are not accessed
    // concurrently. Identity map covers currently executing code at 0x40080000.
    unsafe { crate::arch::aarch64::mmu::init_mmu() };
    advance_boot_phase(EarlyBootPhase::MmuEnabled);

    // Initialize buddy allocator from UEFI memory map.
    extern "C" {
        static __kernel_end: u8;
    }
    let kernel_start = boot_info.kernel_phys_base as usize;
    // SAFETY: __kernel_end is defined by the linker script.
    let kernel_end = unsafe { &__kernel_end as *const u8 as usize };
    // Use the larger of BootInfo's kernel_size and linker-computed end.
    let kernel_end = kernel_end.max(kernel_start + boot_info.kernel_size as usize);

    // SAFETY: Memory map is valid, MMU identity map is active, kernel range
    // is correctly computed from BootInfo and linker symbols.
    let free_pages = unsafe {
        crate::mm::buddy::init(
            boot_info.memory_map_addr,
            boot_info.memory_map_count,
            boot_info.memory_map_entry_size,
            kernel_start,
            kernel_end,
        )
    };
    println!(
        "[boot] PageAllocatorReady — {} pages free ({} MiB)",
        free_pages,
        (free_pages * 4096) / (1024 * 1024)
    );
    advance_boot_phase(EarlyBootPhase::PageAllocatorReady);

    // Switch global allocator from bump to slab (backed by buddy).
    crate::mm::enable_slab_allocator();
    advance_boot_phase(EarlyBootPhase::HeapReady);

    // Verify heap works with a Box allocation.
    {
        use alloc::boxed::Box;
        let x = Box::new(42u32);
        assert_eq!(*x, 42);
        println!("[boot] Box::new(42) = {} — heap verified", *x);
    }

    println!("[boot] Boot sequence complete, entering idle loop");

    loop {
        // SAFETY: wfe is a hint instruction that puts the core in low-power
        // state until an event occurs. Safe to execute at EL1.
        unsafe { core::arch::asm!("wfe") }
    }
}

/// Halt the CPU in a low-power loop (never returns).
fn halt() -> ! {
    loop {
        // SAFETY: wfe is a hint instruction, safe at any EL.
        unsafe { core::arch::asm!("wfe") }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // SAFETY: UartWriter accesses PL011 MMIO at the current UART base address.
    // In the panic path, correctness of output is best-effort.
    let mut w = crate::arch::aarch64::uart::UartWriter;
    let _ = writeln!(&mut w, "PANIC: {}", info);
    halt()
}
