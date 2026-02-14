# AIOS POSIX Compatibility Layer

## Deep Technical Architecture

**Parent document:** [aios-architecture.md](../project/aios-architecture.md)
**Related:** [aios-ipc-syscalls.md](../kernel/aios-ipc-syscalls.md) — Syscall interface and POSIX translation table, [aios-subsystem-framework.md](./aios-subsystem-framework.md) — PosixBridge trait and /dev nodes, [aios-spaces.md](../storage/aios-spaces.md) — Space-to-path mapping, [aios-flow.md](../storage/aios-flow.md) — Clipboard POSIX bridge

-----

## 1. Overview

An operating system with no software is a technology demo. AIOS can have the most elegant microkernel, the most innovative space storage, the most intelligent context engine — none of it matters if you cannot run `grep`, compile C code, or SSH into a server. Developer adoption requires immediate productivity, and immediate productivity requires Unix tools.

AIOS solves this without carrying decades of Unix kernel baggage. The approach: take FreeBSD's userland — battle-tested tools under BSD license — and run them on top of a thin POSIX translation layer that converts POSIX system calls into AIOS's native IPC-based architecture. The kernel never sees a POSIX system call. It only sees its own ~20 AIOS syscalls. POSIX is a userspace library, not a kernel commitment.

This is fundamentally different from Linux compatibility layers (WSL, Darling) that try to emulate an entire kernel interface. AIOS doesn't emulate Linux. It provides just enough POSIX semantics — file I/O, process management, pipes, sockets — for BSD tools to work. The translation layer is lean because AIOS's native syscalls were designed with this translation in mind: `ProcessCreate` maps cleanly to `fork()`/`exec()`, `ChannelCreate` maps to `pipe()`, IPC messages to Space Service map to `open()`/`read()`/`write()`.

The result: every FreeBSD command-line tool works. `ls`, `grep`, `awk`, `sed`, `find`, `make`, `clang`, `ssh`, `curl` — all unmodified. The developer who sits down at an AIOS terminal and types `ls` gets a directory listing. They never need to know that underneath, a space query was translated into directory entries. They never need to know that their `pipe()` call created an IPC channel. The abstraction is complete.

