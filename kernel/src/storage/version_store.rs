//! Version Store — Merkle DAG for object versioning.
//!
//! Every object modification creates a new version node linked to its parent
//! by hash. The chain forms a Merkle DAG that supports listing, rollback,
//! and tamper detection.
//!
//! Per spaces.md §5.1 Merkle DAG, §5.3 DAG Operations.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use shared::storage::*;

use super::block_engine;

// ---------------------------------------------------------------------------
// Version Store operations
// ---------------------------------------------------------------------------

/// Create a new version for an object.
///
/// Stores the Version node as a content-addressed block in the Block Engine,
/// then updates the object's version_head pointer.
#[allow(dead_code)]
pub fn version_create(
    object_id: &ObjectId,
    content_hash: ContentHash,
    content_size: u32,
    author: &[u8],
    message: &[u8],
) -> Result<ContentHash, StorageError> {
    block_engine::with_engine(|engine| {
        // Look up current head version for this object.
        let obj = engine
            .object_index()
            .get(object_id)
            .ok_or(StorageError::ObjectNotFound)?;
        let parent = obj.version_head;

        // Build Version node.
        let tick = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let now = Timestamp(tick);

        let version_hash = compute_version_hash(&parent, &content_hash, now, object_id);

        let mut version = Version::ZERO;
        version.hash = version_hash;
        version.parent = parent;
        version.content_hash = content_hash;
        version.content_size = content_size;
        version.object_id = *object_id;
        version.timestamp = now;
        version.set_message(message);
        // Copy author.
        let author_len = author.len().min(MAX_AUTHOR_LEN);
        version.author[..author_len].copy_from_slice(&author[..author_len]);

        // Store version node as a block in the Block Engine.
        // SAFETY: Version is repr(C), 256 bytes, plain data.
        let version_bytes = unsafe {
            core::slice::from_raw_parts(
                &version as *const Version as *const u8,
                core::mem::size_of::<Version>(),
            )
        };
        engine.write_block(version_bytes)?;

        // Update object's version_head.
        let obj_mut = engine
            .object_index_mut()
            .get_mut(object_id)
            .ok_or(StorageError::ObjectNotFound)?;
        obj_mut.version_head = version_hash;

        Ok(version_hash)
    })?
}

/// List all versions of an object by walking the parent chain from head.
///
/// Returns versions in newest-first order (head → root).
pub fn version_list(object_id: &ObjectId) -> Result<Vec<Version>, StorageError> {
    block_engine::with_engine(|engine| {
        let obj = engine
            .object_index()
            .get(object_id)
            .ok_or(StorageError::ObjectNotFound)?;

        let mut versions = Vec::new();
        let mut current_hash = obj.version_head;
        let mut buf = [0u8; 256];

        // Walk the chain (bounded to prevent infinite loops).
        let max_depth = 1024;
        for _ in 0..max_depth {
            if current_hash.is_zero() {
                break;
            }

            let n = match engine.read_block_by_hash(&current_hash, &mut buf) {
                Ok(n) => n,
                Err(StorageError::BlockNotFound) => break,
                Err(e) => return Err(e),
            };

            if n != core::mem::size_of::<Version>() {
                break;
            }

            // SAFETY: Version is repr(C), 256 bytes, all plain data.
            // read_unaligned handles potential alignment issues from buf.
            let version = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Version) };
            current_hash = version.parent;
            versions.push(version);
        }

        Ok(versions)
    })?
}

