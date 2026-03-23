//! Storage subsystem shared types and constants.
//!
//! Content-addressed block storage primitives, VirtIO MMIO transport
//! definitions, and on-disk layout constants. Used by kernel storage
//! subsystem and shared crate unit tests.
//!
//! Per spaces.md §3.0 (primitive types), §4.1 (on-disk layout).

// ---------------------------------------------------------------------------
// Core storage types
// ---------------------------------------------------------------------------

/// SHA-256 content hash (32 bytes). Primary block identifier.
///
/// Every stored block is addressed by its SHA-256 hash, enabling
/// content-addressed deduplication.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// All-zero hash (invalid / sentinel).
    pub const ZERO: Self = Self([0u8; 32]);

    /// Check if this hash is the zero sentinel.
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }
}

impl core::fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Hash(")?;
        for b in &self.0[..4] {
            write!(f, "{:02x}", b)?;
        }
        write!(f, "...)")
    }
}

/// Type alias: blocks are identified by their content hash.
pub type BlockId = ContentHash;

/// 128-bit object identifier.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
#[repr(transparent)]
pub struct ObjectId(pub [u8; 16]);

impl ObjectId {
    pub const ZERO: Self = Self([0u8; 16]);
}

/// 128-bit space identifier.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
#[repr(transparent)]
pub struct SpaceId(pub [u8; 16]);

impl SpaceId {
    pub const ZERO: Self = Self([0u8; 16]);
}

/// Timestamp in milliseconds since epoch.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct Timestamp(pub u64);

impl Timestamp {
    pub const ZERO: Self = Self(0);
}

/// Content type classification for stored objects.
///
/// Per spaces.md §3.3 — all 18 variants from architecture doc.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum ContentType {
    Directory = 0,
    Document = 1,
    Text = 2,
    Code = 3,
    Markdown = 4,
    Json = 5,
    Xml = 6,
    Image = 7,
    Video = 8,
    Audio = 9,
    Config = 10,
    Credential = 11,
    Executable = 12,
    GameSave = 13,
    CacheEntry = 14,
    SessionToken = 15,
    Cookie = 16,
    Binary = 17,
}

/// Security zone classification.
///
/// Per spaces.md §3.1. Simplified for M13 — `Collaborative` is a plain
/// variant (no `Vec<IdentityId>` member) for `Copy + repr(u8)` compatibility.
/// Full struct variant added when identity system provides `IdentityId`.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum SecurityZone {
    Core = 0,
    Personal = 1,
    Collaborative = 2,
    Untrusted = 3,
    Ephemeral = 4,
}

/// Storage temperature tier for block placement.
///
/// Per spaces.md §4.7 — determines compression level and zone placement.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum StorageTier {
    Hot = 0,
    Warm = 1,
    Cold = 2,
}

/// Location of a data block on disk.
///
/// Per spaces.md §3.0 — byte offset (NOT sector offset). Refcount is
/// tracked separately in the MemTable entry, not in BlockLocation.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(C)]
pub struct BlockLocation {
    /// Byte offset on the raw device partition.
    pub offset: u64,
    /// Data payload size in bytes (excludes on-disk header and padding).
    pub size: u32,
    /// Temperature tier.
    pub tier: StorageTier,
}

/// Storage subsystem errors.
///
/// No `String` fields — all variants are `Copy` for no_std compatibility.
/// `DecryptionFailed` reserved for M14 encryption support.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StorageError {
    BlockNotFound,
    ChecksumFailed,
    DecryptionFailed,
    IoError,
    QuotaExceeded,
    DeviceFull,
    WalFull,
    SuperblockCorrupt,
    DeviceNotFound,
    VirtioError,
    MemTableFull,
    ObjectNotFound,
    SpaceNotFound,
    SpaceNotEmpty,
    VersionNotFound,
    // M15 POSIX bridge errors
    NameExists,
    NotADirectory,
    FdTableFull,
    InvalidFd,
}

// ---------------------------------------------------------------------------
// M14 types: CompactObject, Version, Space, Provenance, Encryption
// ---------------------------------------------------------------------------

/// Maximum length of an object name in bytes.
pub const MAX_OBJECT_NAME_LEN: usize = 64;

/// Maximum length of an author/agent identifier in bytes.
pub const MAX_AUTHOR_LEN: usize = 32;

/// Maximum number of spaces.
pub const MAX_SPACES: usize = 16;

/// Maximum length of a space name in bytes.
pub const MAX_SPACE_NAME_LEN: usize = 32;

/// Maximum length of a version message in bytes.
pub const MAX_VERSION_MESSAGE_LEN: usize = 64;

/// Maximum entries in the object index.
pub const OBJECT_INDEX_MAX_ENTRIES: usize = 16_384;

/// Maximum length of extracted text content in CompactObject.
pub const MAX_TEXT_CONTENT_LEN: usize = 128;

/// Encryption overhead: 12-byte nonce + 16-byte AES-GCM auth tag.
pub const ENCRYPTION_OVERHEAD: usize = 28;

/// Compact object metadata (512 bytes, repr(C)).
///
/// Per spaces.md §3.3.1. Fixed-size on-disk metadata record for each stored
/// object. Fields ordered for alignment: byte arrays first, then u64, u32, u8.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct CompactObject {
    pub id: ObjectId,
    pub space_id: SpaceId,
    pub name: [u8; MAX_OBJECT_NAME_LEN],
    pub content_hash: ContentHash,
    pub version_head: ContentHash,
    pub created_by: [u8; MAX_AUTHOR_LEN],
    pub modified_by: [u8; MAX_AUTHOR_LEN],
    pub text_content: [u8; MAX_TEXT_CONTENT_LEN],
    pub created_at: Timestamp,
    pub modified_at: Timestamp,
    pub content_size: u32,
    pub content_type: ContentType,
    pub name_len: u8,
    pub text_len: u8,
    pub _padding: [u8; 137],
}

