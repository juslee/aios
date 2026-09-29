//! Observability types and logic shared with host tests: log levels,
//! subsystem tags, log entry layout, message splitting over a head and
//! continuation entry, and drain-side reassembly.
//!
//! Per observability.md §2.2–2.4 and §2.7.

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

/// Subsystem tag identifying the origin of a log entry.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Gpu = 13,
    Input = 14,
    Compositor = 15,
}

impl Subsystem {
    /// Total number of subsystem variants.
    pub const COUNT: usize = 16;

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
            Subsystem::Gpu => "Gpu  ",
            Subsystem::Input => "Input",
            Subsystem::Compositor => "Comp ",
        }
    }
}

/// Bytes of message text one [`LogEntry`] carries.
pub const LOG_MSG_CAPACITY: usize = 48;

/// Bytes of message text a head entry and its continuation carry together.
pub const LOG_CHAIN_CAPACITY: usize = 2 * LOG_MSG_CAPACITY;

/// `LogEntry::flags` bit 0: the next entry of the same ring continues this
/// entry's message.
pub const LOG_FLAG_CONTINUED: u8 = 1 << 0;

/// `LogEntry::flags` bit 1: this entry continues the message of the entry
/// before it in the same ring.
pub const LOG_FLAG_CONTINUATION: u8 = 1 << 1;

/// Last byte of a chain whose message did not fit in [`LOG_CHAIN_CAPACITY`]
/// bytes: the text after it was dropped.
pub const LOG_OVERFLOW_MARK: u8 = b'~';

/// Printed after a head entry whose continuation was not the next entry the
/// drain read (for example, another drain call popped it).
pub const LOG_LOST_TAIL_MARK: &str = "~<lost>";

/// Printed before a continuation that the drain read without its head entry
/// (for example, another drain call popped the head).
pub const LOG_LOST_HEAD_MARK: &str = "<lost>~";

/// A single log entry in the kernel ring buffer.
/// Fixed 64 bytes — one per cache line on Cortex-A72.
///
/// A message longer than [`LOG_MSG_CAPACITY`] bytes takes two entries: a head
/// with [`LOG_FLAG_CONTINUED`] and, right after it in the same ring, a
/// continuation with [`LOG_FLAG_CONTINUATION`] that repeats the head's
/// timestamp, core, level and subsystem. See [`LogMessageBuf::entries`] and
/// [`next_log_line`].
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LogEntry {
    pub timestamp: u64,
    pub core_id: u8,
    pub level: LogLevel,
    pub subsystem: Subsystem,
    /// Bit 0: [`LOG_FLAG_CONTINUED`]. Bit 1: [`LOG_FLAG_CONTINUATION`].
    pub flags: u8,
    /// Valid bytes in `message` (0..=48).
    pub msg_len: u8,
    pub _reserved: [u8; 3],
    /// UTF-8 message text, not NUL-terminated.
    pub message: [u8; LOG_MSG_CAPACITY],
}

const _: () = assert!(core::mem::size_of::<LogEntry>() == 64);

impl LogEntry {
    /// Zero-initialized log entry (const, for array fill).
    pub const ZERO: Self = Self {
        timestamp: 0,
        core_id: 0,
        level: LogLevel::Trace,
        subsystem: Subsystem::Boot,
        flags: 0,
        msg_len: 0,
        _reserved: [0; 3],
        message: [0; LOG_MSG_CAPACITY],
    };

    /// True when the next entry of the same ring continues this message.
    pub const fn is_continued(&self) -> bool {
        self.flags & LOG_FLAG_CONTINUED != 0
    }

    /// True when this entry continues the message of the entry before it.
    pub const fn is_continuation(&self) -> bool {
        self.flags & LOG_FLAG_CONTINUATION != 0
    }

