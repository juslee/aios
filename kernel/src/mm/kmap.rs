//! Full kernel address space — replaces boot.S minimal TTBR1.
//!
//! Builds a 4-level page table with:
//! - W^X kernel sections: .text=RX, .rodata=RO, .data/.bss=RW (4KB pages)
//! - Physical memory direct map at DIRECT_MAP_BASE (2MB blocks)
//! - MMIO mapping at MMIO_BASE (2MB blocks, device memory)
//!
//! Called from kernel_main after pool initialization.
//! Per memory.md §3.

use core::ptr;

use crate::arch::aarch64::mmu;
use crate::mm::{frame, pgtable::*};

const PAGE_SIZE: usize = 4096;
const BLOCK_2M: usize = 2 * 1024 * 1024;

// Linker-defined section boundaries (virtual addresses with virtual linking).
extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
}

/// Convert a kernel virtual address to its physical address.
#[inline]
fn virt_to_phys(va: usize) -> usize {
    mmu::virt_to_phys(va as u64) as usize
}

// ── Page table walk helpers ─────────────────────────────────────────────

/// Ensure a table descriptor exists at `table_phys[index]`.
///
/// If the entry is empty, allocates a new zeroed page from Pool::Kernel
/// and installs a table descriptor. If the entry is already a valid table
/// descriptor, returns the next-level table's physical address.
///
/// Panics if the entry is a block descriptor (unexpected in the walk path).
///
/// # Safety
/// `table_phys` must be a valid, page-aligned physical address of a page table.
/// TTBR0 identity map must be active for physical address access.
unsafe fn ensure_table(table_phys: usize, index: usize) -> usize {
    let entry_ptr = (table_phys + index * 8) as *mut u64;
    // SAFETY: table_phys is a valid page-aligned physical address accessible
    // via TTBR0 identity map. index < 512, so the offset is within the 4KB page.
    let raw = ptr::read(entry_ptr);

    if raw != 0 {
        // Entry exists — must be a table descriptor, not a block
        assert!(
            raw & (PageTableEntry::VALID | PageTableEntry::TABLE)
                == (PageTableEntry::VALID | PageTableEntry::TABLE),
            "kmap: non-table entry at index {} (raw={:#x})",
            index,
            raw
        );
        (raw & PageTableEntry::PHYS_MASK) as usize
    } else {
        // Allocate and zero a new page table page
        let new_page = frame::alloc_page().expect("kmap: OOM allocating page table");
        // SAFETY: new_page is a freshly allocated physical page from Pool::Kernel,
        // accessible via TTBR0 identity map. Zeroing 4KB initializes all 512 PTEs
        // to invalid (0).
        ptr::write_bytes(new_page as *mut u8, 0, PAGE_SIZE);
        let desc = PageTableEntry::new_table(new_page);
        // SAFETY: entry_ptr points to a valid PTE slot (see read above).
        ptr::write(entry_ptr, desc.raw());
        new_page
    }
}

/// Map a single 4KB page: `va` → `pa` with given attributes.
///
/// Walks L0→L1→L2→L3, allocating intermediate tables as needed.
///
/// # Safety
/// TTBR0 identity map must be active. `pgd` must be a valid L0 table.
unsafe fn map_page(pgd: usize, va: usize, pa: usize, mair_idx: u64, flags: VmFlags) {
    let l1 = ensure_table(pgd, l0_index(va));
    let l2 = ensure_table(l1, l1_index(va));
    let l3 = ensure_table(l2, l2_index(va));

    let pte = PageTableEntry::new_page(pa, mair_idx, flags);
    let slot = (l3 + l3_index(va) * 8) as *mut u64;
    // SAFETY: l3 is a valid L3 table from ensure_table; slot is within the page.
    ptr::write(slot, pte.raw());
}

