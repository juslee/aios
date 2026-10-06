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
/// Single-producer (owning core) / single-consumer (drain function). The
/// producer writes `head`, `dropped`, `drop_pos` and the free slots; the
/// consumer writes `tail` and `dropped_reported`. A message that does not fit
/// is dropped and counted rather than overwriting entries the drain may be
/// reading, and the drain reports the count where the loss happened (`pop`).
/// Uses `UnsafeCell` for interior mutability of entries (required by Rust's
/// aliasing rules — `&self` methods that write need `UnsafeCell`).
pub struct LogRing {
    entries: UnsafeCell<[LogEntry; LOG_RING_SIZE]>,
    head: AtomicU32,
    tail: AtomicU32,
    /// Messages dropped because the ring was full. Producer only.
    dropped: AtomicU32,
    /// Ring position of the latest drop: the value of `head` when it
    /// happened, so the entries before it were logged before the drop.
    /// Producer only.
    drop_pos: AtomicU32,
    /// Value of `dropped` at the drain's last report. Consumer only.
    dropped_reported: AtomicU32,
}

/// What the drain reads next from a ring (`LogRing::pop`).
enum RingItem {
    /// The next entry.
    Entry(LogEntry),
    /// This many messages were dropped at the drain's position: after the
    /// entries it has read and before the next one.
    Dropped(u32),
}

impl LogRing {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = Self {
        entries: UnsafeCell::new([LogEntry::ZERO; LOG_RING_SIZE]),
        head: AtomicU32::new(0),
        tail: AtomicU32::new(0),
        dropped: AtomicU32::new(0),
        drop_pos: AtomicU32::new(0),
        dropped_reported: AtomicU32::new(0),
    };

    /// Push one message: a head entry and, when the message is longer than
    /// one entry, its continuation. When the ring has no room for the whole
    /// message, the message is dropped, counted in `dropped` and its ring
    /// position kept in `drop_pos`; entries already in the ring are never
    /// overwritten.
    ///
    /// Both entries are written before `head` moves, and `head` moves past
    /// both with one Release store, so the drain sees the pair whole or not
    /// at all. The caller masks IRQs (`log_impl`), so no other producer on
    /// this core runs between the two writes.
    fn push(&self, entry: LogEntry, continuation: Option<LogEntry>) {
        let count = if continuation.is_some() { 2 } else { 1 };
        let head = self.head.load(Ordering::Relaxed);
        let next_head = head.wrapping_add(count);

        // Acquire pairs with the Release store of `tail` in `pop`: the drain
        // has finished reading every slot before `tail`, so those slots can
        // be written again.
        let tail = self.tail.load(Ordering::Acquire);
        if next_head.wrapping_sub(tail) > LOG_RING_SIZE as u32 {
            // Full. Record where the message was lost, then count it. Only
            // this producer writes `drop_pos` and `dropped`, so a load and a
            // store count the drop without an atomic read-modify-write. The
            // Release store of `dropped` publishes `drop_pos` with it.
            self.drop_pos.store(head, Ordering::Relaxed);
            let dropped = self.dropped.load(Ordering::Relaxed);
            self.dropped
                .store(dropped.wrapping_add(1), Ordering::Release);
            return;
        }

        self.write_slot(head, entry);
        if let Some(continuation) = continuation {
            self.write_slot(head.wrapping_add(1), continuation);
        }

        self.head.store(next_head, Ordering::Release);
    }

