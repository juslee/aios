//! Buddy allocator for physical memory.
//!
//! Manages physical pages in orders 0–10 (4 KiB – 4 MiB). Free blocks are
//! linked via intrusive next-pointers stored in the first 8 bytes of each
//! free page (accessible via the identity map after MMU enable).
//!
//! Phase 2 enhancements (M7):
//! - Bitmap-based coalescing: XOR buddy-pair bitmap tracks alloc/free state.
//!   One bit per pair per order. Toggle on alloc/free. bit=0 after free → coalesce.
//! - Security: double-free detection via bitmap, poison fill on free (0xDEAD_DEAD).
//!
//! Per memory.md §2.2 and fuzzing-and-hardening.md §3.3.

use core::sync::atomic::{AtomicBool, Ordering};
use shared::MemoryDescriptor;
use spin::Mutex;

pub const PAGE_SIZE: usize = 4096;
pub const MAX_ORDER: usize = 10; // 2^10 pages = 4 MiB

/// Poison pattern written to freed pages for use-after-free detection.
const POISON_PATTERN: u32 = 0xDEAD_DEAD;

/// Flag indicating the TTBR1 direct map is active. When true, physical
/// addresses must be accessed through `DIRECT_MAP_BASE + phys` rather
/// than directly (identity map may no longer exist in TTBR0).
static DIRECT_MAP_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Enable direct-map mode for physical memory access. Called after
/// `init_kernel_address_space()` installs the TTBR1 direct map.
pub fn enable_direct_map() {
    DIRECT_MAP_ACTIVE.store(true, Ordering::Release);
}

/// Convert a physical address to a pointer usable for read/write.
/// Uses the identity map (TTBR0) early in boot, switches to the
/// TTBR1 direct map after `enable_direct_map()` is called.
#[inline]
fn phys_to_ptr<T>(phys: usize) -> *mut T {
    phys_to_virt(phys) as *mut T
}

/// Convert a physical address to a virtual address.
/// Uses identity (phys == virt) early in boot, TTBR1 direct map after
/// `enable_direct_map()` is called.
#[inline]
pub fn phys_to_virt(phys: usize) -> usize {
    if DIRECT_MAP_ACTIVE.load(Ordering::Relaxed) {
        crate::arch::aarch64::mmu::DIRECT_MAP_BASE + phys
    } else {
        phys
    }
}

/// Convert a virtual address (direct-map region) back to physical.
/// Identity when direct map is not yet active.
pub fn virt_to_phys(virt: usize) -> usize {
    if DIRECT_MAP_ACTIVE.load(Ordering::Relaxed) {
        virt - crate::arch::aarch64::mmu::DIRECT_MAP_BASE
    } else {
        virt
    }
}

/// Buddy allocator for physical page frames.
///
/// Each instance manages a contiguous physical range `[base, end)`.
/// A bitmap in the first pages tracks coalescing state: one bit per buddy pair
/// per order. The bitmap is stored in the first N pages of the managed range.
#[allow(dead_code)] // New fields/methods used by pools/frame/init (Steps 3–5)
pub struct BuddyAllocator {
    /// Start of managed physical range (page-aligned).
    base: usize,
    /// End of managed physical range (page-aligned).
    end: usize,
    /// Physical address of the bitmap (stored in first pages of range).
    bitmap: usize,
    /// Number of pages consumed by the bitmap.
    bitmap_pages: usize,
    /// Head of free list per order (physical address, 0 = empty).
    free_heads: [usize; MAX_ORDER + 1],
    /// Count of free blocks per order.
    free_count: [usize; MAX_ORDER + 1],
    /// Total free pages (sum of free_count[i] * 2^i).
    total_free: usize,
    /// Whether this allocator has been initialized with a range.
    initialized: bool,
}

#[allow(dead_code)] // New methods used by pools/frame/init (M7 Steps 3–5)
impl BuddyAllocator {
    pub const fn new() -> Self {
        Self {
            base: 0,
            end: 0,
            bitmap: 0,
            bitmap_pages: 0,
            free_heads: [0; MAX_ORDER + 1],
            free_count: [0; MAX_ORDER + 1],
            total_free: 0,
            initialized: false,
        }
    }