/// Map a single 2MB block: `va` → `pa` with given attributes.
///
/// Walks L0→L1→L2 and installs a block descriptor at L2.
///
/// # Safety
/// TTBR0 identity map must be active. `pgd` must be a valid L0 table.
/// Both `va` and `pa` must be 2MB-aligned.
unsafe fn map_block_2m(pgd: usize, va: usize, pa: usize, mair_idx: u64, flags: VmFlags) {
    assert!(
        va & (BLOCK_2M - 1) == 0,
        "map_block_2m: VA {:#x} not 2MB-aligned",
        va
    );
    let l1 = ensure_table(pgd, l0_index(va));
    let l2 = ensure_table(l1, l1_index(va));

    let pte = PageTableEntry::new_block_2m(pa, mair_idx, flags);
    let slot = (l2 + l2_index(va) * 8) as *mut u64;
    // SAFETY: l2 is a valid L2 table from ensure_table; slot is within the page.
    ptr::write(slot, pte.raw());
}

/// Map a range of 4KB pages: `va_start..va_end` → `pa_start..`.
///
/// # Safety
/// TTBR0 identity map must be active. `pgd` must be a valid L0 table.
unsafe fn map_range_4k(
    pgd: usize,
    va_start: usize,
    va_end: usize,
    pa_start: usize,
    mair_idx: u64,
    flags: VmFlags,
) {
    let mut va = va_start & !0xFFF;
    let mut pa = pa_start & !0xFFF;
    let end = (va_end + 0xFFF) & !0xFFF; // round up to page boundary
    while va < end {
        map_page(pgd, va, pa, mair_idx, flags);
        va += PAGE_SIZE;
        pa += PAGE_SIZE;
    }
}

/// Map a range of 2MB blocks: `va_start..va_end` → `pa_start..`.
///
/// Both `va_start` and `pa_start` must be 2MB-aligned.
///
/// # Safety
/// TTBR0 identity map must be active. `pgd` must be a valid L0 table.
unsafe fn map_range_2m(
    pgd: usize,
    va_start: usize,
    va_end: usize,
    pa_start: usize,
    mair_idx: u64,
    flags: VmFlags,
) {
    assert!(
        va_start & (BLOCK_2M - 1) == 0,
        "map_range_2m: va_start {:#x} not 2MB-aligned",
        va_start
    );
    assert!(
        pa_start & (BLOCK_2M - 1) == 0,
        "map_range_2m: pa_start {:#x} not 2MB-aligned",
        pa_start
    );
    let mut va = va_start;
    let mut pa = pa_start;
    let end = (va_end + BLOCK_2M - 1) & !(BLOCK_2M - 1); // round up
    while va < end {
        map_block_2m(pgd, va, pa, mair_idx, flags);
        va += BLOCK_2M;
        pa += BLOCK_2M;
    }
}

// ── Public API ──────────────────────────────────────────────────────────

