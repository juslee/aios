//! Owner-stamped spinlock core for the kernel's detect-only IRQ-class lock.
//!
//! Crash-fix step 1b wraps the 9 IRQ-shared statics in a detect-only lock
//! (`kernel/src/sync/irq_spin_lock.rs`). This module holds the part of it that
//! is pure protocol, so that host threads and `just miri` check it:
//!
//! - [`OwnerStamp`]: the lock word is the owner stamp. A separate owner field
//!   beside a spin flag cannot be kept consistent: clearing it before the
//!   unlock lets a tick see "held, no owner", and clearing it after the unlock
//!   can leave a stale owner behind another CPU's acquisition.
//! - [`classify`]: what a contended acquirer learns from the stamp.
//! - [`read_stamp`]: a `(cpu, switch generation)` pair taken without masking IRQs.
//! - [`StampedLock`] / [`StampedGuard`]: the lock itself, with a [`PreRelease`]
//!   hook so the kernel clears its diagnostic holder fields before the unlock.
//! - [`LockClass`], [`SCALAR_INDEX`] and [`TID_NONE`]: names shared with the
//!   tripwire writer.
//!
//! # Lock word
//!
//! ```text
//! bit 63 HELD | bit 62 IRQS_ON | bits 54..=61 cpu | bits 0..=53 switch generation
//! ```
//!
//! The word is 0 when free and a stamp (HELD set, so never 0) when held.
//! Acquisition is one CAS from 0 to the stamp with `Acquire`, and release is
//! `store(0, Release)`. This is the read-modify-write and the orderings of
//! `spin` 0.12.3's `SpinMutex`, including its test-and-test-and-set spin. The
//! lock has no fairness. The holder may replace its own stamp while it holds
//! the lock ([`StampedGuard::restamp`]); nobody else writes a held word.
//!
//! # What the stamp proves
//!
//! The kernel keeps a per-CPU switch generation, `SWITCH_GEN[c]`. [`classify`]
//! reports [`Contention::Reentry`] only when the owner CPU is the caller's CPU
//! and the owner generation equals `SWITCH_GEN[caller cpu]`. That verdict has
//! no false positives provided the kernel keeps two rules:
//!
//! - (a) every `restore_context` on CPU c is preceded, on c with IRQs masked,
//!   by a bump of `SWITCH_GEN[c]` (c read from MPIDR);
//! - (b) the dispatching stream releases every guard it took after that bump
//!   before it calls `restore_context`.
//!
//! Under (a) a thread that leaves CPU c, for any reason, leaves `SWITCH_GEN[c]`
//! changed behind it, and so does a thread that comes back to c. A stamp that
//! still matches therefore names the stream running on c now. A stale stamp
//! (taken before a switch the stamping code did not see) only ever errs
//! towards [`Contention::PreemptedHolder`] or [`Contention::OtherCpuSwitched`],
//! which are counted and never panic.
//!
//! # Holder fields (the kernel's [`PreRelease`] hook)
//!
//! The kernel keeps diagnostic fields (holder thread id and call site) in the
//! hook value `H`. They follow this protocol, which [`StampedLock::consistent_snapshot`]
//! relies on:
//!
//! - store them with `Release` immediately after a successful CAS;
//! - clear them in [`PreRelease::pre_release`], which the guard runs before the
//!   releasing `store(0, Release)`;
//! - read them only inside [`StampedLock::consistent_snapshot`].
//!
//! A reader then never sees a previous holder's fields beside the current
//! stamp, and a snapshot the lock changed under is rejected.

#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::num::NonZeroU64;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{fence, AtomicU64, Ordering};

/// "No thread" in thread-id fields.
///
/// Thread slot 0 is a real thread, so 0 cannot mean "none".
pub const TID_NONE: u32 = u32::MAX;

/// Lock index for a lock that is not an element of a per-CPU array.
///
/// `CURRENT_THREAD` and `RUN_QUEUES` are `[_; MAX_CORES]` arrays whose locks
/// carry their CPU index; the other seven lock classes use this value.
pub const SCALAR_INDEX: u8 = 0xFF;

// ---------------------------------------------------------------------------
// OwnerStamp
// ---------------------------------------------------------------------------

/// The owner stamp stored in a held lock word.
///
/// Never 0: the HELD bit is always set, so `Option<OwnerStamp>` is one word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct OwnerStamp(NonZeroU64);

impl OwnerStamp {
    /// Set in every stamp, so that a held word is never 0.
    pub const HELD: u64 = 1 << 63;
    /// DAIF.I was clear (IRQs enabled) when the stamp was taken.
    pub const IRQS_ON: u64 = 1 << 62;
    /// Position of the 8-bit owner CPU field.
    pub const CPU_SHIFT: u32 = 54;
    /// The owner CPU field, in place.
    pub const CPU_MASK: u64 = 0xFF << Self::CPU_SHIFT;
    /// The switch generation field (the low 54 bits).
    pub const GEN_MASK: u64 = (1 << Self::CPU_SHIFT) - 1;

