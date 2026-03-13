#![no_std]
#![no_main]

extern crate alloc;

mod arch {
    pub mod aarch64;
}
mod bench;
mod boot_phase;
mod cap;
mod drivers;
mod dtb;
mod framebuffer;
mod ipc;
mod mm;
mod observability;
mod platform;
mod sched;
mod service;
mod smp;
mod syscall;
mod task;

use core::fmt::Write;
use core::panic::PanicInfo;
use shared::{BootInfo, BOOTINFO_MAGIC};

use crate::boot_phase::{advance_boot_phase, EarlyBootPhase};

// Include the assembly boot code (entry point + exception vector stubs).
core::arch::global_asm!(include_str!("arch/aarch64/boot.S"));
// Include context switch assembly (save_context / restore_context).
core::arch::global_asm!(include_str!("arch/aarch64/context_switch.S"));

#[no_mangle]
pub extern "C" fn kernel_main(boot_info_ptr: u64) -> ! {
    use crate::arch::aarch64::exceptions;

    kinfo!(Boot, "AIOS kernel booting...");

    // Initialize boot timing from ARM Generic Timer counter.
    boot_phase::init_boot_timing();

    // Validate BootInfo. The UEFI stub allocates BootInfo in LOADER_DATA pages.
    // We extract all needed fields before buddy init, which excludes the memory
    // map buffer pages from the free list to prevent corruption.
    let boot_info = if boot_info_ptr != 0 {
        // SAFETY: The UEFI stub allocates a page-aligned, fully-initialized BootInfo
        // struct and passes its physical address in x0. We validated the pointer is
        // non-zero; the magic check below confirms the struct is intact.
        let bi = unsafe { &*(boot_info_ptr as *const BootInfo) };
        if bi.magic == BOOTINFO_MAGIC {
            kinfo!(Boot, "BootInfo at {:#x}, magic OK", boot_info_ptr);
        } else {
            kerror!(
                Boot,
                "BootInfo at {:#x}, BAD magic {:#x} — halting",
                boot_info_ptr,
                bi.magic
            );
            halt();
        }
        bi
    } else {
        kerror!(Boot, "No BootInfo (Phase 0 mode) — halting");
        halt();
    };

    // Install the Rust-owned exception vector table, replacing the boot.S stub.
    let vbar = exceptions::install_vector_table();

    // Boot diagnostics.
    kinfo!(Boot, "EL: {}", exceptions::current_el());
    kinfo!(Boot, "Core ID: {}", exceptions::core_id());
    kinfo!(Boot, "VBAR_EL1: {:#018x}", vbar);
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
        kwarn!(Boot, "No DTB — using QEMU virt defaults");
        dtb::DeviceTree::qemu_defaults()
    });

    let platform = platform::detect_platform(&dt);
    kinfo!(Boot, "DeviceTreeParsed — {}", platform.name());
    kinfo!(
        Boot,
        "CPUs: {}, PSCI: {}",
        dt.cpu_count(),
        dt.psci_method().unwrap_or("none")
    );
    advance_boot_phase(EarlyBootPhase::DeviceTreeParsed);

    // --- Step 4: Full PL011 UART Initialization ---
    let _uart = platform.init_uart(&dt);
    advance_boot_phase(EarlyBootPhase::UartReady);

    // --- Step 5a: TTBR0 identity map ---
    // Must run before GIC/timer init: QEMU 10.x edk2 firmware may not
    // preserve device-memory MMIO mappings in TTBR0 post-ExitBootServices.
    // Our init_mmu() builds an identity map with 0-1GB device memory.
    // SAFETY: Called once from boot CPU. Page table statics are not accessed
    // concurrently. Identity map covers currently executing code at 0x40080000.
    unsafe { crate::arch::aarch64::mmu::init_mmu() };
    advance_boot_phase(EarlyBootPhase::MmuEnabled);

    // --- Step 5b: GICv3 + Timer ---
    let ic = platform.init_interrupts(&dt);
    advance_boot_phase(EarlyBootPhase::InterruptsReady);

    let timer = platform.init_timer(&dt, &ic);
    advance_boot_phase(EarlyBootPhase::TimerReady);
    kinfo!(Boot, "CNTFRQ={}Hz", timer.frequency());

    // Store tick interval for IRQ handler and secondary core init.
    crate::arch::aarch64::timer::set_tick_interval(timer.tick_interval());

    // --- Step 6: Buddy Allocator, Heap ---

    // Initialize physical memory pools from UEFI memory map.
    // SAFETY: Memory map is valid, MMU identity map is active, called once
    // from boot CPU. init_memory computes kernel range from BootInfo + linker
    // symbols and excludes kernel image + memory map buffer from free pages.
    unsafe { crate::mm::init::init_memory(boot_info) };
    advance_boot_phase(EarlyBootPhase::PageAllocatorReady);

    // Switch global allocator from bump to slab (backed by buddy).
    crate::mm::enable_slab_allocator();
    crate::mm::init_heap();
    advance_boot_phase(EarlyBootPhase::HeapReady);

    // Log rings are now safe to use (heap + slab ready for drain formatting).
    advance_boot_phase(EarlyBootPhase::LogRingsReady);

    // Verify heap works with a Box<[u8; 1024]> write/read/drop cycle.
    {
        use alloc::boxed::Box;
        let mut buf = Box::new([0u8; 1024]);
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte = (i & 0xFF) as u8;
        }
        for (i, byte) in buf.iter().enumerate() {
            assert_eq!(*byte, (i & 0xFF) as u8);
        }
        kinfo!(Boot, "Box<[u8; 1024]> write/read/drop — heap verified");
        // buf drops here, exercising the kfree path through GlobalAlloc::dealloc
    }

    // Flush ring-buffered log entries to UART (no timer tick yet).
    observability::drain_logs();

    // --- Step 6b: KASLR + Full TTBR1 with W^X ---

    // Compute KASLR slide (logged but not applied — slide=0 for M8).
    // The slide is computed for verification; full KASLR address transition
    // requires rebuilding TTBR1 with both old and new mappings active.
    let _kaslr = crate::mm::kaslr::compute_slide(&boot_info.rng_seed);

    // Build full kernel address space: W^X sections, direct map, MMIO.
    // Replaces boot.S minimal TTBR1 (2MB RWX blocks) with 4KB W^X pages.
    // Derive RAM range from the UEFI memory map walk (no hard-coded values).
    let (ram_start, ram_end) = crate::mm::init::phys_ram_range();
    let ram_size = ram_end - ram_start;

    // SAFETY: Called once from boot CPU after pool init. TTBR0 identity map
    // and boot.S TTBR1 are both active. The switch preserves kernel virtual
    // addresses (slide=0), so execution continues transparently.
    unsafe {
        crate::mm::kmap::init_kernel_address_space(ram_start, ram_size);
    };

    // Enable direct-map mode for the buddy allocator. After this point,
    // physical memory accesses go through TTBR1 direct map instead of
    // the TTBR0 identity map (which may be switched away for user spaces).
    crate::mm::buddy::enable_direct_map();

    observability::drain_logs();

    // --- Step 7: SMP Secondary Core Bringup ---
    let _sched = smp::bring_secondaries_online(&dt, ic.gicr_base());
    advance_boot_phase(EarlyBootPhase::ProcessManagerReady);

    // --- Step 8: Framebuffer and First Pixels ---
    if let Some(mut fb) = framebuffer::Framebuffer::from_boot_info(boot_info) {
        kinfo!(
            Boot,
            "Framebuffer: {}x{} stride={}B format={} at {:#x}",
            fb.width(),
            fb.height(),
            fb.stride(),
            fb.format(),
            fb.base_addr()
        );
        fb.render_test_pattern();
        kinfo!(Boot, "Test pattern rendered");
    } else {
        kinfo!(Boot, "No framebuffer available — skipping display");
    }

    // --- Step 10: Per-agent address spaces ---
    // Test TTBR0 switching after all boot steps that need the identity map.
    // Switch UART to TTBR1 MMIO mapping before TTBR0 is repurposed.
    crate::arch::aarch64::uart::update_base(
        crate::arch::aarch64::mmu::MMIO_BASE + crate::arch::aarch64::uart::UART_PHYS,
    );

    {
        use crate::mm::uspace;

        // SAFETY: Frame allocator and TTBR1 direct map are initialized.
        let mut as_a = unsafe { uspace::create_user_address_space("A") };
        let mut as_b = unsafe { uspace::create_user_address_space("B") };

        // Map a test page into each at USER_DATA_BASE
        let test_pa_a = crate::mm::frame::alloc_page().expect("test page A");
        let test_pa_b = crate::mm::frame::alloc_page().expect("test page B");
        // SAFETY: Frame allocator pages are valid physical addresses.
        // TTBR1 direct map is active for page table construction.
        unsafe {
            uspace::map_user_page(
                &mut as_a,
                uspace::USER_DATA_BASE,
                test_pa_a,
                crate::mm::pgtable::VmFlags::READ
                    | crate::mm::pgtable::VmFlags::WRITE
                    | crate::mm::pgtable::VmFlags::USER,
            );
            uspace::map_user_page(
                &mut as_b,
                uspace::USER_DATA_BASE,
                test_pa_b,
                crate::mm::pgtable::VmFlags::READ
                    | crate::mm::pgtable::VmFlags::WRITE
                    | crate::mm::pgtable::VmFlags::USER,
            );
        }

        // Switch between address spaces (verifies TTBR0 programming doesn't fault)
        // SAFETY: Both address spaces have valid PGDs with mapped pages.
        unsafe {
            kinfo!(Mm, "TTBR0 switch: ASID 0 -> ASID {}", as_a.asid().value);
            uspace::switch_address_space(&as_a);
            kinfo!(
                Mm,
                "TTBR0 switch: ASID {} -> ASID {}",
                as_a.asid().value,
                as_b.asid().value
            );
            uspace::switch_address_space(&as_b);
        }
        kinfo!(Mm, "Address space switching verified");
    }

    // Final drain before entering idle loop (timer tick takes over in Step 4).
    observability::drain_logs();

    // --- Step 5: Scheduler Init ---
    // Create idle + test threads but do NOT release secondary cores yet.
    // Secondary cores are parked in enter_scheduler() waiting on SCHED_READY.
    sched::init();
    observability::drain_logs();

    // --- Step 6: IPC Init ---
    // Must run while secondary cores are still parked — ipc::init() allocates
    // threads and processes. If secondary cores were scheduling, they'd starve
    // the boot CPU's THREAD_TABLE access (spin::Mutex has no fairness).
    ipc::init();
    observability::drain_logs();

    // --- Step 7a: Service Manager Init ---
    service::init();
    observability::drain_logs();

    // --- Step 7b: Storage Init ---
    if drivers::virtio_blk::init(&dt) {
        // Write/read test: sector 1000.
        let mut test_buf = [0u8; 512];
        for (i, b) in test_buf.iter_mut().enumerate() {
            *b = (i & 0xFF) as u8;
        }
        if drivers::virtio_blk::write_sector(1000, &test_buf).is_ok() {
            let mut read_buf = [0u8; 512];
            if drivers::virtio_blk::read_sector(1000, &mut read_buf).is_ok() {
                if read_buf == test_buf {
                    kinfo!(Storage, "VirtIO-blk: write/read test sector 1000 — OK");
                } else {
                    kerror!(Storage, "VirtIO-blk: read data mismatch!");
                }
            }
        }
    }
    observability::drain_logs();

    // --- Step 7c: Benchmark Init ---
    bench::init();
    observability::drain_logs();

    // --- Step 7d: Release secondary cores ---
    sched::start();
    observability::drain_logs();

    kinfo!(Boot, "Boot sequence complete, entering scheduler");
    observability::drain_logs();

    // Unmask IRQ (DAIF.I) — timer interrupts now fire every 1ms.
    // Must be after ALL boot initialization is complete.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit only. All interrupt
    // infrastructure (GIC, timer, vector table, handlers) is initialized.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    // Enter the scheduler — picks the first thread and never returns.
    sched::enter_scheduler();
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
