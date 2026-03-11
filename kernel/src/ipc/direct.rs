//! IPC direct switch and priority inheritance.
//!
//! Bypasses the scheduler when the IPC receiver is already waiting.
//! This is the core L4 optimization for sub-5μs IPC round-trips.
//! Per ipc.md §9.1–9.3, scheduler.md §4.2.
//!
//! # Direct Switch Flow (ipc_call fast path)
//!
//! 1. Caller finds receiver already blocked in ipc_recv() on this channel
//! 2. Copy message into ring (already done by caller)
//! 3. Save caller's context
//! 4. Donate caller's remaining time slice to receiver
//! 5. Apply priority inheritance if caller > receiver
//! 6. Restore receiver's context → execution continues in receiver
//!
//! # Reply Switch Flow (ipc_reply fast path)
//!
//! 1. Receiver replies, finds caller on same CPU
//! 2. Copy reply to caller's reply buffer (already done)
//! 3. Restore caller's original scheduling priority
//! 4. Save receiver's context
//! 5. Restore caller's context → caller returns from ipc_call()

use crate::arch::aarch64::exceptions;
use crate::observability::metrics::METRICS;
use crate::task::{ThreadContext, ThreadId, ThreadState, CURRENT_THREAD, THREAD_TABLE};

// Re-export from shared crate so kernel code can use `direct::MAX_INHERITANCE_DEPTH`.
pub use shared::MAX_INHERITANCE_DEPTH;

extern "C" {
    fn save_context(ctx: *mut ThreadContext);
    fn restore_context(ctx: *const ThreadContext) -> !;
}

