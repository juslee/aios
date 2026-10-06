//! Tripwire lines, counter keys and scan classification (crash-fix step 1b).
//!
//! Step 1b of the boot-crash fix adds detect-only instrumentation to the
//! kernel: per-CPU counters, heartbeat scans and fatal-dump context. This
//! module is its pure part, so that host tests pin both the output format the
//! soak harness parses and the rules the scans apply:
//!
//! - [`Key`]: the counter keys, in print order, with their [`Width`]s.
//! - [`CpuCounters`]: one counter row per CPU, written with load/store only.
//! - [`Sink`], [`BufSink`], [`put_dec`] and [`put_hex`]: output without
//!   `core::fmt`.
//! - [`write_line`]: the `[tripwire]` line. [`write_reentry_msg`]: the
//!   `lock re-entry:` panic message.
//! - [`classify_pc`]: where a saved PC points (N5).
//! - [`classify_slot`], [`TwoStrike`], [`EdgeCounter`] and the mask helpers
//!   ([`mask_set`], [`mask_test`], [`pop_lowest`], [`count_tids`]): the
//!   heartbeat scans.
//! - [`WakeSource`], [`UnblockOutcome`], [`ClearResult`], [`classify_reply`]
//!   and [`classify_send`]: `unblock` attribution and the N2 counters.
//!
//! # IRQ-path rules
//!
//! The kernel calls most of this from the timer IRQ, and the IRQ entry saves
//! no V registers. The code here therefore uses no `core::fmt`, no locks, no
//! stack arrays and no copies of aggregates of 16 bytes or more, which the
//! compiler lowers to NEON loads and stores. It never calls `count_ones`,
//! which is NEON `cnt` on aarch64 without FEAT_CSSC; [`count_tids`] counts
//! in general registers. Tables that code iterates at run time are `static`,
//! never `const`, so that no copy is made. [`BufSink`] and
//! [`write_reentry_msg`] serve the panic path and are the only exceptions.
//!
//! Counters and scan state use `Relaxed` load and store only. Each CPU writes
//! only its own [`CpuCounters`] row, with IRQs masked, and only CPU 0 writes
//! the scan state, so no read-modify-write is needed.
//!
//! # The line (schema `v=1`)
//!
//! ```text
//! [tripwire] v=1 src=hb cpu=0 t=12000 ncpu=4 tick=12001,11890,11875,11902 ... twmax=94000 n=14
//! ```
//!
//! - After `[tripwire]`, every token is `key=value`. The prefix `v`, `src`,
//!   `cpu`, `t` and `ncpu` comes first, then the keys in [`Key::ALL`] order,
//!   then `n`. Keys are lowercase. Values match `^[0-9,]+$`, except `src`
//!   ([`LineSrc`]: `hb`, `g1`, `panic` or `exc`).
//! - A key with several values prints them comma-separated in index order
//!   ([`Width`]): per CPU (`ncpu` values), per [`WakeSource`], per
//!   [`LockClass`], per scheduler class, per [`N2Kind`] or per [`BadchanSite`].
//! - `n` is the number of `key=value` tokens before it, the prefix included.
//!   The line ends with one `\n`; the kernel's UART sink adds the `\r`.
//! - [`LineMode::NonZero`] omits every key whose values are all 0, except
//!   the prefix, `twc`, `twn`, `twmax` and `n`. [`LineMode::Full`] prints every
//!   key.
//!
//! **Parser contract:** use the last complete line of a log. A line is
//! complete when its token count equals `n`, and a key missing from it is 0.
//! Most keys count events and only grow, but gauges ([`Key::is_gauge`]) are
//! levels or maxima, and the `*_now` gauges fall. A parser that keeps the last
//! value seen per key across lines is wrong for them.

#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use core::sync::atomic::{AtomicU64, Ordering};

use crate::lock::{LockClass, OwnerStamp, SCALAR_INDEX, TID_NONE};
use crate::sched::{SchedulerClass, ThreadState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The schema version, printed as `v=`.
pub const SCHEMA_VERSION: u64 = 1;

/// The first token of every tripwire line.
pub const LINE_PREFIX: &str = "[tripwire]";

/// The most CPUs a line reports; per-CPU keys print at most this many values.
///
/// Equal to the kernel's `MAX_CORES`.
pub const MAX_CPUS: usize = 8;

/// Scheduler classes, the width of `starved` (index = `SchedulerClass as usize`).
pub const CLASS_COUNT: usize = 4;

/// Thread ids a scan mask covers: bit t stands for thread slot t.
///
/// Equal to the kernel's `MAX_THREADS`.
pub const MASK_TIDS: u32 = 64;

/// The longest `lock re-entry:` message the kernel's panic buffer holds.
pub const MAX_REENTRY_MSG_LEN: usize = 160;

/// A queued Runnable thread is starved once it has not run for more than
/// this many ticks.
pub const STARVE_TICKS: u64 = 1000;

/// [`TwoStrike`] confirms a flag only when every online CPU whose `tick` has
/// ever advanced has advanced at least this much since the previous scan of
/// that kind.
pub const STRIKE_TICK_ADVANCE: u64 = 100;

/// `IRQ_CTX[cpu]` value: thread context.
pub const IRQ_CTX_THREAD: u8 = 0;
/// `IRQ_CTX[cpu]` value: inside the IRQ handler.
pub const IRQ_CTX_IRQ: u8 = 1;
/// `IRQ_CTX[cpu]` value: the IRQ handler's preemption check (`check_preemption`).
pub const IRQ_CTX_EXIT: u8 = 2;

/// `CALL_PHASE`/`RECV_PHASE` value: not in a call (or a receive).
pub const PHASE_IDLE: u8 = 0;
/// `CALL_PHASE`/`RECV_PHASE` step: published as the channel's pending caller
/// (or waiting receiver), with no timeout registered yet.
pub const PHASE_PUBLISHED: u8 = 1;
/// `CALL_PHASE`/`RECV_PHASE` step: the timeout is registered. Stored inside
/// the TIMEOUT_QUEUE critical section, so a waker whose `clear_timeout` came
/// later sees it. An untimed wait stores it at the point where it would
/// register.
pub const PHASE_ARMED: u8 = 2;
/// `CALL_PHASE`/`RECV_PHASE` flag, set with both steps of a wait that has no
/// timeout: `ipc_call` with timeout 0 or `ipc_recv` with `u64::MAX`. No
/// timeout can heal such a waiter, so every skipped wake of it counts as a
/// wedge precursor ([`N2Kind::Rblk`], [`N2Kind::Vblk`]). Build the stored
/// values with [`wait_phase`].
pub const PHASE_UNTIMED: u8 = 0x80;

/// The `CALL_PHASE`/`RECV_PHASE` value a waiter stores at `step`
/// ([`PHASE_PUBLISHED`] or [`PHASE_ARMED`]), flagged [`PHASE_UNTIMED`] when
/// the wait has no timeout.
#[inline]
pub const fn wait_phase(step: u8, timed: bool) -> u8 {
    if timed {
        step
    } else {
        step | PHASE_UNTIMED
    }
}

const _: () = assert!(SchedulerClass::RealTime as usize + 1 == CLASS_COUNT);

// ---------------------------------------------------------------------------
// Index sets: wake sources, N2 kinds, badchan sites
// ---------------------------------------------------------------------------

/// The caller of `unblock`, `wake_with_error` or `try_wake_select`.
///
/// The order is the index order of the 15-wide per-source keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum WakeSource {
    /// `ipc_call` wakes the waiting receiver.
    Call,
    /// `ipc_reply` wakes the caller (fallback after a failed reply switch).
    Reply,
    /// `ipc_send` wakes the waiting receiver.
    Send,
    /// `try_wake_select` from `ipc_call`.
    SelCall,
    /// `try_wake_select` from `ipc_send`.
    SelSend,
    /// `try_wake_select` from `notification_signal`.
    SelSig,
    /// `notification_signal` wakes a waiter.
    Sig,
    /// `notification_destroy` wakes its waiters.
    NDestroy,
    /// A notification wait deadline expired.
    Nto,
    /// A select wait deadline expired (notification timeout scan).
    Nsto,
    /// A process exit wakes a thread in `process_wait`.
    PWait,
    /// `wake_with_error` from `check_timeouts` (an IPC or sleep timeout).
    To,
    /// `wake_with_error` from `channel_destroy`.
    ChDestroy,
    /// `wake_with_error` from a process exit (EPIPE to peers).
    PExit,
    /// `wake_with_error` from `ipc_cancel`.
    Cancel,
}

impl WakeSource {
    /// Number of wake sources.
    pub const COUNT: usize = 15;

    /// Every source, in index order.
    pub const ALL: [WakeSource; Self::COUNT] = [
        WakeSource::Call,
        WakeSource::Reply,
        WakeSource::Send,
        WakeSource::SelCall,
        WakeSource::SelSend,
        WakeSource::SelSig,
        WakeSource::Sig,
        WakeSource::NDestroy,
        WakeSource::Nto,
        WakeSource::Nsto,
        WakeSource::PWait,
        WakeSource::To,
        WakeSource::ChDestroy,
        WakeSource::PExit,
        WakeSource::Cancel,
    ];

    /// Index into 15-wide per-source values.
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The short name used in documentation and parsers.
    pub const fn name(self) -> &'static str {
        match self {
            WakeSource::Call => "call",
            WakeSource::Reply => "reply",
            WakeSource::Send => "send",
            WakeSource::SelCall => "selcall",
            WakeSource::SelSend => "selsend",
            WakeSource::SelSig => "selsig",
            WakeSource::Sig => "sig",
            WakeSource::NDestroy => "ndestroy",
            WakeSource::Nto => "nto",
            WakeSource::Nsto => "nsto",
            WakeSource::PWait => "pwait",
            WakeSource::To => "to",
            WakeSource::ChDestroy => "chdestroy",
            WakeSource::PExit => "pexit",
            WakeSource::Cancel => "cancel",
        }
    }

    /// The `WAKE_PENDING[tid]` marker for this source: its index + 1, so
    /// that 0 means "no wake in flight".
    #[inline]
    pub const fn marker(self) -> u8 {
        (self as u8).wrapping_add(1)
    }

    /// The source a non-zero `WAKE_PENDING` marker names.
    pub fn from_marker(marker: u8) -> Option<WakeSource> {
        let idx = usize::from(marker.checked_sub(1)?);
        Self::ALL.get(idx).copied()
    }
}

/// The four N2 phase counters (index order of the `n2` key).
///
/// A skipped wake counts as a wedge precursor (`rblk`, `vblk`) only when no
/// timeout is left to wake the waiter: the waker's own `clear_timeout`
/// removed the registered one ([`ClearResult::Removed`]), or the wait is
/// untimed ([`PHASE_UNTIMED`]). A timed waiter whose timeout the waker did
/// not remove heals by that timeout.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum N2Kind {
    /// A reply skipped a timed caller that had published the call but not
    /// yet registered its timeout. The caller registers it later and heals
    /// with ETIMEDOUT.
    Rpre,
    /// A reply skipped a caller that has no timeout left: the reply's
    /// `clear_timeout` removed the registered one, or the call is untimed.
    /// The caller blocks with no waker. The exact N2 wedge precursor.
    Rblk,
    /// A send (or call) skipped a timed receiver that had published its wait
    /// but not yet registered its timeout; it heals with ETIMEDOUT.
    Vpre,
    /// A send (or call) skipped a receiver that has no timeout left: the
    /// receive-side analogue of [`N2Kind::Rblk`].
    Vblk,
}

impl N2Kind {
    /// Number of N2 kinds.
    pub const COUNT: usize = 4;

    /// Every kind, in index order.
    pub const ALL: [N2Kind; Self::COUNT] = [N2Kind::Rpre, N2Kind::Rblk, N2Kind::Vpre, N2Kind::Vblk];

    /// Index into the `n2` values.
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The short name used in documentation and parsers.
    pub const fn name(self) -> &'static str {
        match self {
            N2Kind::Rpre => "rpre",
            N2Kind::Rblk => "rblk",
            N2Kind::Vpre => "vpre",
            N2Kind::Vblk => "vblk",
        }
    }
}

/// Where an out-of-range channel id was rejected (index order of `badchan`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum BadchanSite {
    /// The capability check (`cap/mod.rs`).
    Cap,
    /// `ipc_select`'s entry validation.
    Select,
    /// The channel-table slot lookup (`ipc/mod.rs`).
    Slot,
}

impl BadchanSite {
    /// Number of sites.
    pub const COUNT: usize = 3;

    /// Every site, in index order.
    pub const ALL: [BadchanSite; Self::COUNT] =
        [BadchanSite::Cap, BadchanSite::Select, BadchanSite::Slot];

    /// Index into the `badchan` values.
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The short name used in documentation and parsers.
    pub const fn name(self) -> &'static str {
        match self {
            BadchanSite::Cap => "cap",
            BadchanSite::Select => "select",
            BadchanSite::Slot => "slot",
        }
    }
}

// ---------------------------------------------------------------------------
// Key catalogue
// ---------------------------------------------------------------------------

/// How many values a key has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    /// One per online CPU; CPU k's value lives in CPU k's counter row.
    Cpu,
    /// A single value.
    One,
    /// One per [`WakeSource`] (15).
    Source,
    /// One per [`LockClass`] (9).
    Lock,
    /// One per scheduler class (4), indexed by `SchedulerClass as usize`.
    Class,
    /// One per [`N2Kind`] (4).
    N2,
    /// One per [`BadchanSite`] (3).
    Badchan,
}

impl Width {
    /// Counter slots the key takes in each CPU row.
    pub const fn slots(self) -> usize {
        match self {
            Width::Cpu | Width::One => 1,
            Width::Source => WakeSource::COUNT,
            Width::Lock => LockClass::COUNT,
            Width::Class => CLASS_COUNT,
            Width::N2 => N2Kind::COUNT,
            Width::Badchan => BadchanSite::COUNT,
        }
    }

    /// Values the key prints on a line for `ncpu` online CPUs.
    pub const fn values(self, ncpu: usize) -> usize {
        match self {
            Width::Cpu => ncpu,
            _ => self.slots(),
        }
    }
}

