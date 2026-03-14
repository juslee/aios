//! Object Store — content-addressed objects with deduplication.
//!
//! Objects are metadata records (CompactObject) that reference content blocks
//! in the Block Engine. Deduplication is automatic: storing identical content
//! increments a refcount instead of writing a duplicate block.
//!
//! Per spaces.md §3.3 Objects, §3.3.1 Compact vs Full Objects, §4.2 Write Path.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use shared::storage::*;

use super::block_engine;

// ---------------------------------------------------------------------------
// Object ID generation
// ---------------------------------------------------------------------------

/// Monotonic counter for unique object ID generation.
static OBJECT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique ObjectId from TICK_COUNT + monotonic counter.
///
/// Not cryptographically strong but sufficient for uniqueness within a
/// single device. UUID v4 generation deferred to Phase 13.
pub fn generate_object_id() -> ObjectId {
    let tick = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let counter = OBJECT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&tick.to_le_bytes());
    id[8..].copy_from_slice(&counter.to_le_bytes());
    ObjectId(id)
}

// ---------------------------------------------------------------------------
// Object Index (sorted Vec keyed by ObjectId)
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
            Ok(_) => Err(StorageError::IoError), // Duplicate ID
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

    fn binary_search(&self, id: &ObjectId) -> Result<usize, usize> {
        self.entries.binary_search_by(|e| e.id.cmp(id))
    }
}

// ---------------------------------------------------------------------------
// Object Store operations (module-level, use with_engine)
// ---------------------------------------------------------------------------

/// Create a new object with the given name and content.
///
/// 1. Hash content → content_hash (dedup in Block Engine)
/// 2. Store content via Block Engine
/// 3. Create CompactObject metadata
/// 4. Insert into object index
/// 5. Return ObjectId
pub fn object_create(
    space_id: SpaceId,
    name: &[u8],
    content: &[u8],
    content_type: ContentType,
) -> Result<(ObjectId, ContentHash), StorageError> {
    block_engine::with_engine(|engine| {
        // Write content to Block Engine (handles dedup internally).
        let (content_hash, _loc) = engine.write_block(content)?;

        // Generate unique ID.
        let id = generate_object_id();

        // Build CompactObject metadata.
        let mut obj = CompactObject::ZERO;
        obj.id = id;
        obj.space_id = space_id;
        obj.set_name(name);
        obj.content_hash = content_hash;
        obj.content_type = content_type;
        obj.content_size = content.len() as u32;

        let tick = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let now = Timestamp(tick);
        obj.created_at = now;
        obj.modified_at = now;

        // Extract text content for full-text index (simple: store first 128 bytes if text-like).
        if matches!(
            content_type,
            ContentType::Text | ContentType::Code | ContentType::Markdown | ContentType::Json
        ) {
            obj.set_text(content);
        }

        // Create initial version hash (no parent for first version).
        let version_hash =
            shared::storage::compute_version_hash(&ContentHash::ZERO, &content_hash, now, &id);
        obj.version_head = version_hash;

        // Insert into object index.
        engine.object_index_mut().insert(obj)?;

        // Increment space's object count for quota enforcement.
        if let Some(space) = engine.space_table_mut().get_mut(&space_id) {
            space.object_count += 1;
            space.total_size += content.len() as u64;
        }

        Ok((id, content_hash))
    })?
}

/// Read an object's metadata and content by ObjectId.
///
/// Returns (CompactObject, content_bytes_read) into the provided buffer.
pub fn object_read(id: &ObjectId, buf: &mut [u8]) -> Result<(CompactObject, usize), StorageError> {
    block_engine::with_engine(|engine| {
        let obj = engine
            .object_index()
            .get(id)
            .copied()
            .ok_or(StorageError::ObjectNotFound)?;

        let n = engine.read_block_by_hash(&obj.content_hash, buf)?;
        Ok((obj, n))
    })?
}

/// Delete an object by ObjectId.
///
/// Decrements the content block's refcount, walks the version chain to free
/// all version node blocks and their referenced content blocks, and updates
/// the space's object count.
pub fn object_delete(id: &ObjectId) -> Result<(), StorageError> {
    block_engine::with_engine(|engine| {
        let obj = engine
            .object_index_mut()
            .remove(id)
            .ok_or(StorageError::ObjectNotFound)?;

        // Decrement current content block refcount.
        let _ = engine.dec_ref(&obj.content_hash);

        // Walk version chain and free version node blocks + their content references.
        let mut current_hash = obj.version_head;
        let mut buf = [0u8; 256];
        let max_depth = 1024;
        for _ in 0..max_depth {
            if current_hash.is_zero() {
                break;
            }
            let n = match engine.read_block_by_hash(&current_hash, &mut buf) {
                Ok(n) => n,
                Err(StorageError::BlockNotFound) => break,
                Err(_) => break,
            };
            if n != core::mem::size_of::<Version>() {
                break;
            }
            // SAFETY: Version is repr(C), 256 bytes, plain data.
            // Maintained by compile-time assertion: size_of::<Version>() == 256.
            let ver = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Version) };
            // Free the version's referenced content block (if different from current).
            if ver.content_hash != obj.content_hash {
                let _ = engine.dec_ref(&ver.content_hash);
            }
            // Free the version node block itself.
            let next = ver.parent;
            let _ = engine.dec_ref(&current_hash);
            current_hash = next;
        }

        // Decrement space's object count.
        if let Some(space) = engine.space_table_mut().get_mut(&obj.space_id) {
            space.object_count = space.object_count.saturating_sub(1);
            space.total_size = space.total_size.saturating_sub(obj.content_size as u64);
        }

        Ok(())
    })?
}

/// Compute SHA-256 hash of data (convenience wrapper for external callers).
#[allow(dead_code)]
pub fn compute_content_hash(data: &[u8]) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    ContentHash(hash)
}
