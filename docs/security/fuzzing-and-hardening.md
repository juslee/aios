# Fuzzing and Input Hardening

This document provides a technical deep-dive into how AIOS defends against the classes of bugs that fuzzing discovers, and how AIOS adopts fuzzing as a first-class testing strategy across its development phases.

For the fuzzing campaign summary and formal verification targets, see [security.md](security.md) §8.

---

## 1. Why Fuzzing Matters for a Kernel

**Binary fuzzing** is a testing technique that feeds random, malformed, or adversarial input to a program and monitors for crashes, hangs, or memory corruption. Three primary fuzzing strategies exist:

| Strategy | How it works | Best for |
|---|---|---|
| **Mutation-based** | Mutates valid inputs (bit flips, byte insertions, truncation) | File parsers, network protocols |
| **Coverage-guided** | Tracks code paths and evolves inputs toward new coverage | Syscall interfaces, complex state machines |
| **Grammar-based** | Generates inputs from a grammar or protocol specification | Structured formats (manifests, IPC messages, ELF headers) |

Kernels are high-value fuzzing targets because they sit at the trust boundary between unprivileged code and hardware. A single bug in syscall parameter validation can escalate to arbitrary kernel memory access. Linux discovers hundreds of kernel bugs per year through syzkaller, a coverage-guided syscall fuzzer.

**AIOS-specific context.** In AIOS, autonomous AI agents are the primary syscall callers. Unlike traditional user programs written by developers who generally pass valid arguments, agents are opaque programs that may call any syscall with any argument at any time. A compromised or buggy agent is functionally equivalent to a local attacker with syscall access. This makes the syscall interface, IPC message parser, and agent manifest validator the three critical fuzz surfaces.

---

## 2. Attack Surface Map

Every input boundary where external data enters kernel code is a potential fuzz target. The table below maps AIOS subsystems to their input surfaces, the phase at which each becomes relevant, and the invariant that must hold.

| Subsystem | Input boundary | Phase | Invariant |
|---|---|---|---|
| UART driver | MMIO register reads | 0+ | Read values are bounded; busy-wait loops have timeouts |
| Page table management | Mapping requests from kernel subsystems | 1+ | No page is both writable and executable (W^X) |
| Buddy allocator | Allocation/deallocation requests | 2+ | No double-free; no use-after-free; alignment maintained |
| Slab allocator | Object alloc/free from kernel heap | 2+ | Object size matches slab class; freed objects are poisoned |
| Syscall interface | All 31 syscalls with arbitrary parameters | 3+ | No kernel panic; no memory corruption; no capability leak |
| IPC messages | Message payload from sender process | 3+ | Message within size limit; type-checked; capabilities validated |
| Capability tokens | Handle values passed via syscalls | 3+ | Invalid handles rejected; no forge; no escalation |
| ELF loader | Binary headers and section data | 3+ | Malformed ELF does not crash kernel; bounds checked before mapping |
| Scheduler | Priority and affinity parameters | 3+ | Out-of-range values clamped or rejected |
| Agent manifests | TOML/JSON manifest during install | 10+ | Schema validated; circular delegation detected; signatures verified |
| Network stack | Packet data from virtio-net | 7+ | Malformed packets dropped; no buffer overflow in protocol parsing |
| Filesystem | Metadata and file content from storage | 4+ | Corrupted metadata does not panic; bounds checked on all reads |

Cross-reference: [security.md](security.md) §1 (threat model), §3 (capability system), §4 (IPC security).

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

**Early-phase advantage.** In Phases 0-2, AIOS has no heap allocator — all data is static or stack-allocated. This eliminates heap-related bugs entirely until the buddy and slab allocators are introduced in Phase 2.

### 3.2 Syscall Validation

Every syscall validates all parameters at the kernel entry point before any kernel state is accessed or modified. The validation sequence is:

1. **Syscall number**: reject if not in `[0, SYSCALL_MAX]`
2. **Capability handles**: bounds-check against process capability table; verify generation counter matches (prevents use-after-revoke)
3. **Pointer arguments**: must fall within user address range (`0x0000_0000_0000_0000` to `0x0000_FFFF_FFFF_FFFF`); must be aligned to the expected type; must be backed by a mapped, readable (or writable) page
4. **Length arguments**: reject `0` and values exceeding per-syscall maximums; reject `length > buffer_mapping_size`
5. **Enum/flag arguments**: reject values outside the valid set; no "reserved for future use" bits accepted

No syscall implementation trusts any user-supplied value without validation. This is enforced by code review (see [security.md](security.md) §8.1 agent audit tool) and by syscall fuzzing (§4.2 below).

### 3.3 Memory Hardening

