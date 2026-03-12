//! Lightweight notification objects (seL4-style word notifications).
//!
//! A notification is a single-word (u64) atomic bitmap. Signalling ORs bits in;
//! waiting checks a mask and returns+clears matching bits, blocking if none set.
//! Per ipc.md §6.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::observability::metrics::METRICS;
use crate::sched;
use crate::task::{ThreadId, ThreadState, MAX_THREADS};
use shared::{NotificationId, MAX_NOTIFICATIONS, MAX_WAITERS_PER_NOTIFICATION};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A thread waiting on a notification, with a bit mask.
#[derive(Clone, Copy)]
pub(crate) struct Waiter {
    pub(crate) tid: ThreadId,
    pub(crate) mask: u64,
}

/// Create a Waiter (used by select.rs to register on notification waiters).
pub(crate) fn waiter_new(tid: ThreadId, mask: u64) -> Waiter {
    Waiter { tid, mask }
}

/// A notification object: single-word atomic bitmap with bounded waiter list.
#[allow(dead_code)]
pub struct NotificationObject {
    pub id: NotificationId,
    pub(crate) word: AtomicU64,
    pub creator: crate::task::process::ProcessId,
    pub(crate) waiters: [Option<Waiter>; MAX_WAITERS_PER_NOTIFICATION],
}

impl NotificationObject {
    fn new(id: NotificationId, creator: crate::task::process::ProcessId) -> Self {
        Self {
            id,
            word: AtomicU64::new(0),
            creator,
            waiters: [None; MAX_WAITERS_PER_NOTIFICATION],
        }
    }
}

// ---------------------------------------------------------------------------
// Global tables
// ---------------------------------------------------------------------------

/// System-wide notification table. Lock ordering: after SHARED_REGION_TABLE,
/// before CHANNEL_TABLE (per deadlock-prevention §3).
pub(super) static NOTIFICATION_TABLE: Mutex<[Option<NotificationObject>; MAX_NOTIFICATIONS]> =
    Mutex::new([const { None }; MAX_NOTIFICATIONS]);

/// Per-thread result slot: stores the matched bits for a thread woken from
/// notification_wait or IpcSelect. Indexed by ThreadId.0.
pub(super) static NOTIFY_RESULTS: Mutex<[Option<u64>; MAX_THREADS]> =
    Mutex::new([None; MAX_THREADS]);

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/// Create a new notification object. Returns its ID.
pub fn notification_create(pid: crate::task::process::ProcessId) -> Result<NotificationId, i64> {
    let mut table = NOTIFICATION_TABLE.lock();
    let idx = table
        .iter()
        .position(|slot| slot.is_none())
        .ok_or(crate::syscall::IpcError::Enomem as i64)?;

    let id = NotificationId(idx as u32);
    table[idx] = Some(NotificationObject::new(id, pid));
    drop(table);

    #[cfg(feature = "kernel-metrics")]
    METRICS.notify_signal.inc(); // Reuse signal counter for create events too? No — use a dedicated log.

    crate::kinfo!(Ipc, "Notification {} created by pid={}", idx, pid.0);
    Ok(id)
}

