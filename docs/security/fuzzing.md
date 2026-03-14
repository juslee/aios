# Fuzzing and Input Hardening

This document provides a technical deep-dive into how AIOS defends against the classes of bugs that fuzzing discovers, and how AIOS adopts fuzzing as a first-class testing strategy across its development phases.

For the fuzzing campaign summary and formal verification targets, see [security.md](security.md) §8. For the companion deep-dive on static analysis, model checking, and formal verification, see [static-analysis.md](static-analysis.md).

---

## Document Map

This document was split for navigability. Each sub-document preserves the original section numbers for cross-reference stability.

| Document | Sections | Content |
|---|---|---|
| **This file** | §1, §2 | Overview and attack surface map |
| [strategies.md](fuzzing/strategies.md) | §3 | Hardening strategies: language safety, syscall validation, memory, IPC, drivers, manifests, concurrency |
| [adoption-roadmap.md](fuzzing/adoption-roadmap.md) | §4 | Phased fuzzing adoption from host-side (Phase 0) through formal verification (Phase 24) |
| [tooling.md](fuzzing/tooling.md) | §5, §6 | Tiered tooling strategy (3 tiers) and fuzz target catalog |
| [ai-native.md](fuzzing/ai-native.md) | §7 | AI-native fuzzing: development-time tools, kernel-internal models, AIRS-dependent defense |

---

## 1. Why Fuzzing Matters for a Kernel

**Binary fuzzing** is a testing technique that feeds random, malformed, or adversarial input to a program and monitors for crashes, hangs, or memory corruption. Three primary fuzzing strategies exist:

| Strategy | How it works | Best for |
|---|---|---|
| **Mutation-based** | Mutates valid inputs (bit flips, byte insertions, truncation) | File parsers, network protocols |
| **Coverage-guided** | Tracks code paths and evolves inputs toward new coverage | Syscall interfaces, complex state machines |
| **Grammar-based** | Generates inputs from a grammar or protocol specification | Structured formats (manifests, IPC messages, ELF headers) |

Kernels are high-value fuzzing targets because they sit at the trust boundary between unprivileged code and hardware. A single bug in syscall parameter validation can escalate to arbitrary kernel memory access. Linux discovers hundreds of kernel bugs per year through syzkaller, a coverage-guided syscall fuzzer — syzbot has found over 6,800 bugs to date.

**AIOS-specific context.** In AIOS, autonomous AI agents are the primary syscall callers. Unlike traditional user programs written by developers who generally pass valid arguments, agents are opaque programs that may call any syscall with any argument at any time. A compromised or buggy agent is functionally equivalent to a local attacker with syscall access. This makes the syscall interface, IPC message parser, and agent manifest validator the three critical fuzz surfaces.

**State-of-the-art advances.** Kernel fuzzing has evolved significantly beyond random mutation. Modern techniques include stateful syscall fuzzing with learned dependencies (MOCK, NDSS 2024), LLM-generated syscall specifications (KernelGPT, ASPLOS 2025), multi-armed bandit seed scheduling (T-Scheduler, AsiaCCS 2024), and out-of-order concurrency bug detection (OZZ, SOSP 2024 Best Paper). AIOS adopts these techniques in its fuzzing roadmap — see [adoption-roadmap.md](fuzzing/adoption-roadmap.md) §4.2.1 and [ai-native.md](fuzzing/ai-native.md) §7.1.

---

## 2. Attack Surface Map

Every input boundary where external data enters kernel code is a potential fuzz target. The table below maps AIOS subsystems to their input surfaces, the phase at which each becomes relevant, and the invariant that must hold.

