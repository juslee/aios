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
//! interleave byte by byte. After [`MAX_DEFER_TICKS`] ticks of waiting the
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
//! The counters use `Relaxed` load and store only: each CPU writes only its
//! own row, with IRQs masked, so no atomic read-modify-write is needed. The
//! flags, the per-CPU dispatch state and the per-thread stamps are plain
//! loads and stores too.

#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use shared::lock::TID_NONE;
use shared::tripwire::{
    self, CpuCounters, Key, LineMode, LineSrc, Sink, IRQ_CTX_EXIT, IRQ_CTX_IRQ, IRQ_CTX_THREAD,
};

use crate::arch::aarch64::{timer, uart};
use crate::smp::{self, MAX_CORES};
use crate::task::{ThreadId, MAX_THREADS};

const _: () = assert!(tripwire::MAX_CPUS == MAX_CORES);
const _: () = assert!(tripwire::MASK_TIDS as usize == MAX_THREADS);

/// DAIF.I, the IRQ mask bit.
const DAIF_I: u64 = 1 << 7;

/// Ticks CPU 0 holds a pending line back while the console is busy.
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
/// The `asm!` has no `nomem` option, so it is also a compiler barrier: the
/// read stays after an earlier `msr DAIFSet`, and the id cannot come from
/// before an IRQ mask that pins the thread to this CPU.
#[inline(always)]
fn cpu_here() -> u8 {
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

/// DAIF as it is now.
#[inline(always)]
fn read_daif() -> u64 {
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
#[expect(
    dead_code,
    reason = "the lock slow path and the IPC fallbacks are the callers that run with IRQs on"
)]
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
struct UartSink;

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

/// CPU 0's last step in each timer tick: print a pending line, unless the
/// console is busy and the line has waited fewer than [`MAX_DEFER_TICKS`]
/// ticks. Prints at most one line per tick, the heartbeat's first. IRQ
/// context, CPU 0 only.
#[inline(never)]
pub fn end_of_tick() {
    let hb = HB_PENDING.load(Ordering::Relaxed);
    if !hb && !G1_PENDING.load(Ordering::Acquire) {
        return;
    }
    if CONSOLE_BUSY.load(Ordering::Acquire) {
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
/// Cleared when the thread is dispatched.
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
///    mask IRQs, N4);
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
/// `LAST_CPU` never, `LAST_RUN` now, no wake in flight. `allocate_thread`
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
