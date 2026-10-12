//! Core scheduling functions: schedule(), enter_scheduler(), timer_tick(),
//! preemption check, voluntary yield, and block/unblock operations.
//!
//! Per scheduler.md §3–4, §10.2.

use core::sync::atomic::Ordering;

use shared::tripwire::{Key, UnblockOutcome, WakeSource};
use shared::{block_action, wake_action, BlockAction, ProcessId, WaitState, WakeAction};

use crate::arch::aarch64::timer::NEED_RESCHED;
use crate::arch::aarch64::{daif, exceptions};
use crate::observability::metrics::METRICS;
use crate::observability::tripwire::{self, DispatchSite};
use crate::task::{ThreadContext, ThreadId, ThreadState, CURRENT_THREAD, THREAD_TABLE};

use super::{default_slice, wake, IN_SCHEDULER, MAX_CORES, NS_PER_TICK, RUN_QUEUES, SCHED_READY};

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

        // Pick until the pick is not Dead: a Dead thread is dropped from the
        // queue (pick_next popped it) and never dispatched.
        loop {
            let tid = {
                let mut rq = RUN_QUEUES[cpu].lock();
                rq.pick_next()
            };
            let Some(tid) = tid else {
                break;
            };
            let mut table = THREAD_TABLE.lock();
            if let Some(thread) = &mut table[tid.0 as usize] {
                if !thread.sched.state.may_become(&ThreadState::Running) {
                    drop(table);
                    continue;
                }
                // Mark thread as Running so schedule() handles it correctly.
                thread.sched.state = ThreadState::Running;
                // Set this thread as current on this CPU.
                *CURRENT_THREAD[cpu].lock() = Some(tid);
                let last_cpu = tripwire::note_dispatch(cpu, tid, DispatchSite::Enter);
                let ctx_ptr = &thread.context as *const ThreadContext;
                let stack_phys = thread.stack_phys;
                drop(table);

                // Count a saved PC or SP outside kernel text or the stack (N5).
                tripwire::check_restore(ctx_ptr, stack_phys, last_cpu);
                assert_valid_ctx(ctx_ptr, tid);
                // Count rsthold if this stream still holds a lock stamped
                // with the generation note_dispatch just started.
                crate::sync::note_restore();

                // SAFETY: The ThreadContext was set up by Thread::new_kernel with
                // a valid entry point and stack. restore_context will load callee-saved
                // regs, set SP, and branch to the entry function. IRQs remain masked;
                // the thread entry or resume path will unmask when ready.
                unsafe { restore_context(ctx_ptr) };
            }
            drop(table);
            break;
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

/// Which path called `schedule()`. Instrumentation only: the tripwire
/// counts switches committed from the IRQ return path (`irqsw`, `irqsw0`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The IRQ return path (`check_preemption`, timer-driven preemption).
    Irq,
    /// `thread_yield()`.
    Yield,
    /// `block_current()`.
    Block,
}

