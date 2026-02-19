# AIOS: Lessons Learned & Design Retrospective
## Self-Audit Reference Document

**Version:** 2.0
**Scope:** Architecture design and documentation audit (Phases 0–27)
**Last Updated:** 2026-02-19

---

## Overview

This document records lessons learned during the AIOS architecture design process. Its purpose is threefold: (1) prevent repeating mistakes during implementation, (2) provide a checklist for future design reviews, and (3) capture reusable patterns worth carrying forward. Lessons are organized into **strengths to preserve**, **anti-patterns to avoid**, **reusable patterns**, and **resolved open questions** (all 10 questions from the audit have been answered by cross-referencing existing documentation).

---

## Quick Reference

| # | Lesson | Category | Impact | Relevant Docs |
|---|--------|----------|--------|---------------|
| S1 | Spaces abstraction eliminates filesystem problems | Strength | High | [spaces.md](../storage/spaces.md) |
| S2 | Subsystem Framework provides genuine reuse | Strength | High | [subsystem-framework.md](../platform/subsystem-framework.md) |
| S3 | AI as infrastructure, not application | Strength | High | [airs.md](../intelligence/airs.md) |
| S4 | Graceful degradation is explicit, not accidental | Strength | High | [architecture.md](./architecture.md) §6 |
| S5 | Capability system is kernel-enforced | Strength | High | [security.md](../security/security.md) |
| S6 | Honest about tradeoffs and constraints | Strength | Medium | Throughout |
| A1 | OS metaphors can mislead | Anti-pattern | Medium | [architecture.md](./architecture.md) |
| A2 | Premature generalization for future hardware | Anti-pattern | Medium | [airs.md](../intelligence/airs.md) §4.2 |
| A3 | Terminology overload across documents | Anti-pattern | High | Throughout |
| A4 | Monolithic AIRS couples unrelated concerns | Anti-pattern | High | [airs.md](../intelligence/airs.md) |
| A5 | Performance targets without justification | Anti-pattern | Medium | [architecture.md](./architecture.md) §6.9 |
| A6 | Novel mechanisms underspecified | Anti-pattern | High | [security.md](../security/security.md), [context-engine.md](../intelligence/context-engine.md) |
| R1 | Bounded resource consumption | Reusable | High | [scheduler.md](../kernel/scheduler.md) |
| R2 | Device profiles as percentages | Reusable | Medium | [architecture.md](./architecture.md) §9 |
| R3 | Content-addressed storage with Merkle DAG versioning | Reusable | High | [spaces.md](../storage/spaces.md) |
| R4 | Multi-language SDK via uniform syscall abstraction | Reusable | Medium | [architecture.md](./architecture.md) §8 |

---

## 1. Strengths to Preserve

### S1. Spaces Replace Filesystems — and It Works

Objects instead of files. Semantic relationships instead of directory trees. Content-addressed storage (SHA-256) provides automatic deduplication. Versions are first-class via Merkle DAG (like git).

**Why this matters:** Eliminates entire categories of problems — file naming confusion, lost data, path escaping, manual organization. Users find things by meaning, not by remembering where they put them.

**Preserve during implementation:**
- Keep the `Space` and `Object` structs canonical — resist adding filesystem-like fields (e.g., `path`, `extension`)
- Content-addressing must be the storage primitive, not an optimization added later
- The semantic index is infrastructure, not a feature toggle

**Related:** [spaces.md](../storage/spaces.md), [architecture.md](./architecture.md) §2.3

---

### S2. One Framework, Every Subsystem

Every hardware subsystem — network, audio, USB, display, camera, Bluetooth, print — implements the same traits: capability gate, sessions, data channels, audit, power management, POSIX bridge, hotplug.

**Why this matters:** Adding the tenth subsystem is formulaic, not architectural. The framework handles cross-cutting concerns once. This is genuine abstraction reuse, not copy-paste.

**Preserve during implementation:**
- Define the `SubsystemTrait` and its required methods in Phase 4, before the first concrete subsystem
- Resist per-subsystem "special" interfaces that bypass the framework
- The POSIX bridge pattern (translating `/dev` nodes to subsystem sessions) should be tested with at least two subsystems before being declared stable

**Related:** [subsystem-framework.md](../platform/subsystem-framework.md)

---

### S3. AI as Infrastructure, Not Application

AIRS is loaded early at boot. The kernel has inference scheduling primitives. Every subsystem can request AI services (semantic search, intent verification, behavioral monitoring) through system calls, not application APIs.

**Why this matters:** This is what makes AIOS different from "Linux with an AI app." When AI is infrastructure, it can enhance search, security, attention management, and context detection system-wide. When it's an application, each app reinvents these capabilities.

