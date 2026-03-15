# AIOS Virtual Filesystem Emulation

Part of: [linux-compat.md](../linux-compat.md) — Linux Binary & Wayland Compatibility
**Related:** [posix.md](../posix.md) §5–§9 — FD table and device translation, [subsystem-framework.md](../subsystem-framework.md) §8 — PosixBridge trait, [device-model.md](../../kernel/device-model.md) — Device registry

-----

## §11 Virtual Filesystem Emulation

Linux applications expect a rich filesystem hierarchy — `/proc`, `/sys`, `/dev` — that exposes kernel state, device information, and special files. On Linux, these are backed by in-kernel virtual filesystem implementations (procfs, sysfs, devtmpfs) that generate content dynamically from kernel data structures. Applications read from these paths as if they were regular files, and the kernel synthesizes responses on each read.

AIOS provides these as virtual filesystem handlers within the Linux compatibility layer. Rather than implementing VFS infrastructure in the AIOS kernel (which has no VFS — it uses IPC-based space services), the compatibility layer intercepts `open()`, `read()`, `stat()`, and `readdir()` calls targeting `/proc`, `/sys`, and `/dev` paths and synthesizes responses from AIOS kernel state, the device registry, and subsystem services. The virtual filesystem handlers run in userspace as part of the Linux compatibility layer, querying kernel state through AIOS's standard IPC and syscall interfaces.

This approach keeps the AIOS kernel clean — it has no knowledge of procfs or sysfs — while providing the filesystem view that Linux applications depend on. Tools like `htop`, `lsblk`, `lscpu`, `free`, and `sensors` work unmodified because they read the same `/proc` and `/sys` paths they expect on Linux.

### §11.1 procfs (/proc)

The `/proc` filesystem is the primary interface through which Linux applications introspect process state and system configuration. AIOS synthesizes procfs content from its native kernel data structures — process tables, scheduler statistics, memory pool counters, and device registry entries.

#### §11.1.1 Per-Process procfs (/proc/self/\*, /proc/[pid]/\*)

Each process sees its own state through `/proc/self/` (a symlink to `/proc/[pid]/` for the calling process). The compatibility layer translates each file to the appropriate AIOS data source:

| Path | Content | AIOS Data Source |
|---|-----|---|
| `/proc/self/maps` | Memory mappings | UserAddressSpace segment list, formatted as `start-end perms offset dev inode pathname` |
| `/proc/self/status` | Process status | Thread count, VmRSS from page table walk, capabilities (translated from AIOS CapabilityTable) |
| `/proc/self/cmdline` | Command line | Null-separated argument list preserved by ELF loader at process creation |
| `/proc/self/environ` | Environment | Null-separated environment variables preserved by ELF loader |
| `/proc/self/fd/` | Open file descriptors | Directory listing of FD table entries; each entry is a symlink to the target resource |
| `/proc/self/exe` | Executable path | Symlink to ELF binary path within the sandbox's filesystem view |
| `/proc/self/cwd` | Working directory | Symlink to current working directory (translated from space path) |
| `/proc/self/root` | Root directory | Symlink to `/` (always `/` — AIOS uses sandbox filesystem views, not chroot) |
| `/proc/self/stat` | Process statistics | `pid comm state ppid pgrp session tty minflt majflt utime stime priority nice num_threads starttime vsize rss` — sourced from SchedEntity and ProcessControl |
| `/proc/self/statm` | Memory statistics | `total resident shared text data` in pages — computed from UserAddressSpace page table walk |
| `/proc/self/io` | I/O statistics | `rchar wchar syscr syscw read_bytes write_bytes` — from per-process I/O accounting counters |
| `/proc/self/limits` | Resource limits | KernelResourceLimits translated to Linux `rlimit` format (soft/hard pairs) |
| `/proc/self/mountinfo` | Mount table | Sandbox's virtual mount table translated to Linux mountinfo format (`mount_id parent_id major:minor root mount_point options - fstype source super_options`) |
| `/proc/self/net/tcp` | TCP sockets | Active TCP connections for this process, sourced from Network Service via IPC |
| `/proc/self/net/udp` | UDP sockets | Active UDP sockets for this process, sourced from Network Service via IPC |

The handler trait for procfs operations:

