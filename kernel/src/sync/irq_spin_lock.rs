//! `IrqSpinLock`: the detect-only spinlock of the IRQ-shared statics
//! (crash-fix step 1b).
//!
//! The timer IRQ path shares 9 lock statics with thread code
//! ([`LockClass`]: THREAD_TABLE, CURRENT_THREAD[cpu], RUN_QUEUES[cpu],
//! WAKEUP_ERRORS, TIMEOUT_QUEUE, NOTIFY_DEADLINES, NOTIFICATION_TABLE,
//! SELECT_WAITERS and BOOT_LOG). They use [`IrqSpinLock`] instead of
//! `spin::Mutex`. It excludes exactly as `spin::Mutex` does: one CAS takes
//! the word from 0, `store(0, Release)` frees it, a waiter spins on a plain
//! load between attempts, and there is no fairness. What it adds is
//! detection, and it changes no scheduling decision:
//!
//! - The lock word is the holder's owner stamp, `(cpu, switch generation)`
//!   (`shared::lock`). A `lock()` whose word holds its own stream's stamp can
//!   never succeed, so it panics with a one-line `lock re-entry:` message
//!   ([`shared::tripwire::write_reentry_msg`]) where `spin::Mutex` would spin
//!   forever. It does so in every context; the message labels the context.
//! - Other contention is classified once per `lock()` call and counted in the
//!   9-wide per-lock tripwire keys: `lkph`/`lkpho` (a holder switched out on
//!   this CPU, IRQs masked/on), `lkphrun` (that holder is current on another
//!   CPU), `lkoxp` (another CPU's holder that CPU has switched out since),
//!   `lkself` (the holder is the waiter's own thread), `lkstk` (a wait of
//!   more than 2 s), and for `try_lock()` failures `lktry` (a re-entry) and
//!   `lktph` (a switched-out holder). The rare cases also print one
//!   `[tripwire-ev]` line (`kind=ph`, `self` or `stuck`), from this CPU
//!   straight to the UART.
//! - Holder fields (thread slot and `lock()` call site) name the holder in
//!   both.
//!
//! [`held_by_stream`] and [`note_restore`] check rule (b) below at run time,
//! and CPU 0's timer tick holds its tripwire line back while
//! [`held_by_stream`]`(0)`.
//!
//! # Why a re-entry verdict is never false
//!
//! `shared::lock::classify` reports a re-entry only when the word holds the
//! waiter's own stamp `(c, g)` and `SWITCH_GEN[c]`, read after the word was
//! loaded, is still g. That is proof that the holder is the waiter's own
//! stream (its thread earlier in the same generation, or the thread an IRQ
//! waiter interrupted), given three rules the kernel keeps:
//!
//! - (a) every `restore_context` on CPU c is preceded, on c with IRQs masked,
//!   by `tripwire::note_dispatch` bumping `SWITCH_GEN[c]`, with c read from
//!   MPIDR: 5 restores after 4 commit sites (`enter_scheduler`, `schedule()`
//!   twice, `try_direct_switch`, `try_reply_switch`). Stamps read c from
//!   TPIDR_EL1, which boot.S sets to MPIDR Aff0 on every CPU before any Rust
//!   code and nothing writes again, so both reads name the same CPU;
//!   `tripwire::check_tpidr` and `note_dispatch` count a breach
//!   (`tpidrbad`). A CPU whose TPIDR_EL1 named another CPU would stamp with
//!   that CPU's generation, and its holders could match a waiter there;
//! - (b) the dispatching stream releases every guard it took after that bump
//!   before it calls `restore_context`. Those are the `CURRENT_THREAD`
//!   temporaries after `save_context` and `try_reply_switch`'s
//!   `enqueue_on_cpu`; [`note_restore`] counts any breach (`rsthold`);
//! - (c) a thread's execution is ordered across a switch: everything the CPU
//!   that switched it out did first, the bump in (a) included, happens before
//!   the thread's next instruction on any CPU. A thread resumed before its
//!   context was saved (the missing `on_cpu` handshake, crash-fix ADR H2/F4)
//!   breaks this and every other invariant of that thread.
//!
//! Guards that a switched-out thread still holds are expected. Their stale
//! stamps classify as a switched-out holder or another CPU's holder, which
//! are counted and never panic.
//!
//! # Holder fields
//!
//! Stored with `Release` right after the CAS, before the holder re-stamps,
//! and cleared by the guard before its releasing store ([`PreRelease`]). A
//! waiter reads them only through `StampedLock::consistent_snapshot`, and
//! only if the snapshot's word is the word it classified, so it never pairs
//! one holder's fields with another's stamp. An IRQ between the CAS and the
//! field stores, or between the clears and the release, sees `?` fields and
//! still decides correctly. In a short critical section with no branch of
//! its own, the only point where QEMU takes an IRQ is right after the CAS,
//! before the field stores (see IRQ-path rules), so an IRQ that interrupts
//! such a holder sees `?` fields.
//!
//! The call site is stored as a kernel virtual address. CPUs 1-3 run the
//! IRQ path at physical-alias PCs, so `Location::caller()` computed there is
//! a physical address, and [`kva_of`] adds `VIRT_PHYS_OFFSET` to it
//! (step-1b plan §2.11). Lock and static addresses are only dereferenced, so
//! they need no such care.
//!
//! # IRQ-path rules
//!
//! `lock()` and `try_lock()` run in the timer IRQ, whose entry saves no V
//! registers. The lock path uses no `core::fmt`, no other lock, no `klog!`,
//! no stack arrays and no copies of aggregates of 16 bytes or more. Its
//! helpers are `#[inline(never)]`, so a disassembly attributes their
//! instructions to them, except the holder bookkeeping that runs inside the
//! critical section (`IrqSpinLock::hold`: the field stores and
//! [`current_stamp_or`]), which is inline and branch-free. QEMU TCG takes a
//! pending IRQ only where a translation block starts, and every branch or
//! call starts one, so a branch there would widen the window in which the
//! timer IRQ interrupts an IRQs-on holder. The fast paths leave one such
//! point, right after the CAS, as `spin::Mutex` does. Counters use
//! `tripwire::bump`, which masks IRQs for its load and store; there is no
//! atomic read-modify-write besides the lock's own CAS. The one exception to
//! these rules is [`reentry_panic`], which ends in the panic handler and
//! never returns to the interrupted code.