/// A tripwire counter key. The declaration order is the print order.
///
/// Unless noted, a key counts events and never decreases.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Key {
    // -- Per CPU ------------------------------------------------------------
    /// Timer ticks taken.
    Tick,
    /// Switches committed by `schedule()` called from the IRQ return path.
    Irqsw,
    /// Dispatches from the IRQ return path with no current thread.
    Irqsw0,
    /// IRQ entries that found this CPU already in IRQ context.
    Nest,
    /// IRQ returns whose ELR_EL1 differed from the value saved at entry (H1).
    Elrmm,
    /// IRQ returns whose SPSR_EL1 differed from the value saved at entry.
    Spsrmm,
    /// Dispatch commits on a CPU other than the one read before masking (N4).
    N4,
    /// `schedule()` calls turned away by the re-entrancy guard.
    Insched,
    /// `restore_context` calls made while the dispatching stream still held
    /// a lock stamped with its own generation. Expected 0.
    Rsthold,
    /// Checks that found TPIDR_EL1 not equal to this CPU's MPIDR_EL1 Aff0, the
    /// CPU id the IRQ-class lock stamps with. Expected 0.
    Tpidrbad,
    // -- Global scalars -----------------------------------------------------
    /// Direct switches to a receiver that last ran on another CPU.
    Xdir,
    /// Reply switches to a caller that last ran on another CPU.
    Xrep,
    /// Direct or reply switches to a thread never dispatched before.
    Xnever,
    /// `schedule()` on the IRQ return path found its current thread Runnable
    /// and left it unqueued (N1).
    N1,
    /// Saved PCs of 0 at a restore site.
    Pcnull,
    /// Saved PCs in the physical alias of the kernel text (N5).
    Pcphys,
    /// Saved PCs outside the kernel text and its alias, or misaligned.
    Pcother,
    /// Saved SPs outside the thread's stack.
    Spbad,
    /// Reply fallbacks that skipped a caller its timeout wakes: a caller no
    /// longer in that call, or armed with a timeout the reply did not remove.
    Latereply,
    /// Replies that woke a thread not blocked in that call.
    Misrep,
    /// Wakes whose `clear_timeout` found TIMEOUT_QUEUE busy and left the
    /// waiter's entry: reply, send, call and `wake_with_error` wakes, not
    /// the timeout's own (its entry is already gone).
    Ctbusy,
    /// Out-of-range thread ids met by the instrumentation.
    Badtid,
    // -- N2 -----------------------------------------------------------------
    /// N2 phase counts, per [`N2Kind`].
    N2,
    // -- Per wake source ------------------------------------------------------
    /// `unblock` skipped a Running target.
    Ubrun,
    /// `unblock` skipped a Runnable target.
    Ubrbl,
    /// `unblock` revived a Dead, Suspended, BlockedTimer or BlockedIo thread.
    Ubdead,
    /// `unblock` found an empty slot or an out-of-range id.
    Ubnone,
    /// `unblock` queued the thread on a CPU other than the one it last ran on.
    Ubmove,
    // -- Per lock class ---------------------------------------------------------
    /// `lock()` met a preempted holder, with IRQs masked.
    Lkph,
    /// `lock()` met a preempted holder, with IRQs on.
    Lkpho,
    /// A preempted holder found running on another CPU.
    Lkphrun,
    /// `lock()` met another CPU's stamp whose CPU has dispatched since.
    Lkoxp,
    /// The holder's thread id is the waiter's own current thread.
    Lkself,
    /// `try_lock()` failed on a re-entry.
    Lktry,
    /// `try_lock()` failed on a preempted holder.
    Lktph,
    /// `lock()` spun for more than 2 s.
    Lkstk,
    // -- Scans: event counts --------------------------------------------------
    /// Completed scans A (orphans, starvation).
    Scana,
    /// Scans A skipped because a lock was busy.
    Skipa,
    /// Scans A skipped because this CPU's own stream held a lock.
    Skipaself,
    /// Completed scans B (wakers).
    Scanb,
    /// Scans B skipped because a lock was busy.
    Skipb,
    /// Scans whose confirmation the tick-advance gate withheld.
    Scanstall,
    /// Heartbeat or g1 lines printed after the maximum deferral.
    Hbdefer,
    /// Threads that entered the confirmed-orphan set.
    Orphan,
    /// Threads that entered the confirmed no-waker set.
    Nowaker,
    /// Threads that entered the confirmed wake-in-flight set.
    Wakefl,
    /// Threads that entered the confirmed starved set, per scheduler class.
    Starved,
    /// Thread ids current on two CPUs at once.
    Dupcur,
    /// Thread ids queued twice.
    Dupq,
    /// Queued threads that were not Runnable.
    Qbad,
    // -- Scans: gauges --------------------------------------------------------
    /// Gauge: threads in the confirmed-orphan set now.
    OrphanNow,
    /// Gauge: threads in the confirmed no-waker set now.
    NowakerNow,
    /// Gauge: threads in the confirmed wake-in-flight set now.
    WakeflNow,
    /// Gauge: the longest scan-A lock hold, in CNTVCT ticks.
    Scanhold1,
    /// Gauge: the longest scan-B lock hold, in CNTVCT ticks.
    Scanhold2,
    // -- Miscellaneous --------------------------------------------------------
    /// Threads the load balancer migrated.
    Lb,
    /// Enqueues dropped because the run queue was full.
    Enqfull,
    /// Out-of-range channel ids rejected, per [`BadchanSite`].
    Badchan,
    // -- Closing (always printed) ---------------------------------------------
    /// CNTVCT ticks spent printing tripwire lines, cumulative.
    Twc,
    /// Tripwire lines printed.
    Twn,
    /// Gauge: the most CNTVCT ticks one tripwire line took.
    Twmax,
}

impl Key {
    /// Number of keys.
    pub const COUNT: usize = 61;

    /// Every key, in print order.
    pub const ALL: [Key; Self::COUNT] = [
        Key::Tick,
        Key::Irqsw,
        Key::Irqsw0,
        Key::Nest,
        Key::Elrmm,
        Key::Spsrmm,
        Key::N4,
        Key::Insched,
        Key::Rsthold,
        Key::Tpidrbad,
        Key::Xdir,
        Key::Xrep,
        Key::Xnever,
        Key::N1,
        Key::Pcnull,
        Key::Pcphys,
        Key::Pcother,
        Key::Spbad,
        Key::Latereply,
        Key::Misrep,
        Key::Ctbusy,
        Key::Badtid,
        Key::N2,
        Key::Ubrun,
        Key::Ubrbl,
        Key::Ubdead,
        Key::Ubnone,
        Key::Ubmove,
        Key::Lkph,
        Key::Lkpho,
        Key::Lkphrun,
        Key::Lkoxp,
        Key::Lkself,
        Key::Lktry,
        Key::Lktph,
        Key::Lkstk,
        Key::Scana,
        Key::Skipa,
        Key::Skipaself,
        Key::Scanb,
        Key::Skipb,
        Key::Scanstall,
        Key::Hbdefer,
        Key::Orphan,
        Key::Nowaker,
        Key::Wakefl,
        Key::Starved,
        Key::Dupcur,
        Key::Dupq,
        Key::Qbad,
        Key::OrphanNow,
        Key::NowakerNow,
        Key::WakeflNow,
        Key::Scanhold1,
        Key::Scanhold2,
        Key::Lb,
        Key::Enqfull,
        Key::Badchan,
        Key::Twc,
        Key::Twn,
        Key::Twmax,
    ];

    /// The key's position in [`Key::ALL`].
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The key as printed.
    pub const fn name(self) -> &'static str {
        match self {
            Key::Tick => "tick",
            Key::Irqsw => "irqsw",
            Key::Irqsw0 => "irqsw0",
            Key::Nest => "nest",
            Key::Elrmm => "elrmm",
            Key::Spsrmm => "spsrmm",
            Key::N4 => "n4",
            Key::Insched => "insched",
            Key::Rsthold => "rsthold",
            Key::Tpidrbad => "tpidrbad",
            Key::Xdir => "xdir",
            Key::Xrep => "xrep",
            Key::Xnever => "xnever",
            Key::N1 => "n1",
            Key::Pcnull => "pcnull",
            Key::Pcphys => "pcphys",
            Key::Pcother => "pcother",
            Key::Spbad => "spbad",
            Key::Latereply => "latereply",
            Key::Misrep => "misrep",
            Key::Ctbusy => "ctbusy",
            Key::Badtid => "badtid",
            Key::N2 => "n2",
            Key::Ubrun => "ubrun",
            Key::Ubrbl => "ubrbl",
            Key::Ubdead => "ubdead",
            Key::Ubnone => "ubnone",
            Key::Ubmove => "ubmove",
            Key::Lkph => "lkph",
            Key::Lkpho => "lkpho",
            Key::Lkphrun => "lkphrun",
            Key::Lkoxp => "lkoxp",
            Key::Lkself => "lkself",
            Key::Lktry => "lktry",
            Key::Lktph => "lktph",
            Key::Lkstk => "lkstk",
            Key::Scana => "scana",
            Key::Skipa => "skipa",
            Key::Skipaself => "skipaself",
            Key::Scanb => "scanb",
            Key::Skipb => "skipb",
            Key::Scanstall => "scanstall",
            Key::Hbdefer => "hbdefer",
            Key::Orphan => "orphan",
            Key::Nowaker => "nowaker",
            Key::Wakefl => "wakefl",
            Key::Starved => "starved",
            Key::Dupcur => "dupcur",
            Key::Dupq => "dupq",
            Key::Qbad => "qbad",
            Key::OrphanNow => "orphan_now",
            Key::NowakerNow => "nowaker_now",
            Key::WakeflNow => "wakefl_now",
            Key::Scanhold1 => "scanhold1",
            Key::Scanhold2 => "scanhold2",
            Key::Lb => "lb",
            Key::Enqfull => "enqfull",
            Key::Badchan => "badchan",
            Key::Twc => "twc",
            Key::Twn => "twn",
            Key::Twmax => "twmax",
        }
    }

    /// How many values the key has.
    pub const fn width(self) -> Width {
        match self {
            Key::Tick
            | Key::Irqsw
            | Key::Irqsw0
            | Key::Nest
            | Key::Elrmm
            | Key::Spsrmm
            | Key::N4
            | Key::Insched
            | Key::Rsthold
            | Key::Tpidrbad => Width::Cpu,
            Key::N2 => Width::N2,
            Key::Ubrun | Key::Ubrbl | Key::Ubdead | Key::Ubnone | Key::Ubmove => Width::Source,
            Key::Lkph
            | Key::Lkpho
            | Key::Lkphrun
            | Key::Lkoxp
            | Key::Lkself
            | Key::Lktry
            | Key::Lktph
            | Key::Lkstk => Width::Lock,
            Key::Starved => Width::Class,
            Key::Badchan => Width::Badchan,
            _ => Width::One,
        }
    }

    /// Whether the key is a gauge: a level or a maximum, not an event count.
    ///
    /// Gauges are combined across CPU rows by maximum, counters by sum. The
    /// `*_now` gauges fall when a flag clears.
    pub const fn is_gauge(self) -> bool {
        matches!(
            self,
            Key::OrphanNow
                | Key::NowakerNow
                | Key::WakeflNow
                | Key::Scanhold1
                | Key::Scanhold2
                | Key::Twmax
        )
    }

    /// Whether [`LineMode::NonZero`] prints the key even when it is 0.
    pub const fn is_always_printed(self) -> bool {
        matches!(self, Key::Twc | Key::Twn | Key::Twmax)
    }

    /// The counter slot of value `idx` in a CPU row.
    ///
    /// `None` if `idx` is not below [`Width::slots`]. A per-CPU key has one
    /// slot (idx 0); its value for CPU k is that slot in CPU k's row.
    #[inline]
    pub fn slot(self, idx: usize) -> Option<usize> {
        if idx >= self.width().slots() {
            return None;
        }
        SLOT_BASE
            .get(self.index())
            .map(|&base| usize::from(base).wrapping_add(idx))
    }
}

/// [`Key::ALL`] as a static, so that iterating it copies nothing.
static KEYS: [Key; Key::COUNT] = Key::ALL;

/// The first counter slot of each key, in [`Key::ALL`] order.
static SLOT_BASE: [u16; Key::COUNT] = slot_bases();

/// Counter slots per CPU row: the sum of every key's [`Width::slots`].
pub const SLOTS: usize = slot_total();

// Const evaluation only: an overflow or a bad index fails the build.
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
const fn slot_bases() -> [u16; Key::COUNT] {
    let mut out = [0u16; Key::COUNT];
    let mut next = 0usize;
    let mut i = 0;
    while i < Key::COUNT {
        assert!(
            Key::ALL[i] as usize == i,
            "Key::ALL is out of declaration order"
        );
        assert!(next <= u16::MAX as usize);
        out[i] = next as u16;
        next += Key::ALL[i].width().slots();
        i += 1;
    }
    out
}

// Const evaluation only: an overflow or a bad index fails the build.
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
const fn slot_total() -> usize {
    let mut total = 0usize;
    let mut i = 0;
    while i < Key::COUNT {
        total += Key::ALL[i].width().slots();
        i += 1;
    }
    total
}

// ---------------------------------------------------------------------------
// Per-CPU counters
// ---------------------------------------------------------------------------

/// One CPU's counters, on cache lines of its own.
#[repr(C, align(64))]
struct CounterRow([AtomicU64; SLOTS]);

/// Tripwire counters: one row of [`SLOTS`] counters per CPU.
///
/// **Contract:** only CPU c writes row c, and it does so with IRQs masked, so
/// every update is a `Relaxed` load and store (no atomic read-modify-write,
/// which the lock-free paths may not use). Readers on other CPUs may see a
/// value one update old. Out-of-range CPUs and slots are ignored and read 0.
pub struct CpuCounters<const CPUS: usize> {
    rows: [CounterRow; CPUS],
}

impl<const CPUS: usize> CpuCounters<CPUS> {
    /// All counters 0.
    pub const fn new() -> Self {
        Self {
            rows: [const { CounterRow([const { AtomicU64::new(0) }; SLOTS]) }; CPUS],
        }
    }

    #[inline]
    fn cell(&self, cpu: usize, slot: usize) -> Option<&AtomicU64> {
        self.rows.get(cpu)?.0.get(slot)
    }

    /// Add `n` (wrapping) to `slot` in `cpu`'s row. The caller runs on `cpu`
    /// with IRQs masked.
    #[inline]
    pub fn add(&self, cpu: usize, slot: usize, n: u64) {
        if let Some(c) = self.cell(cpu, slot) {
            c.store(c.load(Ordering::Relaxed).wrapping_add(n), Ordering::Relaxed);
        }
    }

    /// Set `slot` in `cpu`'s row (a gauge level). Same contract as [`Self::add`].
    #[inline]
    pub fn store(&self, cpu: usize, slot: usize, value: u64) {
        if let Some(c) = self.cell(cpu, slot) {
            c.store(value, Ordering::Relaxed);
        }
    }

    /// Raise `slot` in `cpu`'s row to `value` if it is lower (a gauge
    /// maximum). Same contract as [`Self::add`].
    #[inline]
    pub fn store_max(&self, cpu: usize, slot: usize, value: u64) {
        if let Some(c) = self.cell(cpu, slot) {
            if value > c.load(Ordering::Relaxed) {
                c.store(value, Ordering::Relaxed);
            }
        }
    }

    /// One counter.
    #[inline]
    pub fn get(&self, cpu: usize, slot: usize) -> u64 {
        self.cell(cpu, slot)
            .map_or(0, |c| c.load(Ordering::Relaxed))
    }

    /// `slot` summed over every row (wrapping).
    pub fn sum(&self, slot: usize) -> u64 {
        self.rows.iter().fold(0u64, |acc, row| {
            acc.wrapping_add(row.0.get(slot).map_or(0, |c| c.load(Ordering::Relaxed)))
        })
    }

    /// The largest value of `slot` over every row.
    pub fn max(&self, slot: usize) -> u64 {
        self.rows.iter().fold(0u64, |acc, row| {
            acc.max(row.0.get(slot).map_or(0, |c| c.load(Ordering::Relaxed)))
        })
    }

    /// The printed value `idx` of `key`: CPU `idx`'s row for a per-CPU key;
    /// otherwise the maximum over rows for a gauge and the sum for a counter.
    /// 0 for an out-of-range `idx`.
    pub fn value(&self, key: Key, idx: usize) -> u64 {
        if key.width() == Width::Cpu {
            return key.slot(0).map_or(0, |slot| self.get(idx, slot));
        }
        match key.slot(idx) {
            Some(slot) if key.is_gauge() => self.max(slot),
            Some(slot) => self.sum(slot),
            None => 0,
        }
    }
}

impl<const CPUS: usize> Default for CpuCounters<CPUS> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Sinks and number output
// ---------------------------------------------------------------------------

/// A byte output: the kernel's UART sink, or a [`BufSink`].
pub trait Sink {
    /// Emit one byte.
    fn put(&mut self, byte: u8);

    /// Emit every byte of `s`.
    #[inline]
    fn put_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.put(b);
        }
    }
}

/// A fixed buffer sink. Bytes past the capacity are dropped and set
/// [`BufSink::overflowed`].
///
/// Its buffer is a byte array, which is zero-filled when the sink is built,
/// so it is for the panic path and tests, not for the IRQ path.
pub struct BufSink<const N: usize> {
    buf: [u8; N],
    len: usize,
    overflowed: bool,
}

impl<const N: usize> BufSink<N> {
    /// An empty sink.
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
            overflowed: false,
        }
    }

    /// The bytes written so far.
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or(&[])
    }

    /// The bytes written so far, as text. If the capacity cut a multi-byte
    /// character, the partial character is left out.
    pub fn as_str(&self) -> &str {
        let bytes = self.as_bytes();
        match core::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => bytes
                .get(..e.valid_up_to())
                .and_then(|b| core::str::from_utf8(b).ok())
                .unwrap_or(""),
        }
    }

    /// Bytes written so far.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing was written.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether a byte was dropped for lack of space.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }
}

impl<const N: usize> Default for BufSink<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Sink for BufSink<N> {
    #[inline]
    fn put(&mut self, byte: u8) {
        match self.buf.get_mut(self.len) {
            Some(slot) => {
                *slot = byte;
                self.len = self.len.wrapping_add(1);
            }
            None => self.overflowed = true,
        }
    }
}

