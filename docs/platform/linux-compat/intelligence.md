# AIOS Linux Compatibility Intelligence & Validation

Part of: [linux-compat.md](../linux-compat.md) — Linux Binary & Wayland Compatibility
**Related:** [ai-native.md](../compositor/ai-native.md) §13 — Kernel-internal ML pattern, [airs.md](../../intelligence/airs.md) — AI Runtime Services, [fuzzing.md](../../security/fuzzing.md) — Fuzzing framework, [observability.md](../../kernel/observability.md) — Structured logging and tracing

-----

## §13 AI-Native Improvements & Validation

The Linux compatibility layer is a translation boundary — every operation crosses it. This makes it an ideal point for AI observation and optimization. The compatibility layer instruments every syscall, buffer transfer, and capability check, providing a rich signal stream for both kernel-internal ML (frozen models, no AIRS dependency) and AIRS-dependent intelligence (semantic understanding, runtime adaptation).

### §13.1 Learned Syscall Prediction (Kernel-Internal)

Desktop applications exhibit highly predictable syscall patterns. A text editor calls `read()` → `write()` → `fsync()` in sequence. A web browser calls `epoll_wait()` → `recvfrom()` → `write()` in tight loops. The compatibility layer can learn these patterns and pre-stage resources for predicted operations.

#### §13.1.1 Prediction Model

The predictor uses a small decision tree (~10 KiB) trained offline on syscall traces from representative Linux workloads:

```rust
pub struct SyscallPredictor {
    /// Decision tree nodes (pre-trained, frozen)
    tree: &'static [DecisionNode],
    /// Recent syscall history (ring buffer, per-thread)
    history: [u16; 8],
    /// Predicted next syscalls (top-3)
    predictions: [SyscallPrediction; 3],
}

pub struct SyscallPrediction {
    syscall_num: u16,
    confidence: u8,    // 0-100%
    resource_hint: ResourceHint,
}

pub enum ResourceHint {
    None,
    PreAllocBuffer { size: usize },
    PreOpenChannel { target: ServiceId },
    PreResolve { path_prefix: &'static str },
}
```

#### §13.1.2 Features

The decision tree uses these features (all derivable from per-thread state, no AIRS needed):

| Feature | Description | Type |
|---|---|---|
| `last_3_syscalls` | Previous 3 syscall numbers | Categorical |
| `last_result` | Success/failure of last syscall | Boolean |
| `fd_type` | Type of last FD operated on (file/socket/pipe/device) | Categorical |
| `time_since_last` | Microseconds since previous syscall | Numeric |
| `thread_state` | Running/blocked/in_handler | Categorical |
| `epoll_pending` | Number of events pending in epoll | Numeric |
| `buffer_size` | Size of last read/write buffer | Numeric |
| `app_class` | Application classification (browser/editor/build/game) | Categorical |

#### §13.1.3 Pre-Staging Actions

When the predictor has high confidence (>80%), the translation layer can:

- **Pre-allocate buffers**: If `read()` is predicted after `epoll_wait()`, allocate a receive buffer before the read syscall arrives
- **Pre-stage IPC channels**: If a Space Service call is predicted, prepare the IPC message envelope
- **Speculative path resolution**: If `openat()` is predicted, begin path resolution for the most likely directory
- **Connection pool warming**: If `connect()` is predicted, begin TCP handshake speculatively

All pre-staging is speculative — if the prediction is wrong, the pre-staged resources are reclaimed. The overhead of wrong predictions is bounded: wasted work is < 100ns per miss, and the predictor is disabled when its accuracy drops below 60%.

### §13.2 Adaptive Translation Optimization (Kernel-Internal)

Beyond predicting individual syscalls, the compatibility layer can optimize translation paths based on observed workload patterns.

#### §13.2.1 Hot Path Detection

The translation dispatcher counts invocations per syscall per thread. Frequently used paths (>1000 calls/sec) trigger optimizations:

- **Inline fast path**: For the top-3 syscalls per thread, bypass the general dispatch table and use specialized handlers with fewer branches
- **JIT argument marshaling**: Pre-compute register-to-argument mapping for hot syscalls (avoid re-parsing on every call)
- **Batched notification delivery**: If an application calls `epoll_wait()` frequently, batch pending notifications into a single delivery

#### §13.2.2 Batch Recognition

Sequential related syscalls can be coalesced:

| Pattern | Optimization | Benefit |
|---|---|---|
| `read()` × N on same FD | Single batched read IPC | Reduce IPC round-trips |
| `write()` × N on same FD | Buffered write, single flush | Reduce I/O syscalls |
| `openat()` + `fstat()` + `read()` + `close()` | Single "read file" IPC | 4 syscalls → 1 IPC |
| `epoll_wait()` → `recvfrom()` × N | Gather completion with data | Reduce context switches |

