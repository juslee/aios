//! Kernel observability: structured logging, metric counters, trace points.
//!
//! Replaces raw `println!()` with per-core ring-buffered structured logging.
//! Per observability.md §2–4.

pub mod metrics;
pub mod trace;

use core::cell::UnsafeCell;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::smp::MAX_CORES;

// Re-export observability types from shared crate.
pub use shared::{LogEntry, LogLevel, Subsystem};

/// Compile-time minimum log level. Entries below this are eliminated entirely.
#[cfg(debug_assertions)]
pub const MIN_LOG_LEVEL: LogLevel = LogLevel::Debug;
#[cfg(not(debug_assertions))]
pub const MIN_LOG_LEVEL: LogLevel = LogLevel::Info;

// ---------------------------------------------------------------------------
// Per-core ring buffer (observability.md §2.5)
// ---------------------------------------------------------------------------

const LOG_RING_SIZE: usize = 256;
const LOG_RING_MASK: u32 = (LOG_RING_SIZE as u32) - 1;

/// Lock-free per-core log ring buffer.
/// Single-producer (owning core) / single-consumer (drain function).
/// Uses `UnsafeCell` for interior mutability of entries (required by Rust's
/// aliasing rules — `&self` methods that write need `UnsafeCell`).
pub struct LogRing {
    entries: UnsafeCell<[LogEntry; LOG_RING_SIZE]>,
    head: AtomicU32,
    tail: AtomicU32,
}

impl LogRing {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = Self {
        entries: UnsafeCell::new([LogEntry::ZERO; LOG_RING_SIZE]),
        head: AtomicU32::new(0),
        tail: AtomicU32::new(0),
    };

    /// Push a log entry. Overwrites oldest on full (advances tail).
    fn push(&self, entry: LogEntry) {
        let head = self.head.load(Ordering::Relaxed);
        let next_head = head.wrapping_add(1);

        // If the ring is full, advance tail to discard the oldest entry.
        let tail = self.tail.load(Ordering::Relaxed);
        if next_head.wrapping_sub(tail) > LOG_RING_SIZE as u32 {
            self.tail.store(tail.wrapping_add(1), Ordering::Relaxed);
        }

        let idx = (head & LOG_RING_MASK) as usize;

        // SAFETY: Single producer (owning core). UnsafeCell provides interior
        // mutability. No concurrent writes to this index because head is only
        // advanced by the owning core.
        unsafe {
            let slot = (*self.entries.get()).as_mut_ptr().add(idx);
            core::ptr::write(slot, entry);
        }

        self.head.store(next_head, Ordering::Release);
    }

    /// Pop the next entry for the drain consumer. Returns None if empty.
    fn pop(&self) -> Option<LogEntry> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None;
        }

        let idx = (tail & LOG_RING_MASK) as usize;

        // SAFETY: Single consumer (drain function). The entry at `idx` was
        // fully written before head was advanced (Release/Acquire pairing).
        let entry = unsafe {
            let slot = (*self.entries.get()).as_ptr().add(idx);
            core::ptr::read(slot)
        };

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(entry)
    }
}

// SAFETY: LogRing is accessed per-core (producer) and by drain (consumer).
// The SPSC protocol ensures no data races.
unsafe impl Sync for LogRing {}

/// Global log rings, one per core. BSS-allocated.
static LOG_RINGS: [LogRing; MAX_CORES] = [const { LogRing::INIT }; MAX_CORES];

// ---------------------------------------------------------------------------
// Core logging implementation
// ---------------------------------------------------------------------------

/// Read CNTVCT_EL0 (virtual timer count) for timestamps.
#[inline(always)]
fn read_cntvct() -> u64 {
    let val: u64;
    // SAFETY: CNTVCT_EL0 is always readable at EL1.
    unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) val) };
    val
}

/// Read CNTFRQ_EL0 (timer frequency).
#[inline(always)]
fn read_cntfrq() -> u64 {
    let val: u64;
    // SAFETY: CNTFRQ_EL0 is always readable at EL1.
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) val) };
    val
}

/// Read core ID from MPIDR_EL1[7:0].
#[inline(always)]
pub fn current_core_id() -> usize {
    let mpidr: u64;
    // SAFETY: MPIDR_EL1 is always readable at EL1.
    unsafe {
        core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr, options(nomem, nostack, preserves_flags))
    };
    (mpidr & 0xFF) as usize
}

/// Helper that formats into a fixed 48-byte buffer.
struct MsgBuf {
    buf: [u8; 48],
    pos: usize,
}

impl MsgBuf {
    fn new() -> Self {
        Self {
            buf: [0; 48],
            pos: 0,
        }
    }
}

impl fmt::Write for MsgBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let avail = 48 - self.pos;
        let copy_len = bytes.len().min(avail);
        self.buf[self.pos..self.pos + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.pos += copy_len;
        Ok(())
    }
}

