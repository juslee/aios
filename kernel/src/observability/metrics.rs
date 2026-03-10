//! Kernel metric counters: per-core sharded Counters, Gauges, Histograms.
//!
//! Feature-gated: `cfg(feature = "kernel-metrics")`. When disabled, all types
//! become zero-sized and all methods are no-ops.
//! Per observability.md §3.

use crate::smp::MAX_CORES;

// ---------------------------------------------------------------------------
// Feature-enabled implementations
// ---------------------------------------------------------------------------

#[cfg(feature = "kernel-metrics")]
#[allow(dead_code)]
mod enabled {
    use super::MAX_CORES;
    use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    /// Cache-line-aligned wrapper to prevent false sharing between cores.
    #[repr(align(64))]
    pub struct CacheAligned<T>(pub T);

    /// A monotonically increasing counter, sharded per core.
    /// Write: ~3 instructions (read MPIDR, index, fetch_add on local shard).
    /// Read: sum all shards (relaxed, eventually consistent).
    pub struct Counter {
        shards: [CacheAligned<AtomicU64>; MAX_CORES],
    }

    impl Counter {
        pub const fn new() -> Self {
            Self {
                shards: [const { CacheAligned(AtomicU64::new(0)) }; MAX_CORES],
            }
        }

        #[inline(always)]
        pub fn inc(&self) {
            let core = crate::observability::current_core_id().min(MAX_CORES - 1);
            self.shards[core].0.fetch_add(1, Ordering::Relaxed);
        }

        #[inline(always)]
        pub fn add(&self, n: u64) {
            let core = crate::observability::current_core_id().min(MAX_CORES - 1);
            self.shards[core].0.fetch_add(n, Ordering::Relaxed);
        }

        pub fn read(&self) -> u64 {
            self.shards
                .iter()
                .map(|s| s.0.load(Ordering::Relaxed))
                .sum()
        }
    }

    // SAFETY: Counter shards are per-core (no contention). Relaxed ordering is
    // sufficient for monotonic counters.
    unsafe impl Sync for Counter {}

    /// A point-in-time value that can go up or down.
    /// Not sharded — gauges represent a single system-wide value.
    pub struct Gauge {
        value: AtomicI64,
    }

    impl Gauge {
        pub const fn new() -> Self {
            Self {
                value: AtomicI64::new(0),
            }
        }

        pub fn set(&self, val: i64) {
            self.value.store(val, Ordering::Relaxed);
        }

        pub fn get(&self) -> i64 {
            self.value.load(Ordering::Relaxed)
        }

        pub fn inc(&self) {
            self.value.fetch_add(1, Ordering::Relaxed);
        }

        pub fn dec(&self) {
            self.value.fetch_sub(1, Ordering::Relaxed);
        }
    }

    // SAFETY: Single AtomicI64, all operations are atomic.
    unsafe impl Sync for Gauge {}

    /// Fixed-bucket histogram for latency distributions.
    /// Each bucket is a sharded Counter.
    pub struct Histogram<const N: usize> {
        pub buckets: [u64; N],
        pub counts: [Counter; N],
        pub sum: Counter,
        pub total: Counter,
    }

    impl<const N: usize> Histogram<N> {
        pub const fn new(buckets: [u64; N]) -> Self {
            Self {
                buckets,
                counts: [const { Counter::new() }; N],
                sum: Counter::new(),
                total: Counter::new(),
            }
        }

        pub fn observe(&self, value_ns: u64) {
            let idx = self
                .buckets
                .iter()
                .position(|&b| value_ns <= b)
                .unwrap_or(N - 1);
            self.counts[idx].inc();
            self.sum.add(value_ns);
            self.total.inc();
        }

        pub fn mean_ns(&self) -> u64 {
            self.sum.read().checked_div(self.total.read()).unwrap_or(0)
        }
    }

    // SAFETY: Histogram is composed of Sync Counters.
    unsafe impl<const N: usize> Sync for Histogram<N> {}
}

// ---------------------------------------------------------------------------
// Feature-disabled stubs (zero-sized, zero-cost)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "kernel-metrics"))]
mod disabled {
    pub struct Counter;
    impl Counter {
        pub const fn new() -> Self {
            Self
        }
        #[inline(always)]
        pub fn inc(&self) {}
        #[inline(always)]
        pub fn add(&self, _n: u64) {}
        pub fn read(&self) -> u64 {
            0
        }
    }
    unsafe impl Sync for Counter {}

    pub struct Gauge;
    impl Gauge {
        pub const fn new() -> Self {
            Self
        }
        pub fn set(&self, _val: i64) {}
        pub fn get(&self) -> i64 {
            0
        }
        pub fn inc(&self) {}
        pub fn dec(&self) {}
    }
    unsafe impl Sync for Gauge {}