    /// Initialize this allocator to manage `[base, end)`.
    ///
    /// Carves bitmap from the first pages, zeros it, then adds remaining pages
    /// to free lists as the largest possible aligned blocks.
    ///
    /// # Safety
    /// - `[base, end)` must be valid, page-aligned, identity-mapped RAM.
    /// - Must be called at most once per instance.
    pub unsafe fn init_with_range(&mut self, base: usize, end: usize) {
        debug_assert!(base & (PAGE_SIZE - 1) == 0);
        debug_assert!(end & (PAGE_SIZE - 1) == 0);
        debug_assert!(base < end);

        self.base = base;
        self.end = end;

        let total_pages = (end - base) / PAGE_SIZE;

        // Bitmap size: one bit per buddy pair per order.
        // At order k, we have ceil(total_pages / 2^(k+1)) pairs.
        // Use div_ceil to account for incomplete pairs at the tail.
        // Total bits ≈ total_pages (geometric series).
        // Round up to bytes, then to pages.
        let bitmap_bits: usize = (0..=MAX_ORDER)
            .map(|k| total_pages.div_ceil(1 << (k + 1)))
            .sum();
        let bitmap_bytes = bitmap_bits.div_ceil(8);
        self.bitmap_pages = bitmap_bytes.div_ceil(PAGE_SIZE);
        self.bitmap = base;

        // Zero the bitmap.
        let bitmap_area = self.bitmap as *mut u8;
        // SAFETY: bitmap is at the start of our managed range, identity-mapped.
        core::ptr::write_bytes(bitmap_area, 0, self.bitmap_pages * PAGE_SIZE);

        // Add remaining pages (after bitmap) to free lists.
        let usable_start = base + self.bitmap_pages * PAGE_SIZE;
        self.add_aligned_blocks(usable_start, end);

        self.initialized = true;
    }

    /// Initialize this allocator to manage `[base, end)`, excluding up to two
    /// reserved ranges (e.g., kernel image and memory map buffer).
    ///
    /// # Safety
    /// Same as `init_with_range`. Exclusion ranges must be page-aligned.
    pub unsafe fn init_with_range_excluding(
        &mut self,
        base: usize,
        end: usize,
        excl1: (usize, usize),
        excl2: (usize, usize),
    ) {
        debug_assert!(base & (PAGE_SIZE - 1) == 0);
        debug_assert!(end & (PAGE_SIZE - 1) == 0);
        debug_assert!(base < end);

        self.base = base;
        self.end = end;

        let total_pages = (end - base) / PAGE_SIZE;

        // Compute and zero bitmap (same as init_with_range).
        let bitmap_bits: usize = (0..=MAX_ORDER)
            .map(|k| total_pages.div_ceil(1 << (k + 1)))
            .sum();
        let bitmap_bytes = bitmap_bits.div_ceil(8);
        self.bitmap_pages = bitmap_bytes.div_ceil(PAGE_SIZE);
        self.bitmap = base;

        // SAFETY: bitmap at start of managed range, identity-mapped.
        let bitmap_area = self.bitmap as *mut u8;
        core::ptr::write_bytes(bitmap_area, 0, self.bitmap_pages * PAGE_SIZE);

        // The bitmap itself is an exclusion. Combine it with the provided exclusions.
        let bitmap_end = base + self.bitmap_pages * PAGE_SIZE;

        // Add all pages in [base, end) excluding bitmap, excl1, excl2.
        // Walk the range and skip excluded regions.
        self.add_region_excluding3(base, end, (base, bitmap_end), excl1, excl2);

        self.initialized = true;
    }

    /// Whether this allocator has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Start of managed range.
    pub fn base(&self) -> usize {
        self.base
    }

    /// End of managed range.
    pub fn end(&self) -> usize {
        self.end
    }