/// Rollback an object to a target version.
///
/// Creates a *new* version node (parent = current head, content = target's content).
/// This preserves the full audit trail — rollback is recorded as a new version.
pub fn version_rollback(
    object_id: &ObjectId,
    target_hash: &ContentHash,
) -> Result<(), StorageError> {
    block_engine::with_engine(|engine| {
        // Verify target version exists and belongs to this object.
        let mut buf = [0u8; 256];
        let n = engine
            .read_block_by_hash(target_hash, &mut buf)
            .map_err(|_| StorageError::VersionNotFound)?;

        if n != core::mem::size_of::<Version>() {
            return Err(StorageError::VersionNotFound);
        }

        // SAFETY: Version is repr(C), 256 bytes.
        let target_version = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Version) };

        if target_version.object_id != *object_id {
            return Err(StorageError::VersionNotFound);
        }

        // Get current head to use as parent of the rollback version.
        let obj = engine
            .object_index()
            .get(object_id)
            .ok_or(StorageError::ObjectNotFound)?;
        let current_head = obj.version_head;
        let old_content_hash = obj.content_hash;

        // Create new version with target's content.
        let tick = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let now = Timestamp(tick);

        let version_hash =
            compute_version_hash(&current_head, &target_version.content_hash, now, object_id);

        let mut version = Version::ZERO;
        version.hash = version_hash;
        version.parent = current_head;
        version.content_hash = target_version.content_hash;
        version.content_size = target_version.content_size;
        version.object_id = *object_id;
        version.timestamp = now;
        version.set_message(b"rollback");

        // Store the rollback version node.
        let version_bytes = unsafe {
            core::slice::from_raw_parts(
                &version as *const Version as *const u8,
                core::mem::size_of::<Version>(),
            )
        };
        engine.write_block(version_bytes)?;

        // Increment refcount on target content (now referenced by rollback version).
        engine.inc_ref(&target_version.content_hash)?;

        // Update object metadata.
        let obj_mut = engine
            .object_index_mut()
            .get_mut(object_id)
            .ok_or(StorageError::ObjectNotFound)?;
        obj_mut.version_head = version_hash;
        obj_mut.content_hash = target_version.content_hash;
        obj_mut.content_size = target_version.content_size;
        obj_mut.modified_at = now;

        // Decrement refcount on old content (the pre-rollback content).
        if old_content_hash != target_version.content_hash {
            let _ = engine.dec_ref(&old_content_hash);
        }

        Ok(())
    })?
}

/// Update an object's content, creating a new version.
///
/// 1. Write new content to Block Engine
/// 2. Create version node (parent = current head)
/// 3. Update CompactObject metadata
/// 4. Decrement refcount on old content
pub fn object_update(
    object_id: &ObjectId,
    new_content: &[u8],
    author: &[u8],
    message: &[u8],
) -> Result<ContentHash, StorageError> {
    block_engine::with_engine(|engine| {
        // Get current object state.
        let obj = engine
            .object_index()
            .get(object_id)
            .ok_or(StorageError::ObjectNotFound)?;
        let old_content_hash = obj.content_hash;
        let current_head = obj.version_head;

        // Write new content to Block Engine.
        let (new_hash, _loc) = engine.write_block(new_content)?;

        // Build version node.
        let tick = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let now = Timestamp(tick);

        let version_hash = compute_version_hash(&current_head, &new_hash, now, object_id);

        let mut version = Version::ZERO;
        version.hash = version_hash;
        version.parent = current_head;
        version.content_hash = new_hash;
        version.content_size = new_content.len() as u32;
        version.object_id = *object_id;
        version.timestamp = now;
        version.set_message(message);
        let author_len = author.len().min(MAX_AUTHOR_LEN);
        version.author[..author_len].copy_from_slice(&author[..author_len]);

        // Store version node.
        let version_bytes = unsafe {
            core::slice::from_raw_parts(
                &version as *const Version as *const u8,
                core::mem::size_of::<Version>(),
            )
        };
        engine.write_block(version_bytes)?;

        // Update object metadata.
        let obj_mut = engine
            .object_index_mut()
            .get_mut(object_id)
            .ok_or(StorageError::ObjectNotFound)?;
        obj_mut.content_hash = new_hash;
        obj_mut.content_size = new_content.len() as u32;
        obj_mut.version_head = version_hash;
        obj_mut.modified_at = now;

        // Decrement refcount on old content.
        if old_content_hash != new_hash {
            let _ = engine.dec_ref(&old_content_hash);
        }

        Ok(new_hash)
    })?
}
