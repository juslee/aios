//! Per-agent user address spaces and TTBR0 switching.
//!
//! Each agent gets its own L0 page table (PGD) and ASID. Switching
//! between agents writes TTBR0_EL1 with the new PGD physical address
//! and ASID. Guard pages are implicit — zeroed PGD entries cause
//! synchronous aborts on unmapped VA access.
//!
//! Page table pages are accessed via TTBR1 direct map (DIRECT_MAP_BASE + phys)
//! since TTBR0 is switched between user address spaces.
//!
//! Per memory.md §5.1, §9.5.

use core::ptr;
use core::sync::atomic::{AtomicU16, Ordering};

use spin::Mutex;

use crate::arch::aarch64::mmu;
use crate::mm::{frame, pgtable::*, tlb};

use super::asid::{Asid, AsidAllocator};

const PAGE_SIZE: usize = 4096;

// ── User VA layout (per memory.md §9.5) ──────────────────────────────

/// User code segment base address.
#[allow(dead_code)]
pub const USER_TEXT_BASE: usize = 0x0000_0000_0040_0000;

/// User data segment base address.
#[allow(dead_code)]
pub const USER_DATA_BASE: usize = 0x0000_0000_0100_0000;

/// User heap base address.
#[allow(dead_code)]
pub const USER_HEAP_BASE: usize = 0x0000_0000_1000_0000;

/// Top of the user stack (grows downward).
#[allow(dead_code)]
pub const USER_STACK_TOP: usize = 0x0000_7FFF_FFFF_F000;

/// Base of the user stack region.
#[allow(dead_code)]
pub const USER_STACK_BASE: usize = 0x0000_7FFF_FFC0_0000;

// ── Global ASID allocator ────────────────────────────────────────────

static ASID_ALLOC: Mutex<AsidAllocator> = Mutex::new(AsidAllocator::new());

/// Current ASID loaded into TTBR0 (for diagnostic printing).
static CURRENT_ASID: AtomicU16 = AtomicU16::new(0);

// ── Memory accounting ────────────────────────────────────────────────

/// Basic memory statistics for an address space.
pub struct MemoryStats {
    pub pages_allocated: usize,
    pub peak_pages: usize,
}

impl MemoryStats {
    const fn new() -> Self {
        Self {
            pages_allocated: 0,
            peak_pages: 0,
        }
    }

    fn track_alloc(&mut self) {
        self.pages_allocated += 1;
        if self.pages_allocated > self.peak_pages {
            self.peak_pages = self.pages_allocated;
        }
    }
}

// ── UserAddressSpace ─────────────────────────────────────────────────

/// A per-agent address space with its own L0 page table and ASID.
pub struct UserAddressSpace {
    /// Physical address of the L0 (PGD) page table.
    pgd_phys: usize,
    /// ASID assigned to this address space.
    asid: Asid,
    /// Memory accounting.
    stats: MemoryStats,
}

impl UserAddressSpace {
    /// Read the ASID for this address space.
    #[allow(dead_code)]
    pub fn asid(&self) -> Asid {
        self.asid
    }

    /// Read the PGD physical address.
    #[allow(dead_code)]
    pub fn pgd_phys(&self) -> usize {
        self.pgd_phys
    }

    /// Read memory statistics.
    #[allow(dead_code)]
    pub fn stats(&self) -> &MemoryStats {
        &self.stats
    }
}

// ── Direct-map page table helpers ────────────────────────────────────
//
// These mirror kmap.rs helpers but access page table pages via the
// TTBR1 direct map instead of the TTBR0 identity map. This is required
// because TTBR0 will be switched between user address spaces.

/// Convert a physical address to a direct-map virtual address.
#[inline]
fn phys_to_dmap(pa: usize) -> usize {
    mmu::DIRECT_MAP_BASE + pa
}

/// Ensure a table descriptor exists at `table_phys[index]`.
///
/// Accesses page table entries via the TTBR1 direct map.
///
/// # Safety
/// `table_phys` must be a valid, page-aligned physical address of a page table.
/// TTBR1 direct map must be active.
unsafe fn ensure_table_dmap(table_phys: usize, index: usize) -> usize {
    debug_assert!(index < 512, "page table index out of bounds: {}", index);
    let entry_va = phys_to_dmap(table_phys) + index * 8;
    // SAFETY: entry_va is in the direct map region, which maps all RAM via
    // TTBR1. table_phys is a valid page table; index < 512 keeps us in-page.
    let raw = ptr::read(entry_va as *const u64);

    if raw != 0 {
        assert!(
            raw & (PageTableEntry::VALID | PageTableEntry::TABLE)
                == (PageTableEntry::VALID | PageTableEntry::TABLE),
            "uspace: non-table entry at index {} (raw={:#x})",
            index,
            raw
        );
        (raw & PageTableEntry::PHYS_MASK) as usize
    } else {
        let new_page = frame::alloc_page().expect("uspace: OOM allocating page table");
        // SAFETY: new_page is a freshly allocated physical page. We zero it
        // via the direct map to initialize all 512 PTEs as invalid.
        let new_page_va = phys_to_dmap(new_page);
        ptr::write_bytes(new_page_va as *mut u8, 0, PAGE_SIZE);
        let desc = PageTableEntry::new_table(new_page);
        // SAFETY: entry_va points to a valid PTE slot (same as read above).
        ptr::write(entry_va as *mut u64, desc.raw());
        new_page
    }
}