    pub struct Histogram<const N: usize>;
    impl<const N: usize> Histogram<N> {
        pub const fn new(_buckets: [u64; N]) -> Self {
            Self
        }
        pub fn observe(&self, _value_ns: u64) {}
        pub fn mean_ns(&self) -> u64 {
            0
        }
    }
    unsafe impl<const N: usize> Sync for Histogram<N> {}
}

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

#[cfg(not(feature = "kernel-metrics"))]
pub use disabled::*;
#[cfg(feature = "kernel-metrics")]
pub use enabled::*;

// ---------------------------------------------------------------------------
// IPC round-trip histogram bucket boundaries (nanoseconds)
// ---------------------------------------------------------------------------

/// Standard latency histogram: <1us, <5us, <10us, <50us, <100us, <500us, <1ms, >1ms
#[allow(dead_code)]
const LATENCY_BUCKETS: [u64; 8] = [
    1_000,
    5_000,
    10_000,
    50_000,
    100_000,
    500_000,
    1_000_000,
    u64::MAX,
];

#[allow(dead_code)]
const MAX_SYSCALLS: usize = 32;

// ---------------------------------------------------------------------------
// Kernel metrics registry (observability.md §3.5)
// ---------------------------------------------------------------------------

/// Central metrics registry. BSS-allocated, zero-initialized, always available.
#[allow(dead_code)]
pub struct KernelMetrics {
    // Memory
    pub mm_page_alloc: Counter,
    pub mm_page_free: Counter,
    pub mm_slab_alloc: Counter,
    pub mm_slab_free: Counter,
    pub mm_slab_oom: Counter,
    pub mm_buddy_split: Counter,
    pub mm_buddy_coalesce: Counter,
    pub mm_free_pages: Gauge,
    pub mm_kernel_free: Gauge,
    pub mm_user_free: Gauge,

    // Scheduler (Phase 3 M11)
    pub sched_context_switch: Counter,
    pub sched_switch_latency_ns: Histogram<8>,
    pub sched_runqueue_depth: [Gauge; MAX_CORES],
    pub sched_idle_ticks: Counter,

    // IPC (Phase 3 M11-M12)
    pub ipc_send: Counter,
    pub ipc_recv: Counter,
    pub ipc_call: Counter,
    pub ipc_direct_switch: Counter,
    pub ipc_roundtrip_ns: Histogram<8>,
    pub ipc_timeout: Counter,
    pub ipc_cap_denied: Counter,

    // Interrupts
    pub irq_total: Counter,
    pub irq_timer: Counter,
    pub irq_uart: Counter,
    pub irq_spurious: Counter,

    // Syscalls (Phase 3)
    pub syscall_total: Counter,
    pub syscall_by_nr: [Counter; MAX_SYSCALLS],

    // TLB
    pub tlb_flush_all: Counter,
    pub tlb_flush_page: Counter,
    pub tlb_flush_asid: Counter,
}

impl KernelMetrics {
    const fn new() -> Self {
        Self {
            mm_page_alloc: Counter::new(),
            mm_page_free: Counter::new(),
            mm_slab_alloc: Counter::new(),
            mm_slab_free: Counter::new(),
            mm_slab_oom: Counter::new(),
            mm_buddy_split: Counter::new(),
            mm_buddy_coalesce: Counter::new(),
            mm_free_pages: Gauge::new(),
            mm_kernel_free: Gauge::new(),
            mm_user_free: Gauge::new(),

            sched_context_switch: Counter::new(),
            sched_switch_latency_ns: Histogram::new(LATENCY_BUCKETS),
            sched_runqueue_depth: [const { Gauge::new() }; MAX_CORES],
            sched_idle_ticks: Counter::new(),

            ipc_send: Counter::new(),
            ipc_recv: Counter::new(),
            ipc_call: Counter::new(),
            ipc_direct_switch: Counter::new(),
            ipc_roundtrip_ns: Histogram::new(LATENCY_BUCKETS),
            ipc_timeout: Counter::new(),
            ipc_cap_denied: Counter::new(),

            irq_total: Counter::new(),
            irq_timer: Counter::new(),
            irq_uart: Counter::new(),
            irq_spurious: Counter::new(),

            syscall_total: Counter::new(),
            syscall_by_nr: [const { Counter::new() }; MAX_SYSCALLS],

            tlb_flush_all: Counter::new(),
            tlb_flush_page: Counter::new(),
            tlb_flush_asid: Counter::new(),
        }
    }
}

// SAFETY: All fields are atomic or composed of atomics.
unsafe impl Sync for KernelMetrics {}

/// Global metrics instance. BSS-allocated.
#[allow(dead_code)]
pub static METRICS: KernelMetrics = KernelMetrics::new();