#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use core::panic::Location;
use core::ptr;
use core::sync::atomic::{compiler_fence, AtomicPtr, AtomicU32, Ordering};

use shared::lock::{
    classify, read_stamp, Contention, CpuView, LockClass, OwnerStamp, PreRelease, StampedGuard,
    StampedLock, SCALAR_INDEX, TID_NONE,
};
use shared::tripwire::{
    put_dec, write_reentry_msg, BufSink, Ctx, HolderSite, Key, ReentryReport, Sink,
    MAX_REENTRY_MSG_LEN,
};

use crate::arch::aarch64::mmu::{KERNEL_BASE, VIRT_PHYS_OFFSET};
use crate::arch::aarch64::timer;
use crate::observability::tripwire::{
    bump, bump_masked, cpu_here, cpu_tpidr, current_tid, irq_ctx, read_daif, switch_gen,
    switch_gen_masked, UartSink, DAIF_I,
};
use crate::smp::{self, MAX_CORES};

/// A `lock()` that has waited this many seconds counts `lkstk` and prints a
/// `kind=stuck` line, once.
const STUCK_SECS: u64 = 2;

/// The spin loop reads the counter once every `STUCK_CHECK_MASK + 1`
/// iterations.
const STUCK_CHECK_MASK: u32 = 1023;

/// `holder_running` value: the holder's thread id is unknown.
const RUNNING_UNKNOWN: u8 = 0xFF;
/// `holder_running` value: the holder is not current on another CPU.
const RUNNING_NONE: u8 = 0xFE;

/// `lock()` bookkeeping flags: what this call has already counted.
const NOTED_PREEMPTED: u8 = 1 << 0;
const NOTED_SWITCHED: u8 = 1 << 1;
const NOTED_STUCK: u8 = 1 << 2;

// ---------------------------------------------------------------------------
// The lock
// ---------------------------------------------------------------------------

/// A lock's identity in counters and messages: its static and its index in
/// a per-CPU array ([`SCALAR_INDEX`] for a scalar static).
#[derive(Clone, Copy)]
struct LockId {
    class: LockClass,
    index: u8,
}

/// The holder's diagnostic fields, the [`PreRelease`] hook of the inner lock.
pub struct HolderFields {
    /// `Location::caller()` of the holder's `lock()`, as a kernel virtual
    /// address; null while no holder has stored it.
    site: AtomicPtr<Location<'static>>,
    /// The holder stream's thread slot (`CURRENT_TID`), or [`TID_NONE`].
    tid: AtomicU32,
}