/// Core logging function. Called by klog! macro.
///
/// Before LogRingsReady: writes directly to UART (synchronous).
/// After LogRingsReady: writes to per-core ring buffer (non-blocking).
pub fn log_impl(level: LogLevel, subsystem: Subsystem, args: fmt::Arguments) {
    use crate::boot_phase::{current_boot_phase, EarlyBootPhase};

    let phase = current_boot_phase();

    if phase < EarlyBootPhase::LogRingsReady {
        // Early boot fallback: write directly to UART.
        // Format: [secs.micros] [core] LEVEL Subsys Message
        early_boot_log(level, subsystem, args);
        return;
    }

    // Write to per-core ring buffer.
    let core = current_core_id().min(MAX_CORES - 1);
    let timestamp = read_cntvct();

    let mut msg = MsgBuf::new();
    let _ = fmt::write(&mut msg, args);

    let entry = LogEntry {
        timestamp,
        core_id: core as u8,
        level,
        subsystem,
        flags: 0,
        msg_len: msg.pos as u8,
        _reserved: [0; 3],
        message: msg.buf,
    };

    LOG_RINGS[core].push(entry);
}

/// Early boot log: format directly to UART, synchronous.
fn early_boot_log(level: LogLevel, subsystem: Subsystem, args: fmt::Arguments) {
    use crate::arch::aarch64::uart::UartWriter;
    use core::fmt::Write;

    let timestamp = read_cntvct();
    let freq = read_cntfrq();
    let core = current_core_id().min(MAX_CORES - 1);

    let (secs, micros) = shared::timestamp_to_secs_micros(timestamp, freq);

    let mut w = UartWriter;
    let _ = write!(
        w,
        "[{:4}.{:06}] [{}] {} {} ",
        secs,
        micros,
        core,
        level.name(),
        subsystem.name()
    );
    let _ = w.write_fmt(args);
    let _ = w.write_str("\n");
}

// ---------------------------------------------------------------------------
// UART drain (observability.md §2.7)
// ---------------------------------------------------------------------------

/// Maximum entries to drain per call (bounds UART hold time).
/// Maximum log entries drained per call. Kept small so timer_tick_handler
/// completes within the 1ms tick budget at 115200 baud (~7ms per log line).
/// With drain every 4th tick (4ms) and 1 entry/call, effective throughput
/// is ~1 entry/4ms which keeps the handler fast. Burst draining happens
/// from explicit drain_logs() calls in kernel_main (boot sequence).
const DRAIN_BATCH_SIZE: usize = 16;

/// Drain all per-core log rings and write formatted entries to UART.
/// Called from timer tick handler and boot-time flush. Must NOT call klog! (re-entrancy).
pub fn drain_logs() {
    use crate::arch::aarch64::uart::UartWriter;
    use core::fmt::Write;

    let freq = read_cntfrq();
    let mut w = UartWriter;
    let mut drained = 0;

    // Round-robin across all cores.
    for ring in LOG_RINGS.iter() {
        while drained < DRAIN_BATCH_SIZE {
            if let Some(entry) = ring.pop() {
                let (secs, micros) = shared::timestamp_to_secs_micros(entry.timestamp, freq);

                let msg_len = (entry.msg_len as usize).min(48);
                let msg = core::str::from_utf8(&entry.message[..msg_len]).unwrap_or("<invalid>");

                let _ = writeln!(
                    w,
                    "[{:4}.{:06}] [{}] {} {} {}",
                    secs,
                    micros,
                    entry.core_id,
                    entry.level.name(),
                    entry.subsystem.name(),
                    msg,
                );
                drained += 1;
            } else {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Logging macros (observability.md §2.6)
// ---------------------------------------------------------------------------

/// Primary structured logging macro.
/// Usage: klog!(Info, Boot, "message {}", arg);
#[macro_export]
macro_rules! klog {
    ($level:ident, $subsys:ident, $($arg:tt)*) => {{
        const _LEVEL: $crate::observability::LogLevel = $crate::observability::LogLevel::$level;
        if _LEVEL >= $crate::observability::MIN_LOG_LEVEL {
            $crate::observability::log_impl(
                _LEVEL,
                $crate::observability::Subsystem::$subsys,
                format_args!($($arg)*),
            );
        }
    }};
}

/// Convenience macros.
#[macro_export]
macro_rules! kinfo {
    ($subsys:ident, $($arg:tt)*) => { $crate::klog!(Info, $subsys, $($arg)*) };
}

#[macro_export]
macro_rules! kwarn {
    ($subsys:ident, $($arg:tt)*) => { $crate::klog!(Warn, $subsys, $($arg)*) };
}

#[macro_export]
macro_rules! kerror {
    ($subsys:ident, $($arg:tt)*) => { $crate::klog!(Error, $subsys, $($arg)*) };
}

#[macro_export]
macro_rules! kdebug {
    ($subsys:ident, $($arg:tt)*) => { $crate::klog!(Debug, $subsys, $($arg)*) };
}

#[macro_export]
macro_rules! ktrace {
    ($subsys:ident, $($arg:tt)*) => { $crate::klog!(Trace, $subsys, $($arg)*) };
}