```rust
/// Handler for /proc virtual filesystem reads.
///
/// Each procfs path is dispatched to a handler method that synthesizes
/// content from AIOS kernel state. Handlers return raw bytes — the
/// compatibility layer manages FD state, offset tracking, and partial reads.
pub trait ProcfsHandler {
    /// Read content from a procfs file.
    ///
    /// `pid` identifies the target process. For `/proc/self/` paths, the
    /// compatibility layer resolves `self` to the calling process's PID
    /// before dispatching.
    ///
    /// Returns the number of bytes written to `buf`, or an error.
    fn read(&self, pid: ProcessId, path: &str, buf: &mut [u8]) -> Result<usize, ProcfsError>;

    /// List entries in a procfs directory.
    ///
    /// Used for paths like `/proc/self/fd/` and `/proc/` (top-level PID listing).
    fn readdir(&self, pid: ProcessId, path: &str) -> Result<Vec<DirEntry>, ProcfsError>;

    /// Return file metadata for a procfs path.
    ///
    /// All procfs files are read-only. Directories return `S_IFDIR | 0o555`.
    /// Regular files return `S_IFREG | 0o444`. Symlinks return `S_IFLNK | 0o777`.
    fn stat(&self, pid: ProcessId, path: &str) -> Result<ProcfsStat, ProcfsError>;
}

/// Procfs-specific errors.
pub enum ProcfsError {
    /// Path does not exist within procfs.
    NotFound,
    /// Target process does not exist or is not visible in this PID namespace.
    ProcessNotFound,
    /// Buffer too small for the synthesized content.
    BufferTooSmall,
    /// The requested data is not available (e.g., network stats when
    /// Network Service is not running).
    DataUnavailable,
}
```

**maps format example:**

```text
00400000-00452000 r-xp 00000000 00:00 1234       /usr/bin/example
01000000-01002000 rw-p 00000000 00:00 1234       /usr/bin/example
10000000-10100000 rw-p 00000000 00:00 0          [heap]
7ffffffe0000-7fffffffffff rw-p 00000000 00:00 0  [stack]
```

The `start-end` ranges come from the UserAddressSpace segment table. The `dev` and `inode` fields use synthetic values (device `00:00`, inode from ObjectId). Permission bits map directly from the AIOS page table entry flags (RX, RW, RO). The `offset` field is always `0` for non-file-backed mappings.

#### §11.1.2 System-Wide procfs

System-wide `/proc` files expose aggregate kernel state. These are synthesized from AIOS kernel statistics, scheduler counters, and device registry data:

| Path | Content | AIOS Data Source |
|---|-----|---|
| `/proc/meminfo` | Memory statistics | FrameAllocator pool stats: MemTotal, MemFree, MemAvailable, Buffers (0), Cached (from slab), SwapTotal, SwapFree |
| `/proc/cpuinfo` | CPU information | DTB-sourced: processor number, model name, features (from `ID_AA64ISAR*`, `ID_AA64PFR*`), CPU implementer, architecture, variant, part, revision |
| `/proc/stat` | Kernel/system statistics | Per-CPU tick counts from scheduler: user, nice, system, idle, iowait (iowait from storage I/O wait counter) |
| `/proc/uptime` | System uptime | `TICK_COUNT / 1000` seconds (with fractional), idle time accumulated from scheduler idle thread counters |
| `/proc/version` | Kernel version | Static string: `"Linux version 6.1.0-aios (aios@build) (gcc 13.0) #1 SMP PREEMPT"` — fake Linux version for compatibility |
| `/proc/loadavg` | Load averages | 1/5/15 minute exponential moving averages from scheduler run queue depth sampling; `running/total` thread counts; last allocated PID |
| `/proc/mounts` | Mount table | Sandbox's virtual mount table in Linux `mtab` format (`device mountpoint fstype options 0 0`) |
| `/proc/filesystems` | Supported filesystems | Static string: `"nodev\ttmpfs\nnodev\tproc\nnodev\tsysfs\n\text4\n"` — declares filesystem types for compatibility |
| `/proc/cmdline` | Kernel command line | AIOS kernel command line from DTB `bootargs` (empty string if none) |
| `/proc/interrupts` | Interrupt statistics | Per-CPU interrupt counts from GIC statistics: columns for each CPU, rows for each INTID |
| `/proc/softirqs` | Software interrupt counts | Translated from AIOS timer tick counts, IPC delivery counts, and scheduler statistics |

**Security filtering:** System-wide `/proc` files may leak information about processes outside the caller's sandbox. For sandboxed Linux binaries, system-wide procfs is filtered:

- `/proc/` top-level directory listing shows only PIDs within the sandbox's PID scope (see §12.1)
- `/proc/stat` aggregates only the sandbox's own CPU usage into user/system/idle counters
- `/proc/meminfo` reports the sandbox's memory quota as MemTotal and current usage as MemFree
- `/proc/loadavg` reports load averages scoped to the sandbox's thread pool
- `/proc/interrupts` and `/proc/softirqs` are empty for sandboxed processes (no hardware visibility)

