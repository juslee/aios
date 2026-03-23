//! Frame allocator — unified interface over page pools.
//!
//! Wraps `PagePools` with pool-aware allocation, deallocation, and memory
//! pressure monitoring. Provides a global `FRAME_ALLOC` static for kernel-wide
//! physical page allocation.
//!
//! Per memory.md §2.3.

use shared::{MemoryPressure, Pool};
use spin::Mutex;

use super::pools::PagePools;

/// Frame allocator wrapping the partitioned page pools.
pub struct FrameAllocator {
    pools: PagePools,
    total_pages: usize,
}

impl FrameAllocator {
    /// Create a new frame allocator from initialized page pools.
    pub fn new(pools: PagePools, total_pages: usize) -> Self {
        Self { pools, total_pages }
    }

    /// Allocate a single page from the specified pool.
    ///
    /// # Safety
    /// Identity map must be active.
    pub unsafe fn alloc_page(&mut self, pool: Pool) -> Option<usize> {
        self.alloc_pages(pool, 0)
    }

    /// Allocate `2^order` contiguous pages from the specified pool.
    ///
    /// # Safety
    /// Identity map must be active.
    pub unsafe fn alloc_pages(&mut self, pool: Pool, order: usize) -> Option<usize> {
        self.pools.get_mut(pool)?.alloc_pages(order)
    }

    /// Free `2^order` contiguous pages at `phys_addr`.
    ///
    /// Determines the owning pool from the address range and frees to that pool.
    ///
    /// # Safety
    /// `phys_addr` must have been returned by a prior `alloc_pages` call with
    /// the same `order`.
    pub unsafe fn free_pages(&mut self, phys_addr: usize, order: usize) {
        let pool = self.pools.pool_for_addr(phys_addr);
        debug_assert!(
            pool.is_some(),
            "[mm] BUG: free_pages({:#x}, {}) — address not in any pool",
            phys_addr,
            order
        );
        if let Some(pool) = pool {
            if let Some(buddy) = self.pools.get_mut(pool) {
                buddy.free_pages(phys_addr, order);
            }
        }
    }

    /// Current memory pressure based on user pool free ratio.
    #[allow(dead_code)]
    pub fn pressure(&self) -> MemoryPressure {
        let user_total = if self.pools.user.is_initialized() {
            (self.pools.user.end() - self.pools.user.base()) / super::buddy::PAGE_SIZE
        } else {
            0
        };
        let user_free = self.pools.user.total_free_pages();
        MemoryPressure::from_free_ratio(user_free, user_total)
    }

    /// Free pages in a specific pool.
    #[allow(dead_code)]
    pub fn pool_free_pages(&self, pool: Pool) -> usize {
        self.pools.get(pool).map_or(0, |b| b.total_free_pages())
    }

    /// Total free pages across all pools.
    pub fn total_free_pages(&self) -> usize {
        self.pools.total_free_pages()
    }

    /// Total managed pages (all pools).
    #[allow(dead_code)]
    pub fn total_pages(&self) -> usize {
        self.total_pages
    }

    /// Print pool statistics to UART.
    pub fn print_stats(&self) {
        use super::buddy::PAGE_SIZE;

        let total_mb = (self.total_pages * PAGE_SIZE) / (1024 * 1024);
        let kernel_mb = pool_mb(&self.pools.kernel);
        let user_mb = pool_mb(&self.pools.user);
        let model_mb = self.pools.model.as_ref().map_or(0, pool_mb);
        let dma_mb = pool_mb(&self.pools.dma);

        crate::kinfo!(Mm, "Physical memory: {} MB total", total_mb);
        crate::kinfo!(
            Mm,
            "Pools: kernel={} MB, user={} MB, model={} MB, dma={} MB",
            kernel_mb,
            user_mb,
            model_mb,
            dma_mb
        );
        crate::kinfo!(
            Mm,
            "Free pages: {} / {}",
            self.total_free_pages(),
            self.total_pages
        );
    }
}

fn pool_mb(buddy: &super::buddy::BuddyAllocator) -> usize {
    if buddy.is_initialized() {
        (buddy.end() - buddy.base()) / (1024 * 1024)
    } else {
        0
    }
}

/// Global frame allocator, initialized by `mm::init::init_memory()`.
pub static FRAME_ALLOC: Mutex<Option<FrameAllocator>> = Mutex::new(None);

/// Allocate a single page from the kernel pool (convenience wrapper).
///
/// Used by the slab allocator and other kernel subsystems.
pub fn alloc_page() -> Option<usize> {
    let mut guard = FRAME_ALLOC.lock();
    let fa = guard.as_mut()?;
    // SAFETY: Identity map is active after MMU enable.
    unsafe { fa.alloc_page(Pool::Kernel) }
}

/// Free a single page (convenience wrapper).
///
/// # Safety
/// `phys_addr` must have been returned by `alloc_page`.
#[allow(dead_code)]
pub unsafe fn free_page(phys_addr: usize) {
    if let Some(fa) = FRAME_ALLOC.lock().as_mut() {
        fa.free_pages(phys_addr, 0);
    }
}

/// Allocate a single page from the user pool (for shared memory / user heaps).
pub fn alloc_user_page() -> Option<usize> {
    let mut guard = FRAME_ALLOC.lock();
    let fa = guard.as_mut()?;
    // SAFETY: Direct map is active after MMU enable.
    unsafe { fa.alloc_page(Pool::User) }
}