    /// The entry's message text: the first `msg_len` bytes, at most
    /// [`LOG_MSG_CAPACITY`]. Bytes that are not valid UTF-8 (a torn entry)
    /// end the text at the last whole character before them, so a bad byte
    /// costs only the text after it rather than the whole message.
    pub fn text(&self) -> &str {
        let bytes = &self.message[..(self.msg_len as usize).min(LOG_MSG_CAPACITY)];
        match core::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => core::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or_default(),
        }
    }

    /// This entry's header with `flags` and a message of `text`, followed by
    /// [`LOG_OVERFLOW_MARK`] when `mark` is set. `text` plus the mark must fit
    /// in [`LOG_MSG_CAPACITY`] bytes; [`split_log_message`] guarantees it.
    fn with_text(self, flags: u8, text: &str, mark: bool) -> Self {
        let mut entry = Self {
            flags,
            msg_len: 0,
            message: [0; LOG_MSG_CAPACITY],
            ..self
        };
        let mut len = text.len();
        entry.message[..len].copy_from_slice(text.as_bytes());
        if mark {
            entry.message[len] = LOG_OVERFLOW_MARK;
            len += 1;
        }
        entry.msg_len = len as u8;
        entry
    }
}

/// End of the longest prefix of `s` that is at most `max` bytes long and ends
/// on a character boundary.
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut end = max;
    // Index 0 is always a boundary, so the loop ends.
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// How one formatted message is laid out over a head entry and an optional
/// continuation entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogSplit<'a> {
    /// Text of the head entry.
    pub head: &'a str,
    /// Text of the continuation entry, when the message needs one.
    pub continuation: Option<&'a str>,
    /// The message did not fit in two entries: the continuation ends with
    /// [`LOG_OVERFLOW_MARK`] after its text.
    pub overflow: bool,
}

/// Split a formatted message over at most two log entries.
///
/// `truncated` says text after `msg` was already dropped (the formatting
/// buffer filled up). Each piece ends on a character boundary, so a
/// multi-byte character that would straddle byte 48 or byte 96 is not cut:
/// at byte 48 it moves whole into the continuation, at byte 96 it is dropped
/// with the rest of the overflow. When the message does not fit, the
/// continuation keeps one byte for [`LOG_OVERFLOW_MARK`], so it carries at
/// most `LOG_MSG_CAPACITY - 1` bytes of text.
pub fn split_log_message(msg: &str, truncated: bool) -> LogSplit<'_> {
    if !truncated && msg.len() <= LOG_MSG_CAPACITY {
        return LogSplit {
            head: msg,
            continuation: None,
            overflow: false,
        };
    }
    let (head, rest) = msg.split_at(floor_char_boundary(msg, LOG_MSG_CAPACITY));
    if !truncated && rest.len() <= LOG_MSG_CAPACITY {
        return LogSplit {
            head,
            continuation: Some(rest),
            overflow: false,
        };
    }
    LogSplit {
        head,
        continuation: Some(&rest[..floor_char_boundary(rest, LOG_MSG_CAPACITY - 1)]),
        overflow: true,
    }
}

/// Fixed buffer that formats one log message for up to two ring entries.
///
/// It keeps at most [`LOG_CHAIN_CAPACITY`] bytes. A write that does not fit
/// keeps the whole characters that do, and marks the buffer truncated; every
/// later write is then dropped, so the kept text is always a prefix of the
/// full message.
pub struct LogMessageBuf {
    buf: [u8; LOG_CHAIN_CAPACITY],
    len: usize,
    truncated: bool,
}

impl LogMessageBuf {
    /// An empty buffer.
    pub const fn new() -> Self {
        Self {
            buf: [0; LOG_CHAIN_CAPACITY],
            len: 0,
            truncated: false,
        }
    }

    /// The text kept so far.
    pub fn as_str(&self) -> &str {
        // Only whole `&str` prefixes that end on a character boundary are
        // copied in, so the kept bytes are always valid UTF-8.
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or_default()
    }

