//! The wake helper of the loop tests (capability-lifetime loop contract): a
//! pid-1 thread, created at init and pinned to CPU 2 with the ipc-timeout
//! thread, that acts on another thread's wait while it waits.
//!
//! The driver (the ipc-timeout thread) posts one [`Command`] at a time with
//! [`post`], calls its wait, then collects the outcome with [`finish`]. The
//! helper polls the mailbox with `thread_yield`, so it runs whenever the
//! driver blocks, waits for the command's due tick, acts, records the tick it
//! acted at, and sets the mailbox to done.
//!
//! Every value crosses in atomics: the command's fields are stored before
//! the mailbox's `Release` store and read after its `Acquire` load, and the
//! outcome the other way round.

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

use crate::arch::aarch64::timer::TICK_COUNT;
use crate::ipc::channel::{ipc_cancel, ipc_recv, ipc_send};
use crate::ipc::{channel_mut, notify, timeout, CHANNEL_TABLE};
use crate::sched;
use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId};
use shared::tripwire::WakeSource;
use shared::{ChannelId, NotificationId, MAX_MESSAGE_SIZE};

use super::TEST_PID;

/// A command for the helper.
#[derive(Clone, Copy, Debug)]
pub(super) enum Command {
    /// A stale wake of `target` inside its wait. The helper waits until the
    /// due tick has passed and the wait has published the record the command
    /// acts on: its TIMEOUT_QUEUE entry when `clear_timeout` is set, and
    /// otherwise its notification deadline (`NOTIFY_DEADLINES[target]` not
    /// `u64::MAX`). With `clear_timeout` it then removes that entry
    /// (`clear_timeout_blocking`), recording whether it did. Then it calls
    /// `unblock(target, WakeSource::SelfTest)`.
    Stale {
        target: ThreadId,
        clear_timeout: bool,
    },
    /// `notification_signal(n, bits)`.
    Signal { n: NotificationId, bits: u64 },
    /// `ipc_send(ch, ...)`.
    Send { ch: ChannelId },
    /// `ipc_recv(ch, ..., timeout)`, blocking.
    RecvBlock { ch: ChannelId, timeout: u64 },
    /// `ipc_cancel(ch)`. The helper waits until the due tick has passed and
    /// a caller is published as `ch`'s `pending_caller`, so the cancel lands
    /// inside that caller's `ipc_call`.
    Cancel { ch: ChannelId },
}

/// What the helper did with a command.
#[derive(Clone, Copy, Debug)]
pub(super) struct Outcome {
    /// `TICK_COUNT` when it acted, or `None` when the driver's case ended
    /// first and the command was abandoned.
    pub(super) acted_at: Option<u64>,
    /// [`Command::Stale`] with `clear_timeout`: whether it removed an entry.
    pub(super) removed_entry: bool,
    /// The call's result: 0 or an error code for `Signal`, `Send` and
    /// `Cancel`, the byte count or an error code for `RecvBlock`, 0 for
    /// `Stale`.
    pub(super) code: i64,
}

/// The CPU the helper and the ipc-timeout thread are pinned to.
const PINNED_CPU: usize = 2;

/// Yields the driver waits, at most, for the helper to finish. The Normal
/// queue is FIFO, so each yield lets every Normal thread queued ahead on
/// CPU 2 run once before the poll runs again.
pub(super) const MAX_POLLS: usize = 32;

/// [`MAILBOX`]: no command.
const EMPTY: u32 = 0;
/// [`MAILBOX`]: a command is posted and not done.
const POSTED: u32 = 1;
/// [`MAILBOX`]: the outcome is recorded.
const DONE: u32 = 2;

/// [`CMD_KIND`] values.
const KIND_STALE: u32 = 0;
const KIND_SIGNAL: u32 = 1;
const KIND_SEND: u32 = 2;
const KIND_RECV: u32 = 3;
const KIND_CANCEL: u32 = 4;

/// [`HELPER_TID`] before the helper exists.
const NO_HELPER: u32 = u32::MAX;

/// The helper's thread slot, or [`NO_HELPER`].
static HELPER_TID: AtomicU32 = AtomicU32::new(NO_HELPER);

/// The mailbox: [`EMPTY`], [`POSTED`] or [`DONE`].
static MAILBOX: AtomicU32 = AtomicU32::new(EMPTY);
/// Set by the driver once its wait has returned: a command still waiting to
/// act is abandoned.
static CASE_OVER: AtomicBool = AtomicBool::new(false);