impl CompactObject {
    /// All-zero sentinel (invalid object).
    pub const ZERO: Self = Self {
        id: ObjectId::ZERO,
        space_id: SpaceId::ZERO,
        name: [0u8; MAX_OBJECT_NAME_LEN],
        content_hash: ContentHash::ZERO,
        version_head: ContentHash::ZERO,
        created_by: [0u8; MAX_AUTHOR_LEN],
        modified_by: [0u8; MAX_AUTHOR_LEN],
        text_content: [0u8; MAX_TEXT_CONTENT_LEN],
        created_at: Timestamp::ZERO,
        modified_at: Timestamp::ZERO,
        content_size: 0,
        content_type: ContentType::Binary,
        name_len: 0,
        text_len: 0,
        _padding: [0u8; 137],
    };

    /// Get the object name as a byte slice. Clamps to array bounds for corruption safety.
    pub fn name_bytes(&self) -> &[u8] {
        let len = (self.name_len as usize).min(MAX_OBJECT_NAME_LEN);
        &self.name[..len]
    }

    /// Set the object name from a byte slice. Truncates to MAX_OBJECT_NAME_LEN.
    pub fn set_name(&mut self, name: &[u8]) {
        let len = name.len().min(MAX_OBJECT_NAME_LEN);
        self.name[..len].copy_from_slice(&name[..len]);
        self.name_len = len as u8;
    }

    /// Get extracted text content as a byte slice.
    pub fn text_bytes(&self) -> &[u8] {
        &self.text_content[..self.text_len as usize]
    }

    /// Set extracted text content. Truncates to MAX_TEXT_CONTENT_LEN.
    pub fn set_text(&mut self, text: &[u8]) {
        let len = text.len().min(MAX_TEXT_CONTENT_LEN);
        self.text_content[..len].copy_from_slice(&text[..len]);
        self.text_len = len as u8;
    }

    /// Check if this is the zero sentinel.
    pub fn is_zero(&self) -> bool {
        self.id == ObjectId::ZERO
    }
}

impl core::fmt::Debug for CompactObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CompactObject")
            .field("id", &self.id)
            .field("space_id", &self.space_id)
            .field("content_hash", &self.content_hash)
            .field("content_size", &self.content_size)
            .field("content_type", &self.content_type)
            .finish()
    }
}

/// Version node in the Merkle DAG (256 bytes, repr(C)).
///
/// Per spaces.md §5.1. Each object modification creates a new version
/// linked to its parent by hash. The chain forms a Merkle DAG.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct Version {
    pub hash: ContentHash,
    pub parent: ContentHash,
    pub merge_parent: ContentHash,
    pub content_hash: ContentHash,
    pub object_id: ObjectId,
    pub author: [u8; MAX_AUTHOR_LEN],
    pub message: [u8; MAX_VERSION_MESSAGE_LEN],
    pub timestamp: Timestamp,
    pub content_size: u32,
    pub message_len: u8,
    pub _padding: [u8; 3],
}

impl Version {
    /// All-zero sentinel (no version).
    pub const ZERO: Self = Self {
        hash: ContentHash::ZERO,
        parent: ContentHash::ZERO,
        merge_parent: ContentHash::ZERO,
        content_hash: ContentHash::ZERO,
        object_id: ObjectId::ZERO,
        author: [0u8; MAX_AUTHOR_LEN],
        message: [0u8; MAX_VERSION_MESSAGE_LEN],
        timestamp: Timestamp::ZERO,
        content_size: 0,
        message_len: 0,
        _padding: [0u8; 3],
    };

    /// Check if the parent is the zero hash (this is the first version).
    pub fn is_root(&self) -> bool {
        self.parent.is_zero()
    }

    /// Get the version message as a byte slice. Clamps to array bounds for corruption safety.
    pub fn message_bytes(&self) -> &[u8] {
        let len = (self.message_len as usize).min(MAX_VERSION_MESSAGE_LEN);
        &self.message[..len]
    }

    /// Set the version message. Truncates to MAX_VERSION_MESSAGE_LEN.
    pub fn set_message(&mut self, msg: &[u8]) {
        let len = msg.len().min(MAX_VERSION_MESSAGE_LEN);
        self.message[..len].copy_from_slice(&msg[..len]);
        self.message_len = len as u8;
    }
}

impl core::fmt::Debug for Version {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Version")
            .field("hash", &self.hash)
            .field("parent", &self.parent)
            .field("object_id", &self.object_id)
            .field("content_size", &self.content_size)
            .finish()
    }
}

/// Provenance action type for tracking object lineage.
///
/// Per spaces.md §3.3. Tracks how an object was created or modified.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum ProvenanceAction {
    Created = 0,
    Modified = 1,
    Derived = 2,
    Imported = 3,
    AiGenerated = 4,
}

/// Provenance entry recording agent actions on objects (144 bytes, repr(C)).
///
/// Tracks who did what to an object, when, and optionally from which source.
/// The signature field is zeroed in Phase 4 (Ed25519 deferred to Phase 13).
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ProvenanceEntry {
    pub agent: [u8; MAX_AUTHOR_LEN],
    pub task: [u8; 16],
    pub signature: [u8; 64],
    pub source_object: ObjectId,
    pub timestamp: Timestamp,
    pub action: ProvenanceAction,
    pub has_task: u8,
    pub _padding: [u8; 6],
}

impl ProvenanceEntry {
    /// All-zero sentinel.
    pub const ZERO: Self = Self {
        agent: [0u8; MAX_AUTHOR_LEN],
        task: [0u8; 16],
        signature: [0u8; 64],
        source_object: ObjectId::ZERO,
        timestamp: Timestamp::ZERO,
        action: ProvenanceAction::Created,
        has_task: 0,
        _padding: [0u8; 6],
    };
}

impl core::fmt::Debug for ProvenanceEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProvenanceEntry")
            .field("action", &self.action)
            .field("timestamp", &self.timestamp)
            .field("has_task", &self.has_task)
            .finish()
    }
}

/// Space quota limits.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(C)]
pub struct SpaceQuota {
    pub max_bytes: u64,
    pub max_objects: u32,
    pub _padding: [u8; 4],
}

impl SpaceQuota {
    pub const UNLIMITED: Self = Self {
        max_bytes: u64::MAX,
        max_objects: u32::MAX,
        _padding: [0u8; 4],
    };
}

