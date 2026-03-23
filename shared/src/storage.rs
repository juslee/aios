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

    /// Get extracted text content as a byte slice. Clamps to array bounds for corruption safety.
    pub fn text_bytes(&self) -> &[u8] {
        let len = (self.text_len as usize).min(MAX_TEXT_CONTENT_LEN);
        &self.text_content[..len]
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

/// Superblock format version (bumped to 2 for compression header format).
pub const SUPERBLOCK_VERSION: u32 = 2;

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
#[derive(Clone, Copy, Debug)]
pub struct DirEntry {
    /// Entry name (up to 64 bytes, null-padded).
    pub name: [u8; MAX_OBJECT_NAME_LEN],
    /// Name length in bytes (fixed-width for cross-boundary ABI stability).
    pub name_len: u32,
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
// CRC-32C (Castagnoli) — 256-entry lookup table
// ---------------------------------------------------------------------------

/// CRC-32C lookup table using Castagnoli polynomial 0x1EDC6F41.
const CRC32C_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let poly: u32 = 0x82F6_3B78; // bit-reversed 0x1EDC6F41
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ poly;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Compute CRC-32C checksum of `data`.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc = CRC32C_TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

// ---------------------------------------------------------------------------
// MemTable — in-memory sorted index for content-addressed blocks
// ---------------------------------------------------------------------------

use alloc::vec;
use alloc::vec::Vec;

/// In-memory sorted index entry.
pub struct MemTableEntry {
    pub key: ContentHash,
    pub location: BlockLocation,
    /// Reference count (separate from BlockLocation per arch doc).
    pub refcount: u32,
}

/// Sorted array MemTable for content-addressed block lookups.
///
/// Heap-allocated sorted array with binary search for O(log n) lookups.
/// Capacity: configurable (default 65536 entries).
///
/// Per spaces.md §4.2.
pub struct MemTable {
    entries: Vec<MemTableEntry>,
    capacity: usize,
}

impl MemTable {
    /// Create a new empty MemTable with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: vec![],
            capacity,
        }
    }

    /// Create a MemTable with default capacity (MEMTABLE_MAX_ENTRIES).
    pub fn with_default_capacity() -> Self {
        Self::new(MEMTABLE_MAX_ENTRIES)
    }

    /// Number of entries.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Is the table full?
    pub fn is_full(&self) -> bool {
        self.entries.len() >= self.capacity
    }

    /// Look up a block by content hash.
    pub fn get(&self, key: &ContentHash) -> Option<&MemTableEntry> {
        let idx = self.binary_search(key).ok()?;
        Some(&self.entries[idx])
    }

    /// Look up a block by content hash (mutable, for refcount updates).
    pub fn get_mut(&mut self, key: &ContentHash) -> Option<&mut MemTableEntry> {
        let idx = self.binary_search(key).ok()?;
        Some(&mut self.entries[idx])
    }

    /// Insert a new entry. If key already exists, increments refcount instead.
    ///
    /// Returns `true` if this was a new insertion, `false` if dedup (refcount bump).
    pub fn insert(
        &mut self,
        key: ContentHash,
        location: BlockLocation,
    ) -> Result<bool, StorageError> {
        match self.binary_search(&key) {
            Ok(idx) => {
                // Key exists — increment refcount (dedup).
                self.entries[idx].refcount += 1;
                Ok(false)
            }
            Err(insert_pos) => {
                if self.is_full() {
                    return Err(StorageError::MemTableFull);
                }
                self.entries.insert(
                    insert_pos,
                    MemTableEntry {
                        key,
                        location,
                        refcount: 1,
                    },
                );
                Ok(true)
            }
        }
    }

    /// Remove an entry by key.
    pub fn remove(&mut self, key: &ContentHash) -> Option<BlockLocation> {
        let idx = self.binary_search(key).ok()?;
        let entry = self.entries.remove(idx);
        Some(entry.location)
    }

    /// Decrement refcount for a key. If refcount reaches 0, removes the entry.
    ///
    /// Returns `Some((location, freed))` where `freed` is true if the entry was removed.
    /// Returns `None` if the key was not found.
    pub fn dec_ref(&mut self, key: &ContentHash) -> Option<(BlockLocation, bool)> {
        let idx = self.binary_search(key).ok()?;
        let entry = &mut self.entries[idx];
        let location = entry.location;
        if entry.refcount <= 1 {
            self.entries.remove(idx);
            Some((location, true))
        } else {
            entry.refcount -= 1;
            Some((location, false))
        }
    }

    /// Binary search for a key. Returns Ok(index) if found, Err(insert_pos) if not.
    fn binary_search(&self, key: &ContentHash) -> Result<usize, usize> {
        self.entries
            .binary_search_by(|entry| entry.key.0.cmp(&key.0))
    }
}

