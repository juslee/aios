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

/// Compute the smallest buddy order such that `2^order >= pages`.
///
/// Used by the shared memory manager and frame allocator to determine
/// the allocation granularity for a given page count.
///
/// `order_for_pages(0)` = 0, `order_for_pages(1)` = 0,
/// `order_for_pages(3)` = 2 (2^2 = 4 >= 3),
/// `order_for_pages(5)` = 3 (2^3 = 8 >= 5).
pub fn order_for_pages(pages: usize) -> usize {
    if pages <= 1 {
        return 0;
    }
    let mut order = 0;
    while (1usize << order) < pages {
        order += 1;
    }
    order
}

/// Convert hardware timer ticks to nanoseconds.
///
/// Uses u128 intermediate arithmetic to avoid overflow for large tick counts.
/// Returns 0 if `freq` is 0 (uninitialized timer).
///
/// # Arguments
/// * `ticks` — raw counter delta (e.g., CNTVCT_EL0 difference)
/// * `freq` — timer frequency in Hz (e.g., CNTFRQ_EL0 = 62_500_000 on QEMU)
pub fn ticks_to_ns(ticks: u64, freq: u64) -> u64 {
    if freq == 0 {
        return 0;
    }
    ((ticks as u128 * 1_000_000_000) / freq as u128) as u64
}

/// Portable benchmark statistics accumulator.
///
/// Tracks min, max, sum, and count for a series of measurements.
/// Percentile computation is done externally on a sample buffer
/// (the caller owns the storage since `no_std` can't heap-allocate).
pub struct BenchStats {
    /// Number of recorded samples.
    pub count: usize,
    /// Minimum observed value.
    pub min: u64,
    /// Maximum observed value.
    pub max: u64,
    /// Sum of all observed values (for average computation).
    pub sum: u64,
}

impl Default for BenchStats {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchStats {
    /// Create a new empty statistics accumulator.
    pub const fn new() -> Self {
        Self {
            count: 0,
            min: u64::MAX,
            max: 0,
            sum: 0,
        }
    }

    /// Record a single measurement.
    pub fn record(&mut self, value: u64) {
        self.count += 1;
        self.sum += value;
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }
    }