    /// [`Self::HELD`] as a non-zero value.
    const HELD_NZ: NonZeroU64 = NonZeroU64::new(Self::HELD).unwrap();

    /// Build a stamp. The generation is truncated to its low 54 bits.
    #[inline]
    pub fn new(cpu: u8, gen: u64, irqs_on: bool) -> Self {
        let irqs = if irqs_on { Self::IRQS_ON } else { 0 };
        Self(Self::HELD_NZ | irqs | (u64::from(cpu) << Self::CPU_SHIFT) | (gen & Self::GEN_MASK))
    }

    /// Decode a lock word. `None` for a word without HELD (a free lock is 0).
    #[inline]
    pub const fn from_word(word: u64) -> Option<Self> {
        if word & Self::HELD == 0 {
            return None;
        }
        match NonZeroU64::new(word) {
            Some(w) => Some(Self(w)),
            None => None,
        }
    }

    /// The raw lock word.
    #[inline]
    pub const fn word(self) -> u64 {
        self.0.get()
    }

    /// The owner CPU.
    #[inline]
    pub const fn cpu(self) -> u8 {
        ((self.0.get() & Self::CPU_MASK) >> Self::CPU_SHIFT) as u8
    }

    /// The owner CPU's switch generation at stamping, truncated to 54 bits.
    #[inline]
    pub const fn gen(self) -> u64 {
        self.0.get() & Self::GEN_MASK
    }

    /// Whether IRQs were enabled when the stamp was taken.
    #[inline]
    pub const fn irqs_on(self) -> bool {
        self.0.get() & Self::IRQS_ON != 0
    }

    /// Whether a full switch generation equals this stamp's truncated one.
    #[inline]
    pub const fn gen_matches(self, gen: u64) -> bool {
        self.gen() == gen & Self::GEN_MASK
    }
}

// ---------------------------------------------------------------------------
// Contention classification
// ---------------------------------------------------------------------------

/// The CPU and switch-generation state a stamp is taken and judged against.
///
/// The kernel reads MPIDR_EL1 Aff0 and its per-CPU `SWITCH_GEN` array; tests
/// supply scripted models. Neither method may panic, for any argument.
pub trait CpuView {
    /// The CPU the caller runs on now.
    fn cpu(&self) -> u8;
    /// `SWITCH_GEN[cpu]`. For a CPU with no generation, return any value.
    fn switch_gen(&self, cpu: u8) -> u64;
}

/// What a contended acquirer learns from the lock word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Contention {
    /// The word is 0: nobody holds the lock.
    Free,
    /// Another CPU's stamp, and that CPU has not dispatched since: the holder
    /// is most likely running there.
    OtherCpu,
    /// Another CPU's stamp, and that CPU has dispatched since: the holder was
    /// switched out and may be running anywhere, including on this CPU.
    OtherCpuSwitched,
    /// This CPU's stamp and generation: the holder is the stream this CPU is
    /// running now, so waiting can never succeed.
    Reentry,
    /// This CPU's stamp from an earlier generation: the holder was switched
    /// out on this CPU. It may have been resumed since, here or elsewhere.
    PreemptedHolder,
}

/// Classify an observed lock word for a caller on `cpu`.
///
/// IRQS_ON is ignored. Only [`Contention::Reentry`] is proof of a same-stream
/// deadlock (see the module documentation for the kernel rules it rests on).
/// The Reentry test reads only the word and `v.switch_gen(cpu)`; the other
/// CPU's generation read in the other-CPU arms only splits a count.
#[inline]
pub fn classify(observed: u64, cpu: u8, v: &impl CpuView) -> Contention {
    let Some(owner) = OwnerStamp::from_word(observed) else {
        return Contention::Free;
    };
    let owner_cpu = owner.cpu();
    let unswitched = owner.gen_matches(v.switch_gen(owner_cpu));
    match (owner_cpu == cpu, unswitched) {
        (true, true) => Contention::Reentry,
        (true, false) => Contention::PreemptedHolder,
        (false, true) => Contention::OtherCpu,
        (false, false) => Contention::OtherCpuSwitched,
    }
}

/// Read `(cpu, SWITCH_GEN[cpu])` without masking IRQs.
///
/// Reads the CPU, that CPU's generation, then the CPU again, and retries until
/// both CPU reads agree. If the thread was switched between the generation
/// read and the second CPU read, it came back to the same CPU through a
/// dispatch that bumped the generation, so the pair is stale and can only
/// classify [`Contention::PreemptedHolder`] or [`Contention::OtherCpuSwitched`],
/// never [`Contention::Reentry`].
#[inline]
pub fn read_stamp(v: &impl CpuView) -> (u8, u64) {
    loop {
        let cpu = v.cpu();
        let gen = v.switch_gen(cpu);
        if v.cpu() == cpu {
            return (cpu, gen);
        }
    }
}

// ---------------------------------------------------------------------------
// StampedLock
// ---------------------------------------------------------------------------

/// Work the guard does while the lock is still held, just before it releases.
pub trait PreRelease {
    /// Called by [`StampedGuard`]'s `Drop` immediately before the releasing
    /// `store(0, Release)`. Must not touch the lock word.
    fn pre_release(&self);
}