| Subsystem | Input boundary | Phase | Invariant |
|---|---|---|---|
| UART driver | MMIO register reads | 0+ | Read values are bounded; busy-wait loops have timeouts |
| Page table management | Mapping requests from kernel subsystems | 1+ | No page is both writable and executable (W^X) |
| ELF loader (UEFI stub) | PT_LOAD segment parsing from kernel ELF | 1+ | Malformed ELF returns error; bounds checked before mapping |
| Buddy allocator | Allocation/deallocation requests | 2+ | No double-free; no use-after-free; alignment maintained; poison fill on free |
| Slab allocator | Object alloc/free from kernel heap | 2+ | Object size matches slab class; freed objects are poisoned; red zones intact |
| Syscall interface | All 31 syscalls with arbitrary parameters | 3+ | No kernel panic; no memory corruption; no capability leak |
| IPC messages | Message payload from sender process | 3+ | Message within size limit; type-checked; capabilities validated |
| Capability tokens | Handle values passed via syscalls | 3+ | Invalid handles rejected; no forge; no escalation |
| ELF loader (kernel) | User binary headers and section data | 3+ | Malformed ELF does not crash kernel; bounds checked before mapping |
| Scheduler | Priority and affinity parameters | 3+ | Out-of-range values clamped or rejected |
| VirtIO-blk driver | MMIO register reads, device capacity, poll responses | 4+ | Invalid magic rejected; poll loops timeout; capacity bounds-checked |
| Block Engine superblock | On-disk superblock bytes (4 KiB) | 4+ | Bad magic/version/checksum returns error; no panic |
| Block Engine WAL | WAL entry stream (64-byte `repr(C)` entries) | 4+ | Corrupted entries skipped on replay; circular buffer bounds enforced |
| Block Engine data blocks | Content blocks with CRC-32C and SHA-256 hash | 4+ | CRC mismatch returns error; hash verified; no corrupt data served |
| MemTable | Insert/get/remove operations with content hashes | 4+ | Capacity (65536) enforced; binary search invariant maintained |
| Compositor | Input events (coordinates, surface IDs) | 6+ | Out-of-range clamped; invalid surface rejected; event floods handled |
| Network stack | Packet data from virtio-net | 7+ | Malformed packets dropped; no buffer overflow in protocol parsing |
| Agent manifests | TOML/JSON manifest during install | 10+ | Schema validated; circular delegation detected; signatures verified |

Cross-reference: [security.md](security.md) §1 (threat model), §§2–3 (IPC security architecture and capability system).

---

## Cross-Reference Index

| Section | Location | Topic |
|---|---|---|
| §1 | This file | Why fuzzing matters |
| §2 | This file | Attack surface map |
| §3.1–3.6 | [strategies.md](fuzzing/strategies.md) | Language, syscall, memory, IPC, driver, manifest hardening |
| §3.7 | [strategies.md](fuzzing/strategies.md) | Concurrency hardening (ARM ordering, Loom, controlled scheduling) |
| §4.1 | [adoption-roadmap.md](fuzzing/adoption-roadmap.md) | Phases 0–2: host-side fuzzing |
| §4.2 | [adoption-roadmap.md](fuzzing/adoption-roadmap.md) | Phases 3–5: syscall/IPC fuzzing + stateful techniques |
| §4.3 | [adoption-roadmap.md](fuzzing/adoption-roadmap.md) | Phase 4: storage fuzzing |
| §4.4–4.7 | [adoption-roadmap.md](fuzzing/adoption-roadmap.md) | Phases 6–24: compositor, network, agent, full campaign, formal verification |
| §5.1–5.3 | [tooling.md](fuzzing/tooling.md) | Tiered tooling strategy (Tier 1–3) |
| §5.4 | [tooling.md](fuzzing/tooling.md) | Corpus management |
| §6 | [tooling.md](fuzzing/tooling.md) | Fuzz target catalog (22 targets) |
| §7.1 | [ai-native.md](fuzzing/ai-native.md) | Development-time AI tools |
| §7.2 | [ai-native.md](fuzzing/ai-native.md) | Kernel-internal AI (frozen models) |
| §7.3 | [ai-native.md](fuzzing/ai-native.md) | AIRS-dependent security |
