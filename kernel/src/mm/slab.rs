//! Slab allocator for kernel heap objects.
//!
//! Fixed-size caches backed by the buddy allocator. Each cache manages
//! objects of a single size class with a per-cache magazine layer for
//! fast-path allocation without touching the shared free list.
//!
//! Standard caches: 64, 128, 256, 512, 4096 bytes.
//! Per memory.md §4.1.

use core::alloc::Layout;

const PAGE_SIZE: usize = 4096;

/// Number of standard size classes.
const NUM_CACHES: usize = 5;

/// Standard cache sizes (per memory.md §4.1).
/// Smaller allocations round up to 64; 1024/2048 round up to 4096.
const CACHE_SIZES: [usize; NUM_CACHES] = [64, 128, 256, 512, 4096];

/// Red zone size in bytes — guard bytes placed before and after each object
/// to detect buffer overflows (per fuzzing-and-hardening.md §3.3).
const RED_ZONE_SIZE: usize = 8;

/// Red zone fill pattern.
const RED_ZONE_PATTERN: u64 = 0xFEFE_FEFE_FEFE_FEFE;

/// Magazine capacity — number of cached object pointers per round.
const MAGAZINE_SIZE: usize = 32;

// ── Magazine layer ──────────────────────────────────────────────────────

/// A single magazine round — a stack of cached free object pointers.
struct MagazineRound {
    /// Cached object pointers (valid entries are indices 0..count).
    objects: [usize; MAGAZINE_SIZE],
    /// Number of valid entries (stack top).
    count: usize,
}

impl MagazineRound {
    const fn new() -> Self {
        Self {
            objects: [0; MAGAZINE_SIZE],
            count: 0,
        }
    }