impl PreRelease for () {
    #[inline(always)]
    fn pre_release(&self) {}
}

/// A spinlock whose word is the holder's [`OwnerStamp`].
///
/// `H` is the kernel's hook value (its holder fields); see [`PreRelease`] and
/// the module documentation. The lock has no `Drop`; only the guard does.
pub struct StampedLock<T, H = ()> {
    word: AtomicU64,
    hook: H,
    data: UnsafeCell<T>,
}

// SAFETY: `data` is reached only through a StampedGuard, and a guard exists
// only after the CAS that took the word from 0, so at most one guard (one
// thread) accesses `data` at a time, which makes sharing sound for T: Send
// (the same bound as spin::Mutex). `hook` is shared by reference, hence H: Sync.
// The CAS in try_lock/try_lock_weak and the store(0) in the guard's Drop
// maintain the invariant; a second path that wrote 0 to a held word would let
// two guards alias `data` (undefined behaviour).
unsafe impl<T: Send, H: Sync> Sync for StampedLock<T, H> {}

impl<T> StampedLock<T> {
    /// A free lock with no hook.
    pub const fn new(data: T) -> Self {
        Self::with_hook(data, ())
    }
}

impl<T, H> StampedLock<T, H> {
    /// A free lock with hook value `hook`.
    pub const fn with_hook(data: T, hook: H) -> Self {
        Self {
            word: AtomicU64::new(0),
            hook,
            data: UnsafeCell::new(data),
        }
    }

    /// The hook value (the kernel's holder fields).
    #[inline]
    pub fn hook(&self) -> &H {
        &self.hook
    }

    /// The lock word (`Relaxed`): 0 when free, otherwise the owner stamp.
    ///
    /// For the spin test, classification and scans. It orders nothing.
    #[inline]
    pub fn owner_word(&self) -> u64 {
        self.word.load(Ordering::Relaxed)
    }

    /// Read the holder fields consistently with the stamp.
    ///
    /// Loads the word (`Acquire`), runs `read` with the decoded stamp and the
    /// hook, then loads the word again after an acquire fence. Returns `None`
    /// if the lock was free at the first load or the two loads differ.
    ///
    /// With the holder-field protocol from the module documentation, `Some`
    /// means `read` saw either the fields of the holder that owns the stamp
    /// or cleared fields (the window between the CAS and the field stores, or
    /// between the clears and the release). It never sees an earlier or a
    /// later holder's fields, unless the very same stamp was released and
    /// taken again in between (same CPU, no dispatch). `read` must only load
    /// atomics in `H`.
    #[inline]
    pub fn consistent_snapshot<R>(&self, read: impl FnOnce(OwnerStamp, &H) -> R) -> Option<R> {
        let first = self.word.load(Ordering::Acquire);
        let stamp = OwnerStamp::from_word(first)?;
        let fields = read(stamp, &self.hook);
        // Keep the second word load after the field loads in `read`. A field
        // value from a later holder was stored with Release after that
        // holder's CAS, so this fence makes the CAS visible to the reload.
        fence(Ordering::Acquire);
        if self.word.load(Ordering::Relaxed) == first {
            Some(fields)
        } else {
            None
        }
    }
}

impl<T, H: PreRelease> StampedLock<T, H> {
    /// One strong CAS from 0 to `stamp`.
    ///
    /// On failure returns the word observed by the CAS, which is never 0.
    #[inline]
    pub fn try_lock(&self, stamp: OwnerStamp) -> Result<StampedGuard<'_, T, H>, u64> {
        self.word
            .compare_exchange(0, stamp.word(), Ordering::Acquire, Ordering::Relaxed)
            .map(|_| StampedGuard::new(self))
    }

    /// One weak CAS from 0 to `stamp`, for use in a retry loop.
    ///
    /// On failure returns the word observed by the CAS. `Err(0)` is a spurious
    /// failure of a free lock; retry.
    #[inline]
    pub fn try_lock_weak(&self, stamp: OwnerStamp) -> Result<StampedGuard<'_, T, H>, u64> {
        self.word
            .compare_exchange_weak(0, stamp.word(), Ordering::Acquire, Ordering::Relaxed)
            .map(|_| StampedGuard::new(self))
    }
}

/// Exclusive access to a [`StampedLock`]'s data. One pointer wide.
///
/// Dropping it runs the hook's [`PreRelease::pre_release`], then stores 0
/// with `Release`. Its own operations are associated functions
/// (`StampedGuard::restamp(&guard, ..)`), so they never shadow methods of `T`.
#[must_use = "dropping the guard releases the lock immediately"]
pub struct StampedGuard<'a, T, H: PreRelease = ()> {
    lock: &'a StampedLock<T, H>,
    /// Makes the guard `Sync` only for `T: Sync`, as a `&mut T` would be.
    _data: PhantomData<&'a mut T>,
}

impl<'a, T, H: PreRelease> StampedGuard<'a, T, H> {
    #[inline]
    fn new(lock: &'a StampedLock<T, H>) -> Self {
        Self {
            lock,
            _data: PhantomData,
        }
    }