| Mechanism | What it prevents | Phase |
|---|---|---|
| **W^X enforcement** | Code injection via writable+executable pages | 1+ |
| **Guard pages** | Stack overflow overwriting adjacent memory | 1+ |
| **Buddy allocator poisoning** | Use-after-free (freed pages filled with `0xDEAD`) | 2+ |
| **Double-free detection** | Freeing already-free pages (bitmap/buddy tree check) | 2+ |
| **Slab red zones** | Buffer overflow within slab objects | 2+ |
| **KASLR** | Predictable kernel addresses for exploit chaining | 1+ |
| **PAC** (Pointer Authentication) | Return address overwrite, ROP chains | 13+ |
| **BTI** (Branch Target Identification) | JOP (Jump-Oriented Programming) attacks | 13+ |
| **MTE** (Memory Tagging Extension) | Spatial and temporal memory errors in `unsafe` blocks | 13+ |

PAC, BTI, and MTE are aarch64 hardware features enabled in Phase 13 (Security Hardening). MTE is particularly valuable for fuzzing: when enabled, the hardware tags every allocation and checks tags on every access, catching use-after-free and out-of-bounds access at the exact instruction — not just when a crash eventually occurs.

### 3.4 IPC Hardening

IPC is the primary inter-process communication mechanism and a high-value fuzz target because every system service receives messages from potentially untrusted agents.

- **Message size**: enforced at kernel level before copying into receiver buffer. Messages exceeding `IPC_MAX_MESSAGE_SIZE` are rejected with an error, never truncated.
- **Type safety**: messages carry a type tag verified against the channel's declared protocol. Type mismatches are rejected.
- **Capability transfer**: when a message carries a capability, the kernel validates that the sender actually holds the capability and that the transfer is permitted by the capability's attenuation rules.
- **No raw pointers in messages**: IPC payloads are copied between address spaces. Pointers in message bodies have no meaning in the receiver's address space and are never interpreted as addresses.

Cross-reference: [security.md](security.md) §4 (IPC security).

### 3.5 Device Driver Hardening

Device drivers read data from hardware via MMIO, which is inherently untrusted (a misbehaving device can return any value).

- **Timeout on busy-wait**: every loop that polls a device status register has a maximum iteration count. If the device does not respond within the timeout, the driver returns an error instead of hanging the kernel.
- **Bounds-check on reads**: values read from device registers are validated before use (e.g., a DMA descriptor length is checked against the expected maximum before allocating a buffer).
- **DMA buffer isolation**: when DMA is introduced (Phase 7+), DMA buffers are allocated from a dedicated pool with guard pages. IOMMU is used (where available) to restrict device access to only the intended buffers.

### 3.6 Manifest and Agent Hardening

Agent manifests are parsed during installation and define the agent's requested capabilities, dependencies, and metadata. Since manifests come from external sources (developers, app stores), they are fully untrusted.

- **Schema validation**: the manifest parser validates the document structure before accessing any field. Missing required fields, unexpected types, and unknown keys are rejected.
- **Circular delegation detection**: the capability delegation graph is checked for cycles before approval. An agent cannot grant itself capabilities through a circular chain.
- **Signature verification**: manifests must be signed by a certificate that chains to the AIOS Root CA. Invalid, expired, or revoked signatures are rejected before any capability is granted.

Cross-reference: [security.md](security.md) §8.1 (agent audit tool).

---

## 4. Fuzzing Adoption Roadmap

AIOS adopts fuzzing incrementally, aligned with the phase at which each subsystem becomes available. The goal is to fuzz every input boundary as soon as it exists, not to defer all fuzzing to a single hardening phase.

### 4.1 Phases 0-2: Host-Side Fuzzing

Before the kernel has a syscall interface, fuzzing targets the `shared/` crate and kernel data structures that can be tested on the host.

**Targets:**
- `BootInfo` deserialization — fuzz the struct validation logic with arbitrary byte sequences
- `PhysAddr` / `VirtAddr` construction — fuzz alignment and range checks
- Buddy allocator (once implemented in Phase 2) — fuzz allocation/deallocation sequences for invariant violations

**Tools:**
- `cargo-fuzz` with `libFuzzer` — runs on the host (not in QEMU)
- `proptest` for property-based testing — generates structured random inputs and checks invariants

**Integration:** Add fuzz targets to CI as nightly jobs. Corpus is committed to the repository under `fuzz/corpus/`.

### 4.2 Phases 3-5: Syscall Fuzzing

Once the syscall interface exists (Phase 3), kernel fuzzing begins in earnest.

**Approach:** A custom fuzzer runs inside a QEMU guest, invokes syscalls with randomized parameters, and reports crashes via the UART. The host-side harness monitors QEMU for:
- Kernel panics (panic handler prints to UART)
- Hangs (no UART output within timeout)
- Memory corruption (detected by allocator poisoning or MTE when available)

**Coverage guidance:** The fuzzer instruments the kernel binary (via LLVM SanitizerCoverage or equivalent) to track which syscall paths are exercised, evolving inputs toward uncovered code.

**Scope:** All 31 syscalls, all parameter combinations. The fuzzer generates both valid and invalid inputs — valid inputs exercise normal paths, invalid inputs exercise error-handling paths.

### 4.3 Phases 10-12: Agent and Manifest Fuzzing

When the agent framework is introduced (Phase 10), three new fuzz surfaces appear:

- **Manifest parser**: fuzz with malformed TOML, truncated files, oversized fields, invalid UTF-8
- **IPC message bodies**: fuzz with truncated payloads, wrong type tags, oversized messages, capability references to non-existent handles
- **Agent capability requests**: fuzz the capability approval flow with invalid delegation chains, circular references, expired tokens

