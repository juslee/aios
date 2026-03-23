//! POSIX Bridge — path mapping and file operations for Space Storage.
//!
//! Maps POSIX-style paths to Space + Object operations:
//!   /spaces/[name]/[path] → space lookup by name + object access
//!   /home/user/            → user/home/ space
//!   /tmp/                  → ephemeral/ space
//!
//! Per spaces.md §9.1 Path Mapping.

use alloc::vec::Vec;

use shared::storage::{
    posix_flags, ContentType, DirEntry, ObjectId, PosixStat, SpaceId, StorageError, MAX_FDS,
    MAX_OBJECT_NAME_LEN,
};

use super::{block_engine, object_store, version_store};

// ---------------------------------------------------------------------------
// File descriptor table
// ---------------------------------------------------------------------------

/// A single open file descriptor entry.
#[allow(dead_code)]
struct FdEntry {
    space_id: SpaceId,
    object_id: ObjectId,
    /// Read/write cursor position.
    offset: u64,
    /// POSIX flags (O_RDONLY, O_WRONLY, O_RDWR, O_CREAT, O_APPEND).
    flags: u32,
    /// True if this fd represents a directory.
    is_directory: bool,
}

/// POSIX bridge state. Global singleton for Phase 4 (per-process in future).
pub struct PosixSpaceBridge {
    fd_table: [Option<FdEntry>; MAX_FDS],
    next_fd: usize,
}

impl PosixSpaceBridge {
    /// Create a new empty POSIX bridge.
    pub const fn new() -> Self {
        // Note: Option<FdEntry> is not Copy, but const init with None is safe.
        // We use a manual array init since [None; 256] requires Copy.
        const NONE_FD: Option<FdEntry> = None;
        Self {
            fd_table: [NONE_FD; MAX_FDS],
            next_fd: 0,
        }
    }

    /// Allocate a file descriptor slot.
    fn alloc_fd(&mut self) -> Result<usize, StorageError> {
        // Linear scan from next_fd, wrapping around.
        for i in 0..MAX_FDS {
            let idx = (self.next_fd + i) % MAX_FDS;
            if self.fd_table[idx].is_none() {
                self.next_fd = (idx + 1) % MAX_FDS;
                return Ok(idx);
            }
        }
        Err(StorageError::FdTableFull)
    }

    /// Open a file by POSIX path.
    ///
    /// Returns a file descriptor on success.
    pub fn open(&mut self, path: &[u8], flags: u32) -> Result<usize, StorageError> {
        let (space_id, obj_name) = resolve_path(path)?;

        // Look up existing object.
        let existing_id = block_engine::with_engine(|engine| {
            engine.object_index().find_by_name(&space_id, obj_name)
        })?;

        let (object_id, is_dir) = if let Some(id) = existing_id {
            // Check if it's a directory by reading the object metadata.
            let obj = block_engine::with_engine(|engine| engine.object_index().get(&id).copied())?
                .ok_or(StorageError::ObjectNotFound)?;
            (id, obj.content_type == ContentType::Directory)
        } else if flags & posix_flags::O_CREAT != 0 {
            // Create new file with empty content (single null byte — block engine
            // requires non-empty data; POSIX read returns 0 bytes for size-0 objects).
            let (id, _hash) =
                object_store::object_create(space_id, obj_name, b"\0", ContentType::Text)?;
            (id, false)
        } else {
            return Err(StorageError::ObjectNotFound);
        };

        let fd = self.alloc_fd()?;
        self.fd_table[fd] = Some(FdEntry {
            space_id,
            object_id,
            offset: 0,
            flags,
            is_directory: is_dir,
        });

        Ok(fd)
    }

    /// Read from an open file descriptor.
    ///
    /// Returns number of bytes read.
    pub fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize, StorageError> {
        let entry = self
            .fd_table
            .get(fd)
            .and_then(|e| e.as_ref())
            .ok_or(StorageError::InvalidFd)?;

        // Check read permission.
        let mode = entry.flags & 0x3;
        if mode == posix_flags::O_WRONLY {
            return Err(StorageError::IoError);
        }

        let object_id = entry.object_id;
        let offset = entry.offset as usize;

        // Read full content into a temporary buffer, then copy from offset.
        let mut full_buf = [0u8; 4096];
        let (_obj, total) = object_store::object_read(&object_id, &mut full_buf)?;

        if offset >= total {
            return Ok(0); // EOF
        }

        let available = total - offset;
        let to_copy = available.min(buf.len());
        buf[..to_copy].copy_from_slice(&full_buf[offset..offset + to_copy]);

        // Advance offset.
        if let Some(Some(entry)) = self.fd_table.get_mut(fd) {
            entry.offset += to_copy as u64;
        }

        Ok(to_copy)
    }