-----

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                     BSD Userland (unmodified)                         │
│                                                                      │
│  ls  cp  mv  rm  mkdir  cat  grep  sed  awk  find  sort  diff       │
│  make  clang/lld  ar  nm  strip  tar  gzip  curl  ssh  nvi         │
│  FreeBSD /bin/sh (ash-based, POSIX-compliant, BSD-licensed)         │
│                                                                      │
│  These tools call standard POSIX functions: open(), read(), write(), │
│  fork(), exec(), pipe(), socket(), stat(), readdir(), mmap(), etc.   │
├─────────────────────────────────────────────────────────────────────┤
│                     musl libc (MIT-licensed)                          │
│                                                                      │
│  Standard C library providing POSIX function signatures              │
│  Syscall wrappers modified to target AIOS translation layer          │
│  ~100K lines (vs glibc 1.5M) — portable, auditable                  │
├─────────────────────────────────────────────────────────────────────┤
│                     POSIX Translation Layer                           │
│                                                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────────┐  │
│  │   FD Table    │  │  Path        │  │  Process Lifecycle        │  │
│  │               │  │  Resolver    │  │                           │  │
│  │  maps POSIX   │  │              │  │  fork() → ProcessCreate   │  │
│  │  file descr.  │  │  /spaces/*   │  │  exec() → ProcessCreate   │  │
│  │  to IPC       │  │  /dev/*      │  │  waitpid() → ProcessWait  │  │
│  │  channels     │  │  /home/*     │  │  exit() → ProcessExit     │  │
│  │  and sessions │  │  /tmp/*      │  │  pipe() → ChannelCreate   │  │
│  └──────┬───────┘  │  /proc/*     │  └────────────┬──────────────┘  │
│         │          │  /bin/*      │               │                  │
│         │          └──────┬───────┘               │                  │
│         │                 │                       │                  │
│  ┌──────┴─────────────────┴───────────────────────┴──────────────┐  │
│  │                    IPC Dispatch                                │  │
│  │                                                               │  │
│  │  Routes translated calls to the correct system service:       │  │
│  │    File ops    → Space Service (via IPC)                      │  │
│  │    Socket ops  → Network Service (via IPC)                    │  │
│  │    Device ops  → Subsystem POSIX Bridge (via IPC)             │  │
│  │    Memory ops  → Kernel direct (MemoryMap/MemoryUnmap)        │  │
│  │    Process ops → Kernel direct (ProcessCreate/ProcessExit)    │  │
│  │    Time ops    → Kernel direct (TimeGet/TimeSleep)            │  │
│  └───────────────────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────────────────┤
│                     AIOS Kernel (~20 syscalls)                       │
│                                                                      │
│  IpcCall  IpcSend  IpcRecv  IpcSelect  ChannelCreate                │
│  CapabilityTransfer  CapabilityAttenuate  CapabilityRevoke          │
│  MemoryMap  MemoryUnmap  SharedMemoryCreate  SharedMemoryMap        │
│  ProcessCreate  ProcessExit  ProcessWait                             │
│  TimeGet  TimeSleep  TimerSet  AuditLog                              │
└─────────────────────────────────────────────────────────────────────┘
```

The critical insight: the boundary between "POSIX world" and "AIOS world" is entirely in userspace. The kernel is clean — it has never heard of `open()` or `fork()` or `socket()`. When a future optimization eliminates a translation step, no kernel change is needed. When a BSD tool is eventually rewritten as a native AIOS agent, it simply stops using the translation layer and talks to services directly via IPC.

-----

## 3. Why BSD, Not GNU

This is a licensing and engineering decision, not ideology.

| | FreeBSD Userland | GNU Coreutils |
|---|---|---|
| **License** | BSD-2-Clause (permissive) | GPL (copyleft) |
| **OS distribution** | No copyleft obligations | Must distribute source, may trigger linking concerns |
| **libc coupling** | Works with musl, minimal assumptions | Deeply tied to glibc, uses glibc extensions |
| **Portability** | Proven on macOS, PlayStation, Nintendo Switch, embedded | Primarily Linux, increasingly Linux-specific |
| **Self-contained** | Each tool is a standalone program | Heavy use of shared gnulib |
| **Codebase** | Smaller, more auditable per-tool | Larger, more features per-tool |

**Shell:** FreeBSD `/bin/sh` (ash-based, POSIX-compliant, BSD-licensed). Not bash (GPLv3). Not zsh (MIT, but large and complex). The goal is a minimal POSIX-compliant shell that runs scripts, not a user's interactive daily driver. Users who want bash or zsh or fish can install them — the POSIX layer supports them.

**Compiler:** LLVM/clang (Apache-2.0). Not GCC (GPL). LLVM provides clang (C/C++), lld (linker), compiler-rt (runtime), llvm-ar, llvm-nm, llvm-strip. This makes AIOS self-hosting: it can compile software for itself without any GPL toolchain.

-----

## 4. Why musl, Not glibc

musl is the C library through which all POSIX calls flow. The choice of musl over glibc is fundamental:

| | musl | glibc |
|---|---|---|
| **License** | MIT | LGPL-2.1 |
| **Lines of code** | ~100K | ~1.5M |
| **Static linking** | First-class, produces small binaries | Technically possible, practically broken |
| **Thread-safety** | Designed for it from the start | Retrofitted, some interfaces still unsafe |
| **Linux coupling** | Minimal — designed for portability | Deep Linux-specific dependencies |
| **Custom syscall layer** | Clean hook point for translation | Requires invasive patching |
| **Proven** | Alpine Linux, postmarketOS, embedded | Every mainstream Linux distro |

musl's syscall wrappers are the modification point. In standard musl on Linux, `open()` eventually calls `syscall(__NR_openat, ...)`. In AIOS musl, `open()` calls into the POSIX translation layer instead. The modification is surgical: replace the syscall dispatch with IPC dispatch. Every POSIX function signature remains identical. Every tool that compiles against musl works without source changes.

### 4.1 The musl Modification

```rust
// Standard musl on Linux:
//   open(path, flags) → syscall(SYS_openat, AT_FDCWD, path, flags)
//
// AIOS musl:
//   open(path, flags) → posix_translate_open(path, flags)
//                     → path_resolve(path)
//                     → IPC to Space Service / Subsystem Bridge
//                     → fd_table_insert(ipc_channel)
//                     → return fd

/// musl syscall entry point — redirected to POSIX translation
#[no_mangle]
pub extern "C" fn __syscall_dispatch(num: i64, args: &[i64; 6]) -> i64 {
    match num {
        SYS_OPENAT    => translate_openat(args),
        SYS_READ      => translate_read(args),
        SYS_WRITE     => translate_write(args),
        SYS_CLOSE     => translate_close(args),
        SYS_FSTAT     => translate_fstat(args),
        SYS_LSEEK     => translate_lseek(args),
        SYS_GETDENTS  => translate_getdents(args),
        SYS_FORK      => translate_fork(args),
        SYS_EXECVE    => translate_execve(args),
        SYS_WAITID    => translate_waitid(args),
        SYS_EXIT      => translate_exit(args),
        SYS_PIPE2     => translate_pipe2(args),
        SYS_DUP3      => translate_dup3(args),
        SYS_SOCKET    => translate_socket(args),
        SYS_CONNECT   => translate_connect(args),
        SYS_SENDTO    => translate_sendto(args),
        SYS_RECVFROM  => translate_recvfrom(args),
        SYS_MMAP      => translate_mmap(args),
        SYS_MUNMAP    => translate_munmap(args),
        SYS_IOCTL     => translate_ioctl(args),
        SYS_CLOCK_GETTIME => translate_clock_gettime(args),
        SYS_NANOSLEEP => translate_nanosleep(args),
        // ... ~60 total POSIX syscalls translated
        _ => -ENOSYS,  // unsupported syscall
    }
}
```

Of the ~450 Linux syscalls, only ~60 need translation. The rest are either Linux-specific (epoll, futex, io_uring) or obsolete. BSD tools use a conservative POSIX subset. The translation surface is small enough to test exhaustively.

-----

## 5. File Descriptor Table

The FD table is the central data structure of the POSIX translation layer. It maps integer file descriptors (what BSD tools see) to AIOS resources (IPC channels, space objects, device sessions, pipes).

### 5.1 Data Model

```rust
/// Per-process file descriptor table
pub struct FdTable {
    entries: Vec<Option<FdEntry>>,
    next_fd: i32,
}

pub struct FdEntry {
    kind: FdKind,
    flags: FdFlags,
    offset: u64,          // current read/write position
    status: FdStatus,
}

pub enum FdKind {
    /// Space object (files, directories)
    SpaceObject {
        space: SpaceId,
        object: ObjectId,
        channel: ChannelId,       // IPC channel to Space Service
        shared_mem: SharedMemoryId, // zero-copy buffer
        content_type: ContentType,
    },

    /// Directory handle (readdir iteration)
    Directory {
        space: SpaceId,
        listing: Vec<ObjectSummary>,  // cached listing from Space Service
        position: usize,              // readdir cursor
    },

    /// Pipe (anonymous IPC channel)
    Pipe {
        channel: ChannelId,
        direction: PipeDirection,  // ReadEnd or WriteEnd
    },

    /// Socket (network connection)
    Socket {
        channel: ChannelId,        // IPC channel to Network Service
        domain: SocketDomain,      // AF_INET, AF_INET6, AF_UNIX
        sock_type: SocketType,     // SOCK_STREAM, SOCK_DGRAM
        state: SocketState,        // Unbound, Listening, Connected
    },

    /// Device node (/dev/*)
    Device {
        subsystem: SubsystemId,
        session: SessionId,        // active subsystem session
        channel: ChannelId,        // IPC channel to subsystem's POSIX bridge
    },

    /// Special: stdin/stdout/stderr (connected to terminal agent)
    Terminal {
        channel: ChannelId,        // IPC to terminal agent
        is_tty: bool,
    },

    /// Special: /proc/self/* (read-only process introspection)
    ProcSelf {
        field: ProcField,          // status, cmdline, maps, fd
    },
}

pub struct FdFlags {
    pub close_on_exec: bool,       // O_CLOEXEC
    pub nonblock: bool,            // O_NONBLOCK
    pub append: bool,              // O_APPEND
}

pub enum PipeDirection {
    ReadEnd,
    WriteEnd,
}
```

### 5.2 Standard Descriptors

Every process starts with three file descriptors:

```
fd 0 (stdin)  → Terminal channel (read end)
fd 1 (stdout) → Terminal channel (write end)
fd 2 (stderr) → Terminal channel (write end, may be same as stdout)
```

When a shell sets up a pipeline (`ls | grep foo`), it creates pipes via `ChannelCreate` and uses `dup2()` to wire stdin/stdout of child processes to the pipe endpoints. The FD table manipulation is entirely in userspace — the kernel just sees IPC channel operations.

### 5.3 FD Lifecycle

```
open("/spaces/research/paper.md", O_RDONLY)
  1. Path Resolver: /spaces/research/paper.md → space "research", object "paper"
  2. Capability check: does this process have read access to "research" space?
  3. IPC to Space Service: SpaceRequest::Read { space, object }
  4. Space Service returns: SharedMemoryId (content mapped into our address space)
  5. FD Table: allocate fd, create SpaceObject entry
  6. Return fd to caller

read(fd, buf, 4096)
  1. FD Table: lookup fd → SpaceObject { shared_mem, offset: 0 }
  2. Copy from shared_mem[offset..offset+4096] into buf
  3. Update offset += bytes_read
  4. Return bytes_read

close(fd)
  1. FD Table: lookup fd → SpaceObject
  2. IPC to Space Service: close notification (if object was opened for write, flush)
  3. Unmap shared memory
  4. FD Table: remove entry
  5. Return 0
```

For small objects (<256KB), the shared memory mapping avoids all data copies — the `read()` call memcpys directly from the Space Service's shared buffer. For large objects, the Space Service can stream content through the IPC channel in chunks.

-----

## 6. Path Resolution

The path resolver maps POSIX filesystem paths to AIOS space objects and system resources. It is the bridge between the directory tree that BSD tools expect and the semantic object graph that AIOS provides.

### 6.1 Path Mapping Table

```
POSIX Path                     AIOS Resource
──────────                     ─────────────
/spaces/<name>/                Space named <name>
/spaces/<name>/<object>        Object in space <name>
/spaces/<name>/dir/file        Object "file" with parent relation to "dir"

/home/user/                    Personal space (alias for default user space)
/home/user/<object>            Object in personal space

/tmp/                          Ephemeral space (auto-cleaned on reboot)
/tmp/<name>                    Temporary object

/dev/null                      Discard sink (always writable, always empty on read)
/dev/urandom                   Cryptographic random bytes (kernel RNG)
/dev/zero                      Infinite zero bytes
/dev/audio*                    Audio subsystem POSIX bridge
/dev/video*                    Camera subsystem POSIX bridge
/dev/input/event*              Input subsystem POSIX bridge
/dev/fb*                       Display subsystem POSIX bridge (framebuffer)
/dev/sd*, /dev/nvme*           Storage subsystem POSIX bridge (raw block)
/dev/tty, /dev/pts/*           Terminal agent channels
/dev/bluetooth*                Bluetooth subsystem POSIX bridge

/proc/self/status              Process state (pid, memory, threads)
/proc/self/cmdline             Command-line arguments
/proc/self/maps                Memory map (read-only)
/proc/self/fd/                 Open file descriptors (symlinks to resources)

/bin/, /usr/bin/               System utilities (read-only, from initramfs)
/usr/lib/                      Shared libraries (musl, LLVM runtime)
/usr/include/                  C/C++ headers (musl, LLVM)
/etc/                          System configuration objects (from system space)
```

### 6.2 Path Resolver Implementation

```rust
pub struct PathResolver {
    space_service: ChannelId,   // IPC channel to Space Service
    mount_table: Vec<MountPoint>,
}

struct MountPoint {
    path: &'static str,
    handler: MountHandler,
}

enum MountHandler {
    /// Maps to a named space
    Space(SpaceId),
    /// Maps to a virtual filesystem (procfs, devfs)
    Virtual(Box<dyn VirtualFs>),
    /// Maps to a read-only initramfs region
    Initramfs(SharedMemoryId),
}

impl PathResolver {
    fn resolve(&self, path: &str) -> Result<ResolvedPath> {
        // Canonicalize: resolve "..", ".", symlinks
        let canonical = self.canonicalize(path)?;

        // Find the longest matching mount point
        let mount = self.mount_table.iter()
            .filter(|m| canonical.starts_with(m.path))
            .max_by_key(|m| m.path.len())
            .ok_or(ENOENT)?;

        let remainder = &canonical[mount.path.len()..];

        match &mount.handler {
            MountHandler::Space(space_id) => {
                if remainder.is_empty() {
                    Ok(ResolvedPath::SpaceRoot(*space_id))
                } else {
                    // Split remainder into path components
                    // Query Space Service to find the object
                    let object = self.resolve_in_space(*space_id, remainder)?;
                    Ok(ResolvedPath::SpaceObject(*space_id, object))
                }
            }
            MountHandler::Virtual(vfs) => {
                vfs.resolve(remainder)
            }
            MountHandler::Initramfs(region) => {
                // Simple read-only lookup in the initramfs image
                Ok(ResolvedPath::Initramfs(*region, remainder.to_string()))
            }
        }
    }
}
```

### 6.3 Directory Emulation

Spaces are flat object stores — there is no inherent directory hierarchy. But POSIX tools expect directories. The translation layer synthesizes directory structure from space objects and their parent-child relations:

```rust
/// When a tool calls readdir() on /spaces/research/
fn translate_readdir(space: SpaceId) -> Vec<DirEntry> {
    // Query Space Service: list all objects in space
    let objects = ipc_call(space_service, SpaceRequest::List {
        space,
        filter: None,
    });

    // Convert each object to a directory entry
    objects.iter().map(|obj| DirEntry {
        name: obj.name.clone(),
        inode: obj.id.as_u64(),  // object ID as inode number
        file_type: match obj.content_type {
            ContentType::Document => DT_REG,
            ContentType::Code => DT_REG,
            ContentType::Image => DT_REG,
            // Objects with children appear as directories
            _ if obj.has_children => DT_DIR,
            _ => DT_REG,
        },
    }).collect()
}
```

When tools use nested paths like `/spaces/research/notes/meeting.md`, the path resolver walks the parent-child relationships: find object "notes" in space "research", then find object "meeting.md" that has a parent relation to "notes". This preserves the familiar directory tree mental model while the underlying storage remains a semantic object graph.

### 6.4 stat() Translation

```rust
fn translate_stat(path: &str) -> Result<Stat> {
    let resolved = path_resolver.resolve(path)?;

    match resolved {
        ResolvedPath::SpaceObject(space, object_id) => {
            let meta = ipc_call(space_service, SpaceRequest::Query {
                space,
                query: SpaceQuery::Metadata(object_id),
            })?;

            Ok(Stat {
                st_ino: object_id.as_u64(),
                st_mode: content_type_to_mode(meta.content_type) | perm_bits(meta.access),
                st_size: meta.size,
                st_mtime: meta.modified.as_timespec(),
                st_ctime: meta.created.as_timespec(),
                st_atime: meta.accessed.as_timespec(),
                st_nlink: 1 + meta.relations.len() as u64,
                st_uid: 1000,   // AIOS is single-user
                st_gid: 1000,
                st_blksize: 4096,
                st_blocks: (meta.size + 511) / 512,
                ..Default::default()
            })
        }
        ResolvedPath::SpaceRoot(space) => {
            // Space root appears as a directory
            Ok(Stat {
                st_mode: S_IFDIR | 0o755,
                ..Default::default()
            })
        }
        ResolvedPath::DevNode(dev) => {
            Ok(Stat {
                st_mode: S_IFCHR | dev.permissions,
                st_rdev: dev.major_minor(),
                ..Default::default()
            })
        }
        _ => { /* handle other mount types */ }
    }
}
```

-----

## 7. Process Lifecycle Translation

### 7.1 fork()

`fork()` is the hardest POSIX call to translate. It creates a copy of the calling process — address space, file descriptors, capabilities. AIOS translates this to `ProcessCreate` with copy-on-write semantics:

```rust
fn translate_fork() -> Result<pid_t> {
    // 1. Snapshot the FD table (clone all entries)
    let fd_snapshot = current_process().fd_table.snapshot();

    // 2. Clone capability set (child inherits parent capabilities)
    let cap_snapshot = current_process().capabilities.clone();

    // 3. Create new process with COW address space
    let child_pid = syscall(Syscall::ProcessCreate {
        image: ContentHash::FORK_CURRENT,  // special: COW clone of current
        capabilities: cap_snapshot.as_ptr(),
        cap_count: cap_snapshot.len(),
        args: ptr::null(),
        args_len: 0,
    })?;

    // 4. In parent: return child PID
    // 5. In child: return 0 (kernel arranges this via register state)
    //
    // The child's FD table is initialized from fd_snapshot.
    // IPC channels are duplicated (both parent and child hold endpoints).
    // Shared memory regions remain shared (COW for private mappings).

    Ok(child_pid)
}
```

**Optimization:** Most `fork()` calls are immediately followed by `exec()`. The translation layer detects the `fork()`/`exec()` pattern and uses vfork semantics internally — the child shares the parent's address space until `exec()` replaces it. This avoids the cost of COW page table duplication for the common case.

### 7.2 exec()

```rust
fn translate_execve(path: &str, argv: &[&str], envp: &[&str]) -> Result<()> {
    // 1. Resolve executable path
    let resolved = path_resolver.resolve(path)?;
    let binary_hash = match resolved {
        ResolvedPath::SpaceObject(space, obj) => {
            // Fetch content hash from Space Service
            ipc_call(space_service, SpaceRequest::ContentHash { space, object: obj })?
        }
        ResolvedPath::Initramfs(region, name) => {
            // Look up in initramfs (system utilities)
            initramfs_lookup(region, &name)?
        }
        _ => return Err(EACCES),
    };

    // 2. Determine capabilities for new process
    //    exec'd process inherits caller's capabilities minus execve-clear ones
    let caps = current_process().capabilities
        .filter(|c| !c.clear_on_exec);

    // 3. Replace current process image
    syscall(Syscall::ProcessCreate {
        image: binary_hash,
        capabilities: caps.as_ptr(),
        cap_count: caps.len(),
        args: serialize_argv_envp(argv, envp),
        args_len: serialized_len,
    })?;

    // Never returns (process image replaced)
    unreachable!()
}
```

### 7.3 pipe()

```rust
fn translate_pipe2(flags: i32) -> Result<(i32, i32)> {
    // Create an anonymous IPC channel
    let channel_pair = syscall(Syscall::ChannelCreate {
        flags: ChannelFlags {
            max_message: 64 * 1024,  // 64KB pipe buffer
            queue_depth: 16,
            audit: false,            // pipes are not audited by default
        },
    })?;

    // Insert both ends into the FD table
    let read_fd = fd_table.insert(FdEntry {
        kind: FdKind::Pipe {
            channel: channel_pair.read_end,
            direction: PipeDirection::ReadEnd,
        },
        flags: FdFlags::from_pipe_flags(flags),
        offset: 0,
        status: FdStatus::Open,
    });

    let write_fd = fd_table.insert(FdEntry {
        kind: FdKind::Pipe {
            channel: channel_pair.write_end,
            direction: PipeDirection::WriteEnd,
        },
        flags: FdFlags::from_pipe_flags(flags),
        offset: 0,
        status: FdStatus::Open,
    });

    Ok((read_fd, write_fd))
}
```

When a shell runs `ls /spaces/research | grep paper`, it:
1. Creates a pipe (two IPC channel endpoints)
2. Forks twice (two `ProcessCreate` calls with COW)
3. Wires the pipe into stdin/stdout via `dup2()` (FD table manipulation)
4. Execs `ls` and `grep` (two `ProcessCreate` with new images)
5. Data flows through the IPC channel — `ls` writes directory entries, `grep` reads and filters them

The entire pipeline works through AIOS IPC. No kernel pipe buffer, no special kernel pipe implementation. Just IPC channels.

### 7.4 Signal Translation

POSIX signals are translated to AIOS notification messages:

```rust
fn translate_kill(pid: pid_t, sig: i32) -> Result<()> {
    let notification = match sig {
        SIGTERM => ProcessNotification::TerminateGraceful,
        SIGKILL => ProcessNotification::TerminateForced,
        SIGINT  => ProcessNotification::Interrupt,
        SIGSTOP => ProcessNotification::Suspend,
        SIGCONT => ProcessNotification::Resume,
        SIGCHLD => ProcessNotification::ChildStateChanged,
        SIGUSR1 | SIGUSR2 => ProcessNotification::UserDefined(sig),
        _ => ProcessNotification::Generic(sig),
    };

    // Send via IPC notification channel to target process
    ipc_send(process_notification_channel(pid), notification)?;
    Ok(())
}
```

Signal handlers registered via `sigaction()` are callbacks invoked when the translation layer receives a `ProcessNotification` on the process's notification channel. The kernel has no concept of signals — it just delivers IPC messages.

-----

## 8. Socket Translation

Network operations are the second most complex translation after process management. POSIX socket calls are translated to IPC messages to the Network Service.

```rust
fn translate_socket(domain: i32, sock_type: i32, protocol: i32) -> Result<i32> {
    // 1. Validate: only AF_INET, AF_INET6, AF_UNIX supported
    let aios_domain = match domain {
        AF_INET  => SocketDomain::IPv4,
        AF_INET6 => SocketDomain::IPv6,
        AF_UNIX  => SocketDomain::Local,
        _ => return Err(EAFNOSUPPORT),
    };

    // 2. Capability check: does this process have network access?
    capability_check(current_process(), Capability::Network)?;

    // 3. IPC to Network Service: create a connection object
    let channel = ipc_call(network_service, NetworkRequest::SocketCreate {
        domain: aios_domain,
        sock_type: match sock_type & SOCK_TYPE_MASK {
            SOCK_STREAM => SocketType::Stream,
            SOCK_DGRAM  => SocketType::Datagram,
            _ => return Err(EPROTONOSUPPORT),
        },
    })?;

    // 4. Insert into FD table
    let fd = fd_table.insert(FdEntry {
        kind: FdKind::Socket {
            channel,
            domain: aios_domain,
            sock_type: sock_type.into(),
            state: SocketState::Unbound,
        },
        flags: FdFlags::from_socket_flags(sock_type),
        offset: 0,
        status: FdStatus::Open,
    });

    Ok(fd)
}

