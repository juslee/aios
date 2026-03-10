//! Memory management subsystem.
//!
//! Provides the global allocator (switchable from bump to slab), the buddy
//! physical page allocator, and the slab object allocator.

#[allow(dead_code)]
pub mod asid;
pub mod buddy;
pub mod bump;
pub mod frame;
pub mod heap;
pub mod init;
#[allow(dead_code)]
pub mod kaslr;
#[allow(dead_code)]
pub mod kmap;
#[allow(dead_code)]
pub mod pgtable;
pub mod pools;
pub mod slab;
#[allow(dead_code)]
pub mod tlb;
#[allow(dead_code)]
pub mod uspace;

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

/// Print heap readiness message with configured cache sizes.
pub fn init_heap() {
    let sizes = slab::cache_sizes();
    // Build the cache size list into a stack buffer to emit a single log entry.
    let mut buf = [0u8; 64];
    let mut pos = 0;
    for (i, s) in sizes.iter().enumerate() {
        if i > 0 && pos < buf.len() {
            buf[pos] = b',';
            pos += 1;
        }
        if pos < buf.len() {
            buf[pos] = b' ';
            pos += 1;
        }
        // Write the decimal number (max 5 digits for sizes up to 99999).
        let mut tmp = [0u8; 8];
        let mut n = *s;
        let mut len = 0;
        loop {
            if len >= tmp.len() {
                break;
            }
            tmp[len] = b'0' + (n % 10) as u8;
            len += 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        for j in (0..len).rev() {
            if pos < buf.len() {
                buf[pos] = tmp[j];
                pos += 1;
            }
        }
    }
    let cache_str = core::str::from_utf8(&buf[..pos]).unwrap_or("?");
    crate::kinfo!(Mm, "Kernel heap ready (slab caches:{})", cache_str);
}
