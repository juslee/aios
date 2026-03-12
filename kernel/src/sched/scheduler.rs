//! Core scheduling functions: schedule(), enter_scheduler(), timer_tick(),
//! preemption check, voluntary yield, and block/unblock operations.
//!
//! Per scheduler.md §3–4, §10.2.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::exceptions;
use crate::arch::aarch64::timer::NEED_RESCHED;
use crate::observability::metrics::METRICS;
use crate::task::{ThreadContext, ThreadId, ThreadState, CURRENT_THREAD, THREAD_TABLE};

use super::{default_slice, IN_SCHEDULER, MAX_CORES, NS_PER_TICK, RUN_QUEUES, SCHED_READY};

extern "C" {
    fn save_context(ctx: *mut ThreadContext);
    fn restore_context(ctx: *const ThreadContext) -> !;
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