This filtering is conceptually equivalent to a Linux PID namespace — the sandbox sees only its own world. The compatibility layer applies filtering based on the SandboxId associated with the calling process.

-----

### §11.2 sysfs (/sys)

The `/sys` filesystem exposes the device hierarchy and kernel subsystem attributes as a directory tree. Linux applications and tools (`lscpu`, `lsblk`, `sensors`, `NetworkManager`) read sysfs to discover hardware capabilities, configure devices, and monitor state.

AIOS synthesizes sysfs content from the DeviceRegistry ([device-model.md](../../kernel/device-model.md) §4) and subsystem services. Each sysfs directory subtree is backed by a handler that queries the appropriate AIOS service via IPC.

#### §11.2.1 Device Hierarchy

**Network devices (`/sys/class/net/`):**

| Path | Content | AIOS Source |
|---|-----|---|
| `eth0/address` | MAC address | Network Service: interface hardware address |
| `eth0/carrier` | Link state (`0` or `1`) | Network Service: link detection |
| `eth0/duplex` | `full` or `half` | Network Service: negotiated duplex |
| `eth0/mtu` | MTU in bytes | Network Service: current MTU |
| `eth0/speed` | Link speed in Mbps | Network Service: negotiated speed |
| `eth0/statistics/rx_bytes` | Received bytes | Network Service: per-interface counters |
| `eth0/statistics/tx_bytes` | Transmitted bytes | Network Service: per-interface counters |
| `eth0/statistics/rx_packets` | Received packets | Network Service: per-interface counters |
| `eth0/statistics/tx_packets` | Transmitted packets | Network Service: per-interface counters |
| `eth0/operstate` | `up`, `down`, `unknown` | Network Service: interface state |

Cross-ref: [networking/stack.md](../networking/stack.md) §4.1 (smoltcp integration), [networking/components.md](../networking/components.md) §3.2 (Connection Manager)

**Input devices (`/sys/class/input/`):**

| Path | Content | AIOS Source |
|---|-----|---|
| `input0/name` | Device name string | Input Subsystem: device descriptor |
| `input0/phys` | Physical path | Input Subsystem: bus topology |
| `input0/uniq` | Unique identifier | Input Subsystem: serial number |
| `input0/id/bustype` | Bus type code | Input Subsystem: HID descriptor |
| `input0/id/vendor` | Vendor ID | Input Subsystem: HID descriptor |
| `input0/id/product` | Product ID | Input Subsystem: HID descriptor |
| `input0/id/version` | Version | Input Subsystem: HID descriptor |

Cross-ref: [input/devices.md](../input/devices.md) §3.1 (device taxonomy), §3.3 (VirtIO-input)

**Display/GPU devices (`/sys/class/drm/`):**

| Path | Content | AIOS Source |
|---|-----|---|
| `card0/device/` | PCI/platform device symlink | DeviceRegistry: GPU device node |
| `card0/gt_cur_freq_mhz` | Current GPU frequency | GPU Service: DVFS state |
| `card0/gt_max_freq_mhz` | Maximum GPU frequency | GPU Service: frequency table |
| `card0-HDMI-A-1/enabled` | `enabled` or `disabled` | GPU Service: connector state |
| `card0-HDMI-A-1/status` | `connected` or `disconnected` | GPU Service: hotplug detect |
| `card0-HDMI-A-1/modes` | Supported display modes | GPU Service: EDID parsing |
| `card0-HDMI-A-1/edid` | Raw EDID blob | GPU Service: monitor EDID data |

Cross-ref: [gpu/display.md](../gpu/display.md) §6 (display controller), [gpu/drivers.md](../gpu/drivers.md) §3 (VirtIO-GPU)

**CPU topology (`/sys/devices/system/cpu/`):**

| Path | Content | AIOS Source |
|---|-----|---|
| `cpu0/online` | `1` (online) or `0` (offline) | SMP state: core online bitmap |
| `cpu0/topology/core_id` | Core ID within package | DTB: MPIDR-derived core ID |
| `cpu0/topology/physical_package_id` | Package (cluster) ID | DTB: MPIDR Aff1 field |
| `cpu0/cpufreq/scaling_cur_freq` | Current frequency in kHz | Timer: CNTFRQ_EL0 (or DVFS state) |
| `cpu0/cpufreq/scaling_governor` | Active governor name | Thermal/power: governor string |
| `cpu0/cpufreq/cpuinfo_max_freq` | Maximum frequency in kHz | DTB or platform: max frequency |
| `possible` | CPU mask (e.g., `0-3`) | SMP: MAX_CORES configuration |
| `present` | Present CPU mask | SMP: detected cores from DTB |
| `online` | Online CPU mask | SMP: successfully booted cores |

