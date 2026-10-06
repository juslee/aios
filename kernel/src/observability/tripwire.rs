//! Tripwire runtime: per-CPU counters and the `[tripwire]` line (crash-fix
//! step 1b).
//!
//! `shared::tripwire` holds the pure parts: the key catalogue, the counter
//! rows and the line writer. This module owns the kernel's one counter table
//! and prints lines to the PL011 UART:
//!
//! - [`bump_masked`] and [`bump`] add 1 to a counter in the running CPU's row.
//! - [`print_line`] writes one `[tripwire]` line and records its own cost in
//!   `twc`, `twn` and `twmax`.
//! - The timer tick calls [`note_heartbeat`] when CPU 0 prints a heartbeat,
//!   and [`end_of_tick`] as CPU 0's last step. The Gate 1 bench calls
//!   [`set_console_busy`] and [`request_g1_line`].
//! - Dispatch bookkeeping: [`note_dispatch`] at the four `CURRENT_THREAD`
//!   commit sites keeps the per-CPU switch generation, the lock-free
//!   current-thread mirror and the per-thread last-CPU and last-run stamps.
//!   `irq_handler_el1` and `schedule()` keep the per-CPU IRQ context label
//!   ([`irq_enter`], [`irq_preempt_check`], [`irq_leave`], [`irq_ctx`],
//!   [`set_irq_ctx`]).
//! - Wake attribution: `unblock` reports each wake by source
//!   ([`note_unblock`], [`note_unblock_target`]). The IPC wait paths store
//!   their call and receive phases ([`wait_published`], [`wait_armed`],
//!   [`wait_ended`]), and the reply, send and call wakers classify a skipped
//!   or misdirected wake against them ([`note_reply_wake`],
//!   [`note_send_wake`], [`note_reply_switch`]): the N2 counters. Wakers mark
//!   a wake in flight ([`mark_wake_pending`]) when they take a thread's last
//!   waiter reference.
//! - Restore-site checks (N5): [`check_restore`] counts a saved PC outside
//!   the kernel text (`pcnull`, `pcphys`, `pcother`) or a saved SP outside
//!   the thread's stack (`spbad`) before each `restore_context`, against the
//!   text bounds `kernel_main` captured ([`capture_text_layout`]).
//! - CPU ids: [`cpu_here`] reads MPIDR_EL1 Aff0, and [`cpu_tpidr`] the copy
//!   boot.S puts in TPIDR_EL1, which the IRQ-class lock stamps with.
//!   [`check_tpidr`] and [`note_dispatch`] count a mismatch (`tpidrbad`).
//!
//! # When lines print
//!
//! - `src=hb`, [`LineMode::NonZero`]: once per `[heartbeat] tick=N` line.
//! - `src=g1`, [`LineMode::Full`]: once, after the bench prints
//!   `=== Gate 1 Complete ===`.
//!
//! Both print at the end of CPU 0's timer tick, after the tick's own work
//! (time slice, IPC timeouts, load balance, `NEED_RESCHED`), so that work
//! keeps its timing. A line waits while a thread holds the console
//! ([`set_console_busy`]): the UART has no lock, and two writers would
//! interleave byte by byte. It also waits while CPU 0's interrupted stream
//! holds an IRQ-class lock (`sync::held_by_stream`), so that the print does
//! not stretch that hold. After [`MAX_DEFER_TICKS`] ticks of waiting the
//! line prints anyway and `hbdefer` counts it. That cap is far below the 1000
//! ticks between heartbeats, so every heartbeat gets its line before the next
//! heartbeat. At most one line prints per tick; the heartbeat's goes first.
//!
//! # IRQ-path rules
//!
//! [`bump_masked`], [`end_of_tick`] and [`print_line`] run in the timer IRQ,
//! whose entry saves no V registers. They use no `core::fmt`, no locks, no
//! `klog!`, no stack arrays and no copies of aggregates of 16 bytes or more,
//! and they are `#[inline(never)]`, so a disassembly attributes their
//! instructions to them. The dispatch and IRQ-context functions follow the
//! same rules: `schedule()` and `irq_handler_el1` call them in the timer IRQ.
//! So do [`note_unblock`], [`note_unblock_target`], [`mark_wake_pending`] and
//! [`clear_wake_pending`], which the timeout scans reach from the timer IRQ,
//! and [`check_restore`], which `schedule()` calls.
//! The counters use `Relaxed` load and store only: each CPU writes only its
//! own row, with IRQs masked, so no atomic read-modify-write is needed. The
//! flags, the per-CPU dispatch state and the per-thread stamps are plain
//! loads and stores too.

#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use shared::lock::TID_NONE;
use shared::tripwire::{
    self, ClearResult, CpuCounters, Key, LineMode, LineSrc, Sink, TextLayout, UnblockKind,
    UnblockOutcome, WakeSource, IRQ_CTX_EXIT, IRQ_CTX_IRQ, IRQ_CTX_THREAD, PHASE_ARMED, PHASE_IDLE,
    PHASE_PUBLISHED,
};

use crate::arch::aarch64::{mmu, timer, uart};
use crate::sched::STACK_SIZE;
use crate::smp::{self, MAX_CORES};
use crate::task::{ThreadContext, ThreadId, ThreadState, MAX_THREADS, NEW_KERNEL_SP_OFFSET};

const _: () = assert!(tripwire::MAX_CPUS == MAX_CORES);
const _: () = assert!(tripwire::MASK_TIDS as usize == MAX_THREADS);

/// DAIF.I, the IRQ mask bit.
pub(crate) const DAIF_I: u64 = 1 << 7;

