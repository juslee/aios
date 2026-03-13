# Fuzzing: Tooling and Infrastructure

Part of: [fuzzing-and-hardening.md](./fuzzing-and-hardening.md) — Fuzzing and Input Hardening
**Related:** [fuzzing-hardening-strategies.md](./fuzzing-hardening-strategies.md) — Hardening strategies, [fuzzing-adoption-roadmap.md](./fuzzing-adoption-roadmap.md) — Phased adoption, [fuzzing-ai-native.md](./fuzzing-ai-native.md) — AI-native strategies

---

## 5. Tooling Strategy

AIOS adopts a tiered tooling strategy that matches tool maturity and complexity to the kernel's development phase. Tools graduate from Tier 1 (immediate adoption) through Tier 3 (custom infrastructure) as the project matures.

### 5.1 Tier 1: Immediate Adoption (Phase 0+)

Tools that require no custom infrastructure and integrate directly into the existing Rust/Cargo workflow.

| Tool | Purpose | Target | CI Integration |
|---|---|---|---|
| **cargo-fuzz** (libFuzzer) | Host-side fuzz harnesses | `shared/` crate APIs, allocator logic, parsers | Target: nightly `cargo fuzz run <target> -- -max_total_time=3600` |
| **proptest** | Property-based testing with shrinking | Data structure invariants (buddy coalescing, ring buffer ordering, PTE bit manipulation) | Target: every PR as standard `#[test]` |
| **Loom** | Exhaustive atomic interleaving exploration | Atomic protocols: `TICK_COUNT`, `NEED_RESCHED`, `IN_SCHEDULER`, slab magazine swap | Target: every PR as `#[test]` with `--release` |
| **Rudra** | Unsafe Rust pattern detection | Entire codebase: panic safety, Send/Sync variance, higher-order invariant bugs | Target: weekly Docker scan |
| **Kani** | Bit-precise bounded model checking (CBMC) | PTE arithmetic, capability attenuation subset proof, buddy allocator math, W^X invariant | Target: nightly `cargo kani --harness <name>` |
| **Bolero** | Unified fuzz/property-test framework | Dual-mode harnesses: same test runs under proptest (quick) and libFuzzer (deep) | Target: every PR (proptest) + nightly (fuzz) |

**Rudra** (Bae et al., SOSP 2021) detects three bug patterns at ecosystem scale: panic safety in unsafe contexts, higher-order invariant violations, and Send/Sync variance bugs. It has found 264 bugs including 76 CVEs across the Rust ecosystem, including bugs in the standard library. Parts of its analysis are now integrated into the official Rust linter.

**Kani** (AWS) proves absence of undefined behavior via bit-precise model checking. Unlike fuzzing, which finds bugs, Kani proves that specific properties hold for all possible inputs within a bounded state space. It integrates with Bolero for harness reuse — the same test harness can run as a fuzz target and a Kani proof.

### 5.2 Tier 2: Phase 5+ Adoption

Tools that require moderate infrastructure (modified QEMU builds, hardware features, custom instrumentation).

| Tool | Purpose | Target | Key Capability |
|---|---|---|---|
| **LibAFL** | Modular fuzzing framework (no_std) | Custom QEMU-based kernel fuzzer | Only major fuzzer with explicit `no_std` support; Rust-native; 120k exec/sec demonstrated |
| **Snapchange** | Snapshot-based QEMU fuzzing | Boot-phase-targeted fuzzing | Snapshot AIOS at specific boot phases; fuzz from consistent state; eliminates boot overhead |
| **FourFuzz** | Selective unsafe instrumentation | Kernel unsafe blocks | Instruments only ~20% of functions (those reaching unsafe); 15% more unsafe locations triggered |
| **ARM MTE** (QEMU) | Hardware memory tagging emulation | All heap allocations | 4-bit tag per 16-byte granule; instant fault on tag mismatch; QEMU emulation available |

**LibAFL** (Fioraldi et al., CCS 2022) is the strongest candidate for building a custom AIOS fuzzer. Its modular architecture allows composing fuzzer components (mutators, schedulers, observers, feedback) independently. The `no_std` support means LibAFL could theoretically be embedded into the AIOS kernel itself for in-kernel fuzzing, or used as the framework for a host-side QEMU-based fuzzer.

**Snapchange** (AWS, 2023+) snapshots QEMU VM state and restores it thousands of times per second, eliminating the cost of rebooting the kernel for each fuzz iteration. Written in Rust, it requires minimal target modification. For AIOS, snapshots can be taken at key boot milestones (post-memory-init, post-scheduler-init, post-IPC-init) to fuzz specific subsystems without replaying the full boot sequence.

**FourFuzz** (EASE 2025) selectively instruments only functions containing or reaching `unsafe` code, reducing instrumentation overhead to ~20% of functions while focusing coverage feedback on the most critical paths. This is ideal for AIOS, where ~90% of kernel code is safe Rust and the critical bugs live in the ~10% of `unsafe` blocks handling MMIO, page tables, and context switching.

### 5.3 Tier 3: Phase 10+ Custom Infrastructure

Tools that require significant custom development or specialized hardware.

| Tool | Purpose | Target | Effort |
|---|---|---|---|
| **Custom syscall fuzzer** | AIOS-specific grammar-based fuzzer | All 31 syscalls with state dependencies | High — builds on LibAFL + KernelGPT specs |
| **OZZ-style barrier verifier** | ARM memory ordering correctness | DSB/DMB/ISB placement in kernel code | High — adapts QEMU to ARM memory model |
| **ARM CoreSight ETM** | Hardware execution tracing | Zero-overhead coverage on real hardware | Very High — requires CoreSight-capable board (Pi 5+) |
| **Continuous fuzzing farm** | 24/7 corpus evolution | All fuzz targets | High — dedicated CI runners, crash filing, regression generation |