**Platform devices (`/sys/devices/platform/`):**

Directory entries generated from the DeviceRegistry. Each registered device appears as a subdirectory with standard attributes (`modalias`, `driver`, `uevent`).

Cross-ref: [device-model/representation.md](../../kernel/device-model/representation.md) §3 (HardwareDescriptor), §4 (DeviceRegistry)

The sysfs handler uses a tree structure to organize the virtual filesystem:

```rust
/// A node in the sysfs virtual filesystem tree.
///
/// The tree is built at sandbox creation time from the DeviceRegistry
/// and subsystem service state. Leaf nodes have a `read_fn` that
/// synthesizes content on demand; directory nodes have children.
pub struct SysfsNode {
    /// Node name (directory entry name).
    name: &'static str,
    /// Node type determines stat() behavior.
    node_type: SysfsNodeType,
    /// Child nodes (empty for files and symlinks).
    children: Vec<SysfsNode>,
    /// Read function for file nodes. Called on each read() to
    /// synthesize current content from AIOS state.
    read_fn: Option<fn(&SysfsContext) -> String>,
}

/// Sysfs node types mirror Linux sysfs conventions.
pub enum SysfsNodeType {
    /// Directory node (S_IFDIR | 0o555).
    Directory,
    /// Regular file with specified permissions (typically 0o444 for
    /// read-only attributes, 0o644 for writable tuning knobs).
    File { permissions: u16 },
    /// Symbolic link to another sysfs path or device node.
    Symlink { target: String },
}

/// Context passed to sysfs read functions, providing access to
/// AIOS kernel state and subsystem services.
pub struct SysfsContext {
    /// IPC channel to the DeviceRegistry query interface.
    device_registry: ChannelId,
    /// IPC channel to Network Service (for /sys/class/net/).
    network_service: Option<ChannelId>,
    /// IPC channel to GPU Service (for /sys/class/drm/).
    gpu_service: Option<ChannelId>,
    /// IPC channel to Input Subsystem (for /sys/class/input/).
    input_service: Option<ChannelId>,
    /// IPC channel to Thermal Service (for /sys/class/thermal/).
    thermal_service: Option<ChannelId>,
    /// Sandbox ID for capability-filtered views.
    sandbox_id: SandboxId,
}
```

#### §11.2.2 Power and Thermal

**Thermal zones (`/sys/class/thermal/`):**

| Path | Content | AIOS Source |
|---|-----|---|
| `thermal_zone0/temp` | Temperature in millidegrees C | Thermal subsystem: ThermalZone sensor reading |
| `thermal_zone0/type` | Zone type (`cpu`, `gpu`, etc.) | Thermal subsystem: ThermalZoneType |
| `thermal_zone0/trip_point_0_temp` | Trip point temperature | Thermal subsystem: ThermalTripPoint |
| `thermal_zone0/trip_point_0_type` | Trip type (`passive`, `critical`) | Thermal subsystem: trip point escalation type |
| `thermal_zone0/policy` | Thermal governor name | Thermal subsystem: active governor |

Cross-ref: [thermal/zones.md](../thermal/zones.md) §2 (ThermalZone), §3 (trip points)

**Power supply (`/sys/class/power_supply/`):**

| Path | Content | AIOS Source |
|---|-----|---|
| `BAT0/status` | `Charging`, `Discharging`, `Full` | Power management: battery state |
| `BAT0/capacity` | Percentage (0–100) | Power management: charge level |
| `BAT0/voltage_now` | Voltage in microvolts | Power management: ADC reading |
| `BAT0/current_now` | Current in microamps | Power management: ADC reading |
| `BAT0/technology` | `Li-ion`, `Li-poly` | Power management: battery type |
| `AC/online` | `1` (plugged) or `0` | Power management: AC adapter state |

Cross-ref: [power-management.md](../power-management.md)

**CPU frequency scaling (`/sys/devices/system/cpu/cpufreq/`):**

| Path | Content | AIOS Source |
|---|-----|---|
| `scaling_available_governors` | Space-separated list | Thermal/power: registered governors |
| `scaling_governor` | Active governor name | Thermal/power: active governor |
| `scaling_cur_freq` | Current frequency in kHz | DVFS state or CNTFRQ_EL0 |
| `scaling_min_freq` | Minimum frequency in kHz | Platform: frequency table |
| `scaling_max_freq` | Maximum frequency in kHz | Platform: frequency table |

Cross-ref: [thermal/cooling.md](../thermal/cooling.md) §4 (DVFS), [thermal/scheduling.md](../thermal/scheduling.md) §6 (ThermalState)

