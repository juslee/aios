//! IPC timeout infrastructure, reply slots, and wakeup error tracking.
//!
//! Provides the timeout queue (checked every timer tick), per-thread reply
//! buffers for synchronous IPC, and helper functions for waking threads
//! with error codes.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::daif;
use crate::arch::aarch64::timer::TICK_COUNT;
use crate::observability::tripwire::{self, WaitSide};
use crate::sched::{self, wake};
use crate::sync::IrqSpinLock;
use crate::task::{ThreadId, ThreadState, MAX_THREADS};
use shared::deadline_after;
use shared::lock::LockClass;
use shared::tripwire::{ClearResult, Key, WakeSource};
use spin::Mutex;

use super::DeadlineArm;

// ---------------------------------------------------------------------------
// Timeout queue
// ---------------------------------------------------------------------------

/// Entry in the timeout queue — tracks a thread's IPC deadline.
pub(super) struct TimeoutEntry {
    pub(super) tid: ThreadId,
    pub(super) wake_at_tick: u64,
    pub(super) error_code: i64,
}

/// Pending IPC and sleep deadlines, by thread slot. An IRQ-class lock
/// (`sync::IrqSpinLock`): `check_timeouts` takes it from the timer IRQ.
pub(super) static TIMEOUT_QUEUE: IrqSpinLock<[Option<TimeoutEntry>; MAX_THREADS]> =
    IrqSpinLock::new(LockClass::TimeoutQueue, [const { None }; MAX_THREADS]);

// ---------------------------------------------------------------------------
// Reply slots
// ---------------------------------------------------------------------------

/// Reply buffer: when a caller blocks in ipc_call(), it registers where
/// the reply should be written. Protected by the CHANNEL_TABLE lock
/// (reply is written while the lock is held in ipc_reply).
///
/// Per-thread: index = thread table index. Only one outstanding ipc_call
/// per thread is possible (thread is blocked).
pub(super) struct ReplySlot {
    /// Pointer to the caller's reply buffer (kernel VA).
    pub(super) buf: *mut u8,
    /// Maximum reply buffer size.
    pub(super) buf_len: usize,
    /// Actual bytes written by the replier (set by ipc_reply).
    pub(super) bytes_written: usize,
}

// SAFETY: ReplySlot contains a *mut u8 pointing to a blocked thread's kernel
// stack buffer. The thread is blocked (cannot run) while the slot is active,
// so the pointer is stable and exclusively accessed via REPLY_SLOTS lock.
unsafe impl Send for ReplySlot {}
unsafe impl Sync for ReplySlot {}

pub(super) static REPLY_SLOTS: Mutex<[Option<ReplySlot>; MAX_THREADS]> = {
    const NONE: Option<ReplySlot> = None;
    Mutex::new([NONE; MAX_THREADS])
};

// ---------------------------------------------------------------------------
// Per-thread wakeup error tracking
// ---------------------------------------------------------------------------

/// Per-thread wakeup error codes. Set by wake_with_error(), read by
/// the woken thread to distinguish normal wakeup from error wakeup. An
/// IRQ-class lock (`sync::IrqSpinLock`): `check_timeouts` wakes threads with
/// errors from the timer IRQ.
static WAKEUP_ERRORS: IrqSpinLock<[i64; MAX_THREADS]> =
    IrqSpinLock::new(LockClass::WakeupErrors, [0; MAX_THREADS]);

/// Visit the lock words of TIMEOUT_QUEUE and WAKEUP_ERRORS, for
/// `sync::held_by_stream`.
pub(super) fn irq_lock_words(f: &mut impl FnMut(u64)) {
    f(TIMEOUT_QUEUE.owner_word());
    f(WAKEUP_ERRORS.owner_word());
}

// ---------------------------------------------------------------------------
// Timeout checking — called from timer tick handler
// ---------------------------------------------------------------------------

/// Check for expired IPC timeouts. Called every tick from timer_tick_handler.
///
/// First expires notification and select deadlines
/// (`notify::check_notification_timeouts`), whatever the state of
/// TIMEOUT_QUEUE, so a busy TIMEOUT_QUEUE skips only the IPC timeout scan.
/// Then scans the timeout queue (MAX_THREADS entries) and wakes any thread
/// whose deadline has passed. O(n) where n = MAX_THREADS = 64.
pub fn check_timeouts() {
    let now = TICK_COUNT.load(Ordering::Relaxed);

    // Notification and select deadlines: a separate table, try-locked alone
    // and released before any unblock.
    super::notify::check_notification_timeouts(now);

    // Use try_lock() because this is called from IRQ context.
    // If the lock is held (e.g., ipc_call is registering a timeout on this
    // core), skip this tick — the next tick will catch it.
    let mut tq = match TIMEOUT_QUEUE.try_lock() {
        Some(guard) => guard,
        None => return,
    };

    // Collect expired entries under the lock, then wake outside the lock
    // to avoid lock ordering issues (TIMEOUT_QUEUE → THREAD_TABLE).
    let mut expired: [(ThreadId, i64); MAX_THREADS] = [(ThreadId(0), 0); MAX_THREADS];
    let mut count = 0;

    for entry in tq.iter_mut() {
        if let Some(te) = entry {
            if now >= te.wake_at_tick {
                expired[count] = (te.tid, te.error_code);
                count += 1;
                // The entry was the thread's timeout waker: its wake is in
                // flight until wake_with_error reaches unblock.
                tripwire::mark_wake_pending(te.tid, WakeSource::To);
                *entry = None;
            }
        }
    }
    drop(tq);

    // Wake expired threads outside the lock.
    for &(tid, error) in expired[..count].iter() {
        wake_with_error(tid, error, WakeSource::To);
    }
}

