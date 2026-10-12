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

/// The deadline tick of a wait for `timeout_ticks` ticks that starts at tick
/// `now`, in the form the timeout queue and the notification deadlines store.
///
/// `u64::MAX` means no deadline: the wait is unbounded and the result is
/// `None`. Any other value gives `now + timeout_ticks`, so a timeout taken
/// straight from a syscall register cannot overflow (#217) and never gives a
/// deadline before `now`. A sum that reaches or passes `u64::MAX` is also
/// `None`: `check_timeouts` can never reach tick `u64::MAX` (2^64 ms is about
/// 585 million years), so such a deadline bounds nothing, and calling it
/// "no deadline" keeps the tripwire scans, which treat a `u64::MAX` entry as
/// no waker, consistent with the wait's `timed` flag.
///
/// `0` gives `Some(now)`, a deadline the next timer tick passes. A caller for
/// which 0 means something else decides that before calling: `ipc_call`
/// treats 0 as no deadline, `ipc_recv` as a poll, and `sleep_ticks` returns
/// at once.
pub const fn deadline_after(now: u64, timeout_ticks: u64) -> Option<u64> {
    if timeout_ticks == u64::MAX {
        return None;
    }
    match now.checked_add(timeout_ticks) {
        Some(deadline) if deadline != u64::MAX => Some(deadline),
        _ => None,
    }
}

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
// Id layout (channels and shared memory regions)
// ---------------------------------------------------------------------------

/// Width of the slot field in the low bits of a `ChannelId` or
/// `SharedMemoryId`. The generation fills the 24 bits above it.
pub const ID_SLOT_BITS: u32 = 8;

/// Mask of the slot field (`0xFF`).
pub const ID_SLOT_MASK: u32 = (1 << ID_SLOT_BITS) - 1;

/// Highest generation an id carries (`0xFF_FFFF`). A slot freed at this
/// generation is retired for the rest of the boot (`next_generation`).
pub const ID_GEN_MAX: u32 = (1 << (32 - ID_SLOT_BITS)) - 1;

/// Generation of a slot's first object.
pub const ID_GEN_FIRST: u32 = 1;

/// Generation that marks a retired slot. It is never issued, so no valid id
/// is below `1 << ID_SLOT_BITS`.
pub const ID_GEN_RETIRED: u32 = 0;

// Strict: slot 255 is always malformed, so u32::MAX never decodes to a valid id.
const _: () = assert!(MAX_CHANNELS < 1 << ID_SLOT_BITS);
const _: () = assert!(MAX_SHARED_REGIONS < 1 << ID_SLOT_BITS);

/// Generation after a free. Saturation retires the slot for good: a slot
/// freed at `ID_GEN_MAX`, or one already retired, gets `ID_GEN_RETIRED`.
pub const fn next_generation(g: u32) -> u32 {
    if g == ID_GEN_RETIRED || g >= ID_GEN_MAX {
        ID_GEN_RETIRED
    } else {
        g + 1
    }
}

/// Methods shared by the slot-and-generation id types. `$max` is the table
/// size: slots `0..$max` are valid and `$max..=ID_SLOT_MASK` are malformed.
macro_rules! slot_generation_id {
    ($name:ident, $max:expr, $table:literal) => {
        impl $name {
            #[doc = concat!("The id of the object in slot `slot` of ", $table, " at `generation`.")]
            ///
            /// Kernel allocation only: ids from callers come through
            /// [`Self::from_raw`].
            ///
            /// # Panics
            ///
            /// If `slot` is not a valid slot or `generation` is outside
            /// `ID_GEN_FIRST..=ID_GEN_MAX`. Both come from the kernel's own
            /// allocator, so either is a kernel bug.
            #[inline]
            pub const fn new(slot: usize, generation: u32) -> Self {
                assert!(slot < $max);
                assert!(generation >= ID_GEN_FIRST && generation <= ID_GEN_MAX);
                Self(slot as u32 | generation << ID_SLOT_BITS)
            }

            /// The id a raw value names, as it comes from a register, a wire
            /// struct or a test. It may be malformed or stale; nothing is
            /// checked here.
            #[inline]
            pub const fn from_raw(raw: u32) -> Self {
                Self(raw)
            }

            /// The raw value, `slot | generation << ID_SLOT_BITS`: the form
            /// that crosses registers and wire structs.
            #[inline]
            pub const fn raw(self) -> u32 {
                self.0
            }

            #[doc = concat!("Slot index of this id in ", $table, ".")]
            ///
            /// Returns `None` for a malformed slot (at or above the table
            /// size), at any generation: no object can exist there, and
            /// `u32::MAX` is always malformed. Always index the table through
            /// this method, never through the raw value: ids can come from
            /// untrusted callers, and a raw index past the end of the table
            /// panics the kernel. A `Some` slot says nothing about whether the
            /// object there carries this id's generation.
            #[inline]
            pub const fn index(self) -> Option<usize> {
                let slot = (self.0 & ID_SLOT_MASK) as usize;
                if slot < $max {
                    Some(slot)
                } else {
                    None
                }
            }

            /// The generation field (bits 8–31). `ID_GEN_RETIRED` (0) is
            /// never issued.
            #[inline]
            pub const fn generation(self) -> u32 {
                self.0 >> ID_SLOT_BITS
            }
        }

        /// Prints `slot.generation`, the form log lines use.
        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}.{}", self.0 & ID_SLOT_MASK, self.generation())
            }
        }

        /// Prints `Name(slot.generation)`.
        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self)
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Channel and region identity
// ---------------------------------------------------------------------------