These are grammar-based fuzzing targets — the fuzzer generates inputs from the manifest schema and IPC protocol definitions, then mutates them to exercise error paths.

### 4.4 Phase 13+: Full Fuzzing Campaign

Phase 13 (Security Hardening) enables hardware security features that make fuzzing dramatically more effective:

- **MTE-enabled fuzzing**: every heap allocation gets a 4-bit tag. Accessing freed memory or overflowing into an adjacent allocation causes an immediate synchronous exception — not a silent corruption that manifests later. This turns temporal and spatial memory errors from "hard to detect" to "instantly caught."
- **PAC-enabled builds**: corrupted return addresses are detected at function return, catching control-flow hijacking.
- **Continuous fuzzing in CI**: the fuzzer runs 24/7 on a dedicated CI runner. New crashes are filed automatically. Regression tests are generated from crash inputs and added to the test suite.
- **Corpus management**: the fuzzing corpus is stored in the repository and shared across CI runs. Interesting inputs (those that found new coverage) are kept; redundant inputs are minimized.

### 4.5 Phase 24: Formal Verification Complements Fuzzing

Fuzzing finds bugs but cannot prove their absence. Formal verification provides mathematical guarantees for the most critical subsystems:

| Target | Method | Property |
|---|---|---|
| Capability system | TLA+ model (Phase 13) → Coq proofs (Phase 24) | No forge, no escalation |
| IPC | TLA+ model | No cross-address-space memory leak |
| Provenance chain | Coq proofs | Append-only, tamper-evident |
| W^X enforcement | Exhaustive path analysis | No page is ever both writable and executable |

Fuzzing and formal verification are complementary: fuzzing catches implementation bugs in code that is too complex to verify formally, while verification proves that the design of critical subsystems is correct regardless of input.

Cross-reference: [security.md](security.md) §8.3 (formal verification targets).

---

## 5. Tooling and Infrastructure

| Tool | Purpose | Target |
|---|---|---|
| `cargo-fuzz` / `libFuzzer` | Host-side fuzz harnesses | `shared/` crate, allocator, parsers |
| `proptest` | Property-based testing with shrinking | Data structure invariants |
| Custom QEMU fuzzer | In-guest syscall fuzzing with UART crash detection | Kernel syscall interface |
| LLVM SanitizerCoverage | Coverage instrumentation for guided fuzzing | Kernel binary (QEMU builds) |
| MTE (hardware) | Tag-based memory error detection | All heap allocations (Phase 13+) |
| CI runner (nightly) | Continuous fuzzing with corpus management | All fuzz targets |

**Corpus management.** Fuzz corpora live under `fuzz/corpus/<target_name>/`. Interesting inputs are committed; crash-triggering inputs become regression tests under `fuzz/regressions/`. The CI pipeline runs `cargo fuzz run <target> -- -max_total_time=3600` nightly.

**Crash triage.** Crashes are deduplicated by stack trace. Each unique crash is filed with: the crashing input, the stack trace, the git commit, and the reproducer command. Crashes that involve `unsafe` blocks are treated as P0 (highest priority).

---

## 6. Fuzz Target Catalog

The table below lists concrete fuzz targets, the phase at which they become available, and the invariant that the fuzzer checks.

| Target | Subsystem | Phase | Input type | Invariant |
|---|---|---|---|---|
| `fuzz_bootinfo_parse` | shared | 0 | `[u8; N]` | No panic on any input; invalid magic rejected |
| `fuzz_physaddr_new` | shared | 0 | `u64` | Misaligned/out-of-range values rejected |
| `fuzz_buddy_alloc` | memory | 2 | `Vec<(AllocOp, usize)>` | No double-free; all freed memory returned to pool |
| `fuzz_slab_alloc` | memory | 2 | `Vec<(AllocOp, usize)>` | Object sizes match slab class; no overlap |
| `fuzz_syscall_*` | syscall | 3 | `(u64, u64, u64, u64, u64, u64)` | No kernel panic; no capability leak; error returned |
| `fuzz_ipc_send` | IPC | 3 | `(channel_id, &[u8], Option<CapHandle>)` | No panic; oversized rejected; invalid cap rejected |
| `fuzz_ipc_receive` | IPC | 3 | `(channel_id, &mut [u8])` | No panic; buffer bounds respected |
| `fuzz_elf_load` | loader | 3 | `&[u8]` (ELF binary) | No panic; malformed ELF returns error |
| `fuzz_manifest_parse` | agent | 10 | `&str` (TOML) | No panic; invalid schema rejected; cycles detected |
| `fuzz_cap_attenuate` | capability | 3 | `(CapHandle, AttenuationMask)` | Result is subset of source; invalid handles rejected |
| `fuzz_packet_parse` | network | 7 | `&[u8]` (raw packet) | No panic; malformed packets dropped |
| `fuzz_fs_metadata` | filesystem | 4 | `&[u8]` (metadata block) | No panic; corrupted metadata returns error |