    /// Write to an open file descriptor.
    ///
    /// Returns number of bytes written.
    pub fn write(&mut self, fd: usize, data: &[u8]) -> Result<usize, StorageError> {
        let entry = self
            .fd_table
            .get(fd)
            .and_then(|e| e.as_ref())
            .ok_or(StorageError::InvalidFd)?;

        // Check write permission.
        let mode = entry.flags & 0x3;
        if mode == posix_flags::O_RDONLY {
            return Err(StorageError::IoError);
        }

        let object_id = entry.object_id;
        let is_append = entry.flags & posix_flags::O_APPEND != 0;
        let offset = if is_append { u64::MAX } else { entry.offset };

        // Read current content.
        let mut current = [0u8; 4096];
        let current_len = match object_store::object_read(&object_id, &mut current) {
            Ok((_, n)) => n,
            Err(StorageError::ObjectNotFound) => 0,
            Err(e) => return Err(e),
        };

        // Build new content, preserving trailing bytes beyond the write region.
        let write_offset = if offset == u64::MAX {
            current_len
        } else {
            (offset as usize).min(current_len)
        };

        // Guard against usize overflow before computing write_end.
        if data.len() > 4096 || write_offset.saturating_add(data.len()) > 4096 {
            return Err(StorageError::QuotaExceeded);
        }
        let write_end = write_offset + data.len();
        let new_len = write_end.max(current_len);

        let mut new_content = [0u8; 4096];
        new_content[..write_offset].copy_from_slice(&current[..write_offset]);
        new_content[write_offset..write_end].copy_from_slice(data);
        // Preserve trailing bytes from original content beyond the write region.
        if current_len > write_end {
            new_content[write_end..current_len].copy_from_slice(&current[write_end..current_len]);
        }

        // Update object via version store (creates new version).
        version_store::object_update(&object_id, &new_content[..new_len], b"posix", b"write")?;

        // Advance offset by bytes written (POSIX: offset += count).
        if let Some(Some(entry)) = self.fd_table.get_mut(fd) {
            entry.offset = (write_offset + data.len()) as u64;
        }

        Ok(data.len())
    }

    /// Close a file descriptor.
    pub fn close(&mut self, fd: usize) -> Result<(), StorageError> {
        if fd >= MAX_FDS || self.fd_table[fd].is_none() {
            return Err(StorageError::InvalidFd);
        }
        self.fd_table[fd] = None;
        Ok(())
    }

    /// Stat a path (without opening).
    pub fn stat(&self, path: &[u8]) -> Result<PosixStat, StorageError> {
        let (space_id, obj_name) = resolve_path(path)?;

        // Empty obj_name means the space root (directory).
        if obj_name.is_empty() {
            return Ok(PosixStat {
                size: 0,
                mode: 0o755,
                modified: 0,
                nlink: 1,
            });
        }

        let obj_id = block_engine::with_engine(|engine| {
            engine.object_index().find_by_name(&space_id, obj_name)
        })?
        .ok_or(StorageError::ObjectNotFound)?;

        let obj = block_engine::with_engine(|engine| engine.object_index().get(&obj_id).copied())?
            .ok_or(StorageError::ObjectNotFound)?;

        let mode = match obj.content_type {
            ContentType::Directory => 0o755,
            _ => 0o644,
        };

        Ok(PosixStat {
            size: obj.content_size as u64,
            mode,
            modified: obj.modified_at.0,
            nlink: 1,
        })
    }

    /// Read directory entries for a path.
    pub fn readdir(&self, path: &[u8]) -> Result<Vec<DirEntry>, StorageError> {
        let (space_id, _prefix) = resolve_path(path)?;

        let object_ids =
            block_engine::with_engine(|engine| engine.object_index().list_by_space(&space_id))?;

        let mut entries = Vec::new();
        for oid in &object_ids {
            let obj = block_engine::with_engine(|engine| engine.object_index().get(oid).copied())?
                .ok_or(StorageError::ObjectNotFound)?;

            let mut entry = DirEntry {
                name: [0u8; MAX_OBJECT_NAME_LEN],
                name_len: 0,
                object_id: *oid,
                content_type: obj.content_type,
                size: obj.content_size as u64,
            };
            let name = obj.name_bytes();
            let len = name.len().min(MAX_OBJECT_NAME_LEN);
            entry.name[..len].copy_from_slice(&name[..len]);
            entry.name_len = len as u32;
            entries.push(entry);
        }

        Ok(entries)
    }

