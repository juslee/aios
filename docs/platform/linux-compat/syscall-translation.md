# AIOS Linux Syscall Translation

Part of: [linux-compat.md](../linux-compat.md) — Linux Binary & Wayland Compatibility
**Related:** [posix.md](../posix.md) §4 — musl syscall dispatch, [elf-loader.md](./elf-loader.md) — ELF loading and glibc shim

-----

## §5 Linux Syscall Translation Table

### §5.1 Translation Architecture

Linux binaries reach the AIOS compatibility layer through two interception paths (see [linux-compat.md](../linux-compat.md) §2.3 for the full picture):

**Library path (musl-linked binaries):** The musl `__syscall_dispatch()` function (see [posix.md](../posix.md) §4.1) is extended from ~60 POSIX entries to ~200 Linux-specific entries. Syscalls that map directly to POSIX operations (file I/O, process lifecycle, sockets) delegate to the existing POSIX translation layer. Linux-specific syscalls (epoll, futex, io_uring, signalfd, timerfd) are handled by dedicated translation functions within the Linux compatibility module.

**Trap path (pre-compiled Linux binaries):** Unmodified Linux aarch64 binaries issue `SVC #0` from EL0 with the Linux syscall convention: syscall number in `x8`, arguments in `x0`–`x5`, return value in `x0`. The kernel's EL0 sync exception handler (see [ipc.md](../../kernel/ipc.md) §3) detects these as Linux-ABI syscalls and performs an upcall to the userspace `linux_syscall_dispatch()` function. The upcall delivers the syscall number and arguments to the compatibility layer, which translates them and returns the result back through the kernel to the application.

The dispatch entry point:

```rust
/// Linux syscall dispatch — extends musl's __syscall_dispatch for Linux ABI.
/// Called from both library path (direct) and trap path (via kernel upcall).
/// Returns a Linux-compatible return value (negative errno on error).
pub fn linux_syscall_dispatch(num: u32, args: &[u64; 6]) -> i64 {
    match num {
        // File I/O — delegate to POSIX translation layer
        56  => posix::translate_openat(args),
        57  => posix::translate_close(args),
        63  => posix::translate_read(args),
        64  => posix::translate_write(args),
        // ...

        // Linux-specific — handled by compat layer directly
        20  => linux::translate_epoll_create1(args),
        21  => linux::translate_epoll_ctl(args),
        22  => linux::translate_epoll_pwait(args),
        98  => linux::translate_futex(args),
        425 => linux::translate_io_uring_setup(args),
        // ...

        _   => {
            audit_log(AuditEvent::UnsupportedSyscall { num });
            -ENOSYS
        }
    }
}
```

**Performance budget:**

| Path | Target Latency | Overhead Source |
|---|---|---|
| Library path | < 50ns | Direct function call, argument remapping |
| Trap path | < 200ns | SVC trap → kernel upcall → dispatch → return |
| POSIX delegation | +10–30ns | Additional indirection through POSIX layer |

**Error code translation:** Linux uses negative errno values (`-ENOENT`, `-EINVAL`, etc.) returned directly in `x0`. AIOS uses `Result<T, AiosError>` internally. The translation layer converts between the two representations at the boundary:

```rust
/// Map AIOS errors to Linux errno values
fn aios_to_linux_errno(err: AiosError) -> i64 {
    match err {
        AiosError::NotFound       => -2,   // ENOENT
        AiosError::PermissionDenied => -13, // EACCES
        AiosError::InvalidArgument => -22,  // EINVAL
        AiosError::WouldBlock     => -11,   // EAGAIN
        AiosError::TimedOut       => -110,  // ETIMEDOUT
        AiosError::NoCapability   => -1,    // EPERM
        AiosError::Unsupported    => -38,   // ENOSYS
        // ... exhaustive mapping for ~50 error codes
    }
}
```

**Thread-safety:** Each Linux process maintains per-thread translation state:

```rust
/// Per-thread state for Linux syscall translation
pub struct LinuxThreadState {
    fd_table: Arc<Mutex<FdTable>>,        // shared with POSIX layer
    signal_mask: u64,                      // rt_sigprocmask state
    signal_stack: Option<SignalStack>,     // sigaltstack state
    robust_list: Option<VirtAddr>,         // set_robust_list pointer
    clear_child_tid: Option<VirtAddr>,     // set_tid_address pointer
    epoll_contexts: Vec<EpollContext>,     // active epoll instances
    futex_state: FutexThreadState,         // per-thread futex bookkeeping
}
```

-----

### §5.2 Syscall Categories and Translation Table

The following tables list every Linux syscall supported by the compatibility layer, organized by functional category. All syscall numbers are **aarch64 Linux** numbers (which differ significantly from x86_64). Syscalls that map to existing POSIX translations reference [posix.md](../posix.md); Linux-specific syscalls are translated directly to AIOS primitives.

#### §5.2.1 File I/O