    /// Pop a pointer from the magazine. Returns None if empty.
    fn pop(&mut self) -> Option<*mut u8> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        Some(self.objects[self.count] as *mut u8)
    }

    /// Push a pointer onto the magazine. Returns false if full.
    fn push(&mut self, ptr: *mut u8) -> bool {
        if self.count >= MAGAZINE_SIZE {
            return false;
        }
        self.objects[self.count] = ptr as usize;
        self.count += 1;
        true
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Per-cache magazine with current/prev swap for two-chance fast path.
struct Magazine {
    current: MagazineRound,
    prev: MagazineRound,
}

impl Magazine {
    const fn new() -> Self {
        Self {
            current: MagazineRound::new(),
            prev: MagazineRound::new(),
        }
    }
}

// ── Slab cache ──────────────────────────────────────────────────────────

/// A single slab cache for fixed-size objects.
struct SlabCache {
    /// User-visible object size.
    object_size: usize,
    /// Internal allocation size (object_size + 2 * RED_ZONE_SIZE).
    alloc_size: usize,
    /// Head of the shared free list (pointer to next free slot, 0 = empty).
    free_head: usize,
    /// Number of pages allocated from buddy for this cache.
    pages_used: usize,
    /// Per-cache magazine for fast-path alloc/free.
    magazine: Magazine,
}

impl SlabCache {
    const fn new(size: usize) -> Self {
        Self {
            object_size: size,
            alloc_size: size + 2 * RED_ZONE_SIZE,
            free_head: 0,
            pages_used: 0,
            magazine: Magazine::new(),
        }
    }

    /// Allocate one object from this cache.
    ///
    /// Fast path: pop from magazine (current, then swap+retry).
    /// Slow path: refill magazine from shared free list, then pop.
    ///
    /// # Safety
    /// Identity map or direct map must be active for memory access.
    unsafe fn alloc(&mut self) -> *mut u8 {
        // Fast path 1: pop from current magazine
        if let Some(ptr) = self.magazine.current.pop() {
            return ptr;
        }

        // Fast path 2: swap current ↔ prev, try again
        core::mem::swap(&mut self.magazine.current, &mut self.magazine.prev);
        if let Some(ptr) = self.magazine.current.pop() {
            return ptr;
        }

        // Slow path: refill magazine from shared free list
        self.refill_magazine();
        self.magazine.current.pop().unwrap_or(core::ptr::null_mut())
    }

    /// Return an object to this cache.
    ///
    /// Fast path: push to current magazine.
    /// Overflow: swap, try again; if both full, flush prev to free list.
    ///
    /// # Safety
    /// `ptr` must have been returned by `alloc` on this same cache.
    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        // Verify red zones before returning to cache
        self.check_red_zones(ptr);

        // Fast path: push to current magazine
        if self.magazine.current.push(ptr) {
            return;
        }

        // Swap current ↔ prev, try again
        core::mem::swap(&mut self.magazine.current, &mut self.magazine.prev);
        if self.magazine.current.push(ptr) {
            return;
        }

        // Both magazines full — flush prev (now in current) to shared free list
        self.flush_magazine_to_freelist();
        // Now current is empty; push the object
        let ok = self.magazine.current.push(ptr);
        debug_assert!(ok);
    }

    /// Refill the current magazine from the shared free list.
    /// If the free list is also empty, grows the cache from buddy.
    unsafe fn refill_magazine(&mut self) {
        let mut refilled = 0;
        while refilled < MAGAZINE_SIZE && self.free_head != 0 {
            let slot_addr = self.free_head;
            // SAFETY: free_head points to a free slot whose first 8 bytes
            // contain the next pointer. The slot is in a buddy-allocated page.
            self.free_head = core::ptr::read(slot_addr as *const usize);

            // The slot_addr points to the start of the alloc_size region.
            // Return the user pointer (after the leading red zone).
            let user_ptr = (slot_addr + RED_ZONE_SIZE) as *mut u8;
            self.fill_red_zones(slot_addr);

            let ok = self.magazine.current.push(user_ptr);
            debug_assert!(ok);
            refilled += 1;
        }

        // If we got nothing from the free list, grow from buddy
        if refilled == 0 {
            self.grow();
            // Try once more after growing
            while refilled < MAGAZINE_SIZE && self.free_head != 0 {
                let slot_addr = self.free_head;
                self.free_head = core::ptr::read(slot_addr as *const usize);

                let user_ptr = (slot_addr + RED_ZONE_SIZE) as *mut u8;
                self.fill_red_zones(slot_addr);

                let ok = self.magazine.current.push(user_ptr);
                debug_assert!(ok);
                refilled += 1;
            }
        }
    }

    /// Flush the current magazine back to the shared free list.
    unsafe fn flush_magazine_to_freelist(&mut self) {
        while let Some(user_ptr) = self.magazine.current.pop() {
            // Convert user pointer back to slot start (before red zone)
            let slot_addr = user_ptr as usize - RED_ZONE_SIZE;
            // SAFETY: slot_addr was originally from the free list; writing
            // the next pointer into the first 8 bytes is safe.
            core::ptr::write(slot_addr as *mut usize, self.free_head);
            self.free_head = slot_addr;
        }
    }

    /// Fill red zone guard bytes around an object at `slot_addr`.
    unsafe fn fill_red_zones(&self, slot_addr: usize) {
        // Leading red zone: slot_addr..slot_addr+RED_ZONE_SIZE
        // SAFETY: slot_addr points to an alloc_size region within a buddy page.
        core::ptr::write(slot_addr as *mut u64, RED_ZONE_PATTERN);
        // Trailing red zone: slot_addr+RED_ZONE_SIZE+object_size..
        let trailing = slot_addr + RED_ZONE_SIZE + self.object_size;
        core::ptr::write(trailing as *mut u64, RED_ZONE_PATTERN);
    }

    /// Check red zone integrity on dealloc. Prints warning if corrupted.
    unsafe fn check_red_zones(&self, user_ptr: *mut u8) {
        let slot_addr = user_ptr as usize - RED_ZONE_SIZE;

        // SAFETY: slot_addr and trailing point within the alloc_size region.
        let leading = core::ptr::read(slot_addr as *const u64);
        let trailing_addr = user_ptr as usize + self.object_size;
        let trailing = core::ptr::read(trailing_addr as *const u64);

        if leading != RED_ZONE_PATTERN || trailing != RED_ZONE_PATTERN {
            crate::println!(
                "[slab] RED ZONE CORRUPTION: cache={} ptr={:#x} leading={:#x} trailing={:#x}",
                self.object_size,
                user_ptr as usize,
                leading,
                trailing,
            );
        }
    }

    /// Request a page from the buddy allocator and carve it into free slots.
    unsafe fn grow(&mut self) {
        let page = super::frame::alloc_page().or_else(super::buddy::alloc_page);
        let Some(page_addr) = page else { return };

        self.pages_used += 1;

        // Carve the page into alloc_size slots and link into the free list.
        // Each slot's first 8 bytes store the next pointer.
        let count = PAGE_SIZE / self.alloc_size;
        for i in (0..count).rev() {
            let slot_addr = page_addr + i * self.alloc_size;
            // SAFETY: page_addr is a valid page from buddy. Each slot_addr is
            // within the page. Writing the next pointer into the first 8 bytes
            // of each free slot is safe (alloc_size >= 64 + 16 = 80 >= 8).
            core::ptr::write(slot_addr as *mut usize, self.free_head);
            self.free_head = slot_addr;
        }
    }
}