**Preserve during implementation:**
- AIRS must be a boot-critical service, not a user-startable daemon
- Inference scheduling must be kernel-aware (deadline, priority, preemption) — not just "run a thread"
- Every AIRS-dependent feature must have a fallback (see S4)

**Related:** [airs.md](../intelligence/airs.md), [boot.md](../kernel/boot.md)

---

### S4. Graceful Degradation Is Explicit

Every AI-dependent feature has a defined fallback path:

| Feature | With AIRS | Without AIRS |
|---------|-----------|--------------|
| Search | Semantic similarity | Keyword match |
| Intent verification | AI comparison against goal | Skipped (capability check only) |
| Behavioral monitoring | Anomaly detection | Rate limits only |
| Context engine | Multi-signal inference | Time-of-day heuristic |
| Attention management | AI triage | Rule-based priority |

**Why this matters:** AIRS will crash. Models will fail to load. Inference will be slow on constrained hardware. The system must remain usable. This isn't a nice-to-have — it's the difference between an OS and a demo.

**Preserve during implementation:**
- Write the fallback path *first*, before the AI-enhanced path
- Test fallbacks by intentionally killing AIRS mid-operation
- The fallback path is the baseline that must always work; the AI path is the enhancement

**Related:** [architecture.md](./architecture.md) §6

---

### S5. Capabilities Over Permissions

Capabilities are unforgeable kernel-managed tokens. Fine-grained (per-space, per-device, per-service). Revocable. Expiring. The kernel enforces them — not userspace conventions.

**Why this matters:** Prevents confused deputy attacks. A compromised agent can't escalate privileges because capability tokens can't be forged. Browser tab isolation is kernel-enforced via TTBR0, not browser policy. This is security by construction, not by convention.

**Preserve during implementation:**
- Capability tokens must live in kernel memory, never in agent-accessible address space
- `CapabilityTransfer` must be a first-class IPC operation, not a serialization hack
- Every syscall must check capabilities before acting — no "trusted" services that skip checks

**Related:** [security.md](../security/security.md), [ipc.md](../kernel/ipc.md)

---

### S6. Honest About Tradeoffs

The architecture explicitly states constraints as constraints, not design choices:
- AIRS is monolithic because 4–8 GB hardware can't afford multiple processes each holding model copies
- IPC must be < 5 microseconds because every syscall is an IPC round-trip in a microkernel
- The scheduler optimizes for latency, not throughput, because this is an interactive OS

**Why this matters:** Honest constraint documentation prevents future developers from "fixing" something that was a deliberate tradeoff. It also makes it clear when a constraint is lifted (e.g., on 32 GB hardware, AIRS can split).

**Preserve during implementation:**
- When making a tradeoff, document it where the code lives, not just in architecture docs
- Use `// TRADEOFF:` comments in code to link to the relevant doc section
- Revisit constraints when hardware targets change

---

## 2. Anti-Patterns to Avoid

### A1. OS Metaphors Can Mislead

**Problem:** The docs use "kernel," "process," and "filesystem" extensively. But AIOS's kernel is radically different from Unix — it's an IPC broker + capability manager + scheduler with no filesystem, no device drivers, and no dynamic kernel object allocation. Developers from Unix/Linux backgrounds will make incorrect assumptions.

**Specific examples:**
- "Kernel" implies memory management, device drivers, filesystem — AIOS's kernel does none of these
- "Tasks" vs "Processes" are conflated — a Task is a user-intent + agents + context, not a 1:1 process mapping
- "Spaces" sounds like a filesystem abstraction — they're actually semantic object databases

**Resolution:**
- In developer-facing documentation and SDK docs, explicitly state: "If you're coming from Linux, here's what's different"
- Consider a terminology glossary at the top of `architecture.md`
- In code, use precise types (`KernelIpcBroker`, not `Kernel`) so the name communicates what it does

---

### A2. Premature Generalization for Future Hardware

**Problem:** The architecture designs for phones, tablets, TVs, and SBCs — none of which will run AIOS for 2+ years. Device profiles, model eviction policies, storage budgets with percentage-based quotas, and adaptive memory pressure systems add complexity to early phases.

**Specific examples:**
- Device profile system (`architecture.md` §9) tunes for RAM/storage/compute across 5 tiers — but Phase 0–15 targets only QEMU and Pi
- AIRS model eviction and LRU caching (`airs.md` §4.2) is designed for phones with 2 GB RAM — initial target is 4–8 GB laptops
- Storage budgets (20% models, 30% version history) are premature for 256 GB+ hardware