impl Default for SpaceQuota {
    fn default() -> Self {
        Self::UNLIMITED
    }
}

/// Space metadata (128 bytes, repr(C)).
///
/// Per spaces.md §3.1. Spaces organize objects into security zones
/// with quotas and hierarchical structure.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct Space {
    pub id: SpaceId,
    pub name: [u8; MAX_SPACE_NAME_LEN],
    pub parent: SpaceId,
    pub created_at: Timestamp,
    pub modified_at: Timestamp,
    pub quota_max_bytes: u64,
    pub total_size: u64,
    pub object_count: u32,
    pub quota_max_objects: u32,
    pub security_zone: SecurityZone,
    pub name_len: u8,
    pub _padding: [u8; 22],
}

impl Space {
    /// All-zero sentinel.
    pub const ZERO: Self = Self {
        id: SpaceId::ZERO,
        name: [0u8; MAX_SPACE_NAME_LEN],
        parent: SpaceId::ZERO,
        created_at: Timestamp::ZERO,
        modified_at: Timestamp::ZERO,
        quota_max_bytes: 0,
        total_size: 0,
        object_count: 0,
        quota_max_objects: 0,
        security_zone: SecurityZone::Core,
        name_len: 0,
        _padding: [0u8; 22],
    };

    /// Get the space name as a byte slice. Clamps to array bounds for corruption safety.
    pub fn name_bytes(&self) -> &[u8] {
        let len = (self.name_len as usize).min(MAX_SPACE_NAME_LEN);
        &self.name[..len]
    }

    /// Set the space name. Truncates to MAX_SPACE_NAME_LEN.
    pub fn set_name(&mut self, name: &[u8]) {
        let len = name.len().min(MAX_SPACE_NAME_LEN);
        self.name[..len].copy_from_slice(&name[..len]);
        self.name_len = len as u8;
    }

    /// Check if this is the zero sentinel.
    pub fn is_zero(&self) -> bool {
        self.id == SpaceId::ZERO
    }

    /// Apply a SpaceQuota to this space.
    pub fn set_quota(&mut self, quota: SpaceQuota) {
        self.quota_max_bytes = quota.max_bytes;
        self.quota_max_objects = quota.max_objects;
    }

    /// Get quota as a SpaceQuota struct.
    pub fn quota(&self) -> SpaceQuota {
        SpaceQuota {
            max_bytes: self.quota_max_bytes,
            max_objects: self.quota_max_objects,
            _padding: [0u8; 4],
        }
    }

    /// Check if adding `bytes` would exceed the quota.
    pub fn would_exceed_quota(&self, bytes: u64) -> bool {
        self.object_count >= self.quota_max_objects
            || self.total_size.saturating_add(bytes) > self.quota_max_bytes
    }
}

impl core::fmt::Debug for Space {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Space")
            .field("id", &self.id)
            .field("security_zone", &self.security_zone)
            .field("object_count", &self.object_count)
            .field("total_size", &self.total_size)
            .finish()
    }
}

/// Encryption state for a space or device.
///
/// Per spaces.md §6.1. Phase 4 uses DeviceOnly for all spaces.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum EncryptionState {
    DeviceOnly = 0,
    SpaceEncrypted = 1,
}

/// Object index entry: maps ObjectId to its metadata block location.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(C)]
pub struct ObjectIndexEntry {
    pub key: ObjectId,
    pub location: BlockLocation,
}

/// Compute a version hash from its components using SHA-256.
///
/// Per spaces.md §5.1: hash = SHA-256(parent || content_hash || timestamp || object_id).
/// This creates the Merkle DAG linkage — each version's hash depends on its parent.
pub fn compute_version_hash(
    parent: &ContentHash,
    content_hash: &ContentHash,
    timestamp: Timestamp,
    object_id: &ObjectId,
) -> ContentHash {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(parent.0);
    hasher.update(content_hash.0);
    hasher.update(timestamp.0.to_le_bytes());
    hasher.update(object_id.0);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    ContentHash(hash)
}

// ---------------------------------------------------------------------------
// On-disk layout constants
// ---------------------------------------------------------------------------

/// Superblock magic: "AIOSPACE" as u64.
pub const SUPERBLOCK_MAGIC: u64 = 0x41494F53_50414345;

/// Superblock format version.
pub const SUPERBLOCK_VERSION: u32 = 1;

/// Disk sector size in bytes.
pub const SECTOR_SIZE: usize = 512;

/// Logical block size in bytes.
pub const BLOCK_SIZE: usize = 4096;

/// WAL region starts at sector 8 (after 4 KiB superblock = 8 sectors).
pub const WAL_START_SECTOR: u64 = 8;

/// WAL region size: 64 MiB = 131072 sectors.
pub const WAL_SIZE_SECTORS: u64 = 131_072;

/// Data region starts immediately after WAL.
pub const DATA_START_SECTOR: u64 = WAL_START_SECTOR + WAL_SIZE_SECTORS;

/// Maximum MemTable entries (in-memory sorted index).
pub const MEMTABLE_MAX_ENTRIES: usize = 65_536;

/// On-disk WAL entry size in bytes.
pub const WAL_ENTRY_SIZE: usize = 64;

/// Number of WAL entries that fit in one sector.
pub const WAL_ENTRIES_PER_SECTOR: usize = SECTOR_SIZE / WAL_ENTRY_SIZE;

// ---------------------------------------------------------------------------
// VirtIO MMIO transport constants
// ---------------------------------------------------------------------------

/// VirtIO MMIO magic value ("virt" in LE).
pub const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976;

/// VirtIO MMIO version for modern (non-legacy) devices.
pub const VIRTIO_MMIO_VERSION_MODERN: u32 = 2;

/// VirtIO device ID for block devices.
pub const VIRTIO_DEVICE_ID_BLK: u32 = 2;

/// QEMU virt machine VirtIO MMIO region base address.
pub const VIRTIO_MMIO_REGION_BASE: u64 = 0x0A00_0000;

