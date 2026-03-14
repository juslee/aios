//! Memory subsystem initialization — bootstraps from UEFI memory map.
//!
//! `init_memory()` walks the BootInfo memory map, computes pool sizes,
//! initializes per-pool buddy allocators, and stores the global FrameAllocator.
//!
//! Per memory.md §2.1 (Bootstrap) and boot-firmware.md §2.2 (BootInfo).

use core::sync::atomic::{AtomicUsize, Ordering};

use shared::{BootInfo, MemoryDescriptor, PoolConfig};

use super::buddy::PAGE_SIZE;
use super::frame::{FrameAllocator, FRAME_ALLOC};
use super::pools::PagePools;

use crate::arch::aarch64::mmu;

/// Discovered physical RAM extent (set by `init_memory()`).
/// Used by `kmap::init_kernel_address_space()` to build the direct map.
static PHYS_RAM_START: AtomicUsize = AtomicUsize::new(0);
static PHYS_RAM_END: AtomicUsize = AtomicUsize::new(0);

/// Returns the discovered physical RAM range `(start, end)`.
/// Only valid after `init_memory()` has been called.
pub fn phys_ram_range() -> (usize, usize) {
    (
        PHYS_RAM_START.load(Ordering::Relaxed),
        PHYS_RAM_END.load(Ordering::Relaxed),
    )
}

/// Initialize the physical memory subsystem from the UEFI memory map.
///
/// 1. Walks the memory map to find the overall usable physical range.
/// 2. Computes pool sizes via `PoolConfig::from_total_ram()`.
/// 3. Initializes `PagePools` (per-pool buddy allocators).
/// 4. Stores the global `FrameAllocator`.
///
/// # Safety
/// - `boot_info` must point to a valid, identity-mapped BootInfo struct.
/// - The identity map must be active (MMU enabled).
/// - Must be called exactly once, from the boot CPU.
pub unsafe fn init_memory(boot_info: &BootInfo) {
    // Step 1: Walk memory map to find overall physical extent and total usable bytes.
    let map_base = boot_info.memory_map_addr as *const u8;
    let map_count = boot_info.memory_map_count;
    let entry_size = boot_info.memory_map_entry_size;

    let mut phys_min: usize = usize::MAX;
    let mut phys_max: usize = 0;
    let mut total_usable_bytes: usize = 0;

    for i in 0..map_count {
        let ptr = map_base.add(i as usize * entry_size as usize);
        // SAFETY: The UEFI stub stores valid MemoryDescriptors. We read each
        // descriptor before any writes to the same memory.
        let desc = &*(ptr as *const MemoryDescriptor);

        // Only use memory types reclaimable after ExitBootServices.
        let usable = matches!(
            desc.ty,
            1 | 2 | 3 | 4 | 7 // LoaderCode, LoaderData, BSCode, BSData, Conventional
        );
        if !usable {
            continue;
        }

        let start = desc.phys_start as usize;
        let end = start + (desc.page_count as usize) * PAGE_SIZE;

        if start < phys_min {
            phys_min = start;
        }
        if end > phys_max {
            phys_max = end;
        }
        total_usable_bytes += end - start;
    }

    // Validate: we must have found at least some usable memory.
    assert!(
        total_usable_bytes > 0 && phys_min < phys_max,
        "[mm] FATAL: no usable physical memory found in UEFI memory map"
    );

    // Page-align the extent.
    phys_min = (phys_min + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let phys_max = phys_max & !(PAGE_SIZE - 1);

    // Validate contiguity: on QEMU virt, usable descriptors tile contiguously.
    // If there are significant gaps (>1% of total), the linear partitioning
    // approach would treat gaps as allocatable RAM. Warn and use the smaller
    // value to avoid over-committing.
    let extent = phys_max - phys_min;
    let gap = extent.saturating_sub(total_usable_bytes);
    if gap > extent / 100 {
        crate::kwarn!(
            Mm,
            "{}% gap in physical range ({:#x}..{:#x}, extent={} MB, usable={} MB)",
            gap * 100 / extent,
            phys_min,
            phys_max,
            extent / (1024 * 1024),
            total_usable_bytes / (1024 * 1024)
        );
    }

    // Step 2: Compute pool sizes.
    let config = PoolConfig::from_total_ram(total_usable_bytes);
    let total_pages = total_usable_bytes / PAGE_SIZE;

    // Step 3: Compute exclusion ranges (kernel image + memory map buffer).
    //
    // With virtual linking, linker symbols yield virtual addresses. Convert
    // __kernel_end to physical via the virt-phys offset so the exclusion
    // range stays in the physical address space used by the buddy allocator.
    extern "C" {
        static __kernel_end: u8;
    }
    let kernel_start = boot_info.kernel_phys_base as usize;
    let kernel_end_linker_virt = &__kernel_end as *const u8 as usize;
    let kernel_end_linker = kernel_end_linker_virt.wrapping_sub(mmu::VIRT_PHYS_OFFSET as usize);
    let kernel_end = kernel_end_linker.max(kernel_start + boot_info.kernel_size as usize);
    let kernel_end = (kernel_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let map_buf_start = boot_info.memory_map_addr as usize;
    let map_buf_end = map_buf_start + map_count as usize * entry_size as usize;
    let map_buf_end = (map_buf_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Step 4: Initialize page pools from the contiguous physical range.
    let pools = PagePools::init(
        phys_min,
        phys_max,
        &config,
        (kernel_start, kernel_end),
        (map_buf_start, map_buf_end),
    );

    // Export discovered RAM extent for kmap direct map.
    PHYS_RAM_START.store(phys_min, Ordering::Relaxed);
    PHYS_RAM_END.store(phys_max, Ordering::Relaxed);

    // Step 5: Create and store global FrameAllocator.
    let fa = FrameAllocator::new(pools, total_pages);
    fa.print_stats();

    *FRAME_ALLOC.lock() = Some(fa);
}