/// Masks a CPU id into `0..MAX_CORES` ([`switch_gen_masked`]).
const CPU_INDEX_MASK: usize = MAX_CORES - 1;
const _: () = assert!(MAX_CORES.is_power_of_two());

/// Ticks CPU 0 holds a pending line back while it must wait (the console is
/// busy, or CPU 0's interrupted stream holds an IRQ-class lock).
pub const MAX_DEFER_TICKS: u64 = 256;

/// The tripwire counters: one row per CPU.
static COUNTERS: CpuCounters<MAX_CORES> = CpuCounters::new();

/// A heartbeat was printed and its `src=hb` line is still due. Written and
/// read by CPU 0's timer tick only.
static HB_PENDING: AtomicBool = AtomicBool::new(false);

/// The bench finished and the `src=g1` line is still due. Set by the bench
/// thread on any CPU, cleared by CPU 0's timer tick.
static G1_PENDING: AtomicBool = AtomicBool::new(false);

/// A thread is printing straight to the UART, and CPU 0 holds its lines back.
static CONSOLE_BUSY: AtomicBool = AtomicBool::new(false);

/// Consecutive ticks CPU 0 has held a pending line back. CPU 0 only.
static DEFERRED_TICKS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Counters
// ---------------------------------------------------------------------------

/// This CPU's id, MPIDR_EL1 Aff0.
///
/// The `asm!` has none of the `pure`, `nomem` or `readonly` options, so every
/// call reads the register afresh and is a compiler barrier: the read stays
/// after an earlier `msr DAIFSet`, and the id cannot come from before an IRQ
/// mask that pins the thread to this CPU. The IRQ-class lock's stamps read
/// the same id from TPIDR_EL1 instead ([`cpu_tpidr`]), which is cheaper under
/// QEMU TCG.
#[inline(always)]
pub(crate) fn cpu_here() -> u8 {
    let mpidr: u64;
    // SAFETY: MPIDR_EL1 is readable at EL1 and reading it has no side
    // effects. All kernel code runs at EL1, which the boot path establishes;
    // at EL0 the read would trap as an undefined instruction and the
    // exception handler would report it.
    unsafe {
        core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr, options(nostack, preserves_flags))
    };
    (mpidr & 0xFF) as u8
}

/// TPIDR_EL1 as it is now.
///
/// boot.S writes it once on every CPU, before any Rust code (`_start` and
/// `_secondary_entry`), with that CPU's MPIDR_EL1 Aff0, and nothing else
/// writes it: the context switch neither saves nor restores it, so it stays
/// with the CPU and a thread that moves reads its new CPU's value. The `asm!`
/// has none of `pure`, `nomem` or `readonly`, as in [`cpu_here`].
#[inline(always)]
fn read_tpidr_el1() -> u64 {
    let tpidr: u64;
    // SAFETY: TPIDR_EL1 is readable at EL1 and reading it has no side
    // effects. All kernel code runs at EL1, which the boot path establishes;
    // at EL0 the read would trap as an undefined instruction and the
    // exception handler would report it.
    unsafe {
        core::arch::asm!("mrs {}, TPIDR_EL1", out(reg) tpidr, options(nostack, preserves_flags))
    };
    tpidr
}

/// This CPU's id as boot.S stored it in TPIDR_EL1: MPIDR_EL1 Aff0.
///
/// The CPU id the IRQ-class lock stamps with (`sync::irq_spin_lock`). It has
/// [`cpu_here`]'s contract, a fresh read and a compiler barrier on every call,
/// so it is also the `cpu()` that `shared::lock::CpuView` requires. Under QEMU
/// TCG a TPIDR_EL1 read is one inline load from the CPU state, where an
/// MPIDR_EL1 read is two helper calls; a lock acquisition reads the CPU id
/// several times. [`check_tpidr`] and [`note_dispatch`] count a value that
/// is not this CPU's MPIDR Aff0 (`tpidrbad`), which would make stamps name
/// the wrong CPU.
#[inline(always)]
pub(crate) fn cpu_tpidr() -> u8 {
    (read_tpidr_el1() & 0xFF) as u8
}

/// Count `tpidrbad` if TPIDR_EL1 is not this CPU's MPIDR_EL1 Aff0 (the whole
/// register is compared, so any other writer shows). Count only: it never
/// panics and changes nothing. `kernel_main` and `secondary_main` call it
/// first, before their first IRQ-class lock; [`note_dispatch`] repeats the
/// check at every dispatch. Expected 0.
#[inline(never)]
pub fn check_tpidr() {
    if read_tpidr_el1() != u64::from(cpu_here()) {
        bump(Key::Tpidrbad, 0);
    }
}

/// DAIF as it is now.
#[inline(always)]
pub(crate) fn read_daif() -> u64 {
    let daif: u64;
    // SAFETY: DAIF is readable at EL1 and reading it has no side effects.
    // The asm has no memory operand. A read at the wrong point would only
    // mislabel the mask state; it cannot corrupt anything.
    unsafe { core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nostack, preserves_flags)) };
    daif
}

/// Add `n` to value `idx` of `key` in `cpu`'s row. The caller runs on `cpu`
/// with IRQs masked. An `idx` beyond the key's width is ignored.
#[inline(always)]
fn add_row(cpu: u8, key: Key, idx: usize, n: u64) {
    if let Some(slot) = key.slot(idx) {
        COUNTERS.add(usize::from(cpu), slot, n);
    }
}