/// Channel identifier: a CHANNEL_TABLE slot and a generation (the id layout
/// above). Build one with `new` (kernel allocation) or `from_raw` (callers),
/// and index the table only through `index`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChannelId(u32);

slot_generation_id!(ChannelId, MAX_CHANNELS, "CHANNEL_TABLE");

/// Shared memory region identifier: a SHARED_REGION_TABLE slot and a
/// generation (the id layout above). Build one with `new` (kernel allocation)
/// or `from_raw` (callers), and index the table only through `index`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SharedMemoryId(u32);

slot_generation_id!(SharedMemoryId, MAX_SHARED_REGIONS, "SHARED_REGION_TABLE");

/// Unique notification object identifier (index into NOTIFICATION_TABLE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationId(pub u32);

// ---------------------------------------------------------------------------
// Select (multi-wait) types
// ---------------------------------------------------------------------------

/// Maximum entries in a single IpcSelect wait set.
pub const MAX_SELECT_ENTRIES: usize = 8;

/// Kind of source in a select wait set (ipc.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectKind {
    /// Wait for a message on a channel.
    Channel(ChannelId),
    /// Wait for notification bits matching a mask.
    Notification(NotificationId, u64),
}

/// A single entry in the IpcSelect wait set.
///
/// This is the kernel's decoded form. It wraps a Rust enum and has no fixed
/// layout, so it never crosses the syscall boundary: EL0 passes
/// [`RawSelectEntry`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectEntry {
    pub kind: SelectKind,
}

/// `RawSelectEntry::kind` for a channel entry: `id` is a `ChannelId` and
/// `mask` is ignored.
pub const SELECT_KIND_CHANNEL: u32 = 0;

/// `RawSelectEntry::kind` for a notification entry: `id` is a
/// `NotificationId` and `mask` selects the bits to wait for.
pub const SELECT_KIND_NOTIFICATION: u32 = 1;

/// One IpcSelect entry as EL0 lays it out in the array that x0 points to
/// (ipc.md §3.1, `IpcSelect`).
///
/// The wire format is this `repr(C)` struct: 16 bytes, `kind` at offset 0,
/// `id` at 4 and `mask` at 8, in the machine's native byte order (little
/// endian on AArch64), with no padding. The kernel copies the whole array into
/// a kernel buffer as bytes, so the user array needs no particular alignment,
/// then reads each entry with [`RawSelectEntry::from_bytes`] and decodes it
/// with `SelectEntry::try_from`, which rejects an unknown `kind` with EINVAL.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSelectEntry {
    /// `SELECT_KIND_CHANNEL` or `SELECT_KIND_NOTIFICATION`.
    pub kind: u32,
    /// Channel or notification id.
    pub id: u32,
    /// Notification bit mask; ignored for a channel entry.
    pub mask: u64,
}

const _: () = assert!(core::mem::size_of::<RawSelectEntry>() == RawSelectEntry::SIZE);
const _: () = assert!(core::mem::offset_of!(RawSelectEntry, kind) == 0);
const _: () = assert!(core::mem::offset_of!(RawSelectEntry, id) == 4);
const _: () = assert!(core::mem::offset_of!(RawSelectEntry, mask) == 8);

impl RawSelectEntry {
    /// Size of one entry on the wire, in bytes.
    pub const SIZE: usize = 16;

    /// Read an entry from its 16 wire bytes (native byte order, the layout
    /// documented on the type).
    pub const fn from_bytes(b: &[u8; Self::SIZE]) -> Self {
        Self {
            kind: u32::from_ne_bytes([b[0], b[1], b[2], b[3]]),
            id: u32::from_ne_bytes([b[4], b[5], b[6], b[7]]),
            mask: u64::from_ne_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]),
        }
    }
}

