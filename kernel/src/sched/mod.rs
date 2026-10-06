//! Scheduler: per-CPU run queues, context switch, timer-driven preemption.
//!
//! 4-class scheduling (RT, Interactive, Normal, Idle) with simple FIFO
//! per class. Phase 3 uses FixedQueue arrays; full EDF/WFQ comes later.
//! Per scheduler.md §3–4, §10.2.

mod init;
mod scheduler;

use core::sync::atomic::AtomicBool;

use crate::mm::buddy::PAGE_SIZE;
use crate::observability::tripwire::{self, ScanLock};
use crate::smp::MAX_CORES;
use crate::sync::IrqSpinLock;
use crate::task::{SchedulerClass, Thread, ThreadId, MAX_THREADS};
use shared::lock::LockClass;
use shared::tripwire::SlotState;
use shared::FixedQueue;

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

    /// Queue `tid` in its class's queue. A full queue drops it, which the
    /// tripwire counts (`enqfull`).
    pub(crate) fn enqueue(&mut self, tid: ThreadId, class: SchedulerClass) {
        let queued = match class {
            SchedulerClass::RealTime => self.rt.push_back(tid),
            SchedulerClass::Interactive => self.interactive.push_back(tid),
            SchedulerClass::Normal => self.normal.push_back(tid),
            SchedulerClass::Idle => self.idle.push_back(tid),
        };
        if !queued {
            tripwire::bump(shared::tripwire::Key::Enqfull, 0);
        }
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

    /// Visit every queued thread with the class of the queue it is in, in
    /// `pick_next` order (RT, Interactive, Normal, Idle; front to back),
    /// without changing any queue. For the heartbeat scan, under the lock.
    #[inline(always)]
    pub(crate) fn for_each(&self, mut f: impl FnMut(ThreadId, SchedulerClass)) {
        for tid in self.rt.iter() {
            f(tid, SchedulerClass::RealTime);
        }
        for tid in self.interactive.iter() {
            f(tid, SchedulerClass::Interactive);
        }
        for tid in self.normal.iter() {
            f(tid, SchedulerClass::Normal);
        }
        for tid in self.idle.iter() {
            f(tid, SchedulerClass::Idle);
        }
    }
}

// ---------------------------------------------------------------------------
// Global scheduler state
// ---------------------------------------------------------------------------

/// Per-CPU run queues. Lock ordering: ascending CPU index. Each entry is an
/// IRQ-class lock (`sync::IrqSpinLock`) that reports its CPU as its index.
static RUN_QUEUES: [IrqSpinLock<RunQueue>; MAX_CORES] = {
    let mut queues = [const { IrqSpinLock::new(LockClass::RunQueues, RunQueue::new()) }; MAX_CORES];
    let mut cpu = 0;
    while cpu < MAX_CORES {
        queues[cpu].set_index(cpu as u8);
        cpu += 1;
    }
    queues
};

/// Visit the lock words of the per-CPU run queues, for
/// `sync::held_by_stream`.
pub(crate) fn irq_lock_words(f: &mut impl FnMut(u64)) {
    for queue in RUN_QUEUES.iter() {
        f(queue.owner_word());
    }
}

// ---------------------------------------------------------------------------
// Heartbeat scan A: run queues and the thread table (crash-fix step 1b)
// ---------------------------------------------------------------------------

/// Phase 1 of the heartbeat scans (`observability::tripwire`): snapshot the
/// run queues, the current threads and every thread slot in one consistent
/// hold, and hand them to the tripwire's scan-A accumulators.
///
/// Takes all [`MAX_CORES`] run queues in ascending CPU order and then
/// THREAD_TABLE, the only order the scheduler and the balancer use, and
/// holds them all together: the balancer moves a thread while it holds both
/// of its queues, so a queue-by-queue walk could miss it or see it twice.
/// Every lock is taken with `try_lock_quiet`, so the scan never waits, never
/// panics and counts nothing in the lock keys. The first busy lock ends the
/// scan with every guard released ([`ScanLock::Busy`], or
/// [`ScanLock::OwnStream`] when CPU 0's interrupted stream holds it).
///
/// Under THREAD_TABLE it reads `CURRENT_TID[k]` for the `ncpu` online CPUs
/// instead of locking `CURRENT_THREAD[k]`: `note_dispatch` writes it only
/// under THREAD_TABLE, so the two are equal here.
///
/// CPU 0's timer IRQ only (IRQs masked). Holds stay short: the tripwire
/// records the longest in `scanhold1`.
#[inline(never)]
#[deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
pub(crate) fn scan_snapshot(ncpu: usize) -> ScanLock {
    scan_queue_level(0, ncpu)
}

/// Hold run queue `cpu`, record its threads, and recurse to the next queue
/// (THREAD_TABLE after the last). The guard is dropped only after the deeper
/// levels return, so all locks are held together, and released in reverse
/// order of acquisition.
#[inline(never)]
#[deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
fn scan_queue_level(cpu: usize, ncpu: usize) -> ScanLock {
    let Some(queue) = RUN_QUEUES.get(cpu) else {
        return scan_thread_table(ncpu);
    };
    let Some(guard) = queue.try_lock_quiet() else {
        return tripwire::scan_lock_busy(queue.owner_word());
    };
    guard.for_each(tripwire::scan_note_queued);
    let result = scan_queue_level(cpu.wrapping_add(1), ncpu);
    drop(guard);
    result
}

/// The innermost level of [`scan_snapshot`]: with every run queue held, hold
/// THREAD_TABLE and record the current threads and each slot's state.
#[inline(never)]
#[deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
fn scan_thread_table(ncpu: usize) -> ScanLock {
    let Some(table) = crate::task::THREAD_TABLE.try_lock_quiet() else {
        return tripwire::scan_lock_busy(crate::task::THREAD_TABLE.owner_word());
    };
    tripwire::scan_note_currents(ncpu);
    // A range, not `enumerate()`, whose counter carries an overflow-check
    // panic in the dev build.
    for slot in 0..MAX_THREADS {
        let thread = table.get(slot).and_then(Option::as_ref);
        let state = SlotState::of(thread.map(|t| &t.sched.state));
        tripwire::scan_note_slot(slot as u32, state);
    }
    drop(table);
    ScanLock::Done
}

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
///
/// Resets the slot's tripwire stamps (last CPU, last run, wake in flight)
/// under THREAD_TABLE before it fills the slot.
pub fn allocate_thread(thread: Thread) -> Option<usize> {
    let mut table = crate::task::THREAD_TABLE.lock();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            tripwire::reset_thread_stamps(i);
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