/// Stride between VirtIO MMIO slots on QEMU virt.
pub const VIRTIO_MMIO_REGION_STRIDE: u64 = 0x200;

/// Maximum number of VirtIO MMIO slots to probe.
pub const VIRTIO_MMIO_SLOT_COUNT: usize = 32;

// ---------------------------------------------------------------------------
// VirtIO MMIO register offsets (VirtIO spec §4.2.2)
// ---------------------------------------------------------------------------

pub const VIRTIO_MMIO_MAGIC_VALUE: usize = 0x000;
pub const VIRTIO_MMIO_VERSION: usize = 0x004;
pub const VIRTIO_MMIO_DEVICE_ID: usize = 0x008;
pub const VIRTIO_MMIO_VENDOR_ID: usize = 0x00C;
pub const VIRTIO_MMIO_DEVICE_FEATURES: usize = 0x010;
pub const VIRTIO_MMIO_DEVICE_FEATURES_SEL: usize = 0x014;
pub const VIRTIO_MMIO_DRIVER_FEATURES: usize = 0x020;
pub const VIRTIO_MMIO_DRIVER_FEATURES_SEL: usize = 0x024;
pub const VIRTIO_MMIO_QUEUE_SEL: usize = 0x030;
pub const VIRTIO_MMIO_QUEUE_NUM_MAX: usize = 0x034;
pub const VIRTIO_MMIO_QUEUE_NUM: usize = 0x038;
pub const VIRTIO_MMIO_GUEST_PAGE_SIZE: usize = 0x028; // Legacy (v1): write page size (4096)
pub const VIRTIO_MMIO_QUEUE_ALIGN: usize = 0x03C; // Legacy (v1): queue alignment
pub const VIRTIO_MMIO_QUEUE_PFN: usize = 0x040; // Legacy (v1): queue page frame number
pub const VIRTIO_MMIO_QUEUE_READY: usize = 0x044; // Modern (v2) only
pub const VIRTIO_MMIO_QUEUE_NOTIFY: usize = 0x050;
pub const VIRTIO_MMIO_INTERRUPT_STATUS: usize = 0x060;
pub const VIRTIO_MMIO_INTERRUPT_ACK: usize = 0x064;
pub const VIRTIO_MMIO_STATUS: usize = 0x070;
pub const VIRTIO_MMIO_QUEUE_DESC_LOW: usize = 0x080;
pub const VIRTIO_MMIO_QUEUE_DESC_HIGH: usize = 0x084;
pub const VIRTIO_MMIO_QUEUE_AVAIL_LOW: usize = 0x090;
pub const VIRTIO_MMIO_QUEUE_AVAIL_HIGH: usize = 0x094;
pub const VIRTIO_MMIO_QUEUE_USED_LOW: usize = 0x0A0;
pub const VIRTIO_MMIO_QUEUE_USED_HIGH: usize = 0x0A4;
pub const VIRTIO_MMIO_CONFIG_GENERATION: usize = 0x0FC;
pub const VIRTIO_MMIO_CONFIG_SPACE: usize = 0x100;

// ---------------------------------------------------------------------------
// VirtIO device status bits (VirtIO spec §2.1)
// ---------------------------------------------------------------------------

pub const VIRTIO_STATUS_ACKNOWLEDGE: u32 = 1;
pub const VIRTIO_STATUS_DRIVER: u32 = 2;
pub const VIRTIO_STATUS_DRIVER_OK: u32 = 4;
pub const VIRTIO_STATUS_FEATURES_OK: u32 = 8;
pub const VIRTIO_STATUS_NEEDS_RESET: u32 = 64;

// ---------------------------------------------------------------------------
// VirtIO-blk feature bits (VirtIO spec §5.2.3)
// ---------------------------------------------------------------------------

pub const VIRTIO_BLK_F_SIZE_MAX: u64 = 1 << 1;
pub const VIRTIO_BLK_F_SEG_MAX: u64 = 1 << 2;
pub const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

// ---------------------------------------------------------------------------
// VirtIO-blk request types (VirtIO spec §5.2.6)
// ---------------------------------------------------------------------------

pub const VIRTIO_BLK_T_IN: u32 = 0;
pub const VIRTIO_BLK_T_OUT: u32 = 1;

// ---------------------------------------------------------------------------
// Virtqueue descriptor flags (VirtIO spec §2.7.5)
// ---------------------------------------------------------------------------

pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

// ---------------------------------------------------------------------------
// VirtIO structures (repr(C) for MMIO/DMA layout)
// ---------------------------------------------------------------------------

/// Virtqueue descriptor (VirtIO spec §2.7.5). 16 bytes.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct VirtqDesc {
    /// Physical address of the buffer.
    pub addr: u64,
    /// Length of the buffer in bytes.
    pub len: u32,
    /// Descriptor flags (NEXT, WRITE, INDIRECT).
    pub flags: u16,
    /// Index of the next descriptor in the chain.
    pub next: u16,
}

/// VirtIO-blk request header (VirtIO spec §5.2.6). 16 bytes.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct VirtioBlkReqHeader {
    /// Request type: VIRTIO_BLK_T_IN (read) or VIRTIO_BLK_T_OUT (write).
    pub req_type: u32,
    /// Reserved field (must be zero).
    pub reserved: u32,
    /// Sector number to read from / write to.
    pub sector: u64,
}

// Compile-time size assertions.
const _: () = assert!(core::mem::size_of::<VirtqDesc>() == 16);
const _: () = assert!(core::mem::size_of::<VirtioBlkReqHeader>() == 16);
const _: () = assert!(core::mem::size_of::<BlockLocation>() == 16);
const _: () = assert!(WAL_ENTRIES_PER_SECTOR == 8);
const _: () = assert!(DATA_START_SECTOR == 131_080);
const _: () = assert!(core::mem::size_of::<CompactObject>() == 512);
const _: () = assert!(core::mem::size_of::<Version>() == 256);
const _: () = assert!(core::mem::size_of::<Space>() == 128);
const _: () = assert!(core::mem::size_of::<ProvenanceEntry>() == 144);
const _: () = assert!(core::mem::size_of::<ObjectIndexEntry>() == 32);
const _: () = assert!(core::mem::size_of::<SpaceQuota>() == 16);