**Custom syscall fuzzer.** Built on LibAFL, using KernelGPT-generated specifications ([fuzzing-adoption-roadmap.md](fuzzing-adoption-roadmap.md) §4.2.1) for all 31 syscalls. Incorporates MOCK-style context-aware mutations and T-Scheduler MAB seed scheduling. Runs in-guest with coverage feedback via shared memory.

**ARM CoreSight ETM** (Embedded Trace Macrocell) provides hardware-level execution tracing on ARM processors — the aarch64 equivalent of Intel PT. CoreSight captures branch decisions with near-zero runtime overhead, enabling coverage-guided fuzzing without instrumentation. QEMU does not emulate CoreSight; this targets real hardware (Cortex-A72+ boards, Raspberry Pi 5). Tools: Stalker (AFL-based with CoreSight), ARMOR (IEEE TIFS 2024).

### 5.4 Corpus Management

Fuzz corpora will live under `fuzz/corpus/<target_name>/` (created when first fuzz targets are added). Interesting inputs are committed; crash-triggering inputs become regression tests under `fuzz/regressions/`. The target CI pipeline runs `cargo fuzz run <target> -- -max_total_time=3600` nightly.

**Crash triage.** Crashes are deduplicated by stack trace. Each unique crash is filed with: the crashing input, the stack trace, the git commit, and the reproducer command. Crashes that involve `unsafe` blocks are treated as P0 (highest priority).

**Corpus evolution.** The corpus grows over time as the fuzzer discovers inputs that reach new code paths. Periodic minimization (`cargo fuzz cmin`) removes redundant inputs. Coverage plateaus signal the need for new harnesses or mutation strategies — this is where AI-guided approaches ([fuzzing-ai-native.md](fuzzing-ai-native.md) §7.1) provide the most value.

---

## 6. Fuzz Target Catalog

The table below lists concrete fuzz targets, the phase at which they become available, and the invariant that the fuzzer checks.

| Target | Subsystem | Phase | Input type | Invariant |
|---|---|---|---|---|
| `fuzz_bootinfo_parse` | shared | 0 | `[u8; N]` | No panic on any input; invalid magic rejected |
| `fuzz_physaddr_new` | shared | 0 | `u64` | Misaligned/out-of-range values rejected |
| `fuzz_buddy_alloc` | memory | 2 | `Vec<(AllocOp, usize)>` | No double-free; all freed memory returned to pool; poison fill verified |
| `fuzz_slab_alloc` | memory | 2 | `Vec<(AllocOp, usize)>` | Object sizes match slab class; no overlap; red zone integrity |
| `fuzz_ring_buffer` | shared | 0 | `Vec<(PushPop, T)>` | Capacity respected; FIFO ordering; no data loss |
| `fuzz_syscall_*` | syscall | 3 | `(u64, u64, u64, u64, u64, u64)` | No kernel panic; no capability leak; error returned |
| `fuzz_ipc_send` | IPC | 3 | `(ChannelId, &[u8], Option<CapHandle>)` | No panic; oversized rejected; invalid cap rejected |
| `fuzz_ipc_receive` | IPC | 3 | `(ChannelId, &mut [u8])` | No panic; buffer bounds respected |
| `fuzz_ipc_select` | IPC | 3 | `(Vec<SelectEntry>, Timeout)` | No panic; MAX_SELECT_ENTRIES enforced; timeout respected |
| `fuzz_elf_load_stub` | uefi-stub | 1 | `&[u8]` (ELF binary) | No panic; malformed ELF returns error; PT_LOAD bounds checked |
| `fuzz_elf_load_kernel` | loader | 3 | `&[u8]` (ELF binary) | No panic; malformed ELF returns error; no mapping of invalid segments |
| `fuzz_manifest_parse` | agent | 10 | `&str` (TOML) | No panic; invalid schema rejected; cycles detected |
| `fuzz_cap_attenuate` | capability | 3 | `(CapHandle, AttenuationMask)` | Result is subset of source; invalid handles rejected |
| `fuzz_cap_cascade` | capability | 3 | `Vec<(CapOp, CapHandle)>` | Revoke cascades to all children; no orphaned tokens |
| `fuzz_packet_parse` | network | 7 | `&[u8]` (raw packet) | No panic; malformed packets dropped |
| `fuzz_compositor_event` | compositor | 6 | `(SurfaceId, EventType, Coords)` | No panic; out-of-range clamped; invalid surface rejected |
| `fuzz_virtio_mmio_probe` | drivers | 4 | `[u8; 512]` (MMIO slot) | Bad magic/device ID rejected; no hang on invalid response |
| `fuzz_superblock_parse` | storage | 4 | `[u8; 4096]` | Bad magic/version/checksum returns error; no panic |
| `fuzz_wal_replay` | storage | 4 | `Vec<[u8; 64]>` (WAL entries) | Corrupted entries skipped; valid entries applied; no panic |
| `fuzz_block_read` | storage | 4 | `(BlockId, [u8; 512])` | CRC-32C mismatch returns error; no corrupt data returned |
| `fuzz_memtable_ops` | storage | 4 | `Vec<(Op, ContentHash, BlockLocation)>` | Capacity enforced; binary search correct; refcount consistent |
| `fuzz_content_hash` | storage | 4 | `&[u8]` | SHA-256 consistent; no panic on empty/large input |