/// Main scheduling function. Called from (`origin`):
/// - Timer tick return path (preemption), [`Origin::Irq`]
/// - thread_yield() (voluntary), [`Origin::Yield`]
/// - block_current() (blocking IPC/sleep), [`Origin::Block`]
///
/// Must be called with IRQs masked (DAIF.I set).
pub fn schedule(origin: Origin) {
    let cpu = exceptions::core_id() as usize;

    // Re-entrancy guard: skip if already in scheduler on this CPU.
    if IN_SCHEDULER[cpu].swap(true, Ordering::Acquire) {
        tripwire::bump_masked(Key::Insched, 0);
        return;
    }

    // The caller's context label (thread, or the IRQ handler's preemption
    // check). A switch below relabels this CPU for the next thread; when this
    // thread is resumed, it restores its own label on the CPU it resumes on.
    let irq_ctx = tripwire::irq_ctx(cpu);

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
                // Thread blocked or dead — don't re-enqueue. A Runnable
                // current thread is left unqueued here too. From the IRQ
                // path that is the N1 orphan (a direct-switch receiver that
                // nothing queued), so only Origin::Irq counts n1. From
                // Origin::Block it is F4(3): a waker on another CPU saw the
                // Blocked state inside block_current's window and queued or
                // ran the thread, so it is not orphaned. Origin::Yield gets
                // here only for a Dead thread, whose Running write
                // thread_yield skips (Dead is terminal): it is dropped, not
                // re-enqueued, and counts nothing.
                if origin == Origin::Irq && thread.sched.state == ThreadState::Runnable {
                    tripwire::bump_masked(Key::N1, 0);
                }
                drop(table);
            }
        } else {
            drop(table);
        }
    }

    // Pick the next thread from the run queue. A picked Dead thread is
    // dropped (pick_next popped it) and the pick repeated, before any
    // Running write, note_repick, note_dispatch or CURRENT_THREAD write, so
    // a Dead thread never runs again and never shows in SWITCH_GEN or
    // CURRENT_TID. THREAD_TABLE is released before each re-pick takes
    // RUN_QUEUES again.
    let (next_tid, mut table) = loop {
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
                if !thread.sched.state.may_become(&ThreadState::Running) {
                    drop(table);
                    continue;
                }
                thread.sched.state = ThreadState::Running;
                tripwire::note_repick(next_tid);
            }
            IN_SCHEDULER[cpu].store(false, Ordering::Release);
            return;
        }

        let table = THREAD_TABLE.lock();
        if table[next_tid.0 as usize]
            .as_ref()
            .is_some_and(|thread| !thread.sched.state.may_become(&ThreadState::Running))
        {
            drop(table);
            continue;
        }
        break (next_tid, table);
    };

    // Context switch: save current, restore next. `table` is the
    // THREAD_TABLE hold in which the pick was found not Dead.
    {
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
        let (next_ctx_ptr, next_stack_phys) = if let Some(thread) = &mut table[next_tid.0 as usize]
        {
            thread.sched.state = ThreadState::Running;
            (&thread.context as *const ThreadContext, thread.stack_phys)
        } else {
            IN_SCHEDULER[cpu].store(false, Ordering::Release);
            return;
        };

        // Update current thread tracking.
        *CURRENT_THREAD[cpu].lock() = Some(next_tid);
        let next_last_cpu = tripwire::note_dispatch(cpu, next_tid, DispatchSite::Schedule);
        if origin == Origin::Irq {
            // A switch from the IRQ return path; irqsw0 when this CPU had no
            // current thread to save.
            let key = if old_ctx_ptr.is_null() {
                Key::Irqsw0
            } else {
                Key::Irqsw
            };
            tripwire::bump_masked(key, 0);
        }

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
                tripwire::set_irq_ctx(actual_cpu, irq_ctx);
                IN_SCHEDULER[actual_cpu].store(false, Ordering::Release);
                return;
            }

            // We're still the caller — switch to the next thread.
            IN_SCHEDULER[actual_cpu].store(false, Ordering::Release);
            tripwire::check_restore(next_ctx_ptr, next_stack_phys, next_last_cpu);
            assert_valid_ctx(next_ctx_ptr, next_tid);
            crate::sync::note_restore();
            // SAFETY: next_ctx_ptr points to the next thread's ThreadContext.
            // restore_context loads callee-saved regs, SP, and branches to
            // the saved PC. This never returns.
            unsafe { restore_context(next_ctx_ptr) };
        } else {
            // No current thread (first schedule on this CPU).
            IN_SCHEDULER[cpu].store(false, Ordering::Release);
            tripwire::check_restore(next_ctx_ptr, next_stack_phys, next_last_cpu);
            assert_valid_ctx(next_ctx_ptr, next_tid);
            crate::sync::note_restore();
            // SAFETY: next_ctx_ptr is valid (checked above).
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
            // Reset time slice and mark runnable for re-enqueue: schedule()
            // moves a Running thread to Runnable. A Dead thread stays Dead
            // (Dead is terminal), so schedule() drops it instead.
            thread.sched.time_slice_remaining = 0;
            if thread.sched.state.may_become(&ThreadState::Running) {
                thread.sched.state = ThreadState::Running;
            }
        }
        drop(table);
    }

    schedule(Origin::Yield);

    // Unmask IRQs after returning from schedule.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
}