/// Allocate `2^order` contiguous pages from the user pool.
pub fn alloc_user_pages(order: usize) -> Option<usize> {
    let mut guard = FRAME_ALLOC.lock();
    let fa = guard.as_mut()?;
    // SAFETY: Direct map is active after MMU enable.
    unsafe { fa.alloc_pages(Pool::User, order) }
}

/// Free a single page back to its owning pool.
///
/// # Safety
/// `phys_addr` must have been returned by `alloc_user_page`.
pub unsafe fn free_user_page(phys_addr: usize) {
    if let Some(fa) = FRAME_ALLOC.lock().as_mut() {
        fa.free_pages(phys_addr, 0);
    }
}

/// Free `2^order` contiguous pages back to their owning pool.
///
/// # Safety
/// `phys_addr` must have been returned by `alloc_user_pages` with the same `order`.
pub unsafe fn free_user_pages(phys_addr: usize, order: usize) {
    if let Some(fa) = FRAME_ALLOC.lock().as_mut() {
        fa.free_pages(phys_addr, order);
    }
}

/// Allocate a single page from the DMA pool (for device I/O buffers).
///
/// NOTE: QEMU virt is DMA-coherent, so WB direct map is safe for DMA buffers.
/// Real hardware may require Non-Cacheable mapping (hal.md §9).
pub fn alloc_dma_page() -> Option<usize> {
    let mut guard = FRAME_ALLOC.lock();
    let fa = guard.as_mut()?;
    // SAFETY: Direct map is active after MMU enable.
    unsafe { fa.alloc_page(Pool::Dma) }
}

/// Allocate `2^order` contiguous pages from the DMA pool.
pub fn alloc_dma_pages(order: usize) -> Option<usize> {
    let mut guard = FRAME_ALLOC.lock();
    let fa = guard.as_mut()?;
    // SAFETY: Direct map is active after MMU enable.
    unsafe { fa.alloc_pages(Pool::Dma, order) }
}

// ---------------------------------------------------------------------------
// Memory Kit trait implementation
// ---------------------------------------------------------------------------

use shared::kits::memory::{
    self as memory_kit, MemoryError, PhysFrame, PoolStats,
};

/// Kernel-side implementation of the Memory Kit's `FrameAllocator` and
/// `MemoryPressureMonitor` traits.
///
/// This is a zero-sized unit struct that delegates to the global `FRAME_ALLOC`.
#[allow(dead_code)]
pub struct KernelFrameAllocator;

impl memory_kit::FrameAllocator for KernelFrameAllocator {
    fn alloc_frame(&self, pool: Pool) -> Result<PhysFrame, MemoryError> {
        let mut guard = FRAME_ALLOC.lock();
        let fa = guard.as_mut().ok_or(MemoryError::OutOfMemory)?;
        // SAFETY: Identity/direct map is active after Phase 1 boot completes.
        // The buddy allocator writes intrusive free-list pointers into freed
        // pages via the identity map. If the identity map is not active,
        // the allocator would fault on page metadata writes.
        let addr = unsafe { fa.alloc_page(pool) }.ok_or(MemoryError::OutOfMemory)?;
        Ok(PhysFrame {
            addr: addr as u64,
            pool,
        })
    }

    fn free_frame(&self, frame: PhysFrame) -> Result<(), MemoryError> {
        let mut guard = FRAME_ALLOC.lock();
        let fa = guard.as_mut().ok_or(MemoryError::OutOfMemory)?;
        // SAFETY: The PhysFrame was obtained from alloc_frame, so the address
        // is valid and was previously allocated as a single page (order 0).
        // The identity/direct map is active, making the buddy's intrusive
        // free-list pointer writes safe.
        unsafe { fa.free_pages(frame.addr as usize, 0) };
        Ok(())
    }

    fn pool_pressure(&self, pool: Pool) -> MemoryPressure {
        let guard = FRAME_ALLOC.lock();
        let fa = match guard.as_ref() {
            Some(fa) => fa,
            None => return MemoryPressure::Oom,
        };
        let free = fa.pool_free_pages(pool);
        let total = pool_total_pages(fa, pool);
        MemoryPressure::from_free_ratio(free, total)
    }

    fn pool_stats(&self, pool: Pool) -> PoolStats {
        let guard = FRAME_ALLOC.lock();
        let fa = match guard.as_ref() {
            Some(fa) => fa,
            None => return PoolStats { free_frames: 0, total_frames: 0 },
        };
        PoolStats {
            free_frames: fa.pool_free_pages(pool),
            total_frames: pool_total_pages(fa, pool),
        }
    }
}

impl memory_kit::MemoryPressureMonitor for KernelFrameAllocator {
    fn current_level(&self) -> MemoryPressure {
        let guard = FRAME_ALLOC.lock();
        match guard.as_ref() {
            Some(fa) => fa.pressure(),
            None => MemoryPressure::Oom,
        }
    }
}

/// Compute total pages for a specific pool from the buddy allocator range.
#[allow(dead_code)]
fn pool_total_pages(fa: &FrameAllocator, pool: Pool) -> usize {
    use super::buddy::PAGE_SIZE;
    match fa.pools.get(pool) {
        Some(buddy) if buddy.is_initialized() => {
            (buddy.end() - buddy.base()) / PAGE_SIZE
        }
        _ => 0,
    }
}
