//! MMU and page table management for aarch64.
//!
//! Builds kernel page tables and swaps TTBR0 to our identity map.
//! Phase 1 uses edk2's existing MAIR/TCR configuration with 1 GB block
//! descriptors. Per memory.md §3.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Boot CPU MMU register state (for secondary core MMU enable) ─────
// Saved during init_mmu() after TTBR0 swap. Secondary cores load these
// to enable their MMU with the same configuration as the boot CPU.
#[no_mangle]
static BOOT_MAIR: AtomicU64 = AtomicU64::new(0);
#[no_mangle]
static BOOT_TCR: AtomicU64 = AtomicU64::new(0);
#[no_mangle]
static BOOT_SCTLR: AtomicU64 = AtomicU64::new(0);

// Kernel virtual address space layout (memory.md §3.1)
#[allow(dead_code)]
pub const KERNEL_BASE: usize = 0xFFFF_0000_0000_0000;
pub const DIRECT_MAP_BASE: usize = 0xFFFF_0001_0000_0000;
pub const MMIO_BASE: usize = 0xFFFF_0010_0000_0000;
#[allow(dead_code)]
pub const PAGE_SIZE: usize = 4096;

// ── Page table storage ────────────────────────────────────────────────
// Two static tables: L0+L1 for TTBR0 (identity map).
// Each table is 4 KiB (512 × 8-byte entries), page-aligned.

#[repr(C, align(4096))]
struct RawPageTable {
    entries: UnsafeCell<[u64; 512]>,
}

// SAFETY: Page tables are written once during single-core boot, then read-only
// by the MMU hardware. No concurrent access during init.
unsafe impl Sync for RawPageTable {}

#[no_mangle]
static TTBR0_L0: RawPageTable = RawPageTable {
    entries: UnsafeCell::new([0; 512]),
};
static TTBR0_L1: RawPageTable = RawPageTable {
    entries: UnsafeCell::new([0; 512]),
};

// ── Descriptor helpers ────────────────────────────────────────────────

// Page table entry bits
const PTE_VALID: u64 = 1 << 0;
const PTE_TABLE: u64 = 1 << 1; // table descriptor (not block)
const PTE_AF: u64 = 1 << 10; // access flag
const PTE_SH_INNER: u64 = 0b11 << 8; // inner shareable
const PTE_PXN: u64 = 1 << 53; // privileged execute-never
const PTE_UXN: u64 = 1 << 54; // unprivileged execute-never

// MAIR attribute indices — matched to edk2's MAIR configuration:
//   edk2 MAIR = 0xffbb4400: Attr0=0x00(Device), Attr1=0x44(NC), Attr2=0xbb(WT), Attr3=0xff(WB)
// We use Attr0 for device memory and Attr3 for normal memory (write-back
// cacheable). Phase 1 originally used Attr1 (NC); upgraded to WB in Phase 2 M8
// to prevent attribute aliasing and enable spin::Mutex on SMP.
pub const MAIR_DEVICE_IDX: u64 = 0;
#[allow(dead_code)]
pub const MAIR_NORMAL_NC_IDX: u64 = 1;
pub const MAIR_NORMAL_WB_IDX: u64 = 3;

// TTBR0 identity map uses WB cacheable (Attr3) to match TTBR1 attributes.
// Phase 1 used NC (Attr1) as a conservative choice. Phase 2 M8 upgrades
// to WB to prevent attribute aliasing (CONSTRAINED UNPREDICTABLE on ARM
// when the same physical page is mapped with different cacheability).
// This also enables spin::Mutex — exclusive load/store pairs (ldaxr/stlxr)
// require Inner Shareable + Cacheable memory for the global exclusive monitor.
const MAIR_NORMAL_IDX: u64 = MAIR_NORMAL_WB_IDX;

/// Build a table descriptor pointing to the next-level table.
fn table_descriptor(next_table_phys: u64) -> u64 {
    (next_table_phys & 0x0000_FFFF_FFFF_F000) | PTE_TABLE | PTE_VALID
}

/// Build a 1 GB block descriptor at L1.
///
/// `phys_addr` must be 1 GB aligned. `mair_idx` selects device (0) or
/// normal WB (3) memory attributes. If `executable` is false, PXN+UXN are set.
fn l1_block_descriptor(phys_addr: u64, mair_idx: u64, executable: bool) -> u64 {
    let mut desc = (phys_addr & 0x0000_FFFF_C000_0000) | PTE_VALID;
    // bit 1 = 0 → block descriptor (not table)
    desc |= (mair_idx & 0x7) << 2; // AttrIndx[4:2]
    desc |= PTE_AF;
    desc |= PTE_SH_INNER;
    if !executable {
        desc |= PTE_PXN | PTE_UXN;
    }
    desc
}

// ── Public API ────────────────────────────────────────────────────────

/// Virt-to-phys offset for kernel statics (virtual linking).
///
/// With virtual linking (VMA at KERNEL_VIRT, LMA at KERNEL_PHYS), Rust
/// addresses of statics are virtual. TTBR0_EL1 and table descriptors need
/// physical addresses. Subtract this offset to convert.
///
/// = KERNEL_VIRT + (KERNEL_PHYS & 0x1FFFFF) - KERNEL_PHYS
/// = 0xFFFF_0000_0008_0000 - 0x4008_0000
/// = 0xFFFE_FFFF_C000_0000
///
/// Canonical source — imported by kmap.rs, mm/init.rs, smp.rs.
pub const VIRT_PHYS_OFFSET: u64 = 0xFFFE_FFFF_C000_0000;

/// Convert a kernel virtual address to physical.
#[inline]
pub fn virt_to_phys(va: u64) -> u64 {
    va.wrapping_sub(VIRT_PHYS_OFFSET)
}