    // ── Bitmap operations ───────────────────────────────────────────────

    /// Compute bitmap bit index for a buddy pair at a given order.
    ///
    /// At order k, the bit index for address `addr` is:
    /// `pfn_relative >> (k + 1)` plus the offset for all lower orders.
    fn bitmap_index(&self, addr: usize, order: usize) -> (usize, u8) {
        let pfn = (addr - self.base) / PAGE_SIZE;
        let pair_index = pfn >> (order + 1);

        // Offset: sum of pair counts for orders 0..order.
        // Must use div_ceil to match bitmap sizing (accounts for incomplete pairs).
        let total_pages = (self.end - self.base) / PAGE_SIZE;
        let mut offset = 0;
        for k in 0..order {
            offset += total_pages.div_ceil(1 << (k + 1));
        }

        let bit_pos = offset + pair_index;
        let byte_idx = bit_pos / 8;
        let bit_in_byte = (bit_pos % 8) as u8;
        (byte_idx, bit_in_byte)
    }

    /// Toggle the bitmap bit for a buddy pair. Returns the new bit value.
    ///
    /// # Safety
    /// The bitmap must be initialized and the address must be within range.
    unsafe fn bitmap_toggle(&self, addr: usize, order: usize) -> bool {
        let (byte_idx, bit) = self.bitmap_index(addr, order);
        let byte_ptr = phys_to_ptr::<u8>(self.bitmap).add(byte_idx);
        // SAFETY: byte_ptr is within the bitmap area, accessible via
        // identity map (early boot) or TTBR1 direct map (post-kmap).
        let val = core::ptr::read_volatile(byte_ptr);
        let new_val = val ^ (1 << bit);
        core::ptr::write_volatile(byte_ptr, new_val);
        (new_val >> bit) & 1 == 1
    }

    /// Read the bitmap bit for a buddy pair (without toggling).
    ///
    /// # Safety
    /// The bitmap must be initialized.
    unsafe fn bitmap_read(&self, addr: usize, order: usize) -> bool {
        let (byte_idx, bit) = self.bitmap_index(addr, order);
        let byte_ptr = phys_to_ptr::<u8>(self.bitmap).add(byte_idx) as *const u8;
        // SAFETY: byte_ptr is within the bitmap area.
        let val = core::ptr::read_volatile(byte_ptr);
        (val >> bit) & 1 == 1
    }

    // ── Free list operations ────────────────────────────────────────────

    /// Add a single block of `2^order` pages starting at `phys_addr` to the free list.
    ///
    /// # Safety
    /// `phys_addr` must be page-aligned and point to usable RAM accessible via
    /// the identity map.
    unsafe fn push_block(&mut self, phys_addr: usize, order: usize) {
        let ptr = phys_to_ptr::<usize>(phys_addr);
        // SAFETY: phys_addr is accessible via identity map or TTBR1 direct map;
        // writing the next pointer into the first 8 bytes of the free block.
        core::ptr::write_volatile(ptr, self.free_heads[order]);
        self.free_heads[order] = phys_addr;
        self.free_count[order] += 1;
        self.total_free += 1 << order;
    }

    /// Pop a block from the free list at `order`.
    ///
    /// # Safety
    /// Caller must ensure the identity map is active.
    unsafe fn pop_block(&mut self, order: usize) -> Option<usize> {
        let head = self.free_heads[order];
        if head == 0 {
            return None;
        }
        // SAFETY: head is a valid address of a free block, accessible via
        // identity map or TTBR1 direct map.
        let next = core::ptr::read_volatile(phys_to_ptr::<usize>(head) as *const usize);
        self.free_heads[order] = next;
        self.free_count[order] -= 1;
        Some(head)
    }