/// Add 1 to value `idx` of `key` in this CPU's row, at a site that always
/// runs with IRQs masked (IRQ context, or under `msr DAIFSet`).
///
/// A per-CPU key takes `idx` 0. A call with IRQs on could lose a count to a
/// preempting update of the same slot, so a debug build asserts DAIF.I.
#[inline(never)]
pub fn bump_masked(key: Key, idx: usize) {
    debug_assert!(read_daif() & DAIF_I != 0);
    add_row(cpu_here(), key, idx, 1);
}

/// Add 1 to value `idx` of `key` in this CPU's row, at a site that may run
/// with IRQs on.
///
/// Masks IRQs for the update and restores the caller's mask state after it,
/// so the update cannot be interrupted or moved to another CPU half done.
#[inline(never)]
pub fn bump(key: Key, idx: usize) {
    let daif = read_daif();
    // SAFETY: DAIFSet #0x2 sets PSTATE.I at EL1, which only defers IRQs. The
    // asm is a compiler barrier, so the counter update below stays inside
    // the masked window. The mask is restored below; leaving it set would
    // stall this CPU's timer until the next unmask.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2", options(nostack, preserves_flags)) };
    add_row(cpu_here(), key, idx, 1);
    if daif & DAIF_I == 0 {
        // SAFETY: IRQs were on when this function was entered, so clearing
        // PSTATE.I restores the caller's state. The asm is a compiler
        // barrier, so the update above stays before it. Clearing the mask of
        // a caller that had IRQs masked would break its critical section,
        // which the DAIF.I test above rules out.
        unsafe { core::arch::asm!("msr DAIFClr, #0x2", options(nostack, preserves_flags)) };
    }
}

// ---------------------------------------------------------------------------
// The line
// ---------------------------------------------------------------------------

/// The PL011 UART as a tripwire sink: `putc` per byte, `\n` sent as `\r\n`.
/// The lock's `[tripwire-ev]` lines use it too.
pub(crate) struct UartSink;

impl Sink for UartSink {
    #[inline]
    fn put(&mut self, byte: u8) {
        if byte == b'\n' {
            uart::putc(b'\r');
        }
        uart::putc(byte);
    }
}

/// Print one `[tripwire]` line from this CPU's view of the counters.
///
/// `t` is `TICK_COUNT` and `ncpu` the online CPU count. The line's cost in
/// CNTVCT ticks is added to `twc` and raises `twmax`, and `twn` counts it, in
/// this CPU's row, so the line after it reports it. The caller runs with IRQs
/// masked. Uses `putc` only: no `core::fmt`, no lock and no buffer.
#[inline(never)]
pub fn print_line(src: LineSrc, mode: LineMode) {
    let cpu = cpu_here();
    let start = timer::read_counter();
    tripwire::write_line(
        &mut UartSink,
        src,
        mode,
        cpu,
        timer::TICK_COUNT.load(Ordering::Relaxed),
        smp::online_cpus(),
        |key, idx| COUNTERS.value(key, idx),
    );
    let spent = timer::read_counter().wrapping_sub(start);
    add_row(cpu, Key::Twc, 0, spent);
    add_row(cpu, Key::Twn, 0, 1);
    if let Some(slot) = Key::Twmax.slot(0) {
        COUNTERS.store_max(usize::from(cpu), slot, spent);
    }
}

// ---------------------------------------------------------------------------
// Scheduling the heartbeat and g1 lines
// ---------------------------------------------------------------------------

/// CPU 0 printed `[heartbeat] tick=N`: its `src=hb` line is due. Called from
/// CPU 0's timer tick.
#[inline]
pub fn note_heartbeat() {
    HB_PENDING.store(true, Ordering::Relaxed);
}

/// The Gate 1 bench finished: the `src=g1` line is due.
pub fn request_g1_line() {
    G1_PENDING.store(true, Ordering::Release);
}

/// Mark the console busy (`true`) while a thread prints a block of lines
/// straight to the UART, or free it (`false`). CPU 0 holds tripwire lines
/// back while it is busy, for at most [`MAX_DEFER_TICKS`] ticks.
pub fn set_console_busy(busy: bool) {
    CONSOLE_BUSY.store(busy, Ordering::Release);
}

/// CPU 0's last step in each timer tick: print a pending line, unless it must
/// wait and has waited fewer than [`MAX_DEFER_TICKS`] ticks. It waits while
/// the console is busy, or while CPU 0's interrupted stream holds one of the
/// IRQ-class locks (`sync::held_by_stream`): the other CPUs may be spinning
/// on that lock, and the print would hold them for its whole length. Prints
/// at most one line per tick, the heartbeat's first. IRQ context, CPU 0 only.
#[inline(never)]
pub fn end_of_tick() {
    let hb = HB_PENDING.load(Ordering::Relaxed);
    if !hb && !G1_PENDING.load(Ordering::Acquire) {
        return;
    }
    if CONSOLE_BUSY.load(Ordering::Acquire) || crate::sync::held_by_stream(0) {
        let deferred = DEFERRED_TICKS.load(Ordering::Relaxed);
        if deferred < MAX_DEFER_TICKS {
            DEFERRED_TICKS.store(deferred.wrapping_add(1), Ordering::Relaxed);
            return;
        }
        bump_masked(Key::Hbdefer, 0);
    }
    DEFERRED_TICKS.store(0, Ordering::Relaxed);
    if hb {
        HB_PENDING.store(false, Ordering::Relaxed);
        print_line(LineSrc::Hb, LineMode::NonZero);
    } else {
        G1_PENDING.store(false, Ordering::Relaxed);
        print_line(LineSrc::G1, LineMode::Full);
    }
}