#### §13.2.3 Speculative Resolution

Path resolution is expensive: `openat("/usr/lib/libfoo.so")` requires traversing the mount namespace, space mapping, and object index. The translation layer caches recent resolutions and speculatively resolves paths based on access patterns:

- **Positive cache**: recently resolved paths → space object IDs (TTL: 30 seconds)
- **Negative cache**: recently failed lookups → ENOENT (TTL: 5 seconds)
- **Prefetch**: if the last 3 `openat()` calls were in the same directory, prefetch the directory listing

### §13.3 Anomaly Detection for Sandboxed Binaries (Kernel-Internal + AIRS)

#### §13.3.1 Kernel-Internal Anomaly Detection

A lightweight anomaly detector runs in the translation layer, using a decision tree trained on known-benign syscall patterns:

| Anomaly | Detection | Response |
|---|---|---|
| Unusual syscall sequence | N-gram model deviation > 3σ | Audit event, alert Inspector |
| Excessive fork/clone | >100 processes in 1 second | Deny fork, ENOMEM |
| Rapid file scanning | >1000 openat() in 1 second outside home | Audit event, throttle |
| Network burst | >10 connect() to distinct IPs in 1 second | Audit event, notify user |
| Memory exhaustion attempt | mmap() total > 2× memory limit | Deny, ENOMEM |
| Privilege probing | >10 denied capability checks in 1 second | Audit event, alert |

The anomaly detector uses per-sandbox counters with exponential decay (window: 1 second, decay factor: 0.5). This keeps memory usage constant (~64 bytes per sandbox) while tracking recent behavior.

#### §13.3.2 AIRS-Dependent Intelligence (Phase 29+)

When AIRS is available, deeper behavioral analysis becomes possible:

- **Dynamic sandbox policy recommendation**: AIRS observes application behavior during first run and recommends a minimum-privilege sandbox profile. "Firefox needs DisplayCapability, NetworkCapability, and AudioCapability. Camera and location are not needed."
- **Application compatibility prediction**: Before running a binary, AIRS analyzes its ELF imports and predicts which syscalls it will use. If critical syscalls are unsupported, AIRS warns the user before launch.
- **Automatic portal configuration**: AIRS observes which portal services an application accesses and pre-configures consent decisions for the next launch.
- **Cross-application correlation**: AIRS detects when multiple sandboxed applications are cooperating (e.g., a malicious app launching a helper to exfiltrate data) by correlating network activity, clipboard usage, and IPC patterns.
- **Behavioral fingerprinting**: AIRS builds a behavioral profile for each application (typical syscall distribution, memory usage, network pattern). Deviations from the profile trigger alerts — useful for detecting compromised applications.

### §13.4 Kernel-Internal ML Summary

All kernel-internal ML in the compatibility layer follows the same constraints as elsewhere in AIOS (see [compositor/ai-native.md](../compositor/ai-native.md) §13):

| Constraint | Value |
|---|---|
| Total memory budget | < 64 KiB for all models |
| Model type | Decision trees (frozen, no runtime training) |
| Inference latency | < 1 µs per prediction |
| Update mechanism | Shipped with OS updates, not trained on-device |
| Fallback | Models disabled by default; opt-in via system settings |
| AIRS dependency | None — works offline, no semantic understanding needed |

-----

### §13.5 Testing & Validation Strategy

The Linux compatibility layer requires comprehensive testing at multiple levels to ensure that unmodified Linux applications work correctly.

#### §13.5.1 Syscall Conformance Testing

**Linux Test Project (LTP):**
The Linux Test Project provides ~3000 syscall test cases. The compatibility layer targets a subset:

| Category | LTP Tests | Target Pass Rate | Notes |
|---|---|---|---|
| File I/O | ~400 | 95% | Core POSIX I/O, well-tested by Phase 15 |
| Process/thread | ~350 | 90% | clone, fork, exec, wait |
| Memory management | ~250 | 85% | mmap, mprotect, mremap |
| Signal handling | ~200 | 85% | sigaction, signal delivery |
| IPC (Linux-specific) | ~150 | 80% | epoll, futex, eventfd, timerfd |
| Network | ~300 | 80% | Socket operations, networking |
| Filesystem metadata | ~200 | 75% | statfs, inotify, extended attrs |
| io_uring | ~100 | 70% | Newer API, partial support |

Known failures are annotated with explanations (e.g., "fails: requires kernel module loading" or "fails: uses unsupported bpf()"). Each release tracks the pass rate trend.

**Per-syscall unit tests:**
Every translated syscall has a dedicated test that:
1. Issues the Linux syscall with known arguments
2. Verifies the return value matches Linux behavior
3. Verifies side effects (file created, signal delivered, memory mapped)
4. Tests error conditions (invalid arguments → correct errno)