/// Emit `v` in decimal, most significant digit first, with no buffer.
pub fn put_dec<S: Sink + ?Sized>(sink: &mut S, v: u64) {
    // The largest power of 10 not above v (10^19 fits in a u64).
    let mut div: u64 = 1;
    while let Some(next) = div.checked_mul(10) {
        if next > v {
            break;
        }
        div = next;
    }
    loop {
        let digit = v.checked_div(div).unwrap_or(0).wrapping_rem(10);
        sink.put(b'0'.wrapping_add(digit as u8));
        if div <= 1 {
            break;
        }
        div = div.wrapping_div(10);
    }
}

/// Emit `v` as `0x` and 16 lowercase hex digits.
pub fn put_hex<S: Sink + ?Sized>(sink: &mut S, v: u64) {
    sink.put_str("0x");
    let mut shift: u32 = 60;
    loop {
        let nibble = (v.wrapping_shr(shift) & 0xF) as u8;
        let c = if nibble < 10 {
            b'0'.wrapping_add(nibble)
        } else {
            b'a'.wrapping_add(nibble.wrapping_sub(10))
        };
        sink.put(c);
        if shift == 0 {
            break;
        }
        shift = shift.wrapping_sub(4);
    }
}

// ---------------------------------------------------------------------------
// The tripwire line
// ---------------------------------------------------------------------------

/// What printed a line (the `src=` value).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineSrc {
    /// The heartbeat (CPU 0, once per heartbeat).
    Hb,
    /// The end of the Gate 1 bench.
    G1,
    /// The panic handler.
    Panic,
    /// The synchronous exception handler.
    Exc,
}

impl LineSrc {
    /// Every source.
    pub const ALL: [LineSrc; 4] = [LineSrc::Hb, LineSrc::G1, LineSrc::Panic, LineSrc::Exc];

    /// The `src=` value.
    pub const fn name(self) -> &'static str {
        match self {
            LineSrc::Hb => "hb",
            LineSrc::G1 => "g1",
            LineSrc::Panic => "panic",
            LineSrc::Exc => "exc",
        }
    }
}

/// Which keys a line prints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineMode {
    /// Omit keys whose values are all 0 (heartbeat lines).
    NonZero,
    /// Print every key (g1 lines and fatal dumps).
    Full,
}

/// The prefix tokens: `v`, `src`, `cpu`, `t` and `ncpu`.
const PREFIX_TOKENS: u64 = 5;

/// The longest line [`write_line`] can produce, `\n` included: every key
/// printed, every value `u64::MAX`, `MAX_CPUS` CPUs, `cpu=255`.
pub const MAX_LINE_LEN: usize = max_line_len();

// Const evaluation only: an overflow or a bad index fails the build.
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
const fn max_line_len() -> usize {
    let mut src_len = 0;
    let mut i = 0;
    while i < LineSrc::ALL.len() {
        let len = LineSrc::ALL[i].name().len();
        if len > src_len {
            src_len = len;
        }
        i += 1;
    }
    let value_len = dec_len(u64::MAX);
    let mut len = LINE_PREFIX.len()
        + " v=".len()
        + dec_len(SCHEMA_VERSION)
        + " src=".len()
        + src_len
        + " cpu=".len()
        + dec_len(u8::MAX as u64)
        + " t=".len()
        + value_len
        + " ncpu=".len()
        + dec_len(MAX_CPUS as u64);
    i = 0;
    while i < Key::COUNT {
        let key = Key::ALL[i];
        let values = key.width().values(MAX_CPUS);
        // " name=" + values + (values - 1) commas
        len += 1 + key.name().len() + 1 + values * value_len + (values - 1);
        i += 1;
    }
    len + " n=".len() + dec_len(PREFIX_TOKENS + Key::COUNT as u64) + 1
}

// Const evaluation only.
#[allow(clippy::arithmetic_side_effects)]
const fn dec_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 10 {
        v /= 10;
        n += 1;
    }
    n
}

/// Write one `[tripwire]` line, pulling each value from `value(key, idx)`.
///
/// `ncpu` is the number of online CPUs, clamped to `1..=MAX_CPUS`: per-CPU
/// keys print that many values. `t` is the kernel's tick count. In
/// [`LineMode::NonZero`], `value` may be called twice per value (a zero test,
/// then the print); it should only load counters.
///
/// Uses no `core::fmt` and no buffer, so it is safe on the IRQ path.
pub fn write_line<S: Sink + ?Sized>(
    sink: &mut S,
    src: LineSrc,
    mode: LineMode,
    cpu: u8,
    t: u64,
    ncpu: usize,
    mut value: impl FnMut(Key, usize) -> u64,
) {
    let ncpu = ncpu.clamp(1, MAX_CPUS);
    sink.put_str(LINE_PREFIX);
    sink.put_str(" v=");
    put_dec(sink, SCHEMA_VERSION);
    sink.put_str(" src=");
    sink.put_str(src.name());
    sink.put_str(" cpu=");
    put_dec(sink, u64::from(cpu));
    sink.put_str(" t=");
    put_dec(sink, t);
    sink.put_str(" ncpu=");
    put_dec(sink, ncpu as u64);

    let mut tokens = PREFIX_TOKENS;
    for &key in KEYS.iter() {
        let count = key.width().values(ncpu);
        if mode == LineMode::NonZero
            && !key.is_always_printed()
            && (0..count).all(|idx| value(key, idx) == 0)
        {
            continue;
        }
        sink.put(b' ');
        sink.put_str(key.name());
        sink.put(b'=');
        for idx in 0..count {
            if idx != 0 {
                sink.put(b',');
            }
            put_dec(sink, value(key, idx));
        }
        tokens = tokens.wrapping_add(1);
    }
    sink.put_str(" n=");
    put_dec(sink, tokens);
    sink.put(b'\n');
}

// ---------------------------------------------------------------------------
// The lock re-entry message
// ---------------------------------------------------------------------------

/// The execution context of a lock acquisition, as labelled in messages.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctx {
    /// Thread context with IRQs on.
    Thread,
    /// Thread context with IRQs masked.
    ThreadOff,
    /// Inside the IRQ handler.
    Irq,
    /// The IRQ handler's preemption check.
    IrqExit,
    /// An `IRQ_CTX` value that names no context.
    Unknown,
}

impl Ctx {
    /// Label an `IRQ_CTX[cpu]` value; `irqs_masked` is DAIF.I.
    pub const fn from_raw(irq_ctx: u8, irqs_masked: bool) -> Ctx {
        match irq_ctx {
            IRQ_CTX_THREAD if irqs_masked => Ctx::ThreadOff,
            IRQ_CTX_THREAD => Ctx::Thread,
            IRQ_CTX_IRQ => Ctx::Irq,
            IRQ_CTX_EXIT => Ctx::IrqExit,
            _ => Ctx::Unknown,
        }
    }

    /// The `ctx=` label.
    pub const fn name(self) -> &'static str {
        match self {
            Ctx::Thread => "thread",
            Ctx::ThreadOff => "thread-off",
            Ctx::Irq => "irq",
            Ctx::IrqExit => "irq-exit",
            Ctx::Unknown => "?",
        }
    }

    /// Whether this is IRQ context, where a re-entry is H3 evidence.
    pub const fn is_irq(self) -> bool {
        matches!(self, Ctx::Irq | Ctx::IrqExit)
    }
}

/// A holder's call site: `Location::caller()` of its `lock()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HolderSite<'a> {
    /// The source file, as `Location::file()` gives it.
    pub file: &'a str,
    /// The line.
    pub line: u32,
}

/// Everything the `lock re-entry:` message reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReentryReport<'a> {
    /// The lock static.
    pub class: LockClass,
    /// The array index, or [`SCALAR_INDEX`] for a scalar lock.
    pub index: u8,
    /// The CPU the re-entry happened on.
    pub cpu: u8,
    /// The waiter's context.
    pub ctx: Ctx,
    /// The holder's call site, if the holder fields were consistent.
    pub holder: Option<HolderSite<'a>>,
    /// The observed lock word: its IRQS_ON bit and generation are printed.
    pub owner: OwnerStamp,
    /// The holder's thread id, or [`TID_NONE`].
    pub tid: u32,
}

/// Write the one-line `lock re-entry:` panic message.
///
/// ```text
/// lock re-entry: CURRENT_THREAD[0] on CPU 0 ctx=irq-exit holder=kernel/src/ipc/timeout.rs:118 holder_irqs=on tid=12 gen=4711
/// ```
///
/// A missing holder site or [`TID_NONE`] prints `?`. For every lock and the
/// kernel's source paths the message fits [`MAX_REENTRY_MSG_LEN`] (host-tested).
pub fn write_reentry_msg<S: Sink + ?Sized>(sink: &mut S, report: &ReentryReport<'_>) {
    sink.put_str("lock re-entry: ");
    sink.put_str(report.class.name());
    if report.index != SCALAR_INDEX {
        sink.put(b'[');
        put_dec(sink, u64::from(report.index));
        sink.put(b']');
    }
    sink.put_str(" on CPU ");
    put_dec(sink, u64::from(report.cpu));
    sink.put_str(" ctx=");
    sink.put_str(report.ctx.name());
    sink.put_str(" holder=");
    match report.holder {
        Some(site) => {
            sink.put_str(site.file);
            sink.put(b':');
            put_dec(sink, u64::from(site.line));
        }
        None => sink.put(b'?'),
    }
    sink.put_str(" holder_irqs=");
    sink.put_str(if report.owner.irqs_on() { "on" } else { "off" });
    sink.put_str(" tid=");
    if report.tid == TID_NONE {
        sink.put(b'?');
    } else {
        put_dec(sink, u64::from(report.tid));
    }
    sink.put_str(" gen=");
    put_dec(sink, report.owner.gen());
}

// ---------------------------------------------------------------------------
// Saved-PC classification (N5)
// ---------------------------------------------------------------------------

/// The kernel text bounds `[lo, hi)`, as kernel virtual addresses.
///
/// The kernel must capture them at thread level on CPU 0 (from `__text_start`
/// and `__text_end`). CPUs 1-3 run their IRQ path at physical-alias PCs, where
/// an address computed from a symbol is physical; bounds taken there would
/// make every virtual PC classify [`PcClass::Other`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextLayout {
    /// `__text_start`.
    pub lo: u64,
    /// `__text_end` (exclusive).
    pub hi: u64,
}

impl TextLayout {
    /// Whether `pc` is in `[lo, hi)`.
    #[inline]
    pub fn contains(self, pc: u64) -> bool {
        (self.lo..self.hi).contains(&pc)
    }
}

/// Where a saved PC points.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PcClass {
    /// In the kernel text and 4-byte aligned.
    Text,
    /// 0.
    Null,
    /// In the physical alias of the kernel text and 4-byte aligned (N5).
    Phys,
    /// Anything else, including a misaligned PC.
    Other,
}

impl PcClass {
    /// The counter for this class; `None` for [`PcClass::Text`].
    pub const fn key(self) -> Option<Key> {
        match self {
            PcClass::Text => None,
            PcClass::Null => Some(Key::Pcnull),
            PcClass::Phys => Some(Key::Pcphys),
            PcClass::Other => Some(Key::Pcother),
        }
    }
}

/// Classify a saved PC against the kernel text.
///
/// `virt_phys_offset` is the kernel's `VIRT_PHYS_OFFSET` (virtual minus
/// physical): the physical alias of the text is `[lo, hi)` minus it.
#[inline]
pub fn classify_pc(pc: u64, text: TextLayout, virt_phys_offset: u64) -> PcClass {
    if pc == 0 {
        return PcClass::Null;
    }
    if pc & 3 != 0 {
        return PcClass::Other;
    }
    if text.contains(pc) {
        return PcClass::Text;
    }
    let alias = TextLayout {
        lo: text.lo.wrapping_sub(virt_phys_offset),
        hi: text.hi.wrapping_sub(virt_phys_offset),
    };
    if alias.contains(pc) {
        PcClass::Phys
    } else {
        PcClass::Other
    }
}

// ---------------------------------------------------------------------------
// Thread masks
// ---------------------------------------------------------------------------

/// The mask bit of thread id `tid`, or `None` if `tid >= MASK_TIDS`.
#[inline]
pub const fn mask_bit(tid: u32) -> Option<u64> {
    if tid < MASK_TIDS {
        Some(1u64 << tid)
    } else {
        None
    }
}

/// What [`mask_set`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MaskSet {
    /// The bit was clear and is now set.
    New,
    /// The bit was already set (a duplicate: `dupq`, `dupcur`).
    Dup,
    /// `tid >= MASK_TIDS`: nothing set. The caller counts `badtid`.
    BadTid,
}

/// Set `tid`'s bit in `mask`, without panicking for any `tid`.
///
/// [`TID_NONE`] is reported as [`MaskSet::BadTid`] like any other
/// out-of-range id; a caller for which it means "no thread" filters it first.
#[inline]
#[must_use]
pub fn mask_set(mask: &mut u64, tid: u32) -> MaskSet {
    match mask_bit(tid) {
        None => MaskSet::BadTid,
        Some(bit) if *mask & bit != 0 => MaskSet::Dup,
        Some(bit) => {
            *mask |= bit;
            MaskSet::New
        }
    }
}

/// Whether `tid`'s bit is set in `mask`; `false` for an out-of-range `tid`.
#[inline]
pub const fn mask_test(mask: u64, tid: u32) -> bool {
    match mask_bit(tid) {
        Some(bit) => mask & bit != 0,
        None => false,
    }
}

/// Clear the lowest set bit of `mask` and return its thread id.
#[inline]
pub fn pop_lowest(mask: &mut u64) -> Option<u32> {
    if *mask == 0 {
        return None;
    }
    let tid = mask.trailing_zeros();
    *mask &= mask.wrapping_sub(1);
    Some(tid)
}

/// Set bits in each 4-bit value.
static NIBBLE_BITS: [u8; 16] = [0, 1, 1, 2, 1, 2, 2, 3, 1, 2, 2, 3, 2, 3, 3, 4];

/// The number of threads in `mask` (its set bits), counted in general
/// registers.
///
/// `u64::count_ones` compiles to NEON `fmov`/`cnt`/`addv` on aarch64 without
/// FEAT_CSSC, and the IRQ entry saves no V registers. The loop adds table
/// entries rather than a constant, so LLVM does not rewrite it into a
/// population count either.
#[inline]
pub fn count_tids(mask: u64) -> u64 {
    let mut rest = mask;
    let mut count = 0u64;
    while rest != 0 {
        let nibble = (rest & 0xF) as usize;
        count = count.wrapping_add(u64::from(NIBBLE_BITS.get(nibble).copied().unwrap_or(0)));
        rest = rest.wrapping_shr(4);
    }
    count
}

// ---------------------------------------------------------------------------
// Scan classification
// ---------------------------------------------------------------------------

/// A thread slot's state, as the scans see it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum SlotState {
    /// No thread in the slot.
    Empty,
    /// `ThreadState::Runnable`.
    Runnable,
    /// `ThreadState::Running`.
    Running,
    /// `ThreadState::BlockedIpc`, including sleep (`BlockedIpc { channel: u64::MAX }`,
    /// whose waker is its timeout).
    BlockedIpc,
    /// `ThreadState::BlockedNotification`.
    BlockedNotification,
    /// `ThreadState::BlockedSelect`.
    BlockedSelect,
    /// A blocked state the no-waker scan does not check: `BlockedProcessWait`,
    /// `BlockedTimer` or `BlockedIo`.
    BlockedOther,
    /// `ThreadState::Suspended`.
    Suspended,
    /// `ThreadState::Dead`.
    Dead,
}

impl SlotState {
    /// The scan state of a thread-table slot (`None` = empty).
    pub const fn of(state: Option<&ThreadState>) -> SlotState {
        match state {
            None => SlotState::Empty,
            Some(ThreadState::Runnable) => SlotState::Runnable,
            Some(ThreadState::Running) => SlotState::Running,
            Some(ThreadState::BlockedIpc { .. }) => SlotState::BlockedIpc,
            Some(ThreadState::BlockedNotification { .. }) => SlotState::BlockedNotification,
            Some(ThreadState::BlockedSelect) => SlotState::BlockedSelect,
            Some(
                ThreadState::BlockedProcessWait { .. }
                | ThreadState::BlockedTimer { .. }
                | ThreadState::BlockedIo,
            ) => SlotState::BlockedOther,
            Some(ThreadState::Suspended) => SlotState::Suspended,
            Some(ThreadState::Dead) => SlotState::Dead,
        }
    }
}

