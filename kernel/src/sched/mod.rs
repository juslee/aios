//! Scheduler: per-CPU run queues, context switch, timer-driven preemption.
//!
//! 4-class scheduling (RT, Interactive, Normal, Idle) with simple FIFO
//! per class. Phase 3 uses FixedQueue arrays; full EDF/WFQ comes later.
//! Per scheduler.md §3–4, §10.2.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::aarch64::exceptions;
use crate::arch::aarch64::timer::{NEED_RESCHED, TICK_COUNT};
use crate::mm::buddy::PAGE_SIZE;
use crate::observability::metrics::METRICS;
use crate::smp::MAX_CORES;
use crate::task::{
    CpuSet, SchedulerClass, Thread, ThreadContext, ThreadId, ThreadState, CURRENT_THREAD,
    MAX_THREADS, THREAD_TABLE,
};
use shared::FixedQueue;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Configuration (scheduler.md §10.2)
// ---------------------------------------------------------------------------

// Re-export time slice constants from shared for use in this module.
use shared::{default_slice, IDLE_SLICE_NS, NORMAL_SLICE_NS};

/// Nanoseconds per tick (1ms = 1_000_000 ns).
const NS_PER_TICK: u64 = 1_000_000;

// ---------------------------------------------------------------------------
// Per-CPU RunQueue (uses shared::FixedQueue<T, N>)
// ---------------------------------------------------------------------------

/// Type alias for scheduler queue (ThreadId elements, MAX_THREADS capacity).
type SchedQueue = FixedQueue<ThreadId, MAX_THREADS>;

/// Per-CPU run queue with 4 scheduling classes.
struct RunQueue {
    rt: SchedQueue,
    interactive: SchedQueue,
    normal: SchedQueue,
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

    fn enqueue(&mut self, tid: ThreadId, class: SchedulerClass) {
        match class {
            SchedulerClass::RealTime => self.rt.push_back(tid),
            SchedulerClass::Interactive => self.interactive.push_back(tid),
            SchedulerClass::Normal => self.normal.push_back(tid),
            SchedulerClass::Idle => self.idle.push_back(tid),
        };
    }

