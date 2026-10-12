//! The wake token (capability-lifetime ADR §8): one wait word per thread
//! slot, so that a wake landing between a waiter's publish and its block is
//! kept instead of lost.
//!
//! A waiting site arms its word ([`prepare_block`]) before it publishes the
//! first record a waker can find it by, then blocks (`block_current`) or
//! cancels ([`cancel_block`]). A waker that finds its target Running or
//! Runnable and Armed leaves the token (`unblock`, through [`settle_wake`]),
//! and the target's `block_current` consumes it and returns at once instead
//! of blocking. The decisions are `shared::wake_action` and
//! `shared::block_action`, host-tested with a model of every interleaving.
//!
//! [`WAIT_STATE`] is an atomic array, not a lock: it has no lock class, no
//! rank and no `IrqSpinLock`, and the token adds no lock, nesting edge or
//! `DAIFClr` site. Atomic read-modify-write is safe on it, because the array
//! is in the kernel image's `.bss`, which boot.S maps Write-Back and Inner
//! Shareable, and threads run only after M8.
//!
//! # Memory ordering: all `Relaxed`
//!
//! The protocol needs three things, and none of them needs a fence:
//!
//! 1. **A waker sees Armed.** A waker finds its waiter only through the
//!    lock that publishes the waiter's record (`CHANNEL_TABLE`,
//!    `TIMEOUT_QUEUE`, `NOTIFICATION_TABLE`, `NOTIFY_DEADLINES`,
//!    `PROCESS_WAITERS`). The arm
//!    is sequenced before the waiter's release of that lock, and the waker
//!    reads the word after its acquire of it, so the lock's
//!    release/acquire pair orders the arm before the read.
//! 2. **A token is never lost against a block.** The waker's
//!    compare-and-swap ([`settle_wake`], in `unblock`) and the block site's
//!    consume swap ([`take`], in `block_current` and `try_direct_switch`)
//!    both run under THREAD_TABLE, each in the critical section of the
//!    state check or write that goes with it, so they are totally ordered:
//!    either the wake sees the thread Blocked (and queues it) or the block
//!    sees the token (and does not block).
//! 3. **The thread's own swaps race safely with wakers.** [`prepare_block`]
//!    and [`cancel_block`] take no lock, but they are read-modify-writes on
//!    the same location as the waker's compare-and-swap, so all of them fall
//!    in one modification order, and each sees the value the one before it
//!    left.
//!
//! A future lock-free waker, one that finds a waiter without the publishing
//! lock, would need `Release` on the arm and `Acquire` on its read.

use core::sync::atomic::{AtomicU8, Ordering};

use shared::{wake_action, WaitState, WakeAction};

use crate::task::{ThreadId, ThreadState, MAX_THREADS};

/// One wait word per thread slot, holding a [`WaitState::raw`] value.
static WAIT_STATE: [AtomicU8; MAX_THREADS] =
    [const { AtomicU8::new(WaitState::Idle.raw()) }; MAX_THREADS];

/// The word of `tid`, `None` for an out-of-range tid.
#[inline(always)]
fn word(tid: ThreadId) -> Option<&'static AtomicU8> {
    WAIT_STATE.get(tid.0 as usize)
}

/// Decode a stored word. Only [`WaitState::raw`] values are ever stored, so
/// any other value is a corrupted word: a debug build asserts, and a release
/// build reads it as Idle.
#[inline(always)]
fn decode(raw: u8) -> WaitState {
    let state = WaitState::from_raw(raw);
    debug_assert!(state.is_some(), "corrupted wait word {raw}");
    state.unwrap_or(WaitState::Idle)
}