impl HolderFields {
    const fn new() -> Self {
        Self {
            site: AtomicPtr::new(ptr::null_mut()),
            tid: AtomicU32::new(TID_NONE),
        }
    }
}

impl PreRelease for HolderFields {
    /// Clear the fields while the lock is still held, so that no reader sees
    /// them beside the next holder's stamp.
    #[inline(always)]
    fn pre_release(&self) {
        self.site.store(ptr::null_mut(), Ordering::Relaxed);
        self.tid.store(TID_NONE, Ordering::Relaxed);
    }
}

/// Exclusive access to an [`IrqSpinLock`]'s data, one pointer wide. Dropping
/// it clears the holder fields and then releases the lock.
pub type IrqSpinLockGuard<'a, T> = StampedGuard<'a, T, HolderFields>;

/// The detect-only IRQ-class spinlock. See the module documentation.
///
/// Excludes like `spin::Mutex`; a re-entry by the holder's own stream
/// panics (`lock re-entry:`), and other contention is counted. The
/// re-entry verdict rests on rules (a) to (c) in the module documentation.
pub struct IrqSpinLock<T> {
    inner: StampedLock<T, HolderFields>,
    id: LockId,
}

impl<T> IrqSpinLock<T> {
    /// A free lock of static `class`, with index [`SCALAR_INDEX`].
    pub const fn new(class: LockClass, data: T) -> Self {
        Self {
            inner: StampedLock::with_hook(data, HolderFields::new()),
            id: LockId {
                class,
                index: SCALAR_INDEX,
            },
        }
    }

    /// Set the per-CPU array index the lock reports. For the static
    /// initialisers of `[IrqSpinLock<T>; MAX_CORES]` arrays.
    pub const fn set_index(&mut self, index: u8) {
        self.id.index = index;
    }

    /// The lock word: 0 when free, otherwise the holder's owner stamp.
    /// `Relaxed`; it orders nothing.
    #[inline]
    pub fn owner_word(&self) -> u64 {
        self.inner.owner_word()
    }