**Resolution:**
- Implement the simplest viable version for the initial target hardware (4–8 GB laptop/Pi)
- Gate generalization on actual hardware support — don't build phone-scale optimizations until Phase 27
- Mark device profile code as `#[cfg(feature = "multi_device")]` so it's compiled out of early builds
- The phase plan already defers phone support post-MVP — the architecture docs should match this discipline

---

### A3. Terminology Overload Across Documents

**Problem:** Key terms have overlapping or context-dependent meanings:

| Term | Meaning 1 | Meaning 2 | Meaning 3 |
|------|-----------|-----------|-----------|
| Agent | Executable + manifest | Running process + capabilities | The capability set itself |
| Task | User-facing intent | Decomposed subtask | Instance of scheduled work |
| Session | Device subsystem session | User login session | AIRS inference session |
| Service | Userspace daemon process | Capability-gated endpoint | Any system service |
| Space | Object collection | Mounted namespace | Security zone container |

**Impact:** Developers will misunderstand interfaces. Confusion propagates through implementation. Makes the codebase harder to reason about.

**Resolution:**
- Create a canonical glossary (this is high priority for Phase 0)
- Use Rust's type system to disambiguate: `AgentManifest`, `AgentProcess`, `AgentCapabilitySet` are three separate types, not one "Agent"
- In docs, always use the most specific term: "subsystem session" not "session," "inference session" not "session"
- Audit all cross-references during implementation to catch conflation

---

### A4. Monolithic AIRS Couples Unrelated Concerns

