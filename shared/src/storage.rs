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
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
#[repr(transparent)]
pub struct ObjectId(pub [u8; 16]);

impl ObjectId {
    pub const ZERO: Self = Self([0u8; 16]);
}

/// 128-bit space identifier.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
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
    /// Total block size in bytes (header + data + padding).
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
        ];
        assert_eq!(errors.len(), 11);
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
}
