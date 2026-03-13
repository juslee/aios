//! Scheduler initialization, thread entries, and load balancer.
//!
//! Creates idle threads (one per CPU), test threads, and the load balancer
//! that migrates threads between CPU run queues.
//! Per scheduler.md §3, §10.2.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::exceptions;
use crate::arch::aarch64::timer::{NEED_RESCHED, TICK_COUNT};
use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId, THREAD_TABLE};

use super::{
    alloc_kernel_stack, allocate_thread, phys_to_virt, scheduler::thread_yield, RUN_QUEUES,
    SCHED_READY, STACK_SIZE,
};
use shared::{IDLE_SLICE_NS, NORMAL_SLICE_NS};

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
        crate::observability::metrics::METRICS
            .sched_idle_ticks
            .inc();

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
// Load balancer — called from timer tick handler
// ---------------------------------------------------------------------------

/// Attempt to balance load across CPU run queues by migrating a thread
/// from the most loaded CPU to the least loaded CPU.
///
/// Called every 4 ticks from timer_tick_handler. Lock ordering: ascending
/// CPU index to avoid deadlock.
pub fn try_load_balance() {
    let online = crate::smp::online_cpus();
    if online < 2 {
        return;
    }

    // Find most loaded and least loaded CPUs.
    let mut max_cpu = 0;
    let mut max_depth = 0;
    let mut min_cpu = 0;
    let mut min_depth = usize::MAX;

    for (cpu, rq_lock) in RUN_QUEUES.iter().enumerate().take(online) {
        // Use try_lock to avoid contention from IRQ context.
        let depth = match rq_lock.try_lock() {
            Some(rq) => rq.total_depth(),
            None => continue,
        };
        if depth > max_depth {
            max_depth = depth;
            max_cpu = cpu;
        }
        if depth < min_depth {
            min_depth = depth;
            min_cpu = cpu;
        }
    }

    // Only migrate if difference > 1.
    if max_depth <= min_depth + 1 || max_cpu == min_cpu {
        return;
    }

    // Lock in ascending CPU order to prevent deadlock.
    let (first, second) = if max_cpu < min_cpu {
        (max_cpu, min_cpu)
    } else {
        (min_cpu, max_cpu)
    };

    let mut rq_first = match RUN_QUEUES[first].try_lock() {
        Some(rq) => rq,
        None => return,
    };
    let mut rq_second = match RUN_QUEUES[second].try_lock() {
        Some(rq) => rq,
        None => return,
    };

    // Determine source and destination from the locked queues.
    let (src, dst) = if first == max_cpu {
        (&mut *rq_first, &mut *rq_second)
    } else {
        (&mut *rq_second, &mut *rq_first)
    };

    // Try to migrate from Normal queue (most common, least latency-sensitive).
    if let Some(tid) = src.normal.pop_front() {
        // Check affinity before migrating.
        let can_migrate = {
            let table = THREAD_TABLE.lock();
            table[tid.0 as usize]
                .as_ref()
                .map(|t| t.sched.affinity.contains(min_cpu))
                .unwrap_or(false)
        };
        if can_migrate {
            dst.normal.push_back(tid);
            crate::kinfo!(
                Sched,
                "Load balance: migrated tid={} from CPU {} to CPU {}",
                tid.0,
                max_cpu,
                min_cpu
            );
        } else {
            // Put it back — thread is pinned.
            src.normal.push_back(tid);
        }
    }
}