**Problem:** AIRS contains inference engine + indexer + context engine + attention manager + intent verifier + behavioral monitor + adversarial defense + tool manager — all in one process. The justification is memory efficiency (can't duplicate model weights across processes).

**Why this is concerning:**
- A crash in the Space Indexer crashes AIRS, which degrades the entire system
- The inference engine becomes a bottleneck for all intelligence services
- Security-critical paths (intent verification) share address space with non-critical paths (indexing)
- Behavioral monitoring is monitoring itself — the monitor and the monitored share state

**Resolution:**
- Accept the monolithic design for 4–8 GB hardware, but document the split plan explicitly
- Use internal process isolation (Rust modules with clear API boundaries, no shared mutable state)
- Define the future split architecture now: which components become separate processes, what IPC they use, how model weights are shared via memory-mapped regions
- Implement crash containment within AIRS: if the indexer panics, catch it and restart that subsystem without restarting inference

---

### A5. Performance Targets Without Justification

**Problem:** Performance targets are stated without explaining why that specific number:

| Target | Stated | Question |
|--------|--------|----------|
| IPC round-trip | < 5 microseconds | Why not 50 microseconds? What breaks at 6? |
| Context switch | < 10 microseconds | Derived from what workload? |
| Semantic search | < 500 milliseconds | User-perceptible threshold? Or arbitrary? |
| First token latency | < 500 milliseconds | Based on model size? Hardware? |

**Impact:** Without justification, teams can't make informed tradeoff decisions. Is 6 microseconds IPC a disaster or fine? Is 800ms semantic search acceptable for batch operations?

**Resolution:**
- For each performance target, document: (a) what workload drives it, (b) what breaks if it's 2x slower, (c) what comparable system achieves this target
- Use decision gates in `development-plan.md` as the actual go/no-go thresholds — the architecture targets can be aspirational
- Measure first, optimize second — don't spend weeks going from 6 to 4 microseconds when the bottleneck is elsewhere

---

### A6. Novel Mechanisms Are Underspecified

**Problem:** The most innovative parts of the design have the least specification detail:

**Adversarial defense (Layer 5):**
- "Control/data plane separation prevents prompt injection from escalating"
- But: How are agent instructions delivered from the kernel? What counts as "data" vs "instruction"? How is injection detected? What's the false positive rate?

**Context Engine:**
- "Continuously infers user context from signals"
- But: How are signals weighted? What's the update frequency? What's the failure mode? How is `work_engagement: f32` calculated?

**Space Query composition:**
- `SpaceQuery` defines Filter, TextSearch, Semantic, Traverse
- But: Can queries compose (AND/OR)? What indices support which queries? What are latency characteristics?

**Resolution:**
- Prioritize specifying novel mechanisms *because* they're novel — there's no existing implementation to reference
- For adversarial defense: write a threat model with specific attack scenarios and how each is detected/prevented
- For Context Engine: define the algorithm as pseudocode, including signal weights, update frequency, and default behavior
- For Space Query: define a formal grammar and specify which indices each query type uses

---

## 3. Reusable Patterns

### R1. Bounded Resource Consumption

Agents have memory limits, CPU quotas, and storage quotas. The system proactively prevents any single agent from consuming all resources. The scheduler enforces these limits via four scheduling classes (Real-Time, Interactive, Normal, Idle).

**When to apply this pattern:** Any system where untrusted or semi-trusted code runs alongside system-critical services. The key insight is that limits should be *proactive* (enforced before exhaustion) not *reactive* (killed after OOM).

```rust
pub struct AgentLimits {
    memory_max: usize,        // hard limit, kernel-enforced
    cpu_quota: CpuQuota,      // proportional share, scheduler-enforced
    storage_quota: SpaceQuota, // per-space, storage-service-enforced
    ipc_rate: u32,            // messages per second, kernel-enforced
}
```

---

### R2. Device Profiles as Percentages

Resource budgets are expressed as percentages of available hardware, not absolute numbers. "20% of RAM for models" adapts from 2 GB to 256 GB without code changes.

**When to apply this pattern:** Any system targeting heterogeneous hardware. Avoids hardcoded thresholds that become wrong on different hardware.

**Caveat (see A2):** Don't build the full device profile system until you need it. Start with one profile for your initial target hardware. The percentage-based approach is the right *mechanism*; premature generalization across 5 tiers is the wrong *timing*.

---

### R3. Content-Addressed Storage with Merkle DAG Versioning

SHA-256 hashing of content provides automatic deduplication. Merkle DAG (like git) provides full version history. Hash mismatches detect corruption. Content-addressed blocks are immutable by definition.

**When to apply this pattern:** Any storage system where data integrity, deduplication, and version history matter. The combination of content-addressing + Merkle DAG is well-understood (git, IPFS, Nix) and provides strong guarantees with simple implementation.

```rust
pub struct StoredBlock {
    hash: [u8; 32],           // SHA-256 of content
    content: Vec<u8>,
    ref_count: u32,           // GC when zero
    parent_hashes: Vec<[u8; 32]>, // Merkle DAG links
}
```

---

### R4. Multi-Language SDK via Uniform Syscall Abstraction

The syscall interface is the same regardless of language. Rust (native), Python (popular for AI), TypeScript (web-adjacent), WASM (sandboxed) all use identical capability tokens and IPC messages. Language-specific SDKs are thin wrappers.

**When to apply this pattern:** Any platform targeting developer adoption. The key insight is that the *syscall layer* is the API — language SDKs just make it ergonomic. This means language support is additive (add Swift SDK later) without changing the platform.

---

## 4. Open Questions — Resolved

These questions surfaced during the audit. All 10 have been resolved by cross-referencing the existing documentation. Each resolution cites the specific doc section that answers it. Where the source documentation had gaps, the answers have been integrated directly into the relevant doc:

| Question | Answer Integrated Into |
|---|---|
| Q1. IPC performance justification | [ipc.md](../kernel/ipc.md) §9.1 — new penalty chain table and comparable systems table |
| Q2. Capability token structure | Already complete in [security.md](../security/security.md) §2.2 |
| Q3. Space Query composition | [spaces.md](../storage/spaces.md) §7.5 — new composition rules and latency table |
| Q4. Block engine details | Already complete in [spaces.md](../storage/spaces.md) §4 |
| Q5. Encryption key management | [spaces.md](../storage/spaces.md) §6.3 — new key escrow and recovery section |
| Q6. AIRS crash containment | [airs.md](../intelligence/airs.md) §10.1.1 — new internal crash containment with `catch_unwind` strategy |
| Q7. AIRS split roadmap | Already complete in [airs.md](../intelligence/airs.md) §2.1 |
| Q8. Context Engine algorithm | Already complete in [context-engine.md](../intelligence/context-engine.md) §3–4 |
| Q9. Adversarial defense | [security.md](../security/security.md) §1.5 — new consolidated 11-scenario summary table |
| Q10. Terminology glossary | [architecture.md](./architecture.md) Terminology Glossary — new section before §1 |

---

### Phase 3: IPC & Capabilities

#### Q1. IPC performance target justification ✓

**Question:** What comparable microkernel achieves < 5μs? What workload requires this?

**Resolution:** The target is justified by back-of-envelope math in [ipc.md](../kernel/ipc.md) §9.1. The fast path costs ~415 cycles (~0.2μs at 2 GHz) of kernel overhead. The < 5μs target includes service processing time on both ends. The justification chain is:

1. Every AIOS syscall is an IPC round-trip (microkernel design)
2. A single `read()` in Linux takes ~0.2–0.5μs (cached). In AIOS, that same `read()` is an IPC to Space Service
3. At 50μs IPC, BSD tools performing thousands of small reads would be 100–250x slower than Linux
4. At 5μs IPC, the penalty is 10–25x — still significant but manageable with POSIX shim caching ([ipc.md](../kernel/ipc.md) §12.2 Gap 6)
5. seL4 achieves ~0.5–1μs on ARM for raw IPC; AIOS targets < 5μs for the full round-trip including service processing

**Comparable systems:** seL4 (~0.5–1μs raw IPC on ARM), QNX (~2–5μs round-trip), Fuchsia/Zircon (~1–3μs). AIOS's target is in the same range. The decision gate in `development-plan.md` uses < 10μs as the go/no-go threshold; the 5μs target is aspirational.

**What breaks at 2x:** At 10μs, the POSIX compatibility layer becomes noticeably slow for build tools (grep, find, cc). The shim caching in [ipc.md](../kernel/ipc.md) §12.2 mitigates this. At 50μs, the system would be unusable for POSIX workloads.

---

#### Q2. Capability token structure ✓

**Question:** Where do tokens live? How large? Maximum per agent?

**Resolution:** Fully specified in [security.md](../security/security.md) §2.2:

- **Location:** Per-process `CapabilityTable` stored in kernel memory. Agents hold `CapabilityHandle(u32)` — an index into the kernel's table, not the token itself. Agents cannot read or modify the table.
- **Structure:** `CapabilityToken` contains: `id`, `capability`, `holder`, `granted_by`, `created_at`, `expires`, `delegatable`, `attenuations`, `revoked`, `parent_token`, `usage_count`, `last_used`. Estimated size: ~200–300 bytes per token.
- **Maximum:** Fixed-size array of 256 tokens per agent (`MAX_CAPS_PER_AGENT`). O(1) lookup via handle index. No heap allocation.
- **Trust-level defaults:** System services (Level 1) get up to 256 channels, 128 shared memory regions. Web content (Level 4) gets 16 channels, 8 regions. See [ipc.md](../kernel/ipc.md) §3.3 for the full table.
- **Validation:** 7-step O(1) flow (bounds check → slot check → revoked? → expired? → capability match? → attenuations? → grant). All steps in kernel space, no IPC.

---

### Phase 4: Storage

#### Q3. Space Query grammar ✓

**Question:** Can queries compose? What indices back each type? Latency characteristics?

**Resolution:** The query engine is specified in [spaces.md](../storage/spaces.md) §2 architecture diagram:

| Query Type | Index | Always Available? | Expected Latency |
|------------|-------|-------------------|------------------|
| `Filter` | Object metadata (in-memory hash maps) | Yes | < 1ms |
| `TextSearch` | Inverted index (BM25) | Yes | < 50ms |
| `Semantic` | HNSW embedding index | Requires AIRS | < 500ms |
| `Traverse` | Relationship graph (adjacency lists, bidirectional) | Yes | < 10ms per hop |

**Composition:** The `SpaceQuery` enum in [architecture.md](./architecture.md) §6.6 shows nested constructors (e.g., `SpaceQuery::Filter` combined with `SpaceQuery::Semantic`). The SDK provides typed query builders. The query engine evaluates composed queries by intersecting result sets — each sub-query runs against its index, then results are intersected. Boolean AND is implicit (all sub-queries must match); OR and NOT can be expressed as separate queries with result-set union/difference.

**Composition rules** (now specified in [spaces.md](../storage/spaces.md) §7.5): AND is implicit (intersect result sets), OR via result-set union, NOT via result-set difference. Sub-queries run in parallel against their respective indices. Semantic sub-queries degrade gracefully (return empty set when AIRS unavailable). A formal BNF grammar for the SDK remains desirable for Phase 4.

---

#### Q4. Block engine details ✓

**Question:** Block size? Dedup algorithm? Compaction strategy?

**Resolution:** Fully specified in [spaces.md](../storage/spaces.md) §4:

- **Block size:** Variable size, content-addressed. Each block has header (hash, size, checksum) + data. The Superblock is 4 KB. The MemTable is ~4 MB.
- **Deduplication:** Sub-block dedup using **Rabin rolling hash** with content-defined chunking (CDC). This means chunk boundaries are determined by content, not fixed offsets — identical sub-sequences are deduplicated even if they appear at different offsets.
- **LSM-tree compaction:** 4-level LSM-tree (L0 in-memory + L1–L3 on disk).
  - MemTable flushes to L1 SSTable when it reaches 4 MB
  - L1 compacts to L2 when > 4 SSTables accumulate
  - Compaction produces sorted, deduplicated SSTables
  - Bloom filters (10 bits/key, ~1% false positive) avoid unnecessary disk reads
  - Write stalling: slowdown at 8 L0 SSTables, full stall at 12
  - Tombstone handling for block deletion (shadows live entries in lower levels)
  - SSTable manifest for crash safety (orphaned SSTables detected and deleted on recovery)
  - WAL captures both data blocks and index entries for crash recovery
  - Compaction runs at lowest I/O priority, paused during inference

---

#### Q5. Encryption key management ✓

**Question:** Where are keys stored? How is escrow implemented? What if the user forgets their passphrase?

**Resolution:** Partially specified in [spaces.md](../storage/spaces.md) §2 (Encryption Layer):

- **Algorithm:** AES-256-GCM per space
- **Key derivation:** From identity using Argon2id (memory-hard KDF — resistant to GPU brute-force)
- **Key escrow:** Optional, user-controlled. The user can choose to escrow recovery keys.
- **Transparent operation:** Encrypt/decrypt happens transparently on read/write at the Encryption Layer, below the Object Store

**Operational details** (now specified in [spaces.md](../storage/spaces.md) §6.1.2 and §6.3):
- **Key memory:** `DecryptedSpaceKey` stored on mlock'd kernel pages (`VmFlags::PINNED | VmFlags::NO_DUMP`), auto-zeroized on drop, zeroed on lock/logout/unmount
- **Key escrow:** Master key encrypted with 256-bit recovery key, stored in `system/identity/` (Core zone). Recovery key presented as 24-word BIP-39 mnemonic, never stored on-device.
- **Passphrase forgotten:** If escrow enabled → enter mnemonic → decrypt master → re-derive space keys → set new passphrase. If escrow disabled → data irrecoverable by design.

---

### Phase 8: AIRS Core

#### Q6. AIRS crash containment ✓

**Question:** How does the monolithic process isolate subsystem failures?

**Resolution:** Specified across [airs.md](../intelligence/airs.md) §2.1 and §10.1:

- **Internal isolation:** Security path and intelligence/resource path share no mutable state. Each subsystem is a Rust module with a defined interface. The monolithic process is "not monolithic code."
- **Priority fence:** Security checks (intent verification, behavioral analysis, injection detection) always preempt resource operations. Security IPC uses a dedicated high-priority channel. If AIRS compute is saturated, resource directives are dropped; security checks are never dropped ([security.md](../security/security.md) §2.1).
- **External monitoring:** The kernel monitors AIRS externally — AIRS is a Trust Level 1 process like any other. The kernel enforces capabilities regardless of AIRS's internal structure. If AIRS anomalous behavior is detected (e.g., 200 directives/second vs baseline 5–15/second), the kernel falls back to static heuristics.
- **Service restart:** The microkernel's service restart protocol ([ipc.md](../kernel/ipc.md) §5.5) applies to AIRS. If AIRS crashes: kernel detects death, unblocks all clients with EPIPE, Service Manager restarts AIRS, rebuilds channels, clients retry via SDK auto-reconnection. Recovery target: < 500ms.

**Internal panic handling** (now specified in [airs.md](../intelligence/airs.md) §10.1.1): Each subsystem runs within a `catch_unwind` boundary via `SubsystemRunner`. On panic: log, increment counter, restart module. After 3 panics in 60 seconds: disable subsystem, notify user. The Inference Engine is the exception — if it panics, the full AIRS process restarts via Service Manager (< 500ms recovery).

---

#### Q7. AIRS split roadmap ✓

**Question:** At what threshold does AIRS split? How do they share model weights?

**Resolution:** Fully specified in [airs.md](../intelligence/airs.md) §2.1 with a three-phase roadmap:

| Phase | Hardware | Architecture | Rationale |
|-------|----------|-------------|-----------|
| Phase 1 | 4–8 GB | Single process, internal isolation | One model serves all; splitting wastes KV cache memory |
| Phase 2 | 16 GB | Single process, multiple models | Security tasks get a dedicated small model (~1B) alongside primary (~8B). No process split yet. |
| Phase 3 | 32+ GB | Three separate processes | Split becomes worthwhile: AIRS-Security (own 3B model), AIRS-Intelligence (own 8B+ model), AIRS-Resource (minimal LLM, mostly stats) |

**At 32 GB, the split is worth it because:**
- Security intent checks shouldn't wait behind 2000-token conversation generation
- Each service gets its own failure domain
- Kernel enforces separation with distinct capability sets per service

**Model weight sharing at 32 GB:** Each process loads its own model. At 32 GB, memory allows a 3B security model (~2 GB) + an 8B conversation model (~6 GB) + overhead. No shared memory for model weights needed — each process is self-contained.

**Migration path:** The internal module boundaries are the future process boundaries. The Rust module interfaces become IPC protocols. No architectural redesign required — it's a deployment change, not a code restructure.

---

### Phase 9: Intelligence

#### Q8. Context Engine algorithm ✓

**Question:** Signal weights, update frequency, default state, failure mode?

**Resolution:** Fully specified in [context-engine.md](../intelligence/context-engine.md) §3–4:

**Signal weights** (base weights for rule-based fallback; feature importance priors for AIRS classifier):

| Signal | Weight | Rationale |
|--------|--------|-----------|
| ExplicitIntent | 1.0 | User said what they want. Overrides everything. |
| CalendarState | 0.8 | User scheduled it. High confidence, time-bounded. |
| ActiveSpace | 0.7 | Spaces have clear categories (Work/Media/Gaming/etc.). |
| RunningAgents | 0.6 | Agent combinations reveal intent well. |
| InputPattern | 0.5 | Typing cadence is informative but noisy. |
| MediaPlayback | 0.5 | Video/game strong. Music alone ambiguous. |
| UserHistory | 0.4 | Modifier, not primary. Adjusts other signal weights. |
| TimeOfDay | 0.3 | Weakest. Tiebreaker only. |

**Update frequency:**
- Event-driven signals (ActiveSpace, RunningAgents, CalendarState, MediaPlayback, ExplicitIntent): pushed immediately on change
- Polled signals: InputPattern every 5 seconds, TimeOfDay every 60 seconds
- Coalescing window: 500ms after last signal before running inference (prevents computation during rapid Alt-Tab cycling)

**Algorithm:** Two implementations:
1. **AIRS classifier (primary):** 32-feature vector extracted from signals → small GGML classifier model → `ContextState`. Runs in < 1ms on CPU.
2. **Rule-based fallback (secondary):** Weighted average of per-signal scores. Each signal produces a work_engagement score (0.0–1.0), weighted and normalized. No AIRS required. Runs in < 0.1ms.

**Default state:** If no signals available → `work_engagement: 0.5` (neutral). If total_weight is 0 → neutral.

**Failure mode:** If AIRS unavailable → automatic fallback to rule-based model. If all signal sources fail → neutral state (0.5 work_engagement, Ambient AI engagement, NextBreak notification threshold). System degrades to "dumber but never broken."

**Hysteresis:** Work→Leisure requires 5 minutes sustained leisure signals. Leisure→Work requires 2 minutes sustained work signals. Minimum 0.1 change in work_engagement to trigger publish. Minimum 10 seconds between state transitions. This prevents flickering during brief context switches.

---

#### Q9. Adversarial defense mechanism ✓

**Question:** Threat model with specific attack scenarios and detection strategies?

**Resolution:** [security.md](../security/security.md) §1.4 provides 5 detailed attack scenarios with layer-by-layer analysis. [ipc.md](../kernel/ipc.md) §13 provides 6 additional AI-native attack scenarios with damage ceiling analysis. Combined, these cover 11 specific scenarios:

| # | Scenario | Primary Defense | Detection |
|---|----------|----------------|-----------|
| 1 | Malicious agent reads banking data | Layer 2: capability check (EPERM) | Kernel audit log |
| 2 | Prompt injection via web content | Layer 5: control/data plane separation | AIRS injection detection module |
| 3 | Supply chain attack (compromised dependency) | Layer 3: behavioral baseline deviation (z-score > 3σ) | New access patterns flagged |
| 4 | Fork bomb / DoS | Layer 8: blast radius (max_children, max_memory) | Kernel resource limits |
| 5 | Resource manipulation via AIRS hints | Layer 5: hint screening (over-broad rejected) + Layer 2: kernel caps | AIRS anomaly rate detection |
| 6 | Misrouting via crafted intent | Layer 2: capability set unchanged regardless of intent | Wrong service rejects (wrong protocol) |
| 7 | Warming hint exploitation | Kernel validates capabilities on every channel creation | Agents cannot publish WarmingHint |
| 8 | Batch inference manipulation | Per-agent KV caches, kernel caps batch_window_ms | IpcCall timeouts bound wait |
| 9 | Context spoofing for priority boost | Only AIRS publishes context hints, max +1 class promotion | Agents cannot publish ContextHint |
| 10 | Provenance tag laundering | Kernel writes tags, taint is monotonic (never decreases) | Tags kernel-enforced, not agent-modifiable |
| 11 | Dormant capability exploitation | Only manifest-approved caps can activate, kernel validates scope | Short TTLs, behavioral monitoring post-activation |

**The core mechanism** (control/data plane separation) works as follows:
- Agent **instructions** come from the kernel: manifest, capability set, behavioral policy
- Agent **data** comes from spaces, network, user input — this is the data plane
- AIRS processes data-plane content as data, never as instructions
- Even if AIRS is "jailbroken" by adversarial content, it cannot grant capabilities, modify agent instructions, or bypass kernel enforcement
- **Damage ceiling for a compromised AIRS:** The system behaves like a traditional capability OS (all manifest-approved caps active, no behavioral screening, no intent verification). Slower and dumber, but equally secure at the kernel level.

---

### Phase 10: Agents

#### Q10. Terminology glossary ✓

**Question:** Canonical definitions for Agent, Task, Session, Service, Space.

**Resolution:** Derived from the type system already in the docs. Here is the canonical glossary:

| Term | Context | Canonical Type | Definition | Source |
|------|---------|---------------|------------|--------|
| **Agent** (manifest) | Installation | `AgentManifest` | Signed package declaring name, author, requested capabilities, code hash, dependencies, and AI security analysis | [architecture.md](./architecture.md) §2.3 |
| **Agent** (process) | Runtime | `AgentProcess` | A running process created from a manifest, with a PID, capability table, resource limits, and behavioral baseline | [security.md](../security/security.md) §2.2–2.3 |
| **Task** (intent) | User-facing | `Task` | A user's goal decomposed into subtasks, with agents assigned, capabilities scoped, and activity logged | [architecture.md](./architecture.md) §2.3 |
| **Task** (scheduler) | Kernel | `Thread` / scheduling entity | A schedulable unit of work assigned to a scheduling class (RT, Interactive, Normal, Idle) | [scheduler.md](../kernel/scheduler.md) |
| **Session** (subsystem) | Hardware | `SubsystemSession` | A bounded interaction with a hardware subsystem (audio output session, camera capture session) | [subsystem-framework.md](../platform/subsystem-framework.md) |
| **Session** (inference) | AIRS | `InferenceSession` | A single inference request with its own KV cache, priority, token callback, and stop sequences | [airs.md](../intelligence/airs.md) §3.1 |
| **Service** (process) | System | Trust Level 1 process | A userspace daemon (AIRS, Space Storage, Compositor, NTM, Service Manager) with elevated capabilities | [architecture.md](./architecture.md) §2.1 |
| **Service** (endpoint) | IPC | `ChannelId` + protocol | A capability-gated IPC channel with a registered protocol that clients call via `IpcCall` | [ipc.md](../kernel/ipc.md) §5 |
| **Space** (collection) | Storage | `Space` struct | A named collection of typed objects with a security zone, encryption state, quota, and parent hierarchy | [spaces.md](../storage/spaces.md) §3.1 |
| **Space** (security) | Security | `SecurityZone` | The zone classification of a space: Core, Personal, Collaborative, or Untrusted | [security.md](../security/security.md) §1.2 |

**Implementation rule:** In code, always use the specific type name (`AgentManifest`, `InferenceSession`, `SubsystemSession`), never the bare term ("agent," "session"). The bare term is acceptable in user-facing UI and conversation but never in code or API documentation.

**Status:** This glossary has been added to [architecture.md](./architecture.md) as a dedicated section before §1 (Vision). All subsequent docs should reference it.

---

## 5. Audit Methodology

This retrospective was produced via a multi-pass documentation audit:

1. **Pass 1: Cross-document consistency.** Read all 28 documents and flagged contradictions in data models, numbers, terminology, and timelines. Fixed 18 issues.
2. **Pass 2: Deep structural review.** Audited kernel/ and project/ documents for internal consistency, missing sections, and specification gaps. Fixed 11 issues.
3. **Pass 3: Lessons extraction.** Categorized findings into strengths, anti-patterns, reusable patterns, and open questions.

Total issues identified and fixed: **29 across 13 files.**

Future audits should follow this same methodology: consistency first, depth second, synthesis third.

---

## 6. Document Index Cross-Reference

This document relates to:

| Document | Relationship |
|----------|-------------|
| [architecture.md](./architecture.md) | Primary source for most lessons |
| [development-plan.md](./development-plan.md) | Phase gates referenced in open questions |
| [airs.md](../intelligence/airs.md) | Source for A4 (monolithic AIRS) and S3 (AI as infrastructure) |
| [spaces.md](../storage/spaces.md) | Source for S1 (Spaces abstraction) and R3 (content-addressed storage) |
| [security.md](../security/security.md) | Source for S5 (capabilities) and A6 (adversarial defense) |
| [subsystem-framework.md](../platform/subsystem-framework.md) | Source for S2 (one framework, every subsystem) |
| [context-engine.md](../intelligence/context-engine.md) | Source for A6 (underspecified novel mechanisms) |
| [scheduler.md](../kernel/scheduler.md) | Source for R1 (bounded resource consumption) |
| [boot.md](../kernel/boot.md) | Audit pass 1 fixed boot budget math |
| [boot-lifecycle.md](../kernel/boot-lifecycle.md) | Audit pass 1 fixed phase naming disambiguation |