/// Signal a notification: atomically OR `bits` into the word, then wake any
/// waiters whose mask intersects the new value.
pub fn notification_signal(id: NotificationId, bits: u64) {
    if id.0 as usize >= MAX_NOTIFICATIONS {
        return;
    }

    // OR the bits in first (before acquiring the table lock).
    // We need the table lock to read waiters, but the atomic OR is lock-free.
    // However, we must hold the lock while checking waiters to avoid races
    // where a waiter registers after we OR but before we check.
    //
    // Strategy: lock table, OR bits, check waiters, wake matches.
    let mut table = NOTIFICATION_TABLE.lock();
    let notif = match &mut table[id.0 as usize] {
        Some(n) => n,
        None => return,
    };

    // Atomic OR — visible to concurrent readers even under lock.
    notif.word.fetch_or(bits, Ordering::Release);

    // Collect threads to wake (we must drop the table lock before unblocking).
    let mut to_wake: [(ThreadId, u64); MAX_WAITERS_PER_NOTIFICATION] =
        [(ThreadId(0), 0); MAX_WAITERS_PER_NOTIFICATION];
    let mut wake_count = 0usize;

    for slot in notif.waiters.iter_mut() {
        if let Some(waiter) = slot {
            let current_word = notif.word.load(Ordering::Acquire);
            let matched = current_word & waiter.mask;
            if matched != 0 {
                // Clear the matched bits atomically.
                notif.word.fetch_and(!matched, Ordering::AcqRel);
                to_wake[wake_count] = (waiter.tid, matched);
                wake_count += 1;
                *slot = None; // Remove waiter
            }
        }
    }

    drop(table);

    // Store results and unblock outside the table lock.
    if wake_count > 0 {
        let mut results = NOTIFY_RESULTS.lock();
        for &(tid, matched) in to_wake.iter().take(wake_count) {
            results[tid.0 as usize] = Some(matched);
        }
        drop(results);

        for &(tid, matched) in to_wake.iter().take(wake_count) {
            // If the thread is select-blocked, wake via select path (sets ready_index).
            if !super::select::try_wake_select(
                tid,
                shared::SelectKind::Notification(id, matched),
                matched,
            ) {
                sched::unblock(tid);
            }
        }
    }

    #[cfg(feature = "kernel-metrics")]
    METRICS.notify_signal.inc();
}

/// Wait on a notification: if any bits matching `mask` are set, return+clear
/// them immediately. Otherwise block until signalled or timeout expires.
///
/// Returns the matched bits on success, or an error code.
pub fn notification_wait(id: NotificationId, mask: u64, timeout_ticks: u64) -> Result<u64, i64> {
    use crate::arch::aarch64::timer::TICK_COUNT;

    if id.0 as usize >= MAX_NOTIFICATIONS {
        return Err(crate::syscall::IpcError::Einval as i64);
    }

    let my_tid = crate::ipc::current_thread_id().ok_or(crate::syscall::IpcError::Einval as i64)?;

    #[cfg(feature = "kernel-metrics")]
    METRICS.notify_wait.inc();

    // Fast path: check if bits are already set.
    {
        let table = NOTIFICATION_TABLE.lock();
        let notif = table[id.0 as usize]
            .as_ref()
            .ok_or(crate::syscall::IpcError::Einval as i64)?;

        let current = notif.word.load(Ordering::Acquire);
        let matched = current & mask;
        if matched != 0 {
            notif.word.fetch_and(!matched, Ordering::AcqRel);
            drop(table);

            #[cfg(feature = "kernel-metrics")]
            METRICS.notify_wake.inc();

            return Ok(matched);
        }
    }
    // Table lock dropped here before blocking.

    // Slow path: register as waiter and block.
    let deadline = if timeout_ticks == u64::MAX {
        u64::MAX
    } else {
        TICK_COUNT
            .load(Ordering::Relaxed)
            .saturating_add(timeout_ticks)
    };

    {
        let mut table = NOTIFICATION_TABLE.lock();
        let notif = table[id.0 as usize]
            .as_mut()
            .ok_or(crate::syscall::IpcError::Einval as i64)?;

        // Double-check after re-acquiring lock (signal may have arrived).
        let current = notif.word.load(Ordering::Acquire);
        let matched = current & mask;
        if matched != 0 {
            notif.word.fetch_and(!matched, Ordering::AcqRel);
            drop(table);

            #[cfg(feature = "kernel-metrics")]
            METRICS.notify_wake.inc();

            return Ok(matched);
        }

        // Register as waiter.
        let slot = notif
            .waiters
            .iter_mut()
            .find(|s| s.is_none())
            .ok_or(crate::syscall::IpcError::Enomem as i64)?;
        *slot = Some(Waiter { tid: my_tid, mask });
    }
    // Table lock dropped before schedule().

    // Store deadline for timeout checking.
    set_thread_deadline(my_tid, deadline);

    // Block — scheduler will context-switch away.
    sched::block_current(ThreadState::BlockedNotification { notification: id.0 });

    // Woken up — check result.
    let result = {
        let mut results = NOTIFY_RESULTS.lock();
        results[my_tid.0 as usize].take()
    };

    match result {
        Some(bits) => {
            #[cfg(feature = "kernel-metrics")]
            METRICS.notify_wake.inc();

            Ok(bits)
        }
        None => {
            // Timeout or spurious wake — clean up waiter registration.
            cleanup_waiter(id, my_tid);
            Err(crate::syscall::IpcError::Etimedout as i64)
        }
    }
}

