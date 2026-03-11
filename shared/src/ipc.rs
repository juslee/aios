//! IPC-related shared types, constants, and validation helpers.
//!
//! Per ipc.md §4.1–4.3.

use crate::sched::ThreadId;

// ---------------------------------------------------------------------------
// IPC constants
// ---------------------------------------------------------------------------

/// Maximum channels system-wide.
pub const MAX_CHANNELS: usize = 128;

/// Maximum messages in a channel's ring buffer.
pub const RING_CAPACITY: usize = 16;

/// Maximum inline message payload (bytes).
pub const MAX_MESSAGE_SIZE: usize = 256;

/// Default IPC timeout in ticks (5 seconds at 1 kHz).
pub const DEFAULT_TIMEOUT_TICKS: u64 = 5_000;

/// Maximum priority inheritance depth (ipc.md §9.2).
///
/// Bounds transitive inheritance chains to prevent runaway elevation.
/// When thread A calls B calls C calls D..., each hop increments the
/// depth counter. At depth 8, no further inheritance propagation occurs.
pub const MAX_INHERITANCE_DEPTH: u8 = 8;

/// Maximum shared memory regions system-wide.
pub const MAX_SHARED_REGIONS: usize = 64;

/// Maximum per-region mappings (processes that can map the same region).
pub const MAX_SHARED_MAPPINGS: usize = 8;

/// Maximum notification objects system-wide.
pub const MAX_NOTIFICATIONS: usize = 64;

/// Maximum waiters per notification object.
pub const MAX_WAITERS_PER_NOTIFICATION: usize = 8;

// ---------------------------------------------------------------------------
// Channel identity
// ---------------------------------------------------------------------------

/// Unique channel identifier (index into CHANNEL_TABLE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelId(pub u32);

/// Unique shared memory region identifier (index into SHARED_REGION_TABLE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedMemoryId(pub u32);

/// Unique notification object identifier (index into NOTIFICATION_TABLE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationId(pub u32);

// ---------------------------------------------------------------------------
// Endpoint state
// ---------------------------------------------------------------------------

/// State of a channel endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointState {
    Active,
    Dead,
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// Inline IPC message with fixed-size payload.
#[derive(Clone)]
pub struct RawMessage {
    /// Thread that sent this message.
    pub sender: ThreadId,
    /// Inline payload.
    pub data: [u8; MAX_MESSAGE_SIZE],
    /// Actual length of payload (0..=MAX_MESSAGE_SIZE).
    pub len: usize,
}

impl RawMessage {
    /// Empty message constant (for array initialization).
    pub const EMPTY: Self = Self {
        sender: ThreadId(0),
        data: [0; MAX_MESSAGE_SIZE],
        len: 0,
    };
}

// ---------------------------------------------------------------------------
// User VA validation
// ---------------------------------------------------------------------------

/// Upper bound of the user virtual address space (exclusive).
///
/// AArch64 convention: addresses below 0x0000_8000_0000_0000 belong to user
/// space (TTBR0), addresses at or above belong to kernel space (TTBR1).
pub const USER_VA_LIMIT: usize = 0x0000_8000_0000_0000;