    /// Remove a specific block from the free list at `order`.
    ///
    /// O(n) walk — required for coalescing when we need to remove the buddy
    /// (which may not be at the head of the list).
    ///
    /// # Safety
    /// `target` must be a block currently in the free list at `order`.
    unsafe fn remove_from_free_list(&mut self, target: usize, order: usize) -> bool {
        if self.free_heads[order] == target {
            // It's the head — just pop.
            self.pop_block(order);
            return true;
        }

        let mut prev = self.free_heads[order];
        while prev != 0 {
            // SAFETY: prev is a valid free block, accessible via identity map
            // or TTBR1 direct map.
            let next = core::ptr::read_volatile(phys_to_ptr::<usize>(prev) as *const usize);
            if next == target {
                // Unlink target: prev->next = target->next.
                let target_next =
                    core::ptr::read_volatile(phys_to_ptr::<usize>(target) as *const usize);
                core::ptr::write_volatile(phys_to_ptr::<usize>(prev), target_next);
                self.free_count[order] -= 1;
                return true;
            }
            prev = next;
        }
        false
    }

    // ── Core alloc / free ───────────────────────────────────────────────

    /// Allocate `2^order` contiguous pages. Returns the physical address.
    ///
    /// # Safety
    /// Identity map must be active.
    pub unsafe fn alloc_pages(&mut self, order: usize) -> Option<usize> {
        if order > MAX_ORDER {
            return None;
        }

        // Try the exact order first
        if let Some(addr) = self.pop_block(order) {
            self.total_free -= 1 << order;
            // Toggle bitmap for coalescing tracking.
            if self.initialized {
                self.bitmap_toggle(addr, order);
            }
            return Some(addr);
        }

        // Split a larger block
        for higher in (order + 1)..=MAX_ORDER {
            if let Some(addr) = self.pop_block(higher) {
                // Toggle bitmap for the higher-order allocation.
                if self.initialized {
                    self.bitmap_toggle(addr, higher);
                }
                // Split: put upper halves back as free blocks.
                for split_order in (order..higher).rev() {
                    let buddy_addr = addr + (PAGE_SIZE << split_order);
                    self.push_block(buddy_addr, split_order);
                    // Toggle bitmap for each split buddy we're freeing.
                    if self.initialized {
                        self.bitmap_toggle(buddy_addr, split_order);
                    }
                }
                // push_block added buddy pages to total_free, but we never
                // subtracted the popped higher-order block. Subtract 1<<higher
                // to account for both the pop and the allocation of 1<<order.
                self.total_free -= 1 << higher;
                return Some(addr);
            }
        }

        None // out of memory
    }