// ---------------------------------------------------------------------------
// ObjectIndex — sorted Vec keyed by ObjectId
// ---------------------------------------------------------------------------

/// Entry in the object index: ObjectId → CompactObject metadata.
struct ObjectEntry {
    id: ObjectId,
    object: CompactObject,
}

/// Sorted index of objects, keyed by ObjectId. Binary search for O(log n) lookups.
pub struct ObjectIndex {
    entries: Vec<ObjectEntry>,
}

impl Default for ObjectIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectIndex {
    /// Create an empty object index.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Number of objects in the index.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Look up an object by ID.
    pub fn get(&self, id: &ObjectId) -> Option<&CompactObject> {
        let idx = self.binary_search(id).ok()?;
        Some(&self.entries[idx].object)
    }

    /// Look up an object by ID (mutable).
    pub fn get_mut(&mut self, id: &ObjectId) -> Option<&mut CompactObject> {
        let idx = self.binary_search(id).ok()?;
        Some(&mut self.entries[idx].object)
    }

    /// Insert a new object. Returns error if index is full or ID already exists.
    pub fn insert(&mut self, object: CompactObject) -> Result<(), StorageError> {
        if self.entries.len() >= OBJECT_INDEX_MAX_ENTRIES {
            return Err(StorageError::MemTableFull);
        }
        match self.binary_search(&object.id) {
            Ok(_) => Err(StorageError::NameExists), // Duplicate ID
            Err(pos) => {
                self.entries.insert(
                    pos,
                    ObjectEntry {
                        id: object.id,
                        object,
                    },
                );
                Ok(())
            }
        }
    }

    /// Remove an object by ID. Returns the removed CompactObject.
    pub fn remove(&mut self, id: &ObjectId) -> Option<CompactObject> {
        let idx = self.binary_search(id).ok()?;
        Some(self.entries.remove(idx).object)
    }

    /// Find an object by space and name (linear scan).
    pub fn find_by_name(&self, space_id: &SpaceId, name: &[u8]) -> Option<ObjectId> {
        self.entries
            .iter()
            .find(|e| e.object.space_id == *space_id && e.object.name_bytes() == name)
            .map(|e| e.id)
    }

    /// List all object IDs in a given space (linear scan).
    pub fn list_by_space(&self, space_id: &SpaceId) -> Vec<ObjectId> {
        self.entries
            .iter()
            .filter(|e| e.object.space_id == *space_id)
            .map(|e| e.id)
            .collect()
    }

    fn binary_search(&self, id: &ObjectId) -> Result<usize, usize> {
        self.entries.binary_search_by(|e| e.id.cmp(id))
    }
}

// ---------------------------------------------------------------------------
// SpaceTable — fixed-size array of optional spaces
// ---------------------------------------------------------------------------

/// In-memory space registry. Fixed-size array of optional spaces.
pub struct SpaceTable {
    spaces: [Option<Space>; MAX_SPACES],
}

impl Default for SpaceTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SpaceTable {
    /// Create an empty space table.
    pub const fn new() -> Self {
        Self {
            spaces: [None; MAX_SPACES],
        }
    }