These syscalls route through the POSIX translation layer's FD table and path resolver. Cross-ref: [posix.md](../posix.md) §5 (FD table), §6 (path resolver).

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 56 | `openat` | POSIX `translate_openat()` → Space Service IPC | AT_FDCWD support, O_CLOEXEC default |
| 63 | `read` | POSIX `translate_read()` → channel recv | FD → IPC channel lookup |
| 64 | `write` | POSIX `translate_write()` → channel send | FD → IPC channel lookup |
| 57 | `close` | POSIX `translate_close()` → FD table remove | Releases IPC channel |
| 80 | `fstat` | POSIX `translate_fstat()` → Space Service query | Populates `struct stat` from object metadata |
| 62 | `lseek` | POSIX `translate_lseek()` → FD offset update | SEEK_SET / SEEK_CUR / SEEK_END |
| 67 | `pread64` | POSIX `translate_pread()` → positioned read | Offset-based, does not update FD position |
| 68 | `pwrite64` | POSIX `translate_pwrite()` → positioned write | Offset-based, does not update FD position |
| 65 | `readv` | Scatter read → multiple `translate_read()` | Vectored I/O into iovec array |
| 66 | `writev` | Gather write → multiple `translate_write()` | Vectored I/O from iovec array |
| 47 | `fallocate` | Space Service `allocate_extent()` IPC | Pre-allocates storage blocks |
| 46 | `ftruncate` | Space Service `truncate()` IPC | Resize object content |
| 61 | `getdents64` | POSIX `translate_getdents()` → Space query | Directory iteration as `linux_dirent64` |
| 48 | `faccessat` | POSIX `translate_faccessat()` → capability check | R_OK / W_OK / X_OK mapped to AIOS capabilities |
| 79 | `fstatat` | POSIX `translate_fstatat()` → Space Service query | `stat` at path relative to dirfd |
| 78 | `readlinkat` | Space Service `read_symlink()` IPC | Symbolic link resolution |
| 276 | `renameat2` | Space Service `rename()` IPC | RENAME_NOREPLACE, RENAME_EXCHANGE flags |
| 35 | `unlinkat` | Space Service `delete()` IPC | AT_REMOVEDIR for directory removal |
| 34 | `mkdirat` | Space Service `create_directory()` IPC | Creates Space container object |
| 37 | `linkat` | Space Service `create_link()` IPC | Hard links map to object references |
| 36 | `symlinkat` | Space Service `create_symlink()` IPC | Symbolic links as Space objects with link content |
| 53 | `fchmodat` | Capability attenuation on object | Permission bits → AIOS capability flags |
| 54 | `fchownat` | Audit-only (AIOS uses capabilities, not UIDs) | Returns 0, logs ownership change request |
| 88 | `utimensat` | Space Service `update_timestamps()` IPC | Modifies object atime/mtime |
| 291 | `statx` | Space Service extended query | Extended attributes, birth time, mount ID |

#### §5.2.2 Process & Thread

Cross-ref: [posix.md](../posix.md) §7 (process/thread translation).

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 220 | `clone` | AIOS `ProcessCreate` / `ThreadCreate` | Flags determine process vs thread semantics |
| 435 | `clone3` | AIOS `ProcessCreate` / `ThreadCreate` | Extended struct-based clone with `set_tid` |
| 221 | `execve` | ELF loader + address space replace | Loads new ELF, resets capabilities to manifest |
| 281 | `execveat` | ELF loader (fd-relative path) | Like execve but relative to dirfd |
| 93 | `exit` | AIOS thread termination | Single thread exit, process continues |
| 94 | `exit_group` | AIOS `ProcessExit` | Terminates all threads in process |
| 260 | `wait4` | AIOS `ProcessWait` + rusage collection | Waits for child state change, collects stats |
| 95 | `waitid` | AIOS `ProcessWait` | P_PID / P_PGID / P_ALL selectors |
| 96 | `set_tid_address` | Store pointer in `LinuxThreadState` | Kernel clears `*tidptr` and futex-wakes on exit |
| 178 | `gettid` | AIOS `ThreadId` → Linux TID mapping | Thread-local TID from translation state |
| 172 | `getpid` | AIOS `ProcessId` → Linux PID mapping | PID namespace translation |
| 173 | `getppid` | Parent `ProcessId` → Linux PID | Returns parent PID from process table |
| 157 | `setsid` | Process group leader detach | Creates new session, returns SID |
| 154 | `setpgid` | Process group assignment | Maps to AIOS process group tracking |
| 155 | `getpgid` | Process group query | Returns PGID for given PID |
| 167 | `prctl` | Per-operation dispatch | PR_SET_NAME → thread name, PR_SET_SECCOMP → sandbox |
| 261 | `prlimit64` | AIOS `KernelResourceLimits` query/set | Maps rlimit kinds to AIOS resource controls |
| 124 | `sched_yield` | AIOS `schedule()` voluntary yield | Moves thread to back of run queue |
| 122 | `sched_setaffinity` | AIOS `CpuSet` modification | Restricts thread to specified cores |
| 123 | `sched_getaffinity` | AIOS `CpuSet` query | Returns current affinity mask |

**fork semantics:** Linux `fork()` is implemented via `clone(SIGCHLD, 0)`. The translation layer invokes AIOS `ProcessCreate` to create a new process with a copy of the parent's address space (copy-on-write pages), FD table (cloned), and capability set (attenuated to child-safe subset).

#### §5.2.3 Memory Management