impl From<SelectEntry> for RawSelectEntry {
    /// Encode an entry in the wire format (the EL0 side of the ABI).
    fn from(entry: SelectEntry) -> Self {
        match entry.kind {
            SelectKind::Channel(ch) => Self {
                kind: SELECT_KIND_CHANNEL,
                id: ch.raw(),
                mask: 0,
            },
            SelectKind::Notification(n, mask) => Self {
                kind: SELECT_KIND_NOTIFICATION,
                id: n.0,
                mask,
            },
        }
    }
}

impl TryFrom<RawSelectEntry> for SelectEntry {
    type Error = i64;

    /// Decode a wire entry. Returns `Err(EINVAL)` for a `kind` other than
    /// `SELECT_KIND_CHANNEL` or `SELECT_KIND_NOTIFICATION`. Ids are not
    /// range-checked here; `ipc_select` does that.
    fn try_from(raw: RawSelectEntry) -> Result<Self, i64> {
        let kind = match raw.kind {
            SELECT_KIND_CHANNEL => SelectKind::Channel(ChannelId::from_raw(raw.id)),
            SELECT_KIND_NOTIFICATION => SelectKind::Notification(NotificationId(raw.id), raw.mask),
            _ => return Err(crate::syscall::IpcError::Einval as i64),
        };
        Ok(SelectEntry { kind })
    }
}

// ---------------------------------------------------------------------------
// Service types
// ---------------------------------------------------------------------------

/// Maximum number of registered services.
pub const MAX_SERVICES: usize = 16;

/// Maximum service name length in bytes.
pub const MAX_SERVICE_NAME_LEN: usize = 32;

/// Service lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Dead,
}

/// Fixed-size service name with length tracking.
///
/// Service names are stored as byte arrays for `no_std` compatibility.
/// Names are compared by value (not pointer), truncated to 32 bytes.
#[derive(Debug, Clone, Copy)]
pub struct ServiceName {
    pub bytes: [u8; MAX_SERVICE_NAME_LEN],
    pub len: usize,
}

impl ServiceName {
    /// Create a service name from a byte slice (truncated to 32 bytes).
    pub fn from_bytes(name: &[u8]) -> Self {
        let len = name.len().min(MAX_SERVICE_NAME_LEN);
        let mut bytes = [0u8; MAX_SERVICE_NAME_LEN];
        bytes[..len].copy_from_slice(&name[..len]);
        Self { bytes, len }
    }

    /// Get the name as a byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// Check if this name matches a byte slice.
    pub fn matches(&self, other: &[u8]) -> bool {
        let other_len = other.len().min(MAX_SERVICE_NAME_LEN);
        self.len == other_len && self.bytes[..self.len] == other[..other_len]
    }
}

impl PartialEq for ServiceName {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.bytes[..self.len] == other.bytes[..other.len]
    }
}

impl Eq for ServiceName {}

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

// RawMessage: ThreadId(4B) + padding(4B) + data(256B) + len(8B) = 272B on 64-bit.
const _: () = assert!(core::mem::size_of::<RawMessage>() == 272);

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

/// Exclusive upper bound of the addresses `validate_user_va` accepts (2^47).
///
/// The bound keeps a user range out of the TTBR1 half; it is not the TTBR0
/// boundary. With the T0SZ=20 that `boot.S` keeps from edk2, TTBR0
/// translates only `[0, 2^44)`, so an address from 2^44 up to this bound is
/// in neither half and takes a translation fault. Addresses from this bound
/// up to the TTBR1 base 0xFFFF_0000_0000_0000 (T1SZ=16) are in neither half
/// either. See `kernel/src/syscall/user.rs`.
pub const USER_VA_LIMIT: usize = 0x0000_8000_0000_0000;

/// Lowest address a user buffer may start at (inclusive).
///
/// Page 0 is never mapped in a user address space, so a pointer into it,
/// null included, is always a caller error. Rejecting it here also keeps a
/// null pointer away from `core::ptr::copy_nonoverlapping` and
/// `core::slice::from_raw_parts`, which require a non-null pointer.
pub const USER_VA_MIN: usize = 0x1000;

