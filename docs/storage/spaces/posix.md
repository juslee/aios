# AIOS Space Storage — POSIX Compatibility

Part of: [spaces.md](../spaces.md) — Space Storage System
**Related:** [data-structures.md](./data-structures.md) — Core Data Structures, [block-engine.md](./block-engine.md) — Block Engine, [versioning.md](./versioning.md) — Version Store

-----

## 9. POSIX Compatibility

### 9.1 Path Mapping

The POSIX emulation layer maps filesystem paths to space operations:

```text
/spaces/[space-name]/[object-path]  →  space query + object access
/home/user/                          →  user/home/ space
/tmp/                                →  ephemeral space (auto-cleaned; no version history,
                                        device-encrypted only, cleared on shutdown)
/dev/null, /dev/urandom             →  device capabilities
/proc/self/                          →  process introspection
/bin/, /usr/bin/                     →  system utilities space
```

**Path resolution:** `/spaces/research/papers/ml/bert.pdf` resolves to space-name `"research"` (first component after `/spaces/`) and object-path `"papers/ml/bert.pdf"` (remaining components). Objects with `/` in their name (uncommon) are URL-encoded as `%2F` in the POSIX path. The POSIX bridge decodes on translation.

### 9.2 Translation Layer

```rust
/// POSIX directory entry, returned by readdir().
pub struct DirEntry {
    name: String,
    object_id: ObjectId,
    content_type: ContentType,
    size: u64,
    modified_at: Timestamp,
}

/// POSIX stat result, returned by stat().
pub struct Stat {
    size: u64,
    modified: u64,                      // seconds since epoch
    mode: u32,                          // synthesized POSIX mode bits
    nlink: u32,                         // always 1 (spaces don't have hard links)
}
```

Object methods used by the POSIX bridge:

```rust
impl Object {
    /// Convert to POSIX directory entry.
    pub fn to_dir_entry(&self) -> DirEntry {
        DirEntry {
            name: self.name.clone(),
            object_id: self.id,
            content_type: self.content_type,
            size: self.content_size,
            modified_at: self.modified_at,
        }
    }

    /// Synthesize POSIX mode bits from capabilities.
    /// Read bits set if calling agent has ReadSpace; write bits if WriteSpace.
    /// Directories get 0o755; files get 0o644 by default.
    pub fn to_posix_mode(&self) -> u32 {
        if self.content_type == ContentType::Directory { 0o755 } else { 0o644 }
    }
}
```

```rust
/// Standard POSIX types used in this section:
///   OpenFlags — bitflags (O_RDONLY, O_WRONLY, O_RDWR, O_CREAT, O_EXCL, etc.)
///   AccessMode, Mode — POSIX permission types
///   CapabilitySet — kernel capability set (architecture.md §3.2)

pub struct PosixSpaceBridge {
    mount_table: Vec<MountEntry>,
}

pub struct MountEntry {
    posix_path: String,                 // "/spaces/research"
    space: SpaceId,
    capabilities: CapabilitySet,        // from calling process's agent (architecture.md §3.2)
}

impl PosixSpaceBridge {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Fd> {
        let (space, object_path) = self.resolve_path(path)?;
        let cap = if flags.intersects(O_WRONLY | O_RDWR) {
            Capability::WriteSpace(space)
        } else {
            Capability::ReadSpace(space)
        };
        gate_check(current_agent(), cap)?;
        let object = match space.resolve_object(object_path) {
            Ok(obj) => {
                if flags.contains(O_CREAT | O_EXCL) {
                    return Err(Error::NameExists); // EEXIST
                }
                obj
            }
            Err(Error::ObjectNotFound) if flags.contains(O_CREAT) => {
                space.create_object(object_path, ContentType::Document, &[])?
            }
            Err(e) => return Err(e),
        };
        Ok(self.create_fd(object, flags))
    }

    fn readdir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let (space, prefix) = self.resolve_path(path)?;
        let object_ids = space.query(SpaceQuery::Filter {
            parent: Some(prefix),
            ..Default::default()
        })?;
        // query() returns Vec<ObjectId>; resolve each to a full Object.
        // Errors from get_object() are propagated — partial results could
        // hide permission or I/O failures from the caller.
        let objects: Vec<Object> = object_ids.iter()
            .map(|id| space.get_object(*id))
            .collect::<Result<Vec<_>>>()?;
        Ok(objects.iter().map(|o| o.to_dir_entry()).collect())
    }

    fn stat(&self, path: &str) -> Result<Stat> {
        let (space, object_path) = self.resolve_path(path)?;
        let object = space.resolve_object(object_path)?;
        Ok(Stat {
            size: object.content_size,
            modified: object.modified_at.as_millis() / 1000, // seconds since epoch
            mode: object.to_posix_mode(),
            // ...
        })
    }
}
```

BSD tools never know they're not on a traditional filesystem. `ls /spaces/research/` returns a directory listing. `grep` searches file content. `cat` reads objects. The translation is transparent.