#### §13.5.2 Application Compatibility Matrix

| Tier | Application | Category | Key Requirements | Target |
|---|---|---|---|---|
| **Tier 1 (must work)** | Firefox | Web browser | Wayland, GPU, network, audio | Full functionality |
| | GIMP | Image editor | Wayland, file I/O, X11 (some plugins) | Full functionality |
| | LibreOffice | Office suite | Wayland, file I/O, printing | Full functionality |
| | VS Code | Code editor | Wayland (Electron), file I/O, terminal, network | Full functionality |
| | Blender | 3D modeling | Wayland, GPU (Vulkan), file I/O | Full functionality |
| **Tier 2 (should work)** | Electron apps | Framework | Chromium runtime, Wayland | Most apps work |
| | GNOME apps | Desktop | GTK4, Wayland, D-Bus | Most apps work |
| | KDE apps | Desktop | Qt6, Wayland, D-Bus | Most apps work |
| | Steam | Gaming | Wayland, Vulkan, network | Launcher works; game compat varies |
| | Wine | Compat layer | Wayland, GPU, large address space | Wine itself runs; app compat varies |
| **Tier 3 (best effort)** | Docker CLI | Container | Namespace creation (limited) | CLI only, no daemon |
| | Flatpak | Package manager | Portal integration | Install + run with AIOS portals |
| | Android apps (via Waydroid) | Mobile | Full Android stack | Experimental |

Each Tier 1 application has an automated test harness:
1. Launch the application in a sandbox
2. Perform scripted interactions (open file, save, render)
3. Capture screenshots and compare against reference images
4. Measure launch time, memory usage, and CPU utilization
5. Check audit log for denied operations

#### §13.5.3 Performance Benchmarks

| Benchmark | Metric | Target | Method |
|---|---|---|---|
| Syscall overhead | Latency per translated syscall | < 200ns (trap), < 50ns (library) | `getpid()` microbenchmark (1M iterations) |
| File I/O throughput | MB/s for sequential read/write | > 80% of native AIOS | `dd if=/dev/zero of=test bs=4K count=10000` |
| Process creation | fork + exec latency | < 10ms | Launch `/bin/true` 1000 times |
| Application launch | Time to first frame | Firefox < 5s, VS Code < 3s | Measure from exec to first Wayland surface commit |
| Wayland rendering | Frames per second (FPS) | > 55 FPS at 1080p | `glmark2-wayland` benchmark |
| Memory overhead | Per-sandbox memory cost | < 8 MB baseline | Measure RSS of sandbox runtime without application |
| epoll throughput | Events per second | > 500K events/sec | `epoll_wait()` with many active FDs |
| futex contention | Lock acquisition latency | < 1µs uncontended | Mutex lock/unlock microbenchmark |

Performance regression testing runs on every commit that modifies the translation layer. Results are tracked in a time-series dashboard.

#### §13.5.4 Fuzzing

The compatibility layer is a security-critical attack surface — it accepts arbitrary syscall numbers and arguments from untrusted binaries. Fuzzing is essential.

| Fuzz Target | Input Space | Engine | Priority |
|---|---|---|---|
| `linux_syscall_dispatch()` | Random (syscall_num, args[6]) | cargo-fuzz / libFuzzer | Critical |
| ELF loader | Malformed ELF files | cargo-fuzz + custom generator | Critical |
| Wayland protocol parser | Malformed Wayland messages | Smithay test harness + libFuzzer | High |
| /proc emulation | Random /proc paths and reads | cargo-fuzz | High |
| DRM ioctl translation | Random ioctl numbers and args | cargo-fuzz | Medium |
| Signal delivery | Random signal sequences | Custom test harness | Medium |
| auxv construction | Malformed ELF headers | cargo-fuzz (part of ELF fuzzer) | Medium |

Fuzz targets are integrated into CI with corpus-based regression testing. New crash inputs are added to the corpus after fixing.

Cross-ref: [fuzzing.md](../../security/fuzzing.md) §6 (fuzz target catalog) — add Linux compat targets to the catalog.

#### §13.5.5 Compatibility Certification

A Linux binary can receive an AIOS compatibility certification:

| Level | Meaning | Criteria |
|---|---|---|
| **Certified** | Fully tested, all features work | Passes automated test suite, manual QA review |
| **Compatible** | Works with known limitations | Passes core tests, minor features may not work |
| **Experimental** | May work, not fully tested | Launches successfully, basic functionality verified |
| **Incompatible** | Known to not work | Uses unsupported syscalls, kernel modules, or features |

The certification database is maintained as a Space object, queryable by users before installing an application.
