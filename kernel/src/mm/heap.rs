//! Typed kernel heap API — kalloc/kfree.
//!
//! Routes small allocations through the slab allocator and large ones
//! through the buddy/frame allocator. Provides typed wrappers on top
//! of the existing `#[global_allocator]` infrastructure.
//!
//! Per memory.md §4.2.

use core::alloc::Layout;

/// Allocate memory for a value of type `T`.
///
/// Uses slab for sizes ≤ 4096, buddy for larger.
/// Panics on OOM — kernel allocation failure is fatal.
#[allow(dead_code)]
pub fn kalloc<T>() -> *mut T {
    let layout = Layout::new::<T>();
    // SAFETY: Slab/buddy allocators are initialized before kalloc is called.
    // The layout is derived from a concrete type, so size and alignment are valid.
    let ptr = unsafe { super::slab::alloc(layout) };
    if ptr.is_null() {
        panic!(
            "[mm] kalloc: OOM for {} bytes (align {})",
            layout.size(),
            layout.align()
        );
    }
    ptr as *mut T
}

/// Free memory previously allocated by `kalloc<T>`.
///
/// # Safety
/// `ptr` must have been returned by `kalloc::<T>()` and must not be freed twice.
#[allow(dead_code)]
pub unsafe fn kfree<T>(ptr: *mut T) {
    if ptr.is_null() {
        return;
    }
    let layout = Layout::new::<T>();
    super::slab::dealloc(ptr as *mut u8, layout);
}

/// Allocate a byte buffer with the given layout. Panics on OOM.
#[allow(dead_code)]
pub fn kalloc_layout(layout: Layout) -> *mut u8 {
    // SAFETY: Slab/buddy allocators are initialized before kalloc_layout is called.
    let ptr = unsafe { super::slab::alloc(layout) };
    if ptr.is_null() {
        panic!(
            "[mm] kalloc_layout: OOM for {} bytes (align {})",
            layout.size(),
            layout.align()
        );
    }
    ptr
}

/// Free a byte buffer allocated by `kalloc_layout`.
///
/// # Safety
/// `ptr` must have been returned by `kalloc_layout` with the same `layout`.
#[allow(dead_code)]
pub unsafe fn kfree_layout(ptr: *mut u8, layout: Layout) {
    if !ptr.is_null() {
        super::slab::dealloc(ptr, layout);
    }
}