    /// Replace the stamp in the held word.
    ///
    /// For the IRQs-on acquisition path: when a fresh [`read_stamp`] after the
    /// CAS differs from the stamp used (the thread was switched in between),
    /// the holder stores the fresh one so that a re-entry on its current CPU
    /// is caught. The store is `Release`, so a reader that acquires the new
    /// word also sees the previous holder's field clears.
    #[inline]
    pub fn restamp(this: &Self, stamp: OwnerStamp) {
        this.lock.word.store(stamp.word(), Ordering::Release);
    }
}

impl<T, H: PreRelease> Deref for StampedGuard<'_, T, H> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the guard exists only after a successful CAS from 0, and the
        // word stays non-zero until this guard's Drop, so no other guard
        // exists for this lock (StampedLock's Sync invariant, kept by
        // try_lock/try_lock_weak and Drop). A second guard would alias `data`
        // mutably: undefined behaviour.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T, H: PreRelease> DerefMut for StampedGuard<'_, T, H> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`: this guard is the only one for the lock, and
        // `&mut self` makes this the only live reference derived from it.
        // Violating exclusivity would alias `data` mutably: undefined behaviour.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T, H: PreRelease> Drop for StampedGuard<'_, T, H> {
    #[inline]
    fn drop(&mut self) {
        self.lock.hook.pre_release();
        self.lock.word.store(0, Ordering::Release);
    }
}

// The IRQ entry saves no V registers, so values on the IRQ path stay in
// general registers: the guard and a decoded stamp are one word, and a
// try_lock result is two (returned in x0/x1).
const _: () = assert!(core::mem::size_of::<Option<OwnerStamp>>() == 8);
const _: () =
    assert!(core::mem::size_of::<StampedGuard<'static, u64>>() == core::mem::size_of::<usize>());
const _: () = assert!(
    core::mem::size_of::<Result<StampedGuard<'static, u64>, u64>>()
        == 2 * core::mem::size_of::<usize>()
);

// ---------------------------------------------------------------------------
// LockClass
// ---------------------------------------------------------------------------

/// The 9 IRQ-shared lock statics.
///
/// The order is the index order of the 9-wide per-lock tripwire keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum LockClass {
    /// `task::THREAD_TABLE`.
    ThreadTable,
    /// `task::CURRENT_THREAD[cpu]`.
    CurrentThread,
    /// `sched::RUN_QUEUES[cpu]`.
    RunQueues,
    /// `ipc::timeout::WAKEUP_ERRORS`.
    WakeupErrors,
    /// `ipc::timeout::TIMEOUT_QUEUE`.
    TimeoutQueue,
    /// `ipc::notify::NOTIFY_DEADLINES`.
    NotifyDeadlines,
    /// `ipc::notify::NOTIFICATION_TABLE`.
    NotificationTable,
    /// `ipc::select::SELECT_WAITERS`.
    SelectWaiters,
    /// `observability::BOOT_LOG`.
    BootLog,
}

impl LockClass {
    /// Number of lock classes.
    pub const COUNT: usize = 9;

    /// Every class, in index order.
    pub const ALL: [LockClass; Self::COUNT] = [
        LockClass::ThreadTable,
        LockClass::CurrentThread,
        LockClass::RunQueues,
        LockClass::WakeupErrors,
        LockClass::TimeoutQueue,
        LockClass::NotifyDeadlines,
        LockClass::NotificationTable,
        LockClass::SelectWaiters,
        LockClass::BootLog,
    ];

    /// Index into 9-wide per-lock arrays.
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The static's name, as printed in `lock re-entry:` messages and
    /// `[tripwire-ev]` lines.
    pub const fn name(self) -> &'static str {
        match self {
            LockClass::ThreadTable => "THREAD_TABLE",
            LockClass::CurrentThread => "CURRENT_THREAD",
            LockClass::RunQueues => "RUN_QUEUES",
            LockClass::WakeupErrors => "WAKEUP_ERRORS",
            LockClass::TimeoutQueue => "TIMEOUT_QUEUE",
            LockClass::NotifyDeadlines => "NOTIFY_DEADLINES",
            LockClass::NotificationTable => "NOTIFICATION_TABLE",
            LockClass::SelectWaiters => "SELECT_WAITERS",
            LockClass::BootLog => "BOOT_LOG",
        }
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
mod tests {
    extern crate std;

    use super::*;
    use core::cell::Cell;
    use core::sync::atomic::AtomicU32;
    use std::thread;

    /// A fixed machine state seen from one CPU. CPUs past the array have
    /// generation `u64::MAX`.
    struct FixedView {
        cpu: u8,
        gens: [u64; 4],
    }

    impl CpuView for FixedView {
        fn cpu(&self) -> u8 {
            self.cpu
        }

        fn switch_gen(&self, cpu: u8) -> u64 {
            self.gens.get(usize::from(cpu)).copied().unwrap_or(u64::MAX)
        }
    }

    fn view(cpu: u8, gens: [u64; 2]) -> FixedView {
        FixedView {
            cpu,
            gens: [gens[0], gens[1], 0, 0],
        }
    }

