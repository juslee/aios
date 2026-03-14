# Fuzzing: Hardening Strategies

Part of: [fuzzing.md](../fuzzing.md) — Fuzzing and Input Hardening
**Related:** [adoption-roadmap.md](./adoption-roadmap.md) — Phased adoption, [tooling.md](./tooling.md) — Tooling and catalog, [ai-native.md](./ai-native.md) — AI-native strategies

---

## 3. Hardening Strategies

AIOS employs defense in depth: multiple layers of hardening so that a bypass at one layer does not compromise the system. Each layer below addresses a class of bugs that fuzzing commonly discovers.

### 3.1 Language-Level Safety (Rust)

Rust's ownership model eliminates the three most common vulnerability classes found by fuzzers in C/C++ kernels:

| Vulnerability class | C/C++ frequency | Rust mitigation |
|---|---|---|
| Buffer overflow | ~35% of kernel CVEs | Bounds-checked indexing in safe Rust; slices carry length |
| Use-after-free | ~20% of kernel CVEs | Ownership + borrow checker prevents dangling references |
| Uninitialized memory | ~10% of kernel CVEs | All variables must be initialized; `MaybeUninit` explicit |

**What remains.** Rust's safety guarantees do not extend into `unsafe` blocks, which AIOS requires for:

- MMIO register access (`read_volatile` / `write_volatile`)
- Inline assembly (boot code, exception vectors, system register access)
- Raw pointer manipulation (page table walks, physical-to-virtual conversions)
- FFI boundaries (if any external code is linked)

Every `unsafe` block in AIOS follows the documentation standard defined in `CLAUDE.md`: a `// SAFETY:` comment stating the invariant, who maintains it, and what happens if violated. These blocks are the primary audit surface and the highest-priority fuzz targets.

**Rust kernel fuzzing is still essential.** Research confirms that Rust kernel code is not immune to bugs — Check Point Research (2025) found vulnerabilities in Windows kernel Rust components through targeted fuzzing. The `unsafe` blocks required for hardware interaction, plus logic errors in safe code (integer overflow, infinite loops, deadlocks), make fuzzing indispensable even in a Rust kernel.

### 3.2 Syscall Validation

Every syscall validates all parameters at the kernel entry point before any kernel state is accessed or modified. The validation sequence is:

1. **Syscall number**: reject if not in `[0, SYSCALL_COUNT)` (currently 31 syscalls, defined in `shared/src/syscall.rs`)
2. **Capability handles**: bounds-check against process capability table; verify generation counter matches (prevents use-after-revoke)
3. **Pointer arguments**: must fall within user address range (below `USER_VA_LIMIT = 0x0000_8000_0000_0000`); must be aligned to the expected type; must be backed by a mapped, readable (or writable) page
4. **Length arguments**: reject `0` and values exceeding per-syscall maximums; reject `length > buffer_mapping_size`
5. **Enum/flag arguments**: reject values outside the valid set; no "reserved for future use" bits accepted

No syscall implementation trusts any user-supplied value without validation. This is enforced by code review (see [security.md](model.md) §8.1 agent audit tool) and by syscall fuzzing ([adoption-roadmap.md](adoption-roadmap.md) §4.2).

### 3.3 Memory Hardening

| Mechanism | What it prevents | Phase |
|---|---|---|
| **W^X enforcement** | Code injection via writable+executable pages | 1+ |
| **Guard pages** | Stack overflow overwriting adjacent memory | 1+ |
| **Buddy allocator poisoning** | Use-after-free (freed pages filled with `0xDEAD_DEAD`) | 2+ |
| **Double-free detection** | Freeing already-free pages (bitmap/buddy tree check) | 2+ |
| **Slab red zones** | Buffer overflow within slab objects (8-byte zones, pattern `0xFEFE_FEFE_FEFE_FEFE`) | 2+ |
| **KASLR** | Predictable kernel addresses for exploit chaining | 1+ |
| **PAC** (Pointer Authentication) | Return address overwrite, ROP chains | 13+ |
| **BTI** (Branch Target Identification) | JOP (Jump-Oriented Programming) attacks | 13+ |
| **MTE** (Memory Tagging Extension) | Spatial and temporal memory errors in `unsafe` blocks | 13+ |