-----

### §11.3 devtmpfs (/dev)

The `/dev` directory provides device nodes — character and block special files that Linux applications use to access hardware. AIOS provides these as virtual device nodes within the compatibility layer, routing I/O operations to the appropriate AIOS subsystem via its PosixBridge trait.

Each device node has a major/minor number pair that Linux applications use with `stat()` and that the compatibility layer uses for internal routing. The device nodes are not backed by actual Linux device drivers — they are translation endpoints.

| Device | Type | Major,Minor | AIOS Backend | Notes |
|---|-----|---|-----|---|
| `/dev/null` | char | 1,3 | Kernel-handled | Write discards all data, read returns EOF |
| `/dev/zero` | char | 1,5 | Kernel-handled | Read returns zero bytes, write discards |
| `/dev/full` | char | 1,7 | Kernel-handled | Write returns `ENOSPC`, read returns zero bytes |
| `/dev/random` | char | 1,8 | AIOS RNG service | Blocking (legacy compat; identical to urandom on Linux 5.6+) |
| `/dev/urandom` | char | 1,9 | AIOS RNG service | Non-blocking cryptographic random bytes |
| `/dev/tty` | char | 5,0 | Terminal subsystem | Controlling terminal for the process |
| `/dev/pts/*` | char | 136,N | PTY from terminal | Pseudo-terminal slave devices (N = PTY index) |
| `/dev/ptmx` | char | 5,2 | PTY multiplexer | Opens new PTY master/slave pair via `posix_openpt()` |
| `/dev/shm/` | tmpfs | — | SharedMemory service | POSIX shared memory (`shm_open()`/`shm_unlink()`) |
| `/dev/dri/card0` | char | 226,0 | DRM Bridge | GPU display + rendering (requires `DisplayCapability`) |
| `/dev/dri/renderD128` | char | 226,128 | DRM Bridge | GPU render-only node (sandboxed compute/rendering) |
| `/dev/input/event*` | char | 13,64+N | Input subsystem | evdev input devices (requires `InputCapability`) |
| `/dev/snd/controlC0` | char | 116,0 | Audio subsystem | ALSA control device |
| `/dev/snd/pcmC0D0p` | char | 116,16 | Audio subsystem | ALSA playback device |
| `/dev/snd/pcmC0D0c` | char | 116,24 | Audio subsystem | ALSA capture device (requires `AudioCaptureCapability`) |
| `/dev/video*` | char | 81,N | Camera subsystem | V4L2 camera devices (requires `CameraCapability`) |
| `/dev/fb0` | char | 29,0 | Framebuffer compat | Legacy framebuffer interface (deprecated; prefer DRM) |
| `/dev/fuse` | char | 10,229 | **Denied** | FUSE not supported — returns `EACCES` |
| `/dev/kvm` | char | 10,232 | **Denied** | KVM not supported — returns `EACCES` |
| `/dev/net/tun` | char | 10,200 | **Denied** | TUN/TAP not supported in sandbox — returns `EACCES` |
| `/dev/stdin` | symlink | — | — | Symlink to `/proc/self/fd/0` |
| `/dev/stdout` | symlink | — | — | Symlink to `/proc/self/fd/1` |
| `/dev/stderr` | symlink | — | — | Symlink to `/proc/self/fd/2` |
| `/dev/fd` | symlink | — | — | Symlink to `/proc/self/fd` |

**Capability gating:** Device nodes that access sensitive hardware (camera, microphone, input, GPU) require the corresponding AIOS capability token. Opening a device node without the required capability returns `EACCES`. The capability check occurs at `open()` time — if a capability is revoked after open, subsequent I/O operations on the existing FD return `EACCES`.

**Device discovery:** The set of device nodes visible in `/dev` depends on the sandbox's device capabilities. A sandbox without `InputCapability` does not see `/dev/input/` entries in `readdir()`. A sandbox without `CameraCapability` does not see `/dev/video*` entries. This prevents device enumeration by unprivileged sandboxes.

Cross-ref: [posix.md](../posix.md) §9 (device translation), [subsystem-framework.md](../subsystem-framework.md) §8 (PosixBridge trait), [camera/security.md](../camera/security.md) §8.3 (CameraCapability), [audio/subsystem.md](../audio/subsystem.md) §3.2 (audio capabilities)

-----

## §12 Namespace & Resource Isolation

Linux applications — especially those designed for containers, Flatpak, or multi-user environments — expect namespace isolation: separate PID spaces, mount hierarchies, and network stacks. Linux provides these through the `clone()` flags `CLONE_NEWPID`, `CLONE_NEWNS`, `CLONE_NEWNET`, and the cgroup filesystem for resource control.