Cross-ref: [memory/virtual.md](../../kernel/memory/virtual.md) §5 (per-agent address spaces).

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 222 | `mmap` | AIOS `MemMap` syscall | Anonymous or file-backed mapping with W^X enforcement |
| 215 | `munmap` | AIOS `MemUnmap` syscall | Releases virtual address range |
| 226 | `mprotect` | AIOS `MemProtect` syscall | W^X enforced: cannot set RWX simultaneously |
| 216 | `mremap` | Unmap + re-map at new size | MREMAP_MAYMOVE supported, MREMAP_FIXED optional |
| 233 | `madvise` | Hint to AIOS memory subsystem | MADV_DONTNEED → page release, MADV_WILLNEED → prefetch |
| 214 | `brk` | Heap expansion via `MemMap` | Grows/shrinks program break (data segment end) |
| 227 | `msync` | Flush dirty pages to Space storage | MS_SYNC blocks until write complete |
| 228 | `mlock` | Pin pages (prevent swap/reclaim) | Requires `MemoryPin` capability |
| 229 | `munlock` | Unpin pages | Releases pinning constraint |
| 232 | `mincore` | Query page residency | Returns per-page resident/non-resident bitmap |
| 279 | `memfd_create` | AIOS `SharedMemoryCreate` | Anonymous shared memory; see §6.5 |
| 270 | `process_vm_readv` | Cross-process read via shared memory | Requires `ProcessInspect` capability |
| 271 | `process_vm_writev` | Cross-process write via shared memory | Requires `ProcessInspect` capability |

**mmap translation detail:** The `mmap` flags are translated as follows:

| Linux Flag | AIOS Equivalent | Behavior |
|---|---|---|
| `MAP_ANONYMOUS` | Anonymous `MemMap` | Zero-filled pages from user pool |
| `MAP_PRIVATE` | Copy-on-write mapping | Private copy of file/anon pages |
| `MAP_SHARED` | `SharedMemoryCreate` + `SharedMemoryMap` | Shared region visible to other processes |
| `MAP_FIXED` | Forced address placement | Unmaps existing mappings in range |
| `MAP_NORESERVE` | Lazy commit | Pages allocated on first access |
| `PROT_READ` | `VmFlags::READ` | Readable pages |
| `PROT_WRITE` | `VmFlags::WRITE` | Writable pages (cannot combine with EXEC) |
| `PROT_EXEC` | `VmFlags::EXECUTE` | Executable pages (cannot combine with WRITE) |

#### §5.2.4 Signal

Signal delivery translates Linux signal numbers and semantics into AIOS notification objects. Cross-ref: [elf-loader.md](./elf-loader.md) §4.3 (signal handling translation).

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 134 | `rt_sigaction` | Register notification handler | Maps signal number → notification callback |
| 135 | `rt_sigprocmask` | Update per-thread signal mask | SIG_BLOCK / SIG_UNBLOCK / SIG_SETMASK |
| 139 | `rt_sigreturn` | Restore context from signal frame | Pops saved registers from signal stack |
| 129 | `kill` | AIOS notification signal to process | Signal → notification with signal-number payload |
| 131 | `tgkill` | AIOS notification signal to thread | Thread-directed signal delivery |
| 130 | `tkill` | AIOS notification signal to thread | Deprecated, maps to tgkill internally |
| 132 | `sigaltstack` | Configure alternate signal stack | Stored in `LinuxThreadState.signal_stack` |
| 74 | `signalfd4` | Signal-to-FD bridge | See §6.4 |
| 136 | `rt_sigpending` | Query pending signals | Returns signals blocked but pending delivery |
| 133 | `rt_sigsuspend` | Atomically set mask + wait | Blocks until signal received, then restores mask |

**Signal number mapping:** Linux aarch64 uses standard signal numbers (SIGHUP=1, SIGINT=2, ..., SIGRTMAX=64). The compatibility layer maintains a bidirectional map between Linux signal numbers and AIOS notification IDs. Real-time signals (33–64) map to a pool of reserved AIOS notifications per process.

#### §5.2.5 IPC & Synchronization

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 59 | `pipe2` | AIOS `ChannelCreate` | Returns two FDs for read/write ends of IPC channel |
| 199 | `socketpair` | Two `ChannelCreate` + cross-link | AF_UNIX SOCK_STREAM pair via IPC channels |
| 19 | `eventfd2` | Notification channel with counter | See §6.4 |
| 85 | `timerfd_create` | Timer + notification FD | See §6.4 |
| 86 | `timerfd_settime` | Arm/disarm timer | Periodic or one-shot timer via AIOS timer subsystem |
| 87 | `timerfd_gettime` | Query remaining time | Returns time until next expiration |
| 98 | `futex` | Wait queue keyed by address | See §6.2 for full deep dive |
| 240 | `set_robust_list` | Store robust list head pointer | Kernel walks list on thread exit |
| 241 | `get_robust_list` | Return stored robust list pointer | Used by pthread cleanup |

#### §5.2.6 Networking