/// The posted command: its kind, two operands and its due tick.
static CMD_KIND: AtomicU32 = AtomicU32::new(KIND_STALE);
static CMD_A: AtomicU64 = AtomicU64::new(0);
static CMD_B: AtomicU64 = AtomicU64::new(0);
static CMD_DUE: AtomicU64 = AtomicU64::new(0);

/// The outcome: the tick acted at (`u64::MAX`: abandoned), the removed
/// entry flag and the result code.
static RES_AT: AtomicU64 = AtomicU64::new(u64::MAX);
static RES_REMOVED: AtomicBool = AtomicBool::new(false);
static RES_CODE: AtomicI64 = AtomicI64::new(0);

/// Create the helper: process 1, pinned to CPU 2 and queued there. Called
/// from `tests::init` before `enter_scheduler`.
pub(super) fn init() {
    let stack_phys = sched::alloc_kernel_stack();
    let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

    let mut thread = Thread::new_kernel(
        ThreadId(0x205),
        b"wake-helper\0\0\0\0\0",
        helper_entry as *const () as usize,
        stack_phys,
    );
    thread.sched.class = SchedulerClass::Normal;
    thread.sched.effective_class = SchedulerClass::Normal;
    thread.sched.affinity = CpuSet::single(PINNED_CPU);
    thread.context.sp = stack_virt_top as u64;
    thread.owner_pid = Some(TEST_PID);

    let idx = sched::allocate_thread(thread).expect("thread table full for wake helper");
    HELPER_TID.store(idx as u32, Ordering::Release);
    sched::enqueue_on_cpu(PINNED_CPU, ThreadId(idx as u32), SchedulerClass::Normal);
}

/// The helper's tid, once [`init`] has created it.
pub(super) fn helper_tid() -> Option<ThreadId> {
    let raw = HELPER_TID.load(Ordering::Acquire);
    (raw != NO_HELPER).then_some(ThreadId(raw))
}

/// Post `cmd`, due `due_in` ticks from now. `false`, posting nothing, when
/// the helper does not exist or an earlier command was never collected.
pub(super) fn post(cmd: Command, due_in: u64) -> bool {
    if helper_tid().is_none() || MAILBOX.load(Ordering::Acquire) != EMPTY {
        return false;
    }
    let (kind, a, b) = match cmd {
        Command::Stale {
            target,
            clear_timeout,
        } => (KIND_STALE, u64::from(target.0), u64::from(clear_timeout)),
        Command::Signal { n, bits } => (KIND_SIGNAL, u64::from(n.0), bits),
        Command::Send { ch } => (KIND_SEND, u64::from(ch.raw()), 0),
        Command::RecvBlock { ch, timeout } => (KIND_RECV, u64::from(ch.raw()), timeout),
        Command::Cancel { ch } => (KIND_CANCEL, u64::from(ch.raw()), 0),
    };
    let now = TICK_COUNT.load(Ordering::Relaxed);
    CMD_KIND.store(kind, Ordering::Relaxed);
    CMD_A.store(a, Ordering::Relaxed);
    CMD_B.store(b, Ordering::Relaxed);
    CMD_DUE.store(now.saturating_add(due_in), Ordering::Relaxed);
    CASE_OVER.store(false, Ordering::Relaxed);
    MAILBOX.store(POSTED, Ordering::Release);
    true
}

/// End the driver's case: set the case-over flag, so a command still
/// waiting to act is abandoned, then yield until the helper is done (at
/// most [`MAX_POLLS`] yields) and collect its outcome. `None` when it is not
/// done; the mailbox then stays taken, and later posts fail.
pub(super) fn finish() -> Option<Outcome> {
    CASE_OVER.store(true, Ordering::Release);
    let mut yields = 0;
    while MAILBOX.load(Ordering::Acquire) != DONE && yields < MAX_POLLS {
        sched::thread_yield();
        yields += 1;
    }
    if MAILBOX.load(Ordering::Acquire) != DONE {
        return None;
    }
    let at = RES_AT.load(Ordering::Relaxed);
    let outcome = Outcome {
        acted_at: (at != u64::MAX).then_some(at),
        removed_entry: RES_REMOVED.load(Ordering::Relaxed),
        code: RES_CODE.load(Ordering::Relaxed),
    };
    MAILBOX.store(EMPTY, Ordering::Release);
    Some(outcome)
}

/// Collect a command a failed case left behind (its [`finish`] gave
/// `None`), once the caller has ended whatever the helper was waiting for,
/// so the next case can post again. Nothing when the mailbox is empty.
pub(super) fn reclaim() {
    if MAILBOX.load(Ordering::Acquire) != EMPTY {
        let _ = finish();
    }
}