/// Validate that a (ptr, len) range lies entirely within
/// `[USER_VA_MIN, USER_VA_LIMIT)`.
///
/// Returns false if:
/// - `ptr` is in page 0 (`ptr < USER_VA_MIN`), null included, at any `len`
/// - `ptr + len` overflows
/// - `ptr >= USER_VA_LIMIT` (at or above 2^47)
/// - `ptr + len > USER_VA_LIMIT`
///
/// Passing does not mean the range is mapped or translatable by TTBR0; see
/// `USER_VA_LIMIT`.
///
/// A zero-length range at a user address outside page 0 is valid.
pub fn validate_user_va(ptr: usize, len: usize) -> bool {
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    (USER_VA_MIN..USER_VA_LIMIT).contains(&ptr) && end <= USER_VA_LIMIT
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

    // --- deadline_after tests (#217) ---

    #[test]
    fn deadline_after_max_is_unbounded() {
        assert_eq!(deadline_after(0, u64::MAX), None);
        assert_eq!(deadline_after(12_345, u64::MAX), None);
        assert_eq!(deadline_after(u64::MAX, u64::MAX), None);
    }

    #[test]
    fn deadline_after_zero_is_now() {
        assert_eq!(deadline_after(0, 0), Some(0));
        assert_eq!(deadline_after(12_345, 0), Some(12_345));
        // At tick u64::MAX even a 0 timeout gives a deadline that can never fire.
        assert_eq!(deadline_after(u64::MAX, 0), None);
    }

    #[test]
    fn deadline_after_adds_when_it_fits() {
        assert_eq!(deadline_after(0, 100), Some(100));
        assert_eq!(deadline_after(12_345, DEFAULT_TIMEOUT_TICKS), Some(17_345));
        assert_eq!(deadline_after(1, u64::MAX - 2), Some(u64::MAX - 1));
    }

    #[test]
    fn deadline_after_max_minus_one_is_unbounded_once_it_overflows() {
        // The issue's failing input: a timeout of u64::MAX - 1 once the tick
        // count is above 1.
        assert_eq!(deadline_after(0, u64::MAX - 1), Some(u64::MAX - 1));
        assert_eq!(deadline_after(1, u64::MAX - 1), None);
        assert_eq!(deadline_after(12_345, u64::MAX - 1), None);
    }

    #[test]
    fn deadline_after_now_near_max_is_unbounded_at_max() {
        assert_eq!(deadline_after(u64::MAX - 10, 5), Some(u64::MAX - 5));
        assert_eq!(deadline_after(u64::MAX - 1, 1), None);
        assert_eq!(deadline_after(u64::MAX - 1, 2), None);
        assert_eq!(deadline_after(u64::MAX, 1), None);
    }

    #[test]
    fn deadline_after_is_never_before_now_nor_late() {
        let nows = [
            0,
            1,
            2,
            12_345,
            u64::MAX / 2,
            u64::MAX - 2,
            u64::MAX - 1,
            u64::MAX,
        ];
        let timeouts = [0, 1, 100, u64::MAX / 2, u64::MAX - 2, u64::MAX - 1];
        for &now in &nows {
            for &t in &timeouts {
                match deadline_after(now, t) {
                    Some(d) => {
                        assert!(d >= now, "deadline {d} before now {now} (timeout {t})");
                        assert!(d - now == t, "deadline {d} is not {t} after {now}");
                        assert!(d != u64::MAX, "deadline {d} can never fire");
                    }
                    None => assert!(
                        now.checked_add(t).is_none_or(|s| s == u64::MAX),
                        "no deadline for {now} + {t}, which fits below u64::MAX"
                    ),
                }
            }
        }
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

    // --- Id layout tests (capability lifetime step 1) ---

    /// Generations the layout tests sweep: both ends, retired, and a few
    /// values between.
    const GENERATIONS: [u32; 6] = [
        ID_GEN_RETIRED,
        ID_GEN_FIRST,
        2,
        0x1234,
        ID_GEN_MAX - 1,
        ID_GEN_MAX,
    ];

    #[test]
    fn id_layout_constants() {
        assert_eq!(ID_SLOT_BITS, 8);
        assert_eq!(ID_SLOT_MASK, 0xFF);
        assert_eq!(ID_GEN_MAX, 0xFF_FFFF);
        assert_eq!(ID_GEN_FIRST, 1);
        assert_eq!(ID_GEN_RETIRED, 0);
        // The strict asserts: slot 255 is malformed in both tables.
        assert!(MAX_CHANNELS <= ID_SLOT_MASK as usize);
        assert!(MAX_SHARED_REGIONS <= ID_SLOT_MASK as usize);
    }

    #[test]
    fn channel_id_round_trip_at_first_generation() {
        for slot in 0..MAX_CHANNELS {
            let id = ChannelId::new(slot, ID_GEN_FIRST);
            assert_eq!(id.raw(), slot as u32 | 1 << ID_SLOT_BITS);
            assert_eq!(id.index(), Some(slot));
            assert_eq!(id.generation(), ID_GEN_FIRST);
            assert_eq!(ChannelId::from_raw(id.raw()), id);
        }
    }

    #[test]
    fn channel_id_round_trip_at_max_generation() {
        for slot in 0..MAX_CHANNELS {
            let id = ChannelId::new(slot, ID_GEN_MAX);
            assert_eq!(id.raw(), slot as u32 | 0xFFFF_FF00);
            assert_eq!(id.index(), Some(slot));
            assert_eq!(id.generation(), ID_GEN_MAX);
            assert_eq!(ChannelId::from_raw(id.raw()), id);
        }
    }

    #[test]
    fn shared_memory_id_round_trip_at_first_generation() {
        for slot in 0..MAX_SHARED_REGIONS {
            let id = SharedMemoryId::new(slot, ID_GEN_FIRST);
            assert_eq!(id.raw(), slot as u32 | 1 << ID_SLOT_BITS);
            assert_eq!(id.index(), Some(slot));
            assert_eq!(id.generation(), ID_GEN_FIRST);
            assert_eq!(SharedMemoryId::from_raw(id.raw()), id);
        }
    }

    #[test]
    fn shared_memory_id_round_trip_at_max_generation() {
        for slot in 0..MAX_SHARED_REGIONS {
            let id = SharedMemoryId::new(slot, ID_GEN_MAX);
            assert_eq!(id.raw(), slot as u32 | 0xFFFF_FF00);
            assert_eq!(id.index(), Some(slot));
            assert_eq!(id.generation(), ID_GEN_MAX);
            assert_eq!(SharedMemoryId::from_raw(id.raw()), id);
        }
    }

    #[test]
    fn channel_id_malformed_slot_has_no_index_at_any_generation() {
        for slot in MAX_CHANNELS as u32..=ID_SLOT_MASK {
            for g in GENERATIONS {
                let id = ChannelId::from_raw(slot | g << ID_SLOT_BITS);
                assert_eq!(id.index(), None, "slot {slot} generation {g}");
                assert_eq!(id.generation(), g);
            }
        }
        assert_eq!(ChannelId::from_raw(u32::MAX).index(), None);
    }

    #[test]
    fn shared_memory_id_malformed_slot_has_no_index_at_any_generation() {
        for slot in MAX_SHARED_REGIONS as u32..=ID_SLOT_MASK {
            for g in GENERATIONS {
                let id = SharedMemoryId::from_raw(slot | g << ID_SLOT_BITS);
                assert_eq!(id.index(), None, "slot {slot} generation {g}");
                assert_eq!(id.generation(), g);
            }
        }
        assert_eq!(SharedMemoryId::from_raw(u32::MAX).index(), None);
    }

    #[test]
    fn valid_slot_has_its_index_at_any_generation() {
        for g in GENERATIONS {
            for slot in 0..MAX_CHANNELS as u32 {
                let id = ChannelId::from_raw(slot | g << ID_SLOT_BITS);
                assert_eq!(id.index(), Some(slot as usize));
            }
            for slot in 0..MAX_SHARED_REGIONS as u32 {
                let id = SharedMemoryId::from_raw(slot | g << ID_SLOT_BITS);
                assert_eq!(id.index(), Some(slot as usize));
            }
        }
    }

    #[test]
    fn same_slot_other_generation_compares_unequal() {
        let a = ChannelId::new(5, ID_GEN_FIRST);
        let b = ChannelId::new(5, ID_GEN_FIRST + 1);
        assert_eq!(a.index(), b.index());
        assert_ne!(a, b);
        assert_ne!(
            ChannelId::new(0, ID_GEN_MAX),
            ChannelId::new(0, ID_GEN_FIRST)
        );

        let r1 = SharedMemoryId::new(3, ID_GEN_FIRST);
        let r2 = SharedMemoryId::new(3, ID_GEN_MAX);
        assert_eq!(r1.index(), r2.index());
        assert_ne!(r1, r2);
        assert_eq!(r1, SharedMemoryId::new(3, ID_GEN_FIRST));
    }

    #[test]
    fn next_generation_advances_below_max() {
        assert_eq!(next_generation(ID_GEN_FIRST), 2);
        assert_eq!(next_generation(0x1234), 0x1235);
        assert_eq!(next_generation(ID_GEN_MAX - 1), ID_GEN_MAX);
    }

    #[test]
    fn next_generation_retires_at_max_and_stays_retired() {
        let mut g = ID_GEN_MAX - 2;
        let mut seen = [0u32; 6];
        for s in &mut seen {
            g = next_generation(g);
            *s = g;
        }
        assert_eq!(
            seen,
            [
                ID_GEN_MAX - 1,
                ID_GEN_MAX,
                ID_GEN_RETIRED,
                ID_GEN_RETIRED,
                ID_GEN_RETIRED,
                ID_GEN_RETIRED,
            ]
        );
        // Values past the field (never stored) also retire.
        assert_eq!(next_generation(ID_GEN_MAX + 1), ID_GEN_RETIRED);
        assert_eq!(next_generation(u32::MAX), ID_GEN_RETIRED);
    }

    #[test]
    #[should_panic]
    fn channel_id_new_rejects_a_malformed_slot() {
        let _ = ChannelId::new(MAX_CHANNELS, ID_GEN_FIRST);
    }

    #[test]
    #[should_panic]
    fn shared_memory_id_new_rejects_a_malformed_slot() {
        let _ = SharedMemoryId::new(MAX_SHARED_REGIONS, ID_GEN_FIRST);
    }

    #[test]
    #[should_panic]
    fn id_new_rejects_the_retired_generation() {
        let _ = ChannelId::new(0, ID_GEN_RETIRED);
    }

    #[test]
    #[should_panic]
    fn id_new_rejects_a_generation_past_max() {
        let _ = SharedMemoryId::new(0, ID_GEN_MAX + 1);
    }

    #[test]
    fn ids_print_as_slot_dot_generation() {
        assert_eq!(alloc::format!("{}", ChannelId::new(5, ID_GEN_FIRST)), "5.1");
        assert_eq!(
            alloc::format!("{}", ChannelId::new(127, ID_GEN_MAX)),
            "127.16777215"
        );
        assert_eq!(alloc::format!("{}", SharedMemoryId::new(3, 2)), "3.2");
        assert_eq!(
            alloc::format!("{}", ChannelId::from_raw(u32::MAX)),
            "255.16777215"
        );
        assert_eq!(
            alloc::format!("{:?}", ChannelId::new(5, ID_GEN_FIRST)),
            "ChannelId(5.1)"
        );
        assert_eq!(
            alloc::format!("{:?}", SharedMemoryId::new(63, ID_GEN_FIRST)),
            "SharedMemoryId(63.1)"
        );
    }

    #[test]
    fn id_raw_is_usable_in_const_context() {
        // `raw()` is a const fn: wire structs build from it in const fns.
        const RAW: u32 = SharedMemoryId::new(2, ID_GEN_FIRST).raw();
        assert_eq!(RAW, 0x102);
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
        let ch = ChannelId::new(42, ID_GEN_FIRST);
        let ch2 = ch;
        assert_eq!(ch, ch2);
    }

    #[test]
    fn channel_id_equality() {
        assert_eq!(ChannelId::from_raw(0), ChannelId::from_raw(0));
        assert_ne!(ChannelId::from_raw(0), ChannelId::from_raw(1));
    }

    #[test]
    fn channel_id_max_valid() {
        let ch = ChannelId::new(MAX_CHANNELS - 1, ID_GEN_FIRST);
        assert_eq!(ch.index(), Some(MAX_CHANNELS - 1));
    }

    #[test]
    fn channel_id_index_zero() {
        assert_eq!(ChannelId::from_raw(0).index(), Some(0));
    }

    #[test]
    fn channel_id_index_last_valid() {
        assert_eq!(
            ChannelId::from_raw((MAX_CHANNELS - 1) as u32).index(),
            Some(MAX_CHANNELS - 1)
        );
    }

    #[test]
    fn channel_id_index_max_channels_is_out_of_range() {
        assert_eq!(ChannelId::from_raw(MAX_CHANNELS as u32).index(), None);
    }

    #[test]
    fn channel_id_index_u32_max_is_out_of_range() {
        assert_eq!(ChannelId::from_raw(u32::MAX).index(), None);
    }

    #[test]
    fn channel_id_index_in_range_ids_fit_the_table() {
        // Every id that index() accepts must be a valid position in a
        // MAX_CHANNELS-entry table, i.e. safe to index with, whatever its
        // generation.
        let table = [(); MAX_CHANNELS];
        for g in GENERATIONS {
            for slot in 0..=ID_SLOT_MASK {
                match ChannelId::from_raw(slot | g << ID_SLOT_BITS).index() {
                    Some(idx) => {
                        assert_eq!(idx, slot as usize);
                        assert!(table.get(idx).is_some());
                    }
                    None => assert!(slot as usize >= MAX_CHANNELS),
                }
            }
        }
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
        assert!(validate_user_va(USER_VA_MIN, 256));
        assert!(validate_user_va(USER_VA_LIMIT - 1, 1));
    }

    #[test]
    fn user_va_zero_len() {
        assert!(validate_user_va(USER_VA_MIN, 0));
        assert!(validate_user_va(0x40_0000, 0));
        assert!(validate_user_va(USER_VA_LIMIT - 1, 0));
    }

    #[test]
    fn user_va_null_is_rejected_at_any_len() {
        assert!(!validate_user_va(0, 0));
        assert!(!validate_user_va(0, 1));
        assert!(!validate_user_va(0, 256));
        assert!(!validate_user_va(0, USER_VA_LIMIT));
    }

    #[test]
    fn user_va_page_zero_is_rejected() {
        assert!(!validate_user_va(1, 0));
        assert!(!validate_user_va(USER_VA_MIN - 1, 1));
        assert!(!validate_user_va(USER_VA_MIN - 1, 2));
        // The first byte of page 1 is the lowest valid start.
        assert!(validate_user_va(USER_VA_MIN, 1));
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
        // A valid start whose end wraps: only the overflow check rejects
        // these. A wrapped end (0xFFF, 0x3F_EFFF) would pass the limit test.
        assert!(!validate_user_va(USER_VA_MIN, usize::MAX));
        assert!(!validate_user_va(0x40_0000, usize::MAX - 0x1000));
    }

    #[test]
    fn user_va_large_valid() {
        // The whole user range above page 0 is one valid buffer.
        assert!(validate_user_va(USER_VA_MIN, USER_VA_LIMIT - USER_VA_MIN));
        assert!(validate_user_va(
            USER_VA_MIN,
            USER_VA_LIMIT - USER_VA_MIN - 1
        ));
        assert!(!validate_user_va(
            USER_VA_MIN,
            USER_VA_LIMIT - USER_VA_MIN + 1
        ));
    }

    // --- SelectKind / SelectEntry tests ---

    #[test]
    fn select_kind_channel_equality() {
        let a = SelectKind::Channel(ChannelId::new(5, ID_GEN_FIRST));
        let b = SelectKind::Channel(ChannelId::new(5, ID_GEN_FIRST));
        let c = SelectKind::Channel(ChannelId::new(6, ID_GEN_FIRST));
        let d = SelectKind::Channel(ChannelId::new(5, ID_GEN_FIRST + 1));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d); // Same slot, other generation.
    }

    #[test]
    fn select_kind_notification_equality() {
        let a = SelectKind::Notification(NotificationId(1), 0xFF);
        let b = SelectKind::Notification(NotificationId(1), 0xFF);
        let c = SelectKind::Notification(NotificationId(1), 0x0F);
        let d = SelectKind::Notification(NotificationId(2), 0xFF);
        assert_eq!(a, b);
        assert_ne!(a, c); // Different mask.
        assert_ne!(a, d); // Different notification id.
    }

    #[test]
    fn select_kind_cross_variant() {
        let ch = SelectKind::Channel(ChannelId::from_raw(0));
        let nf = SelectKind::Notification(NotificationId(0), 0);
        assert_ne!(ch, nf);
    }

    #[test]
    fn select_entry_copy_clone() {
        let e = SelectEntry {
            kind: SelectKind::Channel(ChannelId::new(42, ID_GEN_FIRST)),
        };
        let e2 = e;
        assert_eq!(e, e2);
    }

    // --- RawSelectEntry (IpcSelect wire format) tests ---

    /// Wire bytes built field by field at the documented offsets.
    fn wire(kind: u32, id: u32, mask: u64) -> [u8; RawSelectEntry::SIZE] {
        let mut b = [0u8; RawSelectEntry::SIZE];
        b[0..4].copy_from_slice(&kind.to_ne_bytes());
        b[4..8].copy_from_slice(&id.to_ne_bytes());
        b[8..16].copy_from_slice(&mask.to_ne_bytes());
        b
    }

    #[test]
    fn raw_select_entry_is_16_bytes() {
        assert_eq!(core::mem::size_of::<RawSelectEntry>(), 16);
        assert_eq!(RawSelectEntry::SIZE, 16);
    }

    #[test]
    fn raw_select_entry_from_bytes_reads_documented_offsets() {
        let raw = RawSelectEntry::from_bytes(&wire(1, 0x0102_0304, 0x1122_3344_5566_7788));
        assert_eq!(
            raw,
            RawSelectEntry {
                kind: 1,
                id: 0x0102_0304,
                mask: 0x1122_3344_5566_7788,
            }
        );
    }

    #[test]
    fn raw_select_entry_decodes_channel() {
        let raw = RawSelectEntry::from_bytes(&wire(SELECT_KIND_CHANNEL, 5, 0xFF));
        // A channel entry ignores its mask.
        assert_eq!(
            SelectEntry::try_from(raw),
            Ok(SelectEntry {
                kind: SelectKind::Channel(ChannelId::from_raw(5)),
            })
        );
    }

    #[test]
    fn raw_select_entry_decodes_the_full_channel_id() {
        // The generation bits survive decoding: the id is not truncated to
        // its slot.
        let id = ChannelId::new(9, 0xAB_CDEF);
        let raw = RawSelectEntry::from_bytes(&wire(SELECT_KIND_CHANNEL, id.raw(), 0));
        assert_eq!(raw.id, 0xABCD_EF09);
        assert_eq!(
            SelectEntry::try_from(raw),
            Ok(SelectEntry {
                kind: SelectKind::Channel(id),
            })
        );
    }

    #[test]
    fn raw_select_entry_decodes_notification() {
        let raw = RawSelectEntry::from_bytes(&wire(SELECT_KIND_NOTIFICATION, 3, 0b1010));
        assert_eq!(
            SelectEntry::try_from(raw),
            Ok(SelectEntry {
                kind: SelectKind::Notification(NotificationId(3), 0b1010),
            })
        );
    }

    #[test]
    fn raw_select_entry_unknown_kind_is_einval() {
        let einval = Err(crate::syscall::IpcError::Einval as i64);
        for kind in [2, 0x100, u32::MAX] {
            let raw = RawSelectEntry::from_bytes(&wire(kind, 0, 0));
            assert_eq!(SelectEntry::try_from(raw), einval, "kind {kind}");
        }
    }

    #[test]
    fn raw_select_entry_round_trip() {
        let entries = [
            SelectEntry {
                kind: SelectKind::Channel(ChannelId::new(127, ID_GEN_FIRST)),
            },
            SelectEntry {
                kind: SelectKind::Channel(ChannelId::new(0, ID_GEN_MAX)),
            },
            SelectEntry {
                kind: SelectKind::Channel(ChannelId::new(127, 0x12_3456)),
            },
            SelectEntry {
                kind: SelectKind::Notification(NotificationId(63), u64::MAX),
            },
        ];
        for e in entries {
            let raw = RawSelectEntry::from(e);
            if let SelectKind::Channel(ch) = e.kind {
                // The wire carries the full id, generation included.
                assert_eq!(raw.id, ch.raw());
            }
            let bytes = wire(raw.kind, raw.id, raw.mask);
            assert_eq!(
                SelectEntry::try_from(RawSelectEntry::from_bytes(&bytes)),
                Ok(e)
            );
        }
    }

    #[test]
    fn max_select_entries_bounded() {
        assert!(MAX_SELECT_ENTRIES >= 2);
        assert!(MAX_SELECT_ENTRIES <= 64);
    }

    // --- ServiceState tests ---

    #[test]
    fn service_state_equality() {
        assert_eq!(ServiceState::Running, ServiceState::Running);
        assert_eq!(ServiceState::Dead, ServiceState::Dead);
        assert_ne!(ServiceState::Running, ServiceState::Dead);
    }

    #[test]
    fn service_state_copy() {
        let s = ServiceState::Running;
        let s2 = s;
        assert_eq!(s, s2);
    }

    // --- ServiceName tests ---

    #[test]
    fn service_name_from_bytes() {
        let name = ServiceName::from_bytes(b"echo");
        assert_eq!(name.len, 4);
        assert_eq!(name.as_bytes(), b"echo");
    }

    #[test]
    fn service_name_empty() {
        let name = ServiceName::from_bytes(b"");
        assert_eq!(name.len, 0);
        assert_eq!(name.as_bytes(), b"");
    }

    #[test]
    fn service_name_truncation() {
        // Exactly 32 bytes.
        let long = [b'a'; 32];
        let name = ServiceName::from_bytes(&long);
        assert_eq!(name.len, 32);
        assert_eq!(name.as_bytes(), &long[..]);

        // 64 bytes — should truncate to 32.
        let too_long = [b'b'; 64];
        let name = ServiceName::from_bytes(&too_long);
        assert_eq!(name.len, 32);
    }

    #[test]
    fn service_name_matches() {
        let name = ServiceName::from_bytes(b"echo");
        assert!(name.matches(b"echo"));
        assert!(!name.matches(b"ech"));
        assert!(!name.matches(b"echo!"));
        assert!(!name.matches(b"ECHO"));
        assert!(!name.matches(b""));
    }

    #[test]
    fn service_name_equality() {
        let a = ServiceName::from_bytes(b"hello");
        let b = ServiceName::from_bytes(b"hello");
        let c = ServiceName::from_bytes(b"world");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn service_name_different_lengths() {
        let a = ServiceName::from_bytes(b"foo");
        let b = ServiceName::from_bytes(b"foobar");
        assert_ne!(a, b);
    }

    #[test]
    fn max_services_bounded() {
        assert!(MAX_SERVICES >= 4);
        assert!(MAX_SERVICES <= 256);
    }
}
