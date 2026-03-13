# Fuzzing: Adoption Roadmap

Part of: [fuzzing-and-hardening.md](./fuzzing-and-hardening.md) — Fuzzing and Input Hardening
**Related:** [fuzzing-hardening-strategies.md](./fuzzing-hardening-strategies.md) — Hardening strategies, [fuzzing-tooling.md](./fuzzing-tooling.md) — Tooling and catalog, [fuzzing-ai-native.md](./fuzzing-ai-native.md) — AI-native strategies

---

## 4. Fuzzing Adoption Roadmap

AIOS adopts fuzzing incrementally, aligned with the phase at which each subsystem becomes available. The goal is to fuzz every input boundary as soon as it exists, not to defer all fuzzing to a single hardening phase.

### 4.1 Phases 0–2: Host-Side Fuzzing

Before the kernel has a syscall interface, fuzzing targets the `shared/` crate and kernel data structures that can be tested on the host.

**Targets:**

- `BootInfo` deserialization — fuzz the struct validation logic with arbitrary byte sequences
- `PhysAddr` / `VirtAddr` construction — fuzz alignment and range checks
- Buddy allocator — fuzz allocation/deallocation sequences for invariant violations (no double-free, correct coalescing, poison fill on free)
- Slab allocator — fuzz object alloc/free sequences (size class matching, red zone integrity, magazine layer swap)
- Ring buffer and fixed queue — fuzz push/pop sequences for capacity and ordering invariants

**Tools:**

- `cargo-fuzz` with `libFuzzer` — runs on the host (not in QEMU)
- `proptest` for property-based testing — generates structured random inputs and checks invariants (e.g., "any sequence of buddy alloc/free operations leaves the allocator in a consistent state")
- `Loom` for atomic protocol verification — exhaustive interleaving exploration for `TICK_COUNT`, `NEED_RESCHED`, boot phase state machine

**Integration:** Add fuzz targets to CI as nightly jobs. Corpus is committed to the repository under `fuzz/corpus/`.

### 4.2 Phases 3–5: Syscall and IPC Fuzzing

Once the syscall interface exists (Phase 3), kernel fuzzing begins in earnest.

**Approach.** A custom fuzzer runs inside a QEMU guest, invokes syscalls with randomized parameters, and reports crashes via the UART. The host-side harness monitors QEMU for:

- Kernel panics (panic handler prints to UART)
- Hangs (no UART output within timeout)
- Memory corruption (detected by allocator poisoning or MTE when available)

**Coverage guidance.** The fuzzer instruments the kernel binary (via LLVM SanitizerCoverage or equivalent) to track which syscall paths are exercised, evolving inputs toward uncovered code.

**Scope.** All 31 syscalls (IPC 0–9, Notify 10–12, Stats 13, Cap 14–17, Mem 18–22, Proc 23–25, Time 26–28, Audit 29, Debug 30), all parameter combinations. The fuzzer generates both valid and invalid inputs — valid inputs exercise normal paths, invalid inputs exercise error-handling paths.

#### 4.2.1 Stateful Syscall Fuzzing

Random syscall parameters miss multi-step state dependencies. For example, `ipc_send` requires a valid `channel_id` from a prior `channel_create`; `cap_grant` requires an existing capability. Stateful fuzzing builds valid kernel state through dependent syscall sequences.

**N-gram dependency mining.** Psyzkaller (2024) mines syscall dependency relations from execution traces using N-gram models, discovering that certain syscall pairs (e.g., `channel_create` → `ipc_send`) dramatically increase coverage. Applied to AIOS's 31-syscall interface, N-gram analysis learns dependencies such as:

- `ChannelCreate` → `IpcSend`/`IpcRecv`/`IpcCall`/`IpcReply` (channel lifecycle)
- `CapGrant` → `IpcCall` with granted capability (capability-gated IPC)
- `ShmemCreate` → `ShmemMap` → `ShmemShare` (shared memory lifecycle)
- `NotificationCreate` → `NotificationSignal`/`NotificationWait` (notification lifecycle)
- `ProcessCreate` → `ProcessWait`/`ProcessExit` (process lifecycle)

**Automated specification generation.** KernelGPT (ASPLOS 2025) uses LLMs to auto-generate syscall fuzzer specifications from kernel source code, achieving 24 new bugs in Linux with 11 CVEs. Applied to AIOS, an LLM analyzes `kernel/src/syscall/mod.rs` and `shared/src/syscall.rs` to generate descriptions for all 31 syscalls — argument types, valid ranges, state preconditions, and expected error returns. This eliminates manual specification writing.

**Adaptive seed scheduling.** T-Scheduler (AsiaCCS 2024) applies multi-armed bandit (MAB) algorithms to seed selection, dynamically prioritizing syscall sequences that discover new coverage. Unlike fixed-priority schedulers, T-Scheduler requires no hyperparameter tuning — it automatically adapts to the target kernel's coverage characteristics. This is a drop-in improvement for any AIOS syscall fuzzer.

**IPC protocol fuzzing.** ChatAFL (NDSS 2024) uses LLMs to extract protocol grammars and generate state-transition-covering message sequences. Applied to AIOS IPC, this technique treats the IPC channel as a protocol state machine:

```text
channel_create → [open]
  → ipc_send → [message_pending]
  → ipc_recv → [message_delivered]
  → ipc_call → [reply_pending] → ipc_reply → [call_complete]
  → ipc_cancel → [cancelled]
  → channel_destroy → [closed]
```