// ---------------------------------------------------------------------------
// Block / Unblock
// ---------------------------------------------------------------------------

/// Block the current thread with the given state. Triggers a reschedule.
/// The thread is NOT re-enqueued; the unblock() caller must re-enqueue it.
///
/// Under THREAD_TABLE it swaps the thread's wait word to Idle
/// (`wake::take`) and applies `shared::block_action` to the state, the old
/// word and `new_state`:
///
/// - `AlreadyDead` (a process exit marked the thread): no state write and no
///   consume (the swap reset the word), then `schedule()`, which does not
///   re-enqueue a Dead thread, so it never runs again.
/// - `Exit` (`new_state` is Dead, `exit_current`): write Dead, then
///   `schedule()`. A pending token never makes an exit return.
/// - `ConsumeToken` (the word was Woken: a waker found this thread armed and
///   not yet blocked): no state write and no `schedule()`. It returns at
///   once, and the caller re-checks its condition.
/// - `Sleep` (Armed or Idle): write `new_state`, then `schedule()`.
///
/// Every case leaves through the one IRQ unmask at the end, so the token
/// adds no `DAIFClr` site.
pub fn block_current(new_state: ThreadState) {
    // Mask IRQs during schedule.
    // SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

    let cpu = exceptions::core_id() as usize;
    let current = { *CURRENT_THREAD[cpu].lock() };

    let mut consumed = false;
    if let Some(tid) = current {
        let mut table = THREAD_TABLE.lock();
        if let Some(thread) = &mut table[tid.0 as usize] {
            // The consume swap runs in this THREAD_TABLE critical section,
            // as every waker's token CAS does in unblock's, so the two are
            // totally ordered (sched::wake, "Memory ordering").
            let word = wake::take(tid);
            match block_action(&thread.sched.state, word, &new_state) {
                BlockAction::AlreadyDead => {}
                BlockAction::Exit => thread.sched.state = ThreadState::Dead,
                BlockAction::ConsumeToken => consumed = true,
                BlockAction::Sleep => thread.sched.state = new_state,
            }
        }
        drop(table);
    }

    if !consumed {
        schedule(Origin::Block);
    }

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
///
/// One decision under THREAD_TABLE, `shared::wake_action` of the target's
/// state and wait word:
///
/// - Dead: return, no token and no enqueue (Dead is terminal: a stray wake
///   never revives a thread a process exit killed).
/// - Running or Runnable and Armed (`LeaveToken`): compare-and-swap the word
///   to Woken (`wake::settle_wake`), with no state change and no enqueue.
///   The target's `block_current` consumes the token instead of blocking.
///   Runnable gets a token on purpose: an armed thread preempted by a tick
///   is Runnable and queued, and a thread `try_direct_switch` started is
///   Runnable while it runs.
/// - Running or Runnable with a token already pending (`TokenPending`), or
///   Idle (`Skip`, a stale wake outside any armed window): return.
/// - Blocked or Suspended (`Enqueue`): make it Runnable and queue it.
///
/// Every non-enqueue case leaves through the one early return, so the token
/// adds no `DAIFClr` site.
///
/// `src` names the caller for the tripwire counters (`ubrun`, `ubrbl`,
/// `ubdead`, `ubnone`, `ubtok`, `ubpend`, `ubmove` by source). The returned
/// outcome is what `unblock` did, with the target's call or receive phase
/// for the reply, send and call wakers; it is instrumentation only, and
/// callers that do not classify a wake ignore it. An out-of-range `tid` counts `badtid` and
/// `ubnone` and returns, like an empty slot.
pub fn unblock(tid: ThreadId, src: WakeSource) -> UnblockOutcome {
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
    let state = table
        .get(tid.0 as usize)
        .and_then(Option::as_ref)
        .map(|thread| &thread.sched.state);
    // The token step: leave the token for an Armed Running or Runnable
    // target. Its CAS runs in this THREAD_TABLE critical section, as the
    // block sites' consume swap does in theirs, so the two are totally
    // ordered (sched::wake, "Memory ordering"). `word` is the value the
    // decision stands on: wake_action(state, word) is the action taken.
    let word = state.map_or(WaitState::Idle, |state| wake::settle_wake(tid, state));
    // Count the outcome by source and snapshot the target's phase under this
    // hold, so the phase is read at the same instant as the state decided on.
    let outcome = tripwire::note_unblock(tid, src, state, word);
    let (class, affinity) =
        if let Some(thread) = table.get_mut(tid.0 as usize).and_then(Option::as_mut) {
            // Only a blocked or Suspended target is made Runnable and
            // queued. Dead, a token left or pending, and a stale wake of a
            // Running or Runnable target all return here.
            if wake_action(&thread.sched.state, word) != WakeAction::Enqueue {
                drop(table);
                if !irqs_were_masked {
                    // SAFETY: Restore IRQ state. Safe at EL1.
                    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
                }
                return outcome;
            }
            thread.sched.state = ThreadState::Runnable;
            thread.sched.time_slice_remaining = default_slice(thread.sched.effective_class);
            (thread.sched.effective_class, thread.sched.affinity)
        } else {
            if !irqs_were_masked {
                // SAFETY: Restore IRQ state. Safe at EL1.
                unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
            }
            return outcome;
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
    tripwire::note_unblock_target(tid, src, target);

    RUN_QUEUES[target].lock().enqueue(tid, class);

    // Restore original IRQ masking state. Only unmask if IRQs were unmasked
    // on entry. This prevents unmasking IRQs inside an IRQ handler.
    if !irqs_were_masked {
        // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
    }
    outcome
}

// ---------------------------------------------------------------------------
// Dead is terminal: exit, kill and probe (capability-lifetime ADR §7)
// ---------------------------------------------------------------------------

/// End the calling thread: block it as Dead, so it never runs again.
///
/// `block_current(Dead)` switches away and `schedule()` never re-enqueues a
/// Dead thread, nor dispatches one it picks. It cannot return: the switch
/// always finds a thread to run, because every CPU's idle thread is queued
/// whenever it is not running. A return means that invariant broke, so it
/// panics.
pub fn exit_current() -> ! {
    block_current(ThreadState::Dead);
    panic!("exit_current: a Dead thread was resumed");
}

/// Mark every thread of `pid` Dead, except the calling thread (exit step B),
/// and reset each victim's wait word, in the same masked walk. Returns the
/// victims as a mask (bit t = thread slot t): every thread of `pid`, the
/// caller included when it belongs to `pid`, for exit steps C and E, which
/// clean up after every victim.
///
/// The whole walk runs in one IRQ-masked section: the caller's tid is read
/// from `CURRENT_THREAD[core_id()]` after masking, so the thread cannot
/// migrate between the CPU read and the lock (crash-fix N4), and the
/// `THREAD_TABLE` hold cannot meet this CPU's own tick, whose `schedule()`
/// takes `THREAD_TABLE` too (a `lock re-entry` panic). `CURRENT_THREAD` is
/// released before `THREAD_TABLE` is taken; nothing nests.
///
/// The caller is left out so that a self-exit stays runnable until it calls
/// [`exit_current`]; a Dead caller would be switched off by its next tick
/// wherever it is, holding whatever lock it holds. A victim that is queued
/// stays queued and is dropped when its CPU picks it; a victim running on
/// another CPU keeps running until its next switch. Such a victim can still
/// arm after the reset (the transient exception to the wait-word invariant
/// until crash-fix step 8's safe point); its next `block_current` finds it
/// Dead (`AlreadyDead`) and resets the word again.
pub fn kill_process_threads(pid: ProcessId) -> u64 {
    daif::with_irqs_masked(|| {
        let cpu = exceptions::core_id() as usize;
        let caller = { *CURRENT_THREAD[cpu].lock() };
        let mut table = THREAD_TABLE.lock();
        let mut victims = 0u64;
        for (idx, slot) in table.iter_mut().enumerate() {
            if let Some(thread) = slot {
                if thread.owner_pid != Some(pid) {
                    continue;
                }
                // MAX_THREADS is 64, so every slot has a bit.
                victims |= 1u64 << idx;
                let victim = ThreadId(idx as u32);
                if caller != Some(victim) {
                    thread.sched.state = ThreadState::Dead;
                    wake::reset(victim);
                }
            }
        }
        victims
    })
}

/// The calling thread's tid and the pid of the process it belongs to, read
/// in one IRQ-masked section: `CURRENT_THREAD[core_id()]`, released, then
/// the thread's `owner_pid` under THREAD_TABLE. Masked, the thread cannot
/// migrate between the CPU read and the `CURRENT_THREAD` read (crash-fix N4),
/// so the tid is the caller's own, as `wake::prepare_block` requires (the
/// own-tid rule, ADR §8), and neither IRQ-class hold can meet this CPU's
/// tick. `None` when the CPU has no current thread or the thread has no
/// process. For the process syscalls (SVC 24 and 25).
pub fn current_thread_and_pid() -> Option<(ThreadId, ProcessId)> {
    daif::with_irqs_masked(|| {
        let cpu = exceptions::core_id() as usize;
        let tid = { *CURRENT_THREAD[cpu].lock() }?;
        let table = THREAD_TABLE.lock();
        let pid = table.get(tid.0 as usize)?.as_ref()?.owner_pid?;
        Some((tid, pid))
    })
}

/// What [`thread_probe`] read about a thread.
#[derive(Clone, Copy, Debug)]
pub struct ThreadProbe {
    /// The thread's state, `None` for an empty slot or an out-of-range tid.
    pub state: Option<ThreadState>,
    /// Whether any online CPU's run queue holds the thread.
    pub queued: bool,
}

/// Read `tid`'s state and whether a run queue holds it, after an optional
/// `unblock(tid, WakeSource::SelfTest)` (`do_unblock`). For self-tests.
///
/// All of it runs in one IRQ-masked section, so this CPU's tick cannot run
/// `schedule()` between the `unblock` and the reads. The locks are taken
/// one at a time, never nested: `unblock` takes and releases its own (it
/// drops `THREAD_TABLE` before `RUN_QUEUES`); then `THREAD_TABLE` alone, to
/// copy the state; then each online CPU's `RUN_QUEUES` lock alone, in
/// ascending CPU order. `THREAD_TABLE` is never held while a `RUN_QUEUES`
/// lock is taken, because the IRQ-path load balancer on another CPU holds
/// two run queue locks while it takes `THREAD_TABLE`.
///
/// The state and the queue scan are not one snapshot: another CPU can act
/// on the thread between them. A probe of a Dead thread, whose state is
/// terminal, or of a thread pinned to the probing CPU, needs none.
pub fn thread_probe(tid: ThreadId, do_unblock: bool) -> ThreadProbe {
    daif::with_irqs_masked(|| {
        if do_unblock {
            unblock(tid, WakeSource::SelfTest);
        }
        let state = {
            let table = THREAD_TABLE.lock();
            table
                .get(tid.0 as usize)
                .and_then(Option::as_ref)
                .map(|thread| thread.sched.state)
        };
        let ncpu = crate::smp::online_cpus().min(MAX_CORES);
        let queued = (0..ncpu).any(|cpu| RUN_QUEUES[cpu].lock().contains(tid));
        ThreadProbe { state, queued }
    })
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
        tripwire::bump_masked(Key::Insched, 0);
        return;
    }
    schedule(Origin::Irq);
}
