//! Memory management subsystem.
//!
//! Provides the global allocator (switchable from bump to slab), the buddy
//! physical page allocator, and the slab object allocator.

#[allow(dead_code)]
pub mod asid;
pub mod buddy;
pub mod bump;
pub mod frame;
pub mod init;
#[allow(dead_code)]
pub mod kmap;
#[allow(dead_code)]
pub mod pgtable;
pub mod pools;
pub mod slab;
#[allow(dead_code)]
pub mod tlb;

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

/// Flag set to true once the slab allocator is initialized and ready.
static SLAB_READY: AtomicBool = AtomicBool::new(false);

/// Switchable kernel allocator: bump during early boot, slab after heap init.
struct KernelAllocator;

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator;

// SAFETY: The KernelAllocator delegates to bump (which is lock-free via atomics)
// or slab (which is behind a spin::Mutex). Both are safe for concurrent access.
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if SLAB_READY.load(Ordering::Acquire) {
            slab::alloc(layout)
        } else {
            bump::alloc(layout)
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if SLAB_READY.load(Ordering::Acquire) {
            slab::dealloc(ptr, layout);
        }
        // Bump allocator never frees — early boot allocations are leaked.
    }
}

/// Switch the global allocator from bump to slab.
///
/// After this call, all `alloc::` allocations go through the slab allocator.
pub fn enable_slab_allocator() {
    SLAB_READY.store(true, Ordering::Release);
}
