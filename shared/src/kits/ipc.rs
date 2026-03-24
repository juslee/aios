//! IPC Kit — channel operations, notifications, select, and shared memory.
//!
//! Architecture reference: `docs/kits/kernel/ipc.md`

use crate::cap::Capability;
use crate::ipc::{ChannelId, NotificationId, RawMessage, SelectEntry, SharedMemoryId};
use crate::syscall::IpcError;
use crate::VirtAddr;

// Re-export IPC types and constants so consumers can import via the Kit module.
pub use crate::ipc::{
    DEFAULT_TIMEOUT_TICKS, MAX_CHANNELS, MAX_MESSAGE_SIZE, MAX_NOTIFICATIONS, MAX_SELECT_ENTRIES,
    MAX_SHARED_MAPPINGS, MAX_SHARED_REGIONS, RING_CAPACITY,
};

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
            IpcError::Eperm => IpcKitError::SharedMemoryError {
                reason: "operation not permitted",
            },
            IpcError::Enospc => IpcKitError::SharedMemoryError {
                reason: "out of space",
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
// Kit traits
// ---------------------------------------------------------------------------

/// Channel lifecycle and message-passing operations.
///
/// Covers synchronous call/reply, asynchronous send, and blocking receive.
pub trait ChannelOps {
    /// Create a new IPC channel. Returns the channel ID.
    fn channel_create(&mut self) -> Result<ChannelId, IpcKitError>;

    /// Destroy an IPC channel. Wakes any blocked threads with an error.
    fn channel_destroy(&mut self, id: ChannelId) -> Result<(), IpcKitError>;

    /// Fire-and-forget send of a message on a channel.
    fn send(&self, id: ChannelId, msg: &RawMessage) -> Result<(), IpcKitError>;

    /// Blocking receive on a channel. Returns the received message.
    fn recv(&self, id: ChannelId, timeout_ticks: u64) -> Result<RawMessage, IpcKitError>;

    /// Synchronous call: send a request and block until a reply arrives.
    fn call(
        &self,
        id: ChannelId,
        request: &RawMessage,
        timeout_ticks: u64,
    ) -> Result<RawMessage, IpcKitError>;

    /// Reply to a pending call on the specified channel.
    fn reply(&self, id: ChannelId, msg: &RawMessage) -> Result<(), IpcKitError>;
}

/// Notification object operations (seL4-style bitmap signals).
pub trait NotificationOps {
    /// Create a new notification object.
    fn notification_create(&mut self) -> Result<NotificationId, IpcKitError>;

    /// Atomically OR `bits` into the notification word, waking matched waiters.
    fn signal(&self, id: NotificationId, bits: u64) -> Result<(), IpcKitError>;

    /// Wait for masked bits on a notification. Returns the matched bits.
    fn wait(&self, id: NotificationId, mask: u64, timeout_ticks: u64) -> Result<u64, IpcKitError>;
}

/// Multi-wait on channels and notifications.
pub trait SelectOps {
    /// Block until one of the entries is ready, or timeout expires.
    ///
    /// Returns `(ready_index, matched_bits)`. For channel entries,
    /// `matched_bits` is 0.
    fn select(
        &self,
        entries: &[SelectEntry],
        timeout_ticks: u64,
    ) -> Result<(usize, u64), IpcKitError>;
}

/// Shared memory region lifecycle.
pub trait SharedMemoryOps {
    /// Create a new shared memory region of the specified size.
    ///
    /// `flags` encodes permission bits (bit 0 = read, bit 1 = write,
    /// bit 2 = execute, bit 3 = user). W^X is enforced.
    fn shmem_create(&mut self, size: usize, flags: u64) -> Result<SharedMemoryId, IpcKitError>;

    /// Map a shared memory region into the caller's address space.
    ///
    /// `vaddr` is a hint (the kernel may choose the actual address).
    fn shmem_map(
        &mut self,
        id: SharedMemoryId,
        vaddr: VirtAddr,
        flags: u64,
    ) -> Result<(), IpcKitError>;

    /// Unmap a shared memory region from the caller's address space.
    fn shmem_unmap(&mut self, id: SharedMemoryId) -> Result<(), IpcKitError>;

    /// Destroy a shared memory region, unmapping all mappings.
    fn shmem_destroy(&mut self, id: SharedMemoryId) -> Result<(), IpcKitError>;
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
        // Verify each IpcError maps to the correct IpcKitError variant.
        // Field values are placeholders — we check variant kind via matches!.
        assert!(matches!(
            IpcKitError::from(IpcError::Etimedout),
            IpcKitError::Timeout { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Epipe),
            IpcKitError::InvalidChannel { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Eagain),
            IpcKitError::ChannelFull { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Ecanceled),
            IpcKitError::Cancelled
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Eacces),
            IpcKitError::CapabilityDenied { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Eperm),
            IpcKitError::SharedMemoryError { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Enospc),
            IpcKitError::SharedMemoryError { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Eproto),
            IpcKitError::NoReply
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Enotsup),
            IpcKitError::SharedMemoryError { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::EcapDormant),
            IpcKitError::CapabilityDenied { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Eexist),
            IpcKitError::SharedMemoryError { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Einval),
            IpcKitError::InvalidChannel { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Enomem),
            IpcKitError::SharedMemoryError { .. }
        ));
    }

    // -- Round-trip: IpcKitError -> IpcError -> IpcKitError --

    #[test]
    fn ipc_kit_error_round_trip_preserves_variant_kind() {
        // Only variants with 1:1 mappings survive round-trip.
        // MessageTooLarge -> Enospc -> SharedMemoryError (lossy — excluded).
        // SharedMemoryError -> Eperm -> SharedMemoryError (survives now).
        assert!(matches!(
            IpcKitError::from(IpcError::from(IpcKitError::InvalidChannel {
                id: ChannelId(42)
            })),
            IpcKitError::InvalidChannel { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::from(IpcKitError::ChannelFull {
                id: ChannelId(7),
                capacity: 16
            })),
            IpcKitError::ChannelFull { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::from(IpcKitError::Timeout {
                elapsed_ticks: 5000
            })),
            IpcKitError::Timeout { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::from(IpcKitError::Cancelled)),
            IpcKitError::Cancelled
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::from(IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate
            })),
            IpcKitError::CapabilityDenied { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::from(IpcKitError::SharedMemoryError {
                reason: "test"
            })),
            IpcKitError::SharedMemoryError { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::from(IpcKitError::NoReply)),
            IpcKitError::NoReply
        ));
    }

    #[test]
    fn ipc_kit_error_message_too_large_lossy_round_trip() {
        // MessageTooLarge maps to Enospc, which now maps back to SharedMemoryError.
        // This is expected — the conversion is intentionally lossy for this variant.
        let orig = IpcKitError::MessageTooLarge {
            size: 500,
            max: 256,
        };
        let ipc_err = IpcError::from(orig);
        assert_eq!(ipc_err, IpcError::Enospc);
        let back = IpcKitError::from(ipc_err);
        assert!(matches!(back, IpcKitError::SharedMemoryError { .. }));
    }

    // -- Trait dyn-compatibility --

    fn _assert_channel_ops_dyn(_: &dyn ChannelOps) {}
    fn _assert_notification_ops_dyn(_: &dyn NotificationOps) {}
    fn _assert_select_ops_dyn(_: &dyn SelectOps) {}
    fn _assert_shared_memory_ops_dyn(_: &dyn SharedMemoryOps) {}

    #[test]
    fn traits_are_dyn_compatible() {
        // Compilation of the above assertion functions is the real test.
        // If any trait is not object-safe, the functions above won't compile.
    }

    // -- Constants accessible from Kit module --

    #[test]
    fn kit_constants_accessible() {
        // Verify constants are re-exported from the Kit module.
        assert_eq!(super::MAX_CHANNELS, 128);
        assert_eq!(super::DEFAULT_TIMEOUT_TICKS, 5_000);
        assert_eq!(super::MAX_NOTIFICATIONS, 64);
        assert_eq!(super::MAX_SELECT_ENTRIES, 8);
        assert_eq!(super::MAX_SHARED_REGIONS, 64);
        assert_eq!(super::MAX_SHARED_MAPPINGS, 8);
    }

    #[test]
    fn kit_message_constants() {
        // MAX_MESSAGE_SIZE and RING_CAPACITY are used in trait impls.
        assert_eq!(MAX_MESSAGE_SIZE, 256);
        assert_eq!(RING_CAPACITY, 16);
    }

    // -- IpcKitError -> IpcError: every variant maps to a specific code --

    #[test]
    fn ipc_kit_error_to_i64_via_ipc_error() {
        assert_eq!(
            IpcError::from(IpcKitError::InvalidChannel { id: ChannelId(0) }) as i64,
            -2
        );
        assert_eq!(
            IpcError::from(IpcKitError::ChannelFull {
                id: ChannelId(0),
                capacity: 16
            }) as i64,
            -3
        );
        assert_eq!(
            IpcError::from(IpcKitError::Timeout { elapsed_ticks: 0 }) as i64,
            -1
        );
        assert_eq!(IpcError::from(IpcKitError::Cancelled) as i64, -4);
        assert_eq!(
            IpcError::from(IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate
            }) as i64,
            -5
        );
        assert_eq!(
            IpcError::from(IpcKitError::SharedMemoryError { reason: "x" }) as i64,
            -6
        );
        assert_eq!(
            IpcError::from(IpcKitError::MessageTooLarge { size: 0, max: 256 }) as i64,
            -7
        );
        assert_eq!(IpcError::from(IpcKitError::NoReply) as i64, -8);
    }

    // -- Backward compatibility: existing ipc types still accessible --

    #[test]
    fn backward_compat_ipc_types() {
        // Verify types used in trait signatures are the same as shared::ipc types.
        let ch = ChannelId(42);
        let ch2: crate::ipc::ChannelId = ch;
        assert_eq!(ch, ch2);

        let nid = NotificationId(7);
        let nid2: crate::ipc::NotificationId = nid;
        assert_eq!(nid, nid2);

        let sid = SharedMemoryId(3);
        let sid2: crate::ipc::SharedMemoryId = sid;
        assert_eq!(sid, sid2);
    }
}