    /// Write `entry` into the slot for ring position `pos`. Producer only:
    /// `pos` is at or after `head` and before `tail + LOG_RING_SIZE`.
    fn write_slot(&self, pos: u32, entry: LogEntry) {
        let idx = (pos & LOG_RING_MASK) as usize;

        // SAFETY: `idx` is masked to LOG_RING_SIZE, so the slot is in
        // bounds. `push` passes only positions from `head` up to before
        // `tail + LOG_RING_SIZE`, and drops the message otherwise, so a
        // single drain is not reading the slot: it reads only positions
        // before `head`, and it finished its last read of this slot before
        // its Release store of `tail` that `push` loaded with Acquire.
        // `log_impl` keeps this core the ring's only writer: it pushes only
        // to its own core's ring and masks IRQs for the whole push.
        // UnsafeCell provides the interior mutability. A second writer on the
        // ring (an unmasked IRQ producer, or a thread that migrated mid-push)
        // would tear or lose entries. Two overlapping `drain_logs` calls (the
        // known gap in `unsafe impl Sync` below) also break this: one can
        // store `tail` past a slot the other is still reading, and this write
        // can then tear the entry that drain reads.
        unsafe {
            let slot = (*self.entries.get()).as_mut_ptr().add(idx);
            core::ptr::write(slot, entry);
        }
    }

    /// Pop the next item for the drain consumer, or None if the ring is
    /// empty. Messages dropped at the drain's position come first, as one
    /// `Dropped` count, so the drain reports them after the entries logged
    /// before them and before the entries logged after them. If the ring
    /// drops messages at more than one position before the drain reaches the
    /// first, the one count comes at the latest position and covers them all.
    fn pop(&self) -> Option<RingItem> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        // A message dropped at `tail` was dropped before the entry at `tail`
        // was pushed. If that entry is published, the Acquire load of `head`
        // above makes the drop visible here, so it is reported before the
        // entry; if the ring is empty, a later call sees it, still at `tail`.
        // The Acquire load of `dropped` pairs with its Release store in
        // `push`, so `drop_pos` is at least as new as the count: every drop
        // counted happened at or before `drop_pos`, and the count is never
        // reported ahead of an entry logged before one of its drops.
        let dropped = self.dropped.load(Ordering::Acquire);
        let reported = self.dropped_reported.load(Ordering::Relaxed);
        if dropped != reported && self.drop_pos.load(Ordering::Relaxed) == tail {
            self.dropped_reported.store(dropped, Ordering::Relaxed);
            return Some(RingItem::Dropped(dropped.wrapping_sub(reported)));
        }

        if tail == head {
            return None;
        }

        let idx = (tail & LOG_RING_MASK) as usize;

        // SAFETY: The entry at `idx` was fully written before `head` moved
        // past it (Release/Acquire pairing), and the producer does not write
        // it again until the Release store of `tail` below moves past it.
        // `drain_logs` is the only consumer. If the producer wrote this slot
        // before `tail` moved past it, this read would return a torn entry.
        // A second consumer popping this ring at the same time could read
        // the same slot twice, so the entry would print twice, or store a
        // `tail` that skips entries (the known overlapping-drain gap in
        // `unsafe impl Sync` below).
        let entry = unsafe {
            let slot = (*self.entries.get()).as_ptr().add(idx);
            core::ptr::read(slot)
        };

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(RingItem::Entry(entry))
    }
}

// SAFETY: Each field of a ring has one writer. The producer is `log_impl` on
// the owning core, with IRQs masked for the whole push: it writes `head`,
// `dropped`, `drop_pos` and the slots from `head` up to before
// `tail + LOG_RING_SIZE`. The consumer is `drain_logs`: it writes `tail` and
// `dropped_reported` and reads the slots from `tail` up to before `head`.
// The Release stores and Acquire loads of `head` and `tail` hand each slot
// from one side to the other, so no slot is read and written at once. A
// second producer, or two consumers popping one ring at once, would break
// this and tear, repeat or lose entries. `drain_logs` also has callers
// besides the CPU 0 timer tick (see `DRAIN_BATCH_SIZE`), and one can overlap
// the tick's drain: a known gap in this protocol, which shows as lost marks
// when it splits a pair, and can repeat a drop report or hold it back until
// the ring's next drop.
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
/// text past two entries is dropped and marked (observability.md §2.4). A
/// message that does not fit in the ring is dropped and counted (§2.5).
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

