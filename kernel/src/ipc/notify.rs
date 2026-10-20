//! Lightweight notification objects (seL4-style word notifications).
//!
//! A notification is a single-word (u64) atomic bitmap. Signalling ORs bits in;
//! waiting checks a mask and returns+clears matching bits, blocking if none set.
//! Per ipc.md §6.
//!
//! Waiters find each other through the notification's waiter records, one per
//! waiting `notification_wait` ([`WaitKind::Wait`]) and one per notification an
//! `ipc_select` names ([`WaitKind::Select`]). The signaller acts on them under
//! NOTIFICATION_TABLE: for a wait record it moves the matched bits into the
//! record's `fired` word, which only the waiter clears, so a completed wait
//! never loses bits; a select record it removes (claims), leaving the bits in
//! the word for the select's next pass. It calls `unblock` after the release.
//! The waits arm their wake token (`sched::wake`) inside the critical section
//! that publishes the record, and loop until a pass finds their condition, a
//! failure or their deadline (capability-lifetime ADR §8, §9).

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::aarch64::daif;
use crate::arch::aarch64::timer::TICK_COUNT;
use crate::observability::metrics::METRICS;
use crate::observability::tripwire;
use crate::sched::{self, wake};
use crate::sync::IrqSpinLock;
use crate::syscall::IpcError;
use crate::task::{ThreadId, ThreadState, MAX_THREADS};
use shared::lock::LockClass;
use shared::tripwire::{pop_lowest, WakeSource};
use shared::{deadline_after, NotificationId, MAX_NOTIFICATIONS, MAX_WAITERS_PER_NOTIFICATION};

use super::DeadlineArm;

// The deadline scan collects expired tids in a u64 mask, one bit per slot.
const _: () = assert!(MAX_THREADS <= 64);

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// What a waiter record waits for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WaitKind {
    /// A `notification_wait`. The signaller consumes the matched bits into the
    /// record's `fired` word and leaves the record for the waiter to remove.
    Wait,
    /// An `ipc_select` entry. The signaller removes the record (the claim)
    /// and leaves the bits in the word, for the select's next pass.
    Select,
}

/// A thread waiting on a notification, with a bit mask.
#[derive(Clone, Copy)]
pub(crate) struct Waiter {
    pub(crate) tid: ThreadId,
    pub(crate) mask: u64,
    pub(crate) kind: WaitKind,
    /// The bits a signal consumed for this [`WaitKind::Wait`] record (0: not
    /// fired). Always 0 for a [`WaitKind::Select`] record.
    pub(crate) fired: u64,
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

    /// Consume the bits of `mask` that are set in the word. Returns them, or
    /// `None` if none is set.
    fn take_bits(&self, mask: u64) -> Option<u64> {
        let matched = self.word.load(Ordering::Acquire) & mask;
        if matched == 0 {
            return None;
        }
        self.word.fetch_and(!matched, Ordering::AcqRel);
        Some(matched)
    }

    /// The slot of `tid`'s record of `kind`, if it has one.
    fn own_record(&self, tid: ThreadId, kind: WaitKind) -> Option<usize> {
        self.waiters
            .iter()
            .position(|slot| slot.is_some_and(|w| w.tid == tid && w.kind == kind))
    }