/// Build and activate a full kernel address space with W^X enforcement.
///
/// Replaces the minimal boot.S TTBR1 (2MB RWX blocks) with fine-grained
/// 4KB page mappings:
/// - `.text` → RX (read + execute)
/// - `.rodata` → RO (read only)
/// - `.data/.bss/stack` → RW (read + write)
/// - Direct map at `DIRECT_MAP_BASE` → RW (2MB blocks, all RAM)
/// - MMIO at `MMIO_BASE` → RW device memory (2MB blocks)
///
/// `ram_start` and `ram_size` define the physical RAM range to direct-map.
/// Both must be 2MB-aligned.
///
/// # Safety
/// Must be called exactly once from the boot CPU after pool initialization.
/// TTBR0 identity map and boot.S TTBR1 must both be active.
pub unsafe fn init_kernel_address_space(ram_start: usize, ram_size: usize) {
    // Allocate PGD (L0 page table) from kernel pool
    let pgd = frame::alloc_page().expect("kmap: cannot allocate PGD");
    // SAFETY: pgd is a freshly allocated page from Pool::Kernel, accessible
    // via TTBR0 identity map. Zeroing initializes all 512 entries to invalid.
    ptr::write_bytes(pgd as *mut u8, 0, PAGE_SIZE);

    // ── Kernel sections with W^X ────────────────────────────────────

    // Section boundaries are virtual addresses (from linker symbols).
    // Convert to physical for the PA side of the mapping.
    let text_start_va = &__text_start as *const u8 as usize;
    let text_end_va = &__text_end as *const u8 as usize;
    let rodata_start_va = &__rodata_start as *const u8 as usize;
    let rodata_end_va = &__rodata_end as *const u8 as usize;
    let data_start_va = &__data_start as *const u8 as usize;
    let data_end_va = &__data_end as *const u8 as usize;

    let text_pa = virt_to_phys(text_start_va);
    let rodata_pa = virt_to_phys(rodata_start_va);
    let data_pa = virt_to_phys(data_start_va);

    let text_pages = (text_end_va - text_start_va).div_ceil(PAGE_SIZE);
    let rodata_pages = (rodata_end_va - rodata_start_va).div_ceil(PAGE_SIZE);
    let data_pages = (data_end_va - data_start_va).div_ceil(PAGE_SIZE);

    // .text: RX — kernel code, executable, not writable
    map_range_4k(
        pgd,
        text_start_va,
        text_end_va,
        text_pa,
        mmu::MAIR_NORMAL_WB_IDX,
        VmFlags::READ | VmFlags::EXECUTE,
    );

    // .rodata: RO — read-only data, not writable, not executable
    map_range_4k(
        pgd,
        rodata_start_va,
        rodata_end_va,
        rodata_pa,
        mmu::MAIR_NORMAL_WB_IDX,
        VmFlags::READ,
    );

    // .data + .bss + stack: RW — writable data, not executable
    map_range_4k(
        pgd,
        data_start_va,
        data_end_va,
        data_pa,
        mmu::MAIR_NORMAL_WB_IDX,
        VmFlags::READ | VmFlags::WRITE,
    );

    // ── Direct map: physical RAM at DIRECT_MAP_BASE ─────────────────
    //
    // VA = DIRECT_MAP_BASE + physical_address
    // Enables phys_to_virt/virt_to_phys conversion after TTBR0 is retired.
    let dm_va_start = mmu::DIRECT_MAP_BASE + ram_start;
    let dm_va_end = mmu::DIRECT_MAP_BASE + ram_start + ram_size;
    map_range_2m(
        pgd,
        dm_va_start,
        dm_va_end,
        ram_start,
        mmu::MAIR_NORMAL_WB_IDX,
        VmFlags::READ | VmFlags::WRITE,
    );

    // ── MMIO: device memory at MMIO_BASE ────────────────────────────
    //
    // Maps PA 0x0000_0000..0x4000_0000 (1 GB device region) with device
    // memory attributes. Covers UART (0x0900_0000) and GIC (0x0800_0000).
    map_range_2m(
        pgd,
        mmu::MMIO_BASE,
        mmu::MMIO_BASE + 0x4000_0000,
        0,
        mmu::MAIR_DEVICE_IDX,
        VmFlags::READ | VmFlags::WRITE,
    );

    // ── Switch TTBR1 to the full page tables ────────────────────────
    //
    // SAFETY: DSB ensures all page table writes are visible to the table
    // walker before the TTBR1 swap. ISB ensures the new TTBR1 is used
    // for all subsequent instruction fetches. TLBI invalidates stale TLB
    // entries from the boot.S minimal mapping.
    //
    // The kernel virtual addresses are unchanged (same as boot.S mapping),
    // so the switch is transparent to running code. Only the permissions
    // change (2MB RWX blocks → 4KB W^X pages).
    core::arch::asm!("dsb sy");
    core::arch::asm!(
        "msr TTBR1_EL1, {pgd}",
        "isb",
        pgd = in(reg) pgd as u64,
    );
    // SAFETY: TLBI VMALLE1IS broadcasts TLB invalidation to all PEs in the
    // Inner Shareable domain. DSB ISH ensures completion before proceeding.
    // Secondary cores are not yet online (SMP bringup is step 7), so this
    // is safe even though it broadcasts.
    core::arch::asm!("tlbi vmalle1is", "dsb ish", "isb",);

    crate::kinfo!(Mm, "TTBR1 active — kernel mapped with W^X");
    crate::kinfo!(
        Mm,
        "W^X: text=RX ({} pages), rodata=RO ({} pages), data=RW ({} pages)",
        text_pages,
        rodata_pages,
        data_pages
    );
    crate::kinfo!(
        Mm,
        "Direct map: {:#x}..{:#x} ({} MB)",
        dm_va_start,
        dm_va_end,
        ram_size / (1024 * 1024)
    );
    crate::kinfo!(
        Mm,
        "MMIO: {:#x}..{:#x}",
        mmu::MMIO_BASE,
        mmu::MMIO_BASE + 0x4000_0000_usize
    );
}