POSIX syscall translations are dispatched through IPC to the Space Storage service. See [ipc.md §12.2 Gap 6](../../kernel/ipc.md) for the POSIX translation performance model (5 μs round-trip target) and the read-ahead, vnode cache, batched readdir, and write-coalescing optimizations that amortize IPC cost.

### 9.3 Write Path

The POSIX bridge translates mutation syscalls into space operations:

```rust
impl PosixSpaceBridge {
    fn write(&self, fd: Fd, buf: &[u8]) -> Result<usize> {
        let file = self.fd_table.get_mut(fd)?;
        gate_check(current_agent(), Capability::WriteSpace(file.space))?;
        // Buffer writes in the fd's write buffer (write coalescing —
        // see ipc.md §12.2 Gap 6). Flush to Space Storage service on fsync/close
        // or when buffer is full (default 64 KB).
        file.write_buf.extend_from_slice(buf);
        if file.write_buf.len() >= WRITE_COALESCE_THRESHOLD {
            self.flush(fd)?;
        }
        file.cursor += buf.len() as u64;
        Ok(buf.len())
    }

    fn close(&self, fd: Fd) -> Result<()> {
        let file = self.fd_table.get(fd)?;
        // Flush any buffered writes
        if !file.write_buf.is_empty() {
            self.flush(fd)?;
        }
        // Release the fd. If this is the last reference (no dup'd copies),
        // the object handle is released back to the Space Storage service.
        self.fd_table.release(fd)
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<()> {
        let (src_space, src_obj_path) = self.resolve_path(old_path)?;
        let (dst_space, dst_obj_path) = self.resolve_path(new_path)?;
        gate_check(current_agent(), Capability::WriteSpace(src_space))?;
        if src_space != dst_space {
            gate_check(current_agent(), Capability::WriteSpace(dst_space))?;
        }
        // Rename is a metadata update — the content blocks are unchanged.
        // Cross-space rename is a copy + delete (atomic via WAL).
        src_space.rename_object(src_obj_path, dst_space, dst_obj_path)
    }

    fn unlink(&self, path: &str) -> Result<()> {
        let (space, object_path) = self.resolve_path(path)?;
        gate_check(current_agent(), Capability::WriteSpace(space))?;
        // Unlink removes the object from the space. The version DAG (§5)
        // is retained — the object can be recovered via rollback until
        // version retention prunes the history (§5.4).
        space.delete_object(object_path)
    }

    fn mkdir(&self, path: &str, _mode: Mode) -> Result<()> {
        let (space, dir_path) = self.resolve_path(path)?;
        gate_check(current_agent(), Capability::WriteSpace(space))?;
        // Directories in spaces are implicit — they exist when objects
        // have matching prefixes. mkdir creates a zero-length marker object
        // with content_type Directory so that readdir returns the directory
        // even when empty.
        space.create_object(dir_path, ContentType::Directory, &[])
    }
}

const WRITE_COALESCE_THRESHOLD: usize = 64 * 1024; // 64 KB
```

Write coalescing applies to syscall-based writes (`write`, `pwrite`). `mmap` is not supported in Phase 4 (spaces use content-addressed blocks, not page-granularity storage). `O_DIRECT` flag is ignored — all writes go through the coalesce buffer for consistency.

**Atomicity:** `rename` within a single space is atomic (single LSM-tree key update, WAL-protected). Cross-space rename is atomic via a compound WAL entry: `[type=COMPOUND_OP][op1=DELETE src_space/src_path][op2=CREATE dst_space/dst_path content_hash][checksum]`. On crash recovery, WAL replay either commits both operations or neither — intermediate states (source deleted, destination not created) never persist to the LSM-tree.

### 9.4 File Descriptor Lifecycle

File descriptors are the POSIX bridge's core state. Each open fd tracks its object binding, cursor position, and buffered I/O state:

```rust
pub struct OpenFile {
    fd: Fd,
    space: SpaceId,
    object: ObjectId,
    /// The version hash this fd was opened against. Reads always see this
    /// version's content, even if the object is modified by another agent
    /// after open. This is snapshot isolation — consistent with POSIX
    /// semantics where open() returns a stable file reference.
    pinned_version: Hash,
    cursor: u64,
    flags: OpenFlags,
    mode: AccessMode,
    /// Reference count. Incremented by dup/dup2/fork. The fd is released
    /// only when refcount drops to zero.
    refcount: u32,
    /// Write buffer for coalesced writes (§9.3).
    write_buf: Vec<u8>,
    /// Read-ahead buffer (ipc.md §12.2 Gap 6).
    read_buf: ReadAheadBuffer,
}

pub struct ReadAheadBuffer {
    data: [u8; READ_AHEAD_SIZE],
    /// Range of the object currently cached in this buffer.
    /// Coherent with pinned_version: the buffer always contains data from
    /// the pinned version. If another agent modifies the object, a new version
    /// is created but the pinned version's content blocks are immutable, so
    /// the read-ahead buffer remains valid.
    cached_range: Option<Range<u64>>,
}

const READ_AHEAD_SIZE: usize = 64 * 1024; // 64 KB
```

