//! Physical memory pool types, pressure levels, and buddy allocator helpers.

const MIB: usize = 1024 * 1024;
const GIB: usize = 1024 * MIB;

/// Physical memory pool classification.
///
/// Each pool is backed by its own buddy allocator instance with a dedicated
/// physical address range. Pool assignment is determined at boot based on
/// total detected RAM (see `PoolConfig::from_total_ram`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// Kernel heap, page tables, slab caches.
    Kernel = 0,
    /// User-space process pages.
    User = 1,
    /// AI model weights and inference buffers (0 on small-RAM systems).
    Model = 2,
    /// DMA-capable buffers for device I/O (low physical addresses preferred).
    Dma = 3,
}

/// Memory pressure level based on free page ratio in the user pool.
///
/// Thresholds from memory.md §2.3:
/// - Normal:   >20% free
/// - Low:      11–20% free
/// - Critical: 5–10% free
/// - Oom:      <5% free
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryPressure {
    Normal = 0,
    Low = 1,
    Critical = 2,
    Oom = 3,
}

impl MemoryPressure {
    /// Determine pressure level from free and total page counts.
    pub fn from_free_ratio(free: usize, total: usize) -> Self {
        if total == 0 {
            return MemoryPressure::Oom;
        }
        let percent = (free * 100) / total;
        if percent > 20 {
            MemoryPressure::Normal
        } else if percent > 10 {
            MemoryPressure::Low
        } else if percent >= 5 {
            MemoryPressure::Critical
        } else {
            MemoryPressure::Oom
        }
    }
}

/// Per-pool byte budgets computed from total detected RAM.
///
/// See memory.md §2.4 for the tier table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    pub kernel: usize,
    pub model: usize,
    pub user: usize,
    pub dma: usize,
    pub reserved: usize,
}

impl PoolConfig {
    /// Compute pool sizes from total detected RAM.
    ///
    /// Tiers (memory.md §2.4):
    /// - <4 GB:   kernel=128M, model=0,  dma=64M,  reserved=64M,  user=remainder
    /// - <8 GB:   kernel=256M, model=2G, dma=128M, reserved=128M, user=remainder
    /// - <16 GB:  kernel=256M, model=4G, dma=128M, reserved=128M, user=remainder
    /// - ≥16 GB:  kernel=256M, model=8G, dma=128M, reserved=128M, user=remainder
    pub fn from_total_ram(total: usize) -> Self {
        let (kernel, model, dma, reserved) = if total < 4 * GIB {
            (128 * MIB, 0, 64 * MIB, 64 * MIB)
        } else if total < 8 * GIB {
            (256 * MIB, 2 * GIB, 128 * MIB, 128 * MIB)
        } else if total < 16 * GIB {
            (256 * MIB, 4 * GIB, 128 * MIB, 128 * MIB)
        } else {
            (256 * MIB, 8 * GIB, 128 * MIB, 128 * MIB)
        };

        let fixed = kernel + model + dma + reserved;
        let user = total.saturating_sub(fixed);

        PoolConfig {
            kernel,
            model,
            user,
            dma,
            reserved,
        }
    }
}

