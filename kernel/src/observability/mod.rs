//! Kernel observability: structured logging, metric counters, trace points.
//!
//! Replaces raw `println!()` with per-core ring-buffered structured logging.
//! Per observability.md §2–4.

pub mod metrics;
pub mod trace;

use core::cell::UnsafeCell;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::aarch64::daif::with_irqs_masked;
use crate::smp::MAX_CORES;
use shared::observability::{next_log_line, LogMessageBuf};

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

    /// Push one message: a head entry and, when the message is longer than
    /// one entry, its continuation. Overwrites the oldest entries when full
    /// (advances tail).
    ///
    /// Both entries are written before `head` moves, and `head` moves past
    /// both with one Release store, so the drain sees the pair whole or not
    /// at all. The caller masks IRQs (`log_impl`), so no other producer on
    /// this core runs between the two writes.
    fn push(&self, entry: LogEntry, continuation: Option<LogEntry>) {
        let count = if continuation.is_some() { 2 } else { 1 };
        let head = self.head.load(Ordering::Relaxed);
        let next_head = head.wrapping_add(count);

        // If the ring is full, advance tail to discard the oldest entries. A
        // continuation left at the new tail without its head reaches the
        // drain alone, which prints it with LOG_LOST_HEAD_MARK.
        let tail = self.tail.load(Ordering::Relaxed);
        if next_head.wrapping_sub(tail) > LOG_RING_SIZE as u32 {
            self.tail.store(
                next_head.wrapping_sub(LOG_RING_SIZE as u32),
                Ordering::Relaxed,
            );
        }

        self.write_slot(head, entry);
        if let Some(continuation) = continuation {
            self.write_slot(head.wrapping_add(1), continuation);
        }

        self.head.store(next_head, Ordering::Release);
    }

    /// Write `entry` into the slot for ring position `pos`. Producer only.
    fn write_slot(&self, pos: u32, entry: LogEntry) {
        let idx = (pos & LOG_RING_MASK) as usize;

        // SAFETY: `idx` is masked to LOG_RING_SIZE, so the slot is in
        // bounds, and the slot is not yet published (`head` has not moved
        // past it), so the drain does not read it. `log_impl` keeps this
        // core the ring's only writer: it pushes only to its own core's ring
        // and masks IRQs for the whole push. UnsafeCell provides the
        // interior mutability. A second writer on the ring (an unmasked IRQ
        // producer, or a thread that migrated mid-push) would tear or lose
        // entries.
        unsafe {
            let slot = (*self.entries.get()).as_mut_ptr().add(idx);
            core::ptr::write(slot, entry);
        }
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

// SAFETY: LogRing is accessed per-core (producer: the owning core, with IRQs
// masked by `log_impl`) and by drain (consumer). The SPSC protocol ensures
// no data races: an entry is written before a Release store of `head`
// publishes it. `log_impl` and `drain_logs` maintain this; a second
// producer or consumer on one ring would tear or lose entries.
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

/// Core logging function. Called by klog! macro.
///
/// Before LogRingsReady: writes directly to UART (synchronous).
/// After LogRingsReady: writes to per-core ring buffer (non-blocking). A
/// message longer than one entry takes a head entry and a continuation, and
/// text past two entries is dropped and marked (observability.md §2.4).
pub fn log_impl(level: LogLevel, subsystem: Subsystem, args: fmt::Arguments) {
    use crate::boot_phase::{current_boot_phase, EarlyBootPhase};

    let phase = current_boot_phase();

    if phase < EarlyBootPhase::LogRingsReady {
        // Early boot fallback: write directly to UART.
        // Format: [secs.micros] [core] LEVEL Subsys Message
        early_boot_log(level, subsystem, args);
        return;
    }

    let timestamp = read_cntvct();

    // Format before masking IRQs: formatting is the slow part.
    let mut msg = LogMessageBuf::new();
    let _ = fmt::write(&mut msg, args);

    // Pick the ring and push with IRQs masked. An IRQ-context producer on
    // this core (the load balancer, crash-fix ADR N6) then cannot run
    // between the head entry and its continuation or tear either one, and
    // the thread cannot migrate between reading the core id and pushing to
    // that core's ring.
    with_irqs_masked(|| {
        let core = current_core_id().min(MAX_CORES - 1);
        let (entry, continuation) = msg.entries(timestamp, core as u8, level, subsystem);
        LOG_RINGS[core].push(entry, continuation);
    });
}

/// Early boot log: format directly to UART, synchronous.
/// Also captures to BootLogBuffer for GPU text rendering.
fn early_boot_log(level: LogLevel, subsystem: Subsystem, args: fmt::Arguments) {
    use crate::arch::aarch64::uart::UartWriter;
    use core::fmt::Write;

    let timestamp = read_cntvct();
    let freq = read_cntfrq();
    let core = current_core_id().min(MAX_CORES - 1);

    let (secs, micros) = shared::timestamp_to_secs_micros(timestamp, freq);

    // Format to stack buffer first for dual output (UART + boot log capture).
    let mut line_storage = [0u8; MAX_LINE_LEN];
    let mut lb = LineBuf::new(&mut line_storage);
    let _ = write!(
        lb,
        "[{:4}.{:06}] [{}] {} {} ",
        secs,
        micros,
        core,
        level.name(),
        subsystem.name()
    );
    let _ = lb.write_fmt(args);
    let line_len = lb.len();

    // Write to UART. Use valid_up_to() on truncated UTF-8 to avoid dropping the whole line.
    let mut w = UartWriter;
    let utf8_str = match core::str::from_utf8(&line_storage[..line_len]) {
        Ok(s) => s,
        Err(e) => {
            // SAFETY: valid_up_to() is on a UTF-8 code point boundary per Utf8Error contract.
            unsafe { core::str::from_utf8_unchecked(&line_storage[..e.valid_up_to()]) }
        }
    };
    let _ = w.write_str(utf8_str);
    let _ = w.write_str("\n");

    // Capture to boot log buffer.
    capture_to_boot_log(&line_storage[..line_len]);
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
/// Also captures to BootLogBuffer for GPU text rendering when capture is enabled.
/// Called from timer tick handler and boot-time flush. Must NOT call klog! (re-entrancy).
///
/// A head entry and its continuation print as one line and count as one
/// against DRAIN_BATCH_SIZE. An entry of a message lost to a ring overwrite
/// prints with LOG_LOST_TAIL_MARK or LOG_LOST_HEAD_MARK (see `next_log_line`).
pub fn drain_logs() {
    use crate::arch::aarch64::uart::UartWriter;
    use core::fmt::Write;

    let freq = read_cntfrq();
    let mut w = UartWriter;
    let mut drained = 0;

    // Round-robin across all cores.
    for ring in LOG_RINGS.iter() {
        // An entry popped while looking for a continuation that was not
        // there. It is printed next, even past the batch limit, because it
        // cannot be put back.
        let mut pending = None;
        while drained < DRAIN_BATCH_SIZE || pending.is_some() {
            let Some(line) = next_log_line(&mut pending, || ring.pop()) else {
                break;
            };
            let entry = &line.first;
            let (secs, micros) = shared::timestamp_to_secs_micros(entry.timestamp, freq);

            // Format to stack buffer for dual output (UART + boot log capture).
            let mut line_storage = [0u8; MAX_LINE_LEN];
            let mut lb = LineBuf::new(&mut line_storage);
            let _ = write!(
                lb,
                "[{:4}.{:06}] [{}] {} {} ",
                secs,
                micros,
                entry.core_id,
                entry.level.name(),
                entry.subsystem.name(),
            );
            let _ = line.write_message(&mut lb);
            let line_len = lb.len();

            // Write to UART. A line cut at MAX_LINE_LEN inside a character
            // prints up to the last whole character.
            let line_str = match core::str::from_utf8(&line_storage[..line_len]) {
                Ok(s) => s,
                Err(e) => {
                    core::str::from_utf8(&line_storage[..e.valid_up_to()]).unwrap_or_default()
                }
            };
            let _ = w.write_str(line_str);
            let _ = w.write_str("\n");

            // Capture to boot log buffer.
            capture_to_boot_log(&line_storage[..line_len]);

            drained += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Boot log capture buffer (Phase 6 M21 — text rendering)
// ---------------------------------------------------------------------------

use core::sync::atomic::AtomicBool;

/// Maximum boot log lines captured for GPU display.
pub const MAX_LOG_LINES: usize = 256;

/// Maximum characters per boot log line.
pub const MAX_LINE_LEN: usize = 160;

/// Static buffer capturing formatted boot log lines for GPU text rendering.
///
/// Entries are written by `drain_logs()` and `early_boot_log()` during boot,
/// then read once by the GPU Service's `draw_boot_log()` function.
struct BootLogBuffer {
    lines: [[u8; MAX_LINE_LEN]; MAX_LOG_LINES],
    line_lens: [u8; MAX_LOG_LINES],
    count: usize,
}

impl BootLogBuffer {
    const fn new() -> Self {
        Self {
            lines: [[0u8; MAX_LINE_LEN]; MAX_LOG_LINES],
            line_lens: [0u8; MAX_LOG_LINES],
            count: 0,
        }
    }

    /// Push a formatted log line. Overwrites oldest when full (ring behavior).
    fn push_line(&mut self, line: &[u8]) {
        let idx = self.count % MAX_LOG_LINES;
        let len = line.len().min(MAX_LINE_LEN);
        self.lines[idx][..len].copy_from_slice(&line[..len]);
        self.line_lens[idx] = len as u8;
        self.count += 1;
    }
}

static BOOT_LOG: spin::Mutex<BootLogBuffer> = spin::Mutex::new(BootLogBuffer::new());

/// When true, `drain_logs()` and `early_boot_log()` capture formatted lines
/// to `BOOT_LOG`. Set to false by `take_boot_log()` once the GPU Service reads
/// the buffer. Starts enabled so the full boot sequence is captured.
static BOOT_LOG_CAPTURE: AtomicBool = AtomicBool::new(true);

/// Helper that formats into a fixed-size byte buffer for boot log capture.
/// Silently truncates if the formatted output exceeds the buffer.
struct LineBuf<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> LineBuf<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn len(&self) -> usize {
        self.pos
    }
}

impl fmt::Write for LineBuf<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let avail = self.buf.len() - self.pos;
        let copy_len = bytes.len().min(avail);
        self.buf[self.pos..self.pos + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.pos += copy_len;
        Ok(()) // Always Ok — silently truncates
    }
}

/// Capture a formatted log line to `BOOT_LOG` if capture is enabled.
/// Uses `try_lock()` to avoid deadlock in IRQ context (timer tick handler).
fn capture_to_boot_log(line: &[u8]) {
    if !BOOT_LOG_CAPTURE.load(Ordering::Relaxed) {
        return;
    }
    if let Some(mut guard) = BOOT_LOG.try_lock() {
        guard.push_line(line);
    }
}

/// Retrieve boot log lines for GPU text rendering.
///
/// Disables capture BEFORE locking to prevent IRQ-context deadlock,
/// then copies the most recent lines to the caller's buffers.
/// Returns the number of lines copied.
pub fn take_boot_log(out_lines: &mut [[u8; MAX_LINE_LEN]], out_lens: &mut [u8]) -> usize {
    // Disable capture FIRST — prevents timer tick drain_logs from locking.
    BOOT_LOG_CAPTURE.store(false, Ordering::Release);

    let guard = BOOT_LOG.lock();
    let total = guard.count;
    let capacity = out_lines.len().min(out_lens.len());
    let available = total.min(MAX_LOG_LINES); // Can't have more than ring size
    let copy_count = available.min(capacity);

    // Copy the most recent `copy_count` lines.
    // Ring buffer: entries are at indices (count-available..count-1) % MAX_LOG_LINES.
    let start = total.saturating_sub(copy_count);
    for i in 0..copy_count {
        let src_idx = (start + i) % MAX_LOG_LINES;
        out_lines[i] = guard.lines[src_idx];
        out_lens[i] = guard.line_lens[src_idx];
    }

    copy_count
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