Cross-ref: [posix.md](../posix.md) §8 (socket translation), [networking.md](../networking.md) §4 (smoltcp integration).

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 198 | `socket` | POSIX `translate_socket()` → Network Service IPC | AF_INET / AF_INET6 / AF_UNIX |
| 203 | `connect` | POSIX `translate_connect()` → connection manager | TCP connect or UDP peer assignment |
| 200 | `bind` | POSIX `translate_bind()` → port assignment | Requires `NetworkBind` capability |
| 201 | `listen` | POSIX `translate_listen()` → accept queue setup | Backlog parameter maps to channel queue depth |
| 242 | `accept4` | POSIX `translate_accept()` → new channel | SOCK_CLOEXEC / SOCK_NONBLOCK flags |
| 206 | `sendto` | POSIX `translate_sendto()` → channel send | Destination address for UDP |
| 207 | `recvfrom` | POSIX `translate_recvfrom()` → channel recv | Source address populated for UDP |
| 211 | `sendmsg` | Vectored send with ancillary data | SCM_RIGHTS → capability transfer |
| 212 | `recvmsg` | Vectored recv with ancillary data | SCM_RIGHTS → capability receive |
| 209 | `getsockopt` | Network Service query | SO_ERROR, TCP_NODELAY, etc. |
| 208 | `setsockopt` | Network Service configure | Maps socket options to AIOS channel/network config |
| 210 | `shutdown` | Half-close IPC channel | SHUT_RD / SHUT_WR / SHUT_RDWR |
| 205 | `getpeername` | Network Service peer query | Returns connected peer address |
| 204 | `getsockname` | Network Service local query | Returns local bound address |
| 269 | `sendmmsg` | Batched `sendmsg` | Multiple messages in single syscall |
| 243 | `recvmmsg` | Batched `recvmsg` | Multiple messages with timeout |

**AF_UNIX translation:** Unix domain sockets are translated to AIOS IPC channels. `connect()` to a path creates a channel to the service listening at that path (resolved via the Space path resolver). `SCM_RIGHTS` (file descriptor passing) is translated to AIOS capability transfer across the channel.

#### §5.2.7 I/O Multiplexing

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 20 | `epoll_create1` | Allocate `EpollContext` | Returns FD for epoll instance; see §6.1 |
| 21 | `epoll_ctl` | Modify `EpollContext` entries | ADD / MOD / DEL operations |
| 22 | `epoll_pwait` | `ipc_select()` with event conversion | Blocks until events ready or timeout; aarch64 has no separate `epoll_wait` — glibc's `epoll_wait()` calls `epoll_pwait` with NULL sigmask |
| 73 | `ppoll` | `ipc_select()` on FD set | Timespec-based timeout, per-FD event mask |
| 72 | `pselect6` | `ipc_select()` on FD bitmasks | Legacy select interface, 3 bitmasks (read/write/except) |

All I/O multiplexing syscalls are ultimately translated to AIOS `IpcSelect` calls. The translation layer maintains a mapping from Linux FDs to `SelectEntry` structures (channels and notifications). See §6.1 for the detailed epoll translation design.

#### §5.2.8 Timer & Clock

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 113 | `clock_gettime` | AIOS timer subsystem query | CLOCK_REALTIME, CLOCK_MONOTONIC, CLOCK_BOOTTIME |
| 114 | `clock_getres` | Timer resolution query | Returns 1ns nominal (actual: timer tick granularity) |
| 115 | `clock_nanosleep` | AIOS `TimeNanosleep` syscall | Absolute or relative sleep with clock selection |
| 101 | `nanosleep` | AIOS `TimeNanosleep` syscall | CLOCK_MONOTONIC relative sleep |
| 102 | `getitimer` | Query interval timer state | ITIMER_REAL / ITIMER_VIRTUAL / ITIMER_PROF |
| 103 | `setitimer` | Configure interval timer | Signal delivery on expiration via notification |
| 107 | `timer_create` | AIOS timer + notification | Per-process POSIX timer with signal or thread delivery |
| 110 | `timer_settime` | Arm/disarm POSIX timer | One-shot or periodic, absolute or relative |

**VDSO fast path:** `clock_gettime(CLOCK_MONOTONIC)` and `clock_gettime(CLOCK_REALTIME)` are served via the injected VDSO page (see [elf-loader.md](./elf-loader.md) §3.3) — no syscall trap required. The VDSO reads the kernel's shared time page directly, achieving < 10ns latency.

#### §5.2.9 File System Metadata

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 43 | `statfs` | Space Service quota query | Populates `struct statfs` from Space metadata |
| 44 | `fstatfs` | Space Service quota query (by FD) | Same as statfs but via open FD |
| 26 | `inotify_init1` | Space change notification channel | Returns FD for change events |
| 27 | `inotify_add_watch` | Register path for change events | IN_CREATE / IN_DELETE / IN_MODIFY / IN_MOVED_* |
| 28 | `inotify_rm_watch` | Unregister watch | Stops notifications for this watch descriptor |
| 262 | `fanotify_init` | Extended change notification | Permission events, mount-wide monitoring |
| 263 | `fanotify_mark` | Configure fanotify watch | Mark filesystem / mount / inode |
| 40 | `mount` | Denied (sandbox) or restricted | Only permitted for privileged compat processes |
| 39 | `umount2` | Denied (sandbox) or restricted | Same restrictions as mount |

**inotify translation:** The Space storage system supports object change notifications natively. The inotify translation maps Linux inotify watch descriptors to Space change subscriptions. When a Space object changes, the translation layer synthesizes an `inotify_event` structure and delivers it via the inotify FD's read buffer.