    /// Free `2^order` contiguous pages at `phys_addr`.
    ///
    /// With coalescing: toggles the bitmap, and if the buddy is also free
    /// (bit=0 after toggle), removes the buddy from its free list and merges
    /// upward. Repeats until MAX_ORDER or no merge possible.
    ///
    /// Security hardening:
    /// - Double-free detection: bitmap check before toggle.
    /// - Poison fill: freed pages filled with 0xDEAD_DEAD.
    ///
    /// # Safety
    /// `phys_addr` must have been previously returned by `alloc_pages` with
    /// the same `order`. Identity map must be active.
    pub unsafe fn free_pages(&mut self, phys_addr: usize, order: usize) {
        debug_assert!(phys_addr >= self.base && phys_addr < self.end);
        debug_assert!(phys_addr & (PAGE_SIZE - 1) == 0);

        if !self.initialized {
            // Legacy path: no coalescing.
            self.push_block(phys_addr, order);
            return;
        }

        // Security: double-free detection via poison pattern check.
        // If the first word already contains the poison pattern, the block was
        // already freed — this is a double-free bug.
        let first_word = core::ptr::read_volatile(phys_to_ptr::<u32>(phys_addr) as *const u32);
        assert!(
            first_word != POISON_PATTERN,
            "[mm] BUG: double-free detected at {:#x} order {}",
            phys_addr,
            order
        );

        // Security: poison freed pages with 0xDEAD_DEAD pattern.
        let block_bytes = PAGE_SIZE << order;
        let ptr = phys_to_ptr::<u32>(phys_addr);
        let words = block_bytes / 4;
        for i in 0..words {
            // SAFETY: phys_addr is accessible via identity map or TTBR1
            // direct map, block_bytes is the allocation size.
            core::ptr::write_volatile(ptr.add(i), POISON_PATTERN);
        }

        // Coalescing loop.
        let mut addr = phys_addr;
        let mut current_order = order;

        while current_order < MAX_ORDER {
            // Toggle bitmap for this pair.
            let bit_is_set = self.bitmap_toggle(addr, current_order);

            if bit_is_set {
                // bit=1 means buddy is NOT free (different states). Stop merging.
                break;
            }

            // bit=0 means both buddies are now free. Coalesce!
            let buddy_addr = shared::buddy_of(addr, self.base, current_order);

            // Verify buddy is within our range.
            if buddy_addr < self.base || buddy_addr + (PAGE_SIZE << current_order) > self.end {
                // Buddy is out of range — can't coalesce. Re-toggle to undo.
                self.bitmap_toggle(addr, current_order);
                break;
            }

            // Remove buddy from its free list.
            let removed = self.remove_from_free_list(buddy_addr, current_order);
            if !removed {
                // Buddy wasn't in free list (shouldn't happen with correct bitmap).
                // Re-toggle and stop.
                self.bitmap_toggle(addr, current_order);
                break;
            }
            // Adjust total_free for the removed buddy block.
            self.total_free -= 1 << current_order;

            // Merge: the combined block starts at the lower address.
            addr = addr.min(buddy_addr);
            current_order += 1;
        }

        // If we exited the loop at MAX_ORDER without breaking, toggle the MAX_ORDER bit.
        if current_order == MAX_ORDER {
            self.bitmap_toggle(addr, current_order);
        }

        // Add the (possibly merged) block to the free list.
        self.push_block(addr, current_order);
    }

    /// Total number of free 4 KiB pages.
    pub fn total_free_pages(&self) -> usize {
        self.total_free
    }

    /// Number of pages consumed by the bitmap.
    pub fn bitmap_pages(&self) -> usize {
        self.bitmap_pages
    }

    // ── Legacy init (kept for backward compatibility during transition) ──