/// What the scans found about one thread slot, as bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SlotFlags(pub u8);

impl SlotFlags {
    /// No flag.
    pub const NONE: SlotFlags = SlotFlags(0);
    /// In a run queue (scan A).
    pub const QUEUED: SlotFlags = SlotFlags(1 << 0);
    /// Current on a CPU (`CURRENT_TID`, scan A).
    pub const CURRENT: SlotFlags = SlotFlags(1 << 1);
    /// Has a TIMEOUT_QUEUE entry (scan B).
    pub const TIMEOUT: SlotFlags = SlotFlags(1 << 2);
    /// Is a channel's pending caller or waiting receiver (scan B).
    pub const CHAN_REF: SlotFlags = SlotFlags(1 << 3);
    /// Has a NOTIFY_DEADLINES entry (scan B).
    pub const NDL: SlotFlags = SlotFlags(1 << 4);
    /// Is a notification's waiter (scan B).
    pub const NOTIF_REF: SlotFlags = SlotFlags(1 << 5);
    /// `WAKE_PENDING[tid] != 0`: a waker took its reference and has not yet
    /// run `unblock` (scan B).
    pub const WAKE_PENDING: SlotFlags = SlotFlags(1 << 6);

    /// These flags and `other`.
    #[inline]
    pub const fn with(self, other: SlotFlags) -> SlotFlags {
        SlotFlags(self.0 | other.0)
    }

    /// These flags, and `other` if `cond`.
    #[inline]
    pub const fn with_if(self, other: SlotFlags, cond: bool) -> SlotFlags {
        if cond {
            self.with(other)
        } else {
            self
        }
    }

    /// Whether every flag of `other` is set.
    #[inline]
    pub const fn contains(self, other: SlotFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any flag of `other` is set.
    #[inline]
    pub const fn intersects(self, other: SlotFlags) -> bool {
        self.0 & other.0 != 0
    }
}

impl core::ops::BitOr for SlotFlags {
    type Output = SlotFlags;

    #[inline]
    fn bitor(self, rhs: SlotFlags) -> SlotFlags {
        self.with(rhs)
    }
}

/// One scan's verdict on one thread slot. Two strikes confirm it (except
/// [`SlotVerdict::QueuedBad`], a single-scan count).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotVerdict {
    /// Nothing to report.
    Clear,
    /// Runnable, not queued and not current (scan A).
    Orphan,
    /// Queued and Runnable, but not run for more than [`STARVE_TICKS`] (scan A).
    Starved,
    /// Blocked with no waker and no wake in flight (scan B).
    NoWaker,
    /// Blocked with no waker, but a waker has taken its reference and not
    /// yet run `unblock` (scan B): starvation or N1(b), not a lost wakeup.
    WakeInFlight,
    /// Queued but not Runnable (`qbad`).
    QueuedBad,
}

/// Whether a queued Runnable thread last run at `last_run` is starved at `now`.
///
/// A `last_run` after `now` (a stamp from another CPU that raced the scan)
/// is not starved.
#[inline]
pub const fn is_starved(now: u64, last_run: u64) -> bool {
    now.saturating_sub(last_run) > STARVE_TICKS
}

/// Classify one thread slot.
///
/// Which inputs each verdict reads:
/// - Runnable: `QUEUED` and `CURRENT`, and `now`/`last_run` when queued.
///   Not queued and not current is [`SlotVerdict::Orphan`], even when it
///   is also old; queued and old is [`SlotVerdict::Starved`].
/// - Any other state with `QUEUED`: [`SlotVerdict::QueuedBad`].
/// - BlockedIpc, BlockedNotification and BlockedSelect, not current: the
///   waker flags. BlockedIpc is woken by `TIMEOUT` or `CHAN_REF`;
///   BlockedNotification by `NDL`, `NOTIF_REF` or `TIMEOUT`; BlockedSelect
///   by any of the four. With none of them, `WAKE_PENDING` gives
///   [`SlotVerdict::WakeInFlight`], otherwise [`SlotVerdict::NoWaker`].
/// - Everything else is [`SlotVerdict::Clear`]. A current thread that is
///   Blocked is between setting its state and switching away.
///
/// So scan A may call this before the waker flags are known and use only
/// the Orphan, Starved and QueuedBad verdicts; scan B uses NoWaker and
/// WakeInFlight.
#[inline]
pub fn classify_slot(state: SlotState, flags: SlotFlags, now: u64, last_run: u64) -> SlotVerdict {
    let queued = flags.contains(SlotFlags::QUEUED);
    let current = flags.contains(SlotFlags::CURRENT);
    if state == SlotState::Runnable {
        return if queued {
            if is_starved(now, last_run) {
                SlotVerdict::Starved
            } else {
                SlotVerdict::Clear
            }
        } else if current {
            SlotVerdict::Clear
        } else {
            SlotVerdict::Orphan
        };
    }
    if queued {
        return SlotVerdict::QueuedBad;
    }
    if current {
        return SlotVerdict::Clear;
    }
    let wakers = match state {
        SlotState::BlockedIpc => SlotFlags::TIMEOUT.with(SlotFlags::CHAN_REF),
        SlotState::BlockedNotification => SlotFlags::NDL
            .with(SlotFlags::NOTIF_REF)
            .with(SlotFlags::TIMEOUT),
        SlotState::BlockedSelect => SlotFlags::NDL
            .with(SlotFlags::CHAN_REF)
            .with(SlotFlags::NOTIF_REF)
            .with(SlotFlags::TIMEOUT),
        _ => return SlotVerdict::Clear,
    };
    if flags.intersects(wakers) {
        SlotVerdict::Clear
    } else if flags.contains(SlotFlags::WAKE_PENDING) {
        SlotVerdict::WakeInFlight
    } else {
        SlotVerdict::NoWaker
    }
}

// ---------------------------------------------------------------------------
// Two strikes and edge counting
// ---------------------------------------------------------------------------

/// The result of one [`TwoStrike::scan`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StrikeResult {
    /// Threads confirmed by this scan.
    pub confirmed: u64,
    /// The tick-advance gate withheld confirmation: count `scanstall`, and
    /// leave the kind's [`EdgeCounter`] as it is.
    pub stalled: bool,
}

/// Two-strike confirmation for one scan kind (A-orphan, A-starved,
/// B-nowaker or B-wakefl), kept in atomics so that it can live in a static.
///
/// A thread is confirmed when two consecutive completed scans of this kind
/// flag it, its `LAST_RUN` is the same at both, and every online CPU whose
/// `tick` has ever advanced (reads non-zero now) advanced by at least
/// [`STRIKE_TICK_ADVANCE`] in between. Each kind keeps its own history,
/// updated only when its own scan completes, so a scan of another kind in
/// between changes nothing here.
///
/// If such a CPU's tick has not advanced enough and a confirmation is
/// pending, the scan is a stall: nothing changes, and the first strikes are
/// kept for the next scan. A stalled vCPU therefore never lets a thread in
/// transit on it be confirmed. A CPU whose `tick` is still 0 takes no timer
/// IRQs (CPUs 1-3 on a kernel whose secondaries leave the timer PPI in
/// Group 0, #200) and does not gate; that it might be stalled is not
/// detected (owner decision, 2026-09-28).
///
/// Only CPU 0's scan uses an instance, so `Relaxed` load and store suffice.
pub struct TwoStrike<const CPUS: usize> {
    /// Threads flagged by the last completed scan.
    armed: AtomicU64,
    /// `LAST_RUN[t]` when that scan flagged t.
    last_run: [AtomicU64; MASK_TIDS as usize],
    /// Each CPU's `tick` at that scan.
    ticks: [AtomicU64; CPUS],
}

impl<const CPUS: usize> TwoStrike<CPUS> {
    /// No history.
    pub const fn new() -> Self {
        Self {
            armed: AtomicU64::new(0),
            last_run: [const { AtomicU64::new(0) }; MASK_TIDS as usize],
            ticks: [const { AtomicU64::new(0) }; CPUS],
        }
    }

    /// Threads flagged by the last completed scan (first strikes).
    #[inline]
    pub fn armed(&self) -> u64 {
        self.armed.load(Ordering::Relaxed)
    }

    /// Record a completed scan that flagged `flagged`.
    ///
    /// `last_run(t)` reads `LAST_RUN[t]` and `tick(c)` reads CPU c's `tick`
    /// counter; `ncpu` is the number of online CPUs (clamped to `CPUS`).
    pub fn scan(
        &self,
        flagged: u64,
        ncpu: usize,
        mut last_run: impl FnMut(u32) -> u64,
        mut tick: impl FnMut(usize) -> u64,
    ) -> StrikeResult {
        let online = ncpu.min(CPUS);
        let candidates = flagged & self.armed.load(Ordering::Relaxed);
        // Ranges with `get`, not `enumerate()`, whose counter carries an
        // overflow-check panic in a dev build: this runs in the timer IRQ.
        if candidates != 0 {
            for cpu in 0..online {
                let Some(before) = self.ticks.get(cpu) else {
                    break;
                };
                let now = tick(cpu);
                if now != 0
                    && now.wrapping_sub(before.load(Ordering::Relaxed)) < STRIKE_TICK_ADVANCE
                {
                    return StrikeResult {
                        confirmed: 0,
                        stalled: true,
                    };
                }
            }
        }
        let mut confirmed = 0u64;
        let mut rest = flagged;
        while let Some(tid) = pop_lowest(&mut rest) {
            let Some(snap) = self.last_run.get(tid as usize) else {
                continue;
            };
            let now = last_run(tid);
            if mask_test(candidates, tid) && snap.load(Ordering::Relaxed) == now {
                confirmed |= mask_bit(tid).unwrap_or(0);
            }
            snap.store(now, Ordering::Relaxed);
        }
        self.armed.store(flagged, Ordering::Relaxed);
        for cpu in 0..online {
            if let Some(snap) = self.ticks.get(cpu) {
                snap.store(tick(cpu), Ordering::Relaxed);
            }
        }
        StrikeResult {
            confirmed,
            stalled: false,
        }
    }
}

impl<const CPUS: usize> Default for TwoStrike<CPUS> {
    fn default() -> Self {
        Self::new()
    }
}

/// The result of one [`EdgeCounter::update`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EdgeUpdate {
    /// Threads that entered the confirmed set: add their count to the event key.
    pub entered: u64,
    /// The confirmed set now.
    pub confirmed: u64,
}

impl EdgeUpdate {
    /// How many threads entered the set.
    #[inline]
    pub fn entered_count(self) -> u64 {
        count_tids(self.entered)
    }

    /// The `*_now` gauge: the size of the confirmed set.
    #[inline]
    pub fn gauge(self) -> u64 {
        count_tids(self.confirmed)
    }
}

/// The confirmed set of one scan kind, for edge counting: a thread counts
/// once when it enters the set, and again only after leaving and re-entering.
pub struct EdgeCounter {
    confirmed: AtomicU64,
}

impl EdgeCounter {
    /// An empty set.
    pub const fn new() -> Self {
        Self {
            confirmed: AtomicU64::new(0),
        }
    }

    /// Replace the set with `confirmed` (a [`StrikeResult::confirmed`]).
    #[inline]
    pub fn update(&self, confirmed: u64) -> EdgeUpdate {
        let before = self.confirmed.load(Ordering::Relaxed);
        self.confirmed.store(confirmed, Ordering::Relaxed);
        EdgeUpdate {
            entered: confirmed & !before,
            confirmed,
        }
    }

    /// The confirmed set.
    #[inline]
    pub fn confirmed(&self) -> u64 {
        self.confirmed.load(Ordering::Relaxed)
    }
}

impl Default for EdgeCounter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Wake attribution and the N2 table
// ---------------------------------------------------------------------------

/// What `unblock` did with its target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum UnblockKind {
    /// Woke a thread in BlockedIpc, BlockedNotification, BlockedSelect or
    /// BlockedProcessWait.
    Woke,
    /// Made a Dead, Suspended, BlockedTimer or BlockedIo thread Runnable
    /// (`ubdead`).
    Revived,
    /// Skipped a Running target (`ubrun`).
    SkipRunning,
    /// Skipped a Runnable target (`ubrbl`).
    SkipRunnable,
    /// Found an empty slot or an out-of-range id (`ubnone`).
    NoThread,
}

impl UnblockKind {
    /// Every kind.
    pub const ALL: [UnblockKind; 5] = [
        UnblockKind::Woke,
        UnblockKind::Revived,
        UnblockKind::SkipRunning,
        UnblockKind::SkipRunnable,
        UnblockKind::NoThread,
    ];

    /// `unblock`'s decision for a slot (`None` = empty or out of range).
    /// Woke and Revived both make the thread Runnable and queue it; the
    /// skips return without a change.
    pub const fn of(state: Option<&ThreadState>) -> UnblockKind {
        match state {
            None => UnblockKind::NoThread,
            Some(ThreadState::Running) => UnblockKind::SkipRunning,
            Some(ThreadState::Runnable) => UnblockKind::SkipRunnable,
            Some(
                ThreadState::BlockedIpc { .. }
                | ThreadState::BlockedNotification { .. }
                | ThreadState::BlockedSelect
                | ThreadState::BlockedProcessWait { .. },
            ) => UnblockKind::Woke,
            Some(
                ThreadState::BlockedTimer { .. }
                | ThreadState::BlockedIo
                | ThreadState::Suspended
                | ThreadState::Dead,
            ) => UnblockKind::Revived,
        }
    }

    /// Whether `unblock` left the target as it was.
    #[inline]
    pub const fn skipped(self) -> bool {
        matches!(self, UnblockKind::SkipRunning | UnblockKind::SkipRunnable)
    }

    /// Whether `unblock` made the target Runnable.
    #[inline]
    pub const fn made_runnable(self) -> bool {
        matches!(self, UnblockKind::Woke | UnblockKind::Revived)
    }

    /// The per-source outcome key; `None` for [`UnblockKind::Woke`].
    pub const fn key(self) -> Option<Key> {
        match self {
            UnblockKind::Woke => None,
            UnblockKind::Revived => Some(Key::Ubdead),
            UnblockKind::SkipRunning => Some(Key::Ubrun),
            UnblockKind::SkipRunnable => Some(Key::Ubrbl),
            UnblockKind::NoThread => Some(Key::Ubnone),
        }
    }
}

/// `unblock`'s result, returned in two registers.
///
/// For [`WakeSource::Reply`] `phase`/`chan` are the target's `CALL_PHASE` and
/// `CALL_CHAN`; for [`WakeSource::Call`] and [`WakeSource::Send`] its
/// `RECV_PHASE` and `RECV_CHAN`. Both are read under THREAD_TABLE, at the
/// same moment as the state `kind` was decided on. For other sources they
/// are 0.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UnblockOutcome {
    /// What `unblock` did.
    pub kind: UnblockKind,
    /// The target's phase: [`PHASE_IDLE`], or a [`wait_phase`] value
    /// ([`PHASE_PUBLISHED`] or [`PHASE_ARMED`], maybe with [`PHASE_UNTIMED`]).
    pub phase: u8,
    /// The channel the target's phase belongs to.
    pub chan: u64,
}

const _: () = assert!(core::mem::size_of::<UnblockOutcome>() == 16);

/// What `clear_timeout` found.
///
/// A reply, send or call passes the result of its own `clear_timeout` on the
/// target to [`classify_reply`] or [`classify_send`]: only a removed entry
/// can leave a timed waiter with no waker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClearResult {
    /// The entry was removed.
    Removed,
    /// There was no entry: the timeout had fired, was not yet registered, or
    /// the wait is untimed.
    Absent,
    /// TIMEOUT_QUEUE was busy, so the entry (if any) is still there
    /// (`ctbusy`).
    Busy,
}

/// The N2-table verdict on a wake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WakeVerdict {
    /// No N2-table counter applies.
    NotCounted,
    /// One of the four `n2` counters.
    N2(N2Kind),
    /// `latereply`: the reply skipped a caller that its timeout wakes, or has
    /// already woken.
    LateReply,
    /// `misrep`: the reply woke a thread that was not waiting for it.
    Misrep,
}