#### §5.2.10 Miscellaneous

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 29 | `ioctl` | Per-device dispatch | tty → terminal, DRM → GPU bridge, input → input subsystem |
| 25 | `fcntl` | FD flag manipulation | F_DUPFD, F_GETFL/F_SETFL, F_GETFD/F_SETFD, F_ADD_SEALS |
| 23 | `dup` | FD table duplicate entry | New FD pointing to same underlying resource |
| 24 | `dup3` | FD table duplicate with target | Atomically closes target FD if open |
| 278 | `getrandom` | AIOS entropy source | GRND_RANDOM / GRND_NONBLOCK; backed by CNTPCT + HW RNG |
| 17 | `getcwd` | POSIX `translate_getcwd()` | Returns current Space path |
| 49 | `chdir` | POSIX `translate_chdir()` | Updates per-process working directory (Space path) |
| 50 | `fchdir` | Change directory by FD | Working directory from open directory FD |
| 166 | `umask` | Per-process creation mask | Stored in `LinuxThreadState`, applied to openat/mkdirat |
| 174 | `getuid` | Returns mapped UID | UID 1000 for regular processes, 0 for privileged |
| 176 | `getgid` | Returns mapped GID | GID 1000 for regular processes, 0 for privileged |
| 175 | `geteuid` | Returns effective UID | Same as getuid (no setuid support) |
| 177 | `getegid` | Returns effective GID | Same as getgid (no setgid support) |
| 158 | `getgroups` | Returns supplementary groups | Single group (mapped GID) |
| 160 | `uname` | Synthesized `utsname` | sysname="Linux", release="6.1.0-aios", machine="aarch64" |
| 179 | `sysinfo` | AIOS system stats | Populates uptime, RAM, load averages from kernel metrics |

**ioctl routing:** The `ioctl` syscall is dispatched based on the FD's underlying device type:

```rust
fn translate_ioctl(fd: i32, request: u64, arg: u64) -> i64 {
    let entry = fd_table.get(fd)?;
    match entry.kind {
        FdKind::Terminal { .. }   => terminal_ioctl(request, arg),   // TIOCGWINSZ, TCGETS, etc.
        FdKind::DrmDevice { .. }  => drm_ioctl(request, arg),       // DRM_IOCTL_* → GPU service
        FdKind::InputDevice { .. } => input_ioctl(request, arg),    // EVIOCG* → input subsystem
        FdKind::Socket { .. }     => socket_ioctl(request, arg),    // SIOCGIFADDR, etc.
        _ => -ENOTTY,
    }
}
```

**uname values:** The `uname` syscall reports `sysname = "Linux"` intentionally. Many applications check `uname.sysname` to detect Linux and enable Linux-specific code paths. Reporting "AIOS" would cause applications to fall back to generic POSIX paths or fail entirely.

Cross-ref: [posix.md](../posix.md) §9 (device ioctl).

#### §5.2.11 io_uring

| Linux NR | Name | AIOS Translation | Notes |
|---|---|---|---|
| 425 | `io_uring_setup` | Allocate SQ/CQ shared memory regions | Returns FD; see §6.3 |
| 426 | `io_uring_enter` | Drain SQ → batched AIOS IPC | Submit operations, wait for completions |
| 427 | `io_uring_register` | Pre-register buffers and FDs | IORING_REGISTER_BUFFERS, IORING_REGISTER_FILES |

See §6.3 for the full io_uring translation design.

-----

## §6 Linux-Specific Syscall Deep Dives

The following syscalls have no POSIX equivalent and require dedicated translation logic. They represent some of the most commonly used Linux-specific interfaces in modern applications.

### §6.1 epoll → IpcSelect Translation

The `epoll` family is the dominant I/O multiplexing interface in Linux applications. Nearly every event loop (GLib, libuv, tokio, Qt, systemd) uses `epoll` for file descriptor readiness notification. The translation maps `epoll` semantics onto AIOS `IpcSelect` (see [ipc.md](../../kernel/ipc.md) §3.1 — `IpcSelect`).

**Data structures:**

```rust
/// Per-process epoll instance, created by epoll_create1()
pub struct EpollContext {
    entries: Vec<EpollEntry>,            // registered FDs and their event masks
    select_context: IpcSelectContext,    // underlying AIOS IpcSelect state
}

/// Single entry in an epoll interest list
pub struct EpollEntry {
    fd: i32,                // Linux FD being monitored
    events: u32,            // EPOLLIN, EPOLLOUT, EPOLLHUP, EPOLLERR, etc.
    data: u64,              // user-provided data returned with events (epoll_event.data)
    edge_triggered: bool,   // EPOLLET flag — only report state transitions
    oneshot: bool,          // EPOLLONESHOT — disable after first event delivery
    last_state: u32,        // previous readiness state (for edge-trigger detection)
}
```

**Operation translation:**

`epoll_create1(flags)`:

- Allocate a new `EpollContext` with an empty entry list
- Create an `IpcSelectContext` to track the underlying AIOS wait state
- Return a new FD in the process FD table, typed as `FdKind::Epoll`
- EPOLL_CLOEXEC flag is stored in the FD table entry

`epoll_ctl(epfd, op, fd, event)`:

```text
EPOLL_CTL_ADD:
  1. Resolve target FD to its AIOS resource (channel or notification)
  2. Create EpollEntry with translated event mask
  3. Add SelectEntry to the IpcSelectContext
  4. Store user data (event.data) for return during epoll_wait

EPOLL_CTL_MOD:
  1. Find existing EpollEntry for this FD
  2. Update event mask and user data
  3. Update corresponding SelectEntry in IpcSelectContext

EPOLL_CTL_DEL:
  1. Remove EpollEntry for this FD
  2. Remove corresponding SelectEntry from IpcSelectContext
```

`epoll_wait(epfd, events, maxevents, timeout)`:

```text
1. Call ipc_select() on the EpollContext's SelectEntry array
   - timeout: -1 → indefinite, 0 → non-blocking poll, >0 → milliseconds
2. For each ready SelectEntry:
   a. Look up corresponding EpollEntry
   b. Compute current readiness state from AIOS channel/notification state
   c. Edge-triggered (EPOLLET): report only if state differs from last_state
   d. Level-triggered (default): report if any monitored events are active
   e. EPOLLONESHOT: clear events mask after reporting (requires EPOLL_CTL_MOD to re-arm)
   f. Fill in epoll_event { events, data } in caller's array
3. Update last_state for edge-triggered entries
4. Return number of ready events (0 on timeout, -1 on error)
```

**Event mask translation:**

| Linux Event | AIOS IpcSelect Equivalent | Description |
|---|---|---|
| EPOLLIN | Channel has pending message / notification signaled | Data available for reading |
| EPOLLOUT | Channel send buffer has space | Ready for writing |
| EPOLLHUP | Channel peer closed | Hangup (peer disconnected) |
| EPOLLERR | Channel error state | Error condition on FD |
| EPOLLRDHUP | Peer shutdown write half | Read half of connection closed |
| EPOLLPRI | Out-of-band data available | Urgent data (rarely used) |

**Performance:** The translation overhead is in event mask conversion — a few bitwise operations per event. The actual blocking wait is a single `ipc_select()` call, which has the same performance as any native AIOS multi-wait.

-----

### §6.2 futex → AIOS Synchronization

The `futex` (fast userspace mutex) system call is the foundation of all userspace synchronization in Linux. Every `pthread_mutex`, `pthread_cond`, `sem_wait`, `std::mutex`, and `std::condition_variable` ultimately calls `futex`. The translation maps futex operations onto AIOS wait queues keyed by physical address.

**Data structures:**

```rust
/// Global futex wait table, keyed by physical page + offset
pub struct FutexTable {
    waiters: BTreeMap<FutexKey, Vec<FutexWaiter>>,
}

/// Unique key for a futex location — identifies the physical memory word
pub struct FutexKey {
    page_phys: PhysAddr,    // physical address of the page containing the futex
    offset: u32,            // byte offset within the page (0..4095)
}

/// A thread waiting on a futex word
pub struct FutexWaiter {
    thread: ThreadId,       // blocked thread
    bitset: u32,            // FUTEX_WAIT_BITSET mask (0xFFFFFFFF for plain FUTEX_WAIT)
    timeout: Option<u64>,   // absolute tick count for timeout, None = indefinite
}
```

**Why physical address?** Two processes can share memory (via `mmap MAP_SHARED` or `memfd_create`) and use a futex on a word in that shared region. The virtual addresses may differ between processes, but the physical address is the same. Using the physical page + offset as the key ensures that futex operations work correctly across processes sharing memory.

**Operation translation:**

`futex(uaddr, FUTEX_WAIT, val, timeout)`:

```text
1. Translate uaddr (virtual) to physical page + offset → FutexKey
2. Atomically: read *uaddr
   a. If *uaddr != val → return -EAGAIN (value changed, no wait needed)
   b. If *uaddr == val → add FutexWaiter to FutexTable[key]
3. Block current thread (AIOS thread state → Blocked)
4. On timeout expiry: remove waiter, return -ETIMEDOUT
5. On wake: remove waiter, return 0
```

`futex(uaddr, FUTEX_WAKE, val)`:

```text
1. Translate uaddr to FutexKey
2. Look up FutexTable[key]
3. Wake up to `val` waiters (val=1 for mutex unlock, val=INT_MAX for broadcast)
4. Woken threads are moved to AIOS scheduler run queue
5. Return number of threads woken
```

`futex(uaddr, FUTEX_WAIT_BITSET, val, timeout, bitset)`:

```text
Same as FUTEX_WAIT, but waiter stores the bitset mask.
FUTEX_WAKE_BITSET only wakes waiters whose bitset ANDs non-zero with the wake bitset.
Used for selective wakeup (e.g., readers vs writers in rwlock implementations).
```

`futex(uaddr, FUTEX_LOCK_PI, ...)`:

```text
1. Read *uaddr — if 0, CAS to current TID → lock acquired, return 0
2. If *uaddr != 0, extract owner TID
3. Set FUTEX_WAITERS bit in *uaddr
4. Register waiter with priority inheritance:
   a. Look up owner thread in AIOS scheduler
   b. Boost owner's priority to max(owner_priority, waiter_priority)
   c. Track PI chain (transitive: if owner is itself waiting on a PI futex)
5. Block current thread
6. On unlock (FUTEX_UNLOCK_PI): owner clears *uaddr, wakes highest-priority waiter
```

Cross-ref: [scheduler.md](../../kernel/scheduler.md) — priority inheritance is tracked in `SchedEntity.inherited_priority` and bounded to `MAX_INHERITANCE_DEPTH=8`.

**Robust futex list (`set_robust_list` / `get_robust_list`):**

When a thread exits (or is killed), the kernel walks the thread's robust futex list. For each entry:

```text
1. Read the futex word at the listed address
2. If the owner TID matches the dying thread:
   a. Set FUTEX_OWNER_DIED bit in the futex word
   b. Clear owner TID
   c. Wake one waiter (who will see FUTEX_OWNER_DIED and can recover the lock)
```

