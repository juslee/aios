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

// ---------------------------------------------------------------------------
// Log levels (observability.md §2.2)
// ---------------------------------------------------------------------------

/// Log severity levels, ordered from most to least verbose.
/// Compile-time filtering eliminates levels below the configured minimum.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
    #[allow(dead_code)]
    Fatal = 5,
}

impl LogLevel {
    /// 5-character padded name for formatted output.
    pub const fn name(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO ",
            LogLevel::Warn => "WARN ",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        }
    }
}

/// Compile-time minimum log level. Entries below this are eliminated entirely.
#[cfg(debug_assertions)]
pub const MIN_LOG_LEVEL: LogLevel = LogLevel::Debug;
#[cfg(not(debug_assertions))]
pub const MIN_LOG_LEVEL: LogLevel = LogLevel::Info;

// ---------------------------------------------------------------------------
// Subsystem tags (observability.md §2.3)
// ---------------------------------------------------------------------------

/// Subsystem tag identifying the origin of a log entry.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Subsystem {
    Boot = 0,
    Mm = 1,
    Sched = 2,
    Ipc = 3,
    Cap = 4,
    Irq = 5,
    Timer = 6,
    Uart = 7,
    Gic = 8,
    Mmu = 9,
    Smp = 10,
    Storage = 11,
    Audit = 12,
}

impl Subsystem {
    #[allow(dead_code)]
    pub const COUNT: usize = 13;

    /// 5-character padded name for formatted output.
    pub const fn name(self) -> &'static str {
        match self {
            Subsystem::Boot => "Boot ",
            Subsystem::Mm => "Mm   ",
            Subsystem::Sched => "Sched",
            Subsystem::Ipc => "Ipc  ",
            Subsystem::Cap => "Cap  ",
            Subsystem::Irq => "Irq  ",
            Subsystem::Timer => "Timer",
            Subsystem::Uart => "Uart ",
            Subsystem::Gic => "Gic  ",
            Subsystem::Mmu => "Mmu  ",
            Subsystem::Smp => "Smp  ",
            Subsystem::Storage => "Stor ",
            Subsystem::Audit => "Audit",
        }
    }
}

// ---------------------------------------------------------------------------
// Log entry (observability.md §2.4)
// ---------------------------------------------------------------------------

/// A single log entry in the kernel ring buffer.
/// Fixed 64 bytes — one per cache line on Cortex-A72.
#[repr(C)]
pub struct LogEntry {
    pub timestamp: u64,
    pub core_id: u8,
    pub level: LogLevel,
    pub subsystem: Subsystem,
    pub flags: u8,
    pub msg_len: u8,
    pub _reserved: [u8; 3],
    pub message: [u8; 48],
}

const _: () = assert!(core::mem::size_of::<LogEntry>() == 64);

impl LogEntry {
    const ZERO: Self = Self {
        timestamp: 0,
        core_id: 0,
        level: LogLevel::Trace,
        subsystem: Subsystem::Boot,
        flags: 0,
        msg_len: 0,
        _reserved: [0; 3],
        message: [0; 48],
    };
}

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

    /// Push a log entry. Overwrites oldest on full.
    fn push(&self, entry: LogEntry) {
        let head = self.head.load(Ordering::Relaxed);
        let idx = (head & LOG_RING_MASK) as usize;

        // SAFETY: Single producer (owning core). UnsafeCell provides interior
        // mutability. No concurrent writes to this index because head is only
        // advanced by the owning core.
        unsafe {
            let slot = (*self.entries.get()).as_mut_ptr().add(idx);
            core::ptr::write(slot, entry);
        }

        self.head.store(head.wrapping_add(1), Ordering::Release);
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

    let (secs, micros) = if freq > 0 {
        let total_us = timestamp / (freq / 1_000_000);
        (total_us / 1_000_000, total_us % 1_000_000)
    } else {
        (0, 0)
    };

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
                let (secs, micros) = if freq > 0 {
                    let total_us = entry.timestamp / (freq / 1_000_000);
                    (total_us / 1_000_000, total_us % 1_000_000)
                } else {
                    (0, 0)
                };

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