/// Map a single 4KB page: `va` -> `pa` with given attributes.
///
/// Walks L0->L1->L2->L3 via the direct map, allocating intermediate
/// tables as needed.
///
/// # Safety
/// TTBR1 direct map must be active. `pgd_phys` must be a valid L0 table.
unsafe fn map_page_dmap(pgd_phys: usize, va: usize, pa: usize, mair_idx: u64, flags: VmFlags) {
    let l1 = ensure_table_dmap(pgd_phys, l0_index(va));
    let l2 = ensure_table_dmap(l1, l1_index(va));
    let l3 = ensure_table_dmap(l2, l2_index(va));

    let pte = PageTableEntry::new_page(pa, mair_idx, flags);
    let slot_va = phys_to_dmap(l3) + l3_index(va) * 8;
    // SAFETY: l3 is a valid L3 table from ensure_table_dmap; slot is within page.
    ptr::write(slot_va as *mut u64, pte.raw());
}

// ── Public API ───────────────────────────────────────────────────────

/// Create a new user address space with an empty L0 page table.
///
/// Allocates a PGD page from the kernel pool and assigns a unique ASID.
/// Performs a full TLB flush if the ASID generation wrapped.
///
/// # Safety
/// Frame allocator and TTBR1 direct map must be initialized.
pub unsafe fn create_user_address_space(label: &str) -> UserAddressSpace {
    // Allocate and zero PGD
    let pgd_phys = frame::alloc_page().expect("uspace: OOM allocating PGD");
    // SAFETY: pgd_phys is a freshly allocated page. Zero via direct map.
    let pgd_va = phys_to_dmap(pgd_phys);
    ptr::write_bytes(pgd_va as *mut u8, 0, PAGE_SIZE);

    // Allocate ASID
    let (asid, needs_flush) = ASID_ALLOC.lock().alloc();
    if needs_flush {
        tlb::tlbi_all();
    }

    crate::println!("[mm] Address space {} created (ASID={})", label, asid.value);

    UserAddressSpace {
        pgd_phys,
        asid,
        stats: MemoryStats::new(),
    }
}

/// Map a single user page in the given address space.
///
/// `va` and `pa` should be page-aligned. `flags` must include `VmFlags::USER`.
///
/// # Safety
/// TTBR1 direct map must be active. `as_` must have a valid PGD.
pub unsafe fn map_user_page(as_: &mut UserAddressSpace, va: usize, pa: usize, flags: VmFlags) {
    assert!(
        flags.contains(VmFlags::USER),
        "map_user_page: flags must include USER"
    );
    assert!(
        va < 0x0001_0000_0000_0000,
        "map_user_page: VA {:#x} is outside user address range",
        va
    );
    assert!(
        va & (PAGE_SIZE - 1) == 0,
        "map_user_page: VA {:#x} is not page-aligned",
        va
    );
    assert!(
        pa & (PAGE_SIZE - 1) == 0,
        "map_user_page: PA {:#x} is not page-aligned",
        pa
    );
    map_page_dmap(as_.pgd_phys, va, pa, mmu::MAIR_NORMAL_WB_IDX, flags);
    as_.stats.track_alloc();
}

/// Switch the active user address space by writing TTBR0_EL1.
///
/// Encodes the ASID in bits [63:48] and PGD physical address in bits [47:0].
///
/// # Safety
/// `target` must have a valid, initialized PGD.
pub unsafe fn switch_address_space(target: &UserAddressSpace) {
    let ttbr0_val = ((target.asid.value as u64) << 48) | (target.pgd_phys as u64);

    // SAFETY: Writing TTBR0_EL1 switches user-space page tables.
    // Caller must ensure no preemption/interrupts during the switch.
    // The TTBR0 write requires barriers:
    // - DSB SY ensures all prior memory operations complete
    // - MSR writes the new TTBR0 value
    // - TLBI VMALLE1IS (broadcast) invalidates stale TLB entries on all cores
    // - DSB ISH ensures TLBI completion across the inner shareable domain
    // - ISB synchronizes instruction fetch with new translations
    core::arch::asm!(
        "dsb sy",
        "msr TTBR0_EL1, {val}",
        "tlbi vmalle1is",
        "dsb ish",
        "isb",
        val = in(reg) ttbr0_val,
    );

    CURRENT_ASID.store(target.asid.value, Ordering::Relaxed);
}