/// The kernel slab allocator with multiple size-class caches.
pub struct SlabAllocator {
    caches: [SlabCache; NUM_CACHES],
}

impl SlabAllocator {
    const fn new() -> Self {
        Self {
            caches: [
                SlabCache::new(CACHE_SIZES[0]),
                SlabCache::new(CACHE_SIZES[1]),
                SlabCache::new(CACHE_SIZES[2]),
                SlabCache::new(CACHE_SIZES[3]),
                SlabCache::new(CACHE_SIZES[4]),
            ],
        }
    }

    /// Find the cache index for a given layout.
    fn cache_index(layout: &Layout) -> Option<usize> {
        let size = layout.size().max(layout.align());
        CACHE_SIZES.iter().position(|&s| s >= size)
    }
}

/// Global slab allocator instance.
pub static SLAB: spin::Mutex<SlabAllocator> = spin::Mutex::new(SlabAllocator::new());

/// Allocate from the slab allocator.
///
/// # Safety
/// Buddy allocator must be initialized. Identity map must be active.
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    // Use the larger of size and alignment to ensure the buddy allocator
    // returns a block that satisfies both requirements.
    let effective_size = layout.size().max(layout.align());

    if effective_size > PAGE_SIZE {
        // Large allocation: get pages directly from the frame allocator (or legacy buddy).
        let pages = effective_size.div_ceil(PAGE_SIZE);
        let page_order = pages.next_power_of_two().trailing_zeros() as usize;
        // SAFETY: Identity map is active; frame allocator or buddy is initialized.
        let addr = {
            let mut guard = super::frame::FRAME_ALLOC.lock();
            if let Some(fa) = guard.as_mut() {
                fa.alloc_pages(shared::Pool::Kernel, page_order)
            } else {
                let mut buddy = super::buddy::BUDDY.lock();
                buddy.alloc_pages(page_order)
            }
        };
        return addr.map_or(core::ptr::null_mut(), |a| a as *mut u8);
    }

    let Some(idx) = SlabAllocator::cache_index(&layout) else {
        return core::ptr::null_mut();
    };

    let mut slab = SLAB.lock();
    slab.caches[idx].alloc()
}

/// Deallocate to the slab allocator.
///
/// # Safety
/// `ptr` must have been returned by `alloc` with the same `layout`.
pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    if ptr.is_null() {
        return;
    }

    let effective_size = layout.size().max(layout.align());

    if effective_size > PAGE_SIZE {
        // Large allocation: return pages to frame allocator (or legacy buddy).
        let pages = effective_size.div_ceil(PAGE_SIZE);
        let page_order = pages.next_power_of_two().trailing_zeros() as usize;
        // SAFETY: ptr was returned by alloc_pages with the same order.
        let mut guard = super::frame::FRAME_ALLOC.lock();
        if let Some(fa) = guard.as_mut() {
            fa.free_pages(ptr as usize, page_order);
        } else {
            let mut buddy = super::buddy::BUDDY.lock();
            buddy.free_pages(ptr as usize, page_order);
        }
        return;
    }

    let Some(idx) = SlabAllocator::cache_index(&layout) else {
        return;
    };

    let mut slab = SLAB.lock();
    slab.caches[idx].dealloc(ptr);
}

/// Return the standard cache sizes for diagnostic printing.
#[allow(dead_code)]
pub fn cache_sizes() -> &'static [usize] {
    &CACHE_SIZES
}
