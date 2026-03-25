//! Shared VirtIO MMIO legacy (v1) transport helpers.
//!
//! Provides MMIO access functions, virtqueue layout calculations, and
//! constants shared by all VirtIO device drivers (block, GPU, etc.).
//!
//! Wire-format types (`VirtqDesc`, register offsets, status constants) live in
//! `shared::storage` — this module only contains kernel-side helpers.

/// Virtqueue size (number of descriptors). Must be ≤ QUEUE_NUM_MAX from device.
pub const QUEUE_SIZE: u16 = 128;

/// Polling timeout iterations for virtqueue completion.
pub const POLL_TIMEOUT: u32 = 10_000_000;

/// Page size used for legacy VirtIO MMIO queue alignment.
pub const VIRT_PAGE_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// MMIO access helpers
// ---------------------------------------------------------------------------

/// Read a 32-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid MMIO register address mapped as device memory
/// (e.g., via the TTBR1 MMIO map at `MMIO_BASE + phys`).
#[inline(always)]
pub unsafe fn mmio_read32(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

/// Write a 32-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid MMIO register address mapped as device memory.
#[inline(always)]
pub unsafe fn mmio_write32(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}

// ---------------------------------------------------------------------------
// Legacy virtqueue layout helpers
// ---------------------------------------------------------------------------

/// Calculate the byte offset of the available ring from the start of the
/// virtqueue allocation (immediately after the descriptor table).
pub const fn avail_offset(queue_size: usize) -> usize {
    // Descriptor table: queue_size × 16 bytes.
    queue_size * 16
}

/// Calculate the byte offset of the used ring from the start of the
/// virtqueue allocation (page-aligned after the available ring).
pub const fn used_offset(queue_size: usize) -> usize {
    // Available ring: 4 bytes header + queue_size × 2 bytes + 2 bytes used_event.
    let avail_end = avail_offset(queue_size) + 4 + queue_size * 2 + 2;
    // Align up to page boundary.
    (avail_end + VIRT_PAGE_SIZE - 1) & !(VIRT_PAGE_SIZE - 1)
}

/// Total size of the virtqueue allocation in bytes.
pub const fn virtqueue_size(queue_size: usize) -> usize {
    // Used ring: 4 bytes header + queue_size × 8 bytes + 2 bytes avail_event.
    let used_end = used_offset(queue_size) + 4 + queue_size * 8 + 2;
    // Align up to page boundary.
    (used_end + VIRT_PAGE_SIZE - 1) & !(VIRT_PAGE_SIZE - 1)
}
