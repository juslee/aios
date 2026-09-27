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
//! instructions to them. The counters use `Relaxed` load and store only: each
//! CPU writes only its own row, with IRQs masked, so no atomic
//! read-modify-write is needed. The flags here are plain loads and stores
//! too.

#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use shared::tripwire::{self, CpuCounters, Key, LineMode, LineSrc, Sink};

use crate::arch::aarch64::{timer, uart};
use crate::smp::{self, MAX_CORES};

const _: () = assert!(tripwire::MAX_CPUS == MAX_CORES);
const _: () = assert!(tripwire::MASK_TIDS as usize == crate::task::MAX_THREADS);

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