// ---------------------------------------------------------------------------
// Helper: get current thread ID
// ---------------------------------------------------------------------------

pub fn current_thread_id() -> Option<ThreadId> {
    let cpu = crate::arch::aarch64::exceptions::core_id() as usize;
    *crate::task::CURRENT_THREAD[cpu].lock()
}

// ---------------------------------------------------------------------------
// Helper: wake a thread with an error code
// ---------------------------------------------------------------------------

/// Wake a blocked thread and signal an error condition.
///
/// We store the error code in a per-thread wakeup error slot so the
/// thread can check it after being unblocked. Also clears any pending
/// timeout entry to prevent stale timeouts from firing later.
///
/// `src` names the caller for the tripwire's per-source `unblock` counters.
pub(crate) fn wake_with_error(tid: ThreadId, error: i64, src: WakeSource) {
    // Store error in the wakeup error slot.
    {
        let mut errors = WAKEUP_ERRORS.lock();
        errors[tid.0 as usize] = error;
    }
    // Clear any pending timeout — the thread is being woken for a
    // different reason (cancel, destroy, etc.), so the timeout must
    // not fire later and overwrite this error code. The timeout's own
    // wake (`check_timeouts`) has already taken the entry, so a busy clear
    // there leaves nothing behind and is not a `ctbusy`.
    let clear = clear_timeout(tid);
    if !matches!(src, WakeSource::To) {
        tripwire::note_clear(clear);
    }
    sched::unblock(tid, src);
}

/// Get and clear the wakeup error for a thread. Returns 0 if no error.
pub(super) fn get_wakeup_error(tid: ThreadId) -> i64 {
    let mut errors = WAKEUP_ERRORS.lock();
    let error = errors[tid.0 as usize];
    errors[tid.0 as usize] = 0;
    error
}

/// Clear any pending timeout entry for a thread.
/// Called when a thread is woken by a non-timeout path (reply, send,
/// cancel, destroy) to prevent stale timeouts from firing later.
///
/// Returns what it found, for the tripwire's N2 classification: the reply,
/// send and call wakers pass their own result to it. A busy TIMEOUT_QUEUE
/// leaves the entry; the callers whose entry that can be count `ctbusy`
/// with [`tripwire::note_clear`].
pub(super) fn clear_timeout(tid: ThreadId) -> ClearResult {
    if let Some(mut tq) = TIMEOUT_QUEUE.try_lock() {
        let entry = &mut tq[tid.0 as usize];
        let found = entry.is_some();
        *entry = None;
        return if found {
            ClearResult::Removed
        } else {
            ClearResult::Absent
        };
    }
    // If the lock is contended (IRQ handler running check_timeouts),
    // skip — the timeout handler will see the thread is already awake
    // and the wakeup error slot is already set, so it's benign.
    ClearResult::Busy
}

/// Remove `tid`'s timeout entry, waiting for TIMEOUT_QUEUE if it is busy.
/// Returns whether it removed one. For thread context only (process exit
/// step C, a wait's own exit through [`clear_own_timeout`], the self-tests'
/// wake helper): unlike [`clear_timeout`], it never leaves an entry behind to
/// fire later. The hold runs with IRQs masked, because TIMEOUT_QUEUE is an
/// IRQ-class lock that this CPU's tick try-locks.
pub(super) fn clear_timeout_blocking(tid: ThreadId) -> bool {
    daif::with_irqs_masked(|| {
        TIMEOUT_QUEUE
            .lock()
            .get_mut(tid.0 as usize)
            .and_then(Option::take)
            .is_some()
    })
}

/// Whether `tid` has a TIMEOUT_QUEUE entry, read with IRQs masked (an
/// IRQ-class lock that this CPU's tick try-locks). For self-tests.
pub(super) fn has_timeout_entry(tid: ThreadId) -> bool {
    daif::with_irqs_masked(|| {
        TIMEOUT_QUEUE
            .lock()
            .get(tid.0 as usize)
            .is_some_and(Option::is_some)
    })
}

/// Drop `tid`'s reply slot, if any: the buffer it points at belongs to a
/// thread that will never read it (process exit step C). REPLY_SLOTS is not
/// IRQ-class.
pub(super) fn clear_reply_slot(tid: ThreadId) {
    if let Some(slot) = REPLY_SLOTS.lock().get_mut(tid.0 as usize) {
        *slot = None;
    }
}