    /// Acquire the lock, spinning while another stream holds it.
    ///
    /// Always inlined, as `spin::Mutex::lock` was at every site: a call
    /// would put its return, one more translation-block start, inside the
    /// caller's critical section (see `Self::hold`).
    ///
    /// # Panics
    ///
    /// If the holder is the calling stream itself (a wait that could never
    /// end), with a `lock re-entry:` message located at the caller.
    #[inline(always)]
    #[track_caller]
    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        let (stamp, tid) = whoami();
        match self.inner.try_lock_weak(stamp) {
            Some(guard) => {
                self.hold(&guard, stamp, tid, Location::caller());
                guard
            }
            None => self.lock_contended(stamp, tid),
        }
    }

    /// Acquire the lock if it is free. Never waits and never panics.
    ///
    /// A failure counts `lktry` if the holder is the calling stream, or
    /// `lktph` if it is a thread switched out on this CPU. Always inlined,
    /// as [`Self::lock`].
    #[inline(always)]
    #[track_caller]
    pub fn try_lock(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let (stamp, tid) = whoami();
        match self.inner.try_lock(stamp) {
            Some(guard) => {
                self.hold(&guard, stamp, tid, Location::caller());
                Some(guard)
            }
            None => {
                // `stamp` was taken before this load, as `classify` needs.
                note_try_failed(self.id.class, stamp, self.inner.owner_word());
                None
            }
        }
    }

    /// [`Self::try_lock`] without the failure counters, for the heartbeat
    /// scans, which would otherwise count their own probes.
    #[expect(
        dead_code,
        reason = "the heartbeat scans (step-1b task K8) are the callers"
    )]
    #[inline(always)]
    #[track_caller]
    pub fn try_lock_quiet(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let (stamp, tid) = whoami();
        let guard = self.inner.try_lock(stamp)?;
        self.hold(&guard, stamp, tid, Location::caller());
        Some(guard)
    }

    /// Record the new holder, right after its CAS: store the holder fields
    /// (`Release`), then store the holder's stamp as it is now, which differs
    /// from `stamp` only if the holder was switched between its stamp and
    /// the CAS (possible only with IRQs on).
    ///
    /// This runs inside the critical section, so it is inline and has no
    /// branch or call: the re-stamp is a select and an unconditional store.
    /// QEMU TCG takes a pending IRQ only where a translation block starts,
    /// and every branch or call starts one. Each one inside an IRQs-on hold
    /// widens the window in which the timer IRQ can interrupt the holder,
    /// which is the re-entry hazard this lock exists to report (step-1b
    /// plan, K5 decisions).
    #[inline(always)]
    fn hold(
        &self,
        guard: &IrqSpinLockGuard<'_, T>,
        stamp: OwnerStamp,
        tid: u32,
        site: &'static Location<'static>,
    ) {
        let fields = self.inner.hook();
        fields.tid.store(tid, Ordering::Release);
        fields.site.store(kva_of(site), Ordering::Release);
        StampedGuard::restamp(guard, current_stamp_or(stamp));
    }

    /// The holder fields of the holder whose word is `expect`, or
    /// `(TID_NONE, null)` if the lock changed hands or was free.
    #[inline(always)]
    fn holder_snapshot(&self, expect: u64) -> (u32, *const Location<'static>) {
        let snap = self.inner.consistent_snapshot(|stamp, fields| {
            (
                stamp.word(),
                fields.tid.load(Ordering::Relaxed),
                fields.site.load(Ordering::Relaxed),
            )
        });
        match snap {
            Some((word, tid, site)) if word == expect => (tid, site.cast_const()),
            _ => (TID_NONE, ptr::null()),
        }
    }

    /// The contended path of [`Self::lock`]: classify, count once per call,
    /// spin until the word is 0, and retry.
    ///
    /// `stamp` is the stamp the failed CAS used. Each round classifies the
    /// word loaded after that CAS failed, so the stamp was taken before the
    /// load, as `classify` requires; the word is 0 if the holder has released
    /// since or the weak CAS failed spuriously. A waiter with IRQs on takes a
    /// fresh stamp after each spin, before its next CAS.
    #[inline(never)]
    #[track_caller]
    fn lock_contended(&self, mut stamp: OwnerStamp, mut tid: u32) -> IrqSpinLockGuard<'_, T> {
        let start = timer::read_counter();
        let mut noted: u8 = 0;
        loop {
            let observed = self.inner.owner_word();
            if observed != 0 {
                match classify(observed, stamp, &KernelView) {
                    Contention::Reentry => {
                        let (holder_tid, site) = self.holder_snapshot(observed);
                        reentry_panic(self.id, stamp, observed, holder_tid, site);
                    }
                    Contention::PreemptedHolder if noted & NOTED_PREEMPTED == 0 => {
                        noted |= NOTED_PREEMPTED;
                        let (holder_tid, site) = self.holder_snapshot(observed);
                        note_preempted_holder(self.id, stamp, tid, observed, holder_tid, site);
                    }
                    Contention::OtherCpuSwitched if noted & NOTED_SWITCHED == 0 => {
                        noted |= NOTED_SWITCHED;
                        let (holder_tid, site) = self.holder_snapshot(observed);
                        note_other_cpu_switched(self.id, tid, observed, holder_tid, site);
                    }
                    Contention::Free
                    | Contention::OtherCpu
                    | Contention::PreemptedHolder
                    | Contention::OtherCpuSwitched => {}
                }
                let mut spins: u32 = 0;
                while self.inner.owner_word() != 0 {
                    core::hint::spin_loop();
                    spins = spins.wrapping_add(1);
                    if spins & STUCK_CHECK_MASK == 0
                        && noted & NOTED_STUCK == 0
                        && waited_too_long(start)
                    {
                        noted |= NOTED_STUCK;
                        let word = self.inner.owner_word();
                        let (holder_tid, site) = self.holder_snapshot(word);
                        note_stuck(self.id, tid, word, holder_tid, site);
                    }
                }
                if stamp.irqs_on() {
                    // The thread may have been switched or moved while it
                    // spun, so the next classification needs a new stamp.
                    (stamp, tid) = whoami();
                }
            }
            if let Some(guard) = self.inner.try_lock_weak(stamp) {
                self.hold(&guard, stamp, tid, Location::caller());
                return guard;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stamps
// ---------------------------------------------------------------------------

/// The running kernel as `shared::lock` sees it.
///
/// `cpu()` is `tripwire::cpu_tpidr`, the MPIDR Aff0 copy boot.S keeps in
/// TPIDR_EL1 (rule (a)). Its `asm!` has none of `pure`, `nomem` or
/// `readonly`: a fresh read and a compiler barrier on every call. Under QEMU
/// TCG it is one inline load, where an MPIDR read is two helper calls, and a
/// stamp reads the CPU id twice per round. `switch_gen()` is a fresh
/// `Relaxed` load of `SWITCH_GEN[cpu]`. Both are what `CpuView` requires.
struct KernelView;

impl CpuView for KernelView {
    #[inline(always)]
    fn cpu(&self) -> u8 {
        cpu_tpidr()
    }

    #[inline(always)]
    fn switch_gen(&self, cpu: u8) -> u64 {
        switch_gen(cpu)
    }
}

/// The calling stream's owner stamp and thread slot.
///
/// Takes `(cpu, gen)` with `read_stamp`, reads `CURRENT_TID[cpu]`, then
/// `SWITCH_GEN[cpu]` again, and retries if the generation moved. An
/// unchanged generation means the caller stayed on `cpu` from the first
/// generation read to the second: a switch away bumps it (rule (a)), and
/// under rule (c) the caller sees that bump wherever it runs next. So the
/// slot is the caller's own thread's (the interrupted thread's, in an IRQ),
/// even if the stamp itself is stale by the time the CAS runs. A thread's
/// slot does not change when it moves, so the slot stays right after a
/// re-stamp. IRQS_ON records DAIF.I at the call.
#[inline(never)]
fn whoami() -> (OwnerStamp, u32) {
    let irqs_on = read_daif() & DAIF_I == 0;
    loop {
        let (cpu, gen) = read_stamp(&KernelView);
        let tid = current_tid(cpu);
        // Keep the second generation load after the slot load. The CPU id
        // read that ends read_stamp already keeps the slot load after the
        // first one.
        compiler_fence(Ordering::Acquire);
        if switch_gen(cpu) == gen {
            return (OwnerStamp::new(cpu, gen, irqs_on), tid);
        }
    }
}

/// A holder's call site as a kernel virtual address.
///
/// `Location` statics live in the kernel image. Code at a physical-alias PC
/// (the IRQ path on CPUs 1-3) computes their physical address, which lies
/// below [`KERNEL_BASE`]; adding `VIRT_PHYS_OFFSET` gives the same object's
/// kernel virtual address.
#[inline(always)]
fn kva_of(site: &'static Location<'static>) -> *mut Location<'static> {
    let site: *const Location<'static> = site;
    site.cast_mut().map_addr(|addr| {
        if addr < KERNEL_BASE {
            addr.wrapping_add(VIRT_PHYS_OFFSET as usize)
        } else {
            addr
        }
    })
}

/// The calling stream's stamp now, for a holder right after its CAS, with
/// `stamp`'s IRQS_ON bit; `stamp` itself if the caller moved during the
/// reads.
///
/// One round of `read_stamp` without its retry: the CPU id, `SWITCH_GEN[cpu]`,
/// the CPU id again, from TPIDR_EL1 as in [`KernelView`] (each `asm!` is a
/// compiler barrier, so the load stays between them). If both CPU reads
/// agree, the pair is what `read_stamp` would return, so it names the
/// caller's current generation or one that had already ended. Otherwise `stamp`, which came from `read_stamp` in
/// [`whoami`], has the same property. That is all the re-entry verdict
/// needs from a holder's word: an ended generation never matches a waiter
/// that is still in its own, so a stale word can only hide a re-entry,
/// never invent one. With IRQs masked nothing can move the caller or bump
/// its CPU's generation, so the result equals `stamp`.
///
/// Branch-free, because it runs inside the critical section: the index of
/// `switch_gen_masked` needs no bounds branch, and the choice is a select.
/// `select_unpredictable` is used for its codegen (a `csel`), not because
/// the choice is hard to predict.
#[inline(always)]
fn current_stamp_or(stamp: OwnerStamp) -> OwnerStamp {
    let cpu = cpu_tpidr();
    let gen = switch_gen_masked(cpu);
    let again = cpu_tpidr();
    core::hint::select_unpredictable(
        cpu == again,
        OwnerStamp::new(cpu, gen, stamp.irqs_on()),
        stamp,
    )
}

// ---------------------------------------------------------------------------
// Contention bookkeeping
// ---------------------------------------------------------------------------

/// A `try_lock()` found the lock held: count a re-entry (`lktry`) or a
/// holder switched out on this CPU (`lktph`). `observed` is the word loaded
/// after the failed CAS; 0 (released since) counts nothing.
#[inline(never)]
fn note_try_failed(class: LockClass, me: OwnerStamp, observed: u64) {
    let key = match classify(observed, me, &KernelView) {
        Contention::Reentry => Key::Lktry,
        Contention::PreemptedHolder => Key::Lktph,
        Contention::Free | Contention::OtherCpu | Contention::OtherCpuSwitched => return,
    };
    bump(key, class.index());
}

/// Whether the holder's thread is the waiter's own: then waiting cannot end
/// while the waiter spins, whatever the stamps say.
#[inline(always)]
fn is_self(holder_tid: u32, my_tid: u32) -> bool {
    holder_tid != TID_NONE && holder_tid == my_tid
}

/// The first online CPU other than `exclude` whose current thread is
/// `holder_tid`, [`RUNNING_NONE`] if there is none, or [`RUNNING_UNKNOWN`]
/// if the holder's thread is unknown.
#[inline(never)]
fn holder_running(holder_tid: u32, exclude: u8) -> u8 {
    if holder_tid == TID_NONE {
        return RUNNING_UNKNOWN;
    }
    let online = smp::online_cpus().min(MAX_CORES);
    let mut cpu: u8 = 0;
    while usize::from(cpu) < online {
        if cpu != exclude && current_tid(cpu) == holder_tid {
            return cpu;
        }
        cpu = cpu.wrapping_add(1);
    }
    RUNNING_NONE
}

/// `lock()` met a holder switched out on this CPU (once per call).
///
/// Counts `lkph` (IRQs masked) or `lkpho` (IRQs on). Then: the holder is the
/// waiter's own thread (`lkself`, and a `kind=self` line); or it is current
/// on another CPU (`lkphrun`, no line); or, with IRQs masked, a `kind=ph`
/// line. A `ph` line says only that the holder is not running now; it is
/// not by itself evidence of a wedge.
#[inline(never)]
fn note_preempted_holder(
    id: LockId,
    me: OwnerStamp,
    my_tid: u32,
    observed: u64,
    holder_tid: u32,
    site: *const Location<'static>,
) {
    let class = id.class.index();
    bump(if me.irqs_on() { Key::Lkpho } else { Key::Lkph }, class);
    let running = holder_running(holder_tid, me.cpu());
    if is_self(holder_tid, my_tid) {
        bump(Key::Lkself, class);
        print_event(
            EventKind::SelfHeld,
            id,
            observed,
            holder_tid,
            my_tid,
            running,
            site,
        );
    } else if usize::from(running) < MAX_CORES {
        bump(Key::Lkphrun, class);
    } else if !me.irqs_on() {
        print_event(
            EventKind::Preempted,
            id,
            observed,
            holder_tid,
            my_tid,
            running,
            site,
        );
    }
}

/// `lock()` met another CPU's holder that its CPU has switched out since
/// (once per call): counts `lkoxp`, and `lkself` with a `kind=self` line if
/// the holder is the waiter's own thread (a holder moved to this CPU).
#[inline(never)]
fn note_other_cpu_switched(
    id: LockId,
    my_tid: u32,
    observed: u64,
    holder_tid: u32,
    site: *const Location<'static>,
) {
    let class = id.class.index();
    bump(Key::Lkoxp, class);
    if is_self(holder_tid, my_tid) {
        bump(Key::Lkself, class);
        let running = holder_running(holder_tid, cpu_here());
        print_event(
            EventKind::SelfHeld,
            id,
            observed,
            holder_tid,
            my_tid,
            running,
            site,
        );
    }
}

/// `lock()` has waited more than [`STUCK_SECS`] (once per call): counts
/// `lkstk` and prints a `kind=stuck` line.
#[cold]
#[inline(never)]
fn note_stuck(
    id: LockId,
    my_tid: u32,
    owner: u64,
    holder_tid: u32,
    site: *const Location<'static>,
) {
    bump(Key::Lkstk, id.class.index());
    let running = holder_running(holder_tid, cpu_here());
    print_event(
        EventKind::Stuck,
        id,
        owner,
        holder_tid,
        my_tid,
        running,
        site,
    );
}

/// Whether more than [`STUCK_SECS`] have passed on the counter since `start`.
#[inline(never)]
fn waited_too_long(start: u64) -> bool {
    let limit = timer::read_cntfrq().saturating_mul(STUCK_SECS);
    timer::read_counter().wrapping_sub(start) > limit
}

// ---------------------------------------------------------------------------
// Event lines and the re-entry panic
// ---------------------------------------------------------------------------

/// The `kind=` of a `[tripwire-ev]` line.
#[derive(Clone, Copy)]
enum EventKind {
    /// A holder switched out on this CPU and not current elsewhere, met with
    /// IRQs masked.
    Preempted,
    /// A wait of more than [`STUCK_SECS`].
    Stuck,
    /// The holder is the waiter's own thread.
    SelfHeld,
}

impl EventKind {
    const fn name(self) -> &'static str {
        match self {
            EventKind::Preempted => "ph",
            EventKind::Stuck => "stuck",
            EventKind::SelfHeld => "self",
        }
    }
}