AIOS does not implement Linux namespaces in its kernel. Instead, the Linux compatibility layer provides equivalent isolation using AIOS's native capability system, per-sandbox resource tracking, and virtual filesystem filtering. The result is functionally identical to Linux namespaces from the application's perspective, but implemented entirely in userspace with no kernel namespace infrastructure.

-----

### §12.1 PID Namespace Equivalent

Each sandbox group operates within a virtual PID table that maps between the internal PIDs visible to Linux binaries and the real AIOS ProcessId/ThreadId values. Linux binaries within a sandbox see a clean PID space starting from 1, with no visibility into processes outside their sandbox.

```rust
/// Virtual PID namespace for a sandbox group.
///
/// Maps between virtual PIDs (what Linux binaries see) and real AIOS
/// ProcessId values (what the kernel uses). All PID-related syscalls
/// (getpid, getppid, kill, waitpid) pass through this mapping.
pub struct PidNamespace {
    /// Unique identifier for this sandbox.
    sandbox_id: SandboxId,
    /// Next virtual PID to allocate (starts at 1).
    next_vpid: AtomicU32,
    /// Virtual PID → real AIOS ProcessId.
    vpid_to_real: BTreeMap<u32, ProcessId>,
    /// Real AIOS ProcessId → virtual PID.
    real_to_vpid: BTreeMap<ProcessId, u32>,
}
```

**Behavioral rules:**

- **PID 1:** The first process in the sandbox receives virtual PID 1. This is the sandbox's init process — not the AIOS service manager. If PID 1 exits, all other processes in the sandbox are terminated (matching Linux PID namespace semantics).
- **`getpid()` / `getppid()`:** Return virtual PIDs from the namespace's mapping table. If the parent process is outside the sandbox, `getppid()` returns 0 (the sandbox's init process appears to have no parent).
- **`kill(pid, sig)`:** The `pid` argument is a virtual PID. The compatibility layer translates it to the real ProcessId before dispatching. Attempting to signal a PID outside the namespace returns `ESRCH`.
- **`waitpid(pid, ...)`:** Operates on virtual PIDs. A process can only wait on children within the same sandbox.
- **`/proc/` filtering:** The procfs handler (§11.1) uses the PID namespace to filter directory listings. Only virtual PIDs appear in `/proc/`. `/proc/[vpid]/` files synthesize content using the real ProcessId, but display the virtual PID in output (e.g., `/proc/self/stat`).
- **Process creation:** When a sandboxed Linux binary calls `fork()` or `clone()`, the new process is automatically added to the parent's PID namespace with the next available virtual PID.

-----

### §12.2 Mount Namespace Equivalent

Each sandbox has its own filesystem view, assembled from AIOS space objects and read-only system content. This view is constructed at sandbox creation time and cannot be modified by the sandboxed application (there is no `mount()` syscall support — it returns `EPERM`).

```text
/bin, /usr/bin     → read-only bind from system space (BSD + Linux tools)
/lib, /usr/lib     → read-only bind from system space (musl, glibc shim, shared libraries)
/etc               → read-only base from system space + writable overlay for sandbox-specific config
/home              → bind to user's home space (writable, scoped by SpaceCapability)
/tmp               → per-sandbox tmpfs (memory-backed, cleared on sandbox exit)
/run               → per-sandbox runtime directory (sockets, PID files)
/proc              → procfs handler (filtered by PID namespace, §11.1)
/sys               → sysfs handler (filtered by device capabilities, §11.2)
/dev               → devtmpfs handler (filtered by device capabilities, §11.3)
/var/tmp           → per-sandbox persistent temp (backed by user's home space, quota'd to 256 MiB)
```

**Filesystem view construction:**

```rust
/// Filesystem view definition for a sandbox.
///
/// Each entry maps a path prefix to a content source. Entries are
/// evaluated in longest-prefix-first order (same as AIOS PathResolver).
pub struct FilesystemView {
    /// Sandbox this view belongs to.
    sandbox_id: SandboxId,
    /// Mount entries, sorted by path length (longest first).
    mounts: Vec<ViewMount>,
}

pub struct ViewMount {
    /// Path prefix within the sandbox's filesystem.
    path: String,
    /// Source of content for this mount.
    source: ViewSource,
    /// Whether the mount is writable.
    writable: bool,
}

pub enum ViewSource {
    /// Backed by an AIOS space (files read/written via Space Service).
    Space { space_id: SpaceId, subpath: String },
    /// Read-only system content from initramfs.
    Initramfs { region: SharedMemoryId },
    /// Memory-backed temporary storage (cleared on sandbox exit).
    Tmpfs { max_size: usize },
    /// Virtual filesystem handler (procfs, sysfs, devtmpfs).
    Virtual { handler: VfsHandlerType },
    /// Writable overlay on top of a read-only base.
    Overlay { base: Box<ViewSource>, upper: SpaceId },
}
```