/// Build identity-map page tables and swap TTBR0_EL1.
///
/// Phase 1 strategy: edk2 leaves MMU on with its own MAIR/TCR. Changing
/// these registers while MMU is on is CONSTRAINED UNPREDICTABLE (ARM ARM).
/// Instead, we build page tables compatible with edk2's T0SZ=20 (44-bit VA,
/// 4KB granule) and only swap TTBR0. Memory attributes use edk2's MAIR
/// (Attr0=Device, Attr3=Write-back Normal). RAM blocks use WB cacheable
/// (upgraded from NC in Phase 2 M8 to prevent attribute aliasing).
///
/// After this call, code continues executing at physical addresses via our
/// identity map. The buddy/slab allocators use physical addresses directly.
///
/// # Safety
/// Must be called exactly once from the boot CPU during early init.
/// Caller must ensure no concurrent access to page table statics.
pub unsafe fn init_mmu() {
    // With virtual linking, Rust pointer casts on statics yield virtual
    // addresses. TTBR0_EL1 and table descriptors require physical addresses
    // (the MMU table walker starts from the physical address in TTBRn).
    let l0_phys = virt_to_phys(TTBR0_L0.entries.get() as u64);
    let l1_phys = virt_to_phys(TTBR0_L1.entries.get() as u64);

    // SAFETY: UnsafeCell dereferences are safe because init_mmu is called
    // exactly once from the boot CPU with no concurrent access to these statics.
    // We access statics via their virtual addresses (valid through TTBR1),
    // but write physical addresses into the page table entries.
    let l0 = &mut *TTBR0_L0.entries.get();
    let l1 = &mut *TTBR0_L1.entries.get();

    l0[0] = table_descriptor(l1_phys);

    // Block 0: 0x0000_0000 – device memory (UART, GIC, etc.)
    l1[0] = l1_block_descriptor(0x0000_0000, MAIR_DEVICE_IDX, false);
    // Block 1: 0x4000_0000 – RAM (kernel code + data, must be executable)
    // Phase 1 limitation: 1 GB block is RWX (no AP bits = EL1 RW, executable=true).
    // W^X enforcement at 2 MiB/4 KiB granularity (text=RX, data=RW+XN) is Phase 2.
    l1[1] = l1_block_descriptor(0x4000_0000, MAIR_NORMAL_IDX, true);
    // Block 2: 0x8000_0000 – RAM (rest of 2 GB)
    l1[2] = l1_block_descriptor(0x8000_0000, MAIR_NORMAL_IDX, false);

    // SAFETY: DSB SY ensures all page table writes reach Point of Coherency.
    core::arch::asm!("dsb sy");

    // Force page table pages out of cache to physical memory.
    // Some QEMU versions may have walker coherency quirks.
    // SAFETY: DC CIVAC cleans+invalidates cache lines by VA. l0/l1 are
    // valid virtual addresses of our page table statics.
    let l0_va = l0 as *const [u64; 512] as usize;
    let l1_va = l1 as *const [u64; 512] as usize;
    core::arch::asm!(
        "dc civac, {l0}",
        "dc civac, {l1}",
        "dsb sy",
        l0 = in(reg) l0_va,
        l1 = in(reg) l1_va,
    );

    // Swap TTBR0 to our identity map page tables.
    // edk2's T0SZ=20 (44-bit VA, L0 start): our L0[0]→L1 with 1GB blocks
    // is compatible — L0 index for all our addresses (0x00–0xBF_FFFF_FFFF) is 0.
    // SAFETY: l0_phys is the physical address of a valid, page-aligned L0 page
    // table built above. Writing TTBR0_EL1 at EL1 is architecturally permitted.
    let ttbr0 = l0_phys;
    core::arch::asm!(
        "msr TTBR0_EL1, {ttbr0}",
        "isb",
        ttbr0 = in(reg) ttbr0,
    );

    // SAFETY: TLBI + DSB ISH invalidate all TLB entries in the Inner Shareable
    // domain. DSB ISH is safe (does not hang with parked cores on QEMU 10.x).
    core::arch::asm!("tlbi vmalle1is", "dsb ish", "isb",);

    // Save boot CPU's MMU register state for secondary cores.
    // These registers are read-only from here on; secondary cores load them
    // in boot.S _secondary_entry to enable MMU with the same configuration.
    let mair: u64;
    let tcr: u64;
    let sctlr: u64;
    core::arch::asm!("mrs {}, MAIR_EL1", out(reg) mair, options(nomem, nostack, preserves_flags));
    core::arch::asm!("mrs {}, TCR_EL1", out(reg) tcr, options(nomem, nostack, preserves_flags));
    core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr, options(nomem, nostack, preserves_flags));
    BOOT_MAIR.store(mair, Ordering::Relaxed);
    BOOT_TCR.store(tcr, Ordering::Relaxed);
    BOOT_SCTLR.store(sctlr, Ordering::Relaxed);
}

/// Physical address of the L0 page table (for TTBR0_EL1 on secondary cores).
#[allow(dead_code)]
pub fn ttbr0_l0_addr() -> u64 {
    virt_to_phys(TTBR0_L0.entries.get() as u64)
}

/// Boot CPU's MMU register values: (MAIR_EL1, TCR_EL1, SCTLR_EL1).
/// Secondary cores use these to enable MMU with identical configuration.
#[allow(dead_code)]
pub fn boot_mmu_regs() -> (u64, u64, u64) {
    (
        BOOT_MAIR.load(Ordering::Relaxed),
        BOOT_TCR.load(Ordering::Relaxed),
        BOOT_SCTLR.load(Ordering::Relaxed),
    )
}