    /// Number of active spaces.
    pub fn count(&self) -> usize {
        self.spaces.iter().filter(|s| s.is_some()).count()
    }

    /// Find a space by ID.
    pub fn get(&self, id: &SpaceId) -> Option<&Space> {
        self.spaces
            .iter()
            .filter_map(|s| s.as_ref())
            .find(|s| s.id == *id)
    }

    /// Find a space by ID (mutable, used for quota updates).
    pub fn get_mut(&mut self, id: &SpaceId) -> Option<&mut Space> {
        self.spaces
            .iter_mut()
            .filter_map(|s| s.as_mut())
            .find(|s| s.id == *id)
    }

    /// Insert a new space. Returns error if table is full.
    pub fn insert(&mut self, space: Space) -> Result<(), StorageError> {
        for slot in self.spaces.iter_mut() {
            if slot.is_none() {
                *slot = Some(space);
                return Ok(());
            }
        }
        Err(StorageError::QuotaExceeded)
    }

    /// Remove a space by ID. Returns the removed space.
    pub fn remove(&mut self, id: &SpaceId) -> Option<Space> {
        for slot in self.spaces.iter_mut() {
            if let Some(space) = slot {
                if space.id == *id {
                    return slot.take();
                }
            }
        }
        None
    }

    /// List all active spaces.
    pub fn list(&self) -> Vec<Space> {
        self.spaces.iter().filter_map(|s| *s).collect()
    }

    /// Find a space by name (linear scan).
    pub fn find_by_name(&self, name: &[u8]) -> Option<&Space> {
        self.spaces
            .iter()
            .filter_map(|s| s.as_ref())
            .find(|s| s.name_bytes() == name)
    }
}

// ---------------------------------------------------------------------------
// WalEntry — on-disk WAL entry struct (64 bytes)
// ---------------------------------------------------------------------------

/// On-disk WAL entry (64 bytes, fixed layout).
///
/// The `Wal` struct itself stays in kernel — it calls `virtio_blk::read_sector/write_sector`.
/// Only the entry struct and its pure methods are shared.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WalEntry {
    /// Monotonically increasing sequence number.
    pub sequence_number: u64,
    /// Content hash (SHA-256) of the block data.
    pub block_id: [u8; 32],
    /// Byte offset of the data in the data region (BlockLocation.offset).
    pub data_offset: u64,
    /// Data size in bytes (BlockLocation.size).
    pub data_size: u32,
    /// 0 = pending, 1 = committed.
    pub committed: u8,
    /// Padding.
    _pad: [u8; 3],
    /// CRC-32C of all fields above (bytes 0..56).
    pub checksum: u32,
    /// Padding to 64 bytes.
    _pad2: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<WalEntry>() == WAL_ENTRY_SIZE);

impl WalEntry {
    /// Create a zero-initialized WAL entry.
    pub const ZERO: Self = Self {
        sequence_number: 0,
        block_id: [0; 32],
        data_offset: 0,
        data_size: 0,
        committed: 0,
        _pad: [0; 3],
        checksum: 0,
        _pad2: [0; 4],
    };

    /// Create a new WAL entry with the given fields (padding zeroed, checksum not yet set).
    pub fn new(
        sequence_number: u64,
        block_id: [u8; 32],
        data_offset: u64,
        data_size: u32,
        committed: u8,
    ) -> Self {
        Self {
            sequence_number,
            block_id,
            data_offset,
            data_size,
            committed,
            _pad: [0; 3],
            checksum: 0,
            _pad2: [0; 4],
        }
    }

    /// Compute CRC-32C over the first 56 bytes (everything except checksum + pad2).
    pub fn compute_checksum(&self) -> u32 {
        // SAFETY: WalEntry is repr(C), 64 bytes, plain data (no pointers).
        // Maintained by repr(C) attribute and compile-time size assertion.
        // If violated, CRC is computed over wrong bytes, causing checksum mismatch on validation.
        let bytes = unsafe { core::slice::from_raw_parts(self as *const Self as *const u8, 56) };
        crc32c(bytes)
    }