/// Arm `tid`'s wait: it is about to block, so a waker that finds it Running
/// or Runnable leaves the token instead of skipping it.
///
/// `tid` must be the calling thread's own tid (the own-tid rule, ADR §8).
/// It comes from `sched::current_tid()`, from `current_thread_and_pid()`
/// (for SVC 25) or from a thread pinned to one CPU. It never comes from
/// `current_thread_id()` on an unpinned thread, whose CPU read and
/// `CURRENT_THREAD` read happen with IRQs on, so a migration between them
/// names another CPU's thread (crash-fix N4), and never from the
/// instrumentation-only `tripwire::current_tid(cpu)`. Arming another
/// thread's word would leave that thread a token it never asked for.
///
/// The rules for a waiting site:
///
/// 1. Arm inside, or before, the critical section that publishes the first
///    record any waker can use to find you: a `TIMEOUT_QUEUE` entry, a
///    notification deadline, `pending_caller`, `waiting_receiver`, a
///    notification or select waiter record, or a `PROCESS_WAITERS` entry.
/// 2. Once armed, call `block_current` or [`cancel_block`].
/// 3. After `block_current` returns, re-check the condition under its own
///    lock and loop. Each pass re-arms before it re-publishes a waker record.
///
/// The word must be Idle (a debug build asserts it): every earlier wait of
/// the thread ended in `block_current` or `cancel_block`, which leave it
/// Idle. An out-of-range `tid` is ignored.
pub fn prepare_block(tid: ThreadId) {
    if let Some(word) = word(tid) {
        let old = word.swap(WaitState::Armed.raw(), Ordering::Relaxed);
        debug_assert_eq!(
            decode(old),
            WaitState::Idle,
            "prepare_block: tid {} already in a wait",
            tid.0
        );
    }
}

/// End `tid`'s armed wait without blocking (its condition was met, its
/// deadline passed, or it failed): swap the word to Idle. Returns whether a
/// token was pending, a wake that the caller's next re-check covers.
///
/// `tid` must be the caller's own tid, as for [`prepare_block`]. An
/// out-of-range `tid` returns `false`.
pub fn cancel_block(tid: ThreadId) -> bool {
    word(tid).is_some_and(|word| {
        decode(word.swap(WaitState::Idle.raw(), Ordering::Relaxed)) == WaitState::Woken
    })
}

/// `tid`'s wait word as it is now (Idle for an out-of-range `tid`). For
/// self-tests and the heartbeat scan; a decision must not rest on it unless
/// THREAD_TABLE orders it against the wakers.
pub fn wait_state(tid: ThreadId) -> WaitState {
    word(tid).map_or(WaitState::Idle, |word| decode(word.load(Ordering::Relaxed)))
}

/// Reset `tid`'s word to Idle: for a new thread (`allocate_thread`) and for
/// the victims of exit step B (`kill_process_threads`), both under
/// THREAD_TABLE. `block_current` resets the word of a Dead thread and of a
/// block as Dead with its consume swap ([`take`]) instead. An out-of-range
/// `tid` is ignored.
pub fn reset(tid: ThreadId) {
    if let Some(word) = word(tid) {
        word.store(WaitState::Idle.raw(), Ordering::Relaxed);
    }
}

/// The block sites' consume: swap `tid`'s word to Idle and return what it
/// was, the `word` input of `shared::block_action`. One swap is the consume
/// of a token (Woken), the clear of an arm that blocks (Armed) and the reset
/// of a Dead thread's word. Called under THREAD_TABLE, in the critical
/// section that applies the block decision (ordering point 2 above).
pub(crate) fn take(tid: ThreadId) -> WaitState {
    word(tid).map_or(WaitState::Idle, |word| {
        decode(word.swap(WaitState::Idle.raw(), Ordering::Relaxed))
    })
}

/// `unblock`'s token step for a target in `state`: if
/// `shared::wake_action(state, word)` is [`WakeAction::LeaveToken`],
/// compare-and-swap the word from Armed to Woken. Returns the word the
/// decision stands on, so that `wake_action(state, returned)` is the action
/// taken and the tripwire counts the same outcome.
///
/// Called under THREAD_TABLE (ordering point 2 above). Other wakers also act
/// only under THREAD_TABLE, so only the target itself can change the word
/// between the load and the swap, by cancelling its wait: the swap then
/// fails on Idle, and the decision is taken again on that value (a stale
/// wake, [`WakeAction::Skip`]). The loop ends because every retry is on a
/// value the target wrote, and the target writes Armed only once per pass.
pub(super) fn settle_wake(tid: ThreadId, state: &ThreadState) -> WaitState {
    let Some(word) = word(tid) else {
        return WaitState::Idle;
    };
    let mut current = decode(word.load(Ordering::Relaxed));
    while wake_action(state, current) == WakeAction::LeaveToken {
        match word.compare_exchange(
            WaitState::Armed.raw(),
            WaitState::Woken.raw(),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(seen) => current = decode(seen),
        }
    }
    current
}