/// Validate that a (ptr, len) range lies entirely within user VA space.
///
/// Returns false if:
/// - `ptr + len` overflows
/// - `ptr` is in kernel space (>= USER_VA_LIMIT)
/// - `ptr + len` extends into kernel space (> USER_VA_LIMIT)
pub fn validate_user_va(ptr: usize, len: usize) -> bool {
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    ptr < USER_VA_LIMIT && end <= USER_VA_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- IPC constants tests ---

    #[test]
    fn max_channels_is_power_of_two() {
        assert!(MAX_CHANNELS.is_power_of_two());
    }

    #[test]
    fn ring_capacity_is_power_of_two() {
        assert!(RING_CAPACITY.is_power_of_two());
    }

    #[test]
    fn max_message_size_is_power_of_two() {
        assert!(MAX_MESSAGE_SIZE.is_power_of_two());
    }

    #[test]
    fn default_timeout_is_5_seconds() {
        assert_eq!(DEFAULT_TIMEOUT_TICKS, 5_000);
    }

    #[test]
    fn max_inheritance_depth_is_8() {
        assert_eq!(MAX_INHERITANCE_DEPTH, 8);
    }

    #[test]
    fn max_inheritance_depth_fits_in_u8() {
        // Ensure the constant is small enough to track in a single byte.
        assert!(MAX_INHERITANCE_DEPTH <= u8::MAX);
    }

    #[test]
    fn inheritance_depth_bounding() {
        // Simulates the kernel's `(depth + 1).min(MAX_INHERITANCE_DEPTH)` logic.
        // At depth 0 (fresh call): increments to 1.
        assert_eq!((0u8 + 1).min(MAX_INHERITANCE_DEPTH), 1);
        // At depth 7: increments to 8 (the max).
        assert_eq!((7u8 + 1).min(MAX_INHERITANCE_DEPTH), 8);
        // At depth 8 (already max): stays at 8.
        assert_eq!((MAX_INHERITANCE_DEPTH + 1).min(MAX_INHERITANCE_DEPTH), 8);
    }

    #[test]
    fn inheritance_depth_saturating_sub_on_reply() {
        // Simulates the kernel's `depth.saturating_sub(1)` on reply switch.
        assert_eq!(MAX_INHERITANCE_DEPTH.saturating_sub(1), 7);
        assert_eq!(1u8.saturating_sub(1), 0);
        assert_eq!(0u8.saturating_sub(1), 0); // No underflow.
    }

    // --- SharedMemoryId tests ---

    #[test]
    fn shared_memory_id_copy_clone() {
        let id = SharedMemoryId(7);
        let id2 = id;
        assert_eq!(id, id2);
    }

    #[test]
    fn shared_memory_id_equality() {
        assert_eq!(SharedMemoryId(0), SharedMemoryId(0));
        assert_ne!(SharedMemoryId(0), SharedMemoryId(1));
    }

    #[test]
    fn shared_memory_id_max_valid() {
        let id = SharedMemoryId((MAX_SHARED_REGIONS - 1) as u32);
        assert_eq!(id.0 as usize, MAX_SHARED_REGIONS - 1);
    }

    // --- NotificationId tests ---

    #[test]
    fn notification_id_copy_clone() {
        let id = NotificationId(3);
        let id2 = id;
        assert_eq!(id, id2);
    }

    #[test]
    fn notification_id_equality() {
        assert_eq!(NotificationId(0), NotificationId(0));
        assert_ne!(NotificationId(0), NotificationId(1));
    }

    #[test]
    fn notification_id_max_valid() {
        let id = NotificationId((MAX_NOTIFICATIONS - 1) as u32);
        assert_eq!(id.0 as usize, MAX_NOTIFICATIONS - 1);
    }

    // --- Shared memory / notification constants ---

    #[test]
    fn max_shared_regions_is_power_of_two() {
        assert!(MAX_SHARED_REGIONS.is_power_of_two());
    }

    #[test]
    fn max_notifications_is_power_of_two() {
        assert!(MAX_NOTIFICATIONS.is_power_of_two());
    }

    #[test]
    fn max_shared_mappings_bounded() {
        assert!(MAX_SHARED_MAPPINGS <= 16);
        assert!(MAX_SHARED_MAPPINGS > 0);
    }

    #[test]
    fn max_waiters_per_notification_bounded() {
        assert!(MAX_WAITERS_PER_NOTIFICATION <= 16);
        assert!(MAX_WAITERS_PER_NOTIFICATION > 0);
    }

    // --- ChannelId tests ---

    #[test]
    fn channel_id_copy_clone() {
        let ch = ChannelId(42);
        let ch2 = ch;
        assert_eq!(ch, ch2);
    }

    #[test]
    fn channel_id_equality() {
        assert_eq!(ChannelId(0), ChannelId(0));
        assert_ne!(ChannelId(0), ChannelId(1));
    }

    #[test]
    fn channel_id_max_valid() {
        let ch = ChannelId((MAX_CHANNELS - 1) as u32);
        assert_eq!(ch.0 as usize, MAX_CHANNELS - 1);
    }

    // --- EndpointState tests ---

    #[test]
    fn endpoint_state_equality() {
        assert_eq!(EndpointState::Active, EndpointState::Active);
        assert_eq!(EndpointState::Dead, EndpointState::Dead);
        assert_ne!(EndpointState::Active, EndpointState::Dead);
    }

    #[test]
    fn endpoint_state_copy() {
        let s = EndpointState::Active;
        let s2 = s;
        assert_eq!(s, s2);
    }

    // --- RawMessage tests ---

    #[test]
    fn raw_message_empty_is_zero() {
        let msg = RawMessage::EMPTY;
        assert_eq!(msg.sender, ThreadId(0));
        assert_eq!(msg.len, 0);
        assert!(msg.data.iter().all(|&b| b == 0));
    }

    #[test]
    fn raw_message_payload_capacity() {
        assert_eq!(RawMessage::EMPTY.data.len(), MAX_MESSAGE_SIZE);
    }

    #[test]
    fn raw_message_clone() {
        let mut msg = RawMessage::EMPTY;
        msg.sender = ThreadId(42);
        msg.data[0] = 0xFF;
        msg.len = 1;

        let msg2 = msg.clone();
        assert_eq!(msg2.sender, ThreadId(42));
        assert_eq!(msg2.data[0], 0xFF);
        assert_eq!(msg2.len, 1);
    }

    #[test]
    fn raw_message_fill_max() {
        let mut msg = RawMessage::EMPTY;
        msg.data.fill(0xAA);
        msg.len = MAX_MESSAGE_SIZE;
        assert_eq!(msg.len, 256);
        assert!(msg.data.iter().all(|&b| b == 0xAA));
    }

    // --- User VA validation tests ---

    #[test]
    fn user_va_valid_range() {
        assert!(validate_user_va(0x400000, 4096));
        assert!(validate_user_va(0, 256));
        assert!(validate_user_va(USER_VA_LIMIT - 1, 1));
    }

    #[test]
    fn user_va_zero_len() {
        assert!(validate_user_va(0, 0));
        assert!(validate_user_va(0x1000, 0));
        assert!(validate_user_va(USER_VA_LIMIT - 1, 0));
    }

    #[test]
    fn user_va_at_boundary() {
        assert!(!validate_user_va(USER_VA_LIMIT, 0));
        assert!(!validate_user_va(USER_VA_LIMIT, 1));
    }

    #[test]
    fn user_va_crosses_boundary() {
        assert!(!validate_user_va(USER_VA_LIMIT - 1, 2));
        assert!(!validate_user_va(USER_VA_LIMIT - 100, 200));
    }

    #[test]
    fn user_va_kernel_pointer() {
        assert!(!validate_user_va(0xFFFF_0000_0000_0000, 1));
        assert!(!validate_user_va(0xFFFF_FFFF_FFFF_FFFF, 0));
    }

    #[test]
    fn user_va_overflow() {
        assert!(!validate_user_va(usize::MAX, 1));
        assert!(!validate_user_va(usize::MAX - 10, 100));
        assert!(!validate_user_va(1, usize::MAX));
    }

    #[test]
    fn user_va_large_valid() {
        assert!(validate_user_va(0, USER_VA_LIMIT));
        assert!(validate_user_va(0, USER_VA_LIMIT - 1));
    }
}