/// Publish `me`'s wait deadline in TIMEOUT_QUEUE, after the arm (ADR §8
/// rule 1), where `check_timeouts` wakes it with `code`.
///
/// `TICK_COUNT` is read inside the TIMEOUT_QUEUE hold, which orders the read
/// after any `check_timeouts` that fired and removed the entry: a deadline
/// that has passed (`TICK_COUNT >= deadline`) gives [`DeadlineArm::Expired`]
/// and inserts nothing, so an entry that already fired is never re-inserted,
/// and the wait must not block. Otherwise the entry is inserted when the slot
/// is empty (a later pass of the same wait finds its own entry still there
/// unless a waker removed it) and the result is [`DeadlineArm::Armed`].
/// `None` (no deadline) inserts nothing and gives [`DeadlineArm::Armed`].
/// An out-of-range `me`, whose entry no slot can hold, counts `badtid` and
/// gives [`DeadlineArm::Expired`], so the wait ends instead of blocking with
/// nothing to time it out.
///
/// `side` is the tripwire N2 phase of an `ipc_call` or `ipc_recv`, moved to
/// armed inside the same hold whatever the result, or at the point where it
/// would register for a wait with no deadline; a sleep has no phase (`None`).
/// The hold runs with IRQs masked, because TIMEOUT_QUEUE is an IRQ-class lock
/// that this CPU's tick try-locks.
pub(super) fn arm_timeout(
    me: ThreadId,
    deadline: Option<u64>,
    code: i64,
    side: Option<WaitSide>,
) -> DeadlineArm {
    let Some(wake_at_tick) = deadline else {
        if let Some(side) = side {
            tripwire::wait_armed(side, me, false);
        }
        return DeadlineArm::Armed;
    };
    daif::with_irqs_masked(|| {
        let mut tq = TIMEOUT_QUEUE.lock();
        let arm = match tq.get_mut(me.0 as usize) {
            // No slot can hold the entry, so nothing would end the wait:
            // fail closed rather than block unbounded.
            None => {
                tripwire::bump_masked(Key::Badtid, 0);
                DeadlineArm::Expired
            }
            Some(_) if TICK_COUNT.load(Ordering::Relaxed) >= wake_at_tick => DeadlineArm::Expired,
            Some(slot) => {
                if slot.is_none() {
                    *slot = Some(TimeoutEntry {
                        tid: me,
                        wake_at_tick,
                        error_code: code,
                    });
                }
                DeadlineArm::Armed
            }
        };
        if let Some(side) = side {
            tripwire::wait_armed(side, me, true);
        }
        arm
    })
}

/// Remove `me`'s own timeout entry at a wait's exit, waiting for
/// TIMEOUT_QUEUE if it is busy ([`clear_timeout_blocking`]): nothing is left
/// to fire into a later wait. A no-op when the entry fired or was never
/// inserted.
pub(super) fn clear_own_timeout(me: ThreadId) {
    clear_timeout_blocking(me);
}

/// Sleep the current thread for `ticks` timer ticks.
///
/// 0 returns at once. The deadline comes from `shared::deadline_after`, so a
/// large `ticks` cannot overflow (#217); `u64::MAX`, no deadline there, is
/// stored as the entry at `u64::MAX`, which the tick count never reaches.
/// With no current thread it returns at once.
///
/// The thread's tid comes from `sched::current_tid()`. Its one waker record
/// is its TIMEOUT_QUEUE entry, code 0, which `check_timeouts` fires. Each
/// pass arms (`wake::prepare_block`), then publishes the entry
/// ([`arm_timeout`]): once the deadline has passed the sleep ends; otherwise
/// the thread blocks as `BlockedTimer` and the next pass re-checks. A stale
/// wake (a stale reply or send whose `clear_timeout` removed the entry, or a
/// late `wake_with_error` from an earlier `ipc_call`) costs one pass, which
/// re-inserts the entry, so the sleep never returns before its deadline.
/// The exit removes the entry if a stale wake left it ([`clear_own_timeout`])
/// and then cancels the arm. `WAKEUP_ERRORS` is not taken: the entry's own
/// wake carries 0, and a late non-zero code decides nothing (the next
/// `wake_with_error` overwrites it, or the next `ipc_call`, `ipc_recv` or
/// `ipc_select` exit discards it).
pub fn sleep_ticks(ticks: u64) {
    if ticks == 0 {
        return;
    }
    let Some(me) = sched::current_tid() else {
        return;
    };
    let deadline = deadline_after(TICK_COUNT.load(Ordering::Relaxed), ticks).unwrap_or(u64::MAX);
    loop {
        wake::prepare_block(me);
        if arm_timeout(me, Some(deadline), 0, None) == DeadlineArm::Expired {
            break;
        }
        sched::block_current(ThreadState::BlockedTimer { wake_at: deadline });
    }
    clear_own_timeout(me);
    wake::cancel_block(me);
}