impl WakeVerdict {
    /// The counter and value index to bump, if any.
    pub const fn key(self) -> Option<(Key, usize)> {
        match self {
            WakeVerdict::NotCounted => None,
            WakeVerdict::N2(kind) => Some((Key::N2, kind.index())),
            WakeVerdict::LateReply => Some((Key::Latereply, 0)),
            WakeVerdict::Misrep => Some((Key::Misrep, 0)),
        }
    }
}

/// What a skipped wake leaves the waiter, given its phase on the waker's
/// channel and the waker's own [`ClearResult`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SkipFate {
    /// Not in a wait on this channel: idle, another channel, or an invalid
    /// phase.
    NotInWait,
    /// Timed and published, timeout not yet registered: the waiter registers
    /// it later and heals with ETIMEDOUT.
    HealsBeforeArm,
    /// Timed and armed, and the waker's clear did not remove the entry: the
    /// timeout had fired already, was registered after the clear, or was left
    /// by a busy clear. That timeout wakes the waiter.
    HealsArmed,
    /// No timeout is left: the waker's clear removed it, or the wait is
    /// untimed. The waiter blocks with no waker.
    NoWaker,
}

/// The fate of a waiter whose wake `unblock` skipped.
const fn skip_fate(outcome: UnblockOutcome, channel: u64, clear: ClearResult) -> SkipFate {
    if outcome.chan != channel {
        return SkipFate::NotInWait;
    }
    let untimed = (outcome.phase & PHASE_UNTIMED) != 0;
    match outcome.phase & !PHASE_UNTIMED {
        PHASE_PUBLISHED | PHASE_ARMED if untimed => SkipFate::NoWaker,
        PHASE_PUBLISHED => SkipFate::HealsBeforeArm,
        PHASE_ARMED if matches!(clear, ClearResult::Removed) => SkipFate::NoWaker,
        PHASE_ARMED => SkipFate::HealsArmed,
        _ => SkipFate::NotInWait,
    }
}

/// Classify a reply's wake of its caller (`ipc_reply`'s `unblock` fallback,
/// or `try_reply_switch` with [`UnblockKind::Woke`] and the caller's phase).
/// `clear` is the reply's own `clear_timeout` result for the caller.
///
/// - Skipped (Running or Runnable), caller in this call with no timeout left
///   (untimed, or the reply's clear [`ClearResult::Removed`] it while armed):
///   `rblk`.
/// - Skipped, timed caller published but not armed: `rpre`.
/// - Skipped otherwise: `latereply`. The caller has left the call, or its
///   timeout (fired, registered after the reply's clear, or left by a busy
///   clear) wakes it.
/// - Made Runnable, unless the caller was armed in this call: `misrep`.
/// - Anything else: not counted.
pub const fn classify_reply(
    outcome: UnblockOutcome,
    channel: u64,
    clear: ClearResult,
) -> WakeVerdict {
    if outcome.kind.skipped() {
        return match skip_fate(outcome, channel, clear) {
            SkipFate::NoWaker => WakeVerdict::N2(N2Kind::Rblk),
            SkipFate::HealsBeforeArm => WakeVerdict::N2(N2Kind::Rpre),
            SkipFate::HealsArmed | SkipFate::NotInWait => WakeVerdict::LateReply,
        };
    }
    let armed_here = outcome.chan == channel && (outcome.phase & !PHASE_UNTIMED) == PHASE_ARMED;
    if outcome.kind.made_runnable() && !armed_here {
        return WakeVerdict::Misrep;
    }
    WakeVerdict::NotCounted
}