This prevents deadlock when a thread dies while holding a pthread mutex configured with `PTHREAD_MUTEX_ROBUST`.

-----

### §6.3 io_uring → Batched IPC

`io_uring` is Linux's high-performance asynchronous I/O interface, designed to minimize syscall overhead through shared-memory submission and completion queues. Modern applications (databases, web servers, file managers) increasingly use `io_uring` for batched I/O. The translation maps `io_uring` operations onto batched AIOS IPC calls.

**Architecture:**

```text
┌────────────────────────────────────────────┐
│          Linux Application                  │
│   io_uring_prep_read(sqe, fd, buf, len)    │
│   io_uring_submit(&ring)                   │
│   io_uring_wait_cqe(&ring, &cqe)          │
└────────────────┬───────────────────────────┘
                 │ SVC #0 (io_uring_enter)
                 ▼
┌────────────────────────────────────────────┐
│      io_uring Translation Layer            │
│  1. Drain SQ entries from shared memory    │
│  2. Translate each SQE opcode → AIOS op    │
│  3. Batch compatible ops into IPC calls    │
│  4. Post CQE results to shared memory      │
└────────────────┬───────────────────────────┘
                 │ IPC calls
                 ▼
┌────────────────────────────────────────────┐
│      AIOS Services (Space, Network, etc.)  │
└────────────────────────────────────────────┘
```

**`io_uring_setup(entries, params)` translation:**

```text
1. Allocate two SharedMemoryRegions:
   a. SQ ring: header + SQE array (entries × 64 bytes)
   b. CQ ring: header + CQE array (entries × 16 bytes, typically 2× SQ size)
2. Map both regions into the calling process's address space
3. Initialize ring buffer headers (head, tail, mask, flags)
4. Store params output: sq_off (SQ field offsets), cq_off (CQ field offsets)
5. Return FD for the io_uring instance
```

**`io_uring_enter(fd, to_submit, min_complete, flags)` translation:**

```text
1. Read `to_submit` SQE entries from the SQ ring (advance SQ head)
2. For each SQE, translate opcode:
   - IORING_OP_NOP         → no-op, immediate CQE with result=0
   - IORING_OP_READ        → translate_read(sqe.fd, sqe.buf, sqe.len, sqe.offset)
   - IORING_OP_WRITE       → translate_write(sqe.fd, sqe.buf, sqe.len, sqe.offset)
   - IORING_OP_FSYNC       → flush FD to block engine
   - IORING_OP_POLL_ADD    → add FD to IpcSelect set
   - IORING_OP_POLL_REMOVE → remove FD from IpcSelect set
   - IORING_OP_SENDMSG     → translate_sendmsg(sqe.fd, sqe.msg)
   - IORING_OP_RECVMSG     → translate_recvmsg(sqe.fd, sqe.msg)
   - Unsupported opcode    → CQE with result=-ENOSYS
3. Batch compatible operations:
   - Multiple reads to same service → single batched IPC with scatter list
   - Multiple writes to same service → single batched IPC with gather list
4. If min_complete > 0: block until at least min_complete CQEs are posted
5. Post CQE entries to the CQ ring (advance CQ tail):
   - cqe.res = operation result (bytes transferred or negative errno)
   - cqe.user_data = sqe.user_data (preserved for correlation)
6. Return number of CQEs posted
```

**`io_uring_register(fd, opcode, arg)` translation:**

| Registration Opcode | Translation |
|---|---|
| `IORING_REGISTER_BUFFERS` | Pre-register shared memory regions for zero-copy I/O |
| `IORING_UNREGISTER_BUFFERS` | Release pre-registered buffers |
| `IORING_REGISTER_FILES` | Pre-resolve FDs to AIOS channels for faster dispatch |
| `IORING_UNREGISTER_FILES` | Release pre-resolved channels |

**Performance advantage:** Linux applications using `io_uring` issue fewer syscalls — a single `io_uring_enter` can submit dozens of I/O operations and wait for completions. The translation layer preserves this batching advantage: multiple SQEs are translated and dispatched in a single kernel entry, and the shared-memory SQ/CQ rings avoid data copying between userspace and the compatibility layer.

-----

### §6.4 eventfd / signalfd / timerfd

These three FD types provide event delivery through the standard `read`/`write`/`epoll` interface, allowing applications to unify event handling in a single event loop. They are used extensively by GLib (GTK applications), Qt, systemd, and libuv.

#### eventfd — Counter-Based Notification

`eventfd2(initval, flags)`:

- Create an AIOS notification channel with counter semantics
- Return an FD backed by `FdKind::EventFd { counter: AtomicU64, waiters: WaitQueue }`
- EFD_NONBLOCK: set FD to non-blocking mode
- EFD_CLOEXEC: close-on-exec flag
- EFD_SEMAPHORE: read decrements by 1 instead of resetting to 0

`read(eventfd)`:

- If counter > 0: return counter value as `u64`, reset counter to 0
- If counter > 0 and EFD_SEMAPHORE: return 1, decrement counter by 1
- If counter == 0: block (or return -EAGAIN if non-blocking)

`write(eventfd, val)`:

- Add `val` to counter (saturates at `u64::MAX - 1`)
- If counter was 0 and readers are waiting: wake one reader

`epoll integration`:

- EPOLLIN when counter > 0
- EPOLLOUT when counter < `u64::MAX - 1` (room to write)

#### signalfd — Signal Delivery via File Descriptor