    /// Check if this entry has a valid checksum.
    pub fn is_valid(&self) -> bool {
        self.checksum == self.compute_checksum()
    }

    /// Get the content hash as a ContentHash.
    pub fn content_hash(&self) -> ContentHash {
        ContentHash(self.block_id)
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
            StorageError::NameExists,
            StorageError::NotADirectory,
            StorageError::FdTableFull,
            StorageError::InvalidFd,
        ];
        assert_eq!(errors.len(), 19);
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

    // -- CRC-32C tests --

    #[test]
    fn crc32c_empty() {
        assert_eq!(crc32c(&[]), 0x0000_0000);
    }

    #[test]
    fn crc32c_known_vector() {
        // Standard test vector: "123456789" → 0xE3069283
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn crc32c_single_byte() {
        let result = crc32c(&[0x00]);
        assert_ne!(result, 0); // non-trivial output
    }

    #[test]
    fn crc32c_all_zeros() {
        let data = [0u8; 64];
        let result = crc32c(&data);
        assert_ne!(result, 0);
    }

    #[test]
    fn crc32c_all_ones() {
        let data = [0xFF; 1024];
        let result = crc32c(&data);
        assert_ne!(result, 0);
    }

    #[test]
    fn crc32c_different_data_different_hash() {
        let a = crc32c(b"hello");
        let b = crc32c(b"world");
        assert_ne!(a, b);
    }

    #[test]
    fn crc32c_same_data_same_hash() {
        assert_eq!(crc32c(b"test"), crc32c(b"test"));
    }

    #[test]
    fn crc32c_single_bit_change() {
        let a = crc32c(&[0x00]);
        let b = crc32c(&[0x01]);
        assert_ne!(a, b);
    }

    // -- MemTable tests --

    fn make_hash(val: u8) -> ContentHash {
        let mut h = [0u8; 32];
        h[0] = val;
        ContentHash(h)
    }

    fn make_location(offset: u64) -> BlockLocation {
        BlockLocation {
            offset,
            size: 512,
            tier: StorageTier::Hot,
        }
    }

    #[test]
    fn memtable_new_empty() {
        let mt = MemTable::new(100);
        assert_eq!(mt.count(), 0);
        assert_eq!(mt.capacity(), 100);
        assert!(!mt.is_full());
    }

    #[test]
    fn memtable_insert_and_get() {
        let mut mt = MemTable::new(10);
        let h = make_hash(42);
        let loc = make_location(1000);
        assert!(mt.insert(h, loc).unwrap()); // new insertion
        assert_eq!(mt.count(), 1);
        let entry = mt.get(&h).unwrap();
        assert_eq!(entry.location.offset, 1000);
        assert_eq!(entry.refcount, 1);
    }

    #[test]
    fn memtable_insert_duplicate_increments_refcount() {
        let mut mt = MemTable::new(10);
        let h = make_hash(1);
        let loc = make_location(100);
        assert!(mt.insert(h, loc).unwrap()); // new
        assert!(!mt.insert(h, loc).unwrap()); // dedup
        assert_eq!(mt.count(), 1);
        assert_eq!(mt.get(&h).unwrap().refcount, 2);
    }

    #[test]
    fn memtable_insert_at_capacity_returns_error() {
        let mut mt = MemTable::new(2);
        mt.insert(make_hash(1), make_location(0)).unwrap();
        mt.insert(make_hash(2), make_location(0)).unwrap();
        let result = mt.insert(make_hash(3), make_location(0));
        assert!(matches!(result, Err(StorageError::MemTableFull)));
    }

    #[test]
    fn memtable_get_missing_returns_none() {
        let mt = MemTable::new(10);
        assert!(mt.get(&make_hash(99)).is_none());
    }

    #[test]
    fn memtable_remove_existing() {
        let mut mt = MemTable::new(10);
        let h = make_hash(5);
        mt.insert(h, make_location(500)).unwrap();
        let loc = mt.remove(&h).unwrap();
        assert_eq!(loc.offset, 500);
        assert_eq!(mt.count(), 0);
    }

    #[test]
    fn memtable_remove_missing_returns_none() {
        let mut mt = MemTable::new(10);
        assert!(mt.remove(&make_hash(99)).is_none());
    }

    #[test]
    fn memtable_dec_ref_decrements() {
        let mut mt = MemTable::new(10);
        let h = make_hash(1);
        mt.insert(h, make_location(0)).unwrap();
        mt.insert(h, make_location(0)).unwrap(); // refcount=2
        let (_, freed) = mt.dec_ref(&h).unwrap();
        assert!(!freed); // refcount=1, not freed
        assert_eq!(mt.get(&h).unwrap().refcount, 1);
    }

    #[test]
    fn memtable_dec_ref_removes_at_zero() {
        let mut mt = MemTable::new(10);
        let h = make_hash(1);
        mt.insert(h, make_location(0)).unwrap(); // refcount=1
        let (_, freed) = mt.dec_ref(&h).unwrap();
        assert!(freed);
        assert_eq!(mt.count(), 0);
    }

    #[test]
    fn memtable_sorted_ordering() {
        let mut mt = MemTable::new(100);
        // Insert in reverse order, verify sorted
        for i in (0..10u8).rev() {
            mt.insert(make_hash(i), make_location(i as u64)).unwrap();
        }
        // All should be findable
        for i in 0..10u8 {
            assert!(mt.get(&make_hash(i)).is_some());
        }
    }

    #[test]
    fn memtable_insert_remove_cycle() {
        let mut mt = MemTable::new(5);
        for i in 0..5u8 {
            mt.insert(make_hash(i), make_location(i as u64)).unwrap();
        }
        assert!(mt.is_full());
        mt.remove(&make_hash(2));
        assert!(!mt.is_full());
        mt.insert(make_hash(20), make_location(20)).unwrap();
        assert!(mt.is_full());
    }

    #[test]
    fn memtable_with_default_capacity() {
        let mt = MemTable::with_default_capacity();
        assert_eq!(mt.capacity(), MEMTABLE_MAX_ENTRIES);
    }

    // -- ObjectIndex tests --

    fn make_object_id(val: u8) -> ObjectId {
        let mut id = [0u8; 16];
        id[0] = val;
        ObjectId(id)
    }

    fn make_space_id(val: u8) -> SpaceId {
        let mut id = [0u8; 16];
        id[0] = val;
        SpaceId(id)
    }

    fn make_compact_object(id: ObjectId, space_id: SpaceId, name: &[u8]) -> CompactObject {
        let mut obj = CompactObject::ZERO;
        obj.id = id;
        obj.space_id = space_id;
        obj.set_name(name);
        obj
    }

    #[test]
    fn object_index_new_empty() {
        let idx = ObjectIndex::new();
        assert_eq!(idx.count(), 0);
    }

    #[test]
    fn object_index_insert_and_get() {
        let mut idx = ObjectIndex::new();
        let oid = make_object_id(1);
        let sid = make_space_id(1);
        let obj = make_compact_object(oid, sid, b"test");
        idx.insert(obj).unwrap();
        assert_eq!(idx.count(), 1);
        let got = idx.get(&oid).unwrap();
        assert_eq!(got.id, oid);
    }

    #[test]
    fn object_index_insert_duplicate_returns_error() {
        let mut idx = ObjectIndex::new();
        let oid = make_object_id(1);
        let sid = make_space_id(1);
        idx.insert(make_compact_object(oid, sid, b"a")).unwrap();
        let result = idx.insert(make_compact_object(oid, sid, b"b"));
        assert!(result.is_err());
    }

    #[test]
    fn object_index_remove_existing() {
        let mut idx = ObjectIndex::new();
        let oid = make_object_id(1);
        let sid = make_space_id(1);
        idx.insert(make_compact_object(oid, sid, b"test")).unwrap();
        let removed = idx.remove(&oid).unwrap();
        assert_eq!(removed.id, oid);
        assert_eq!(idx.count(), 0);
    }

    #[test]
    fn object_index_remove_missing() {
        let mut idx = ObjectIndex::new();
        assert!(idx.remove(&make_object_id(99)).is_none());
    }

    #[test]
    fn object_index_find_by_name() {
        let mut idx = ObjectIndex::new();
        let sid = make_space_id(1);
        idx.insert(make_compact_object(make_object_id(1), sid, b"alpha"))
            .unwrap();
        idx.insert(make_compact_object(make_object_id(2), sid, b"beta"))
            .unwrap();
        let found = idx.find_by_name(&sid, b"beta").unwrap();
        assert_eq!(found, make_object_id(2));
    }

    #[test]
    fn object_index_find_by_name_wrong_space() {
        let mut idx = ObjectIndex::new();
        let sid1 = make_space_id(1);
        let sid2 = make_space_id(2);
        idx.insert(make_compact_object(make_object_id(1), sid1, b"file"))
            .unwrap();
        assert!(idx.find_by_name(&sid2, b"file").is_none());
    }

    #[test]
    fn object_index_find_by_name_not_found() {
        let mut idx = ObjectIndex::new();
        let sid = make_space_id(1);
        idx.insert(make_compact_object(make_object_id(1), sid, b"exists"))
            .unwrap();
        assert!(idx.find_by_name(&sid, b"missing").is_none());
    }

    #[test]
    fn object_index_list_by_space() {
        let mut idx = ObjectIndex::new();
        let sid1 = make_space_id(1);
        let sid2 = make_space_id(2);
        idx.insert(make_compact_object(make_object_id(1), sid1, b"a"))
            .unwrap();
        idx.insert(make_compact_object(make_object_id(2), sid1, b"b"))
            .unwrap();
        idx.insert(make_compact_object(make_object_id(3), sid2, b"c"))
            .unwrap();
        let list = idx.list_by_space(&sid1);
        assert_eq!(list.len(), 2);
        assert!(idx.list_by_space(&make_space_id(99)).is_empty());
    }

    #[test]
    fn object_index_sorted_order() {
        let mut idx = ObjectIndex::new();
        let sid = make_space_id(1);
        // Insert in reverse order
        for i in (1..=10u8).rev() {
            idx.insert(make_compact_object(make_object_id(i), sid, b"x"))
                .unwrap();
        }
        // All should be findable
        for i in 1..=10u8 {
            assert!(idx.get(&make_object_id(i)).is_some());
        }
    }

    #[test]
    fn object_index_get_mut() {
        let mut idx = ObjectIndex::new();
        let oid = make_object_id(1);
        let sid = make_space_id(1);
        idx.insert(make_compact_object(oid, sid, b"test")).unwrap();
        let obj = idx.get_mut(&oid).unwrap();
        obj.content_size = 999;
        assert_eq!(idx.get(&oid).unwrap().content_size, 999);
    }

    // -- SpaceTable tests --

    fn make_space(id: SpaceId, name: &[u8]) -> Space {
        let mut space = Space::ZERO;
        space.id = id;
        space.set_name(name);
        space
    }

    #[test]
    fn space_table_new_empty() {
        let st = SpaceTable::new();
        assert_eq!(st.count(), 0);
    }

    #[test]
    fn space_table_insert_and_get() {
        let mut st = SpaceTable::new();
        let sid = make_space_id(1);
        st.insert(make_space(sid, b"test")).unwrap();
        assert_eq!(st.count(), 1);
        let s = st.get(&sid).unwrap();
        assert_eq!(s.id, sid);
    }

    #[test]
    fn space_table_insert_full() {
        let mut st = SpaceTable::new();
        for i in 0..MAX_SPACES as u8 {
            st.insert(make_space(make_space_id(i + 1), b"s")).unwrap();
        }
        let result = st.insert(make_space(make_space_id(255), b"overflow"));
        assert!(result.is_err());
    }

    #[test]
    fn space_table_remove_existing() {
        let mut st = SpaceTable::new();
        let sid = make_space_id(1);
        st.insert(make_space(sid, b"test")).unwrap();
        let removed = st.remove(&sid).unwrap();
        assert_eq!(removed.id, sid);
        assert_eq!(st.count(), 0);
    }

    #[test]
    fn space_table_remove_missing() {
        let mut st = SpaceTable::new();
        assert!(st.remove(&make_space_id(99)).is_none());
    }

    #[test]
    fn space_table_get_mut() {
        let mut st = SpaceTable::new();
        let sid = make_space_id(1);
        st.insert(make_space(sid, b"test")).unwrap();
        let s = st.get_mut(&sid).unwrap();
        s.object_count = 42;
        assert_eq!(st.get(&sid).unwrap().object_count, 42);
    }

    #[test]
    fn space_table_list() {
        let mut st = SpaceTable::new();
        st.insert(make_space(make_space_id(1), b"a")).unwrap();
        st.insert(make_space(make_space_id(2), b"b")).unwrap();
        let list = st.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn space_table_find_by_name() {
        let mut st = SpaceTable::new();
        st.insert(make_space(make_space_id(1), b"alpha")).unwrap();
        st.insert(make_space(make_space_id(2), b"beta")).unwrap();
        let found = st.find_by_name(b"beta").unwrap();
        assert_eq!(found.id, make_space_id(2));
    }

    #[test]
    fn space_table_find_by_name_not_found() {
        let mut st = SpaceTable::new();
        st.insert(make_space(make_space_id(1), b"exists")).unwrap();
        assert!(st.find_by_name(b"missing").is_none());
    }

    #[test]
    fn space_table_count_after_insert_remove() {
        let mut st = SpaceTable::new();
        let sid1 = make_space_id(1);
        let sid2 = make_space_id(2);
        st.insert(make_space(sid1, b"a")).unwrap();
        st.insert(make_space(sid2, b"b")).unwrap();
        assert_eq!(st.count(), 2);
        st.remove(&sid1);
        assert_eq!(st.count(), 1);
    }

    // -- WalEntry tests --

    #[test]
    fn wal_entry_size_assertion() {
        assert_eq!(core::mem::size_of::<WalEntry>(), 64);
    }

    #[test]
    fn wal_entry_zero_checksum() {
        let entry = WalEntry::ZERO;
        let cs = entry.compute_checksum();
        // Zero entry should have a computable checksum
        assert_ne!(cs, 0); // CRC of all-zeros is non-zero
    }

    #[test]
    fn wal_entry_compute_and_validate() {
        let mut entry = WalEntry::ZERO;
        entry.sequence_number = 42;
        entry.data_offset = 1000;
        entry.data_size = 512;
        entry.checksum = entry.compute_checksum();
        assert!(entry.is_valid());
    }

    #[test]
    fn wal_entry_tamper_invalidates() {
        let mut entry = WalEntry::ZERO;
        entry.sequence_number = 1;
        entry.checksum = entry.compute_checksum();
        assert!(entry.is_valid());
        entry.sequence_number = 2; // tamper
        assert!(!entry.is_valid());
    }

    #[test]
    fn wal_entry_content_hash() {
        let mut entry = WalEntry::ZERO;
        entry.block_id = [0xAB; 32];
        let ch = entry.content_hash();
        assert_eq!(ch.0, [0xAB; 32]);
    }

    #[test]
    fn wal_entry_roundtrip_fields() {
        let mut entry = WalEntry::ZERO;
        entry.sequence_number = 100;
        entry.data_offset = 5000;
        entry.data_size = 256;
        entry.committed = 1;
        entry.checksum = entry.compute_checksum();
        assert!(entry.is_valid());
        assert_eq!(entry.sequence_number, 100);
        assert_eq!(entry.data_offset, 5000);
        assert_eq!(entry.data_size, 256);
        assert_eq!(entry.committed, 1);
    }
}