/// Destroy a notification: wake all waiters with error, remove from table.
#[allow(dead_code)]
pub fn notification_destroy(id: NotificationId) {
    if id.0 as usize >= MAX_NOTIFICATIONS {
        return;
    }
    let mut table = NOTIFICATION_TABLE.lock();
    let notif = match table[id.0 as usize].take() {
        Some(n) => n,
        None => return,
    };

    // Collect waiters to wake with error.
    let mut to_wake: [Option<ThreadId>; MAX_WAITERS_PER_NOTIFICATION] =
        [None; MAX_WAITERS_PER_NOTIFICATION];
    for (i, slot) in notif.waiters.iter().enumerate() {
        if let Some(waiter) = slot {
            to_wake[i] = Some(waiter.tid);
        }
    }

    drop(table);

    // Wake all blocked waiters — they'll see None in NOTIFY_RESULTS → timeout error.
    for tid in to_wake.iter().flatten() {
        sched::unblock(*tid);
    }

    crate::kinfo!(Ipc, "Notification {} destroyed", id.0);
}

// ---------------------------------------------------------------------------
// Timeout support
// ---------------------------------------------------------------------------

/// Deadline storage for notification waits (indexed by tid).
/// The timeout checker in the timer tick handler reads this.
static NOTIFY_DEADLINES: Mutex<[u64; MAX_THREADS]> = Mutex::new([u64::MAX; MAX_THREADS]);

fn set_thread_deadline(tid: ThreadId, deadline: u64) {
    let mut deadlines = NOTIFY_DEADLINES.lock();
    deadlines[tid.0 as usize] = deadline;
}

/// Set a deadline for a select-waiting thread (called from select.rs).
pub(crate) fn set_select_deadline(tid: ThreadId, deadline: u64) {
    set_thread_deadline(tid, deadline);
}

/// Called from timer_tick_handler (via ipc::check_timeouts path) to expire
/// notification waits. Uses try_lock to be IRQ-safe.
pub fn check_notification_timeouts(now: u64) {
    // try_lock: safe from IRQ context — never block in timer tick.
    let mut deadlines = match NOTIFY_DEADLINES.try_lock() {
        Some(d) => d,
        None => return,
    };

    for tid_idx in 0..MAX_THREADS {
        if deadlines[tid_idx] <= now {
            deadlines[tid_idx] = u64::MAX;

            // Check thread state to determine cleanup path.
            // Use try_lock: called from IRQ context (timer tick), must not block.
            let thread_state = match crate::task::THREAD_TABLE.try_lock() {
                Some(table) => table[tid_idx].as_ref().map(|t| t.sched.state),
                None => continue, // Contended — retry next tick.
            };

            match thread_state {
                Some(ThreadState::BlockedNotification { notification }) => {
                    // Clean up notification waiter — try_lock to stay IRQ-safe.
                    if let Some(mut ntable) = NOTIFICATION_TABLE.try_lock() {
                        if let Some(notif) = &mut ntable[notification as usize] {
                            for slot in notif.waiters.iter_mut() {
                                if let Some(waiter) = slot {
                                    if waiter.tid.0 == tid_idx as u32 {
                                        *slot = None;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    sched::unblock(ThreadId(tid_idx as u32));
                }
                Some(ThreadState::BlockedSelect) => {
                    // Select timeout — clean up SELECT_WAITERS entry.
                    // The select path will see ready_index=None → return timeout.
                    if let Some(mut sw) = super::select::SELECT_WAITERS.try_lock() {
                        sw[tid_idx] = None;
                    }
                    sched::unblock(ThreadId(tid_idx as u32));
                }
                _ => {
                    // Not in a waitable state — ignore (may have been woken already).
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Remove a thread's waiter registration from a notification.
fn cleanup_waiter(id: NotificationId, tid: ThreadId) {
    let mut table = NOTIFICATION_TABLE.lock();
    if let Some(notif) = &mut table[id.0 as usize] {
        for slot in notif.waiters.iter_mut() {
            if let Some(waiter) = slot {
                if waiter.tid == tid {
                    *slot = None;
                    break;
                }
            }
        }
    }
}