    /// Create a directory (Directory object with null-byte sentinel).
    #[allow(dead_code)]
    pub fn mkdir(&mut self, path: &[u8]) -> Result<(), StorageError> {
        let (space_id, dir_name) = resolve_path(path)?;

        if dir_name.is_empty() {
            return Err(StorageError::IoError);
        }

        // Check if name already exists.
        let exists = block_engine::with_engine(|engine| {
            engine.object_index().find_by_name(&space_id, dir_name)
        })?;

        if exists.is_some() {
            return Err(StorageError::NameExists);
        }

        // Use a single null byte as directory sentinel (Block Engine rejects empty data).
        object_store::object_create(space_id, dir_name, b"\0", ContentType::Directory)?;
        Ok(())
    }

    /// Remove a file.
    pub fn unlink(&mut self, path: &[u8]) -> Result<(), StorageError> {
        let (space_id, obj_name) = resolve_path(path)?;

        let obj_id = block_engine::with_engine(|engine| {
            engine.object_index().find_by_name(&space_id, obj_name)
        })?
        .ok_or(StorageError::ObjectNotFound)?;

        object_store::object_delete(&obj_id)
    }
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// Resolve a POSIX path to (SpaceId, object_name).
///
/// Rules:
///   /spaces/<name>/<path> → space lookup by name, object = path
///   /home/user/<path>     → user/home/ space, object = path
///   /tmp/<path>           → ephemeral/ space, object = path
fn resolve_path(path: &[u8]) -> Result<(SpaceId, &[u8]), StorageError> {
    // Strip leading slash.
    let path = if path.first() == Some(&b'/') {
        &path[1..]
    } else {
        path
    };

    // Reject path traversal components (.. and .).
    if contains_traversal(path) {
        return Err(StorageError::SpaceNotFound);
    }

    if starts_with(path, b"spaces/") {
        // /spaces/<name>/<path>
        let rest = &path[7..]; // skip "spaces/"
        let (name, obj_path) = split_first_component(rest);
        let space = find_space_by_name(name)?;
        Ok((space, obj_path))
    } else if starts_with(path, b"home/user/") {
        let obj_path = &path[10..]; // skip "home/user/"
        let space = find_space_by_name(b"user/home")?;
        Ok((space, obj_path))
    } else if starts_with(path, b"home/user") && path.len() == 9 {
        let space = find_space_by_name(b"user/home")?;
        Ok((space, b""))
    } else if starts_with(path, b"tmp/") {
        let obj_path = &path[4..]; // skip "tmp/"
        let space = find_space_by_name(b"ephemeral")?;
        Ok((space, obj_path))
    } else if path == b"tmp" {
        let space = find_space_by_name(b"ephemeral")?;
        Ok((space, b""))
    } else {
        Err(StorageError::SpaceNotFound)
    }
}

/// Find a space by name using the SpaceTable.
fn find_space_by_name(name: &[u8]) -> Result<SpaceId, StorageError> {
    block_engine::with_engine(|engine| {
        engine
            .space_table()
            .find_by_name(name)
            .map(|s| s.id)
            .ok_or(StorageError::SpaceNotFound)
    })?
}

/// Check if `haystack` starts with `needle`.
fn starts_with(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && &haystack[..needle.len()] == needle
}

/// Reject paths containing `.` or `..` components (path traversal defense).
fn contains_traversal(path: &[u8]) -> bool {
    // Split on '/' and check each component.
    let mut start = 0;
    loop {
        let end = path[start..]
            .iter()
            .position(|&b| b == b'/')
            .map(|p| start + p)
            .unwrap_or(path.len());
        let component = &path[start..end];
        if component == b"." || component == b".." {
            return true;
        }
        if end >= path.len() {
            break;
        }
        start = end + 1;
    }
    false
}

/// Split path into (space_name, object_path) by trying progressively longer
/// prefixes until a matching space is found.
///
/// e.g., "user/home/foo.txt" → tries "user" (no match) → tries "user/home" (match) → ("user/home", "foo.txt")
/// e.g., "system/config.txt" → tries "system" (match) → ("system", "config.txt")
fn split_first_component(path: &[u8]) -> (&[u8], &[u8]) {
    // Try progressively longer prefixes at each '/' boundary.
    let mut search_from = 0;
    while let Some(rel_pos) = path[search_from..].iter().position(|&b| b == b'/') {
        let pos = search_from + rel_pos;
        let candidate = &path[..pos];
        // Check if this prefix matches a space name.
        let found = block_engine::with_engine(|engine| {
            engine.space_table().find_by_name(candidate).is_some()
        })
        .unwrap_or(false);
        if found {
            return (candidate, &path[pos + 1..]);
        }
        search_from = pos + 1;
    }
    // No '/' remaining or no match found — treat entire path as space name.
    (path, b"")
}