**Bind mount emulation:** If a Linux application calls `mount("--bind", source, target, ...)`, the compatibility layer checks whether the operation is permitted by the sandbox profile. Bind mounts within the sandbox's writable directories (e.g., moving content within `/home`) are translated to updates in the virtual mount table. Bind mounts targeting read-only paths or paths outside the sandbox return `EPERM`.

**`/etc` overlay pattern:** System configuration files (`/etc/resolv.conf`, `/etc/hosts`, `/etc/passwd`, `/etc/nsswitch.conf`) need sandbox-specific customization while most of `/etc` should be shared read-only. The overlay source provides this: reads check the writable upper layer first, then fall through to the read-only base. The compatibility layer pre-populates the upper layer with sandbox-specific files at creation time.

-----

### §12.3 Network Namespace Equivalent

Each sandbox gets an isolated network view enforced through AIOS's capability system and the Network Service. There is no kernel-level network namespace — isolation is achieved by routing all network operations through capability-gated IPC to the Network Service.

**Isolation properties:**

- **Loopback (`127.0.0.1`):** Per-sandbox. Two sandboxes binding to `127.0.0.1:8080` do not conflict, and cannot communicate via loopback. The Network Service maintains separate loopback routing tables per sandbox.
- **External network access:** Requires `NetworkCapability`. Without it, all `socket()` calls for `AF_INET` and `AF_INET6` return `EPERM`.
- **AF_UNIX sockets:** Visible only within the sandbox's `/run` directory. A Unix socket created by sandbox A is not visible to sandbox B, even if they share the same user space. The path-based routing uses the sandbox's `FilesystemView`.
- **DNS resolution:** Proxied through the AIOS Network Service. Sandboxes cannot access raw DNS (port 53) — the Network Service performs resolution on their behalf and returns results. This prevents DNS-based data exfiltration.
- **Port binding:** Each sandbox has its own port space for loopback. External port binding requires `NetworkCapability` with a port range grant.

Cross-ref: [networking/security.md](../networking/security.md) §6.3 (per-agent network isolation), §6.5 (layered trust)

-----

### §12.4 cgroup-like Resource Control

AIOS does not implement Linux cgroups. Cgroups are a kernel mechanism tightly coupled to the Linux process model and VFS. Instead, AIOS provides equivalent resource limits through its native capability and resource management infrastructure, mapped to a cgroup-like interface for Linux applications that read cgroup files.

| cgroup v2 Controller | AIOS Equivalent | Mechanism |
|---|-----|---|
| `cpu.max` | CPU time quota | Scheduler: per-sandbox CPU budget enforced as time slice cap per scheduling period |
| `cpu.weight` | CPU share | Scheduler: priority within SchedulerClass (higher weight = more favorable scheduling) |
| `memory.max` | Memory limit | UserAddressSpace: per-sandbox page quota; OOM kills within sandbox only |
| `memory.swap.max` | Swap limit | Per-sandbox swap page quota (from memory/reclamation.md §10 swap budget) |
| `memory.current` | Current usage | UserAddressSpace: sum of resident pages across all sandbox processes |
| `io.max` | I/O bandwidth | Storage subsystem: per-sandbox rate limiter (bytes/sec, IOPS) |
| `io.weight` | I/O share | Storage subsystem: priority in I/O scheduler |
| `pids.max` | Process count | Per-sandbox process table: maximum entries in PidNamespace |
| `devices.allow` | Device access | DeviceCapability tokens: granular per-device access control |
| `cpuset.cpus` | CPU affinity | CpuSet in SchedEntity: restricts which cores the sandbox's threads may run on |
| `cpuset.mems` | Memory node affinity | PagePool assignment: restricts which memory pools the sandbox may allocate from |

Resource limit structure:

```rust
/// Resource limits for a sandbox, equivalent to Linux cgroup v2 controllers.
///
/// These limits are enforced by AIOS's native resource management — the
/// scheduler, memory allocator, and storage subsystem all check sandbox
/// limits before granting resources.
///
/// Maps to AIOS KernelResourceLimits (task/process.rs) for kernel enforcement.
pub struct SandboxResourceLimits {
    /// Maximum number of processes in this sandbox (pids.max).
    max_processes: u32,
    /// Maximum threads per process within the sandbox.
    max_threads_per_process: u32,
    /// Maximum resident memory in bytes (memory.max).
    /// When exceeded, the sandbox's OOM killer selects a victim
    /// within the sandbox — other sandboxes are unaffected.
    max_memory_bytes: usize,
    /// Maximum swap usage in bytes (memory.swap.max).
    max_swap_bytes: usize,
    /// CPU quota: microseconds of CPU time per period (cpu.max numerator).
    cpu_quota_us: u64,
    /// CPU period: period length in microseconds (cpu.max denominator).
    /// A quota of 100000 with period 100000 = 1 full CPU.
    cpu_period_us: u64,
    /// Maximum I/O read bandwidth in bytes per second (io.max rbps).
    io_read_bps: u64,
    /// Maximum I/O write bandwidth in bytes per second (io.max wbps).
    io_write_bps: u64,
    /// CPU affinity mask (cpuset.cpus).
    cpu_affinity: CpuSet,
}
```

**cgroup filesystem emulation:** Linux applications that read cgroup files (e.g., container runtimes, systemd-based tools) expect paths like `/sys/fs/cgroup/memory.max`. The compatibility layer synthesizes these paths from the sandbox's `SandboxResourceLimits`:

| cgroup v2 Path | Content | Source |
|---|-----|---|
| `/sys/fs/cgroup/memory.max` | Memory limit in bytes | `max_memory_bytes` |
| `/sys/fs/cgroup/memory.current` | Current memory usage | UserAddressSpace page count * 4096 |
| `/sys/fs/cgroup/cpu.max` | `quota period` (space-separated) | `cpu_quota_us cpu_period_us` |
| `/sys/fs/cgroup/pids.max` | Process limit | `max_processes` |
| `/sys/fs/cgroup/pids.current` | Current process count | PidNamespace entry count |
| `/sys/fs/cgroup/io.max` | I/O limits by device | `"major:minor rbps=N wbps=N"` |

Writes to cgroup files from within the sandbox return `EPERM` — resource limits are set by the sandbox creator (the AIOS system or user), not by the sandboxed application.

Cross-ref: [task/process.rs](../../kernel/task/process.rs) (KernelResourceLimits) — `SandboxResourceLimits` is the Linux-facing API that maps to AIOS's native KernelResourceLimits at enforcement time.

-----

### §12.5 User Namespace Equivalent

Linux binaries running in AIOS sandboxes see a synthetic UID/GID environment. AIOS is fundamentally a single-user, capability-based system — it has no kernel-level concept of Unix users or groups. The compatibility layer synthesizes user identity for Linux applications that expect it.

**Identity mapping:**

| Linux View | Actual AIOS Behavior |
|---|-----|
| `uid 0` (root) inside sandbox | Mapped to sandbox owner's ProcessId outside. No real root privileges. |
| `uid 1000` (regular user) | Default non-root identity within sandbox. |
| `gid 1000` | Default group identity. |
| `setuid(0)` | No-op — returns success but does not grant additional privileges. |
| `setgid(0)` | No-op — returns success. |
| `setgroups()` | No-op — returns success. |
| Capability checks (`CAP_SYS_ADMIN`, etc.) | Translated to AIOS capability checks. `CAP_SYS_ADMIN` → denied unless sandbox has corresponding AIOS capability. |

**Synthesized identity files:**

- **`/etc/passwd`:** Generated at sandbox creation with entries for root (uid 0, shell `/bin/sh`) and the sandbox user (uid 1000, home `/home/user`). Applications that parse `/etc/passwd` (e.g., `whoami`, `id`, `ls -l`) see consistent identity information.
- **`/etc/group`:** Generated with entries for root (gid 0) and the sandbox user's group (gid 1000).
- **`/etc/shadow`:** Not provided (returns `EACCES`). Password authentication is not supported within sandboxes.

**File ownership:** All files within the sandbox appear owned by the sandbox's virtual uid (1000) and gid (1000). Files in read-only system mounts (`/bin`, `/lib`) appear owned by root (uid 0, gid 0). The compatibility layer translates `stat()` results to include these synthetic ownership values — the underlying AIOS space objects have no Unix ownership metadata.

**Privilege escalation prevention:** Even if a Linux binary has setuid permissions in its ELF metadata, the compatibility layer ignores setuid/setgid bits. All processes within a sandbox run with identical (unprivileged) authority, determined entirely by the sandbox's AIOS capability set. This eliminates an entire class of privilege escalation vulnerabilities present in traditional Unix systems.

Cross-ref: [sandbox.md](./sandbox.md) §9.1 (threat model), [capabilities.md](../../security/model/capabilities.md) §3.1 (capability token lifecycle)