    /// Initialize the buddy allocator from the UEFI memory map.
    ///
    /// This is the Phase 1 init path. Phase 2's `mm::init::init_memory()` replaces
    /// it with per-pool `init_with_range()` calls.
    ///
    /// # Safety
    /// - `map_addr` must point to a valid UEFI memory descriptor array.
    /// - The identity map must be active (MMU enabled).
    /// - `[kernel_start, kernel_end)` is excluded.
    pub unsafe fn init_from_memory_map(
        &mut self,
        map_addr: u64,
        map_count: u64,
        entry_size: u64,
        kernel_start: usize,
        kernel_end: usize,
    ) {
        let base = map_addr as *const u8;

        // Compute the memory map buffer extent so we can exclude it.
        let map_buf_start = map_addr as usize;
        let map_buf_end = map_buf_start + map_count as usize * entry_size as usize;
        let map_buf_end = (map_buf_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        for i in 0..map_count {
            let ptr = base.add(i as usize * entry_size as usize);
            // SAFETY: The UEFI stub stores valid MemoryDescriptors at this address.
            let desc = &*(ptr as *const MemoryDescriptor);

            let usable = matches!(
                desc.ty,
                1 | 2 | 3 | 4 | 7 // LoaderCode, LoaderData, BSCode, BSData, Conventional
            );
            if !usable {
                continue;
            }

            let region_start = desc.phys_start as usize;
            let region_end = region_start + (desc.page_count as usize) * PAGE_SIZE;

            self.add_region_excluding2(
                region_start,
                region_end,
                kernel_start,
                kernel_end,
                map_buf_start,
                map_buf_end,
            );
        }
    }

    // ── Region helpers ──────────────────────────────────────────────────

    /// Add a physical region, excluding two reserved ranges.
    ///
    /// # Safety
    /// Region must be valid, page-aligned RAM accessible via identity map.
    unsafe fn add_region_excluding2(
        &mut self,
        mut start: usize,
        end: usize,
        excl1_start: usize,
        excl1_end: usize,
        excl2_start: usize,
        excl2_end: usize,
    ) {
        start = (start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let end = end & !(PAGE_SIZE - 1);

        if start >= end {
            return;
        }

        let mut ranges: [(usize, usize); 4] = [(0, 0); 4];
        let mut count = 0;

        if start < excl1_end && end > excl1_start {
            if start < excl1_start {
                ranges[count] = (start, excl1_start);
                count += 1;
            }
            if end > excl1_end {
                ranges[count] = (excl1_end, end);
                count += 1;
            }
        } else {
            ranges[count] = (start, end);
            count += 1;
        }

        for &(s, e) in &ranges[..count] {
            if s < excl2_end && e > excl2_start {
                if s < excl2_start {
                    self.add_aligned_blocks(s, excl2_start);
                }
                if e > excl2_end {
                    self.add_aligned_blocks(excl2_end, e);
                }
            } else if s < e {
                self.add_aligned_blocks(s, e);
            }
        }
    }

    /// Add a physical region excluding three reserved ranges.
    ///
    /// # Safety
    /// Region must be valid, page-aligned RAM.
    unsafe fn add_region_excluding3(
        &mut self,
        start: usize,
        end: usize,
        excl1: (usize, usize),
        excl2: (usize, usize),
        excl3: (usize, usize),
    ) {
        // Collect all exclusion zones, sort by start address.
        let mut excls = [excl1, excl2, excl3];
        excls.sort_unstable_by_key(|&(s, _)| s);

        // Walk through the range, skipping exclusion zones.
        let mut cursor = start;
        for &(es, ee) in &excls {
            if es == ee {
                continue; // empty exclusion
            }
            if cursor < es && cursor < end {
                let seg_end = es.min(end);
                self.add_aligned_blocks(cursor, seg_end);
            }
            cursor = cursor.max(ee);
        }
        if cursor < end {
            self.add_aligned_blocks(cursor, end);
        }
    }

    /// Add pages between `start` and `end` as the largest possible aligned blocks.
    ///
    /// # Safety
    /// Range must be valid page-aligned RAM.
    unsafe fn add_aligned_blocks(&mut self, mut addr: usize, end: usize) {
        while addr < end {
            let mut order = MAX_ORDER;
            loop {
                let block_size = PAGE_SIZE << order;
                if (addr & (block_size - 1)) == 0 && addr + block_size <= end {
                    break;
                }
                if order == 0 {
                    break;
                }
                order -= 1;
            }

            let block_size = PAGE_SIZE << order;
            if addr + block_size > end {
                break;
            }

            self.push_block(addr, order);
            addr += block_size;
        }
    }
}

/// Global buddy allocator instance (legacy, used until Phase 2 pool init).
pub static BUDDY: Mutex<BuddyAllocator> = Mutex::new(BuddyAllocator::new());

/// Initialize the buddy allocator from the UEFI memory map (legacy path).
///
/// # Safety
/// Must be called after MMU enable, with a valid BootInfo.
#[allow(dead_code)]
pub unsafe fn init(
    map_addr: u64,
    map_count: u64,
    entry_size: u64,
    kernel_start: usize,
    kernel_end: usize,
) -> usize {
    let mut buddy = BUDDY.lock();
    buddy.init_from_memory_map(map_addr, map_count, entry_size, kernel_start, kernel_end);
    buddy.total_free_pages()
}

/// Allocate a single physical page (order 0). Returns physical address.
pub fn alloc_page() -> Option<usize> {
    let mut buddy = BUDDY.lock();
    // SAFETY: Identity map is active after MMU enable.
    unsafe { buddy.alloc_pages(0) }
}

/// Free a single physical page.
///
/// # Safety
/// `phys_addr` must have been returned by `alloc_page`.
#[allow(dead_code)]
pub unsafe fn free_page(phys_addr: usize) {
    let mut buddy = BUDDY.lock();
    buddy.free_pages(phys_addr, 0);
}