// ---------------------------------------------------------------------------
// M15 types: POSIX bridge, compression, storage budget
// ---------------------------------------------------------------------------

/// POSIX file-open flags (per spaces.md §9.1).
pub mod posix_flags {
    pub const O_RDONLY: u32 = 0;
    pub const O_WRONLY: u32 = 1;
    pub const O_RDWR: u32 = 2;
    pub const O_CREAT: u32 = 0x40;
    pub const O_APPEND: u32 = 0x400;
}

/// Synthesised POSIX stat result.
#[derive(Clone, Copy, Debug)]
pub struct PosixStat {
    /// File size in bytes.
    pub size: u64,
    /// POSIX mode (0o755 dirs, 0o644 files).
    pub mode: u32,
    /// Last modification timestamp (ticks).
    pub modified: u64,
    /// Number of hard links (always 1).
    pub nlink: u32,
}

/// Directory entry returned by readdir.
#[derive(Clone, Copy)]
pub struct DirEntry {
    /// Entry name (up to 64 bytes, null-padded).
    pub name: [u8; MAX_OBJECT_NAME_LEN],
    /// Name length in bytes.
    pub name_len: usize,
    /// Object identifier.
    pub object_id: ObjectId,
    /// Content type.
    pub content_type: ContentType,
    /// Size in bytes.
    pub size: u64,
}

/// Maximum number of open file descriptors per bridge instance.
pub const MAX_FDS: usize = 256;

/// Compression type identifier.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CompressionType {
    /// No compression.
    None = 0,
    /// LZ4 block compression.
    Lz4 = 1,
}

/// Compression header size: 1 byte type + 4 bytes uncompressed size.
pub const COMPRESSION_HEADER_SIZE: usize = 5;

/// Storage budget summary.
#[derive(Clone, Copy, Debug)]
pub struct StorageBudget {
    /// Total data region bytes.
    pub total_bytes: u64,
    /// Used data region bytes.
    pub used_bytes: u64,
    /// Free data region bytes.
    pub free_bytes: u64,
    /// Number of data blocks.
    pub data_blocks: u64,
    /// WAL entries used.
    pub wal_used: u64,
    /// Index entries in MemTable + ObjectIndex.
    pub index_entries: u64,
}

/// Storage pressure level derived from free percentage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PressureLevel {
    /// >20% free.
    Normal,
    /// 10-20% free.
    Warning,
    /// 5-10% free.
    Critical,
    /// <5% free.
    Emergency,
}