**Version pinning:** When a file is opened, the fd records the current head version of the object. All reads through this fd return data from the pinned version. If another agent modifies the object while the fd is open, the fd still sees the old content. This matches POSIX behavior where `open()` + `read()` is not affected by concurrent `write()` to the same file (assuming no shared mmap). New `open()` calls see the latest version.

**dup / fork semantics:** `dup(fd)` increments the refcount on the `OpenFile` — the duplicate shares the same cursor, buffers, and pinned version. `fork()` duplicates the fd table for the child process, incrementing all refcounts. The `OpenFile` is released when all references are closed.

**What happens on object deletion:** If another agent deletes the object while an fd is open, reads through the existing fd continue to work (the pinned version's content blocks are still in the Block Engine — version retention guarantees this). New `open()` calls to the same path return `ENOENT`. This matches POSIX unlink semantics where open file handles survive deletion. In the rare case where the pinned version is garbage-collected due to extreme storage pressure (§5.4), subsequent reads through the fd return `EIO`. In practice, this is unlikely because version retention always preserves at least the current head and most recent snapshot, and fds are typically short-lived.

### 9.5 Error Mapping

Space operations produce structured errors. The POSIX bridge maps them to errno values:

| Space error | POSIX errno | Notes |
|---|---|---|
| `ObjectNotFound` | `ENOENT` | Object does not exist at this path |
| `SpaceNotFound` | `ENOENT` | Space does not exist |
| `CapabilityDenied` | `EACCES` | Agent lacks the required capability |
| `ReadOnlySpace` | `EROFS` | Write to a pull-only synced or system space |
| `SpaceFull` | `ENOSPC` | Space quota exceeded (§10) |
| `DeviceFull` | `ENOSPC` | Device storage exhausted |
| `ObjectLocked` | `EBUSY` | Object is exclusively locked by another operation |
| `InvalidPath` | `EINVAL` | Path contains invalid characters or exceeds length |
| `NameExists` | `EEXIST` | Object already exists at this path (for O_CREAT \| O_EXCL) |
| `TooManyOpenFiles` | `EMFILE` | Process fd table full |
| `VersionConflict` | `EAGAIN` | Concurrent modification detected; retry |
| `EncryptionKeyUnavailable` | `EACCES` | Space is encrypted and the key is not loaded (screen locked) |
| `IoError` | `EIO` | Block Engine or storage driver error |

**Unmapped POSIX concepts:** Spaces do not have traditional POSIX mode bits (`rwxrwxrwx`). The `stat()` call (§9.2) synthesizes mode bits from capabilities: if the calling agent has `ReadSpace`, the read bits are set; if it has `WriteSpace`, the write bits are set. Group and other bits are not meaningful — the capability system replaces POSIX user/group/other permissions. `chmod` and `chown` are no-ops that return success (POSIX compliance without effect, since capabilities are the real access control).

### 9.6 Change Notification

POSIX tools expect filesystem event APIs (`inotify` on Linux, `kqueue` on BSD). The POSIX bridge maps these to space event subscriptions:

```rust
impl PosixSpaceBridge {
    /// inotify_add_watch equivalent. Subscribes to changes on objects
    /// matching a path prefix within a space.
    fn watch(&self, path: &str, events: WatchEvents) -> Result<WatchId> {
        let (space, prefix) = self.resolve_path(path)?;
        gate_check(current_agent(), Capability::ReadSpace(space))?;
        // The Version Store (§5) emits events whenever a new version node
        // is appended. The watch subscription filters these events by
        // object path prefix and event type.
        let sub = space.subscribe(SpaceEventFilter {
            prefix: Some(prefix),
            event_types: events.to_space_events(),
        })?;
        Ok(self.watch_table.register(sub))
    }
}

/// Watch subscription identifier. Returned by watch(), used by unwatch().
pub type WatchId = u64;

pub struct SpaceEventFilter {
    /// Object path prefix to watch (None = entire space).
    prefix: Option<String>,
    /// Event types to subscribe to.
    event_types: Vec<SpaceEventType>,
}

pub enum SpaceEventType {
    Created,                // new object in prefix
    Modified,               // new version appended
    Deleted,                // object deleted
    Renamed,                // object renamed
}

pub struct WatchEvents {
    pub create: bool,    // IN_CREATE → SpaceEventType::Created
    pub modify: bool,    // IN_MODIFY → SpaceEventType::Modified
    pub delete: bool,    // IN_DELETE → SpaceEventType::Deleted
    pub rename: bool,    // IN_MOVED_FROM/TO → SpaceEventType::Renamed
}
```

The Version Store already tracks every modification as a version node (§5). Change notification is a read-only view of version events filtered by path prefix — no new storage machinery is needed. Events are delivered asynchronously via the IPC notification mechanism (ipc.md §3.1 `NotificationSignal`). Tools like `tail -f`, `fswatch`, and build systems with file watchers work transparently.

-----