/// The call site behind a holder-field value, `None` for null.
#[inline(always)]
fn site_ref(site: *const Location<'static>) -> Option<&'static Location<'static>> {
    // SAFETY: HolderFields::site is written only by IrqSpinLock::hold, with
    // `kva_of(Location::caller())` right after the CAS, and by pre_release,
    // with null. A non-null value is therefore the kernel virtual address of
    // a `'static Location` in the kernel image's .rodata, which TTBR1 maps
    // read-only on every CPU for the kernel's lifetime (kmap). A value from
    // anywhere else would make this read fault or print garbage.
    unsafe { site.as_ref() }
}

/// Emit `tid`, or `?` for [`TID_NONE`].
#[inline(always)]
fn put_tid(out: &mut UartSink, tid: u32) {
    if tid == TID_NONE {
        out.put(b'?');
    } else {
        put_dec(out, u64::from(tid));
    }
}

/// Print one `[tripwire-ev]` line straight to the UART, with IRQs masked so
/// that this CPU's own tick cannot split it:
///
/// ```text
/// [tripwire-ev] kind=ph cpu=0 lock=THREAD_TABLE idx=- ctx=irq owner_cpu=0 owner_gen=4711 holder_tid=12 cur_tid=3 holder_running=none holder=kernel/src/cap/mod.rs:39
/// ```
///
/// `idx` is `-` for a scalar static, `owner_*` come from `owner` (the word
/// the waiter saw), `cur_tid` is the waiter's thread, and `holder_running` is
/// a CPU, `none` or `?`. Unknown values print `?`. `putc` only.
#[cold]
#[inline(never)]
fn print_event(
    kind: EventKind,
    id: LockId,
    owner: u64,
    holder_tid: u32,
    cur_tid: u32,
    running: u8,
    site: *const Location<'static>,
) {
    let daif = read_daif();
    // SAFETY: DAIFSet #0x2 sets PSTATE.I at EL1, which only defers IRQs. The
    // asm is a compiler barrier, so the line below is written inside the
    // masked window. The mask is restored below; leaving it set would stall
    // this CPU's timer until the next unmask.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2", options(nostack, preserves_flags)) };
    let cpu = cpu_here();
    let ctx = Ctx::from_raw(irq_ctx(usize::from(cpu)), daif & DAIF_I != 0);
    let owner = OwnerStamp::from_word(owner);
    let out = &mut UartSink;
    out.put_str("[tripwire-ev] kind=");
    out.put_str(kind.name());
    out.put_str(" cpu=");
    put_dec(out, u64::from(cpu));
    out.put_str(" lock=");
    out.put_str(id.class.name());
    out.put_str(" idx=");
    if id.index == SCALAR_INDEX {
        out.put(b'-');
    } else {
        put_dec(out, u64::from(id.index));
    }
    out.put_str(" ctx=");
    out.put_str(ctx.name());
    out.put_str(" owner_cpu=");
    match owner {
        Some(stamp) => put_dec(out, u64::from(stamp.cpu())),
        None => out.put(b'?'),
    }
    out.put_str(" owner_gen=");
    match owner {
        Some(stamp) => put_dec(out, stamp.gen()),
        None => out.put(b'?'),
    }
    out.put_str(" holder_tid=");
    put_tid(out, holder_tid);
    out.put_str(" cur_tid=");
    put_tid(out, cur_tid);
    out.put_str(" holder_running=");
    match running {
        RUNNING_UNKNOWN => out.put(b'?'),
        RUNNING_NONE => out.put_str("none"),
        cpu => put_dec(out, u64::from(cpu)),
    }
    out.put_str(" holder=");
    match site_ref(site) {
        Some(loc) => {
            out.put_str(loc.file());
            out.put(b':');
            put_dec(out, u64::from(loc.line()));
        }
        None => out.put(b'?'),
    }
    out.put(b'\n');
    if daif & DAIF_I == 0 {
        // SAFETY: IRQs were on when this function was entered, so clearing
        // PSTATE.I restores the caller's state. The asm is a compiler
        // barrier, so the line above stays before it. Clearing the mask of a
        // caller that had IRQs masked would break its critical section,
        // which the DAIF.I test above rules out.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2", options(nostack, preserves_flags)) };
    }
}