    /// True when some of the message did not fit and was dropped.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// The message laid out over a head entry and an optional continuation.
    pub fn split(&self) -> LogSplit<'_> {
        split_log_message(self.as_str(), self.truncated)
    }

    /// The ring entries for this message: a head entry, and a continuation
    /// when the message is longer than one entry. Both carry the given
    /// timestamp, core, level and subsystem. The caller pushes the pair so
    /// that the continuation is the next entry of the ring after the head.
    pub fn entries(
        &self,
        timestamp: u64,
        core_id: u8,
        level: LogLevel,
        subsystem: Subsystem,
    ) -> (LogEntry, Option<LogEntry>) {
        let header = LogEntry {
            timestamp,
            core_id,
            level,
            subsystem,
            ..LogEntry::ZERO
        };
        let split = self.split();
        match split.continuation {
            None => (header.with_text(0, split.head, false), None),
            Some(rest) => (
                header.with_text(LOG_FLAG_CONTINUED, split.head, false),
                Some(header.with_text(LOG_FLAG_CONTINUATION, rest, split.overflow)),
            ),
        }
    }
}

impl Default for LogMessageBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Write for LogMessageBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.truncated {
            return Ok(());
        }
        let n = floor_char_boundary(s, LOG_CHAIN_CAPACITY - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        if n < s.len() {
            self.truncated = true;
        }
        // Never an error: a log call must not fail because its text is long.
        Ok(())
    }
}

/// How a reassembled log line relates to the entries it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLineStatus {
    /// A single entry, or a head joined with its continuation.
    Whole,
    /// A head entry whose continuation was not the next entry the drain
    /// read.
    TailLost,
    /// A continuation that the drain read without its head entry.
    HeadLost,
}

/// One log message, reassembled from a ring's entries by [`next_log_line`].
#[derive(Clone, Copy)]
pub struct LogLine {
    /// First entry of the line. Its timestamp, core, level and subsystem
    /// head the printed line.
    pub first: LogEntry,
    /// The continuation joined to `first`, if any.
    pub continuation: Option<LogEntry>,
    /// Whether an entry of the message is missing.
    pub status: LogLineStatus,
}

impl LogLine {
    /// Write the line's message text: the entries' text joined, with
    /// [`LOG_LOST_HEAD_MARK`] or [`LOG_LOST_TAIL_MARK`] where an entry of the
    /// message is missing.
    pub fn write_message<W: core::fmt::Write>(&self, w: &mut W) -> core::fmt::Result {
        if self.status == LogLineStatus::HeadLost {
            w.write_str(LOG_LOST_HEAD_MARK)?;
        }
        w.write_str(self.first.text())?;
        if let Some(continuation) = &self.continuation {
            w.write_str(continuation.text())?;
        }
        if self.status == LogLineStatus::TailLost {
            w.write_str(LOG_LOST_TAIL_MARK)?;
        }
        Ok(())
    }
}

/// Reassemble the next log line from one ring's entries, in ring order.
///
/// `pop` takes the ring's next entry. A head entry is joined with the entry
/// after it when that entry is a continuation with the same timestamp and
/// core. Any other next entry means the continuation was lost: the head is
/// returned as [`LogLineStatus::TailLost`] and the entry is left in `pending`.
/// Pass the same `pending` to the next call, which returns that entry before
/// popping another, and keep calling while `pending` is `Some` so the entry
/// is not dropped. A continuation with no head before it is returned alone as
/// [`LogLineStatus::HeadLost`]. Returns `None` when `pending` is empty and
/// the ring has no entry.
pub fn next_log_line(
    pending: &mut Option<LogEntry>,
    mut pop: impl FnMut() -> Option<LogEntry>,
) -> Option<LogLine> {
    let first = match pending.take() {
        Some(entry) => entry,
        None => pop()?,
    };
    let status = if first.is_continuation() {
        LogLineStatus::HeadLost
    } else if !first.is_continued() {
        LogLineStatus::Whole
    } else {
        match pop() {
            Some(next)
                if next.is_continuation()
                    && next.timestamp == first.timestamp
                    && next.core_id == first.core_id =>
            {
                return Some(LogLine {
                    first,
                    continuation: Some(next),
                    status: LogLineStatus::Whole,
                });
            }
            next => {
                *pending = next;
                LogLineStatus::TailLost
            }
        }
    };
    Some(LogLine {
        first,
        continuation: None,
        status,
    })
}