The LLM generates message sequences that cover all state transitions, including error paths (send to closed channel, reply without pending call, double-destroy).

**Context-aware mutation.** MOCK (NDSS 2024) learns syscall dependencies and adapts mutations based on execution context, achieving 32% more branch coverage and 50% more interrelated sequences than context-unaware fuzzing. This models AIOS's capability system naturally: `cap_create` → `cap_grant` → `ipc_call` (with granted cap) → `cap_revoke`.

### 4.3 Phase 4: Storage Fuzzing

The storage subsystem introduces binary-format fuzz surfaces with on-disk persistence.

**Block Engine targets:**

- **Superblock parsing**: fuzz with corrupted magic (`SUPERBLOCK_MAGIC = 0x41494F53_50414345`), invalid version numbers, bad CRC-32C checksums. The kernel must reject invalid superblocks with `StorageError`, never panic or proceed with corrupt metadata.
- **WAL replay**: fuzz with corrupted WAL entries (64-byte `repr(C)` structs), truncated WAL regions, circular buffer boundary conditions. Invalid entries must be skipped during replay; valid entries must be applied correctly.
- **Content integrity**: fuzz with CRC-32C mismatches between stored checksum and data payload. The block engine must detect corruption on read and return an error, never serve corrupt data.
- **MemTable operations**: fuzz with capacity boundary conditions (65536 entries), binary search edge cases (duplicate keys, empty table), and refcount sequences (insert/insert/remove must leave refcount=1, not underflow).

**VirtIO-blk targets:**

- **MMIO probe**: fuzz with invalid magic values, unexpected device IDs, malformed feature bits during device initialization.
- **Capacity extremes**: test with device-reported capacity of 0, 1, and `u64::MAX` sectors.
- **Poll timeout**: verify that the polling loop returns `StorageError` after timeout exhaustion rather than looping indefinitely.

**Content hashing:** SHA-256 content addressing is deterministic and should produce consistent hashes for identical input. Fuzz with empty inputs, single-byte inputs, and inputs near the block size boundary to verify hash consistency and no-panic behavior.

### 4.4 Phases 6–9: Compositor and Network Fuzzing

When the compositor (Phase 6) and network stack (Phase 7) arrive, new binary-protocol fuzz surfaces open:

- **Compositor input events** (Phase 6): fuzz with out-of-range coordinates, invalid surface IDs, and rapid event floods. The compositor must clamp or reject, never crash.
- **Network packet parser**: fuzz with malformed Ethernet frames, truncated IP/TCP/UDP headers, oversized payloads, invalid checksums. The stack must drop bad packets silently — no buffer overflow, no kernel panic.

These targets are mutation-based: start with valid captured packets or event sequences, then mutate bytes, truncate, and inject garbage. `cargo-fuzz` on host-side parsing logic; QEMU-based fuzzing for the full kernel path.

### 4.5 Phases 10–12: Agent and Manifest Fuzzing

When the agent framework is introduced (Phase 10), three new fuzz surfaces appear:

- **Manifest parser**: fuzz with malformed TOML, truncated files, oversized fields, invalid UTF-8
- **IPC message bodies**: fuzz with truncated payloads, wrong type tags, oversized messages, capability references to non-existent handles
- **Agent capability requests**: fuzz the capability approval flow with invalid delegation chains, circular references, expired tokens

These are grammar-based fuzzing targets — the fuzzer generates inputs from the manifest schema and IPC protocol definitions, then mutates them to exercise error paths.

### 4.6 Phase 13+: Full Fuzzing Campaign

Phase 13 (Security Hardening) enables hardware security features that make fuzzing dramatically more effective:

- **MTE-enabled fuzzing**: every heap allocation gets a 4-bit tag at 16-byte granularity. Accessing freed memory or overflowing into an adjacent allocation causes an immediate synchronous exception — not a silent corruption that manifests later. This turns temporal and spatial memory errors from "hard to detect" to "instantly caught." QEMU supports MTE emulation for testing before hardware availability.
- **PAC-enabled builds**: corrupted return addresses are detected at function return, catching control-flow hijacking.
- **Continuous fuzzing in CI**: the fuzzer runs 24/7 on a dedicated CI runner. New crashes are filed automatically. Regression tests are generated from crash inputs and added to the test suite.
- **Corpus management**: the fuzzing corpus is stored in the repository and shared across CI runs. Interesting inputs (those that found new coverage) are kept; redundant inputs are minimized.

### 4.7 Phase 24: Formal Verification Complements Fuzzing

Fuzzing finds bugs but cannot prove their absence. Formal verification provides mathematical guarantees for the most critical subsystems:

| Target | Method | Property |
|---|---|---|
| Capability system | TLA+ model (Phase 13) → Coq proofs (Phase 24) | No forge, no escalation |
| IPC | TLA+ model | No cross-address-space memory leak |
| Provenance chain | Coq proofs | Append-only, tamper-evident |
| W^X enforcement | Exhaustive path analysis | No page is ever both writable and executable |

Fuzzing and formal verification are complementary: fuzzing catches implementation bugs in code that is too complex to verify formally, while verification proves that the design of critical subsystems is correct regardless of input.

Cross-reference: [security.md](security.md) §8.3 (formal verification targets), [static-analysis.md](static-analysis.md) §4.5–4.7 (Kani, TLA+, Coq).