/// Compute the buddy address for a given address at a given order.
///
/// Classic XOR trick: the buddy of block at `addr` (relative to `base`)
/// with size `1 << (order + PAGE_SHIFT)` is found by flipping bit `order + PAGE_SHIFT`.
///
/// `addr` and `base` are physical addresses; `order` is the buddy order (0 = 4 KiB).
pub const fn buddy_of(addr: usize, base: usize, order: usize) -> usize {
    let page_shift = 12; // 4 KiB pages
    let offset = addr - base;
    let buddy_offset = offset ^ (1 << (order + page_shift));
    base + buddy_offset
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PoolConfig sizing tests ─────────────────────────────────────────

    #[test]
    fn pool_config_2g() {
        let cfg = PoolConfig::from_total_ram(2 * GIB);
        assert_eq!(cfg.kernel, 128 * MIB);
        assert_eq!(cfg.model, 0);
        assert_eq!(cfg.dma, 64 * MIB);
        assert_eq!(cfg.reserved, 64 * MIB);
        assert_eq!(cfg.user, 2 * GIB - 128 * MIB - 64 * MIB - 64 * MIB);
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            2 * GIB
        );
    }

    #[test]
    fn pool_config_4g() {
        let cfg = PoolConfig::from_total_ram(4 * GIB);
        assert_eq!(cfg.kernel, 256 * MIB);
        assert_eq!(cfg.model, 2 * GIB);
        assert_eq!(cfg.dma, 128 * MIB);
        assert_eq!(cfg.reserved, 128 * MIB);
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            4 * GIB
        );
    }

    #[test]
    fn pool_config_8g() {
        let cfg = PoolConfig::from_total_ram(8 * GIB);
        assert_eq!(cfg.kernel, 256 * MIB);
        assert_eq!(cfg.model, 4 * GIB);
        assert_eq!(cfg.dma, 128 * MIB);
        assert_eq!(cfg.reserved, 128 * MIB);
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            8 * GIB
        );
    }

    #[test]
    fn pool_config_16g() {
        let cfg = PoolConfig::from_total_ram(16 * GIB);
        assert_eq!(cfg.kernel, 256 * MIB);
        assert_eq!(cfg.model, 8 * GIB);
        assert_eq!(cfg.dma, 128 * MIB);
        assert_eq!(cfg.reserved, 128 * MIB);
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            16 * GIB
        );
    }

    #[test]
    fn pool_config_32g() {
        let cfg = PoolConfig::from_total_ram(32 * GIB);
        assert_eq!(cfg.kernel, 256 * MIB);
        assert_eq!(cfg.model, 8 * GIB);
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            32 * GIB
        );
    }

    // ── buddy_of XOR tests ──────────────────────────────────────────────

    #[test]
    fn buddy_of_order_0() {
        let base = 0x4000_0000;
        assert_eq!(buddy_of(base, base, 0), base + 0x1000);
        assert_eq!(buddy_of(base + 0x1000, base, 0), base);
    }

    #[test]
    fn buddy_of_order_1() {
        let base = 0x4000_0000;
        assert_eq!(buddy_of(base, base, 1), base + 0x2000);
        assert_eq!(buddy_of(base + 0x2000, base, 1), base);
    }

    #[test]
    fn buddy_of_higher_orders() {
        let base = 0x4000_0000;
        let block = base + 4 * MIB;
        let buddy = buddy_of(block, base, 10);
        assert_eq!(buddy, base);
        assert_eq!(buddy_of(buddy, base, 10), block);
    }

    #[test]
    fn buddy_of_is_symmetric() {
        let base = 0x4000_0000;
        for order in 0..=10 {
            let addr = base + (3 << (order + 12));
            let buddy = buddy_of(addr, base, order);
            assert_eq!(buddy_of(buddy, base, order), addr);
        }
    }

    // ── MemoryPressure tests ────────────────────────────────────────────

    #[test]
    fn pressure_normal() {
        assert_eq!(
            MemoryPressure::from_free_ratio(25, 100),
            MemoryPressure::Normal
        );
        assert_eq!(
            MemoryPressure::from_free_ratio(21, 100),
            MemoryPressure::Normal
        );
    }

    #[test]
    fn pressure_low() {
        assert_eq!(
            MemoryPressure::from_free_ratio(20, 100),
            MemoryPressure::Low
        );
        assert_eq!(
            MemoryPressure::from_free_ratio(11, 100),
            MemoryPressure::Low
        );
    }

    #[test]
    fn pressure_critical() {
        assert_eq!(
            MemoryPressure::from_free_ratio(10, 100),
            MemoryPressure::Critical
        );
        assert_eq!(
            MemoryPressure::from_free_ratio(5, 100),
            MemoryPressure::Critical
        );
    }

    #[test]
    fn pressure_oom() {
        assert_eq!(MemoryPressure::from_free_ratio(4, 100), MemoryPressure::Oom);
        assert_eq!(MemoryPressure::from_free_ratio(0, 100), MemoryPressure::Oom);
        assert_eq!(MemoryPressure::from_free_ratio(0, 0), MemoryPressure::Oom);
    }

    #[test]
    fn pressure_ordering() {
        assert!(MemoryPressure::Normal < MemoryPressure::Low);
        assert!(MemoryPressure::Low < MemoryPressure::Critical);
        assert!(MemoryPressure::Critical < MemoryPressure::Oom);
    }
}