/// The failure, if any, of a [`Command::Stale`] whose outcome is `outcome`
/// (from [`finish`]): the helper never finished, abandoned the command
/// (the wait ended before its record was published), or, asked to clear
/// the timeout entry (`clear_timeout`), removed none, so the wait's
/// re-insert went unexercised.
pub(super) fn stale_failure(outcome: Option<Outcome>, clear_timeout: bool) -> Option<&'static str> {
    match outcome {
        None => Some("helper not done"),
        Some(o) if o.acted_at.is_none() => Some("stale wake abandoned"),
        Some(o) if clear_timeout && !o.removed_entry => Some("stale clear missed"),
        Some(_) => None,
    }
}

/// The helper: poll the mailbox, one yield between polls, and run each
/// posted command. `thread_yield` returns with IRQs unmasked, so the
/// commands run with IRQs on, as the other test threads do.
fn helper_entry() -> ! {
    loop {
        sched::thread_yield();
        if MAILBOX.load(Ordering::Acquire) != POSTED {
            continue;
        }
        let (at, removed, code) = run(
            CMD_KIND.load(Ordering::Relaxed),
            CMD_A.load(Ordering::Relaxed),
            CMD_B.load(Ordering::Relaxed),
            CMD_DUE.load(Ordering::Relaxed),
        );
        RES_AT.store(at, Ordering::Relaxed);
        RES_REMOVED.store(removed, Ordering::Relaxed);
        RES_CODE.store(code, Ordering::Relaxed);
        MAILBOX.store(DONE, Ordering::Release);
    }
}

/// Yield until `ready()` holds once the due tick has passed. `false` when
/// the driver's case ends first (the command is abandoned).
fn wait_until(due: u64, mut ready: impl FnMut() -> bool) -> bool {
    loop {
        if CASE_OVER.load(Ordering::Acquire) {
            return false;
        }
        if TICK_COUNT.load(Ordering::Relaxed) >= due && ready() {
            return true;
        }
        sched::thread_yield();
    }
}

/// Run one command. Returns the tick it acted at (`u64::MAX`: abandoned),
/// whether a `Stale` command removed a timeout entry, and the result code.
fn run(kind: u32, a: u64, b: u64, due: u64) -> (u64, bool, i64) {
    let abandoned = (u64::MAX, false, 0);
    match kind {
        KIND_STALE => {
            let target = ThreadId(a as u32);
            let clear = b != 0;
            let published = || {
                if clear {
                    timeout::has_timeout_entry(target)
                } else {
                    notify::deadline_of(target) != u64::MAX
                }
            };
            if !wait_until(due, published) {
                return abandoned;
            }
            let removed = clear && timeout::clear_timeout_blocking(target);
            let at = TICK_COUNT.load(Ordering::Relaxed);
            sched::unblock(target, WakeSource::SelfTest);
            (at, removed, 0)
        }
        KIND_SIGNAL => {
            if !wait_until(due, || true) {
                return abandoned;
            }
            let at = TICK_COUNT.load(Ordering::Relaxed);
            let code = notify::notification_signal(NotificationId(a as u32), b).err();
            (at, false, code.unwrap_or(0))
        }
        KIND_SEND => {
            if !wait_until(due, || true) {
                return abandoned;
            }
            let at = TICK_COUNT.load(Ordering::Relaxed);
            (at, false, ipc_send(ChannelId::from_raw(a as u32), b"LOOP"))
        }
        KIND_RECV => {
            if !wait_until(due, || true) {
                return abandoned;
            }
            let at = TICK_COUNT.load(Ordering::Relaxed);
            let mut buf = [0u8; MAX_MESSAGE_SIZE];
            let code = match ipc_recv(ChannelId::from_raw(a as u32), &mut buf, b) {
                Ok((len, _sender)) => len as i64,
                Err(e) => e,
            };
            (at, false, code)
        }
        KIND_CANCEL => {
            let ch = ChannelId::from_raw(a as u32);
            // CHANNEL_TABLE is not IRQ-class: no mask needed.
            let pending = || {
                let mut table = CHANNEL_TABLE.lock();
                channel_mut(&mut table, ch).is_ok_and(|c| c.pending_caller.is_some())
            };
            if !wait_until(due, pending) {
                return abandoned;
            }
            let at = TICK_COUNT.load(Ordering::Relaxed);
            (at, false, ipc_cancel(ch))
        }
        _ => abandoned,
    }
}