`signalfd4(fd, mask, flags)`:

- Create (or update) an FD that receives signals matching `mask`
- Signals in `mask` are blocked from normal delivery (the FD consumes them)
- Return an FD backed by `FdKind::SignalFd { mask: u64, pending: VecDeque<SignalInfo> }`

`read(signalfd)`:

- Return one or more `signalfd_siginfo` structures (128 bytes each)
- Each structure contains: signal number, sender PID, sender UID, and additional info
- Blocks if no signals pending (or -EAGAIN if non-blocking)

The translation layer hooks into the AIOS notification delivery path. When a notification arrives that matches a signal mapped in a `signalfd` mask, the notification is converted to a `signalfd_siginfo` and queued on the signalfd's pending buffer instead of being delivered as a normal signal.

#### timerfd — Timer Expiry via File Descriptor

`timerfd_create(clockid, flags)`:

- Create a timer FD backed by an AIOS timer
- `clockid`: CLOCK_REALTIME or CLOCK_MONOTONIC
- Return an FD backed by `FdKind::TimerFd { timer: TimerId, expirations: AtomicU64 }`

`timerfd_settime(fd, flags, new_value, old_value)`:

- Arm the timer: `new_value.it_value` = initial expiration, `new_value.it_interval` = period
- TFD_TIMER_ABSTIME: interpret `it_value` as absolute time (not relative)
- Disarm: `it_value = {0, 0}`
- On each expiration: increment the expiration counter and wake readers

`read(timerfd)`:

- Return the number of expirations since last read as `u64`
- Block if no expirations pending (or -EAGAIN if non-blocking)
- Reset expiration counter to 0 after read

All three FD types integrate with `epoll` via the standard `EpollEntry` mechanism described in §6.1 — they produce `EPOLLIN` events when readable.

-----

### §6.5 memfd_create / userfaultfd

#### memfd_create — Anonymous Shared Memory with Sealing

`memfd_create(name, flags)`:

- Create an anonymous AIOS shared memory region via `SharedMemoryCreate`
- Return an FD backed by `FdKind::MemFd { shmem_id: SharedMemoryId, seals: u32 }`
- MFD_CLOEXEC: close-on-exec flag
- MFD_ALLOW_SEALING: permit `fcntl(F_ADD_SEALS)` on this FD (default: sealing disabled)

`fcntl(fd, F_ADD_SEALS, seals)`:

- Apply one or more seals to the memfd:

| Seal | Effect | AIOS Translation |
|---|---|---|
| F_SEAL_SEAL | Prevent further sealing | Lock the seal set |
| F_SEAL_SHRINK | Prevent `ftruncate` to smaller size | Remove `Resize` capability from shmem |
| F_SEAL_GROW | Prevent `ftruncate` to larger size | Remove `Resize` capability from shmem |
| F_SEAL_WRITE | Prevent all writes | Attenuate shared memory capability to read-only |
| F_SEAL_FUTURE_WRITE | Prevent new writable mappings | Block new `mmap(PROT_WRITE)` on this FD |

Seals are additive — once applied, they cannot be removed. This provides a mechanism for creating immutable shared memory regions, commonly used for:

- **JIT compilation:** Write machine code → F_SEAL_WRITE → mmap(PROT_EXEC). This satisfies W^X because the write capability is revoked before execute permission is granted.
- **Shared data between processes:** Producer fills buffer → seals it → passes FD to consumer. Consumer can trust the data will not be modified.
- **Wayland buffer sharing:** Compositor creates sealed buffers that clients cannot modify while being composited (see [wayland-bridge.md](./wayland-bridge.md) §7.2).

#### userfaultfd — Userspace Page Fault Handling

`userfaultfd(flags)`:

- Create a page fault notification FD
- Return an FD backed by `FdKind::UserFaultFd { regions: Vec<UserfaultRegion> }`
- O_NONBLOCK / O_CLOEXEC flags

`ioctl(uffd, UFFDIO_REGISTER, range)`:

- Register a virtual address range for userspace fault handling
- Pages in this range are not backed by physical memory initially
- Supported modes: UFFDIO_REGISTER_MODE_MISSING (handle page faults for unmapped pages)

`ioctl(uffd, UFFDIO_COPY, copy)`:

- Resolve a pending page fault by providing page contents
- `copy.dst` = faulting address, `copy.src` = source data, `copy.len` = size
- The compatibility layer translates this to an AIOS `MemMap` of the target page with the provided contents

`read(uffd)`:

- Returns `uffd_msg` structures describing pending page faults
- Each message contains: faulting address, fault flags, thread ID
- Blocks until a fault occurs in a registered region

**Use cases in the compatibility layer:**

- **Lazy loading:** Map a large file without reading it all into memory. Only load pages as they are accessed.
- **Live migration:** Transfer process state between machines by faulting in pages on demand from the source.
- **AI model weight loading:** Cross-ref: [memory/ai.md](../../kernel/memory/ai.md) §6 — userfaultfd enables demand-loading of model weights from storage, loading only the layers needed for the current inference pass.

The AIOS translation intercepts page faults in registered userfaultfd regions at the kernel level. Instead of delivering a SIGSEGV, the kernel posts a fault notification to the userfaultfd's read buffer. The userspace handler resolves the fault (e.g., by fetching data from network or decompressing from storage) and calls `UFFDIO_COPY` to provide the page contents.