/// Log lines a `drain_logs` call prints before it stops; a head entry joined
/// with its continuation counts as one line. A call can print more. A ring
/// adds one report line when the drain reaches where it dropped messages (a
/// second report from that ring needs the ring to fill again first, far more
/// entries than one call reads). And the call stops only once no entry is
/// pending: an entry popped while looking for a missing continuation is
/// printed past the limit, usually one more line, but when that entry is a
/// head whose continuation is missing too it pops another, so while drains
/// overlap each such head can add a line, up to what the ring holds. The
/// limit bounds each call's cost for the CPU 0 timer tick, which drains every
/// 4th tick; a full batch still runs well past one 1ms tick (see `timer.rs`).
/// The boot sequence calls `drain_logs` directly as well, to flush bursts,
/// and so does the scheduler's `pc=0` check before it panics, on any CPU.
const DRAIN_BATCH_SIZE: usize = 16;

/// Drain the per-core log rings and write formatted entries to UART,
/// DRAIN_BATCH_SIZE lines per call plus what that doc lists past the limit.
/// Also captures to BootLogBuffer for GPU text rendering when capture is enabled.
/// Called from the timer tick handler, boot-time flushes and the scheduler's
/// `pc=0` check. Must NOT call klog! (re-entrancy).
///
/// A head entry and its continuation print as one line. The producer never
/// splits a pair, so a head without its continuation, or a continuation
/// without its head, means another consumer popped part of the pair; those
/// print with LOG_LOST_TAIL_MARK or LOG_LOST_HEAD_MARK (see `next_log_line`).
/// Messages a full ring dropped are reported where they were lost, as one
/// `[log] core N: K messages dropped (ring full)` line between the entries
/// logged before the drop and those logged after it (`LogRing::pop`).
pub fn drain_logs() {
    use crate::arch::aarch64::uart::UartWriter;
    use core::fmt::Write;

    let freq = read_cntfrq();
    let mut w = UartWriter;
    let mut drained = 0;

    // Round-robin across all cores.
    for (core, ring) in LOG_RINGS.iter().enumerate() {
        // An entry popped while looking for a continuation that was not
        // there. It is printed next, even past the batch limit, because it
        // cannot be put back.
        let mut pending = None;
        while drained < DRAIN_BATCH_SIZE || pending.is_some() {
            let Some(line) = next_log_line(&mut pending, || pop_entry(ring, core)) else {
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
            emit_drained_line(&mut w, &line_storage[..line_len]);

            drained += 1;
        }
    }
}

/// Pop the next entry of `ring`, the ring of core `core`. When the ring
/// dropped messages at this point, their count comes first and is printed
/// here, before the entry, where the loss happened in the log.
fn pop_entry(ring: &LogRing, core: usize) -> Option<LogEntry> {
    loop {
        match ring.pop()? {
            RingItem::Entry(entry) => return Some(entry),
            RingItem::Dropped(count) => report_dropped(core, count),
        }
    }
}

/// Print the `[log] core N: K messages dropped (ring full)` line.
fn report_dropped(core: usize, count: u32) {
    use core::fmt::Write;

    let mut line_storage = [0u8; MAX_LINE_LEN];
    let mut lb = LineBuf::new(&mut line_storage);
    let _ = write!(
        lb,
        "[log] core {}: {} messages dropped (ring full)",
        core, count
    );
    let line_len = lb.len();
    emit_drained_line(
        &mut crate::arch::aarch64::uart::UartWriter,
        &line_storage[..line_len],
    );
}

/// Write one formatted drain line to the UART, then capture it to the boot
/// log buffer. A line cut at MAX_LINE_LEN inside a character prints up to
/// the last whole character.
fn emit_drained_line(w: &mut crate::arch::aarch64::uart::UartWriter, line: &[u8]) {
    use core::fmt::Write;

    let line_str = match core::str::from_utf8(line) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&line[..e.valid_up_to()]).unwrap_or_default(),
    };
    let _ = w.write_str(line_str);
    let _ = w.write_str("\n");
    capture_to_boot_log(line);
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