impl PressureLevel {
    /// Compute pressure level from free percentage (0-100).
    pub fn from_free_percentage(pct: u64) -> Self {
        if pct > 20 {
            PressureLevel::Normal
        } else if pct > 10 {
            PressureLevel::Warning
        } else if pct > 5 {
            PressureLevel::Critical
        } else {
            PressureLevel::Emergency
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ContentHash tests --

    #[test]
    fn hash_zero_is_all_zeros() {
        assert_eq!(ContentHash::ZERO.0, [0u8; 32]);
        assert!(ContentHash::ZERO.is_zero());
    }

    #[test]
    fn hash_nonzero_is_not_zero() {
        let mut h = ContentHash::ZERO;
        h.0[0] = 1;
        assert!(!h.is_zero());
    }

    #[test]
    fn hash_ordering() {
        let a = ContentHash([0u8; 32]);
        let mut b = ContentHash([0u8; 32]);
        b.0[0] = 1;
        assert!(a < b);
    }

    #[test]
    fn hash_equality_and_copy() {
        let a = ContentHash([42u8; 32]);
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn hash_debug_format() {
        extern crate alloc;
        let h = ContentHash([
            0xAB, 0xCD, 0xEF, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0,
        ]);
        let s = alloc::format!("{:?}", h);
        assert!(s.contains("abcdef01"));
    }

    // -- ObjectId / SpaceId tests --

    #[test]
    fn object_id_equality_and_copy() {
        let a = ObjectId([1u8; 16]);
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn space_id_equality_and_copy() {
        let a = SpaceId([2u8; 16]);
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn object_id_and_space_id_distinct_types() {
        // Ensure they don't accidentally alias — different types.
        let _o = ObjectId::ZERO;
        let _s = SpaceId::ZERO;
        // If this compiles, they are distinct types.
    }

    // -- Timestamp tests --

    #[test]
    fn timestamp_ordering() {
        let a = Timestamp(100);
        let b = Timestamp(200);
        assert!(a < b);
    }

    #[test]
    fn timestamp_equality() {
        let a = Timestamp(42);
        let b = Timestamp(42);
        assert_eq!(a, b);
    }

    #[test]
    fn timestamp_zero() {
        assert_eq!(Timestamp::ZERO.0, 0);
    }

    // -- BlockLocation tests --

    #[test]
    fn block_location_copy_and_equality() {
        let loc = BlockLocation {
            offset: 1024,
            size: 512,
            tier: StorageTier::Hot,
        };
        let loc2 = loc;
        assert_eq!(loc, loc2);
    }

    #[test]
    fn block_location_different_tiers() {
        let a = BlockLocation {
            offset: 0,
            size: 100,
            tier: StorageTier::Hot,
        };
        let b = BlockLocation {
            offset: 0,
            size: 100,
            tier: StorageTier::Cold,
        };
        assert_ne!(a, b);
    }

    // -- ContentType tests --

    #[test]
    fn content_type_variant_count() {
        // Verify all 18 variants exist by matching.
        let variants = [
            ContentType::Directory,
            ContentType::Document,
            ContentType::Text,
            ContentType::Code,
            ContentType::Markdown,
            ContentType::Json,
            ContentType::Xml,
            ContentType::Image,
            ContentType::Video,
            ContentType::Audio,
            ContentType::Config,
            ContentType::Credential,
            ContentType::Executable,
            ContentType::GameSave,
            ContentType::CacheEntry,
            ContentType::SessionToken,
            ContentType::Cookie,
            ContentType::Binary,
        ];
        assert_eq!(variants.len(), 18);
    }

    #[test]
    fn content_type_repr_values() {
        assert_eq!(ContentType::Directory as u8, 0);
        assert_eq!(ContentType::Binary as u8, 17);
    }

    // -- SecurityZone tests --

    #[test]
    fn security_zone_all_variants() {
        let zones = [
            SecurityZone::Core,
            SecurityZone::Personal,
            SecurityZone::Collaborative,
            SecurityZone::Untrusted,
            SecurityZone::Ephemeral,
        ];
        assert_eq!(zones.len(), 5);
    }

    #[test]
    fn security_zone_repr_values() {
        assert_eq!(SecurityZone::Core as u8, 0);
        assert_eq!(SecurityZone::Ephemeral as u8, 4);
    }

    // -- StorageTier tests --

    #[test]
    fn storage_tier_all_variants() {
        let tiers = [StorageTier::Hot, StorageTier::Warm, StorageTier::Cold];
        assert_eq!(tiers.len(), 3);
        assert_eq!(StorageTier::Hot as u8, 0);
        assert_eq!(StorageTier::Cold as u8, 2);
    }

    // -- StorageError tests --

    #[test]
    fn storage_error_all_variants_are_copy() {
        let errors = [
            StorageError::BlockNotFound,
            StorageError::ChecksumFailed,
            StorageError::DecryptionFailed,
            StorageError::IoError,
            StorageError::QuotaExceeded,
            StorageError::DeviceFull,
            StorageError::WalFull,
            StorageError::SuperblockCorrupt,
            StorageError::DeviceNotFound,
            StorageError::VirtioError,
            StorageError::MemTableFull,
            StorageError::ObjectNotFound,
            StorageError::SpaceNotFound,
            StorageError::SpaceNotEmpty,
            StorageError::VersionNotFound,
        ];
        assert_eq!(errors.len(), 15);
        // Verify Copy by assignment.
        let e = StorageError::IoError;
        let e2 = e;
        assert_eq!(e, e2);
    }

    // -- Constant assertions --

    #[test]
    fn superblock_magic_is_aiospace() {
        let bytes = SUPERBLOCK_MAGIC.to_be_bytes();
        assert_eq!(&bytes, b"AIOSPACE");
    }

    #[test]
    fn data_start_sector_math() {
        assert_eq!(DATA_START_SECTOR, WAL_START_SECTOR + WAL_SIZE_SECTORS);
        assert_eq!(DATA_START_SECTOR, 131_080);
    }

    #[test]
    fn wal_entries_per_sector() {
        assert_eq!(WAL_ENTRIES_PER_SECTOR, 8);
        assert_eq!(WAL_ENTRY_SIZE * WAL_ENTRIES_PER_SECTOR, SECTOR_SIZE);
    }

    #[test]
    fn wal_size_is_64_mib() {
        assert_eq!(WAL_SIZE_SECTORS * SECTOR_SIZE as u64, 64 * 1024 * 1024);
    }

    #[test]
    fn sector_and_block_sizes() {
        assert_eq!(SECTOR_SIZE, 512);
        assert_eq!(BLOCK_SIZE, 4096);
        assert_eq!(BLOCK_SIZE / SECTOR_SIZE, 8);
    }

    // -- VirtIO constant tests --

    #[test]
    fn virtio_mmio_magic_is_virt() {
        let bytes = VIRTIO_MMIO_MAGIC.to_le_bytes();
        assert_eq!(&bytes, b"virt");
    }

    #[test]
    fn virtio_mmio_region_stride() {
        assert_eq!(VIRTIO_MMIO_REGION_STRIDE, 0x200);
        assert_eq!(VIRTIO_MMIO_SLOT_COUNT, 32);
    }

    #[test]
    fn virtio_status_bits_no_overlap() {
        let all = VIRTIO_STATUS_ACKNOWLEDGE
            | VIRTIO_STATUS_DRIVER
            | VIRTIO_STATUS_DRIVER_OK
            | VIRTIO_STATUS_FEATURES_OK
            | VIRTIO_STATUS_NEEDS_RESET;
        // Each bit is distinct.
        assert_eq!(all, 1 | 2 | 4 | 8 | 64);
    }

    #[test]
    fn virtio_blk_feature_bits() {
        assert_eq!(VIRTIO_BLK_F_SIZE_MAX, 1 << 1);
        assert_eq!(VIRTIO_BLK_F_SEG_MAX, 1 << 2);
        assert_eq!(VIRTIO_BLK_F_BLK_SIZE, 1 << 6);
        assert_eq!(VIRTIO_F_VERSION_1, 1 << 32);
    }

    #[test]
    fn virtio_register_offsets_ordered() {
        assert!(VIRTIO_MMIO_MAGIC_VALUE < VIRTIO_MMIO_VERSION);
        assert!(VIRTIO_MMIO_VERSION < VIRTIO_MMIO_DEVICE_ID);
        assert!(VIRTIO_MMIO_DEVICE_ID < VIRTIO_MMIO_DEVICE_FEATURES);
        assert!(VIRTIO_MMIO_STATUS < VIRTIO_MMIO_QUEUE_DESC_LOW);
        assert!(VIRTIO_MMIO_QUEUE_USED_HIGH < VIRTIO_MMIO_CONFIG_GENERATION);
        assert!(VIRTIO_MMIO_CONFIG_GENERATION < VIRTIO_MMIO_CONFIG_SPACE);
    }

    // -- VirtIO struct tests --

    #[test]
    fn virtq_desc_size() {
        assert_eq!(core::mem::size_of::<VirtqDesc>(), 16);
    }

    #[test]
    fn virtio_blk_req_header_size() {
        assert_eq!(core::mem::size_of::<VirtioBlkReqHeader>(), 16);
    }

    #[test]
    fn block_location_size() {
        // offset(8) + size(4) + tier(1) + padding(3) = 16
        assert_eq!(core::mem::size_of::<BlockLocation>(), 16);
    }

    #[test]
    fn virtq_desc_flags() {
        assert_eq!(VIRTQ_DESC_F_NEXT, 1);
        assert_eq!(VIRTQ_DESC_F_WRITE, 2);
    }

    // -- ObjectId ordering tests --

    #[test]
    fn object_id_ordering() {
        let a = ObjectId([0u8; 16]);
        let mut b = ObjectId([0u8; 16]);
        b.0[0] = 1;
        assert!(a < b);
        assert!(b > a);
    }

    #[test]
    fn space_id_ordering() {
        let a = SpaceId([0u8; 16]);
        let mut b = SpaceId([0u8; 16]);
        b.0[15] = 1;
        assert!(a < b);
    }

    // -- CompactObject tests --

    #[test]
    fn compact_object_size_is_512() {
        assert_eq!(core::mem::size_of::<CompactObject>(), 512);
    }

    #[test]
    fn compact_object_copy() {
        let a = CompactObject::ZERO;
        let b = a;
        assert_eq!(a.id, b.id);
        assert_eq!(a.content_size, b.content_size);
    }

    #[test]
    fn compact_object_zero_sentinel() {
        let obj = CompactObject::ZERO;
        assert!(obj.is_zero());
        assert_eq!(obj.id, ObjectId::ZERO);
        assert_eq!(obj.content_size, 0);
        assert_eq!(obj.name_len, 0);
        assert_eq!(obj.text_len, 0);
    }

    #[test]
    fn compact_object_name_helpers() {
        let mut obj = CompactObject::ZERO;
        obj.set_name(b"hello.txt");
        assert_eq!(obj.name_bytes(), b"hello.txt");
        assert_eq!(obj.name_len, 9);
    }

    #[test]
    fn compact_object_name_truncation() {
        let mut obj = CompactObject::ZERO;
        let long_name = [b'x'; 128];
        obj.set_name(&long_name);
        assert_eq!(obj.name_len as usize, MAX_OBJECT_NAME_LEN);
        assert_eq!(obj.name_bytes().len(), MAX_OBJECT_NAME_LEN);
    }

    #[test]
    fn compact_object_text_helpers() {
        let mut obj = CompactObject::ZERO;
        obj.set_text(b"some extracted text");
        assert_eq!(obj.text_bytes(), b"some extracted text");
        assert_eq!(obj.text_len, 19);
    }

    #[test]
    fn compact_object_text_truncation() {
        let mut obj = CompactObject::ZERO;
        let long_text = [b'a'; 256];
        obj.set_text(&long_text);
        assert_eq!(obj.text_len as usize, MAX_TEXT_CONTENT_LEN);
    }

    // -- Version tests --

    #[test]
    fn version_size_is_256() {
        assert_eq!(core::mem::size_of::<Version>(), 256);
    }

    #[test]
    fn version_copy() {
        let a = Version::ZERO;
        let b = a;
        assert_eq!(a.hash, b.hash);
        assert_eq!(a.content_size, b.content_size);
    }

    #[test]
    fn version_zero_parent_is_root() {
        let v = Version::ZERO;
        assert!(v.is_root());
    }

    #[test]
    fn version_nonzero_parent_is_not_root() {
        let mut v = Version::ZERO;
        v.parent.0[0] = 1;
        assert!(!v.is_root());
    }

    #[test]
    fn version_message_helpers() {
        let mut v = Version::ZERO;
        v.set_message(b"initial commit");
        assert_eq!(v.message_bytes(), b"initial commit");
        assert_eq!(v.message_len, 14);
    }

    #[test]
    fn version_message_truncation() {
        let mut v = Version::ZERO;
        let long = [b'm'; 128];
        v.set_message(&long);
        assert_eq!(v.message_len as usize, MAX_VERSION_MESSAGE_LEN);
    }

    // -- ProvenanceEntry tests --

    #[test]
    fn provenance_entry_size() {
        assert_eq!(core::mem::size_of::<ProvenanceEntry>(), 144);
    }

    #[test]
    fn provenance_entry_copy() {
        let a = ProvenanceEntry::ZERO;
        let b = a;
        assert_eq!(a.action, b.action);
        assert_eq!(a.has_task, b.has_task);
    }

    #[test]
    fn provenance_action_all_variants() {
        let actions = [
            ProvenanceAction::Created,
            ProvenanceAction::Modified,
            ProvenanceAction::Derived,
            ProvenanceAction::Imported,
            ProvenanceAction::AiGenerated,
        ];
        assert_eq!(actions.len(), 5);
        assert_eq!(ProvenanceAction::Created as u8, 0);
        assert_eq!(ProvenanceAction::AiGenerated as u8, 4);
    }

    #[test]
    fn provenance_entry_signature_zeroed() {
        let p = ProvenanceEntry::ZERO;
        assert_eq!(p.signature, [0u8; 64]);
    }

    // -- Space tests --

    #[test]
    fn space_size_is_128() {
        assert_eq!(core::mem::size_of::<Space>(), 128);
    }

    #[test]
    fn space_copy() {
        let a = Space::ZERO;
        let b = a;
        assert_eq!(a.id, b.id);
        assert_eq!(a.object_count, b.object_count);
    }

    #[test]
    fn space_zero_sentinel() {
        let s = Space::ZERO;
        assert!(s.is_zero());
        assert_eq!(s.object_count, 0);
        assert_eq!(s.total_size, 0);
    }

    #[test]
    fn space_name_helpers() {
        let mut s = Space::ZERO;
        s.set_name(b"system");
        assert_eq!(s.name_bytes(), b"system");
        assert_eq!(s.name_len, 6);
    }

    #[test]
    fn space_name_truncation() {
        let mut s = Space::ZERO;
        let long = [b'n'; 64];
        s.set_name(&long);
        assert_eq!(s.name_len as usize, MAX_SPACE_NAME_LEN);
    }

    #[test]
    fn space_quota_set_and_get() {
        let mut s = Space::ZERO;
        let q = SpaceQuota {
            max_bytes: 1024 * 1024,
            max_objects: 100,
            _padding: [0u8; 4],
        };
        s.set_quota(q);
        let q2 = s.quota();
        assert_eq!(q2.max_bytes, 1024 * 1024);
        assert_eq!(q2.max_objects, 100);
    }

    #[test]
    fn space_quota_enforcement() {
        let mut s = Space::ZERO;
        s.set_quota(SpaceQuota {
            max_bytes: 1000,
            max_objects: 10,
            _padding: [0u8; 4],
        });

        // Under quota
        s.object_count = 5;
        s.total_size = 500;
        assert!(!s.would_exceed_quota(100));

        // Object count at limit
        s.object_count = 10;
        assert!(s.would_exceed_quota(1));

        // Byte size would overflow
        s.object_count = 5;
        s.total_size = 950;
        assert!(s.would_exceed_quota(51));
        assert!(!s.would_exceed_quota(50));
    }

    // -- SpaceQuota tests --

    #[test]
    fn space_quota_size() {
        assert_eq!(core::mem::size_of::<SpaceQuota>(), 16);
    }

    #[test]
    fn space_quota_default_is_unlimited() {
        let q = SpaceQuota::default();
        assert_eq!(q.max_bytes, u64::MAX);
        assert_eq!(q.max_objects, u32::MAX);
    }

    #[test]
    fn space_quota_unlimited() {
        let q = SpaceQuota::UNLIMITED;
        assert_eq!(q.max_bytes, u64::MAX);
        assert_eq!(q.max_objects, u32::MAX);
    }

    // -- EncryptionState tests --

    #[test]
    fn encryption_state_copy_and_repr() {
        let a = EncryptionState::DeviceOnly;
        let b = a;
        assert_eq!(a, b);
        assert_eq!(EncryptionState::DeviceOnly as u8, 0);
        assert_eq!(EncryptionState::SpaceEncrypted as u8, 1);
    }

    // -- ObjectIndexEntry tests --

    #[test]
    fn object_index_entry_size() {
        assert_eq!(core::mem::size_of::<ObjectIndexEntry>(), 32);
    }

    // -- compute_version_hash tests --

    #[test]
    fn compute_version_hash_deterministic() {
        let parent = ContentHash([1u8; 32]);
        let content = ContentHash([2u8; 32]);
        let ts = Timestamp(12345);
        let oid = ObjectId([3u8; 16]);

        let h1 = compute_version_hash(&parent, &content, ts, &oid);
        let h2 = compute_version_hash(&parent, &content, ts, &oid);
        assert_eq!(h1, h2);
        assert!(!h1.is_zero());
    }

    #[test]
    fn compute_version_hash_different_inputs_different_hashes() {
        let parent = ContentHash([1u8; 32]);
        let content = ContentHash([2u8; 32]);
        let ts = Timestamp(12345);
        let oid = ObjectId([3u8; 16]);

        let h1 = compute_version_hash(&parent, &content, ts, &oid);

        // Different parent
        let mut parent2 = parent;
        parent2.0[0] = 99;
        let h2 = compute_version_hash(&parent2, &content, ts, &oid);
        assert_ne!(h1, h2);

        // Different content
        let mut content2 = content;
        content2.0[0] = 99;
        let h3 = compute_version_hash(&parent, &content2, ts, &oid);
        assert_ne!(h1, h3);

        // Different timestamp
        let h4 = compute_version_hash(&parent, &content, Timestamp(99999), &oid);
        assert_ne!(h1, h4);

        // Different object id
        let mut oid2 = oid;
        oid2.0[0] = 99;
        let h5 = compute_version_hash(&parent, &content, ts, &oid2);
        assert_ne!(h1, h5);
    }

    // -- M14 constant tests --

    #[test]
    fn m14_constants() {
        assert_eq!(MAX_OBJECT_NAME_LEN, 64);
        assert_eq!(MAX_AUTHOR_LEN, 32);
        assert_eq!(MAX_SPACES, 16);
        assert_eq!(MAX_SPACE_NAME_LEN, 32);
        assert_eq!(MAX_VERSION_MESSAGE_LEN, 64);
        assert_eq!(OBJECT_INDEX_MAX_ENTRIES, 16_384);
        assert_eq!(MAX_TEXT_CONTENT_LEN, 128);
        assert_eq!(ENCRYPTION_OVERHEAD, 28);
    }

    // -- M15 POSIX / compression / budget tests --

    #[test]
    fn m15_posix_flags() {
        assert_eq!(posix_flags::O_RDONLY, 0);
        assert_eq!(posix_flags::O_WRONLY, 1);
        assert_eq!(posix_flags::O_RDWR, 2);
        assert_eq!(posix_flags::O_CREAT, 0x40);
        assert_eq!(posix_flags::O_APPEND, 0x400);
    }

    #[test]
    fn m15_posix_stat_size() {
        assert_eq!(core::mem::size_of::<PosixStat>(), 24);
    }

    #[test]
    fn m15_compression_type_repr() {
        assert_eq!(CompressionType::None as u8, 0);
        assert_eq!(CompressionType::Lz4 as u8, 1);
        assert_eq!(COMPRESSION_HEADER_SIZE, 5);
    }

    #[test]
    fn m15_pressure_level_normal() {
        assert_eq!(
            PressureLevel::from_free_percentage(100),
            PressureLevel::Normal
        );
        assert_eq!(
            PressureLevel::from_free_percentage(21),
            PressureLevel::Normal
        );
    }

    #[test]
    fn m15_pressure_level_warning() {
        assert_eq!(
            PressureLevel::from_free_percentage(20),
            PressureLevel::Warning
        );
        assert_eq!(
            PressureLevel::from_free_percentage(11),
            PressureLevel::Warning
        );
    }

    #[test]
    fn m15_pressure_level_critical() {
        assert_eq!(
            PressureLevel::from_free_percentage(10),
            PressureLevel::Critical
        );
        assert_eq!(
            PressureLevel::from_free_percentage(6),
            PressureLevel::Critical
        );
    }

    #[test]
    fn m15_pressure_level_emergency() {
        assert_eq!(
            PressureLevel::from_free_percentage(5),
            PressureLevel::Emergency
        );
        assert_eq!(
            PressureLevel::from_free_percentage(0),
            PressureLevel::Emergency
        );
    }

    #[test]
    fn m15_max_fds() {
        assert_eq!(MAX_FDS, 256);
    }
}