    /// Pick next thread: RT → Interactive → Normal → Idle.
    fn pick_next(&mut self) -> Option<ThreadId> {
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

    fn total_depth(&self) -> usize {
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
// External assembly functions
// ---------------------------------------------------------------------------

extern "C" {
    fn save_context(ctx: *mut ThreadContext);
    fn restore_context(ctx: *const ThreadContext) -> !;
}

// Time slice helper: uses shared::default_slice()

// ---------------------------------------------------------------------------
// Thread allocation helper
// ---------------------------------------------------------------------------

/// Allocate a thread slot in the global THREAD_TABLE. Returns the index.
pub fn allocate_thread(thread: Thread) -> Option<usize> {
    let mut table = THREAD_TABLE.lock();
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

// ---------------------------------------------------------------------------
// Scheduler initialization
// ---------------------------------------------------------------------------

/// Initialize the scheduler: create idle threads (one per online CPU) and
/// test threads, then enqueue everything.
pub fn init() {
    let online = crate::smp::online_cpus();
    crate::kinfo!(Sched, "Initializing scheduler ({} CPUs)", online);

    // Create one idle thread per online CPU.
    #[allow(clippy::needless_range_loop)]
    for cpu in 0..online {
        let tid = ThreadId((cpu as u32) | 0x8000_0000); // High bit = idle thread
        let stack_phys = alloc_kernel_stack();
        let stack_virt_top = phys_to_virt(stack_phys) + STACK_SIZE;

        let mut thread = Thread::new_kernel(
            tid,
            b"idle",
            idle_thread_entry as *const () as usize,
            stack_phys,
        );
        // Override: Idle class, this CPU only.
        thread.sched.class = SchedulerClass::Idle;
        thread.sched.effective_class = SchedulerClass::Idle;
        thread.sched.priority = 0;
        thread.sched.effective_priority = 0;
        thread.sched.affinity = CpuSet::single(cpu);
        thread.sched.time_slice_remaining = IDLE_SLICE_NS;
        // Fix stack pointer to virtual address.
        thread.context.sp = stack_virt_top as u64;

        let idx = allocate_thread(thread).expect("thread table full for idle");
        RUN_QUEUES[cpu]
            .lock()
            .enqueue(ThreadId(idx as u32), SchedulerClass::Idle);
    }

    // Create test threads that prove multi-core context switching works.
    for i in 0..4u32 {
        let tid = ThreadId(0x100 + i);
        let stack_phys = alloc_kernel_stack();
        let stack_virt_top = phys_to_virt(stack_phys) + STACK_SIZE;

        let name = match i {
            0 => b"test-A\0\0\0\0\0\0\0\0\0\0",
            1 => b"test-B\0\0\0\0\0\0\0\0\0\0",
            2 => b"test-C\0\0\0\0\0\0\0\0\0\0",
            _ => b"test-D\0\0\0\0\0\0\0\0\0\0",
        };
        let mut thread = Thread::new_kernel(
            tid,
            name,
            test_thread_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.time_slice_remaining = NORMAL_SLICE_NS;
        thread.sched.affinity = CpuSet::all();
        // Fix stack pointer to virtual address.
        thread.context.sp = stack_virt_top as u64;
        // Pass thread index in x19 (callee-saved, restored by restore_context).
        thread.context.gp_regs[19] = i as u64;

        let idx = allocate_thread(thread).expect("thread table full for test");
        // Spread test threads across CPUs.
        let target_cpu = (i as usize) % online;
        RUN_QUEUES[target_cpu]
            .lock()
            .enqueue(ThreadId(idx as u32), SchedulerClass::Normal);
    }

    crate::kinfo!(Sched, "Created {} idle + 4 test threads", online);

    // NOTE: Do NOT set SCHED_READY here. Call sched::start() after all
    // initialization (ipc::init(), etc.) is complete. This prevents
    // secondary cores from starting the scheduler and starving the boot
    // CPU's THREAD_TABLE access during init.
}

/// Signal secondary cores to start scheduling.
///
/// Must be called after all initialization that touches THREAD_TABLE
/// (sched::init, ipc::init, etc.) is complete. Secondary cores are
/// parked in enter_scheduler() waiting on SCHED_READY.
pub fn start() {
    SCHED_READY.store(true, Ordering::Release);
    // Wake parked secondary cores (they're in wfe loops).
    // SAFETY: sev is a hint instruction, safe at EL1.
    unsafe { core::arch::asm!("sev") };
    crate::kinfo!(Sched, "Scheduler started — secondary cores released");
}

/// Validate that a ThreadContext has a non-zero pc (entry point) before
/// restore_context. Catches context corruption early.
fn assert_valid_ctx(ctx: *const ThreadContext, tid: ThreadId) {
    // SAFETY: ctx was just obtained from a locked THREAD_TABLE entry.
    let pc = unsafe { (*ctx).pc };
    let sp = unsafe { (*ctx).sp };
    if pc == 0 {
        crate::kerror!(
            Sched,
            "BUG: restore_context pc=0 for tid={} sp={:#x}",
            tid.0,
            sp
        );
        crate::observability::drain_logs();
        panic!("restore_context with pc=0");
    }
}

// ---------------------------------------------------------------------------
// Scheduler entry point (called once per CPU, never returns)
// ---------------------------------------------------------------------------

/// Enter the scheduler loop on the current CPU. Called from kernel_main
/// (boot CPU) and secondary_main (secondary CPUs) after init.
///
/// Secondary cores may call this before init() completes — they spin
/// on SCHED_READY until threads are available.
pub fn enter_scheduler() -> ! {
    let cpu = exceptions::core_id() as usize;

    // Wait for scheduler initialization to complete.
    // Secondary cores have IRQs masked at this point (deferred from smp.rs)
    // to avoid exclusive monitor traffic starving boot CPU locks during init.
    while !SCHED_READY.load(Ordering::Acquire) {
        // SAFETY: wfe is a hint instruction, safe at EL1.
        unsafe { core::arch::asm!("wfe") };
    }

    // Unmask IRQs now that initialization is complete.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. GIC + timer are initialized.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    crate::kinfo!(Sched, "CPU {} entering scheduler", cpu);

    // Try to pick and run a thread. Loop on failure (shouldn't happen
    // after init, since every CPU has at least an idle thread).
    loop {
        // Mask IRQs before touching THREAD_TABLE / CURRENT_THREAD.
        // A timer IRQ while holding these locks would deadlock (same-core
        // re-entrant spinlock).
        // SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
        unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

        let tid = {
            let mut rq = RUN_QUEUES[cpu].lock();
            rq.pick_next()
        };

        if let Some(tid) = tid {
            let mut table = THREAD_TABLE.lock();
            if let Some(thread) = &mut table[tid.0 as usize] {
                // Mark thread as Running so schedule() handles it correctly.
                thread.sched.state = ThreadState::Running;
                // Set this thread as current on this CPU.
                *CURRENT_THREAD[cpu].lock() = Some(tid);
                let ctx_ptr = &thread.context as *const ThreadContext;
                drop(table);

                assert_valid_ctx(ctx_ptr, tid);

                // SAFETY: The ThreadContext was set up by Thread::new_kernel with
                // a valid entry point and stack. restore_context will load callee-saved
                // regs, set SP, and branch to the entry function. IRQs remain masked;
                // the thread entry or resume path will unmask when ready.
                unsafe { restore_context(ctx_ptr) };
            } else {
                drop(table);
            }
        }

        // No thread yet — unmask IRQs and wait for next timer interrupt.
        // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
        // SAFETY: wfe is a hint instruction, safe at EL1.
        unsafe { core::arch::asm!("wfe") };
    }
}

// ---------------------------------------------------------------------------
// Timer tick — called from timer_tick_handler
// ---------------------------------------------------------------------------

/// Called from the timer tick handler. Decrements the current thread's
/// time slice and triggers a reschedule if expired.
pub fn timer_tick(cpu: usize) {
    // Decrement current thread's time slice.
    // Use try_lock to avoid deadlock: if schedule() or enter_scheduler()
    // on this core already holds THREAD_TABLE/CURRENT_THREAD and a timer
    // IRQ fires, a blocking lock() would deadlock (same-core spinlock).
    let current = match CURRENT_THREAD[cpu].try_lock() {
        Some(guard) => *guard,
        None => return, // Lock held on this core — skip this tick.
    };
    if let Some(tid) = current {
        if let Some(mut table) = THREAD_TABLE.try_lock() {
            if let Some(thread) = &mut table[tid.0 as usize] {
                let decrement = NS_PER_TICK.min(thread.sched.time_slice_remaining);
                thread.sched.time_slice_remaining -= decrement;
            }
        }
        // If try_lock fails, we skip the time slice decrement this tick.
        // The next tick will catch it — missing one 1ms tick is harmless.
    }

    // Update run queue depth metrics.
    #[cfg(feature = "kernel-metrics")]
    {
        if let Some(rq) = RUN_QUEUES[cpu].try_lock() {
            METRICS.sched_runqueue_depth[cpu].set(rq.total_depth() as i64);
        }
    }
}

// ---------------------------------------------------------------------------
// schedule() — the core scheduling function
// ---------------------------------------------------------------------------

/// Main scheduling function. Called from:
/// - Timer tick return path (preemption)
/// - thread_yield() (voluntary)
/// - block_current() (blocking IPC/sleep)
///
/// Must be called with IRQs masked (DAIF.I set).
pub fn schedule() {
    let cpu = exceptions::core_id() as usize;

    // Re-entrancy guard: skip if already in scheduler on this CPU.
    if IN_SCHEDULER[cpu].swap(true, Ordering::Acquire) {
        return;
    }

    // Clear preemption flag.
    NEED_RESCHED.store(false, Ordering::Relaxed);

    let current_tid = { *CURRENT_THREAD[cpu].lock() };

    // Re-enqueue current thread if it's still runnable and slice expired.
    if let Some(tid) = current_tid {
        let mut table = THREAD_TABLE.lock();
        if let Some(thread) = &mut table[tid.0 as usize] {
            if thread.sched.state == ThreadState::Running && thread.sched.time_slice_remaining == 0
            {
                // Time slice expired: reset and re-enqueue.
                thread.sched.time_slice_remaining = default_slice(thread.sched.effective_class);
                thread.sched.state = ThreadState::Runnable;
                let class = thread.sched.effective_class;
                drop(table);
                RUN_QUEUES[cpu].lock().enqueue(tid, class);
            } else if thread.sched.state == ThreadState::Running {
                // Still has time — keep running, no switch needed.
                IN_SCHEDULER[cpu].store(false, Ordering::Release);
                return;
            } else {
                // Thread blocked or dead — don't re-enqueue.
                drop(table);
            }
        } else {
            drop(table);
        }
    }

    // Pick next thread from run queue.
    let next_tid = {
        let mut rq = RUN_QUEUES[cpu].lock();
        rq.pick_next()
    };

    let next_tid = match next_tid {
        Some(t) => t,
        None => {
            // No thread available — re-enqueue current if runnable and continue.
            IN_SCHEDULER[cpu].store(false, Ordering::Release);
            return;
        }
    };

    // Same thread? No switch needed.
    if current_tid == Some(next_tid) {
        let mut table = THREAD_TABLE.lock();
        if let Some(thread) = &mut table[next_tid.0 as usize] {
            thread.sched.state = ThreadState::Running;
        }
        IN_SCHEDULER[cpu].store(false, Ordering::Release);
        return;
    }

    // Context switch: save current, restore next.
    {
        let mut table = THREAD_TABLE.lock();

        // Save current context.
        let old_ctx_ptr = if let Some(tid) = current_tid {
            if let Some(thread) = &mut table[tid.0 as usize] {
                if thread.sched.state == ThreadState::Running {
                    thread.sched.state = ThreadState::Runnable;
                }
                &mut thread.context as *mut ThreadContext
            } else {
                core::ptr::null_mut()
            }
        } else {
            core::ptr::null_mut()
        };

        // Set next thread as running.
        let next_ctx_ptr = if let Some(thread) = &mut table[next_tid.0 as usize] {
            thread.sched.state = ThreadState::Running;
            &thread.context as *const ThreadContext
        } else {
            IN_SCHEDULER[cpu].store(false, Ordering::Release);
            return;
        };

        // Update current thread tracking.
        *CURRENT_THREAD[cpu].lock() = Some(next_tid);

        // Drop table lock before the actual context switch.
        drop(table);

        // Increment context switch counter.
        METRICS.sched_context_switch.inc();

        // (debug tracing removed)

        // Perform the context switch.
        if !old_ctx_ptr.is_null() {
            // SAFETY: old_ctx_ptr points to the current thread's ThreadContext
            // in the THREAD_TABLE. save_context stores callee-saved regs and
            // returns. When this thread is later restored, execution resumes
            // right after save_context returns.
            unsafe { save_context(old_ctx_ptr) };

            // After save_context returns, we might be the OLD thread being
            // resumed later, OR we just saved and are about to restore the
            // new thread.
            //
            // Re-read CPU ID from hardware because if we were restored,
            // we might now be on a different core than when we saved.
            let actual_cpu = exceptions::core_id() as usize;
            let current_now = { *CURRENT_THREAD[actual_cpu].lock() };
            if current_now != Some(next_tid) {
                // We were restored as the old thread — schedule() is done for us.
                IN_SCHEDULER[actual_cpu].store(false, Ordering::Release);
                return;
            }

            // We're still the caller — switch to the next thread.
            IN_SCHEDULER[actual_cpu].store(false, Ordering::Release);
            // SAFETY: next_ctx_ptr points to the next thread's ThreadContext.
            // restore_context loads callee-saved regs, SP, and branches to
            // the saved PC. This never returns.
            assert_valid_ctx(next_ctx_ptr, next_tid);
            unsafe { restore_context(next_ctx_ptr) };
        } else {
            // No current thread (first schedule on this CPU).
            IN_SCHEDULER[cpu].store(false, Ordering::Release);
            // SAFETY: next_ctx_ptr is valid (checked above).
            assert_valid_ctx(next_ctx_ptr, next_tid);
            unsafe { restore_context(next_ctx_ptr) };
        }
    }
}

// ---------------------------------------------------------------------------
// Voluntary yield
// ---------------------------------------------------------------------------

/// Current thread voluntarily yields its remaining time slice.
pub fn thread_yield() {
    // Mask IRQs during schedule.
    // SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

    let cpu = exceptions::core_id() as usize;
    let current = { *CURRENT_THREAD[cpu].lock() };

    if let Some(tid) = current {
        let mut table = THREAD_TABLE.lock();
        if let Some(thread) = &mut table[tid.0 as usize] {
            // Reset time slice and mark runnable for re-enqueue.
            thread.sched.time_slice_remaining = 0;
            thread.sched.state = ThreadState::Running; // schedule() will transition
        }
        drop(table);
    }

    schedule();

    // Unmask IRQs after returning from schedule.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
}

// ---------------------------------------------------------------------------
// Block / Unblock
// ---------------------------------------------------------------------------

/// Block the current thread with the given state. Triggers a reschedule.
/// The thread is NOT re-enqueued; the unblock() caller must re-enqueue it.
pub fn block_current(new_state: ThreadState) {
    // Mask IRQs during schedule.
    // SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

    let cpu = exceptions::core_id() as usize;
    let current = { *CURRENT_THREAD[cpu].lock() };

    if let Some(tid) = current {
        let mut table = THREAD_TABLE.lock();
        if let Some(thread) = &mut table[tid.0 as usize] {
            thread.sched.state = new_state;
        }
        drop(table);
    }

    schedule();

    // Unmask IRQs after being unblocked and re-scheduled.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
}

/// Unblock a thread: set it to Runnable and enqueue it on a suitable CPU.
///
/// Masks IRQs internally because this function locks THREAD_TABLE and
/// RUN_QUEUES — both of which are also locked from the timer tick handler.
/// Without IRQ masking, a timer tick on the same core would deadlock.
///
/// Saves and restores DAIF state to avoid unmasking IRQs when called from
/// IRQ context (e.g., check_timeouts → wake_with_error → unblock).
pub fn unblock(tid: ThreadId) {
    // Save current DAIF state so we can restore it on exit.
    // SAFETY: Reading DAIF is a pure register read with no side effects.
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags))
    };
    let irqs_were_masked = (daif & (1 << 7)) != 0; // DAIF.I = bit 7

    // Mask IRQs to prevent timer_tick() deadlock on THREAD_TABLE/RUN_QUEUES.
    // SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

    let mut table = THREAD_TABLE.lock();
    let (class, affinity) = if let Some(thread) = &mut table[tid.0 as usize] {
        // Guard: only unblock threads that are actually blocked.
        // Prevents double-enqueue if a thread is already Running/Runnable.
        match thread.sched.state {
            ThreadState::Running | ThreadState::Runnable => {
                drop(table);
                if !irqs_were_masked {
                    // SAFETY: Restore IRQ state. Safe at EL1.
                    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
                }
                return;
            }
            _ => {}
        }
        thread.sched.state = ThreadState::Runnable;
        thread.sched.time_slice_remaining = default_slice(thread.sched.effective_class);
        (thread.sched.effective_class, thread.sched.affinity)
    } else {
        if !irqs_were_masked {
            // SAFETY: Restore IRQ state. Safe at EL1.
            unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
        }
        return;
    };
    drop(table);

    // Find a suitable CPU (prefer current CPU if allowed).
    let cpu = exceptions::core_id() as usize;
    let target = if affinity.contains(cpu) {
        cpu
    } else {
        // Find first allowed CPU.
        (0..MAX_CORES).find(|&c| affinity.contains(c)).unwrap_or(0)
    };

    RUN_QUEUES[target].lock().enqueue(tid, class);

    // Restore original IRQ masking state. Only unmask if IRQs were unmasked
    // on entry. This prevents unmasking IRQs inside an IRQ handler.
    if !irqs_were_masked {
        // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
    }
}

// ---------------------------------------------------------------------------
// Idle thread entry point
// ---------------------------------------------------------------------------

/// Idle thread: loops forever executing wfe. Timer interrupts wake it,
/// and if preemption is needed, yields to let another thread run.
fn idle_thread_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    loop {
        #[cfg(feature = "kernel-metrics")]
        METRICS.sched_idle_ticks.inc();

        // SAFETY: wfe is a hint instruction, safe at EL1.
        unsafe { core::arch::asm!("wfe") };

        // Check if preemption needed after wakeup.
        // Use thread_yield() which properly masks IRQs around schedule().
        if NEED_RESCHED.load(Ordering::Acquire) {
            thread_yield();
        }
    }
}

// ---------------------------------------------------------------------------
// Test thread entry point
// ---------------------------------------------------------------------------

/// Test thread: prints its ID and the core it's running on, yields a few
/// times, then loops. Proves multi-core context switching works.
fn test_thread_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    // Thread index passed in x19 (callee-saved, set in ThreadContext.gp_regs[19]).
    // restore_context restores x19-x30 from the context, so x19 has our index.
    let thread_idx: u64;
    // SAFETY: Reading x19 which was set up in the ThreadContext before first
    // restore_context. restore_context restores callee-saved registers.
    unsafe { core::arch::asm!("mov {}, x19", out(reg) thread_idx) };

    let names = [b'A', b'B', b'C', b'D'];
    let name = if (thread_idx as usize) < names.len() {
        names[thread_idx as usize]
    } else {
        b'?'
    };

    for iteration in 0..5u32 {
        let cpu = exceptions::core_id();
        let tick = TICK_COUNT.load(Ordering::Relaxed);
        crate::kinfo!(
            Sched,
            "Thread {} on core {} iter={} tick={}",
            name as char,
            cpu,
            iteration,
            tick
        );
        thread_yield();
    }

    // After test iterations, sleep in wfe loop (avoids THREAD_TABLE starvation).
    loop {
        // SAFETY: wfe is a hint instruction, safe at EL1.
        unsafe { core::arch::asm!("wfe") };
    }
}

// ---------------------------------------------------------------------------
// Preemption check — called from IRQ return path
// ---------------------------------------------------------------------------

/// Check if preemption is needed and call schedule() if so.
/// Called from timer tick handler after incrementing tick.
pub fn check_preemption() {
    let cpu = exceptions::core_id() as usize;
    if !NEED_RESCHED.load(Ordering::Acquire) {
        return;
    }
    if IN_SCHEDULER[cpu].load(Ordering::Relaxed) {
        return;
    }
    schedule();
}