// ---------------------------------------------------------------------------
// Dispatch bookkeeping
// ---------------------------------------------------------------------------

/// `LAST_CPU[tid]` of a thread not dispatched since its slot was allocated.
pub const LAST_CPU_NEVER: u8 = 0xFF;

/// Per-CPU switch generation: `SWITCH_GEN[c]` counts the dispatches CPU c has
/// committed ([`note_dispatch`]). A generation `(c, g)` names one stream: the
/// thread dispatched on c in generation g, with the IRQ handlers nested on
/// its stack. Only CPU c writes its entry, with IRQs masked, by load and
/// store. The detect-only lock stamps its word with it (`shared::lock`).
static SWITCH_GEN: [AtomicU64; MAX_CORES] = [const { AtomicU64::new(0) }; MAX_CORES];

/// Lock-free mirror of `CURRENT_THREAD`: `CURRENT_TID[k]` is the slot of CPU
/// k's current thread, or [`TID_NONE`] before CPU k's first dispatch. Written
/// only by [`note_dispatch`], under THREAD_TABLE.
static CURRENT_TID: [AtomicU32; MAX_CORES] = [const { AtomicU32::new(TID_NONE) }; MAX_CORES];

/// Per-CPU context label: [`IRQ_CTX_THREAD`], [`IRQ_CTX_IRQ`] (inside
/// `irq_handler_el1`) or [`IRQ_CTX_EXIT`] (the handler's preemption check,
/// which may switch threads). Written only on the CPU it describes, with IRQs
/// masked.
static IRQ_CTX: [AtomicU8; MAX_CORES] = [const { AtomicU8::new(IRQ_CTX_THREAD) }; MAX_CORES];

/// Per thread slot: the CPU (MPIDR Aff0) that last dispatched the thread, or
/// [`LAST_CPU_NEVER`].
static LAST_CPU: [AtomicU8; MAX_THREADS] = [const { AtomicU8::new(LAST_CPU_NEVER) }; MAX_THREADS];

/// Per thread slot: `TICK_COUNT` when the thread was last dispatched or
/// picked again by `schedule()`, or when its slot was allocated.
static LAST_RUN: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];

/// Per thread slot: a waker has taken the thread's last waiter reference and
/// has not yet run `unblock` (the source's `WakeSource::marker`), or 0.
/// Set by [`mark_wake_pending`]; cleared by `unblock` ([`note_unblock`]), by
/// a dispatch of the thread, and where a marked wake is abandoned
/// ([`clear_wake_pending`]).
static WAKE_PENDING: [AtomicU8; MAX_THREADS] = [const { AtomicU8::new(0) }; MAX_THREADS];

/// Per thread slot: `TICK_COUNT` when `WAKE_PENDING` was last set.
static WAKE_AT: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];

/// Where a dispatch commits its new current thread.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DispatchSite {
    /// `enter_scheduler`: a CPU's first dispatch.
    Enter,
    /// `schedule()`.
    Schedule,
    /// `try_direct_switch`: a caller hands its CPU to the waiting receiver.
    Direct,
    /// `try_reply_switch`: a replier hands its CPU back to the caller.
    Reply,
}

/// `SWITCH_GEN[cpu]`, loaded afresh (`Relaxed`) on every call, as
/// `shared::lock::CpuView::switch_gen` requires. 0 for an out-of-range `cpu`.
#[inline(always)]
pub(crate) fn switch_gen(cpu: u8) -> u64 {
    SWITCH_GEN
        .get(usize::from(cpu))
        .map_or(0, |gen| gen.load(Ordering::Relaxed))
}

/// `SWITCH_GEN[cpu]` indexed modulo [`MAX_CORES`], so that the load needs
/// no bounds branch: for the check the IRQ-class lock makes inside its
/// critical section (`sync::irq_spin_lock`). Every CPU `smp` brings up has
/// an MPIDR Aff0 below `MAX_CORES`, and for those it equals [`switch_gen`].
#[inline(always)]
pub(crate) fn switch_gen_masked(cpu: u8) -> u64 {
    SWITCH_GEN
        .get(usize::from(cpu) & CPU_INDEX_MASK)
        .map_or(0, |gen| gen.load(Ordering::Relaxed))
}

/// `CURRENT_TID[cpu]`: the slot of CPU `cpu`'s current thread, or
/// [`TID_NONE`] before its first dispatch (and for an out-of-range `cpu`).
#[inline(always)]
pub(crate) fn current_tid(cpu: u8) -> u32 {
    CURRENT_TID
        .get(usize::from(cpu))
        .map_or(TID_NONE, |tid| tid.load(Ordering::Relaxed))
}

/// Store `ctx` in `IRQ_CTX[cpu]`. An out-of-range `cpu` is ignored.
#[inline(always)]
fn store_ctx(cpu: usize, ctx: u8) {
    if let Some(slot) = IRQ_CTX.get(cpu) {
        slot.store(ctx, Ordering::Relaxed);
    }
}

