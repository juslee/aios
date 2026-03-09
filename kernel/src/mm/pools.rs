//! Page pool manager — partitions physical memory into kernel/user/model/DMA pools.
//!
//! Each pool is backed by its own `BuddyAllocator` instance with a dedicated
//! physical address range. Pool assignment is determined at boot based on
//! total detected RAM via `PoolConfig::from_total_ram()`.
//!
//! On QEMU virt, all usable RAM occupies a single contiguous physical region
//! starting at 0x4000_0000. The UEFI memory map fragments this into many
//! descriptors (LoaderCode, LoaderData, BSData, Conventional, etc.) but they
//! tile contiguously. We compute the overall physical extent and partition
//! that contiguous space into pools. Each pool's buddy allocator handles
//! exclusions (kernel image, memory map buffer, bitmap pages) internally.
//!
//! Per memory.md §2.4.

use shared::{Pool, PoolConfig};

use super::buddy::BuddyAllocator;

/// Page pools: four buddy allocator instances partitioning physical memory.
pub struct PagePools {
    pub kernel: BuddyAllocator,
    pub user: BuddyAllocator,
    pub model: Option<BuddyAllocator>,
    pub dma: BuddyAllocator,
}

impl PagePools {
    /// Initialize page pools from the overall usable physical range.
    ///
    /// Partitions `[phys_start, phys_end)` linearly into pools:
    /// DMA → kernel → model → reserved → user (remainder).
    ///
    /// `excl1` and `excl2` are ranges to exclude (kernel image, memory map buffer).
    ///
    /// # Safety
    /// - `[phys_start, phys_end)` must be valid, page-aligned, identity-mapped RAM.
    /// - Exclusion ranges must be page-aligned.
    pub unsafe fn init(
        phys_start: usize,
        phys_end: usize,
        config: &PoolConfig,
        excl1: (usize, usize),
        excl2: (usize, usize),
    ) -> Self {
        let total_bytes = phys_end - phys_start;

        // Compute pool boundaries as offsets from phys_start.
        // Order: DMA → kernel → model → reserved → user (remainder).
        let dma_size = config.dma.min(total_bytes);
        let kernel_size = config.kernel.min(total_bytes - dma_size);
        let model_size = config.model.min(total_bytes - dma_size - kernel_size);
        let reserved_size = config
            .reserved
            .min(total_bytes - dma_size - kernel_size - model_size);
        // user gets everything left.

        let dma_base = phys_start;
        let dma_end = dma_base + dma_size;

        let kernel_base = dma_end;
        let kernel_end = kernel_base + kernel_size;

        let model_base = kernel_end;
        let model_end = model_base + model_size;

        // Reserved region is simply skipped (not assigned to any pool).
        let reserved_end = model_end + reserved_size;

        let user_base = reserved_end;
        let user_end = phys_end;

        // Initialize each pool's buddy allocator.
        let mut dma = BuddyAllocator::new();
        if dma_base < dma_end {
            dma.init_with_range_excluding(dma_base, dma_end, excl1, excl2);
        }

        let mut kernel = BuddyAllocator::new();
        if kernel_base < kernel_end {
            kernel.init_with_range_excluding(kernel_base, kernel_end, excl1, excl2);
        }

        let model = if model_base < model_end {
            let mut m = BuddyAllocator::new();
            m.init_with_range_excluding(model_base, model_end, excl1, excl2);
            Some(m)
        } else {
            None
        };

        let mut user = BuddyAllocator::new();
        if user_base < user_end {
            user.init_with_range_excluding(user_base, user_end, excl1, excl2);
        }

        PagePools {
            kernel,
            user,
            model,
            dma,
        }
    }

    /// Get a mutable reference to the buddy allocator for a given pool.
    pub fn get_mut(&mut self, pool: Pool) -> Option<&mut BuddyAllocator> {
        match pool {
            Pool::Kernel => Some(&mut self.kernel),
            Pool::User => Some(&mut self.user),
            Pool::Model => self.model.as_mut(),
            Pool::Dma => Some(&mut self.dma),
        }
    }

    /// Get an immutable reference to the buddy allocator for a given pool.
    #[allow(dead_code)]
    pub fn get(&self, pool: Pool) -> Option<&BuddyAllocator> {
        match pool {
            Pool::Kernel => Some(&self.kernel),
            Pool::User => Some(&self.user),
            Pool::Model => self.model.as_ref(),
            Pool::Dma => Some(&self.dma),
        }
    }

    /// Determine which pool owns a physical address (by range).
    pub fn pool_for_addr(&self, addr: usize) -> Option<Pool> {
        if self.kernel.is_initialized() && addr >= self.kernel.base() && addr < self.kernel.end() {
            Some(Pool::Kernel)
        } else if self.user.is_initialized() && addr >= self.user.base() && addr < self.user.end() {
            Some(Pool::User)
        } else if let Some(ref m) = self.model {
            if m.is_initialized() && addr >= m.base() && addr < m.end() {
                Some(Pool::Model)
            } else if self.dma.is_initialized() && addr >= self.dma.base() && addr < self.dma.end()
            {
                Some(Pool::Dma)
            } else {
                None
            }
        } else if self.dma.is_initialized() && addr >= self.dma.base() && addr < self.dma.end() {
            Some(Pool::Dma)
        } else {
            None
        }
    }

    /// Total free pages across all pools.
    pub fn total_free_pages(&self) -> usize {
        self.kernel.total_free_pages()
            + self.user.total_free_pages()
            + self.model.as_ref().map_or(0, |m| m.total_free_pages())
            + self.dma.total_free_pages()
    }
}
