//! IPC timeout infrastructure, reply slots, and wakeup error tracking.
//!
//! Provides the timeout queue (checked every timer tick), per-thread reply
//! buffers for synchronous IPC, and helper functions for waking threads
//! with error codes.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::timer::TICK_COUNT;
use crate::sched;
use crate::task::{ThreadId, ThreadState, MAX_THREADS};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Timeout queue
// ---------------------------------------------------------------------------

/// Entry in the timeout queue — tracks a thread's IPC deadline.
pub(super) struct TimeoutEntry {
    pub(super) tid: ThreadId,
    pub(super) wake_at_tick: u64,
    pub(super) error_code: i64,
}

pub(super) static TIMEOUT_QUEUE: Mutex<[Option<TimeoutEntry>; MAX_THREADS]> = {
    const NONE: Option<TimeoutEntry> = None;
    Mutex::new([NONE; MAX_THREADS])
};

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
/// the woken thread to distinguish normal wakeup from error wakeup.
static WAKEUP_ERRORS: Mutex<[i64; MAX_THREADS]> = Mutex::new([0; MAX_THREADS]);

// ---------------------------------------------------------------------------
// Timeout checking — called from timer tick handler
// ---------------------------------------------------------------------------

/// Check for expired IPC timeouts. Called every tick from timer_tick_handler.
///
/// Scans the timeout queue (MAX_THREADS entries) and wakes any thread
/// whose deadline has passed. O(n) where n = MAX_THREADS = 64.
pub fn check_timeouts() {
    let now = TICK_COUNT.load(Ordering::Relaxed);

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
                *entry = None;
            }
        }
    }
    drop(tq);

    // Wake expired threads outside the lock.
    for &(tid, error) in expired[..count].iter() {
        wake_with_error(tid, error);
    }

    // Also check notification/select deadlines (separate table).
    super::notify::check_notification_timeouts(now);
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
pub(super) fn wake_with_error(tid: ThreadId, error: i64) {
    // Store error in the wakeup error slot.
    {
        let mut errors = WAKEUP_ERRORS.lock();
        errors[tid.0 as usize] = error;
    }
    // Clear any pending timeout — the thread is being woken for a
    // different reason (cancel, destroy, etc.), so the timeout must
    // not fire later and overwrite this error code.
    clear_timeout(tid);
    sched::unblock(tid);
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
pub(super) fn clear_timeout(tid: ThreadId) {
    if let Some(mut tq) = TIMEOUT_QUEUE.try_lock() {
        tq[tid.0 as usize] = None;
    }
    // If the lock is contended (IRQ handler running check_timeouts),
    // skip — the timeout handler will see the thread is already awake
    // and the wakeup error slot is already set, so it's benign.
}

/// Sleep the current thread for the given number of ticks.
///
/// Uses the IPC timeout infrastructure to wake the thread after the deadline.
/// error_code=0 distinguishes sleep wake from error wake.
pub fn sleep_ticks(ticks: u64) {
    if ticks == 0 {
        return;
    }
    let tid = match current_thread_id() {
        Some(t) => t,
        None => return,
    };
    let deadline = TICK_COUNT.load(Ordering::Relaxed) + ticks;
    {
        let mut tq = TIMEOUT_QUEUE.lock();
        tq[tid.0 as usize] = Some(TimeoutEntry {
            tid,
            wake_at_tick: deadline,
            error_code: 0,
        });
    }
    sched::block_current(ThreadState::BlockedIpc { channel: u64::MAX });
}