/// Convert a timer tick count to (seconds, microseconds).
///
/// Uses u128 intermediate to avoid overflow on large tick counts.
/// Returns (0, 0) if freq is 0.
pub fn timestamp_to_secs_micros(timestamp: u64, freq: u64) -> (u64, u64) {
    if freq == 0 {
        return (0, 0);
    }
    let total_us = (timestamp as u128 * 1_000_000 / freq as u128) as u64;
    (total_us / 1_000_000, total_us % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- LogLevel tests ---

    #[test]
    fn log_level_ordering() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Fatal);
    }

    #[test]
    fn log_level_repr_values() {
        assert_eq!(LogLevel::Trace as u8, 0);
        assert_eq!(LogLevel::Debug as u8, 1);
        assert_eq!(LogLevel::Info as u8, 2);
        assert_eq!(LogLevel::Warn as u8, 3);
        assert_eq!(LogLevel::Error as u8, 4);
        assert_eq!(LogLevel::Fatal as u8, 5);
    }

    #[test]
    fn log_level_names_are_5_chars() {
        let levels = [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
            LogLevel::Fatal,
        ];
        for level in levels {
            assert_eq!(level.name().len(), 5, "{:?} name not 5 chars", level);
        }
    }

    #[test]
    fn log_level_name_content() {
        assert_eq!(LogLevel::Trace.name(), "TRACE");
        assert_eq!(LogLevel::Info.name(), "INFO ");
        assert_eq!(LogLevel::Fatal.name(), "FATAL");
    }

    #[test]
    fn log_level_equality() {
        assert_eq!(LogLevel::Info, LogLevel::Info);
        assert_ne!(LogLevel::Info, LogLevel::Warn);
    }

    // --- Subsystem tests ---

    #[test]
    fn subsystem_count() {
        assert_eq!(Subsystem::COUNT, 16);
        // Compositor is the last variant at index 15.
        assert_eq!(Subsystem::Compositor as u8, 15);
    }

    #[test]
    fn subsystem_repr_values() {
        assert_eq!(Subsystem::Boot as u8, 0);
        assert_eq!(Subsystem::Mm as u8, 1);
        assert_eq!(Subsystem::Sched as u8, 2);
        assert_eq!(Subsystem::Ipc as u8, 3);
        assert_eq!(Subsystem::Cap as u8, 4);
        assert_eq!(Subsystem::Irq as u8, 5);
        assert_eq!(Subsystem::Timer as u8, 6);
        assert_eq!(Subsystem::Uart as u8, 7);
        assert_eq!(Subsystem::Gic as u8, 8);
        assert_eq!(Subsystem::Mmu as u8, 9);
        assert_eq!(Subsystem::Smp as u8, 10);
        assert_eq!(Subsystem::Storage as u8, 11);
        assert_eq!(Subsystem::Audit as u8, 12);
        assert_eq!(Subsystem::Gpu as u8, 13);
        assert_eq!(Subsystem::Input as u8, 14);
        assert_eq!(Subsystem::Compositor as u8, 15);
    }

    #[test]
    fn subsystem_names_are_5_chars() {
        let subsystems = [
            Subsystem::Boot,
            Subsystem::Mm,
            Subsystem::Sched,
            Subsystem::Ipc,
            Subsystem::Cap,
            Subsystem::Irq,
            Subsystem::Timer,
            Subsystem::Uart,
            Subsystem::Gic,
            Subsystem::Mmu,
            Subsystem::Smp,
            Subsystem::Storage,
            Subsystem::Audit,
            Subsystem::Gpu,
            Subsystem::Input,
            Subsystem::Compositor,
        ];
        for sub in subsystems {
            assert_eq!(sub.name().len(), 5, "{:?} name not 5 chars", sub);
        }
    }

    #[test]
    fn subsystem_name_content() {
        assert_eq!(Subsystem::Boot.name(), "Boot ");
        assert_eq!(Subsystem::Sched.name(), "Sched");
        assert_eq!(Subsystem::Storage.name(), "Stor ");
        assert_eq!(Subsystem::Input.name(), "Input");
        assert_eq!(Subsystem::Compositor.name(), "Comp ");
    }

    // --- LogEntry tests ---

    #[test]
    fn log_entry_size_is_64_bytes() {
        assert_eq!(core::mem::size_of::<LogEntry>(), 64);
    }

    #[test]
    fn log_entry_zero_is_valid() {
        let entry = LogEntry::ZERO;
        assert_eq!(entry.timestamp, 0);
        assert_eq!(entry.core_id, 0);
        assert_eq!(entry.level, LogLevel::Trace);
        assert_eq!(entry.subsystem, Subsystem::Boot);
        assert_eq!(entry.msg_len, 0);
    }

    #[test]
    fn log_entry_message_capacity() {
        // 48 bytes for message payload.
        assert_eq!(LogEntry::ZERO.message.len(), 48);
        assert_eq!(LOG_MSG_CAPACITY, 48);
        assert_eq!(LOG_CHAIN_CAPACITY, 96);
    }

    // --- Message splitting and continuation entries ---

    use alloc::string::String;
    use alloc::vec::Vec;
    use core::fmt::Write;

    /// An ASCII message of `len` bytes whose bytes spell their own index mod
    /// 10, so a misplaced cut shows up as a wrong digit.
    fn ascii(len: usize) -> String {
        (0..len)
            .map(|i| char::from(b'0' + (i % 10) as u8))
            .collect()
    }

    /// `msg` formatted through a `LogMessageBuf`, as `log_impl` does.
    fn buffered(msg: &str) -> LogMessageBuf {
        let mut buf = LogMessageBuf::new();
        write!(buf, "{}", msg).unwrap();
        buf
    }

    /// Entries of `msg` as the ring holds them after one push.
    fn ring_entries(msg: &str) -> Vec<LogEntry> {
        let (head, continuation) = buffered(msg).entries(77, 2, LogLevel::Warn, Subsystem::Ipc);
        let mut ring = Vec::from([head]);
        ring.extend(continuation);
        ring
    }

    /// Drain `ring` with `next_log_line`, as `drain_logs` does, into the
    /// printed message texts and their statuses.
    fn drain(ring: Vec<LogEntry>) -> Vec<(String, LogLineStatus)> {
        let mut entries = ring.into_iter();
        let mut pending = None;
        let mut lines = Vec::new();
        while let Some(line) = next_log_line(&mut pending, || entries.next()) {
            let mut text = String::new();
            line.write_message(&mut text).unwrap();
            lines.push((text, line.status));
        }
        assert!(pending.is_none(), "an entry was left pending");
        lines
    }

    #[test]
    fn split_short_messages_take_one_entry() {
        for len in [0, 1, 47, 48] {
            let msg = ascii(len);
            let split = split_log_message(&msg, false);
            assert_eq!(split.head, msg, "len {}", len);
            assert_eq!(split.continuation, None, "len {}", len);
            assert!(!split.overflow, "len {}", len);
        }
    }

    #[test]
    fn split_49_to_96_bytes_takes_a_continuation() {
        for len in [49, 95, 96] {
            let msg = ascii(len);
            let split = split_log_message(&msg, false);
            assert_eq!(split.head, &msg[..48], "len {}", len);
            assert_eq!(split.continuation, Some(&msg[48..]), "len {}", len);
            assert!(!split.overflow, "len {}", len);
        }
    }

    #[test]
    fn split_over_96_bytes_marks_overflow() {
        for len in [97, 200] {
            let msg = ascii(len);
            let split = split_log_message(&msg, false);
            assert_eq!(split.head, &msg[..48], "len {}", len);
            // One byte of the continuation is kept for the overflow mark.
            assert_eq!(split.continuation, Some(&msg[48..95]), "len {}", len);
            assert!(split.overflow, "len {}", len);
        }
    }

    #[test]
    fn split_truncated_input_marks_overflow() {
        // A 96-byte buffer that dropped text still needs the mark.
        let msg = ascii(96);
        let split = split_log_message(&msg, true);
        assert_eq!(split.continuation, Some(&msg[48..95]));
        assert!(split.overflow);
        // A short truncated input (not produced by LogMessageBuf) still ends
        // in a marked continuation rather than silently losing text.
        let split = split_log_message("abc", true);
        assert_eq!(split.head, "abc");
        assert_eq!(split.continuation, Some(""));
        assert!(split.overflow);
    }

    #[test]
    fn split_moves_a_character_straddling_byte_48_into_the_continuation() {
        // 'é' is 2 bytes at 47..49.
        let msg = alloc::format!("{}é{}", ascii(47), ascii(10));
        let split = split_log_message(&msg, false);
        assert_eq!(split.head, ascii(47));
        assert_eq!(split.continuation, Some(&*alloc::format!("é{}", ascii(10))));
        assert!(!split.overflow);

        // '€' is 3 bytes at 46..49.
        let msg = alloc::format!("{}€x", ascii(46));
        let split = split_log_message(&msg, false);
        assert_eq!(split.head, ascii(46));
        assert_eq!(split.continuation, Some("€x"));
    }

    #[test]
    fn split_cuts_a_character_straddling_byte_96_on_its_boundary() {
        // '€' is 3 bytes at 94..97: the message does not fit, and the
        // continuation's 47 text bytes (48..95) would end inside the '€'.
        let msg = alloc::format!("{}{}€c", ascii(48), ascii(46));
        let split = split_log_message(&msg, false);
        assert_eq!(split.head, ascii(48));
        assert_eq!(split.continuation, Some(&msg[48..94]));
        assert!(split.overflow);

        // A character that ends exactly at byte 96 fits.
        let msg = alloc::format!("{}{}é", ascii(48), ascii(46));
        assert_eq!(msg.len(), 96);
        let split = split_log_message(&msg, false);
        assert_eq!(split.continuation, Some(&msg[48..]));
        assert!(!split.overflow);
    }

    #[test]
    fn split_short_head_can_push_a_96_byte_message_into_overflow() {
        // 47 + 'é' + 47 = 96 bytes, but the head ends at 47, leaving 49.
        let msg = alloc::format!("{}é{}", ascii(47), ascii(47));
        assert_eq!(msg.len(), 96);
        let split = split_log_message(&msg, false);
        assert_eq!(split.head, ascii(47));
        assert_eq!(split.continuation.map(str::len), Some(47));
        assert!(split.overflow);
    }

    #[test]
    fn message_buf_keeps_a_prefix_and_records_truncation() {
        let buf = buffered(&ascii(96));
        assert_eq!(buf.as_str(), ascii(96));
        assert!(!buf.is_truncated());

        let buf = buffered(&ascii(200));
        assert_eq!(buf.as_str(), ascii(96));
        assert!(buf.is_truncated());

        // A write that does not fit keeps whole characters only, and later
        // writes that would fit are dropped so the text stays a prefix.
        let mut buf = LogMessageBuf::new();
        write!(buf, "{}", ascii(95)).unwrap();
        write!(buf, "é").unwrap();
        write!(buf, "z").unwrap();
        assert_eq!(buf.as_str(), ascii(95));
        assert!(buf.is_truncated());
    }

    #[test]
    fn entries_of_a_short_message() {
        let (head, continuation) = buffered("hello").entries(5, 3, LogLevel::Info, Subsystem::Mm);
        assert!(continuation.is_none());
        assert_eq!(head.flags, 0);
        assert_eq!(head.text(), "hello");
        assert_eq!(head.timestamp, 5);
        assert_eq!(head.core_id, 3);
        assert_eq!(head.level, LogLevel::Info);
        assert_eq!(head.subsystem, Subsystem::Mm);

        let (head, continuation) = buffered("").entries(5, 3, LogLevel::Info, Subsystem::Mm);
        assert!(continuation.is_none());
        assert_eq!(head.msg_len, 0);
    }

    #[test]
    fn entries_of_a_long_message_chain_with_flags_and_header() {
        let msg = ascii(70);
        let (head, continuation) = buffered(&msg).entries(9, 1, LogLevel::Error, Subsystem::Cap);
        let continuation = continuation.expect("70 bytes need a continuation");
        assert_eq!(head.flags, LOG_FLAG_CONTINUED);
        assert!(head.is_continued() && !head.is_continuation());
        assert_eq!(continuation.flags, LOG_FLAG_CONTINUATION);
        assert!(continuation.is_continuation() && !continuation.is_continued());
        assert_eq!(head.text(), &msg[..48]);
        assert_eq!(continuation.text(), &msg[48..]);
        assert_eq!(continuation.timestamp, 9);
        assert_eq!(continuation.core_id, 1);
        assert_eq!(continuation.level, LogLevel::Error);
        assert_eq!(continuation.subsystem, Subsystem::Cap);
    }

    #[test]
    fn entries_of_an_overflowing_message_end_with_the_mark() {
        let (_, continuation) =
            buffered(&ascii(200)).entries(0, 0, LogLevel::Info, Subsystem::Boot);
        let continuation = continuation.unwrap();
        assert_eq!(continuation.msg_len, 48);
        assert_eq!(
            continuation.text(),
            alloc::format!("{}~", &ascii(96)[48..95])
        );
    }

    #[test]
    fn drain_joins_every_length_back_into_one_line() {
        for len in [0, 1, 47, 48, 49, 95, 96] {
            let msg = ascii(len);
            assert_eq!(
                drain(ring_entries(&msg)),
                [(msg.clone(), LogLineStatus::Whole)],
                "len {}",
                len
            );
        }
        for len in [97, 200] {
            let expected = alloc::format!("{}~", &ascii(len)[..95]);
            assert_eq!(
                drain(ring_entries(&ascii(len))),
                [(expected, LogLineStatus::Whole)],
                "len {}",
                len
            );
        }
        let msg = alloc::format!("{}é{}", ascii(47), ascii(10));
        assert_eq!(
            drain(ring_entries(&msg)),
            [(msg.clone(), LogLineStatus::Whole)]
        );
    }

    #[test]
    fn drain_keeps_consecutive_messages_apart() {
        let mut ring = ring_entries(&ascii(60));
        ring.extend(ring_entries("short"));
        ring.extend(ring_entries(&ascii(90)));
        assert_eq!(
            drain(ring),
            [
                (ascii(60), LogLineStatus::Whole),
                (String::from("short"), LogLineStatus::Whole),
                (ascii(90), LogLineStatus::Whole),
            ]
        );
    }

    #[test]
    fn drain_marks_a_continuation_whose_head_is_missing() {
        let mut ring = ring_entries(&ascii(60));
        ring.remove(0);
        ring.extend(ring_entries("next"));
        assert_eq!(
            drain(ring),
            [
                (
                    alloc::format!("<lost>~{}", &ascii(60)[48..]),
                    LogLineStatus::HeadLost
                ),
                (String::from("next"), LogLineStatus::Whole),
            ]
        );
    }

    #[test]
    fn drain_marks_a_head_whose_continuation_is_missing() {
        // The ring ends right after the head.
        let mut ring = ring_entries(&ascii(60));
        ring.pop();
        assert_eq!(
            drain(ring),
            [(
                alloc::format!("{}~<lost>", &ascii(60)[..48]),
                LogLineStatus::TailLost
            )]
        );

        // Another message follows the head: it is still printed, whole.
        let mut ring = ring_entries(&ascii(60));
        ring.pop();
        ring.extend(ring_entries(&ascii(70)));
        assert_eq!(
            drain(ring),
            [
                (
                    alloc::format!("{}~<lost>", &ascii(60)[..48]),
                    LogLineStatus::TailLost
                ),
                (ascii(70), LogLineStatus::Whole),
            ]
        );
    }

    #[test]
    fn drain_does_not_join_a_continuation_of_another_message() {
        // Head of message A, then the continuation of message B (A's
        // continuation and B's head lost): they have different timestamps.
        let a = ring_entries(&ascii(60));
        let (_, b_continuation) =
            buffered(&ascii(70)).entries(78, 2, LogLevel::Warn, Subsystem::Ipc);
        let ring = Vec::from([a[0], b_continuation.unwrap()]);
        assert_eq!(
            drain(ring),
            [
                (
                    alloc::format!("{}~<lost>", &ascii(60)[..48]),
                    LogLineStatus::TailLost
                ),
                (
                    alloc::format!("<lost>~{}", &ascii(70)[48..]),
                    LogLineStatus::HeadLost
                ),
            ]
        );
    }

    #[test]
    fn entry_text_is_clamped_and_cut_at_invalid_utf8() {
        let mut entry = LogEntry::ZERO.with_text(0, "abcdef", false);
        entry.message[3] = 0xFF;
        assert_eq!(entry.text(), "abc");

        let mut entry = LogEntry::ZERO.with_text(0, &ascii(48), false);
        entry.msg_len = 200;
        assert_eq!(entry.text(), ascii(48));
    }

    // --- timestamp_to_secs_micros tests ---

    #[test]
    fn timestamp_zero_freq() {
        assert_eq!(timestamp_to_secs_micros(1000, 0), (0, 0));
    }

    #[test]
    fn timestamp_zero_ticks() {
        assert_eq!(timestamp_to_secs_micros(0, 62_500_000), (0, 0));
    }

    #[test]
    fn timestamp_one_second() {
        // 62.5 MHz timer, 62_500_000 ticks = 1 second.
        let (secs, micros) = timestamp_to_secs_micros(62_500_000, 62_500_000);
        assert_eq!(secs, 1);
        assert_eq!(micros, 0);
    }

    #[test]
    fn timestamp_fractional() {
        // 31_250_000 ticks at 62.5 MHz = 0.5 seconds = 500000 us.
        let (secs, micros) = timestamp_to_secs_micros(31_250_000, 62_500_000);
        assert_eq!(secs, 0);
        assert_eq!(micros, 500_000);
    }

    #[test]
    fn timestamp_large_value() {
        // 10 minutes at 62.5 MHz = 37_500_000_000 ticks.
        let (secs, micros) = timestamp_to_secs_micros(37_500_000_000, 62_500_000);
        assert_eq!(secs, 600);
        assert_eq!(micros, 0);
    }

    #[test]
    fn timestamp_no_overflow_at_max() {
        // Large but valid tick count — u128 intermediate prevents overflow.
        let (secs, _micros) = timestamp_to_secs_micros(u64::MAX, 62_500_000);
        // u64::MAX / 62.5M ≈ 2.95 × 10^11 seconds — just check it doesn't panic.
        assert!(secs > 0);
    }
}