    /// Kernel-style holder fields: a thread id cleared before release.
    struct Holder {
        tid: AtomicU32,
    }

    impl Holder {
        const fn new() -> Self {
            Self {
                tid: AtomicU32::new(TID_NONE),
            }
        }
    }

    impl PreRelease for Holder {
        fn pre_release(&self) {
            self.tid.store(TID_NONE, Ordering::Relaxed);
        }
    }

    fn read_tid(lock: &StampedLock<u64, Holder>) -> Option<(OwnerStamp, u32)> {
        lock.consistent_snapshot(|stamp, h| (stamp, h.tid.load(Ordering::Relaxed)))
    }

    // -- OwnerStamp codec --------------------------------------------------

    #[test]
    fn stamp_codec_round_trips() {
        let cpus = [0u8, 1, 3, 7, 0x80, 0xFE, 0xFF];
        let gens = [
            0u64,
            1,
            4711,
            OwnerStamp::GEN_MASK - 1,
            OwnerStamp::GEN_MASK,
        ];
        for cpu in cpus {
            for gen in gens {
                for irqs_on in [false, true] {
                    let s = OwnerStamp::new(cpu, gen, irqs_on);
                    assert_ne!(s.word() & OwnerStamp::HELD, 0);
                    assert_eq!(s.cpu(), cpu);
                    assert_eq!(s.gen(), gen);
                    assert_eq!(s.irqs_on(), irqs_on);
                    assert!(s.gen_matches(gen));
                    assert_eq!(OwnerStamp::from_word(s.word()), Some(s));
                }
            }
        }
    }

    #[test]
    fn stamp_bit_layout_is_exact() {
        assert_eq!(
            OwnerStamp::new(3, 5, true).word(),
            (1 << 63) | (1 << 62) | (3 << 54) | 5
        );
        assert_eq!(OwnerStamp::new(0, 0, false).word(), 1 << 63);
        assert_eq!(
            OwnerStamp::new(0xFF, OwnerStamp::GEN_MASK, true).word(),
            u64::MAX
        );
        assert_eq!(OwnerStamp::GEN_MASK, (1 << 54) - 1);
        assert_eq!(OwnerStamp::CPU_MASK, 0xFF << 54);
    }

    #[test]
    fn stamp_generation_truncates_without_touching_other_fields() {
        let s = OwnerStamp::new(2, (OwnerStamp::GEN_MASK + 1) + 9, false);
        assert_eq!((s.cpu(), s.gen(), s.irqs_on()), (2, 9, false));
        assert!(s.gen_matches((OwnerStamp::GEN_MASK + 1) + 9));
        assert!(s.gen_matches(9));
        assert!(!s.gen_matches(10));

        let s = OwnerStamp::new(2, u64::MAX, false);
        assert_eq!(
            (s.cpu(), s.gen(), s.irqs_on()),
            (2, OwnerStamp::GEN_MASK, false)
        );
        assert!(s.gen_matches(u64::MAX));
    }

    #[test]
    fn from_word_rejects_words_without_held() {
        assert_eq!(OwnerStamp::from_word(0), None);
        assert_eq!(OwnerStamp::from_word(OwnerStamp::IRQS_ON | 5), None);
        assert_eq!(OwnerStamp::from_word(!OwnerStamp::HELD), None);
    }

    // -- classify -----------------------------------------------------------

    #[test]
    fn classify_table() {
        use Contention::*;
        let gens = [10, 20, 30, OwnerStamp::GEN_MASK + 1 + 40];
        // (owner cpu, owner gen, caller cpu, expected)
        let rows: [(u8, u64, u8, Contention); 10] = [
            (1, 20, 1, Reentry),
            (1, 19, 1, PreemptedHolder),
            (1, 21, 1, PreemptedHolder),
            (2, 30, 1, OtherCpu),
            (2, 29, 1, OtherCpuSwitched),
            (0, 10, 3, OtherCpu),
            // Generations compare after truncation to 54 bits.
            (3, 40, 3, Reentry),
            (3, 40, 0, OtherCpu),
            // An owner CPU the view has no generation for.
            (200, 0, 1, OtherCpuSwitched),
            (0, 11, 0, PreemptedHolder),
        ];
        for (owner_cpu, owner_gen, caller, expected) in rows {
            for irqs_on in [false, true] {
                let word = OwnerStamp::new(owner_cpu, owner_gen, irqs_on).word();
                let v = FixedView { cpu: caller, gens };
                assert_eq!(
                    classify(word, caller, &v),
                    expected,
                    "owner ({owner_cpu}, {owner_gen}) irqs_on={irqs_on} caller {caller}"
                );
            }
        }
        for caller in 0..4 {
            assert_eq!(classify(0, caller, &FixedView { cpu: caller, gens }), Free);
        }
    }

    #[test]
    fn classify_ignores_irqs_on() {
        let v = view(0, [5, 6]);
        for (cpu, gen) in [(0u8, 5u64), (0, 4), (1, 6), (1, 7)] {
            let off = OwnerStamp::new(cpu, gen, false).word();
            let on = OwnerStamp::new(cpu, gen, true).word();
            assert_eq!(classify(off, 0, &v), classify(on, 0, &v));
        }
    }