PAC, BTI, and MTE are aarch64 hardware features enabled in Phase 13 (Security Hardening). MTE is particularly valuable for fuzzing: when enabled, the hardware tags every 16-byte granule and checks tags on every access, catching use-after-free and out-of-bounds access at the exact instruction — not just when a crash eventually occurs.

**MTE caveat.** TIKTAG research (2024) demonstrated that MTE tags can be leaked via speculative execution side channels. This affects MTE's security guarantees against sophisticated attackers but does not diminish its value as a fuzzing accelerator — during testing, the attacker model is the fuzzer itself, not a speculative side channel.

### 3.4 IPC Hardening

IPC is the primary inter-process communication mechanism and a high-value fuzz target because every system service receives messages from potentially untrusted agents.

- **Message size**: enforced at kernel level before copying into receiver buffer. Messages exceeding `MAX_MESSAGE_SIZE` (256 bytes) are rejected with an error, never truncated.
- **Type safety**: messages carry a type tag verified against the channel's declared protocol. Type mismatches are rejected.
- **Capability transfer**: when a message carries a capability, the kernel validates that the sender actually holds the capability and that the transfer is permitted by the capability's attenuation rules.
- **No raw pointers in messages**: IPC payloads are copied between address spaces. Pointers in message bodies have no meaning in the receiver's address space and are never interpreted as addresses.

Cross-reference: [security.md](model.md) §§2–3 (IPC security architecture and capability system).

### 3.5 Device Driver Hardening

Device drivers read data from hardware via MMIO, which is inherently untrusted (a misbehaving device can return any value).

- **Timeout on busy-wait**: every loop that polls a device status register has a maximum iteration count. If the device does not respond within the timeout, the driver returns an error instead of hanging the kernel.
- **Bounds-check on reads**: values read from device registers are validated before use (e.g., a DMA descriptor length is checked against the expected maximum before allocating a buffer).
- **DMA buffer isolation**: when DMA is introduced (Phase 7+), DMA buffers are allocated from a dedicated pool with guard pages. IOMMU is used (where available) to restrict device access to only the intended buffers.

**VirtIO-blk hardening (Phase 4+).** The VirtIO block driver validates MMIO magic values (`0x74726976`) during device probe — invalid magic causes the probe to skip the device rather than panic. Polling loops for virtqueue completion use configurable iteration-count timeouts; exhaustion returns `StorageError` rather than blocking indefinitely. Device-reported capacity is sanity-checked before use. The polled I/O path validates descriptor status bytes and rejects unexpected values.

### 3.6 Manifest and Agent Hardening

Agent manifests are parsed during installation and define the agent's requested capabilities, dependencies, and metadata. Since manifests come from external sources (developers, app stores), they are fully untrusted.

- **Schema validation**: the manifest parser validates the document structure before accessing any field. Missing required fields, unexpected types, and unknown keys are rejected.
- **Circular delegation detection**: the capability delegation graph is checked for cycles before approval. An agent cannot grant itself capabilities through a circular chain.
- **Signature verification**: manifests must be signed by a certificate that chains to the AIOS Root CA. Invalid, expired, or revoked signatures are rejected before any capability is granted.

Cross-reference: [security.md](model.md) §8.1 (agent audit tool).

### 3.7 Concurrency Hardening

Concurrency bugs are the dominant risk class in kernel `unsafe` code once multi-core support and scheduling are active. ARM's relaxed memory model makes these bugs harder to reproduce and harder to detect than on x86, where stronger ordering masks many races.

#### 3.7.1 ARM Memory Ordering Verification