fn translate_connect(fd: i32, addr: &sockaddr, addrlen: u32) -> Result<()> {
    let entry = fd_table.get_mut(fd)?;
    let channel = match &mut entry.kind {
        FdKind::Socket { channel, state, .. } => {
            *state = SocketState::Connecting;
            *channel
        }
        _ => return Err(ENOTSOCK),
    };

    // IPC to Network Service: initiate connection
    ipc_call(channel, NetworkRequest::Connect {
        address: parse_sockaddr(addr, addrlen)?,
    })?;

    // Update state
    if let FdKind::Socket { state, .. } = &mut fd_table.get_mut(fd)?.kind {
        *state = SocketState::Connected;
    }

    Ok(())
}
```

Once connected, `send()`/`recv()` on the socket fd become IPC messages through the Network Service channel. The Network Service handles TLS, connection pooling, and all transport details. The BSD tool sees a simple byte stream.

-----

## 9. Device Access Translation

When BSD tools access `/dev/*` nodes, the POSIX layer routes operations to the appropriate subsystem's POSIX bridge (defined in the Subsystem Framework):

```rust
fn translate_dev_open(path: &str, flags: i32) -> Result<i32> {
    // 1. Match path to subsystem
    let (subsystem_id, dev_node) = match path {
        p if p.starts_with("/dev/audio")     => ("audio", parse_audio_node(p)?),
        p if p.starts_with("/dev/video")     => ("camera", parse_video_node(p)?),
        p if p.starts_with("/dev/input/")    => ("input", parse_input_node(p)?),
        p if p.starts_with("/dev/fb")        => ("display", parse_fb_node(p)?),
        p if p.starts_with("/dev/sd")        => ("storage", parse_storage_node(p)?),
        p if p.starts_with("/dev/bluetooth") => ("bluetooth", parse_bt_node(p)?),
        // Special devices handled inline
        "/dev/null"    => return open_dev_null(flags),
        "/dev/zero"    => return open_dev_zero(flags),
        "/dev/urandom" => return open_dev_urandom(flags),
        _ => return Err(ENODEV),
    };

    // 2. IPC to subsystem's POSIX bridge: open a session
    let bridge_channel = subsystem_channel(subsystem_id)?;
    let session = ipc_call(bridge_channel, PosixBridgeRequest::Open {
        node: dev_node,
        flags: OpenFlags::from_posix(flags),
        agent: current_agent(),
    })?;

    // 3. Insert into FD table
    let fd = fd_table.insert(FdEntry {
        kind: FdKind::Device {
            subsystem: subsystem_id.into(),
            session: session.id,
            channel: session.channel,
        },
        flags: FdFlags::from_posix(flags),
        offset: 0,
        status: FdStatus::Open,
    });

    Ok(fd)
}
```

Subsequent `read()`, `write()`, `ioctl()`, and `close()` calls on device fds are routed through the subsystem's POSIX bridge. The bridge translates them into the subsystem's native session operations. For example, `ioctl(audio_fd, SNDCTL_DSP_SPEED, &rate)` becomes a format negotiation call on the audio session.

-----

## 10. POSIX-to-Spaces Path Semantics

This section covers the semantic differences between POSIX filesystem operations and AIOS space operations, and how the translation layer bridges them.

### 10.1 Create vs. Write

In POSIX, creating a file and writing to it are separate operations. In AIOS, creating a space object provides it with typed content:

```
POSIX:   open("file.md", O_CREAT|O_WRONLY) → write(fd, data) → close(fd)
AIOS:    SpaceRequest::Create { content_type: Document, content: data }
```

The translation layer buffers writes and commits them to the Space Service on `close()` (or `fsync()`). This means a POSIX program writing a file byte-by-byte doesn't generate thousands of IPC calls — it writes to a shared memory buffer and the Space Service sees a single create/update operation.

### 10.2 Permissions

POSIX has user/group/other permission bits (rwxrwxrwx). AIOS has capabilities. The translation:

```
POSIX              AIOS
─────              ────
r (read)           Read capability on the space
w (write)          Write capability on the space
x (execute)        Execute capability on the binary's content hash
uid/gid            Agent identity (there is one user, many agents)
chmod              Not meaningful — capabilities are per-agent, not per-object
chown              Not meaningful — single-user system
```

`stat()` returns synthetic permission bits based on the calling agent's capabilities for that space. An agent with read-only access sees `r--r--r--`. An agent with read-write sees `rw-rw-rw-`. This gives BSD tools correct behavior for permission checks without implementing the Unix permission model.

### 10.3 Hard Links and Symbolic Links

AIOS spaces have relations, not links. The translation:

```
POSIX              AIOS
─────              ────
symlink            Relation { kind: References, ... }
hard link          Not supported (return ENOTSUP)
readlink           Query relation target
```

Symlinks within `/spaces/` resolve to space object relations. Symlinks to `/dev/` or `/proc/` resolve to the virtual filesystem handlers. Hard links are not supported because space objects are content-addressed — the concept of multiple directory entries pointing to the same inode doesn't map to the AIOS model.

-----

## 11. The Included Toolset

### 11.1 Core Utilities (FreeBSD)

```
File operations:  ls  cp  mv  rm  mkdir  rmdir  ln  chmod  stat  touch  du  df
Text processing:  cat  head  tail  wc  sort  uniq  cut  paste  tr  tee  xargs
Search:           grep  find  which  whereis
Pattern/editing:  sed  awk  diff  patch  ed
Compression:      tar  gzip  bzip2  xz
Other:            date  env  expr  test  true  false  yes  printf  sleep
                  id  whoami  hostname  uname  kill  ps  nice
```

### 11.2 Development Tools

```
Compiler:    clang (C, C++, Objective-C)
Linker:      lld (LLVM linker)
Build:       BSD make
Archiver:    llvm-ar
Symbols:     llvm-nm, llvm-strip, llvm-objdump
Runtime:     compiler-rt (builtins, sanitizers)
Headers:     musl libc headers, LLVM headers
```

This toolchain makes AIOS **self-hosting**: it can compile C and C++ programs for itself. An AIOS system can build musl, build FreeBSD tools, and build clang — bootstrapping its own development environment.

### 11.3 Network Tools

```
HTTP:   curl (transfers, API calls)
SSH:    OpenSSH client and server (ssh, scp, sftp, ssh-keygen)
DNS:    host, dig (DNS lookups)
```

### 11.4 Shell and Editor

```
Shell:   FreeBSD /bin/sh (POSIX-compliant, ash-based)
Editor:  nvi (BSD vi — the original vi implementation, BSD-licensed)
Pager:   less
```

-----

## 12. Capability Mapping for BSD Processes

BSD tools run as AIOS processes with capabilities. The POSIX translation layer checks capabilities before translating operations:

```rust
/// Capabilities granted to a BSD process at spawn time
pub struct BsdProcessCapabilities {
    /// Which spaces this process can read from
    pub space_read: Vec<SpaceId>,

    /// Which spaces this process can write to
    pub space_write: Vec<SpaceId>,

    /// Network access (if any)
    pub network: Option<NetworkCapability>,

    /// Device access
    pub devices: Vec<DeviceCapability>,

    /// Process management (fork, exec, signal)
    pub process: ProcessCapability,
}
```

When a user types `ls /spaces/research/` in the terminal, the shell's process has a read capability for the "research" space (inherited from the terminal agent's capability set). When the shell forks and execs `ls`, the child inherits those capabilities. `ls` calls `opendir()` → `readdir()`, the translation layer checks the read capability, and the Space Service returns the listing.

If a BSD tool tries to access a space it doesn't have capabilities for, the `open()` call returns `EACCES` — the standard POSIX permission denied error. The tool doesn't know about capabilities; it just sees the error it would expect from a permission failure on Unix.

-----

## 13. Performance

### 13.1 Overhead Analysis

Every POSIX call adds translation overhead compared to native AIOS IPC. The question is whether the overhead is acceptable:

```
Operation                    Native AIOS         POSIX Translation      Overhead
─────────                    ───────────         ─────────────────      ────────
Read space object            IPC call (~5 μs)    open + read + close    ~15 μs (3 IPC calls)
                                                 (but buffered: amortized to ~6 μs for
                                                  sequential reads via shared memory)

Create process               ProcessCreate       fork + exec            ~20 μs extra
                             (~50 μs)            (vfork optimization    (COW page tables)
                                                  avoids most overhead)

Pipe data                    ChannelCreate +     pipe + fork + dup2     ~5 μs extra
                             IpcSend (~8 μs)     + write/read           (FD table ops)

Network connect              IPC to Network      socket + connect       ~10 μs extra
                             Service (~10 μs)    + send/recv            (FD table + socket state)

stat() metadata              IPC to Space        stat()                 ~2 μs extra
                             Service (~5 μs)                            (path resolution)
```

### 13.2 Optimization Strategies

**Shared memory buffering:** The translation layer maps space object content into shared memory on `open()`. Subsequent `read()` calls are memory copies from the shared region — no IPC per read. For a tool that reads a file sequentially (cat, grep, sed), the cost is one IPC at open time plus local memory copies.

**FD table in userspace:** The entire FD table is process-local memory. `dup2()`, FD flag manipulation, and FD lookup are pure userspace operations with no kernel involvement.

**Path resolution cache:** Frequently accessed paths (`/dev/null`, `/bin/sh`, `/tmp/`) are cached. Space object lookups are cached per-process with invalidation via notification channels from the Space Service.

**vfork for fork+exec:** When `fork()` is immediately followed by `exec()`, the translation layer uses shared address space (vfork semantics) to avoid COW page table duplication. This covers 95%+ of fork usage in shell pipelines and `system()` calls.

**Batch readdir:** When `opendir()` is called, the translation layer fetches the complete directory listing from the Space Service in one IPC call and caches it. Subsequent `readdir()` calls iterate the cache with no IPC.

-----

## 14. Limitations and Non-Goals

### 14.1 Not Supported

```
POSIX feature              Why not                              Error returned
─────────────              ───────                              ──────────────
hard links                 Content-addressed storage            ENOTSUP
mknod                      No raw device creation by tools      EPERM
chown/chmod                Capability-based, not permission-based  ENOSYS (silently succeeds)
setuid/setgid              No privilege escalation model         ENOSYS
ptrace                     Security risk, not needed for tools   EPERM
System V IPC (shmget, etc) Use AIOS IPC channels instead        ENOSYS
POSIX semaphores           Use AIOS IPC synchronization          ENOSYS
inotify/fanotify           Use Space Service notifications      ENOSYS
epoll                      Use IpcSelect (translated from poll)  ENOSYS (poll works)
```

### 14.2 Intentional Divergences

- **Single user.** There is no multi-user model. `uid` is always 1000. `su` and `sudo` don't exist. Privilege is expressed through capabilities, not user switching.
- **No `/etc/passwd`, `/etc/group`.** Agent identities replace Unix user/group identities.
- **No runlevels, no init scripts.** AIOS boot is the kernel's responsibility, not userland's.
- **No package manager.** Software installation goes through the Agent Store. BSD tools are shipped in the initramfs. Development libraries are provided as space objects.

-----

## 15. Linux Binary Compatibility (Phase 25)

Phase 15 delivers BSD tool compatibility via the POSIX translation layer. Phase 25 extends this to full Linux ELF binary compatibility:

```
Phase 15 (POSIX/BSD):
  BSD tools → musl → POSIX Translation Layer → AIOS syscalls
  Scope: ~60 translated syscalls, BSD userland works

Phase 25 (Linux compat):
  Linux ELF → glibc shim or musl → Linux Syscall Translation → AIOS syscalls
  Scope: ~200 translated syscalls, Linux GUI apps work (Wayland)

  Additional for Phase 25:
  - ELF loader for Linux binaries (different ABI than AIOS native)
  - glibc compatibility shim (translate glibc-specific calls to musl)
  - Linux-specific syscalls: epoll → IpcSelect, futex → AIOS sync,
    io_uring → batched IPC, eventfd → notification channel
  - /proc and /sys emulation beyond /proc/self
  - Wayland protocol translation (Linux Wayland clients → AIOS compositor)
```

Linux binary compatibility is a separate effort built on top of the POSIX layer. The POSIX layer provides the foundation; the Linux layer adds the Linux-specific syscalls and ABI translation.

-----

## 16. Implementation Order

```
Phase 15a: musl libc port — redirect syscall dispatch to POSIX translation
           Depends on: Phase 3 (IPC), Phase 4 (Space Service)
           Deliverable: "Hello World" C program compiles and runs on AIOS

Phase 15b: Path resolver and FD table — /spaces/* mapping, file operations
           Depends on: Phase 15a, Phase 4 (Space Service operational)
           Deliverable: cat, ls, cp, mv work on space objects

Phase 15c: Process lifecycle — fork, exec, pipe, signal translation
           Depends on: Phase 15b
           Deliverable: shell pipelines work (ls | grep | sort)

Phase 15d: Device translation — /dev/* nodes routed to subsystem bridges
           Depends on: Phase 15c, subsystem framework (Phase 16+)
           Deliverable: BSD tools can access /dev/null, /dev/urandom, /dev/tty

Phase 15e: Socket translation — network operations via Network Service
           Depends on: Phase 15c, Phase 5 (Network Service)
           Deliverable: curl and ssh work

Phase 15f: Full FreeBSD userland — all included tools compiled and tested
           Depends on: Phase 15e
           Deliverable: complete BSD environment, self-hosting (clang builds on AIOS)

Phase 25:  Linux binary compatibility (separate phase)
           Depends on: Phase 15f, Phase 20 (compositor/Wayland)
           Deliverable: Linux ELF binaries run, Wayland apps display through compositor
```

-----

## 17. Design Principles

1. **POSIX is a library, not a kernel feature.** The translation layer is userspace code. The kernel knows only AIOS syscalls. This keeps the kernel clean and the translation layer replaceable.
2. **BSD userland, not GNU.** Permissive licensing, smaller codebase, proven portability. No GPL anywhere in the core OS.
3. **musl, not glibc.** MIT-licensed, 15x smaller, designed for portability and static linking. The right libc for a non-Linux OS.
4. **Translate the subset that matters.** ~60 POSIX syscalls cover everything BSD tools need. Don't implement the other 390 Linux syscalls until Phase 25 requires them.
5. **Capability-check at the translation boundary.** Every POSIX operation is checked against AIOS capabilities before being dispatched. BSD tools inherit the security model transparently.
6. **Zero-copy where possible.** Shared memory for file content. FD table in userspace. Cached path resolution. The translation layer adds microseconds, not milliseconds.
7. **Self-hosting is a milestone.** When AIOS can compile clang with clang on AIOS, the POSIX layer is complete. This is the acceptance test.
8. **The goal is a bridge, not a destination.** POSIX compatibility lets developers be productive today. Native AIOS agents are the future. The translation layer is a migration path, not an end state.
