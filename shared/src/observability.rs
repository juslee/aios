//! Observability types: log levels, subsystem tags, log entry layout.
//!
//! Per observability.md §2.2–2.4.

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
}

impl Subsystem {
    /// Total number of subsystem variants.
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
    /// Zero-initialized log entry (const, for array fill).
    pub const ZERO: Self = Self {
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
        assert_eq!(Subsystem::COUNT, 13);
        // Audit is the last variant at index 12.
        assert_eq!(Subsystem::Audit as u8, 12);
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