/// Attempt an IPC direct switch from the current (sender) thread to the
/// receiver thread. Returns `true` if the direct switch was performed
/// (caller resumes here when it's eventually woken), `false` if direct
/// switch was not possible and the caller should fall back to the
/// scheduler path.
///
/// Preconditions (checked by ipc_call before calling):
/// - Message already enqueued in channel ring
/// - `receiver_tid` was the channel's `waiting_receiver`
/// - Channel lock has been released
///
/// This function:
/// 1. Masks IRQs (we're manipulating thread state and switching)
/// 2. Validates both threads exist and receiver is blocked
/// 3. Donates sender's time slice to receiver
/// 4. Applies priority inheritance
/// 5. Saves sender context, restores receiver context
/// 6. When sender is eventually resumed (after reply), returns true
pub fn try_direct_switch(sender_tid: ThreadId, receiver_tid: ThreadId) -> bool {
    let cpu = exceptions::core_id() as usize;

    // Mask IRQs — we're doing a context switch.
    // SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

    let mut table = THREAD_TABLE.lock();

    // Validate sender is current and running.
    let sender_idx = sender_tid.0 as usize;
    let receiver_idx = receiver_tid.0 as usize;

    // Bounds check.
    if sender_idx >= table.len() || receiver_idx >= table.len() {
        drop(table);
        // SAFETY: Restore IRQ state.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
        return false;
    }

    // Both threads must exist.
    if table[sender_idx].is_none() || table[receiver_idx].is_none() {
        drop(table);
        // SAFETY: DAIFClr #0x2 clears the IRQ mask bit, restoring interrupts. Safe at EL1.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
        return false;
    }

    // Receiver must be blocked on IPC (waiting in ipc_recv).
    {
        let receiver = table[receiver_idx].as_ref().unwrap();
        match receiver.sched.state {
            ThreadState::BlockedIpc { .. } => {}
            _ => {
                drop(table);
                // SAFETY: DAIFClr #0x2 clears the IRQ mask bit, restoring interrupts. Safe at EL1.
                unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
                return false;
            }
        }
    }

    // Extract sender fields before taking mutable borrows (avoids
    // simultaneous immutable + mutable borrow of table elements).
    let sender_eff_class;
    let sender_eff_priority;
    let sender_deadline;
    let sender_depth;
    let donated_slice;
    {
        let sender = table[sender_idx].as_ref().unwrap();
        sender_eff_class = sender.sched.effective_class;
        sender_eff_priority = sender.sched.effective_priority;
        sender_deadline = sender.sched.deadline;
        sender_depth = sender.inheritance_depth;
        donated_slice = sender.sched.time_slice_remaining;
    }

    // --- Priority inheritance (ipc.md §9.2) ---
    // If sender's effective class is higher than receiver's base class,
    // temporarily elevate the receiver for the duration of this request.
    {
        let receiver = table[receiver_idx].as_mut().unwrap();

        // Store inheritance info so ipc_reply can restore.
        receiver.sched.inherited_class = Some(sender_eff_class);
        receiver.sched.inherited_priority = Some(sender_eff_priority);
        receiver.sched.inherited_deadline = sender_deadline;

        // Elevate if sender outranks receiver.
        if sender_eff_class > receiver.sched.class {
            receiver.sched.effective_class = sender_eff_class;
            receiver.sched.effective_priority = sender_eff_priority;
        }

        // Track transitive inheritance depth.
        receiver.inheritance_depth = (sender_depth + 1).min(MAX_INHERITANCE_DEPTH);

        // Donate time slice and unblock receiver.
        receiver.sched.time_slice_remaining = donated_slice;
        receiver.sched.state = ThreadState::Runnable;
    }

    // --- Block sender ---
    // The sender blocks until ipc_reply wakes it.
    {
        let sender = table[sender_idx].as_mut().unwrap();
        sender.sched.state = ThreadState::BlockedIpc { channel: 0 };
        sender.sched.time_slice_remaining = 0;
    }

    // Get context pointers. We need raw pointers because save_context
    // writes to the sender's context and restore_context reads from
    // the receiver's context.
    let sender_ctx_ptr = &mut table[sender_idx].as_mut().unwrap().context as *mut ThreadContext;
    let receiver_ctx_ptr = &table[receiver_idx].as_ref().unwrap().context as *const ThreadContext;

    // Update CURRENT_THREAD to receiver.
    *CURRENT_THREAD[cpu].lock() = Some(receiver_tid);

    // Drop table lock before context switch — the receiver will need
    // to acquire it when it runs.
    drop(table);

    #[cfg(feature = "kernel-metrics")]
    {
        METRICS.ipc_direct_switch.inc();
        METRICS.sched_context_switch.inc();
    }

    // --- Context switch ---
    // SAFETY: sender_ctx_ptr points to the sender's ThreadContext in
    // THREAD_TABLE. save_context stores callee-saved regs (x19-x30),
    // SP, and LR. When the sender is later restored (by ipc_reply_switch
    // or scheduler), execution resumes right after save_context returns.
    unsafe { save_context(sender_ctx_ptr) };

    // After save_context returns, we might be:
    // (a) The sender, about to switch to receiver (first time through)
    // (b) The sender, restored later by reply_switch or scheduler
    //
    // Check: are we still supposed to switch to the receiver?
    let actual_cpu = exceptions::core_id() as usize;
    let current_now = { *CURRENT_THREAD[actual_cpu].lock() };

    if current_now == Some(receiver_tid) {
        // First time through — switch to receiver.
        // SAFETY: receiver_ctx_ptr points to the receiver's ThreadContext.
        // restore_context loads callee-saved regs, SP, and branches to
        // the saved PC. The receiver resumes in ipc_recv() after
        // block_current(). This never returns.
        unsafe { restore_context(receiver_ctx_ptr) };
    }

    // We were restored as the sender — the reply has arrived.
    // Unmask IRQs and return.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    true
}