/// Panic on a re-entry: the word holds the waiter's own stream's stamp, so
/// the wait could never end.
///
/// The message is the one line of `write_reentry_msg`, for example
/// `lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-exit holder=kernel/src/cap/mod.rs:39 holder_irqs=on tid=12 gen=4711`.
/// The panic is located at the caller of `lock()`. Exempt from the IRQ-path
/// rules: it builds the message in a stack buffer and never returns.
#[cold]
#[inline(never)]
#[track_caller]
fn reentry_panic(
    id: LockId,
    me: OwnerStamp,
    observed: u64,
    holder_tid: u32,
    site: *const Location<'static>,
) -> ! {
    let cpu = me.cpu();
    let report = ReentryReport {
        class: id.class,
        index: id.index,
        cpu,
        ctx: Ctx::from_raw(irq_ctx(usize::from(cpu)), !me.irqs_on()),
        holder: site_ref(site).map(|loc| HolderSite {
            file: loc.file(),
            line: loc.line(),
        }),
        // A re-entry verdict implies a held word; `me` is its stamp apart
        // from the IRQS_ON bit.
        owner: OwnerStamp::from_word(observed).unwrap_or(me),
        tid: holder_tid,
    };
    let mut msg = BufSink::<MAX_REENTRY_MSG_LEN>::new();
    write_reentry_msg(&mut msg, &report);
    panic!("{}", msg.as_str());
}

