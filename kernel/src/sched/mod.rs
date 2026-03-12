//! Scheduler: per-CPU run queues, context switch, timer-driven preemption.
//!
//! 4-class scheduling (RT, Interactive, Normal, Idle) with simple FIFO
//! per class. Phase 3 uses FixedQueue arrays; full EDF/WFQ comes later.
//! Per scheduler.md §3–4, §10.2.

mod init;
mod scheduler;

use core::sync::atomic::AtomicBool;

use crate::mm::buddy::PAGE_SIZE;
use crate::smp::MAX_CORES;
use crate::task::{SchedulerClass, Thread, ThreadId, MAX_THREADS};
use shared::FixedQueue;
use spin::Mutex;

// Re-export public API from submodules.
pub use init::{init, start, try_load_balance};
pub use scheduler::{
    block_current, check_preemption, enter_scheduler, thread_yield, timer_tick, unblock,
};

// Re-export time slice constants from shared for use in submodules.
use shared::default_slice;

/// Nanoseconds per tick (1ms = 1_000_000 ns).
const NS_PER_TICK: u64 = 1_000_000;

// ---------------------------------------------------------------------------
// Per-CPU RunQueue (uses shared::FixedQueue<T, N>)
// ---------------------------------------------------------------------------

/// Type alias for scheduler queue (ThreadId elements, MAX_THREADS capacity).
type SchedQueue = FixedQueue<ThreadId, MAX_THREADS>;

/// Per-CPU run queue with 4 scheduling classes.
pub(crate) struct RunQueue {
    rt: SchedQueue,
    interactive: SchedQueue,
    pub(crate) normal: SchedQueue,
    idle: SchedQueue,
}

impl RunQueue {
    const fn new() -> Self {
        Self {
            rt: FixedQueue::new(),
            interactive: FixedQueue::new(),
            normal: FixedQueue::new(),
            idle: FixedQueue::new(),
        }
    }

    pub(crate) fn enqueue(&mut self, tid: ThreadId, class: SchedulerClass) {
        match class {
            SchedulerClass::RealTime => self.rt.push_back(tid),
            SchedulerClass::Interactive => self.interactive.push_back(tid),
            SchedulerClass::Normal => self.normal.push_back(tid),
            SchedulerClass::Idle => self.idle.push_back(tid),
        };
    }

    /// Pick next thread: RT → Interactive → Normal → Idle.
    pub(crate) fn pick_next(&mut self) -> Option<ThreadId> {
        if let Some(tid) = self.rt.pop_front() {
            return Some(tid);
        }
        if let Some(tid) = self.interactive.pop_front() {
            return Some(tid);
        }
        if let Some(tid) = self.normal.pop_front() {
            return Some(tid);
        }
        self.idle.pop_front()
    }

    pub(crate) fn total_depth(&self) -> usize {
        self.rt.len() + self.interactive.len() + self.normal.len() + self.idle.len()
    }
}

// ---------------------------------------------------------------------------
// Global scheduler state
// ---------------------------------------------------------------------------

/// Per-CPU run queues. Lock ordering: ascending CPU index.
static RUN_QUEUES: [Mutex<RunQueue>; MAX_CORES] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const RQ: Mutex<RunQueue> = Mutex::new(RunQueue::new());
    [RQ; MAX_CORES]
};

/// Enqueue a thread on a specific CPU's run queue.
pub fn enqueue_on_cpu(cpu: usize, tid: ThreadId, class: SchedulerClass) {
    RUN_QUEUES[cpu].lock().enqueue(tid, class);
}

/// Re-entrancy guard per CPU. Prevents nested schedule() calls from
/// timer tick while already inside the scheduler.
static IN_SCHEDULER: [AtomicBool; MAX_CORES] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const F: AtomicBool = AtomicBool::new(false);
    [F; MAX_CORES]
};

/// Scheduler initialization complete flag. Secondary cores wait for this
/// before attempting to pick threads from their run queues.
static SCHED_READY: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Thread allocation helper
// ---------------------------------------------------------------------------

/// Allocate a thread slot in the global THREAD_TABLE. Returns the index.
pub fn allocate_thread(thread: Thread) -> Option<usize> {
    let mut table = crate::task::THREAD_TABLE.lock();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(thread);
            return Some(i);
        }
    }
    None
}

/// Stack order: order 3 = 8 pages = 32 KiB per thread stack.
const STACK_ORDER: usize = 3;
/// Stack size in bytes (2^STACK_ORDER * PAGE_SIZE).
pub const STACK_SIZE: usize = (1 << STACK_ORDER) * PAGE_SIZE;

/// Allocate a kernel stack from the frame allocator.
/// Returns the physical base address.
pub fn alloc_kernel_stack() -> usize {
    let mut guard = crate::mm::frame::FRAME_ALLOC.lock();
    if let Some(fa) = guard.as_mut() {
        // SAFETY: Frame allocator is initialized and pools are configured by init_memory().
        // The returned physical address is valid RAM in the kernel pool.
        // Caller converts to virtual address before use as stack pointer.
        unsafe { fa.alloc_pages(shared::Pool::Kernel, STACK_ORDER) }
    } else {
        // SAFETY: Buddy allocator is initialized during early boot (init_memory).
        // Returns a physical page address from the kernel memory region.
        // Caller converts to virtual address before use as stack pointer.
        unsafe { crate::mm::buddy::BUDDY.lock().alloc_pages(STACK_ORDER) }
    }
    .expect("Failed to allocate kernel thread stack")
}

/// Convert a physical address to a virtual address via the direct map.
#[inline]
pub fn phys_to_virt(phys: usize) -> usize {
    crate::arch::aarch64::mmu::DIRECT_MAP_BASE + phys
}