/// Record a dispatch: the caller has just set `CURRENT_THREAD[cpu]` to `tid`.
///
/// Called at the four `CURRENT_THREAD` commit sites (`enter_scheduler`,
/// `schedule()`, `try_direct_switch` and `try_reply_switch`), right after the
/// write, with THREAD_TABLE held and IRQs masked. In order, it:
///
/// 1. counts `n4` if this CPU (MPIDR) is not `cpu`, the index the caller
///    used for `CURRENT_THREAD` (the switch functions read it before they
///    mask IRQs, N4), and `tpidrbad` if TPIDR_EL1 is not this CPU's MPIDR
///    Aff0 ([`check_tpidr`]);
/// 2. bumps this CPU's `SWITCH_GEN`, which ends the generation of the stream
///    that was running here;
/// 3. stores `CURRENT_TID[cpu] = tid`, at the caller's index;
/// 4. labels this CPU thread context (`IRQ_CTX`): the dispatched thread runs
///    at thread level, or resumes where it was switched out and restores its
///    own label there;
/// 5. stamps `LAST_CPU[tid]` with this CPU and `LAST_RUN[tid]` with
///    `TICK_COUNT`, and at the direct and reply sites counts `xdir` or
///    `xrep` if the thread last ran on another CPU, or `xnever` if it never
///    ran;
/// 6. clears `WAKE_PENDING[tid]`: any wake in flight has landed.
///
/// Returns the old `LAST_CPU[tid]`, [`LAST_CPU_NEVER`] for a thread's first
/// dispatch. An out-of-range `tid` counts `badtid` and skips step 5 and 6.
///
/// # The `CURRENT_TID` invariant
///
/// `CURRENT_TID` is written only here, and every caller holds THREAD_TABLE.
/// So while THREAD_TABLE is held, `CURRENT_TID[k]` equals
/// `*CURRENT_THREAD[k]` for every CPU k (`TID_NONE` for `None`), and a reader
/// that holds THREAD_TABLE may read it instead of locking
/// `CURRENT_THREAD[k]`. A reader without THREAD_TABLE sees a recent value.
///
/// # The switch generation
///
/// Every `restore_context` follows one of these calls on the same CPU with
/// IRQs masked, so a stream that leaves a CPU for any reason leaves that
/// CPU's `SWITCH_GEN` changed behind it (rule (a) of `shared::lock`).
#[inline(never)]
pub fn note_dispatch(cpu: usize, tid: ThreadId, site: DispatchSite) -> u8 {
    debug_assert!(read_daif() & DAIF_I != 0);
    let here = cpu_here();
    let row = usize::from(here);
    if row != cpu {
        add_row(here, Key::N4, 0, 1);
    }
    if read_tpidr_el1() != u64::from(here) {
        add_row(here, Key::Tpidrbad, 0, 1);
    }
    if let Some(gen) = SWITCH_GEN.get(row) {
        gen.store(
            gen.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
    }
    if let Some(current) = CURRENT_TID.get(cpu) {
        current.store(tid.0, Ordering::Relaxed);
    }
    store_ctx(row, IRQ_CTX_THREAD);

    let slot = tid.0 as usize;
    let Some(last_cpu) = LAST_CPU.get(slot) else {
        add_row(here, Key::Badtid, 0, 1);
        return LAST_CPU_NEVER;
    };
    let old = last_cpu.load(Ordering::Relaxed);
    last_cpu.store(here, Ordering::Relaxed);
    if let Some(last_run) = LAST_RUN.get(slot) {
        last_run.store(timer::TICK_COUNT.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    let cross = match site {
        DispatchSite::Direct => Some(Key::Xdir),
        DispatchSite::Reply => Some(Key::Xrep),
        DispatchSite::Enter | DispatchSite::Schedule => None,
    };
    if let Some(key) = cross {
        if old == LAST_CPU_NEVER {
            add_row(here, Key::Xnever, 0, 1);
        } else if old != here {
            add_row(here, key, 0, 1);
        }
    }
    if let Some(pending) = WAKE_PENDING.get(slot) {
        pending.store(0, Ordering::Relaxed);
    }
    old
}

/// `schedule()` picked its current thread again, so no dispatch happens:
/// stamp the thread's `LAST_RUN` with `TICK_COUNT`. THREAD_TABLE held, IRQs
/// masked.
#[inline(never)]
pub fn note_repick(tid: ThreadId) {
    if let Some(last_run) = LAST_RUN.get(tid.0 as usize) {
        last_run.store(timer::TICK_COUNT.load(Ordering::Relaxed), Ordering::Relaxed);
    }
}

/// Reset the per-thread stamps of thread slot `slot` for a new thread:
/// `LAST_CPU` never, `LAST_RUN` now, no wake in flight, no call or receive
/// phase. `allocate_thread`
/// calls it with THREAD_TABLE held, before it fills the slot.
pub fn reset_thread_stamps(slot: usize) {
    if let Some(last_cpu) = LAST_CPU.get(slot) {
        last_cpu.store(LAST_CPU_NEVER, Ordering::Relaxed);
    }
    if let Some(last_run) = LAST_RUN.get(slot) {
        last_run.store(timer::TICK_COUNT.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    if let Some(pending) = WAKE_PENDING.get(slot) {
        pending.store(0, Ordering::Relaxed);
    }
    if let Some(wake_at) = WAKE_AT.get(slot) {
        wake_at.store(0, Ordering::Relaxed);
    }
    for side in [WaitSide::Call, WaitSide::Recv] {
        let (phases, chans) = wait_slots(side);
        if let Some(phase) = phases.get(slot) {
            phase.store(PHASE_IDLE, Ordering::Relaxed);
        }
        if let Some(chan) = chans.get(slot) {
            chan.store(0, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Restore-site checks (N5)
// ---------------------------------------------------------------------------

/// `__text_start` as a kernel virtual address, or 0 before
/// [`capture_text_layout`].
static TEXT_LO: AtomicU64 = AtomicU64::new(0);

/// `__text_end` (exclusive) as a kernel virtual address, or 0 before
/// [`capture_text_layout`].
static TEXT_HI: AtomicU64 = AtomicU64::new(0);

// Linker-defined bounds of the kernel text (linker.ld). Only their addresses
// are taken, never their contents.
extern "C" {
    static __text_start: u8;
    static __text_end: u8;
}

/// Capture the kernel text bounds for [`check_restore`], as virtual
/// addresses.
///
/// `kernel_main` calls it on CPU 0 at thread level, before `smp` starts the
/// secondaries, and it runs at its link-time virtual address there, so the
/// `adrp`-based symbol addresses are VAs (as in `kmap`). The restore sites
/// must not compute them: on CPUs 1-3 the IRQ path, and with it
/// `schedule()`, runs at physical-alias PCs, where the same symbols give
/// physical addresses (Design §2.11). The secondaries are started after this
/// store, so their loads see it.
pub fn capture_text_layout() {
    let lo = (&raw const __text_start).addr() as u64;
    let hi = (&raw const __text_end).addr() as u64;
    TEXT_LO.store(lo, Ordering::Relaxed);
    TEXT_HI.store(hi, Ordering::Relaxed);
}

/// Check the context `restore_context` is about to load (N5), count only.
///
/// Called right before each of the 5 `restore_context` calls (after the
/// dispatch's THREAD_TABLE guard is dropped), with IRQs masked:
///
/// - the saved PC: `pcnull` for 0, `pcphys` for the physical alias of the
///   kernel text, `pcother` for anything else outside the text or
///   misaligned (`shared::tripwire::classify_pc`, against the bounds
///   [`capture_text_layout`] took);
/// - the saved SP: `spbad` unless it lies in
///   `[phys_to_virt(stack_phys), phys_to_virt(stack_phys) + STACK_SIZE]`.
///   A context dispatched for the first time (`old_last_cpu` is
///   [`LAST_CPU_NEVER`], the value [`note_dispatch`] returned) is exempt
///   while its SP is still [`Thread::new_kernel`]'s physical default,
///   `stack_phys + NEW_KERNEL_SP_OFFSET`.
///
/// It never panics and changes nothing: the restore goes ahead whatever it
/// finds. All arithmetic wraps, so a corrupt `stack_phys` cannot overflow.
///
/// [`Thread::new_kernel`]: crate::task::Thread::new_kernel
#[inline(never)]
pub fn check_restore(ctx: *const ThreadContext, stack_phys: usize, old_last_cpu: u8) {
    debug_assert!(read_daif() & DAIF_I != 0);
    // SAFETY: every caller passes a pointer to the `context` of a thread slot
    // in THREAD_TABLE, which is a static, so it is valid and aligned. The
    // thread was just made Running and current on this CPU, so no other
    // stream writes its context until restore_context has loaded it; the
    // caller reads the same fields next (assert_valid_ctx, restore_context).
    // A dangling pointer would be read here instead of in restore_context,
    // which would fault the same way an instant later.
    let (pc, sp) = unsafe { ((*ctx).pc, (*ctx).sp) };
    let here = cpu_here();

    let text = TextLayout {
        lo: TEXT_LO.load(Ordering::Relaxed),
        hi: TEXT_HI.load(Ordering::Relaxed),
    };
    if let Some(key) = tripwire::classify_pc(pc, text, mmu::VIRT_PHYS_OFFSET).key() {
        add_row(here, key, 0, 1);
    }

    let phys = stack_phys as u64;
    let base = phys.wrapping_add(mmu::DIRECT_MAP_BASE as u64);
    let top = base.wrapping_add(STACK_SIZE as u64);
    let first_default =
        old_last_cpu == LAST_CPU_NEVER && sp == phys.wrapping_add(NEW_KERNEL_SP_OFFSET as u64);
    if !first_default && !(base..=top).contains(&sp) {
        add_row(here, Key::Spbad, 0, 1);
    }
}

// ---------------------------------------------------------------------------
// Wake attribution and the N2 phases
// ---------------------------------------------------------------------------

/// Per thread slot: where the thread is in an `ipc_call` (`CALL_PHASE`):
/// [`PHASE_IDLE`], or a [`tripwire::wait_phase`] value. Only the thread
/// itself writes its slot.
static CALL_PHASE: [AtomicU8; MAX_THREADS] = [const { AtomicU8::new(PHASE_IDLE) }; MAX_THREADS];

/// Per thread slot: the channel of the thread's current or last `ipc_call`.
static CALL_CHAN: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];

/// Per thread slot: where the thread is in an `ipc_recv` (`RECV_PHASE`), as
/// [`CALL_PHASE`].
static RECV_PHASE: [AtomicU8; MAX_THREADS] = [const { AtomicU8::new(PHASE_IDLE) }; MAX_THREADS];

/// Per thread slot: the channel of the thread's current or last `ipc_recv`.
static RECV_CHAN: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];

/// Which wait a phase store describes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WaitSide {
    /// `ipc_call` waiting for its reply (`CALL_PHASE`, `CALL_CHAN`).
    Call,
    /// `ipc_recv` waiting for a message (`RECV_PHASE`, `RECV_CHAN`).
    Recv,
}

/// The phase and channel arrays of `side`.
#[inline(always)]
fn wait_slots(
    side: WaitSide,
) -> (
    &'static [AtomicU8; MAX_THREADS],
    &'static [AtomicU64; MAX_THREADS],
) {
    match side {
        WaitSide::Call => (&CALL_PHASE, &CALL_CHAN),
        WaitSide::Recv => (&RECV_PHASE, &RECV_CHAN),
    }
}

/// The calling thread `tid` has just published itself as the channel's
/// pending caller or waiting receiver: store the channel, then
/// [`PHASE_PUBLISHED`] (flagged untimed if `timed` is false). Thread context;
/// the thread writes only its own slot. An out-of-range `tid` counts
/// `badtid`.
#[inline(never)]
pub fn wait_published(side: WaitSide, tid: ThreadId, channel: u64, timed: bool) {
    let (phases, chans) = wait_slots(side);
    let slot = tid.0 as usize;
    match (phases.get(slot), chans.get(slot)) {
        (Some(phase), Some(chan)) => {
            chan.store(channel, Ordering::Relaxed);
            phase.store(
                tripwire::wait_phase(PHASE_PUBLISHED, timed),
                Ordering::Release,
            );
        }
        _ => bump(Key::Badtid, 0),
    }
}

/// The calling thread `tid` has registered its timeout: store
/// [`PHASE_ARMED`]. A timed wait calls it inside the TIMEOUT_QUEUE critical
/// section that registers the entry, so a waker whose `clear_timeout` took
/// the lock later sees it; an untimed wait calls it at the point where it
/// would register. Thread context.
#[inline(never)]
pub fn wait_armed(side: WaitSide, tid: ThreadId, timed: bool) {
    let (phases, _) = wait_slots(side);
    match phases.get(tid.0 as usize) {
        Some(phase) => phase.store(tripwire::wait_phase(PHASE_ARMED, timed), Ordering::Release),
        None => bump(Key::Badtid, 0),
    }
}

/// The calling thread `tid` has woken from its wait: store [`PHASE_IDLE`].
/// Thread context.
#[inline(never)]
pub fn wait_ended(side: WaitSide, tid: ThreadId) {
    let (phases, _) = wait_slots(side);
    match phases.get(tid.0 as usize) {
        Some(phase) => phase.store(PHASE_IDLE, Ordering::Release),
        None => bump(Key::Badtid, 0),
    }
}

/// `tid`'s phase and channel on `side`: the phase loaded first (`Acquire`),
/// then the channel. `(PHASE_IDLE, 0)` for an out-of-range `tid`.
#[inline(always)]
fn wait_snapshot(side: WaitSide, tid: ThreadId) -> (u8, u64) {
    let (phases, chans) = wait_slots(side);
    let slot = tid.0 as usize;
    match (phases.get(slot), chans.get(slot)) {
        (Some(phase), Some(chan)) => (phase.load(Ordering::Acquire), chan.load(Ordering::Relaxed)),
        _ => (PHASE_IDLE, 0),
    }
}

/// A waker has taken `tid`'s last waiter reference (a channel's pending
/// caller or waiting receiver, a timeout entry, a notification waiter slot
/// or deadline, a process-exit wakeup) and has not yet run `unblock`: store
/// `WAKE_AT[tid] = TICK_COUNT`, then `WAKE_PENDING[tid] = src.marker()`.
/// `unblock` and [`note_dispatch`] clear it. Plain stores; any context. An
/// out-of-range `tid` counts `badtid`.
#[inline(never)]
pub fn mark_wake_pending(tid: ThreadId, src: WakeSource) {
    let slot = tid.0 as usize;
    match (WAKE_PENDING.get(slot), WAKE_AT.get(slot)) {
        (Some(pending), Some(wake_at)) => {
            wake_at.store(timer::TICK_COUNT.load(Ordering::Relaxed), Ordering::Relaxed);
            pending.store(src.marker(), Ordering::Release);
        }
        _ => bump(Key::Badtid, 0),
    }
}

/// The wake marked by [`mark_wake_pending`] was abandoned without an
/// `unblock` (the notification deadline scan gave it up): clear
/// `WAKE_PENDING[tid]`, so that the thread does not look as if a wake were
/// in flight forever. Any context.
#[inline(never)]
pub fn clear_wake_pending(tid: ThreadId) {
    if let Some(pending) = WAKE_PENDING.get(tid.0 as usize) {
        pending.store(0, Ordering::Release);
    }
}

/// `unblock`'s bookkeeping, called with THREAD_TABLE held and IRQs masked,
/// after it has read the target's slot and before it acts on it.
///
/// `state` is the target's state (`None` for an empty slot or an
/// out-of-range `tid`). The function:
///
/// 1. decides the outcome kind the way `unblock` does
///    ([`UnblockKind::of`]) and counts `ubrun`, `ubrbl`, `ubdead` or
///    `ubnone` for `src`; an out-of-range `tid` also counts `badtid`;
/// 2. for [`WakeSource::Reply`] snapshots the target's `CALL_PHASE` and
///    `CALL_CHAN`, and for [`WakeSource::Call`] and [`WakeSource::Send`] its
///    `RECV_PHASE` and `RECV_CHAN`, under the same lock hold that decided the
///    outcome;
/// 3. clears `WAKE_PENDING[tid]`: the wake has landed, whatever it did.
///
/// Returns the outcome, which the reply, send and call wakers classify
/// ([`note_reply_wake`], [`note_send_wake`]).
#[inline(never)]
pub fn note_unblock(tid: ThreadId, src: WakeSource, state: Option<&ThreadState>) -> UnblockOutcome {
    debug_assert!(read_daif() & DAIF_I != 0);
    let here = cpu_here();
    let kind = UnblockKind::of(state);
    if let Some(key) = kind.key() {
        add_row(here, key, src.index(), 1);
    }
    let (phase, chan) = match src {
        WakeSource::Reply => wait_snapshot(WaitSide::Call, tid),
        WakeSource::Call | WakeSource::Send => wait_snapshot(WaitSide::Recv, tid),
        _ => (PHASE_IDLE, 0),
    };
    match WAKE_PENDING.get(tid.0 as usize) {
        Some(pending) => pending.store(0, Ordering::Relaxed),
        None => add_row(here, Key::Badtid, 0, 1),
    }
    UnblockOutcome { kind, phase, chan }
}

/// `unblock` queues `tid` on CPU `target`: count `ubmove` for `src` if the
/// thread last ran on another CPU (a thread never dispatched does not
/// count). IRQs masked.
#[inline(never)]
pub fn note_unblock_target(tid: ThreadId, src: WakeSource, target: usize) {
    debug_assert!(read_daif() & DAIF_I != 0);
    if let Some(last_cpu) = LAST_CPU.get(tid.0 as usize) {
        let last = last_cpu.load(Ordering::Relaxed);
        if last != LAST_CPU_NEVER && usize::from(last) != target {
            add_row(cpu_here(), Key::Ubmove, src.index(), 1);
        }
    }
}

/// Count `ctbusy` when a waker's own `clear_timeout` found TIMEOUT_QUEUE
/// busy ([`ClearResult::Busy`]), which leaves the waiter's entry, if any,
/// registered. Called by the reply, send and call wakers and by
/// `wake_with_error` for every source except the timeout itself, whose
/// entry `check_timeouts` has already taken. Thread or IRQ context.
#[inline(never)]
pub fn note_clear(clear: ClearResult) {
    if matches!(clear, ClearResult::Busy) {
        bump(Key::Ctbusy, 0);
    }
}

/// Count the N2-table verdict ([`tripwire::classify_reply`]) on
/// `ipc_reply`'s `unblock` fallback: `outcome` is that `unblock`'s result,
/// `channel` the reply's channel and `clear` the reply's own
/// `clear_timeout` result for the caller. Thread context.
#[inline(never)]
pub fn note_reply_wake(outcome: UnblockOutcome, channel: u64, clear: ClearResult) {
    if let Some((key, idx)) = tripwire::classify_reply(outcome, channel, clear).key() {
        bump(key, idx);
    }
}

/// Count the N2-table verdict ([`tripwire::classify_send`]) on the
/// `unblock` fallback of `ipc_send` or `ipc_call` waking the waiting
/// receiver: `clear` is the waker's own `clear_timeout` result for the
/// receiver. Thread context.
#[inline(never)]
pub fn note_send_wake(outcome: UnblockOutcome, channel: u64, clear: ClearResult) {
    if let Some((key, idx)) = tripwire::classify_send(outcome, channel, clear).key() {
        bump(key, idx);
    }
}

/// `try_reply_switch` has validated the caller (BlockedIpc) and will switch
/// to it: classify the wake as a reply that woke the caller
/// ([`UnblockKind::Woke`] with the caller's `CALL_PHASE`/`CALL_CHAN`), which
/// counts `misrep` when the caller was not armed in a call on `channel`.
/// THREAD_TABLE held (the caller is blocked, so its phase is stable), IRQs
/// masked.
#[inline(never)]
pub fn note_reply_switch(caller: ThreadId, channel: u64, clear: ClearResult) {
    debug_assert!(read_daif() & DAIF_I != 0);
    let (phase, chan) = wait_snapshot(WaitSide::Call, caller);
    let outcome = UnblockOutcome {
        kind: UnblockKind::Woke,
        phase,
        chan,
    };
    if let Some((key, idx)) = tripwire::classify_reply(outcome, channel, clear).key() {
        add_row(cpu_here(), key, idx, 1);
    }
}

/// `irq_handler_el1`'s first step: this CPU is in IRQ context. Counts `nest`
/// if the CPU was not in thread context, which means an IRQ was taken inside
/// an IRQ handler, or a label was left behind.
#[inline(never)]
pub fn irq_enter() {
    let here = cpu_here();
    if let Some(ctx) = IRQ_CTX.get(usize::from(here)) {
        if ctx.load(Ordering::Relaxed) != IRQ_CTX_THREAD {
            add_row(here, Key::Nest, 0, 1);
        }
        ctx.store(IRQ_CTX_IRQ, Ordering::Relaxed);
    }
}

/// `irq_handler_el1`, before `check_preemption`: the handler's own work is
/// done, and the preemption check, which may switch threads, runs next.
#[inline(never)]
pub fn irq_preempt_check() {
    store_ctx(usize::from(cpu_here()), IRQ_CTX_EXIT);
}

/// `irq_handler_el1`'s last step: back to thread context. The CPU is read
/// again here, because `check_preemption` may have switched this thread out
/// and resumed it on another CPU.
#[inline(never)]
pub fn irq_leave() {
    store_ctx(usize::from(cpu_here()), IRQ_CTX_THREAD);
}

/// `IRQ_CTX[cpu]`: [`IRQ_CTX_THREAD`] for an out-of-range `cpu`. `schedule()`
/// saves its caller's label with it.
#[inline(never)]
pub fn irq_ctx(cpu: usize) -> u8 {
    IRQ_CTX
        .get(cpu)
        .map_or(IRQ_CTX_THREAD, |ctx| ctx.load(Ordering::Relaxed))
}

/// Set `IRQ_CTX[cpu]` to `ctx`. `schedule()` restores its caller's label
/// with it when it resumes as the thread it switched out, on the CPU it
/// resumes on. IRQs masked.
#[inline(never)]
pub fn set_irq_ctx(cpu: usize, ctx: u8) {
    store_ctx(cpu, ctx);
}
