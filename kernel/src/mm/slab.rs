//! Slab allocator for kernel heap objects.
//!
//! Fixed-size caches backed by the buddy allocator. Each cache manages
//! objects of a single size class. Free objects form an intrusive linked
//! list within the slab pages.
//!
//! Per memory.md §4.1. Phase 1: no magazine layer (single-core boot).

use core::alloc::Layout;

const PAGE_SIZE: usize = 4096;

/// Number of size classes: 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096
const NUM_CACHES: usize = 10;

/// Size classes (each must be a power of 2, >= 8 for the next pointer).
const CACHE_SIZES: [usize; NUM_CACHES] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

/// A single slab cache for fixed-size objects.
struct SlabCache {
    object_size: usize,
    /// Head of the free list (pointer to next free object, 0 = empty).
    free_head: usize,
    /// Number of pages allocated from buddy for this cache.
    pages_used: usize,
}

impl SlabCache {
    const fn new(size: usize) -> Self {
        Self {
            object_size: size,
            free_head: 0,
            pages_used: 0,
        }
    }

    /// Allocate one object from this cache.
    ///
    /// If the free list is empty, requests a page from the buddy allocator
    /// and carves it into objects.
    ///
    /// # Safety
    /// Identity map must be active.
    unsafe fn alloc(&mut self) -> *mut u8 {
        if self.free_head == 0 {
            self.grow();
        }

        if self.free_head == 0 {
            return core::ptr::null_mut(); // OOM
        }

        let obj = self.free_head as *mut u8;
        // SAFETY: free_head points to a free object whose first 8 bytes
        // contain the next pointer.
        self.free_head = core::ptr::read(self.free_head as *const usize);
        obj
    }

    /// Return an object to this cache.
    ///
    /// # Safety
    /// `ptr` must have been returned by `alloc` on this same cache.
    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        // SAFETY: ptr was returned by alloc on this cache, so it points to a
        // valid, aligned object of at least 8 bytes (minimum cache size).
        // Writing the free list next-pointer into the first 8 bytes is safe.
        core::ptr::write(ptr as *mut usize, self.free_head);
        self.free_head = ptr as usize;
    }

    /// Request a page from the buddy allocator and carve it into free objects.
    unsafe fn grow(&mut self) {
        let page = super::buddy::alloc_page();
        let Some(page_addr) = page else { return };

        self.pages_used += 1;

        // Carve the page into objects and link them into the free list
        let count = PAGE_SIZE / self.object_size;
        for i in (0..count).rev() {
            let obj_addr = page_addr + i * self.object_size;
            // SAFETY: page_addr is a valid page from buddy. Each obj_addr is
            // aligned to object_size (>= 8) within the page. Writing the next
            // pointer into the first 8 bytes of each free object is safe.
            core::ptr::write(obj_addr as *mut usize, self.free_head);
            self.free_head = obj_addr;
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
                SlabCache::new(CACHE_SIZES[5]),
                SlabCache::new(CACHE_SIZES[6]),
                SlabCache::new(CACHE_SIZES[7]),
                SlabCache::new(CACHE_SIZES[8]),
                SlabCache::new(CACHE_SIZES[9]),
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
    if layout.size() > PAGE_SIZE {
        // Large allocation: get pages directly from buddy.
        // Round up to the next page count, then find the order.
        let pages = layout.size().div_ceil(PAGE_SIZE);
        let page_order = pages.next_power_of_two().trailing_zeros() as usize;
        let mut buddy = super::buddy::BUDDY.lock();
        // SAFETY: buddy allocator is initialized and identity map is active.
        return buddy
            .alloc_pages(page_order)
            .map_or(core::ptr::null_mut(), |a| a as *mut u8);
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

    if layout.size() > PAGE_SIZE {
        // Large allocation: return pages to buddy (must match alloc order)
        let pages = layout.size().div_ceil(PAGE_SIZE);
        let page_order = pages.next_power_of_two().trailing_zeros() as usize;
        let mut buddy = super::buddy::BUDDY.lock();
        // SAFETY: ptr was returned by alloc_pages with the same order.
        buddy.free_pages(ptr as usize, page_order);
        return;
    }

    let Some(idx) = SlabAllocator::cache_index(&layout) else {
        return;
    };

    let mut slab = SLAB.lock();
    slab.caches[idx].dealloc(ptr);
}