    /// Average of recorded values. Returns 0 if no samples recorded.
    pub fn avg(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.sum / self.count as u64
        }
    }

    /// Compute the p-th percentile from a pre-sorted sample slice.
    ///
    /// `sorted_samples` must be sorted in ascending order and contain
    /// `self.count` valid entries (or fewer — will clamp).
    /// `percentile` should be 0..=100.
    pub fn percentile(&self, sorted_samples: &[u64], percentile: usize) -> u64 {
        if sorted_samples.is_empty() {
            return 0;
        }
        let idx = sorted_samples.len() * percentile / 100;
        let clamped = idx.min(sorted_samples.len() - 1);
        sorted_samples[clamped]
    }

    /// In-place insertion sort for a mutable sample buffer.
    ///
    /// Sorts `samples[..count]` in ascending order. Suitable for
    /// small-to-medium arrays (O(n²) but cache-friendly and no allocation).
    pub fn insertion_sort(samples: &mut [u64]) {
        for i in 1..samples.len() {
            let key = samples[i];
            let mut j = i;
            while j > 0 && samples[j - 1] > key {
                samples[j] = samples[j - 1];
                j -= 1;
            }
            samples[j] = key;
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

    // ── order_for_pages tests ──────────────────────────────────────────

    #[test]
    fn order_for_pages_zero_and_one() {
        assert_eq!(order_for_pages(0), 0);
        assert_eq!(order_for_pages(1), 0);
    }

    #[test]
    fn order_for_pages_exact_powers_of_two() {
        assert_eq!(order_for_pages(2), 1);
        assert_eq!(order_for_pages(4), 2);
        assert_eq!(order_for_pages(8), 3);
        assert_eq!(order_for_pages(16), 4);
        assert_eq!(order_for_pages(1024), 10);
    }

    #[test]
    fn order_for_pages_non_powers_round_up() {
        assert_eq!(order_for_pages(3), 2); // 2^2=4 >= 3
        assert_eq!(order_for_pages(5), 3); // 2^3=8 >= 5
        assert_eq!(order_for_pages(7), 3); // 2^3=8 >= 7
        assert_eq!(order_for_pages(9), 4); // 2^4=16 >= 9
        assert_eq!(order_for_pages(17), 5); // 2^5=32 >= 17
        assert_eq!(order_for_pages(100), 7); // 2^7=128 >= 100
    }

    #[test]
    fn order_for_pages_result_covers_input() {
        for pages in 0..=1025 {
            let order = order_for_pages(pages);
            assert!(
                1usize << order >= pages,
                "order_for_pages({}) = {} but 2^{} = {} < {}",
                pages,
                order,
                order,
                1usize << order,
                pages
            );
            // Also verify minimality: if order > 0, the previous order is too small.
            if order > 0 && pages > 0 {
                assert!(
                    1usize << (order - 1) < pages,
                    "order_for_pages({}) = {} is not minimal (2^{} = {} >= {})",
                    pages,
                    order,
                    order - 1,
                    1usize << (order - 1),
                    pages
                );
            }
        }
    }

    #[test]
    fn order_for_pages_max_buddy_order() {
        // Order 10 = 1024 pages = 4 MiB (max in our buddy allocator).
        assert_eq!(order_for_pages(1024), 10);
        assert_eq!(order_for_pages(1025), 11);
    }

    // ── ticks_to_ns tests ──────────────────────────────────────────────

    #[test]
    fn ticks_to_ns_zero_freq() {
        assert_eq!(ticks_to_ns(1000, 0), 0);
    }

    #[test]
    fn ticks_to_ns_zero_ticks() {
        assert_eq!(ticks_to_ns(0, 62_500_000), 0);
    }

    #[test]
    fn ticks_to_ns_one_second() {
        // 62.5 MHz: 62_500_000 ticks = exactly 1 second = 1_000_000_000 ns.
        assert_eq!(ticks_to_ns(62_500_000, 62_500_000), 1_000_000_000);
    }

    #[test]
    fn ticks_to_ns_one_ms() {
        // 62.5 MHz: 62_500 ticks = 1 ms = 1_000_000 ns.
        assert_eq!(ticks_to_ns(62_500, 62_500_000), 1_000_000);
    }

    #[test]
    fn ticks_to_ns_small_delta() {
        // A small delta (e.g., 100 ticks at 62.5 MHz = 1600 ns).
        assert_eq!(ticks_to_ns(100, 62_500_000), 1600);
    }

    #[test]
    fn ticks_to_ns_large_delta_no_overflow() {
        // 10 seconds worth of ticks at 62.5 MHz.
        let ticks = 625_000_000u64;
        let ns = ticks_to_ns(ticks, 62_500_000);
        assert_eq!(ns, 10_000_000_000); // 10 billion ns = 10s
    }

    #[test]
    fn ticks_to_ns_very_large_ticks() {
        // Near u64::MAX ticks — should not overflow thanks to u128 intermediate.
        let ticks = u64::MAX / 2;
        let freq = 62_500_000u64;
        let ns = ticks_to_ns(ticks, freq);
        // Expected: (2^63 - 1) * 10^9 / 62.5M ≈ 1.47 * 10^17 — fits in u64.
        assert!(ns > 0);
        // Verify approximate using u128 to avoid overflow.
        let expected_secs = ticks / freq;
        let expected_ns_approx = (expected_secs as u128 * 1_000_000_000) as u64;
        // Should be close (within rounding error of integer division).
        assert!(ns >= expected_ns_approx);
    }

    #[test]
    fn ticks_to_ns_1ghz_frequency() {
        // 1 GHz timer: 1 tick = 1 ns.
        assert_eq!(ticks_to_ns(1, 1_000_000_000), 1);
        assert_eq!(ticks_to_ns(1000, 1_000_000_000), 1000);
    }

    // ── BenchStats tests ───────────────────────────────────────────────

    #[test]
    fn bench_stats_empty() {
        let s = BenchStats::new();
        assert_eq!(s.count, 0);
        assert_eq!(s.avg(), 0);
        assert_eq!(s.min, u64::MAX);
        assert_eq!(s.max, 0);
    }

    #[test]
    fn bench_stats_single_sample() {
        let mut s = BenchStats::new();
        s.record(42);
        assert_eq!(s.count, 1);
        assert_eq!(s.min, 42);
        assert_eq!(s.max, 42);
        assert_eq!(s.avg(), 42);
    }

    #[test]
    fn bench_stats_multiple_samples() {
        let mut s = BenchStats::new();
        s.record(10);
        s.record(20);
        s.record(30);
        assert_eq!(s.count, 3);
        assert_eq!(s.min, 10);
        assert_eq!(s.max, 30);
        assert_eq!(s.avg(), 20);
        assert_eq!(s.sum, 60);
    }

    #[test]
    fn bench_stats_min_max_tracking() {
        let mut s = BenchStats::new();
        s.record(100);
        s.record(5);
        s.record(200);
        s.record(1);
        assert_eq!(s.min, 1);
        assert_eq!(s.max, 200);
    }

    #[test]
    fn bench_stats_identical_values() {
        let mut s = BenchStats::new();
        for _ in 0..100 {
            s.record(7);
        }
        assert_eq!(s.count, 100);
        assert_eq!(s.min, 7);
        assert_eq!(s.max, 7);
        assert_eq!(s.avg(), 7);
    }

    #[test]
    fn bench_stats_insertion_sort_already_sorted() {
        let mut data = [1, 2, 3, 4, 5];
        BenchStats::insertion_sort(&mut data);
        assert_eq!(data, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn bench_stats_insertion_sort_reverse() {
        let mut data = [5, 4, 3, 2, 1];
        BenchStats::insertion_sort(&mut data);
        assert_eq!(data, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn bench_stats_insertion_sort_duplicates() {
        let mut data = [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5];
        BenchStats::insertion_sort(&mut data);
        assert_eq!(data, [1, 1, 2, 3, 3, 4, 5, 5, 5, 6, 9]);
    }

    #[test]
    fn bench_stats_insertion_sort_single() {
        let mut data = [42];
        BenchStats::insertion_sort(&mut data);
        assert_eq!(data, [42]);
    }

    #[test]
    fn bench_stats_insertion_sort_empty() {
        let mut data: [u64; 0] = [];
        BenchStats::insertion_sort(&mut data);
    }

    #[test]
    fn bench_stats_percentile_basic() {
        // 100 samples: 1, 2, 3, ..., 100 (already sorted).
        let mut data = [0u64; 100];
        for i in 0..100 {
            data[i] = (i + 1) as u64;
        }
        let s = BenchStats {
            count: 100,
            min: 1,
            max: 100,
            sum: 5050,
        };
        // p50 = index 50 → value 51.
        assert_eq!(s.percentile(&data, 50), 51);
        // p99 = index 99 → value 100.
        assert_eq!(s.percentile(&data, 99), 100);
        // p0 = index 0 → value 1.
        assert_eq!(s.percentile(&data, 0), 1);
    }

    #[test]
    fn bench_stats_percentile_empty() {
        let s = BenchStats::new();
        assert_eq!(s.percentile(&[], 99), 0);
    }

    #[test]
    fn bench_stats_percentile_single_element() {
        let data = [42u64];
        let s = BenchStats {
            count: 1,
            min: 42,
            max: 42,
            sum: 42,
        };
        assert_eq!(s.percentile(&data, 0), 42);
        assert_eq!(s.percentile(&data, 50), 42);
        assert_eq!(s.percentile(&data, 100), 42);
    }

    #[test]
    fn bench_stats_p99_with_outliers() {
        // 100 samples: 90 at ~1000ns, 10 outliers at 5000+.
        let mut samples = [0u64; 100];
        let mut s = BenchStats::new();
        for i in 0..90 {
            let val = 1000 + (i as u64 % 50);
            s.record(val);
            samples[i] = val;
        }
        for i in 90..100 {
            let val = 5000 + i as u64;
            s.record(val);
            samples[i] = val;
        }
        BenchStats::insertion_sort(&mut samples);
        let p99 = s.percentile(&samples, 99);
        // p99 index = 99 → should be the last outlier.
        assert!(p99 >= 5000, "p99={} should be >= 5000", p99);
        // p50 should be in the normal range.
        let p50 = s.percentile(&samples, 50);
        assert!(p50 >= 1000 && p50 < 2000, "p50={} should be ~1000", p50);
    }
}
