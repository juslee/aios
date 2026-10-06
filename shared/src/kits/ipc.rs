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
///
/// `From<IpcError>` (and [`IpcKitError::from_code`], which decodes a raw
/// code first) maps each errno to the least specific variant that is correct
/// for every kernel path returning it, except EPERM and ENOSPC, for which no
/// variant is correct on every path, and EPROTO, whose default is kept by
/// choice (see `From<IpcError>`). A Kit wrapper that knows more, the channel
/// id, which capability the kernel checked, or what an errno means for its
/// operation, overrides that default (docs/kits/kernel/ipc.md §6).
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
    /// A shared memory operation failed: the region does not exist, the
    /// caller has no mapping of it, it is already mapped, or the calling
    /// thread has no process.
    SharedMemoryError { reason: &'static str },
    /// The message payload exceeds the maximum size.
    MessageTooLarge { size: usize, max: usize },
    /// A synchronous call completed but no reply was received.
    NoReply,
    /// The operation would have to block and did not: a non-blocking
    /// receive found no message, or another thread is already receiving on
    /// or calling through the channel.
    WouldBlock,
    /// The caller's process is SUSPENDED by the behavioral gate
    /// (docs/kernel/ipc.md §9.1). It may still hold every capability.
    Suspended,
    /// An argument is invalid (an out-of-range id or a missing notification,
    /// an undefined flag bit, a W^X request or flags beyond a region's
    /// maximum, a size above a fixed limit, a bad buffer, or an empty,
    /// oversized or unknown-kind select set; the full list is the EINVAL row
    /// of docs/kernel/ipc.md §3.2), or the caller's state does not allow the
    /// call: no current thread or process, or a reply with no pending call.
    InvalidArgument { reason: &'static str },
    /// A table, queue or memory pool is full.
    ResourceExhausted { reason: &'static str },
    /// The operation is not available.
    Unsupported,
    /// The channel or shared memory region the call names is gone: it was
    /// never created or has been destroyed, or a channel endpoint is dead.
    ObjectGone,
}

impl IpcKitError {
    /// Decode a raw kernel return code through the one errno table:
    /// `IpcError::try_from`, then `From<IpcError>`.
    ///
    /// Every kernel IPC path returns an `IpcError` code, so the fallback for
    /// any other value, `InvalidArgument { reason: "unknown error code" }`,
    /// is only reached through a kernel bug.
    pub fn from_code(code: i64) -> IpcKitError {
        match IpcError::try_from(code) {
            Ok(e) => IpcKitError::from(e),
            Err(_) => IpcKitError::InvalidArgument {
                reason: "unknown error code",
            },
        }
    }
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
            IpcKitError::CapabilityDenied { .. } => IpcError::Eperm,
            IpcKitError::SharedMemoryError { .. } => IpcError::Einval,
            IpcKitError::MessageTooLarge { .. } => IpcError::Enospc,
            IpcKitError::NoReply => IpcError::Eproto,
            IpcKitError::WouldBlock => IpcError::Eagain,
            IpcKitError::Suspended => IpcError::Eacces,
            IpcKitError::InvalidArgument { .. } => IpcError::Einval,
            IpcKitError::ResourceExhausted { .. } => IpcError::Enomem,
            IpcKitError::Unsupported => IpcError::Enotsup,
            IpcKitError::ObjectGone => IpcError::Epipe,
        }
    }
}