    // -- read_stamp, exhaustive over 2 CPUs x 0-2 switch events ---------------

    /// One thread on a 2-CPU machine, moved by switch events scripted by read
    /// number (read 0: first CPU read, 1: generation read, 2: second CPU read).
    ///
    /// A switch follows kernel rule (a): when the thread leaves CPU `old`,
    /// `old` dispatches another thread (`SWITCH_GEN[old] += 1`), and when the
    /// thread resumes on `to`, `to` dispatches it (`SWITCH_GEN[to] += 1`).
    struct Machine {
        on: Cell<u8>,
        gens: [Cell<u64>; 2],
        switches: Cell<u32>,
        reads: Cell<usize>,
        script: [&'static [u8]; 3],
        switches_at_gen_read: Cell<u32>,
    }

    impl Machine {
        fn new(start: u8, script: [&'static [u8]; 3]) -> Self {
            Self {
                on: Cell::new(start),
                gens: [Cell::new(100), Cell::new(200)],
                switches: Cell::new(0),
                reads: Cell::new(0),
                script,
                switches_at_gen_read: Cell::new(0),
            }
        }

        fn switch_to(&self, to: u8) {
            let old = &self.gens[usize::from(self.on.get())];
            old.set(old.get() + 1);
            let new = &self.gens[usize::from(to)];
            new.set(new.get() + 1);
            self.on.set(to);
            self.switches.set(self.switches.get() + 1);
        }

        fn step(&self) {
            let k = self.reads.get();
            self.reads.set(k + 1);
            if let Some(events) = self.script.get(k) {
                for &to in *events {
                    self.switch_to(to);
                }
            }
        }

        fn gens_now(&self) -> [u64; 2] {
            [self.gens[0].get(), self.gens[1].get()]
        }
    }

    impl CpuView for Machine {
        fn cpu(&self) -> u8 {
            self.step();
            self.on.get()
        }

        fn switch_gen(&self, cpu: u8) -> u64 {
            self.step();
            self.switches_at_gen_read.set(self.switches.get());
            self.gens.get(usize::from(cpu)).map_or(u64::MAX, Cell::get)
        }
    }

    const SEQS: [&[u8]; 7] = [&[], &[0], &[1], &[0, 0], &[0, 1], &[1, 0], &[1, 1]];

