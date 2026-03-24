//! Storage Kit — block storage, space management, object store, and versioning.
//!
//! Architecture reference: `docs/kits/platform/storage.md`

extern crate alloc;

use alloc::vec::Vec;

// Re-export key storage types so consumers can import via the Kit module.
pub use crate::storage::{
    BlockId, CompactObject, ContentHash, ContentType, ObjectId, PressureLevel, SecurityZone, Space,
    SpaceId, StorageBudget, StorageError, Version,
};

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Low-level content-addressed block storage.
///
/// Blocks are identified by their SHA-256 content hash. Writing the same data
/// twice returns the same `BlockId` (content-addressed deduplication).
pub trait BlockStore {
    /// Write data to the block store, returning its content hash as block ID.
    fn write_block(&mut self, data: &[u8]) -> Result<BlockId, StorageError>;

    /// Read a block by its content hash, returning the raw data.
    fn read_block(&self, id: &BlockId) -> Result<Vec<u8>, StorageError>;

    /// Check whether a block with the given content hash exists.
    fn block_exists(&self, id: &BlockId) -> bool;
}

/// Space (container) lifecycle management.
///
/// Spaces are the top-level organizational unit in AIOS storage. Each Space
/// has a name, security zone, and quota. The system creates three built-in
/// Spaces at boot: `system/`, `user/home/`, and `ephemeral/`.
pub trait SpaceManager {
    /// Create a new space with the given name and security zone.
    fn create_space(&mut self, name: &str, zone: SecurityZone) -> Result<SpaceId, StorageError>;

    /// Retrieve space metadata by ID.
    fn get_space(&self, id: &SpaceId) -> Result<Space, StorageError>;

    /// List all spaces.
    fn list_spaces(&self) -> Vec<Space>;

    /// Delete a space. Fails if the space is not empty.
    fn delete_space(&mut self, id: &SpaceId) -> Result<(), StorageError>;

    /// Current storage usage statistics.
    fn storage_budget(&self) -> StorageBudget;

    /// Current storage pressure level.
    fn pressure_level(&self) -> PressureLevel;
}

/// Object (file-like content unit) operations within spaces.
///
/// Objects are the fundamental data unit. Each object belongs to exactly one
/// Space, has a content type, and is stored with content-addressed integrity.
pub trait ObjectStore {
    /// Create a new object in the specified space.
    fn create_object(
        &mut self,
        space_id: &SpaceId,
        name: &str,
        content_type: ContentType,
        data: &[u8],
    ) -> Result<ObjectId, StorageError>;

    /// Read an object's data by ID.
    fn read_object(&self, id: &ObjectId) -> Result<Vec<u8>, StorageError>;

    /// Delete an object and all its versions.
    fn delete_object(&mut self, id: &ObjectId) -> Result<(), StorageError>;

    /// List all objects in a space.
    fn list_objects(&self, space_id: &SpaceId) -> Vec<CompactObject>;
}

/// Version history operations (Merkle DAG).
///
/// Every object modification creates a new version linked to its parent by
/// hash. The chain forms a Merkle DAG enabling rollback and audit.
pub trait VersionStoreOps {
    /// Create a new version of an object with updated data and a commit message.
    fn create_version(
        &mut self,
        object_id: &ObjectId,
        data: &[u8],
        message: &str,
    ) -> Result<ContentHash, StorageError>;

    /// List all versions of an object, newest first.
    fn list_versions(&self, object_id: &ObjectId) -> Vec<Version>;

    /// Get the current (head) version hash for an object.
    fn get_head(&self, object_id: &ObjectId) -> Result<ContentHash, StorageError>;

    /// Roll back to a previous version, creating a new version with restored content.
    fn rollback(
        &mut self,
        object_id: &ObjectId,
        version_hash: &ContentHash,
    ) -> Result<ContentHash, StorageError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time dyn-compatibility assertions.
    fn _assert_block_store_dyn_compat(_: &dyn BlockStore) {}
    fn _assert_space_manager_dyn_compat(_: &dyn SpaceManager) {}
    fn _assert_object_store_dyn_compat(_: &dyn ObjectStore) {}
    fn _assert_version_store_ops_dyn_compat(_: &dyn VersionStoreOps) {}

    #[test]
    fn storage_error_reexported() {
        // Verify StorageError is accessible through the Kit module.
        let err = StorageError::BlockNotFound;
        assert_eq!(err, StorageError::BlockNotFound);
    }

    #[test]
    fn pressure_level_reexported() {
        let level = PressureLevel::Normal;
        assert_eq!(level, PressureLevel::Normal);
    }

    #[test]
    fn storage_types_accessible() {
        // Verify key types are re-exported and constructible.
        let _hash = ContentHash::ZERO;
        let _space_id = SpaceId::ZERO;
        let _object_id = ObjectId::ZERO;
        let _obj = CompactObject::ZERO;
        let _ver = Version::ZERO;
    }
}