// ---------------------------------------------------------------------------
// Run-time check of rule (b)
// ---------------------------------------------------------------------------

/// Visit the words of the 23 IRQ-class locks: THREAD_TABLE, the 8
/// CURRENT_THREAD and 8 RUN_QUEUES entries, WAKEUP_ERRORS, TIMEOUT_QUEUE,
/// NOTIFY_DEADLINES, NOTIFICATION_TABLE, SELECT_WAITERS and BOOT_LOG.
///
/// The statics are only dereferenced, never compared or published, so this
/// is correct at physical-alias PCs too.
#[inline(always)]
fn for_each_lock_word(mut f: impl FnMut(u64)) {
    f(crate::task::THREAD_TABLE.owner_word());
    for lock in crate::task::CURRENT_THREAD.iter() {
        f(lock.owner_word());
    }
    crate::sched::irq_lock_words(&mut f);
    crate::ipc::irq_lock_words(&mut f);
    crate::observability::irq_lock_words(&mut f);
}

/// Whether the stream now running on `cpu` holds an IRQ-class lock: a word
/// stamped with `cpu` and `SWITCH_GEN[cpu]`.
///
/// Meaningful for the caller's own CPU with IRQs masked, where no dispatch
/// can move the generation during the walk. A generation never recurs, so a
/// stale stamp never matches.
#[inline(never)]
pub fn held_by_stream(cpu: u8) -> bool {
    let gen = switch_gen(cpu);
    let mut held = false;
    for_each_lock_word(|word| {
        if let Some(stamp) = OwnerStamp::from_word(word) {
            if stamp.cpu() == cpu && stamp.gen_matches(gen) {
                held = true;
            }
        }
    });
    held
}

/// Called right before each of the 5 `restore_context` calls, with IRQs
/// masked: counts `rsthold` if the dispatching stream still holds a lock it
/// took after its dispatch bumped this CPU's generation, which breaks rule
/// (b). The resumed thread would inherit that stamp as its own. Expected 0.
/// A counter, not an assertion, so it adds no panic.
#[inline(never)]
pub fn note_restore() {
    if held_by_stream(cpu_here()) {
        bump_masked(Key::Rsthold, 0);
    }
}