impl From<IpcError> for IpcKitError {
    /// Convert a syscall-level `IpcError` into an `IpcKitError`.
    ///
    /// **Note:** Field values (e.g. `required`, `elapsed_ticks`) are
    /// placeholders — only the error *kind* survives the conversion. No code
    /// decodes to a variant that carries a channel id.
    ///
    /// Each errno maps to the least specific variant that is correct for
    /// every kernel path that returns it:
    /// - EACCES is the behavioral gate's SUSPENDED code, so it maps to
    ///   `Suspended`, not to `CapabilityDenied`: a suspended agent may still
    ///   hold every capability.
    /// - EAGAIN is "would block" (a full ring on send is one case), so it
    ///   maps to `WouldBlock`; `send` overrides it to `ChannelFull`.
    /// - EINVAL comes from every subsystem, so it maps to `InvalidArgument`;
    ///   channel wrappers override it to `InvalidChannel { id }`.
    /// - EPIPE means the object the call names is gone: a destroyed channel
    ///   or dead endpoint on the channel paths, a missing region on the
    ///   shared memory paths (SharedMemoryMap, SharedMemoryShare, MemoryUnmap
    ///   of a shared window address, and the in-kernel shared_memory_unmap
    ///   behind `shmem_unmap`). It maps
    ///   to `ObjectGone`, which names no id; channel wrappers override it to
    ///   `InvalidChannel { id }`, shared memory wrappers to
    ///   `SharedMemoryError`.
    /// - EEXIST maps to `SharedMemoryError`: among IPC Kit paths only
    ///   `shared_memory_map` returns it.
    ///
    /// Two codes have no variant that is correct for every path, so the
    /// table's variant is wrong for some paths, and wrappers for those paths
    /// must override it:
    /// - EPERM maps to `CapabilityDenied`, since every capability check
    ///   returns it (docs/kernel/ipc.md §3.2), but some EPERMs are not a
    ///   missing capability. These syscalls check no capability but return
    ///   EPERM when the caller has no current thread or no process:
    ///   IpcReply, NotificationCreate, MemoryMap, MemoryUnmap,
    ///   CapabilityAttenuate, CapabilityRevoke, CapabilityList, ProcessExit,
    ///   ProcessWait, AuditLog and SharedMemoryShare. (NotificationSignal,
    ///   DebugPrint and TimeGet look up no thread or process, TimeSleep with
    ///   no current thread returns 0 without sleeping, and NotificationWait
    ///   returns EINVAL without one.) Some of them also
    ///   return it for other reasons: an unmap of a region the caller has
    ///   not mapped (MemoryUnmap of a shared window address, or the
    ///   in-kernel shared_memory_unmap behind `shmem_unmap`),
    ///   SharedMemoryShare from a caller that is not the region's creator or
    ///   to a target pid with no process, and ProcessWait for a child pid
    ///   with no process or after a wake that finds no exit code. `reply`,
    ///   `notification_create` and `shmem_unmap` override it; the other
    ///   syscalls named here have no Kit wrapper, so a plain decode of their
    ///   EPERM reads as `CapabilityDenied`.
    /// - ENOSPC maps to `ResourceExhausted`, but several paths return it for
    ///   a request above a fixed limit, which releasing objects or retrying
    ///   cannot fix: a payload above `MAX_MESSAGE_SIZE` (IpcSend, IpcCall,
    ///   IpcReply), an IpcCall or IpcRecv receive length above it, a
    ///   DebugPrint above 256 bytes and an AuditLog event above 48 bytes. Kit
    ///   wrappers reject an oversized payload as `MessageTooLarge` before the
    ///   call and never pass a longer receive length; DebugPrint and AuditLog
    ///   have no Kit wrapper, so a plain decode of their ENOSPC reads as
    ///   `ResourceExhausted`.
    ///
    /// EPROTO is kept as `NoReply` by choice. Its only kernel path today is
    /// `ipc_reply` with no pending call, the replier's side, where
    /// `InvalidArgument` would be correct; `NoReply` stays the default so
    /// that it survives the round trip through EPROTO, and `reply`
    /// overrides it to `InvalidArgument`.
    fn from(e: IpcError) -> IpcKitError {
        match e {
            IpcError::Etimedout => IpcKitError::Timeout { elapsed_ticks: 0 },
            IpcError::Epipe => IpcKitError::ObjectGone,
            IpcError::Eagain => IpcKitError::WouldBlock,
            IpcError::Ecanceled => IpcKitError::Cancelled,
            IpcError::Eacces => IpcKitError::Suspended,
            IpcError::Eperm => IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            IpcError::Enospc => IpcKitError::ResourceExhausted {
                reason: "out of space",
            },
            IpcError::Eproto => IpcKitError::NoReply,
            IpcError::Enotsup => IpcKitError::Unsupported,
            IpcError::EcapDormant => IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            IpcError::Eexist => IpcKitError::SharedMemoryError {
                reason: "already exists",
            },
            IpcError::Einval => IpcKitError::InvalidArgument {
                reason: "invalid argument",
            },
            IpcError::Enomem => IpcKitError::ResourceExhausted {
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
    /// `flags` encodes the region's maximum permissions: bit 0 = read,
    /// bit 1 = write, bit 2 = execute (`crate::syscall::MEMORY_FLAGS_MASK`).
    /// Any other bit is an error, including bit 3 (user), which the kernel
    /// sets itself on every user mapping. W^X is enforced.
    fn shmem_create(&mut self, size: usize, flags: u64) -> Result<SharedMemoryId, IpcKitError>;

    /// Map a shared memory region into the caller's address space.
    ///
    /// `vaddr` is a hint (the kernel may choose the actual address). `flags`
    /// takes the same bits as `shmem_create` and must be a subset of the
    /// region's maximum permissions.
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

    /// One value of every variant.
    fn all_variants() -> [IpcKitError; 14] {
        [
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
            IpcKitError::WouldBlock,
            IpcKitError::Suspended,
            IpcKitError::InvalidArgument { reason: "test" },
            IpcKitError::ResourceExhausted { reason: "test" },
            IpcKitError::Unsupported,
            IpcKitError::ObjectGone,
        ]
    }

    #[test]
    fn ipc_kit_error_debug_all_variants() {
        for v in all_variants() {
            let s = format!("{:?}", v);
            assert!(!s.is_empty());
        }
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
        assert_eq!(IpcError::from(IpcKitError::ObjectGone), IpcError::Epipe);
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
        // A missing capability is EPERM (ipc.md §3.1), not EACCES.
        assert_eq!(
            IpcError::from(IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate
            }),
            IpcError::Eperm
        );
        assert_eq!(
            IpcError::from(IpcKitError::SharedMemoryError { reason: "x" }),
            IpcError::Einval
        );
        assert_eq!(
            IpcError::from(IpcKitError::MessageTooLarge {
                size: 300,
                max: 256
            }),
            IpcError::Enospc
        );
        assert_eq!(IpcError::from(IpcKitError::NoReply), IpcError::Eproto);
        assert_eq!(IpcError::from(IpcKitError::WouldBlock), IpcError::Eagain);
        // The behavioral gate's SUSPENDED code is EACCES (ipc.md §9.1).
        assert_eq!(IpcError::from(IpcKitError::Suspended), IpcError::Eacces);
        assert_eq!(
            IpcError::from(IpcKitError::InvalidArgument { reason: "x" }),
            IpcError::Einval
        );
        assert_eq!(
            IpcError::from(IpcKitError::ResourceExhausted { reason: "x" }),
            IpcError::Enomem
        );
        assert_eq!(IpcError::from(IpcKitError::Unsupported), IpcError::Enotsup);
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
        // EPIPE names no channel id: a region path returns it too.
        assert_eq!(IpcKitError::from(IpcError::Epipe), IpcKitError::ObjectGone);
        assert!(matches!(
            IpcKitError::from(IpcError::Eagain),
            IpcKitError::WouldBlock
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Ecanceled),
            IpcKitError::Cancelled
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Eacces),
            IpcKitError::Suspended
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Eperm),
            IpcKitError::CapabilityDenied { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Enospc),
            IpcKitError::ResourceExhausted { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Eproto),
            IpcKitError::NoReply
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Enotsup),
            IpcKitError::Unsupported
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
            IpcKitError::InvalidArgument { .. }
        ));
        assert!(matches!(
            IpcKitError::from(IpcError::Enomem),
            IpcKitError::ResourceExhausted { .. }
        ));
    }

    // -- Raw code -> IpcKitError --

    #[test]
    fn from_code_agrees_with_from_ipc_error_for_every_errno() {
        for e in IpcError::ALL {
            assert_eq!(IpcKitError::from_code(e as i64), IpcKitError::from(e));
        }
    }

    #[test]
    fn from_code_capability_denial_is_capability_denied() {
        // Every kernel capability check returns EPERM (-6).
        assert!(matches!(
            IpcKitError::from_code(-6),
            IpcKitError::CapabilityDenied { .. }
        ));
    }

    #[test]
    fn from_code_unknown_code_is_invalid_argument() {
        for code in [0, 1, -14, -4001, i64::MIN] {
            assert_eq!(
                IpcKitError::from_code(code),
                IpcKitError::InvalidArgument {
                    reason: "unknown error code"
                }
            );
        }
    }

    // -- Round-trip: IpcKitError -> IpcError -> IpcKitError --

    #[test]
    fn ipc_kit_error_round_trip_preserves_variant_kind() {
        // Only variants whose errno maps back to them survive the round trip.
        // Lossy (tested below): InvalidChannel -> Epipe -> ObjectGone,
        // ChannelFull -> Eagain -> WouldBlock,
        // MessageTooLarge -> Enospc -> ResourceExhausted,
        // SharedMemoryError -> Einval -> InvalidArgument.
        let survivors = [
            IpcKitError::ObjectGone,
            IpcKitError::Timeout {
                elapsed_ticks: 5000,
            },
            IpcKitError::Cancelled,
            IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            IpcKitError::NoReply,
            IpcKitError::WouldBlock,
            IpcKitError::Suspended,
            IpcKitError::InvalidArgument { reason: "x" },
            IpcKitError::ResourceExhausted { reason: "x" },
            IpcKitError::Unsupported,
        ];
        for v in survivors {
            let back = IpcKitError::from(IpcError::from(v.clone()));
            assert_eq!(
                core::mem::discriminant(&back),
                core::mem::discriminant(&v),
                "{v:?}"
            );
        }
    }

    #[test]
    fn ipc_kit_error_lossy_round_trips() {
        // Each of these shares its errno with a less specific variant, which
        // is what the errno decodes to. A wrapper with context restores them.
        let back = |v: IpcKitError| IpcKitError::from(IpcError::from(v));
        assert_eq!(
            back(IpcKitError::InvalidChannel { id: ChannelId(42) }),
            IpcKitError::ObjectGone
        );
        assert!(matches!(
            back(IpcKitError::ChannelFull {
                id: ChannelId(7),
                capacity: 16
            }),
            IpcKitError::WouldBlock
        ));
        assert!(matches!(
            back(IpcKitError::MessageTooLarge {
                size: 500,
                max: 256
            }),
            IpcKitError::ResourceExhausted { .. }
        ));
        assert!(matches!(
            back(IpcKitError::SharedMemoryError { reason: "test" }),
            IpcKitError::InvalidArgument { .. }
        ));
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
        assert_eq!(IpcError::from(IpcKitError::ObjectGone) as i64, -2);
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
            -6
        );
        assert_eq!(IpcError::from(IpcKitError::Suspended) as i64, -5);
        assert_eq!(
            IpcError::from(IpcKitError::SharedMemoryError { reason: "x" }) as i64,
            -12
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