    /// Put `waiter` in a free slot. `Err(ENOMEM)` when every slot is taken.
    fn insert(&mut self, waiter: Waiter) -> Result<(), i64> {
        let slot = self
            .waiters
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(IpcError::Enomem as i64)?;
        *slot = Some(waiter);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Global tables
// ---------------------------------------------------------------------------

/// System-wide notification table. Lock ordering: after SHARED_REGION_TABLE,
/// before CHANNEL_TABLE (per deadlock-prevention §3).
/// An IRQ-class lock (`sync::IrqSpinLock`): CPU 0's heartbeat scan
/// (`ipc::scan`) try-locks it from the timer IRQ. Every hold in this module
/// runs with IRQs masked.
pub(super) static NOTIFICATION_TABLE: IrqSpinLock<[Option<NotificationObject>; MAX_NOTIFICATIONS]> =
    IrqSpinLock::new(
        LockClass::NotificationTable,
        [const { None }; MAX_NOTIFICATIONS],
    );

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/// Create a new notification object. Returns its ID.
pub fn notification_create(pid: crate::task::process::ProcessId) -> Result<NotificationId, i64> {
    let id = daif::with_irqs_masked(|| {
        let mut table = NOTIFICATION_TABLE.lock();
        let idx = table
            .iter()
            .position(|slot| slot.is_none())
            .ok_or(IpcError::Enomem as i64)?;
        let id = NotificationId(idx as u32);
        table[idx] = Some(NotificationObject::new(id, pid));
        Ok::<_, i64>(id)
    })?;
    let idx = id.0;

    #[cfg(feature = "kernel-metrics")]
    METRICS.notify_create.inc();

    crate::kinfo!(Ipc, "Notification {} created by pid={}", idx, pid.0);
    Ok(id)
}

/// Signal a notification: atomically OR `bits` into the word, then act on
/// its waiter records, all in one NOTIFICATION_TABLE critical section with
/// IRQs masked:
///
/// - a [`WaitKind::Wait`] record that has not fired and whose mask matches
///   the word gets the matched bits, consumed from the word, in its `fired`
///   word, and stays (its waiter removes it);
/// - a [`WaitKind::Select`] record whose mask matches the word is removed
///   (claimed); the bits stay in the word for the select's next pass.
///
/// After the release, each thread whose record it acted on is woken
/// (`WakeSource::Sig`). A waiter that has already removed its own record is
/// not found, so a completed wait never loses bits.
///
/// Returns `Err(EINVAL)`, and signals nothing, for an id `>= MAX_NOTIFICATIONS`
/// or an empty slot, the same errors `notification_wait` returns.
pub fn notification_signal(id: NotificationId, bits: u64) -> Result<(), i64> {
    let idx = id.0 as usize;
    if idx >= MAX_NOTIFICATIONS {
        return Err(IpcError::Einval as i64);
    }

    // One wake per record at most.
    let mut to_wake = [ThreadId(0); MAX_WAITERS_PER_NOTIFICATION];
    let mut wake_count = 0usize;
    let found = daif::with_irqs_masked(|| {
        let mut table = NOTIFICATION_TABLE.lock();
        let Some(notif) = table[idx].as_mut() else {
            return false;
        };
        notif.word.fetch_or(bits, Ordering::AcqRel);
        let word = &notif.word;
        for slot in notif.waiters.iter_mut() {
            let Some(waiter) = slot else {
                continue;
            };
            let matched = word.load(Ordering::Acquire) & waiter.mask;
            if matched == 0 {
                continue;
            }
            let tid = waiter.tid;
            match waiter.kind {
                WaitKind::Wait if waiter.fired == 0 => {
                    word.fetch_and(!matched, Ordering::AcqRel);
                    waiter.fired = matched;
                }
                // A fired wait record keeps its bits; later ones stay in
                // the word.
                WaitKind::Wait => continue,
                // The claim: the select's next pass consumes the bits.
                WaitKind::Select => *slot = None,
            }
            // The record was the waiter's reference: its wake is in flight
            // until the unblock below lands.
            tripwire::mark_wake_pending(tid, WakeSource::Sig);
            to_wake[wake_count] = tid;
            wake_count += 1;
        }
        true
    });
    if !found {
        return Err(IpcError::Einval as i64);
    }

    for &tid in &to_wake[..wake_count] {
        sched::unblock(tid, WakeSource::Sig);
    }

    #[cfg(feature = "kernel-metrics")]
    METRICS.notify_signal.inc();

    Ok(())
}

/// One pass of `notification_wait`'s loop under NOTIFICATION_TABLE.
enum WaitPass {
    /// The wait ends with this result; `armed` says whether this pass armed.
    Exit {
        result: Result<u64, i64>,
        armed: bool,
    },
    /// Armed and registered: publish the deadline, then block.
    Registered,
}

/// Wait on a notification: if any bits matching `mask` are set, return+clear
/// them immediately. Otherwise block until signalled, destroyed or past the
/// deadline `timeout_ticks` from now (`shared::deadline_after`: `u64::MAX` is
/// none, and 0 is the current tick, a deadline `arm_deadline` finds already
/// passed, so after the fast path the first pass registers, finds it Expired
/// and returns ETIMEDOUT without blocking).
///
/// The caller's tid comes from `sched::current_tid()` (EPERM with no current
/// thread). After the fast path, which publishes nothing, each pass runs in
/// one NOTIFICATION_TABLE critical section with IRQs masked:
///
/// 1. an empty slot (destroyed) exits EINVAL;
/// 2. this thread's own record, once fired, is removed and its bits returned;
/// 3. bits matching `mask` in the word are consumed and returned;
/// 4. otherwise the thread arms (`wake::prepare_block`), then
/// 5. inserts its record if it has none, or exits ENOMEM when the list is
///    full.
///
/// After the release, `arm_deadline` publishes the deadline; if it has
/// already passed, the record is removed under the lock and the wait returns
/// its fired bits, if a signal came first, or ETIMEDOUT. Otherwise the thread
/// blocks, and the next pass re-checks. Every exit clears the deadline and
/// then, if the pass armed, cancels the arm. The result comes from the
/// notification's state and the wait's own deadline, never from which wake
/// arrived. `WAKEUP_ERRORS` is not used.
///
/// Returns the matched bits on success, or an error code.
pub fn notification_wait(id: NotificationId, mask: u64, timeout_ticks: u64) -> Result<u64, i64> {
    let idx = id.0 as usize;
    if idx >= MAX_NOTIFICATIONS {
        return Err(IpcError::Einval as i64);
    }

    let me = sched::current_tid().ok_or(IpcError::Eperm as i64)?;

    #[cfg(feature = "kernel-metrics")]
    METRICS.notify_wait.inc();

    // Fast path: no arm, no record.
    if let Some(bits) = poll(id, mask)? {
        #[cfg(feature = "kernel-metrics")]
        METRICS.notify_wake.inc();
        return Ok(bits);
    }

    let deadline = deadline_after(TICK_COUNT.load(Ordering::Relaxed), timeout_ticks);
    let (result, armed) = loop {
        let pass = daif::with_irqs_masked(|| {
            let mut table = NOTIFICATION_TABLE.lock();
            let Some(notif) = table[idx].as_mut() else {
                return WaitPass::Exit {
                    result: Err(IpcError::Einval as i64),
                    armed: false,
                };
            };
            let own = notif.own_record(me, WaitKind::Wait);
            let fired = own.and_then(|i| notif.waiters[i]).map_or(0, |w| w.fired);
            let bits = if fired != 0 {
                Some(fired)
            } else {
                notif.take_bits(mask)
            };
            if let Some(bits) = bits {
                if let Some(i) = own {
                    notif.waiters[i] = None;
                }
                return WaitPass::Exit {
                    result: Ok(bits),
                    armed: false,
                };
            }
            // Arm inside the hold that publishes the record (ADR §8 rule 1).
            wake::prepare_block(me);
            if own.is_none() {
                let waiter = Waiter {
                    tid: me,
                    mask,
                    kind: WaitKind::Wait,
                    fired: 0,
                };
                if let Err(code) = notif.insert(waiter) {
                    return WaitPass::Exit {
                        result: Err(code),
                        armed: true,
                    };
                }
            }
            WaitPass::Registered
        });
        if let WaitPass::Exit { result, armed } = pass {
            break (result, armed);
        }
        if arm_deadline(me, deadline) == DeadlineArm::Expired {
            // A fired record wins over the deadline: its bits are consumed.
            let result = daif::with_irqs_masked(|| {
                let mut table = NOTIFICATION_TABLE.lock();
                let fired = table[idx].as_mut().and_then(|notif| {
                    let i = notif.own_record(me, WaitKind::Wait)?;
                    notif.waiters[i].take().map(|w| w.fired)
                });
                match fired {
                    Some(bits) if bits != 0 => Ok(bits),
                    _ => Err(IpcError::Etimedout as i64),
                }
            });
            break (result, true);
        }
        sched::block_current(ThreadState::BlockedNotification { notification: id.0 });
    };

    clear_deadline(me);
    if armed {
        wake::cancel_block(me);
    }

    #[cfg(feature = "kernel-metrics")]
    if result.is_ok() {
        METRICS.notify_wake.inc();
    }

    result
}

/// Destroy a notification: wake all waiters with error, remove from table.
pub fn notification_destroy(id: NotificationId) {
    if id.0 as usize >= MAX_NOTIFICATIONS {
        return;
    }
    // Collect waiters to wake with error.
    let mut to_wake: [Option<ThreadId>; MAX_WAITERS_PER_NOTIFICATION] =
        [None; MAX_WAITERS_PER_NOTIFICATION];
    let found = daif::with_irqs_masked(|| {
        let mut table = NOTIFICATION_TABLE.lock();
        let Some(notif) = table[id.0 as usize].take() else {
            return false;
        };
        for (i, slot) in notif.waiters.iter().enumerate() {
            if let Some(waiter) = slot {
                to_wake[i] = Some(waiter.tid);
                // The destroyed notification held the waiter's last
                // reference: its wake is in flight until the unblock below
                // lands.
                tripwire::mark_wake_pending(waiter.tid, WakeSource::NDestroy);
            }
        }
        true
    });
    if !found {
        return;
    }

    // Wake all waiters: each one's next pass finds the slot empty and
    // returns EINVAL.
    for tid in to_wake.iter().flatten() {
        sched::unblock(*tid, WakeSource::NDestroy);
    }

    crate::kinfo!(Ipc, "Notification {} destroyed", id.0);
}

// ---------------------------------------------------------------------------
// Select support (ipc_select's notification entries)
// ---------------------------------------------------------------------------

/// Consume the bits of `mask` set in notification `id`'s word, in one
/// NOTIFICATION_TABLE critical section with IRQs masked, publishing nothing:
/// `notification_wait`'s fast path and `ipc_select`'s fast scan. `Ok(None)`
/// when none is set; `Err(EINVAL)` for an empty slot. `id` must be in range.
pub(super) fn poll(id: NotificationId, mask: u64) -> Result<Option<u64>, i64> {
    daif::with_irqs_masked(|| {
        let table = NOTIFICATION_TABLE.lock();
        let notif = table[id.0 as usize]
            .as_ref()
            .ok_or(IpcError::Einval as i64)?;
        Ok(notif.take_bits(mask))
    })
}

/// One `ipc_select` pass's check of a notification entry, for the armed
/// thread `me`, in one NOTIFICATION_TABLE critical section with IRQs masked:
/// `Err(EINVAL)` for an empty slot; the bits of `mask` set in the word,
/// consumed; otherwise `Ok(None)` once `me` has one [`WaitKind::Select`]
/// record on `id` with mask `record_mask` (the OR of every entry naming
/// `id`), inserting it if a signal claimed it or it never existed, or
/// `Err(ENOMEM)` if the list is full. `id` must be in range.
pub(super) fn select_register(
    id: NotificationId,
    mask: u64,
    record_mask: u64,
    me: ThreadId,
) -> Result<Option<u64>, i64> {
    daif::with_irqs_masked(|| {
        let mut table = NOTIFICATION_TABLE.lock();
        let notif = table[id.0 as usize]
            .as_mut()
            .ok_or(IpcError::Einval as i64)?;
        if let Some(bits) = notif.take_bits(mask) {
            return Ok(Some(bits));
        }
        match notif.own_record(me, WaitKind::Select) {
            Some(i) => {
                if let Some(waiter) = notif.waiters[i].as_mut() {
                    waiter.mask = record_mask;
                }
            }
            None => notif.insert(Waiter {
                tid: me,
                mask: record_mask,
                kind: WaitKind::Select,
                fired: 0,
            })?,
        }
        Ok(None)
    })
}

/// Remove `me`'s [`WaitKind::Select`] record from notification `id`, if any
/// (`ipc_select`'s exit), with IRQs masked. `id` must be in range.
pub(super) fn select_unregister(id: NotificationId, me: ThreadId) {
    daif::with_irqs_masked(|| {
        let mut table = NOTIFICATION_TABLE.lock();
        if let Some(notif) = table[id.0 as usize].as_mut() {
            if let Some(i) = notif.own_record(me, WaitKind::Select) {
                notif.waiters[i] = None;
            }
        }
    });
}

/// Whether `tid` has a waiter record of either kind on any notification,
/// read with IRQs masked. For self-tests.
pub(super) fn has_waiter_record(tid: ThreadId) -> bool {
    daif::with_irqs_masked(|| {
        NOTIFICATION_TABLE
            .lock()
            .iter()
            .flatten()
            .any(|notif| notif.waiters.iter().flatten().any(|w| w.tid == tid))
    })
}

/// An inert waiter record of `tid`: a [`WaitKind::Wait`] record with mask 0,
/// which no signal matches and so never fires or wakes.
fn inert_record(tid: ThreadId) -> Waiter {
    Waiter {
        tid,
        mask: 0,
        kind: WaitKind::Wait,
        fired: 0,
    }
}

/// Fill every free waiter slot of notification `id` with an inert record of
/// `tid`, through [`NotificationObject::insert`], in one NOTIFICATION_TABLE
/// critical section with IRQs masked. Returns how many inserts succeeded (0
/// when `id`'s table slot is empty, i.e. destroyed): the free slot count,
/// unless `insert` takes a slot that is not free, which the bound of one
/// insert past the list size shows. For self-tests: the caller removes the
/// records with [`clear_inert_records`] before it destroys `id`, which would
/// wake `tid`. `id` must be in range.
pub(super) fn fill_inert_records(id: NotificationId, tid: ThreadId) -> usize {
    daif::with_irqs_masked(|| {
        let mut table = NOTIFICATION_TABLE.lock();
        let Some(notif) = table[id.0 as usize].as_mut() else {
            return 0;
        };
        let mut inserted = 0;
        while inserted <= MAX_WAITERS_PER_NOTIFICATION && notif.insert(inert_record(tid)).is_ok() {
            inserted += 1;
        }
        inserted
    })
}

/// Remove `tid`'s inert records ([`fill_inert_records`]) from notification
/// `id`, with IRQs masked. `id` must be in range.
pub(super) fn clear_inert_records(id: NotificationId, tid: ThreadId) {
    daif::with_irqs_masked(|| {
        let mut table = NOTIFICATION_TABLE.lock();
        if let Some(notif) = table[id.0 as usize].as_mut() {
            for slot in notif.waiters.iter_mut() {
                if slot.is_some_and(|w| w.tid == tid && w.kind == WaitKind::Wait && w.mask == 0) {
                    *slot = None;
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Timeout support
// ---------------------------------------------------------------------------

/// Deadline storage for notification and select waits (indexed by tid;
/// `u64::MAX` is none). The timeout checker in the timer tick handler reads
/// this, so it is an IRQ-class lock (`sync::IrqSpinLock`), and every
/// thread-side hold runs with IRQs masked. The heartbeat scan (`ipc::scan`)
/// try-locks it too. A leaf: nothing is taken under it.
pub(super) static NOTIFY_DEADLINES: IrqSpinLock<[u64; MAX_THREADS]> =
    IrqSpinLock::new(LockClass::NotifyDeadlines, [u64::MAX; MAX_THREADS]);

/// Visit the lock words of NOTIFY_DEADLINES and NOTIFICATION_TABLE, for
/// `sync::held_by_stream`.
pub(super) fn irq_lock_words(f: &mut impl FnMut(u64)) {
    f(NOTIFY_DEADLINES.owner_word());
    f(NOTIFICATION_TABLE.owner_word());
}

/// Publish `me`'s wait deadline, after the arm (ADR §8 rule 1). `TICK_COUNT`
/// is read inside the NOTIFY_DEADLINES hold, which orders it after any
/// [`check_notification_timeouts`] that fired the entry: a deadline that has
/// passed (`TICK_COUNT >= deadline`) gives [`DeadlineArm::Expired`] and is not
/// published, so an entry that already fired is never re-inserted. `None`
/// (no deadline) publishes nothing and gives [`DeadlineArm::Armed`]. IRQs are
/// masked for the hold.
pub(super) fn arm_deadline(me: ThreadId, deadline: Option<u64>) -> DeadlineArm {
    let Some(deadline) = deadline else {
        return DeadlineArm::Armed;
    };
    daif::with_irqs_masked(|| {
        let mut deadlines = NOTIFY_DEADLINES.lock();
        if TICK_COUNT.load(Ordering::Relaxed) >= deadline {
            return DeadlineArm::Expired;
        }
        if let Some(entry) = deadlines.get_mut(me.0 as usize) {
            *entry = deadline;
        }
        DeadlineArm::Armed
    })
}

/// Clear `me`'s wait deadline (set it to `u64::MAX`), with IRQs masked: every
/// exit of a notification or select wait.
pub(super) fn clear_deadline(me: ThreadId) {
    daif::with_irqs_masked(|| {
        if let Some(entry) = NOTIFY_DEADLINES.lock().get_mut(me.0 as usize) {
            *entry = u64::MAX;
        }
    });
}

/// `tid`'s published deadline (`u64::MAX`: none), read with IRQs masked. For
/// self-tests.
pub(super) fn deadline_of(tid: ThreadId) -> u64 {
    daif::with_irqs_masked(|| {
        NOTIFY_DEADLINES
            .lock()
            .get(tid.0 as usize)
            .copied()
            .unwrap_or(u64::MAX)
    })
}

/// Expire notification and select deadlines. Called at the top of
/// `check_timeouts` on every tick of every CPU, IRQ context, so it runs
/// whatever the state of TIMEOUT_QUEUE.
///
/// Under `NOTIFY_DEADLINES.try_lock()` it collects the expired tids in a
/// `u64` mask (no stack array on the IRQ path) and resets their entries to
/// `u64::MAX`; a busy lock skips the tick, and the next tick retries. After
/// the release it marks each wake in flight and calls `unblock`
/// (`WakeSource::Nto`). It takes no other lock and tests no thread state:
/// the woken wait's next pass finds its deadline passed (`arm_deadline`) and
/// cleans up its own records, so no expiry is dropped.
pub fn check_notification_timeouts(now: u64) {
    let mut expired = 0u64;
    {
        // try_lock: IRQ context, never wait in the timer tick.
        let Some(mut deadlines) = NOTIFY_DEADLINES.try_lock() else {
            return;
        };
        for (slot, deadline) in deadlines.iter_mut().enumerate() {
            if *deadline <= now {
                *deadline = u64::MAX;
                expired |= 1u64 << slot;
            }
        }
    }
    while let Some(slot) = pop_lowest(&mut expired) {
        let tid = ThreadId(slot);
        // The deadline was the waiter's timeout waker: its wake is in flight
        // until the unblock below lands.
        tripwire::mark_wake_pending(tid, WakeSource::Nto);
        sched::unblock(tid, WakeSource::Nto);
    }
}

/// Forget thread `tid` (process exit step C): reset its notification
/// deadline, then remove its waiter records, a wait's and a select's alike,
/// from every notification. Each of the two IRQ-class locks is taken alone,
/// with IRQs masked.
pub(super) fn forget_waiter(tid: ThreadId) {
    clear_deadline(tid);
    daif::with_irqs_masked(|| {
        let mut table = NOTIFICATION_TABLE.lock();
        for notif in table.iter_mut().flatten() {
            for slot in notif.waiters.iter_mut() {
                if slot.is_some_and(|waiter| waiter.tid == tid) {
                    *slot = None;
                }
            }
        }
    });
}