/// Perform a direct switch from the replier back to the original caller.
/// Called from ipc_reply() when the caller is on the same CPU.
///
/// Returns `true` if the switch was performed, `false` if fallback to
/// scheduler is needed (e.g., caller migrated to another CPU).
///
/// This function also restores the replier's original scheduling priority
/// (undoing the priority inheritance from the call path).
pub fn try_reply_switch(replier_tid: ThreadId, caller_tid: ThreadId) -> bool {
    let cpu = exceptions::core_id() as usize;

    // Mask IRQs for context switch.
    // SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

    let mut table = THREAD_TABLE.lock();

    let replier_idx = replier_tid.0 as usize;
    let caller_idx = caller_tid.0 as usize;

    if replier_idx >= table.len() || caller_idx >= table.len() {
        drop(table);
        // SAFETY: DAIFClr #0x2 clears the IRQ mask bit, restoring interrupts. Safe at EL1.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
        return false;
    }

    if table[replier_idx].is_none() || table[caller_idx].is_none() {
        drop(table);
        // SAFETY: DAIFClr #0x2 clears the IRQ mask bit, restoring interrupts. Safe at EL1.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
        return false;
    }

    // Caller must be blocked (waiting for reply).
    {
        let caller = table[caller_idx].as_ref().unwrap();
        match caller.sched.state {
            ThreadState::BlockedIpc { .. } => {}
            _ => {
                drop(table);
                // SAFETY: DAIFClr #0x2 clears the IRQ mask bit, restoring interrupts. Safe at EL1.
                unsafe { core::arch::asm!("msr DAIFClr, #0x2") };
                return false;
            }
        }
    }

    // --- Restore replier's original priority (undo inheritance) ---
    {
        let replier = table[replier_idx].as_mut().unwrap();
        replier.sched.effective_class = replier.sched.class;
        replier.sched.effective_priority = replier.sched.priority;
        replier.sched.inherited_class = None;
        replier.sched.inherited_priority = None;
        replier.sched.inherited_deadline = None;
        replier.inheritance_depth = replier.inheritance_depth.saturating_sub(1);
    }

    // --- Donate replier's remaining time slice back to caller ---
    {
        let donated_slice = table[replier_idx]
            .as_ref()
            .unwrap()
            .sched
            .time_slice_remaining;

        let caller = table[caller_idx].as_mut().unwrap();
        caller.sched.time_slice_remaining = donated_slice;
        caller.sched.state = ThreadState::Running;
    }

    // Block replier — it goes back to waiting for the next request.
    // The replier's ipc_recv loop will re-block via sched::block_current()
    // naturally, but we mark it Runnable and re-enqueue so the scheduler
    // picks it up on its next pass.
    {
        let replier = table[replier_idx].as_mut().unwrap();
        replier.sched.state = ThreadState::Runnable;
        replier.sched.time_slice_remaining = 0;
    }

    let replier_ctx_ptr = &mut table[replier_idx].as_mut().unwrap().context as *mut ThreadContext;
    let caller_ctx_ptr = &table[caller_idx].as_ref().unwrap().context as *const ThreadContext;

    let replier_class = table[replier_idx].as_ref().unwrap().sched.effective_class;

    // Update CURRENT_THREAD to caller.
    *CURRENT_THREAD[cpu].lock() = Some(caller_tid);

    drop(table);

    // Re-enqueue the replier so it can handle future requests.
    crate::sched::enqueue_on_cpu(cpu, replier_tid, replier_class);

    #[cfg(feature = "kernel-metrics")]
    {
        METRICS.ipc_direct_switch.inc();
        METRICS.sched_context_switch.inc();
    }

    // --- Context switch: replier → caller ---
    // SAFETY: replier_ctx_ptr points to replier's ThreadContext.
    // save_context stores callee-saved regs and returns.
    unsafe { save_context(replier_ctx_ptr) };

    // Check if we're the replier being resumed later or about to switch.
    let actual_cpu = exceptions::core_id() as usize;
    let current_now = { *CURRENT_THREAD[actual_cpu].lock() };

    if current_now == Some(caller_tid) {
        // First time through — switch to caller.
        // SAFETY: caller_ctx_ptr points to caller's ThreadContext.
        // restore_context resumes the caller where it called save_context
        // in try_direct_switch(). This never returns.
        unsafe { restore_context(caller_ctx_ptr) };
    }

    // Replier was restored by scheduler — continue normally.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    true
}