    #[test]
    fn read_stamp_is_reentry_only_while_unswitched_on_its_cpu() {
        let mut cases = 0;
        let mut retried = 0;
        for start in 0..2u8 {
            for before_gen in SEQS {
                for before_cpu2 in SEQS {
                    for after in SEQS {
                        let m = Machine::new(start, [&[], before_gen, before_cpu2]);
                        let (cpu, gen) = read_stamp(&m);
                        assert_eq!(cpu, m.on.get(), "stamp names the CPU of the last read");
                        if m.reads.get() > 3 {
                            retried += 1;
                        }
                        for &to in after {
                            m.switch_to(to);
                        }
                        // Switches of this thread after the generation read
                        // that read_stamp returned.
                        let switched = m.switches.get() - m.switches_at_gen_read.get();
                        let here = m.on.get();
                        for irqs_on in [false, true] {
                            let word = OwnerStamp::new(cpu, gen, irqs_on).word();
                            for observer in 0..2u8 {
                                let verdict =
                                    classify(word, observer, &view(observer, m.gens_now()));
                                // The stream on `here` is the holder itself
                                // (an IRQ interrupting it); any other CPU runs
                                // another stream.
                                let own = observer == here;
                                let ctx = (start, before_gen, before_cpu2, after, observer);
                                if verdict == Contention::Reentry {
                                    assert!(own && switched == 0, "false Reentry: {ctx:?}");
                                }
                                if own && switched == 0 {
                                    assert_eq!(verdict, Contention::Reentry, "missed: {ctx:?}");
                                }
                                if own && here != cpu {
                                    assert_eq!(verdict, Contention::OtherCpuSwitched, "{ctx:?}");
                                }
                                if !own && observer == cpu {
                                    assert_eq!(verdict, Contention::PreemptedHolder, "{ctx:?}");
                                }
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 2 * 7 * 7 * 7 * 2 * 2);
        assert!(retried > 0, "the CPU-mismatch retry path was never taken");
    }

    // -- Interleaving model: preemption and migration of a holder -----------

    #[test]
    fn preempted_holder_migrated_to_other_cpu_is_never_reentry() {
        use Contention::*;
        let (c, d) = (0u8, 1u8);
        let mut gens = [5u64, 8];

        // Holder H on c takes the lock with IRQs on.
        let (cpu, gen) = read_stamp(&view(c, gens));
        let s = OwnerStamp::new(cpu, gen, true).word();
        // An IRQ on c interrupting H re-enters: Reentry.
        assert_eq!(classify(s, c, &view(c, gens)), Reentry);
        assert_eq!(classify(s, d, &view(d, gens)), OtherCpu);

        // A tick on c preempts H: c dispatches another thread.
        gens[0] += 1;
        assert_eq!(classify(s, c, &view(c, gens)), PreemptedHolder);
        assert_eq!(classify(s, d, &view(d, gens)), OtherCpuSwitched);

        // The balancer migrates H to d, and d dispatches it. H still holds
        // the lock. An IRQ on d that interrupts H and re-enters sees a stamp
        // naming c, which has switched since: OtherCpuSwitched, never Reentry.
        gens[1] += 1;
        assert_eq!(classify(s, d, &view(d, gens)), OtherCpuSwitched);
        for _ in 0..3 {
            gens[0] += 1;
            gens[1] += 1;
            assert_eq!(classify(s, d, &view(d, gens)), OtherCpuSwitched);
            assert_eq!(classify(s, c, &view(c, gens)), PreemptedHolder);
        }

        // Re-stamping on d (the IRQs-on second read_stamp) makes H's own IRQ
        // on d a Reentry again.
        let (cpu, gen) = read_stamp(&view(d, gens));
        let s2 = OwnerStamp::new(cpu, gen, true).word();
        assert_eq!(classify(s2, d, &view(d, gens)), Reentry);
        assert_eq!(classify(s2, c, &view(c, gens)), OtherCpu);
    }

    #[test]
    fn holder_resumed_on_its_own_cpu_is_preempted_holder() {
        // H stamps on c, is switched out (c dispatches another thread) and
        // later resumed on c (c dispatches H). Its own IRQ on c now sees an
        // old generation: PreemptedHolder, not Reentry. The kernel's holder-tid
        // check (lkself) is what reports this same-stream case.
        let mut gens = [40u64, 0];
        let s = OwnerStamp::new(0, gens[0], false).word();
        gens[0] += 2;
        assert_eq!(classify(s, 0, &view(0, gens)), Contention::PreemptedHolder);
    }

    // -- StampedLock ---------------------------------------------------------

    #[test]
    fn try_lock_takes_a_free_lock_and_reports_the_owner_word() {
        let lock = StampedLock::new(5u32);
        let a = OwnerStamp::new(1, 7, true);
        let b = OwnerStamp::new(2, 9, false);
        {
            let mut g = lock.try_lock(a).unwrap();
            assert_eq!(lock.owner_word(), a.word());
            assert_eq!(lock.try_lock(b).err(), Some(a.word()));
            assert_eq!(lock.try_lock(a).err(), Some(a.word()));
            *g += 1;
        }
        assert_eq!(lock.owner_word(), 0);
        let g = lock.try_lock(b).unwrap();
        assert_eq!(*g, 6);
        assert_eq!(lock.owner_word(), b.word());
    }

    #[test]
    fn try_lock_weak_takes_a_free_lock_and_fails_on_a_held_one() {
        let lock = StampedLock::new(0u32);
        let a = OwnerStamp::new(0, 1, false);
        let g = loop {
            match lock.try_lock_weak(a) {
                Ok(g) => break g,
                // Spurious failure of a free lock.
                Err(w) => assert_eq!(w, 0),
            }
        };
        let b = OwnerStamp::new(1, 1, false);
        for _ in 0..8 {
            assert_eq!(lock.try_lock_weak(b).err(), Some(a.word()));
        }
        drop(g);
        assert_eq!(lock.owner_word(), 0);
    }

    #[test]
    fn restamp_replaces_the_stamp_and_keeps_the_lock_held() {
        let lock = StampedLock::new(());
        let a = OwnerStamp::new(0, 3, true);
        let b = OwnerStamp::new(1, 4, true);
        let g = lock.try_lock(a).unwrap();
        StampedGuard::restamp(&g, b);
        assert_eq!(lock.owner_word(), b.word());
        assert_eq!(lock.try_lock(a).err(), Some(b.word()));
        drop(g);
        assert_eq!(lock.owner_word(), 0);
    }

    struct Probe {
        word_at_release: AtomicU64,
        calls: AtomicU32,
    }

    static PROBED: StampedLock<u32, Probe> = StampedLock::with_hook(
        0,
        Probe {
            word_at_release: AtomicU64::new(0),
            calls: AtomicU32::new(0),
        },
    );

    impl PreRelease for Probe {
        fn pre_release(&self) {
            self.word_at_release
                .store(PROBED.owner_word(), Ordering::Relaxed);
            self.calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn pre_release_runs_once_while_the_lock_is_still_held() {
        let a = OwnerStamp::new(4, 44, false);
        let b = OwnerStamp::new(5, 55, true);
        let g = PROBED.try_lock(a).unwrap();
        // A failed acquisition does not run the hook.
        assert!(PROBED.try_lock(b).is_err());
        assert!(PROBED.try_lock_weak(b).is_err());
        assert_eq!(PROBED.hook().calls.load(Ordering::Relaxed), 0);
        drop(g);
        assert_eq!(PROBED.hook().calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            PROBED.hook().word_at_release.load(Ordering::Relaxed),
            a.word()
        );
        assert_eq!(PROBED.owner_word(), 0);
    }

    // -- Consistent snapshot ------------------------------------------------

    #[test]
    fn consistent_snapshot_needs_a_held_word_that_did_not_change() {
        let lock = StampedLock::with_hook(0u64, Holder::new());
        assert_eq!(read_tid(&lock), None, "free lock");

        let a = OwnerStamp::new(1, 7, true);
        let guard = Cell::new(Some(lock.try_lock(a).unwrap()));
        // Between the CAS and the field store: valid, tid still NONE.
        assert_eq!(read_tid(&lock), Some((a, TID_NONE)));
        lock.hook().tid.store(12, Ordering::Release);
        assert_eq!(read_tid(&lock), Some((a, 12)));

        // Restamped between the two word loads: rejected.
        let b = OwnerStamp::new(0, 8, true);
        let r = lock.consistent_snapshot(|s, h| {
            let tid = h.tid.load(Ordering::Relaxed);
            let g = guard.take().unwrap();
            StampedGuard::restamp(&g, b);
            guard.set(Some(g));
            (s, tid)
        });
        assert_eq!(r, None);
        assert_eq!(read_tid(&lock), Some((b, 12)));

        // Released and taken by another holder between the loads: rejected.
        let c = OwnerStamp::new(2, 9, false);
        let r = lock.consistent_snapshot(|s, h| {
            let tid = h.tid.load(Ordering::Relaxed);
            drop(guard.take());
            let g = lock.try_lock(c).unwrap();
            lock.hook().tid.store(34, Ordering::Release);
            guard.set(Some(g));
            (s, tid)
        });
        assert_eq!(r, None);
        assert_eq!(read_tid(&lock), Some((c, 34)));

        // Released between the loads (second load sees 0): rejected.
        let r = lock.consistent_snapshot(|s, h| {
            let tid = h.tid.load(Ordering::Relaxed);
            drop(guard.take());
            (s, tid)
        });
        assert_eq!(r, None);
        assert_eq!(read_tid(&lock), None);
        assert_eq!(
            lock.hook().tid.load(Ordering::Relaxed),
            TID_NONE,
            "cleared by the hook"
        );
    }

    // -- Threads (and Miri) -------------------------------------------------

    #[test]
    fn four_threads_exclude_each_other_and_snapshots_stay_consistent() {
        #[cfg(miri)]
        const ITERS: u64 = 25;
        #[cfg(not(miri))]
        const ITERS: u64 = 20_000;
        const THREADS: u8 = 4;

        let lock = StampedLock::with_hook(0u64, Holder::new());
        thread::scope(|scope| {
            for t in 0..THREADS {
                let lock = &lock;
                scope.spawn(move || {
                    for i in 0..ITERS {
                        // Stamps are unique per acquisition and restamp
                        // (cpu = t, distinct generations), so a snapshot can
                        // never be fooled by a reused word.
                        let stamp = OwnerStamp::new(t, 2 * i + 1, i % 2 == 0);
                        let mut g = loop {
                            match lock.try_lock_weak(stamp) {
                                Ok(g) => break g,
                                Err(_) => {
                                    if let Some((owner, tid)) = read_tid(lock) {
                                        assert!(
                                            tid == TID_NONE || tid == u32::from(owner.cpu()),
                                            "fields of another holder: stamp cpu {} tid {tid}",
                                            owner.cpu()
                                        );
                                    }
                                    while lock.owner_word() != 0 {
                                        core::hint::spin_loop();
                                    }
                                }
                            }
                        };
                        lock.hook().tid.store(u32::from(t), Ordering::Release);
                        // A plain read-modify-write of the protected data:
                        // Miri reports a data race if exclusion ever fails.
                        *g += 1;
                        if i % 3 == 0 {
                            StampedGuard::restamp(&g, OwnerStamp::new(t, 2 * i + 2, false));
                        }
                        drop(g);
                    }
                });
            }
        });
        assert_eq!(lock.owner_word(), 0);
        assert_eq!(lock.hook().tid.load(Ordering::Relaxed), TID_NONE);
        let g = lock.try_lock(OwnerStamp::new(0, 0, false)).unwrap();
        assert_eq!(*g, u64::from(THREADS) * ITERS);
    }

    // -- LockClass -------------------------------------------------------------

    #[test]
    fn lock_class_names_and_indices() {
        let expected = [
            "THREAD_TABLE",
            "CURRENT_THREAD",
            "RUN_QUEUES",
            "WAKEUP_ERRORS",
            "TIMEOUT_QUEUE",
            "NOTIFY_DEADLINES",
            "NOTIFICATION_TABLE",
            "SELECT_WAITERS",
            "BOOT_LOG",
        ];
        assert_eq!(LockClass::ALL.len(), LockClass::COUNT);
        assert_eq!(expected.len(), LockClass::COUNT);
        for (i, class) in LockClass::ALL.into_iter().enumerate() {
            assert_eq!(class.index(), i);
            assert_eq!(class.name(), expected[i]);
            assert!(class
                .name()
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b == b'_'));
            for other in LockClass::ALL {
                if other != class {
                    assert_ne!(other.name(), class.name());
                }
            }
        }
    }
}
