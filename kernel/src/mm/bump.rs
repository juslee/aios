//! Bump allocator for early boot.
//!
//! Provides a simple bump allocator backed by a static buffer. Used during
//! DTB parsing (fdt-parser requires alloc) and early page table construction.
//! Replaced by the slab allocator once the heap is initialized in Step 6.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// 128 KiB static buffer for early allocations.
const BUMP_SIZE: usize = 128 * 1024;

#[repr(C, align(4096))]
struct BumpBuffer {
    data: [u8; BUMP_SIZE],
}

static BUMP_BUFFER: BumpBuffer = BumpBuffer {
    data: [0u8; BUMP_SIZE],
};
static BUMP_OFFSET: AtomicUsize = AtomicUsize::new(0);

pub struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        loop {
            let current = BUMP_OFFSET.load(Ordering::Relaxed);
            let base = BUMP_BUFFER.data.as_ptr() as usize;
            let aligned = (base + current + layout.align() - 1) & !(layout.align() - 1);
            let offset = aligned - base;
            let new_offset = offset + layout.size();
            if new_offset > BUMP_SIZE {
                return core::ptr::null_mut();
            }
            if BUMP_OFFSET
                .compare_exchange_weak(current, new_offset, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return aligned as *mut u8;
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator never frees — memory is reclaimed when the slab
        // allocator takes over in Step 6.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;
