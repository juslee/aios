//! Syscall numbers and IPC error codes.
//!
//! These are ABI-stable values shared between kernel and user space.
//! Per ipc.md §3.1–3.2.

/// Syscall numbers matching the IPC architecture spec.
///
/// Convention: x8 = syscall number, x0-x5 = args, return in x0.
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Syscall {
    IpcCall = 0,
    IpcSend = 1,
    IpcRecv = 2,
    IpcReply = 3,
    IpcCancel = 4,
    IpcSelect = 5,
    ChannelCreate = 6,
    ChannelDestroy = 7,
    RingChannelCreate = 8,
    RingChannelDestroy = 9,
    NotificationCreate = 10,
    NotificationSignal = 11,
    NotificationWait = 12,
    ChannelStats = 13,
    CapabilityTransfer = 14,
    CapabilityAttenuate = 15,
    CapabilityRevoke = 16,
    CapabilityList = 17,
    MemoryMap = 18,
    MemoryUnmap = 19,
    SharedMemoryCreate = 20,
    SharedMemoryMap = 21,
    SharedMemoryShare = 22,
    ProcessCreate = 23,
    ProcessExit = 24,
    ProcessWait = 25,
    TimeGet = 26,
    TimeSleep = 27,
    TimerSet = 28,
    AuditLog = 29,
    DebugPrint = 30,
}

/// Total number of defined syscalls.
pub const SYSCALL_COUNT: usize = 31;

/// Error codes returned in x0 (negative values).
///
/// These match POSIX errno conventions where applicable.
/// Per ipc.md §3.2.
#[repr(i64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum IpcError {
    Etimedout = -1,
    Epipe = -2,
    Eagain = -3,
    Ecanceled = -4,
    Eacces = -5,
    Eperm = -6,
    Enospc = -7,
    Eproto = -8,
    Enotsup = -9,
    EcapDormant = -10,
    Eexist = -11,
    Einval = -12,
    Enomem = -13,
}

/// Number of defined IPC error codes.
pub const IPC_ERROR_COUNT: usize = 13;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_count_matches_enum() {
        // DebugPrint is the last syscall at index 30; count = 31.
        assert_eq!(Syscall::DebugPrint as u64, (SYSCALL_COUNT - 1) as u64);
    }

    #[test]
    fn syscall_first_is_zero() {
        assert_eq!(Syscall::IpcCall as u64, 0);
    }

    #[test]
    fn syscall_values_contiguous() {
        // IPC group: 0-5
        assert_eq!(Syscall::IpcCall as u64, 0);
        assert_eq!(Syscall::IpcSend as u64, 1);
        assert_eq!(Syscall::IpcRecv as u64, 2);
        assert_eq!(Syscall::IpcReply as u64, 3);
        assert_eq!(Syscall::IpcCancel as u64, 4);
        assert_eq!(Syscall::IpcSelect as u64, 5);
    }

    #[test]
    fn syscall_channel_group() {
        assert_eq!(Syscall::ChannelCreate as u64, 6);
        assert_eq!(Syscall::ChannelDestroy as u64, 7);
        assert_eq!(Syscall::RingChannelCreate as u64, 8);
        assert_eq!(Syscall::RingChannelDestroy as u64, 9);
    }

    #[test]
    fn syscall_notification_group() {
        assert_eq!(Syscall::NotificationCreate as u64, 10);
        assert_eq!(Syscall::NotificationSignal as u64, 11);
        assert_eq!(Syscall::NotificationWait as u64, 12);
    }

    #[test]
    fn syscall_capability_group() {
        assert_eq!(Syscall::CapabilityTransfer as u64, 14);
        assert_eq!(Syscall::CapabilityAttenuate as u64, 15);
        assert_eq!(Syscall::CapabilityRevoke as u64, 16);
        assert_eq!(Syscall::CapabilityList as u64, 17);
    }

    #[test]
    fn syscall_memory_group() {
        assert_eq!(Syscall::MemoryMap as u64, 18);
        assert_eq!(Syscall::MemoryUnmap as u64, 19);
        assert_eq!(Syscall::SharedMemoryCreate as u64, 20);
        assert_eq!(Syscall::SharedMemoryMap as u64, 21);
        assert_eq!(Syscall::SharedMemoryShare as u64, 22);
    }

    #[test]
    fn syscall_process_group() {
        assert_eq!(Syscall::ProcessCreate as u64, 23);
        assert_eq!(Syscall::ProcessExit as u64, 24);
        assert_eq!(Syscall::ProcessWait as u64, 25);
    }

    #[test]
    fn syscall_time_group() {
        assert_eq!(Syscall::TimeGet as u64, 26);
        assert_eq!(Syscall::TimeSleep as u64, 27);
        assert_eq!(Syscall::TimerSet as u64, 28);
    }

    #[test]
    fn syscall_debug_group() {
        assert_eq!(Syscall::AuditLog as u64, 29);
        assert_eq!(Syscall::DebugPrint as u64, 30);
    }

    // --- IpcError tests ---

    #[test]
    fn ipc_errors_all_negative() {
        assert!((IpcError::Etimedout as i64) < 0);
        assert!((IpcError::Epipe as i64) < 0);
        assert!((IpcError::Eagain as i64) < 0);
        assert!((IpcError::Ecanceled as i64) < 0);
        assert!((IpcError::Eacces as i64) < 0);
        assert!((IpcError::Eperm as i64) < 0);
        assert!((IpcError::Enospc as i64) < 0);
        assert!((IpcError::Eproto as i64) < 0);
        assert!((IpcError::Enotsup as i64) < 0);
        assert!((IpcError::EcapDormant as i64) < 0);
        assert!((IpcError::Eexist as i64) < 0);
    }

    #[test]
    fn ipc_error_values_unique() {
        let values = [
            IpcError::Etimedout as i64,
            IpcError::Epipe as i64,
            IpcError::Eagain as i64,
            IpcError::Ecanceled as i64,
            IpcError::Eacces as i64,
            IpcError::Eperm as i64,
            IpcError::Enospc as i64,
            IpcError::Eproto as i64,
            IpcError::Enotsup as i64,
            IpcError::EcapDormant as i64,
            IpcError::Eexist as i64,
        ];
        for i in 0..values.len() {
            for j in (i + 1)..values.len() {
                assert_ne!(
                    values[i], values[j],
                    "duplicate error codes at {} and {}",
                    i, j
                );
            }
        }
    }

    #[test]
    fn ipc_error_range() {
        // All errors in -1..-11 range.
        assert_eq!(IpcError::Etimedout as i64, -1);
        assert_eq!(IpcError::Eexist as i64, -11);
    }

    #[test]
    fn ipc_error_count_matches() {
        assert_eq!(IPC_ERROR_COUNT, 13);
        // Enomem is the last at -13.
        assert_eq!(-(IpcError::Enomem as i64) as usize, IPC_ERROR_COUNT);
    }

    #[test]
    fn ipc_error_copy_clone() {
        let e = IpcError::Etimedout;
        let e2 = e;
        assert_eq!(e, e2);
    }
}
