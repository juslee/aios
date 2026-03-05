//! Buddy allocator for physical memory.
//!
//! Manages physical pages in orders 0–10 (4 KiB – 4 MiB). Free blocks are
//! linked via intrusive next-pointers stored in the first 8 bytes of each
//! free page (accessible via the identity map after MMU enable).
//!
//! Per memory.md §2.2. Phase 1: single allocator, no pool partitioning,
//! no coalescing on free (Phase 2).

use shared::MemoryDescriptor;
use spin::Mutex;

const PAGE_SIZE: usize = 4096;
const MAX_ORDER: usize = 10; // 2^10 pages = 4 MiB

/// Buddy allocator for physical page frames.
pub struct BuddyAllocator {
    /// Head of free list per order (physical address, 0 = empty).
    free_heads: [usize; MAX_ORDER + 1],
    /// Count of free blocks per order.
    free_count: [usize; MAX_ORDER + 1],
    /// Total free pages (sum of free_count[i] * 2^i).
    total_free: usize,
}

impl BuddyAllocator {
    const fn new() -> Self {
        Self {
            free_heads: [0; MAX_ORDER + 1],
            free_count: [0; MAX_ORDER + 1],
            total_free: 0,
        }
    }

    /// Add a single block of `2^order` pages starting at `phys_addr` to the free list.
    ///
    /// # Safety
    /// `phys_addr` must be page-aligned and point to usable RAM accessible via
    /// the identity map.
    unsafe fn push_block(&mut self, phys_addr: usize, order: usize) {
        let ptr = phys_addr as *mut usize;
        // SAFETY: phys_addr is identity-mapped RAM; writing the next pointer
        // into the first 8 bytes of the free block.
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
        // SAFETY: head is a valid identity-mapped address of a free block.
        let next = core::ptr::read_volatile(head as *const usize);
        self.free_heads[order] = next;
        self.free_count[order] -= 1;
        Some(head)
    }

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
            return Some(addr);
        }

        // Split a larger block
        for higher in (order + 1)..=MAX_ORDER {
            if let Some(addr) = self.pop_block(higher) {
                // Split: put upper halves back as free blocks
                for split_order in (order..higher).rev() {
                    let buddy_addr = addr + (PAGE_SIZE << split_order);
                    self.push_block(buddy_addr, split_order);
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
    /// Phase 1: no coalescing — just pushes back to the free list.
    ///
    /// # Safety
    /// `phys_addr` must have been previously returned by `alloc_pages` with
    /// the same `order`. Identity map must be active.
    pub unsafe fn free_pages(&mut self, phys_addr: usize, order: usize) {
        self.push_block(phys_addr, order);
    }

    /// Total number of free 4 KiB pages.
    pub fn total_free_pages(&self) -> usize {
        self.total_free
    }

    /// Initialize the buddy allocator from the UEFI memory map.
    ///
    /// Walks the memory map, identifies usable regions, and adds them
    /// as the largest possible aligned blocks.
    ///
    /// # Safety
    /// - `boot_info` must be a valid BootInfo with populated memory map fields.
    /// - The identity map must be active (MMU enabled).
    /// - The kernel range `[kernel_start, kernel_end)` is excluded.
    pub unsafe fn init_from_memory_map(
        &mut self,
        map_addr: u64,
        map_count: u64,
        entry_size: u64,
        kernel_start: usize,
        kernel_end: usize,
    ) {
        let base = map_addr as *const u8;

        for i in 0..map_count {
            let ptr = base.add(i as usize * entry_size as usize);
            // SAFETY: The UEFI stub stores valid MemoryDescriptors at this address.
            let desc = &*(ptr as *const MemoryDescriptor);

            // Only use memory types that are reclaimable after ExitBootServices
            let usable = matches!(
                desc.ty,
                1 | 2 | 3 | 4 | 7 // LoaderCode, LoaderData, BSCode, BSData, Conventional
            );
            if !usable {
                continue;
            }

            let region_start = desc.phys_start as usize;
            let region_end = region_start + (desc.page_count as usize) * PAGE_SIZE;

            // Add usable pages, skipping the kernel image range
            self.add_region(region_start, region_end, kernel_start, kernel_end);
        }
    }

    /// Add a physical region to the buddy allocator, excluding the kernel range.
    ///
    /// # Safety
    /// Region must be valid, page-aligned RAM accessible via identity map.
    unsafe fn add_region(
        &mut self,
        mut start: usize,
        end: usize,
        kernel_start: usize,
        kernel_end: usize,
    ) {
        // Page-align: round start up, round end down
        start = (start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let end = end & !(PAGE_SIZE - 1);

        if start >= end {
            return;
        }

        // If region overlaps kernel, split around it
        if start < kernel_end && end > kernel_start {
            // Part before kernel
            if start < kernel_start {
                self.add_aligned_blocks(start, kernel_start);
            }
            // Part after kernel
            if end > kernel_end {
                self.add_aligned_blocks(kernel_end, end);
            }
        } else {
            self.add_aligned_blocks(start, end);
        }
    }

    /// Add pages between `start` and `end` as the largest possible aligned blocks.
    ///
    /// # Safety
    /// Range must be valid page-aligned RAM.
    unsafe fn add_aligned_blocks(&mut self, mut addr: usize, end: usize) {
        while addr < end {
            // Find the largest order where addr is naturally aligned and the
            // block fits within the remaining range.
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

            // Verify the block fits
            let block_size = PAGE_SIZE << order;
            if addr + block_size > end {
                break;
            }

            self.push_block(addr, order);
            addr += block_size;
        }
    }
}

/// Global buddy allocator instance.
pub static BUDDY: Mutex<BuddyAllocator> = Mutex::new(BuddyAllocator::new());

/// Initialize the buddy allocator from the UEFI memory map.
///
/// # Safety
/// Must be called after MMU enable, with a valid BootInfo.
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