/// Classify a send's (or call's) wake of the waiting receiver. `clear` is
/// the waker's own `clear_timeout` result for the receiver.
///
/// Skipped (Running or Runnable) with the receiver in a receive on this
/// channel: `vblk` when no timeout is left (untimed, or the waker's clear
/// [`ClearResult::Removed`] it while armed), `vpre` when a timed receiver
/// was published but not armed. Anything else is not counted: an armed
/// receiver whose timeout the waker did not remove heals by it, and
/// misdirected receive-side wakes are out of scope.
pub const fn classify_send(
    outcome: UnblockOutcome,
    channel: u64,
    clear: ClearResult,
) -> WakeVerdict {
    if !outcome.kind.skipped() {
        return WakeVerdict::NotCounted;
    }
    match skip_fate(outcome, channel, clear) {
        SkipFate::NoWaker => WakeVerdict::N2(N2Kind::Vblk),
        SkipFate::HealsBeforeArm => WakeVerdict::N2(N2Kind::Vpre),
        SkipFate::HealsArmed | SkipFate::NotInWait => WakeVerdict::NotCounted,
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
mod tests {
    extern crate std;

    use super::*;
    use std::format;
    use std::string::String;
    use std::vec;
    use std::vec::Vec;

    /// The longest `kernel/src` Rust path today (37 bytes); the fs-walking
    /// test re-checks the message bound against the live tree.
    const LONGEST_KERNEL_PATH: &str = "kernel/src/arch/aarch64/exceptions.rs";

    /// Hazard strings the soak classifier keys on; no tripwire output may
    /// contain them.
    const HAZARDS: [&str; 6] = ["[heartbeat]", "ESR=", "ELR=", "EC=0x", "FAR=", "PANIC: "];

    fn render(
        src: LineSrc,
        mode: LineMode,
        cpu: u8,
        t: u64,
        ncpu: usize,
        value: impl FnMut(Key, usize) -> u64,
    ) -> String {
        let mut sink = BufSink::<{ MAX_LINE_LEN }>::new();
        write_line(&mut sink, src, mode, cpu, t, ncpu, value);
        assert!(!sink.overflowed());
        String::from(sink.as_str())
    }

    /// The golden-line value of `key[idx]`: distinct per key and index
    /// (every width is below 100).
    fn golden_value(key: Key, idx: usize) -> u64 {
        (key.index() as u64 + 1) * 100 + idx as u64
    }

    /// Split a line into `(key, value)` tokens, checking the prefix, the
    /// single trailing `\n`, the token shapes and the `n` count.
    fn parse(line: &str) -> Vec<(String, String)> {
        assert_eq!(line.matches('\n').count(), 1, "one newline: {line:?}");
        let body = line.strip_suffix('\n').expect("ends with a newline");
        let mut words = body.split(' ');
        assert_eq!(words.next(), Some(LINE_PREFIX));
        let tokens: Vec<(String, String)> = words
            .map(|w| {
                let (k, v) = w.split_once('=').expect("key=value");
                (String::from(k), String::from(v))
            })
            .collect();
        for (k, v) in &tokens {
            let mut chars = k.chars();
            assert!(
                chars.next().is_some_and(|c| c.is_ascii_lowercase()),
                "key {k}"
            );
            assert!(
                chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "key {k}"
            );
            if k != "src" {
                assert!(!v.is_empty(), "{k} has a value");
                assert!(
                    v.split(',')
                        .all(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit())),
                    "{k}={v}"
                );
            }
        }
        let (last, n) = tokens.last().expect("tokens");
        assert_eq!(last, "n");
        assert_eq!(
            n.parse::<usize>().unwrap(),
            tokens.len() - 1,
            "n counts the tokens before it"
        );
        tokens
    }

    fn assert_no_hazards(text: &str) {
        for h in HAZARDS {
            assert!(!text.contains(h), "{h:?} in {text:?}");
        }
    }

    // -- Numbers -------------------------------------------------------------

    fn dec(v: u64) -> String {
        let mut sink = BufSink::<20>::new();
        put_dec(&mut sink, v);
        assert!(!sink.overflowed());
        String::from(sink.as_str())
    }

    fn hex(v: u64) -> String {
        let mut sink = BufSink::<18>::new();
        put_hex(&mut sink, v);
        assert!(!sink.overflowed());
        String::from(sink.as_str())
    }

    fn sample_values() -> Vec<u64> {
        let mut values = vec![0, 1, 9, 10, 11, 99, 100, 101, 4711, u64::MAX - 1, u64::MAX];
        let mut p: u64 = 1;
        while let Some(next) = p.checked_mul(10) {
            values.extend([next - 1, next, next + 1]);
            p = next;
        }
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..if cfg!(miri) { 64 } else { 4096 } {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            values.push(x);
            values.push(x >> (x % 64));
        }
        values
    }

    #[test]
    fn put_dec_matches_format() {
        for v in sample_values() {
            assert_eq!(dec(v), format!("{v}"));
        }
    }

    #[test]
    fn put_hex_matches_format() {
        for v in sample_values() {
            assert_eq!(hex(v), format!("0x{v:016x}"));
        }
    }

    // -- Sinks ---------------------------------------------------------------

    #[test]
    fn buf_sink_drops_overflow_and_keeps_utf8() {
        let mut sink = BufSink::<4>::new();
        assert!(sink.is_empty());
        sink.put_str("hello");
        assert_eq!(sink.as_bytes(), b"hell");
        assert_eq!(sink.len(), 4);
        assert!(sink.overflowed());

        // "a" + the first byte of a two-byte character: the partial
        // character is left out of the text.
        let mut sink = BufSink::<2>::new();
        sink.put_str("a\u{e9}");
        assert_eq!(sink.as_bytes().len(), 2);
        assert_eq!(sink.as_str(), "a");
        assert!(sink.overflowed());
    }

    // -- Catalogue -----------------------------------------------------------

    #[test]
    fn key_catalogue_order_and_names() {
        assert_eq!(Key::ALL.len(), Key::COUNT);
        let mut names: Vec<&str> = vec!["v", "src", "cpu", "t", "ncpu", "n"];
        for (i, key) in Key::ALL.iter().enumerate() {
            assert_eq!(key.index(), i);
            let name = key.name();
            let mut chars = name.chars();
            assert!(
                chars.next().is_some_and(|c| c.is_ascii_lowercase()),
                "{name}"
            );
            assert!(chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
            names.push(name);
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "names are unique");
    }

    #[test]
    fn key_groups_follow_the_design() {
        let order: Vec<&str> = Key::ALL.iter().map(|k| k.name()).collect();
        let expected = [
            "tick",
            "irqsw",
            "irqsw0",
            "nest",
            "elrmm",
            "spsrmm",
            "n4",
            "insched",
            "rsthold",
            "tpidrbad",
            "xdir",
            "xrep",
            "xnever",
            "n1",
            "pcnull",
            "pcphys",
            "pcother",
            "spbad",
            "latereply",
            "misrep",
            "ctbusy",
            "badtid",
            "n2",
            "ubrun",
            "ubrbl",
            "ubdead",
            "ubnone",
            "ubmove",
            "lkph",
            "lkpho",
            "lkphrun",
            "lkoxp",
            "lkself",
            "lktry",
            "lktph",
            "lkstk",
            "scana",
            "skipa",
            "skipaself",
            "scanb",
            "skipb",
            "scanstall",
            "hbdefer",
            "orphan",
            "nowaker",
            "wakefl",
            "starved",
            "dupcur",
            "dupq",
            "qbad",
            "orphan_now",
            "nowaker_now",
            "wakefl_now",
            "scanhold1",
            "scanhold2",
            "lb",
            "enqfull",
            "badchan",
            "twc",
            "twn",
            "twmax",
        ];
        assert_eq!(order, expected);

        let widths = |w: Width| -> Vec<&str> {
            Key::ALL
                .iter()
                .filter(|k| k.width() == w)
                .map(|k| k.name())
                .collect()
        };
        assert_eq!(
            widths(Width::Cpu),
            [
                "tick", "irqsw", "irqsw0", "nest", "elrmm", "spsrmm", "n4", "insched", "rsthold",
                "tpidrbad"
            ]
        );
        assert_eq!(
            widths(Width::Source),
            ["ubrun", "ubrbl", "ubdead", "ubnone", "ubmove"]
        );
        assert_eq!(
            widths(Width::Lock),
            ["lkph", "lkpho", "lkphrun", "lkoxp", "lkself", "lktry", "lktph", "lkstk"]
        );
        assert_eq!(widths(Width::Class), ["starved"]);
        assert_eq!(widths(Width::N2), ["n2"]);
        assert_eq!(widths(Width::Badchan), ["badchan"]);

        let gauges: Vec<&str> = Key::ALL
            .iter()
            .filter(|k| k.is_gauge())
            .map(|k| k.name())
            .collect();
        assert_eq!(
            gauges,
            [
                "orphan_now",
                "nowaker_now",
                "wakefl_now",
                "scanhold1",
                "scanhold2",
                "twmax"
            ]
        );
        let always: Vec<&str> = Key::ALL
            .iter()
            .filter(|k| k.is_always_printed())
            .map(|k| k.name())
            .collect();
        assert_eq!(always, ["twc", "twn", "twmax"]);

        assert_eq!(Width::Cpu.slots(), 1);
        assert_eq!(Width::Cpu.values(4), 4);
        assert_eq!(Width::One.values(4), 1);
        assert_eq!(Width::Source.slots(), 15);
        assert_eq!(Width::Lock.slots(), 9);
        assert_eq!(Width::Class.slots(), 4);
        assert_eq!(Width::N2.slots(), 4);
        assert_eq!(Width::Badchan.slots(), 3);
    }

    #[test]
    fn key_slots_are_contiguous_and_checked() {
        let mut next = 0;
        for key in Key::ALL {
            let slots = key.width().slots();
            for idx in 0..slots {
                assert_eq!(key.slot(idx), Some(next + idx), "{}[{idx}]", key.name());
            }
            assert_eq!(key.slot(slots), None);
            assert_eq!(key.slot(usize::MAX), None);
            next += slots;
        }
        assert_eq!(next, SLOTS);
        assert_eq!(SLOTS, 10 + 12 + 4 + 5 * 15 + 8 * 9 + 13 + 4 + 5 + 2 + 3 + 3);
    }

    #[test]
    fn index_sets_have_fixed_names_and_order() {
        let sources: Vec<&str> = WakeSource::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(
            sources,
            [
                "call",
                "reply",
                "send",
                "selcall",
                "selsend",
                "selsig",
                "sig",
                "ndestroy",
                "nto",
                "nsto",
                "pwait",
                "to",
                "chdestroy",
                "pexit",
                "cancel"
            ]
        );
        for (i, s) in WakeSource::ALL.iter().enumerate() {
            assert_eq!(s.index(), i);
            assert_eq!(usize::from(s.marker()), i + 1);
            assert_eq!(WakeSource::from_marker(s.marker()), Some(*s));
        }
        assert_eq!(WakeSource::from_marker(0), None);
        assert_eq!(WakeSource::from_marker(16), None);
        assert_eq!(WakeSource::from_marker(u8::MAX), None);

        let n2: Vec<&str> = N2Kind::ALL.iter().map(|k| k.name()).collect();
        assert_eq!(n2, ["rpre", "rblk", "vpre", "vblk"]);
        let sites: Vec<&str> = BadchanSite::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(sites, ["cap", "select", "slot"]);
        for (i, k) in N2Kind::ALL.iter().enumerate() {
            assert_eq!(k.index(), i);
        }
        for (i, s) in BadchanSite::ALL.iter().enumerate() {
            assert_eq!(s.index(), i);
        }
        assert_eq!(SchedulerClass::Idle as usize, 0);
        assert_eq!(SchedulerClass::RealTime as usize, CLASS_COUNT - 1);
    }

    // -- CpuCounters ---------------------------------------------------------

    #[test]
    fn counter_rows_are_isolated_and_wrap() {
        let c = CpuCounters::<4>::new();
        assert_eq!(core::mem::align_of::<CounterRow>(), 64);
        assert_eq!(core::mem::size_of::<CounterRow>() % 64, 0);
        assert!(core::mem::size_of::<CounterRow>() >= SLOTS * 8);

        let tick = Key::Tick.slot(0).unwrap();
        for cpu in 0..4 {
            c.add(cpu, tick, cpu as u64 + 1);
        }
        c.add(1, tick, 10);
        for cpu in 0..4 {
            let expect = cpu as u64 + 1 + if cpu == 1 { 10 } else { 0 };
            assert_eq!(c.get(cpu, tick), expect);
            assert_eq!(c.value(Key::Tick, cpu), expect);
        }
        assert_eq!(c.value(Key::Tick, 4), 0, "no fifth CPU");

        // Neighbouring slots are untouched.
        assert_eq!(c.get(1, Key::Irqsw.slot(0).unwrap()), 0);

        // A global key sums the rows; its indices are separate slots.
        let reply = Key::Ubrun.slot(WakeSource::Reply.index()).unwrap();
        c.add(0, reply, 2);
        c.add(3, reply, 3);
        assert_eq!(c.value(Key::Ubrun, WakeSource::Reply.index()), 5);
        assert_eq!(c.value(Key::Ubrun, WakeSource::Call.index()), 0);
        assert_eq!(c.value(Key::Ubrun, WakeSource::COUNT), 0);

        // Wrap-around, per row.
        let x = Key::Xdir.slot(0).unwrap();
        c.add(2, x, u64::MAX);
        c.add(2, x, 2);
        assert_eq!(c.get(2, x), 1);
        c.add(0, x, u64::MAX);
        assert_eq!(c.sum(x), 0, "the sum wraps too");

        // Gauges combine by maximum.
        let now = Key::OrphanNow.slot(0).unwrap();
        c.store(0, now, 3);
        c.store(1, now, 2);
        assert_eq!(c.value(Key::OrphanNow, 0), 3);
        c.store(0, now, 1);
        assert_eq!(c.value(Key::OrphanNow, 0), 2, "a gauge can fall");
        let hold = Key::Scanhold1.slot(0).unwrap();
        c.store_max(0, hold, 10);
        c.store_max(0, hold, 7);
        assert_eq!(c.value(Key::Scanhold1, 0), 10);

        // Out of range: ignored, never a panic.
        c.add(4, tick, 1);
        c.add(0, SLOTS, 1);
        c.store(9, 0, 1);
        c.store_max(0, usize::MAX, 1);
        assert_eq!(c.get(4, tick), 0);
        assert_eq!(c.get(0, SLOTS), 0);
    }

    // -- The line ------------------------------------------------------------

    const GOLDEN_FULL: &str =
        "[tripwire] v=1 src=g1 cpu=0 t=12000 ncpu=4 tick=100,101,102,103 irqsw=200,201,202,203 \
         irqsw0=300,301,302,303 nest=400,401,402,403 elrmm=500,501,502,503 spsrmm=600,601,602,603 \
         n4=700,701,702,703 insched=800,801,802,803 rsthold=900,901,902,903 \
         tpidrbad=1000,1001,1002,1003 xdir=1100 xrep=1200 xnever=1300 n1=1400 pcnull=1500 \
         pcphys=1600 pcother=1700 spbad=1800 latereply=1900 misrep=2000 ctbusy=2100 badtid=2200 \
         n2=2300,2301,2302,2303 \
         ubrun=2400,2401,2402,2403,2404,2405,2406,2407,2408,2409,2410,2411,2412,2413,2414 \
         ubrbl=2500,2501,2502,2503,2504,2505,2506,2507,2508,2509,2510,2511,2512,2513,2514 \
         ubdead=2600,2601,2602,2603,2604,2605,2606,2607,2608,2609,2610,2611,2612,2613,2614 \
         ubnone=2700,2701,2702,2703,2704,2705,2706,2707,2708,2709,2710,2711,2712,2713,2714 \
         ubmove=2800,2801,2802,2803,2804,2805,2806,2807,2808,2809,2810,2811,2812,2813,2814 \
         lkph=2900,2901,2902,2903,2904,2905,2906,2907,2908 \
         lkpho=3000,3001,3002,3003,3004,3005,3006,3007,3008 \
         lkphrun=3100,3101,3102,3103,3104,3105,3106,3107,3108 \
         lkoxp=3200,3201,3202,3203,3204,3205,3206,3207,3208 \
         lkself=3300,3301,3302,3303,3304,3305,3306,3307,3308 \
         lktry=3400,3401,3402,3403,3404,3405,3406,3407,3408 \
         lktph=3500,3501,3502,3503,3504,3505,3506,3507,3508 \
         lkstk=3600,3601,3602,3603,3604,3605,3606,3607,3608 scana=3700 skipa=3800 skipaself=3900 \
         scanb=4000 skipb=4100 scanstall=4200 hbdefer=4300 orphan=4400 nowaker=4500 wakefl=4600 \
         starved=4700,4701,4702,4703 dupcur=4800 dupq=4900 qbad=5000 orphan_now=5100 \
         nowaker_now=5200 wakefl_now=5300 scanhold1=5400 scanhold2=5500 lb=5600 enqfull=5700 \
         badchan=5800,5801,5802 twc=5900 twn=6000 twmax=6100 n=66\n";

    #[test]
    fn full_line_golden() {
        let line = render(LineSrc::G1, LineMode::Full, 0, 12000, 4, golden_value);
        assert_eq!(line, GOLDEN_FULL);
        let tokens = parse(&line);
        assert_eq!(tokens.len(), 5 + Key::COUNT + 1);
    }

    #[test]
    fn full_line_round_trips_every_value() {
        let line = render(LineSrc::G1, LineMode::Full, 0, 12000, 4, golden_value);
        let tokens = parse(&line);
        let prefix: Vec<(&str, &str)> = tokens[..5]
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            prefix,
            [
                ("v", "1"),
                ("src", "g1"),
                ("cpu", "0"),
                ("t", "12000"),
                ("ncpu", "4")
            ]
        );
        for (key, (name, values)) in Key::ALL.iter().zip(&tokens[5..]) {
            assert_eq!(name, key.name());
            let got: Vec<u64> = values.split(',').map(|v| v.parse().unwrap()).collect();
            let want: Vec<u64> = (0..key.width().values(4))
                .map(|i| golden_value(*key, i))
                .collect();
            assert_eq!(got, want, "{name}");
        }
    }

    const GOLDEN_NONZERO: &str =
        "[tripwire] v=1 src=hb cpu=0 t=12000 ncpu=4 tick=12001,11890,11875,11902 \
         irqsw=812,799,801,790 ubrun=0,3,0,0,0,0,0,0,0,0,0,0,0,0,0 lkph=2,0,0,0,0,0,0,0,0 \
         starved=1,0,0,0 badchan=4,0,6 twc=812345 twn=12 twmax=94000 n=14\n";

    fn nonzero_value(key: Key, idx: usize) -> u64 {
        match (key, idx) {
            (Key::Tick, i) => [12001, 11890, 11875, 11902][i],
            (Key::Irqsw, i) => [812, 799, 801, 790][i],
            (Key::Ubrun, 1) => 3,
            (Key::Lkph, 0) => 2,
            (Key::Starved, 0) => 1,
            (Key::Badchan, 0) => 4,
            (Key::Badchan, 2) => 6,
            (Key::Twc, _) => 812_345,
            (Key::Twn, _) => 12,
            (Key::Twmax, _) => 94_000,
            _ => 0,
        }
    }

    #[test]
    fn nonzero_line_golden() {
        let line = render(LineSrc::Hb, LineMode::NonZero, 0, 12000, 4, nonzero_value);
        assert_eq!(line, GOLDEN_NONZERO);
        let names: Vec<String> = parse(&line).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            names,
            [
                "v", "src", "cpu", "t", "ncpu", "tick", "irqsw", "ubrun", "lkph", "starved",
                "badchan", "twc", "twn", "twmax", "n"
            ]
        );
    }

    #[test]
    fn nonzero_line_keeps_prefix_and_closing_when_all_zero() {
        let line = render(LineSrc::Hb, LineMode::NonZero, 2, 0, 4, |_, _| 0);
        assert_eq!(
            line,
            "[tripwire] v=1 src=hb cpu=2 t=0 ncpu=4 twc=0 twn=0 twmax=0 n=8\n"
        );
        parse(&line);
    }

    /// Miri adds no checks to this safe, single-threaded formatting code and
    /// runs it about 10^4 times slower, so Miri renders fewer lines: one key
    /// per [`Width`] plus an always-printed key, and one line per mode.
    const MIRI_KEYS: [Key; 8] = [
        Key::Tick,
        Key::Xdir,
        Key::N2,
        Key::Ubmove,
        Key::Lkstk,
        Key::Starved,
        Key::Badchan,
        Key::Twmax,
    ];

    #[test]
    fn nonzero_line_prints_a_key_once_any_value_is_set() {
        // Each key alone, at its last index: printed with all its values.
        let keys: &[Key] = if cfg!(miri) { &MIRI_KEYS } else { &Key::ALL };
        for &key in keys {
            let last = key.width().values(4) - 1;
            let line = render(LineSrc::Hb, LineMode::NonZero, 0, 1, 4, |k, i| {
                u64::from(k == key && i == last)
            });
            let tokens = parse(&line);
            let found = tokens
                .iter()
                .find(|(k, _)| k == key.name())
                .expect("printed");
            let values: Vec<&str> = found.1.split(',').collect();
            assert_eq!(values.len(), last + 1);
            assert_eq!(values[last], "1");
            let expected_tokens = 5 + 3 + usize::from(!key.is_always_printed()) + 1;
            assert_eq!(tokens.len(), expected_tokens, "{}", key.name());
        }
    }

    #[test]
    fn lines_have_one_newline_and_no_hazards() {
        // Values are digits, so a hazard could only come from the fixed text:
        // the prefix, the names and the separators.
        for src in LineSrc::ALL {
            assert_no_hazards(&format!("{LINE_PREFIX} v=1 src={} ", src.name()));
        }
        for key in Key::ALL {
            assert_no_hazards(&format!(" {}=", key.name()));
        }
        let srcs: &[LineSrc] = if cfg!(miri) {
            &[LineSrc::Panic]
        } else {
            &LineSrc::ALL
        };
        let values: &[fn(Key, usize) -> u64] = if cfg!(miri) {
            &[golden_value]
        } else {
            &[golden_value, nonzero_value, |_, _| u64::MAX]
        };
        for &src in srcs {
            for mode in [LineMode::NonZero, LineMode::Full] {
                for &value in values {
                    let line = render(src, mode, 3, 77, 4, value);
                    parse(&line);
                    assert_no_hazards(&line);
                }
            }
        }
        let names: Vec<&str> = LineSrc::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(names, ["hb", "g1", "panic", "exc"]);
    }

    #[test]
    fn max_values_fit_max_line_len_exactly() {
        let mut sink = BufSink::<{ MAX_LINE_LEN }>::new();
        write_line(
            &mut sink,
            LineSrc::Panic,
            LineMode::Full,
            u8::MAX,
            u64::MAX,
            MAX_CPUS,
            |_, _| u64::MAX,
        );
        assert!(!sink.overflowed());
        assert_eq!(sink.len(), MAX_LINE_LEN, "the bound is tight");
        parse(sink.as_str());

        let mut short = BufSink::<{ MAX_LINE_LEN - 1 }>::new();
        write_line(
            &mut short,
            LineSrc::Panic,
            LineMode::Full,
            u8::MAX,
            u64::MAX,
            MAX_CPUS,
            |_, _| u64::MAX,
        );
        assert!(short.overflowed(), "an undersized sink reports it");
        assert_eq!(short.len(), MAX_LINE_LEN - 1);
    }

    #[test]
    fn ncpu_is_clamped() {
        let line = render(LineSrc::Hb, LineMode::Full, 0, 1, 0, |_, _| 5);
        let tokens = parse(&line);
        assert!(tokens.contains(&(String::from("ncpu"), String::from("1"))));
        assert!(tokens.contains(&(String::from("tick"), String::from("5"))));

        let line = render(LineSrc::Hb, LineMode::Full, 0, 1, 100, |_, _| 5);
        let tokens = parse(&line);
        assert!(tokens.contains(&(String::from("ncpu"), format!("{MAX_CPUS}"))));
        let tick = &tokens.iter().find(|(k, _)| k == "tick").unwrap().1;
        assert_eq!(tick.split(',').count(), MAX_CPUS);
    }

    // -- The re-entry message ------------------------------------------------

    fn reentry(report: &ReentryReport<'_>) -> (String, bool) {
        let mut sink = BufSink::<MAX_REENTRY_MSG_LEN>::new();
        write_reentry_msg(&mut sink, report);
        (String::from(sink.as_str()), sink.overflowed())
    }

    /// The longest message for `class` with a holder in `file`.
    fn worst_report(class: LockClass, file: &str) -> ReentryReport<'_> {
        let per_cpu = matches!(class, LockClass::CurrentThread | LockClass::RunQueues);
        ReentryReport {
            class,
            index: if per_cpu { 7 } else { SCALAR_INDEX },
            cpu: 7,
            ctx: Ctx::ThreadOff,
            holder: Some(HolderSite { file, line: 99_999 }),
            owner: OwnerStamp::new(7, OwnerStamp::GEN_MASK, false),
            tid: 99,
        }
    }

    #[test]
    fn reentry_msg_example() {
        let report = ReentryReport {
            class: LockClass::CurrentThread,
            index: 0,
            cpu: 0,
            ctx: Ctx::IrqExit,
            holder: Some(HolderSite {
                file: "kernel/src/ipc/timeout.rs",
                line: 118,
            }),
            owner: OwnerStamp::new(0, 4711, true),
            tid: 12,
        };
        let (msg, overflowed) = reentry(&report);
        assert!(!overflowed);
        assert_eq!(
            msg,
            "lock re-entry: CURRENT_THREAD[0] on CPU 0 ctx=irq-exit \
             holder=kernel/src/ipc/timeout.rs:118 holder_irqs=on tid=12 gen=4711"
        );
    }

    #[test]
    fn reentry_msg_prints_question_marks_for_missing_fields() {
        let report = ReentryReport {
            class: LockClass::ThreadTable,
            index: SCALAR_INDEX,
            cpu: 2,
            ctx: Ctx::Irq,
            holder: None,
            owner: OwnerStamp::new(2, 0, false),
            tid: TID_NONE,
        };
        let (msg, _) = reentry(&report);
        assert_eq!(
            msg,
            "lock re-entry: THREAD_TABLE on CPU 2 ctx=irq holder=? holder_irqs=off tid=? gen=0"
        );
        let unknown = ReentryReport {
            ctx: Ctx::Unknown,
            ..report
        };
        assert!(reentry(&unknown).0.contains(" ctx=? "));
    }

    #[test]
    fn reentry_msg_worst_case_fits() {
        assert_eq!(dec(OwnerStamp::GEN_MASK).len(), 17);
        let longest_ctx = [
            Ctx::Thread,
            Ctx::ThreadOff,
            Ctx::Irq,
            Ctx::IrqExit,
            Ctx::Unknown,
        ]
        .iter()
        .map(|c| c.name().len())
        .max();
        assert_eq!(longest_ctx, Some(Ctx::ThreadOff.name().len()));
        for class in LockClass::ALL {
            let (msg, overflowed) = reentry(&worst_report(class, LONGEST_KERNEL_PATH));
            assert!(!overflowed, "{msg}");
            assert!(
                msg.len() <= MAX_REENTRY_MSG_LEN,
                "{} bytes: {msg}",
                msg.len()
            );
            assert!(msg.starts_with("lock re-entry: "));
            assert!(!msg.contains('\n'));
            assert_no_hazards(&msg);
        }
        let (msg, _) = reentry(&worst_report(LockClass::CurrentThread, LONGEST_KERNEL_PATH));
        assert_eq!(
            msg,
            "lock re-entry: CURRENT_THREAD[7] on CPU 7 ctx=thread-off \
             holder=kernel/src/arch/aarch64/exceptions.rs:99999 holder_irqs=off tid=99 \
             gen=18014398509481983"
        );
    }

    /// Every Rust file under `kernel/src`, as `Location::file()` names it.
    fn kernel_source_paths() -> Vec<String> {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let mut out = Vec::new();
        let mut dirs = vec![workspace.join("kernel").join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("readable kernel/src") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let rel = path.strip_prefix(workspace).expect("under the workspace");
                    let parts: Vec<&str> = rel
                        .components()
                        .map(|c| c.as_os_str().to_str().expect("utf-8"))
                        .collect();
                    out.push(parts.join("/"));
                }
            }
        }
        out
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the kernel source tree")]
    fn reentry_msg_fits_for_every_kernel_source_path() {
        let paths = kernel_source_paths();
        assert!(paths.iter().any(|p| p == "kernel/src/main.rs"));
        let longest = paths.iter().map(String::len).max().expect("kernel sources");
        assert!(
            longest <= LONGEST_KERNEL_PATH.len(),
            "a kernel path is longer than {LONGEST_KERNEL_PATH}: update the constant and \
             re-check the message bound"
        );
        for path in &paths {
            for class in LockClass::ALL {
                let (msg, overflowed) = reentry(&worst_report(class, path));
                assert!(!overflowed && msg.len() <= MAX_REENTRY_MSG_LEN, "{msg}");
            }
        }
    }

    #[test]
    fn ctx_labels() {
        assert_eq!(Ctx::from_raw(IRQ_CTX_THREAD, false), Ctx::Thread);
        assert_eq!(Ctx::from_raw(IRQ_CTX_THREAD, true), Ctx::ThreadOff);
        for masked in [false, true] {
            assert_eq!(Ctx::from_raw(IRQ_CTX_IRQ, masked), Ctx::Irq);
            assert_eq!(Ctx::from_raw(IRQ_CTX_EXIT, masked), Ctx::IrqExit);
            assert_eq!(Ctx::from_raw(3, masked), Ctx::Unknown);
            assert_eq!(Ctx::from_raw(u8::MAX, masked), Ctx::Unknown);
        }
        let names: Vec<&str> = [
            Ctx::Thread,
            Ctx::ThreadOff,
            Ctx::Irq,
            Ctx::IrqExit,
            Ctx::Unknown,
        ]
        .iter()
        .map(|c| c.name())
        .collect();
        assert_eq!(names, ["thread", "thread-off", "irq", "irq-exit", "?"]);
        assert!(Ctx::Irq.is_irq() && Ctx::IrqExit.is_irq());
        assert!(!Ctx::Thread.is_irq() && !Ctx::ThreadOff.is_irq() && !Ctx::Unknown.is_irq());
    }

    // -- classify_pc ---------------------------------------------------------

    /// `VIRT_PHYS_OFFSET` (`kernel/src/arch/aarch64/mmu.rs`).
    const OFFSET: u64 = 0xFFFE_FFFF_C000_0000;
    /// `__text_start`/`__text_end` of the kernel ELF at `b07d7e4` (llvm-nm).
    const TEXT: TextLayout = TextLayout {
        lo: 0xFFFF_0000_0008_0000,
        hi: 0xFFFF_0000_000C_E000,
    };

    #[test]
    fn classify_pc_on_the_real_layout() {
        // PCZERO (ELR=0), and the text itself.
        assert_eq!(classify_pc(0, TEXT, OFFSET), PcClass::Null);
        assert_eq!(classify_pc(TEXT.lo, TEXT, OFFSET), PcClass::Text);
        assert_eq!(classify_pc(TEXT.hi - 4, TEXT, OFFSET), PcClass::Text);
        assert_eq!(classify_pc(TEXT.hi, TEXT, OFFSET), PcClass::Other);
        assert_eq!(classify_pc(TEXT.lo - 4, TEXT, OFFSET), PcClass::Other);
        // Valid kernel PCs from the ADR's run-167 exception lines.
        assert_eq!(
            classify_pc(0xFFFF_0000_000A_2830, TEXT, OFFSET),
            PcClass::Text
        );
        assert_eq!(
            classify_pc(0xFFFF_0000_000A_88E8, TEXT, OFFSET),
            PcClass::Text
        );
        // The ADR's direct-map PCs: in kernel VA, but data, and misaligned.
        assert_eq!(
            classify_pc(0xFFFF_0001_440B_5F16, TEXT, OFFSET),
            PcClass::Other
        );
        assert_eq!(
            classify_pc(0xFFFF_0001_440B_6845, TEXT, OFFSET),
            PcClass::Other
        );
        // The same direct-map region, aligned: still not text.
        assert_eq!(
            classify_pc(0xFFFF_0001_440B_5F10, TEXT, OFFSET),
            PcClass::Other
        );
        // Physical-alias PCs (N5): the text at its load address 0x4008_0000.
        assert_eq!(classify_pc(0x4008_0000, TEXT, OFFSET), PcClass::Phys);
        assert_eq!(classify_pc(0x400A_2830, TEXT, OFFSET), PcClass::Phys);
        assert_eq!(classify_pc(0x400C_DFFC, TEXT, OFFSET), PcClass::Phys);
        assert_eq!(classify_pc(0x400C_E000, TEXT, OFFSET), PcClass::Other);
        assert_eq!(classify_pc(0x400A_2832, TEXT, OFFSET), PcClass::Other);
        // Physical RAM outside the text alias, and a user address.
        assert_eq!(classify_pc(0x4400_0000, TEXT, OFFSET), PcClass::Other);
        assert_eq!(
            classify_pc(0x0000_0000_1000_0000, TEXT, OFFSET),
            PcClass::Other
        );
        assert_eq!(classify_pc(u64::MAX - 3, TEXT, OFFSET), PcClass::Other);

        assert_eq!(PcClass::Text.key(), None);
        assert_eq!(PcClass::Null.key(), Some(Key::Pcnull));
        assert_eq!(PcClass::Phys.key(), Some(Key::Pcphys));
        assert_eq!(PcClass::Other.key(), Some(Key::Pcother));
    }

    #[test]
    fn classify_pc_needs_virtual_bounds() {
        // Bounds computed from the symbols on CPUs 1-3's IRQ path come out
        // physical (`adrp` at a physical-alias PC). With them, a virtual
        // kernel PC classifies Other, and only a physical one looks like text:
        // this is why the kernel captures the bounds on CPU 0 at thread level.
        let physical = TextLayout {
            lo: TEXT.lo.wrapping_sub(OFFSET),
            hi: TEXT.hi.wrapping_sub(OFFSET),
        };
        assert_eq!(
            physical,
            TextLayout {
                lo: 0x4008_0000,
                hi: 0x400C_E000
            }
        );
        assert_eq!(
            classify_pc(0xFFFF_0000_000A_2830, physical, OFFSET),
            PcClass::Other
        );
        assert_eq!(classify_pc(0x400A_2830, physical, OFFSET), PcClass::Text);
        // Uncaptured bounds (0, 0) classify every non-zero PC as Other.
        let empty = TextLayout { lo: 0, hi: 0 };
        assert_eq!(
            classify_pc(0xFFFF_0000_000A_2830, empty, OFFSET),
            PcClass::Other
        );
        assert_eq!(classify_pc(0x400A_2830, empty, OFFSET), PcClass::Other);
    }

    // -- Masks ---------------------------------------------------------------

    #[test]
    fn mask_helpers_never_panic() {
        let mut mask = 0u64;
        let mut badtid = 0u64;
        let mut dup = 0u64;
        for tid in [0, 63, 64, 0x8000_0000, u32::MAX, 63, 5] {
            match mask_set(&mut mask, tid) {
                MaskSet::New => {}
                MaskSet::Dup => dup += 1,
                MaskSet::BadTid => badtid += 1,
            }
        }
        assert_eq!(mask, 1 | (1 << 5) | (1 << 63));
        assert_eq!(badtid, 3, "64, 0x8000_0000 and u32::MAX");
        assert_eq!(dup, 1);
        assert_eq!(mask_set(&mut mask, TID_NONE), MaskSet::BadTid);
        assert!(mask_test(mask, 63));
        assert!(!mask_test(mask, 62));
        assert!(!mask_test(u64::MAX, 64));
        assert!(!mask_test(u64::MAX, u32::MAX));
        assert_eq!(mask_bit(63), Some(1 << 63));
        assert_eq!(mask_bit(64), None);

        let mut rest = mask;
        let mut order = Vec::new();
        while let Some(t) = pop_lowest(&mut rest) {
            order.push(t);
        }
        assert_eq!(order, [0, 5, 63]);
        assert_eq!(rest, 0);
    }

    #[test]
    fn count_tids_matches_count_ones() {
        for i in 0..64u64 {
            assert_eq!(count_tids(1 << i), 1);
            assert_eq!(count_tids(u64::MAX >> i), 64 - i);
        }
        for m in sample_values() {
            assert_eq!(count_tids(m), u64::from(m.count_ones()), "{m:#x}");
        }
        assert_eq!(count_tids(0), 0);
    }

    // -- classify_slot -------------------------------------------------------

    #[test]
    fn slot_state_of_thread_states() {
        use ThreadState as S;
        let cases = [
            (None, SlotState::Empty),
            (Some(S::Runnable), SlotState::Runnable),
            (Some(S::Running), SlotState::Running),
            (Some(S::BlockedIpc { channel: 3 }), SlotState::BlockedIpc),
            (
                Some(S::BlockedIpc { channel: u64::MAX }),
                SlotState::BlockedIpc,
            ),
            (
                Some(S::BlockedNotification { notification: 1 }),
                SlotState::BlockedNotification,
            ),
            (Some(S::BlockedSelect), SlotState::BlockedSelect),
            (
                Some(S::BlockedProcessWait { child_pid: 2 }),
                SlotState::BlockedOther,
            ),
            (
                Some(S::BlockedTimer { wake_at: 9 }),
                SlotState::BlockedOther,
            ),
            (Some(S::BlockedIo), SlotState::BlockedOther),
            (Some(S::Suspended), SlotState::Suspended),
            (Some(S::Dead), SlotState::Dead),
        ];
        for (state, want) in cases {
            assert_eq!(SlotState::of(state.as_ref()), want, "{state:?}");
        }
    }

    #[test]
    fn classify_slot_table() {
        use SlotFlags as F;
        use SlotState as S;
        use SlotVerdict as V;
        let q = F::QUEUED;
        let cur = F::CURRENT;
        let wp = F::WAKE_PENDING;
        let fresh = (5000, 4500);
        let old = (5000, 3999); // 1001 ticks
        let edge = (5000, 4000); // exactly STARVE_TICKS: not starved
        let future = (100, 5000); // last_run > now
        let rows: Vec<(S, F, (u64, u64), V)> = vec![
            (S::Empty, F::NONE, fresh, V::Clear),
            (S::Empty, q, fresh, V::QueuedBad),
            (S::Empty, cur, fresh, V::Clear),
            (S::Runnable, F::NONE, fresh, V::Orphan),
            (S::Runnable, F::NONE, old, V::Orphan), // orphan before starved
            (S::Runnable, cur, old, V::Clear),
            (S::Runnable, q, fresh, V::Clear),
            (S::Runnable, q, edge, V::Clear),
            (S::Runnable, q, old, V::Starved),
            (S::Runnable, q | cur, old, V::Starved),
            (S::Runnable, q, future, V::Clear),
            (S::Runnable, q | wp, fresh, V::Clear),
            (S::Running, F::NONE, old, V::Clear),
            (S::Running, cur, old, V::Clear),
            (S::Running, q, fresh, V::QueuedBad),
            (S::BlockedIpc, F::NONE, fresh, V::NoWaker),
            (S::BlockedIpc, F::TIMEOUT, fresh, V::Clear),
            (S::BlockedIpc, F::CHAN_REF, fresh, V::Clear),
            (S::BlockedIpc, F::NDL, fresh, V::NoWaker),
            (S::BlockedIpc, F::NOTIF_REF, fresh, V::NoWaker),
            (S::BlockedIpc, wp, fresh, V::WakeInFlight),
            (S::BlockedIpc, F::TIMEOUT | wp, fresh, V::Clear),
            (S::BlockedIpc, cur, fresh, V::Clear),
            (S::BlockedIpc, q, fresh, V::QueuedBad),
            (S::BlockedIpc, q | F::TIMEOUT, fresh, V::QueuedBad),
            (S::BlockedNotification, F::NONE, fresh, V::NoWaker),
            (S::BlockedNotification, F::NDL, fresh, V::Clear),
            (S::BlockedNotification, F::NOTIF_REF, fresh, V::Clear),
            (S::BlockedNotification, F::TIMEOUT, fresh, V::Clear),
            (S::BlockedNotification, F::CHAN_REF, fresh, V::NoWaker),
            (S::BlockedNotification, wp, fresh, V::WakeInFlight),
            (S::BlockedNotification, cur, fresh, V::Clear),
            (S::BlockedSelect, F::NONE, fresh, V::NoWaker),
            (S::BlockedSelect, F::NDL, fresh, V::Clear),
            (S::BlockedSelect, F::CHAN_REF, fresh, V::Clear),
            (S::BlockedSelect, F::NOTIF_REF, fresh, V::Clear),
            (S::BlockedSelect, F::TIMEOUT, fresh, V::Clear),
            (S::BlockedSelect, wp, fresh, V::WakeInFlight),
            (S::BlockedSelect, cur, fresh, V::Clear),
            (S::BlockedOther, F::NONE, old, V::Clear),
            (S::BlockedOther, q, fresh, V::QueuedBad),
            (S::Suspended, F::NONE, old, V::Clear),
            (S::Dead, F::NONE, old, V::Clear),
            (S::Dead, cur, old, V::Clear),
            (S::Dead, q, old, V::QueuedBad),
        ];
        for (state, flags, (now, last_run), want) in rows {
            assert_eq!(
                classify_slot(state, flags, now, last_run),
                want,
                "{state:?} {flags:?} now={now} last_run={last_run}"
            );
        }

        // Sleep: BlockedIpc { channel: u64::MAX } is woken by its timeout.
        let sleep = SlotState::of(Some(&ThreadState::BlockedIpc { channel: u64::MAX }));
        assert_eq!(classify_slot(sleep, F::TIMEOUT, 0, 0), V::Clear);
        assert_eq!(classify_slot(sleep, F::NONE, 0, 0), V::NoWaker);

        assert!(!is_starved(5000, 4000));
        assert!(is_starved(5000, 3999));
        assert!(!is_starved(0, u64::MAX));
        assert!(is_starved(u64::MAX, 0));

        let flags = F::NONE.with_if(F::QUEUED, true).with_if(F::CURRENT, false);
        assert_eq!(flags, F::QUEUED);
        assert!((F::TIMEOUT | F::NDL).intersects(F::NDL));
        assert!(!(F::TIMEOUT | F::NDL).contains(F::NDL | F::CURRENT));
    }

    // -- TwoStrike and EdgeCounter ------------------------------------------

    /// The scan inputs: `LAST_RUN` per thread and `tick` per CPU.
    struct Machine {
        last_run: [u64; 64],
        ticks: [u64; 4],
    }

    impl Machine {
        fn new() -> Self {
            Self {
                last_run: [0; 64],
                ticks: [0; 4],
            }
        }

        /// Every CPU takes `n` ticks.
        fn run(&mut self, n: u64) {
            for t in &mut self.ticks {
                *t += n;
            }
        }
    }

    fn scan(ts: &TwoStrike<4>, m: &Machine, flagged: u64) -> StrikeResult {
        ts.scan(flagged, 4, |t| m.last_run[t as usize], |c| m.ticks[c])
    }

    fn ok(confirmed: u64) -> StrikeResult {
        StrikeResult {
            confirmed,
            stalled: false,
        }
    }

    const T1: u64 = 1 << 1;
    const T2: u64 = 1 << 2;
    const T5: u64 = 1 << 5;
    const T63: u64 = 1 << 63;

    #[test]
    fn two_strike_single_two_three() {
        let ts = TwoStrike::<4>::new();
        let mut m = Machine::new();
        m.last_run[5] = 42;
        m.run(1000);
        assert_eq!(scan(&ts, &m, T5), ok(0), "one strike");
        assert_eq!(ts.armed(), T5);
        m.run(1000);
        assert_eq!(scan(&ts, &m, T5), ok(T5), "two in a row");
        m.run(1000);
        assert_eq!(scan(&ts, &m, T5), ok(T5), "three: still confirmed");
    }

    #[test]
    fn two_strike_flag_clear_flag() {
        let ts = TwoStrike::<4>::new();
        let mut m = Machine::new();
        assert_eq!(scan(&ts, &m, T5), ok(0));
        m.run(1000);
        assert_eq!(scan(&ts, &m, 0), ok(0));
        m.run(1000);
        assert_eq!(
            scan(&ts, &m, T5),
            ok(0),
            "the clear dropped the first strike"
        );
        m.run(1000);
        assert_eq!(scan(&ts, &m, T5), ok(T5));
    }

    #[test]
    fn two_strike_bits_are_independent() {
        let ts = TwoStrike::<4>::new();
        let mut m = Machine::new();
        assert_eq!(scan(&ts, &m, T1 | T63), ok(0));
        m.run(1000);
        assert_eq!(scan(&ts, &m, T1 | T2), ok(T1));
        m.run(1000);
        assert_eq!(scan(&ts, &m, T2 | T63), ok(T2));
        m.run(1000);
        assert_eq!(scan(&ts, &m, T1 | T2 | T63), ok(T2 | T63));
    }

    #[test]
    fn two_strike_last_run_change_restarts() {
        let ts = TwoStrike::<4>::new();
        let mut m = Machine::new();
        m.last_run[5] = 10;
        assert_eq!(scan(&ts, &m, T5), ok(0));
        m.run(1000);
        m.last_run[5] = 900; // dispatched in between
        assert_eq!(scan(&ts, &m, T5), ok(0), "it ran: not stuck");
        m.run(1000);
        assert_eq!(
            scan(&ts, &m, T5),
            ok(T5),
            "a new first strike, now confirmed"
        );
    }

    #[test]
    fn two_strike_kinds_keep_separate_history() {
        // flag(B) -> dispatch -> A-only scan (phase 2 skipped) -> flag(B):
        // B must compare with its own first strike, not A's newer snapshot.
        let a = TwoStrike::<4>::new();
        let b = TwoStrike::<4>::new();
        let mut m = Machine::new();
        m.last_run[5] = 10;
        assert_eq!(scan(&b, &m, T5), ok(0));
        m.run(1000);
        m.last_run[5] = 20; // dispatched
        assert_eq!(scan(&a, &m, T5), ok(0), "scan A's first strike");
        m.run(1000);
        assert_eq!(
            scan(&b, &m, T5),
            ok(0),
            "B saw LAST_RUN change since its strike"
        );
        m.run(1000);
        assert_eq!(scan(&a, &m, T5), ok(T5), "A's own two strikes confirm");
        assert_eq!(
            scan(&b, &m, T5),
            ok(T5),
            "B's second strike since the dispatch"
        );
    }

    #[test]
    fn two_strike_tick_gate_withholds_then_confirms() {
        let ts = TwoStrike::<4>::new();
        let mut m = Machine::new();
        m.run(1000);
        assert_eq!(scan(&ts, &m, T5), ok(0));
        // CPU 3 stalls: 99 ticks while the others take 1000.
        m.ticks[0] += 1000;
        m.ticks[1] += 1000;
        m.ticks[2] += 1000;
        m.ticks[3] += 99;
        let r = scan(&ts, &m, T5 | T1);
        assert_eq!(
            r,
            StrikeResult {
                confirmed: 0,
                stalled: true
            }
        );
        assert_eq!(
            ts.armed(),
            T5,
            "the first strike is kept; no new strike while stalled"
        );
        // One more tick on CPU 3 completes 100 since the first strike.
        m.ticks[3] += 1;
        assert_eq!(scan(&ts, &m, T5 | T1), ok(T5));
        // Offline CPUs do not gate: with ncpu = 3, CPU 3 may stay still.
        m.ticks[0] += 100;
        m.ticks[1] += 100;
        m.ticks[2] += 100;
        let r = ts.scan(T5 | T1, 3, |t| m.last_run[t as usize], |c| m.ticks[c]);
        assert_eq!(r, ok(T5 | T1));
    }

    #[test]
    fn two_strike_gate_only_applies_to_pending_confirmations() {
        let ts = TwoStrike::<4>::new();
        let mut m = Machine::new();
        m.run(1000);
        assert_eq!(scan(&ts, &m, T5), ok(0));
        assert_eq!(scan(&ts, &m, T2), ok(0), "no candidate, so no stall");
        assert_eq!(ts.armed(), T2);
        assert!(scan(&ts, &m, T2).stalled);
    }

    #[test]
    fn two_strike_gate_skips_cpus_that_never_ticked() {
        // CPUs 1-3 take no timer IRQs (#200): only CPU 0's tick advances.
        let ts = TwoStrike::<4>::new();
        let mut m = Machine::new();
        m.ticks[0] = 1000;
        assert_eq!(scan(&ts, &m, T5), ok(0));
        m.ticks[0] += 1000;
        assert_eq!(scan(&ts, &m, T5), ok(T5), "CPUs at 0 do not gate");
        // A CPU that starts ticking gates from then on: CPU 2 at 5 has not
        // advanced 100 since the last scan's snapshot (0).
        m.ticks[0] += 1000;
        m.ticks[2] = 5;
        assert!(scan(&ts, &m, T5).stalled, "CPU 2 ticked, but only 5");
        m.ticks[0] += 1000;
        m.ticks[2] += 95;
        assert_eq!(scan(&ts, &m, T5), ok(T5));
        // A CPU that ticked once and then stopped keeps gating.
        m.ticks[0] += 1000;
        assert!(scan(&ts, &m, T5).stalled, "CPU 2 stopped at 100");
        // With no CPU ticking at all, nothing gates.
        let fresh = TwoStrike::<4>::new();
        let still = Machine::new();
        assert_eq!(scan(&fresh, &still, T1), ok(0));
        assert_eq!(scan(&fresh, &still, T1), ok(T1));
    }

    #[test]
    fn edge_counter_counts_entries() {
        let e = EdgeCounter::new();
        let u = e.update(T1);
        assert_eq!((u.entered_count(), u.gauge()), (1, 1));
        let u = e.update(T1);
        assert_eq!(
            (u.entered_count(), u.gauge()),
            (0, 1),
            "a persistent flag counts once"
        );
        let u = e.update(0);
        assert_eq!((u.entered_count(), u.gauge()), (0, 0));
        let u = e.update(T1);
        assert_eq!(
            (u.entered_count(), u.gauge()),
            (1, 1),
            "drop then re-flag counts again"
        );
        let u = e.update(T1 | T2 | T63);
        assert_eq!(u.entered, T2 | T63);
        assert_eq!((u.entered_count(), u.gauge()), (2, 3));
        let u = e.update(T2);
        assert_eq!(
            (u.entered_count(), u.gauge()),
            (0, 1),
            "the gauge follows the popcount"
        );
        assert_eq!(e.confirmed(), T2);
    }

    // -- Wake attribution ----------------------------------------------------

    #[test]
    fn unblock_kind_of_thread_states() {
        use ThreadState as S;
        use UnblockKind as K;
        let cases = [
            (None, K::NoThread),
            (Some(S::Running), K::SkipRunning),
            (Some(S::Runnable), K::SkipRunnable),
            (Some(S::BlockedIpc { channel: 1 }), K::Woke),
            (Some(S::BlockedNotification { notification: 1 }), K::Woke),
            (Some(S::BlockedSelect), K::Woke),
            (Some(S::BlockedProcessWait { child_pid: 1 }), K::Woke),
            (Some(S::BlockedTimer { wake_at: 1 }), K::Revived),
            (Some(S::BlockedIo), K::Revived),
            (Some(S::Suspended), K::Revived),
            (Some(S::Dead), K::Revived),
        ];
        for (state, want) in cases {
            assert_eq!(UnblockKind::of(state.as_ref()), want, "{state:?}");
        }
        let keys: Vec<Option<Key>> = K::ALL.iter().map(|k| k.key()).collect();
        assert_eq!(
            keys,
            [
                None,
                Some(Key::Ubdead),
                Some(Key::Ubrun),
                Some(Key::Ubrbl),
                Some(Key::Ubnone)
            ]
        );
        for k in K::ALL {
            assert_eq!(k.skipped(), matches!(k, K::SkipRunning | K::SkipRunnable));
            assert_eq!(k.made_runnable(), matches!(k, K::Woke | K::Revived));
        }
    }

    const CHAN: u64 = 7;

    const CLEARS: [ClearResult; 3] = [ClearResult::Removed, ClearResult::Absent, ClearResult::Busy];

    fn outcome(kind: UnblockKind, phase: u8, same_chan: bool) -> UnblockOutcome {
        UnblockOutcome {
            kind,
            phase,
            chan: if same_chan { CHAN } else { CHAN + 1 },
        }
    }

    /// The phase byte for `step` (0 idle, 1 published, 2 armed, 3 invalid),
    /// flagged untimed with the literal bit so the tables do not depend on
    /// [`wait_phase`].
    fn phase_byte(step: usize, timed: bool) -> u8 {
        let step = u8::try_from(step).unwrap();
        if timed {
            step
        } else {
            step | 0x80
        }
    }

    /// Run `classify` over every (kind, channel match, timed, clear, step)
    /// cell and compare it with `want(kind, same_chan, timed, clear)`, whose
    /// columns are steps 0 to 3. Returns the number of cells checked.
    fn check_every_cell(
        classify: fn(UnblockOutcome, u64, ClearResult) -> WakeVerdict,
        want: impl Fn(UnblockKind, bool, bool, ClearResult) -> [WakeVerdict; 4],
    ) -> usize {
        let mut cells = 0;
        for kind in UnblockKind::ALL {
            for same in [true, false] {
                for timed in [true, false] {
                    for clear in CLEARS {
                        let row = want(kind, same, timed, clear);
                        for (step, expect) in row.iter().enumerate() {
                            let o = outcome(kind, phase_byte(step, timed), same);
                            assert_eq!(
                                classify(o, CHAN, clear),
                                *expect,
                                "{kind:?} same_chan={same} timed={timed} {clear:?} step={step}"
                            );
                            cells += 1;
                        }
                    }
                }
            }
        }
        cells
    }

    #[test]
    fn wait_phase_encoding() {
        assert_eq!(PHASE_IDLE, 0);
        assert_eq!((PHASE_PUBLISHED, PHASE_ARMED), (1, 2));
        assert_eq!(wait_phase(PHASE_PUBLISHED, true), 1);
        assert_eq!(wait_phase(PHASE_ARMED, true), 2);
        assert_eq!(wait_phase(PHASE_PUBLISHED, false), 0x81);
        assert_eq!(wait_phase(PHASE_ARMED, false), 0x82);
        for step in [PHASE_IDLE, PHASE_PUBLISHED, PHASE_ARMED] {
            assert_eq!(step & PHASE_UNTIMED, 0, "the flag is outside every step");
        }
    }

    #[test]
    fn classify_reply_every_cell() {
        use ClearResult as C;
        use UnblockKind as K;
        use WakeVerdict as W;
        const RPRE: W = W::N2(N2Kind::Rpre);
        const RBLK: W = W::N2(N2Kind::Rblk);
        const LATE: W = W::LateReply;
        const MIS: W = W::Misrep;
        const NC: W = W::NotCounted;
        // A skipped caller in this call, per (timed, clear). Columns: step 0
        // (idle), 1 (published), 2 (armed), 3 (invalid).
        let skipped_in_call: [(bool, C, [W; 4]); 6] = [
            (true, C::Removed, [LATE, RPRE, RBLK, LATE]),
            (true, C::Absent, [LATE, RPRE, LATE, LATE]),
            (true, C::Busy, [LATE, RPRE, LATE, LATE]),
            (false, C::Removed, [LATE, RBLK, RBLK, LATE]),
            (false, C::Absent, [LATE, RBLK, RBLK, LATE]),
            (false, C::Busy, [LATE, RBLK, RBLK, LATE]),
        ];
        let cells = check_every_cell(classify_reply, |kind, same, timed, clear| {
            match (kind, same) {
                (K::SkipRunning | K::SkipRunnable, true) => {
                    let row = skipped_in_call
                        .iter()
                        .find(|r| r.0 == timed && r.1 == clear);
                    row.unwrap().2
                }
                (K::SkipRunning | K::SkipRunnable, false) => [LATE; 4],
                (K::Woke | K::Revived, true) => [MIS, MIS, NC, MIS],
                (K::Woke | K::Revived, false) => [MIS; 4],
                (K::NoThread, _) => [NC; 4],
            }
        });
        assert_eq!(cells, 5 * 2 * 2 * 3 * 4);
    }

    #[test]
    fn classify_send_every_cell() {
        use ClearResult as C;
        use WakeVerdict as W;
        const VPRE: W = W::N2(N2Kind::Vpre);
        const VBLK: W = W::N2(N2Kind::Vblk);
        const NC: W = W::NotCounted;
        // A skipped receiver in a receive on this channel, per (timed,
        // clear). Columns as in the reply table.
        let skipped_in_recv: [(bool, C, [W; 4]); 6] = [
            (true, C::Removed, [NC, VPRE, VBLK, NC]),
            (true, C::Absent, [NC, VPRE, NC, NC]),
            (true, C::Busy, [NC, VPRE, NC, NC]),
            (false, C::Removed, [NC, VBLK, VBLK, NC]),
            (false, C::Absent, [NC, VBLK, VBLK, NC]),
            (false, C::Busy, [NC, VBLK, VBLK, NC]),
        ];
        let cells = check_every_cell(classify_send, |kind, same, timed, clear| {
            if kind.skipped() && same {
                let row = skipped_in_recv
                    .iter()
                    .find(|r| r.0 == timed && r.1 == clear);
                row.unwrap().2
            } else {
                [NC; 4]
            }
        });
        assert_eq!(cells, 5 * 2 * 2 * 3 * 4);
    }

    /// The kernel interleavings the N2 table has to separate (S3 review).
    #[test]
    fn n2_interleavings() {
        use ClearResult as C;
        use UnblockKind as K;
        use WakeVerdict as W;
        let armed = wait_phase(PHASE_ARMED, true);
        let published = wait_phase(PHASE_PUBLISHED, true);
        let reply = |kind, phase, clear| classify_reply(outcome(kind, phase, true), CHAN, clear);
        let send = |kind, phase, clear| classify_send(outcome(kind, phase, true), CHAN, clear);

        // N2: the reply removes the registered timeout of a caller that has
        // not blocked yet (Running), or was preempted before blocking
        // (Runnable). The caller then blocks with no waker.
        assert_eq!(
            reply(K::SkipRunning, armed, C::Removed),
            W::N2(N2Kind::Rblk)
        );
        assert_eq!(
            reply(K::SkipRunnable, armed, C::Removed),
            W::N2(N2Kind::Rblk)
        );
        // Late reply: the caller's timeout fired and woke it. `ipc_call`
        // clears its phase only once it runs again, and `pending_caller`
        // later still, so the reply still sees phase armed, finds no entry
        // to clear, and skips a caller that returns ETIMEDOUT.
        assert_eq!(reply(K::SkipRunnable, armed, C::Absent), W::LateReply);
        // Early reply seen armed: the reply's clear ran before the caller
        // registered its timeout, and the caller armed before the reply's
        // `unblock`. The registered timeout heals it.
        assert_eq!(reply(K::SkipRunning, armed, C::Absent), W::LateReply);
        // The same early reply seen before the caller armed.
        assert_eq!(
            reply(K::SkipRunning, published, C::Absent),
            W::N2(N2Kind::Rpre)
        );
        // A busy clear left the entry in place, so the timeout still fires.
        assert_eq!(reply(K::SkipRunning, armed, C::Busy), W::LateReply);
        // An untimed call has no timeout to heal it, at either step.
        for step in [PHASE_PUBLISHED, PHASE_ARMED] {
            let phase = wait_phase(step, false);
            assert_eq!(reply(K::SkipRunning, phase, C::Absent), W::N2(N2Kind::Rblk));
            assert_eq!(send(K::SkipRunning, phase, C::Absent), W::N2(N2Kind::Vblk));
        }
        // The normal reply wake of an untimed caller is not misdirected.
        let untimed_armed = wait_phase(PHASE_ARMED, false);
        assert_eq!(reply(K::Woke, untimed_armed, C::Absent), W::NotCounted);

        // Receive side: the send removes a receiver's registered timeout
        // before it blocks.
        assert_eq!(send(K::SkipRunning, armed, C::Removed), W::N2(N2Kind::Vblk));
        // The receiver's timeout already woke it (its phase stays armed
        // until it runs), or the send cleared before the receiver
        // registered: not counted.
        assert_eq!(send(K::SkipRunnable, armed, C::Absent), W::NotCounted);
        assert_eq!(send(K::SkipRunning, armed, C::Absent), W::NotCounted);
        assert_eq!(
            send(K::SkipRunning, published, C::Absent),
            W::N2(N2Kind::Vpre)
        );
    }

    #[test]
    fn wake_verdict_keys() {
        assert_eq!(WakeVerdict::NotCounted.key(), None);
        for kind in N2Kind::ALL {
            assert_eq!(WakeVerdict::N2(kind).key(), Some((Key::N2, kind.index())));
            assert!(Key::N2.slot(kind.index()).is_some());
        }
        assert_eq!(WakeVerdict::LateReply.key(), Some((Key::Latereply, 0)));
        assert_eq!(WakeVerdict::Misrep.key(), Some((Key::Misrep, 0)));
    }
}