ARM aarch64 permits out-of-order execution and store reordering that x86 prohibits. Missing or incorrect memory barriers (`DSB`, `DMB`, `ISB`) can cause:

- **Stale reads**: a core sees an old value after another core has updated it
- **Reordered stores**: initialization writes become visible after the "ready" flag, causing use of uninitialized data
- **Exclusive monitor failures**: atomic read-modify-write operations (`ldaxr`/`stlxr`) require Inner Shareable + Cacheable memory for the global exclusive monitor

AIOS has direct experience with this class of bug: the Phase 1 identity map uses Non-Cacheable Normal memory (edk2 MAIR Attr1), where `spin::Mutex` and `compare_exchange` hang under multi-core contention because the global exclusive monitor is not functional. The mitigation — using only `load(Acquire)`/`store(Release)` for inter-core synchronization until Phase 2 upgrades to Write-Back cacheable attributes — is a critical invariant that must be verified.

**Verification approach.** OZZ (SOSP 2024 Best Paper) demonstrates that out-of-order concurrency bugs can be detected by emulating memory access reordering during fuzzing. Adapting this technique to AIOS's ARM-specific barrier requirements — verifying that every shared-data access is correctly fenced — is a high-priority fuzzing target.

Cross-reference: [static-analysis.md](static-analysis.md) §4.8 (Converos concurrency model checking).

#### 3.7.2 Atomic Protocol Verification

AIOS uses atomics extensively for lock-free coordination:

| Atomic variable | Type | Protocol |
|---|---|---|
| `TICK_COUNT` | `AtomicU64` | Monotonically increasing, incremented by timer IRQ handler |
| `NEED_RESCHED` | `AtomicBool` | Set by timer tick, cleared by scheduler |
| `IN_SCHEDULER` | Per-CPU `AtomicBool` | Guards re-entrant `schedule()` from timer tick |
| `PRINT_TURN` | `AtomicUsize` | Turn-based protocol: core N waits for `load == N`, prints, then `store(N+1)` |
| Slab magazine | `current`/`prev` | Two-chance swap: exhaust current → swap with prev → allocate new |

Each protocol has implicit invariants (monotonicity, mutual exclusion, progress) that must hold under all possible interleavings. **Loom** (tokio-rs) provides exhaustive state-space exploration for these protocols by permuting all possible concurrent executions under the C11 memory model.

**Limitation.** Loom models C11 atomics, not ARM's weaker memory ordering. A protocol that is correct under C11 may still be incorrect on ARM if it relies on ordering guarantees that C11 provides but ARM does not. Loom verification should be complemented by ARM-specific barrier analysis.

#### 3.7.3 Controlled Scheduling for Race Detection

Traditional fuzzing explores input space but not scheduling space — the same syscall sequence may be bug-free under one thread interleaving and crash under another. Controlled concurrency testing addresses this by deterministically exploring thread schedules.

**LACE** (2025) demonstrates eBPF-powered controlled scheduling with 11.4x speedup in bug exposure and 38% more branches covered. While AIOS cannot use eBPF directly (it is a Linux mechanism), the core technique — serializing non-deterministic execution and mutating both input and schedule — is applicable through QEMU's SMP emulation, which provides deterministic core scheduling when configured appropriately.

**AIOS-specific targets for controlled scheduling:**

- Priority inheritance chains: verify that inheritance bounded to `MAX_INHERITANCE_DEPTH=8` terminates correctly under all interleavings
- IPC direct switch: verify that the scheduler bypass path (sender → receiver context switch without entering `schedule()`) does not violate run-queue invariants
- Lock ordering: verify that the documented lock order (PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE > CHANNEL_TABLE > SELECT_WAITERS > BLOCK_ENGINE > VIRTIO_BLK) is respected under concurrent access patterns
- Load balancer: verify that thread migration between per-CPU run queues does not lose or duplicate threads
