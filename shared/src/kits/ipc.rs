//! IPC Kit — channel operations, notifications, select, and shared memory.
//!
//! Architecture reference: `docs/kits/kernel/ipc.md`

use crate::cap::Capability;
use crate::ipc::{ChannelId, MAX_MESSAGE_SIZE, RING_CAPACITY};
use crate::syscall::IpcError;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by IPC Kit operations.
///
/// `IpcKitError` provides richer, application-level error context than the
/// syscall-level [`IpcError`]. Lossy conversions bridge the two layers:
/// field values become placeholders when converting from `IpcError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcKitError {
    /// The channel does not exist or has been destroyed.
    InvalidChannel { id: ChannelId },
    /// The channel's ring buffer is at capacity.
    ChannelFull { id: ChannelId, capacity: usize },
    /// The operation timed out.
    Timeout { elapsed_ticks: u64 },
    /// The operation was cancelled by the peer.
    Cancelled,
    /// The caller lacks the required capability.
    CapabilityDenied { required: Capability },
    /// A shared memory operation failed.
    SharedMemoryError { reason: &'static str },
    /// The message payload exceeds the maximum size.
    MessageTooLarge { size: usize, max: usize },
    /// A synchronous call completed but no reply was received.
    NoReply,
}

// ---------------------------------------------------------------------------
// IpcKitError <-> IpcError conversions
// ---------------------------------------------------------------------------

impl From<IpcKitError> for IpcError {
    fn from(e: IpcKitError) -> IpcError {
        match e {
            IpcKitError::InvalidChannel { .. } => IpcError::Epipe,
            IpcKitError::ChannelFull { .. } => IpcError::Eagain,
            IpcKitError::Timeout { .. } => IpcError::Etimedout,
            IpcKitError::Cancelled => IpcError::Ecanceled,
            IpcKitError::CapabilityDenied { .. } => IpcError::Eacces,
            IpcKitError::SharedMemoryError { .. } => IpcError::Eperm,
            IpcKitError::MessageTooLarge { .. } => IpcError::Enospc,
            IpcKitError::NoReply => IpcError::Eproto,
        }
    }
}

impl From<IpcError> for IpcKitError {
    /// Convert a syscall-level `IpcError` into an `IpcKitError`.
    ///
    /// **Note:** Field values (e.g. `id`, `required`, `elapsed_ticks`) are
    /// placeholders — only the error *kind* survives the conversion.
    fn from(e: IpcError) -> IpcKitError {
        match e {
            IpcError::Etimedout => IpcKitError::Timeout { elapsed_ticks: 0 },
            IpcError::Epipe => IpcKitError::InvalidChannel { id: ChannelId(0) },
            IpcError::Eagain => IpcKitError::ChannelFull {
                id: ChannelId(0),
                capacity: RING_CAPACITY,
            },
            IpcError::Ecanceled => IpcKitError::Cancelled,
            IpcError::Eacces => IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            IpcError::Eperm => IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            IpcError::Enospc => IpcKitError::MessageTooLarge {
                size: 0,
                max: MAX_MESSAGE_SIZE,
            },
            IpcError::Eproto => IpcKitError::NoReply,
            IpcError::Enotsup => IpcKitError::SharedMemoryError {
                reason: "not supported",
            },
            IpcError::EcapDormant => IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            IpcError::Eexist => IpcKitError::SharedMemoryError {
                reason: "already exists",
            },
            IpcError::Einval => IpcKitError::InvalidChannel { id: ChannelId(0) },
            IpcError::Enomem => IpcKitError::SharedMemoryError {
                reason: "out of memory",
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    // -- IpcKitError --

    #[test]
    fn ipc_kit_error_debug_all_variants() {
        let variants: &[IpcKitError] = &[
            IpcKitError::InvalidChannel { id: ChannelId(0) },
            IpcKitError::ChannelFull {
                id: ChannelId(1),
                capacity: 16,
            },
            IpcKitError::Timeout { elapsed_ticks: 100 },
            IpcKitError::Cancelled,
            IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            IpcKitError::SharedMemoryError {
                reason: "test error",
            },
            IpcKitError::MessageTooLarge {
                size: 512,
                max: 256,
            },
            IpcKitError::NoReply,
        ];
        for v in variants {
            let s = format!("{:?}", v);
            assert!(!s.is_empty());
        }
        assert_eq!(variants.len(), 8);
    }

    #[test]
    fn ipc_kit_error_clone_and_eq() {
        let a = IpcKitError::Cancelled;
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(IpcKitError::Cancelled, IpcKitError::NoReply);
    }

    // -- IpcKitError -> IpcError --

    #[test]
    fn ipc_kit_error_to_ipc_error() {
        assert_eq!(
            IpcError::from(IpcKitError::InvalidChannel { id: ChannelId(5) }),
            IpcError::Epipe
        );
        assert_eq!(
            IpcError::from(IpcKitError::ChannelFull {
                id: ChannelId(0),
                capacity: 16
            }),
            IpcError::Eagain
        );
        assert_eq!(
            IpcError::from(IpcKitError::Timeout { elapsed_ticks: 50 }),
            IpcError::Etimedout
        );
        assert_eq!(IpcError::from(IpcKitError::Cancelled), IpcError::Ecanceled);
        assert_eq!(
            IpcError::from(IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate
            }),
            IpcError::Eacces
        );
        assert_eq!(
            IpcError::from(IpcKitError::SharedMemoryError { reason: "x" }),
            IpcError::Eperm
        );
        assert_eq!(
            IpcError::from(IpcKitError::MessageTooLarge {
                size: 300,
                max: 256
            }),
            IpcError::Enospc
        );
        assert_eq!(IpcError::from(IpcKitError::NoReply), IpcError::Eproto);
    }

    // -- IpcError -> IpcKitError --

    #[test]
    fn ipc_error_to_ipc_kit_error_all_variants() {
        // Verify each IpcError maps to a specific IpcKitError variant kind.
        // Field values are placeholders — we check discriminant only.
        let mapping: &[(IpcError, &str)] = &[
            (IpcError::Etimedout, "Timeout"),
            (IpcError::Epipe, "InvalidChannel"),
            (IpcError::Eagain, "ChannelFull"),
            (IpcError::Ecanceled, "Cancelled"),
            (IpcError::Eacces, "CapabilityDenied"),
            (IpcError::Eperm, "CapabilityDenied"),
            (IpcError::Enospc, "MessageTooLarge"),
            (IpcError::Eproto, "NoReply"),
            (IpcError::Enotsup, "SharedMemoryError"),
            (IpcError::EcapDormant, "CapabilityDenied"),
            (IpcError::Eexist, "SharedMemoryError"),
            (IpcError::Einval, "InvalidChannel"),
            (IpcError::Enomem, "SharedMemoryError"),
        ];
        for (ipc_err, expected_prefix) in mapping {
            let kit_err = IpcKitError::from(*ipc_err);
            let debug = format!("{:?}", kit_err);
            assert!(
                debug.starts_with(expected_prefix),
                "IpcError::{:?} -> {:?}, expected prefix {:?}",
                ipc_err,
                debug,
                expected_prefix
            );
        }
    }

    // -- Round-trip: IpcKitError -> IpcError -> IpcKitError --

    #[test]
    fn ipc_kit_error_round_trip_preserves_variant_kind() {
        // Only variants with 1:1 mappings survive round-trip.
        // SharedMemoryError -> Eperm -> CapabilityDenied (lossy — excluded).
        let originals: &[IpcKitError] = &[
            IpcKitError::InvalidChannel { id: ChannelId(42) },
            IpcKitError::ChannelFull {
                id: ChannelId(7),
                capacity: 16,
            },
            IpcKitError::Timeout {
                elapsed_ticks: 5000,
            },
            IpcKitError::Cancelled,
            IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            IpcKitError::MessageTooLarge {
                size: 500,
                max: 256,
            },
            IpcKitError::NoReply,
        ];
        for orig in originals {
            let ipc_err = IpcError::from(orig.clone());
            let back = IpcKitError::from(ipc_err);
            let orig_debug = format!("{:?}", orig);
            let back_debug = format!("{:?}", back);
            let orig_kind = orig_debug
                .split(|c: char| c == ' ' || c == '{')
                .next()
                .unwrap();
            let back_kind = back_debug
                .split(|c: char| c == ' ' || c == '{')
                .next()
                .unwrap();
            assert_eq!(
                orig_kind, back_kind,
                "round-trip changed variant: {} -> {}",
                orig_debug, back_debug
            );
        }
    }

    #[test]
    fn ipc_kit_error_shared_memory_error_lossy_round_trip() {
        // SharedMemoryError maps to Eperm, which maps back to CapabilityDenied.
        // This is expected — the conversion is intentionally lossy.
        let orig = IpcKitError::SharedMemoryError { reason: "test" };
        let ipc_err = IpcError::from(orig);
        assert_eq!(ipc_err, IpcError::Eperm);
        let back = IpcKitError::from(ipc_err);
        assert!(matches!(back, IpcKitError::CapabilityDenied { .. }));
    }
}
