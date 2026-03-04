# Static Analysis and Formal Verification

This document provides a technical deep-dive into how AIOS uses static analysis, model checking, and formal verification to prevent bugs before they reach runtime — both in kernel development and in agent pre-installation auditing.

For fuzzing and runtime hardening, see [fuzzing-and-hardening.md](fuzzing-and-hardening.md). For the security model overview and formal verification targets, see [security.md](security.md) §8.

---

## 1. Why Static Analysis Matters for a Kernel

**Static analysis** examines code without executing it, using techniques such as type checking, data-flow analysis, pattern matching on AST/MIR, and bounded model checking to detect bugs, security vulnerabilities, and policy violations at compile time or CI time. For a kernel, every bug that reaches runtime is a potential privilege escalation. Static analysis is the first line of defense, complementing fuzzing (which finds runtime bugs on adversarial inputs) and formal verification (which proves absence of entire bug classes).

| Strategy | How it works | Best for |
|---|---|---|
| **Type-system enforcement** | Compiler rejects code violating type, lifetime, and borrow rules | Memory safety, data races, use-after-free |
| **Lint-based analysis** | Pattern-matching on AST/MIR for known anti-patterns | Style, correctness, performance pitfalls |
| **MIR interpretation** | Stepwise execution of Rust MIR under strict memory model rules | Undefined behavior in `unsafe` blocks |
| **Model checking** | Exhaustive exploration of bounded state space | Concurrency bugs, invariant violations, panic reachability |
| **Formal verification** | Mathematical proof of properties over code or specifications | Capability system correctness, IPC isolation, W^X |
| **Dependency auditing** | Scanning dependency tree for vulnerabilities and policy violations | Supply chain attacks, license compliance |
| **AI-assisted review** | LLM analysis of code semantics and intent | Data exfiltration patterns, capability misuse |

**AIOS-specific context.** Two aspects make AIOS unique. First, the kernel is Rust — the borrow checker already eliminates ~65% of the CVE classes that plague C/C++ kernels (see [fuzzing-and-hardening.md](fuzzing-and-hardening.md) §3.1). Static analysis builds on this foundation, focusing on `unsafe` blocks and higher-level invariants. Second, AIOS runs autonomous AI agents that must be statically analyzed before installation, because agents are opaque programs from untrusted developers. A compromised or buggy agent is functionally equivalent to a local attacker with syscall access.

---

## 2. Defect Surface Map — Kernel Development

Every kernel subsystem has defect classes that static analysis can catch before runtime. The table below maps subsystems to the tool that targets each defect class and the phase at which analysis becomes relevant.

| Subsystem | Defect class | Tool | Phase |
|---|---|---|---|
| All Rust code | Memory safety, data races, lifetime errors | `rustc` borrow checker | 0+ |
| All Rust code | Known anti-patterns, correctness pitfalls | Clippy (`-D warnings`) | 0+ |
| `unsafe` blocks (MMIO, asm, page tables) | UB: aliasing, alignment, uninitialized memory | Miri | 0+ |
| `unsafe` blocks | Panic safety, Send/Sync variance, higher-order invariants | Rudra | 2+ |
| Capability system, allocators | State-space invariant violations, panic reachability | Kani model checker | 3+ |
| Scheduler, IPC, allocator concurrency | Lock ordering, deadlock, data races in unsafe concurrent code | Converos | 3+ |
| All dependencies | Known CVEs, yanked crates | `cargo-audit` | 0+ |
| All dependencies | License violations, banned crates, duplicates | `cargo-deny` | 0+ |
| All dependencies | Human audit provenance tracking | `cargo-vet` | 2+ |
| Capability system, IPC | Design-level correctness proofs | TLA+ / Coq | 13+ / 24 |
| Page table management | W^X enforcement proof | Kani / exhaustive path analysis | 13+ |

Cross-reference: [fuzzing-and-hardening.md](fuzzing-and-hardening.md) §2 maps the same subsystems to fuzzing targets; [security.md](security.md) §8.3 lists formal verification targets.

---

## 3. Defect Surface Map — Agent Pre-Installation

The `aios agent audit` tool runs five static analysis passes on every agent before installation. No agent code executes until all passes complete.

| Analysis pass | What it checks | Defect class prevented |
|---|---|---|
| **Manifest analysis** | Capability rationale strings present, no overly broad caps (`ReadSpace("*")`), network destinations specific, capability set consistent with declared purpose | Privilege over-provisioning, confused deputy setup |
| **Code static analysis** | No direct syscall invocations (SDK only), no `unsafe` blocks, no filesystem path manipulation, no environment variable reads, no dynamic library loading | Sandbox escape, privilege escalation |
| **Dependency analysis** | All deps pinned to exact versions, CVE scan against RustSec/advisory databases | Supply chain attacks |
| **Capability usage analysis** | Cross-reference declared capabilities against actual code — flag unused caps (over-provisioned) and caps used but not declared (under-declared) | Manifest dishonesty, over-provisioning |
| **AIRS code review** | LLM-based semantic analysis for data exfiltration patterns, missing input validation, error handling that leaks sensitive info | Behavioral threats undetectable by pattern matching |

For example output and developer UX, see [security.md](security.md) §8.1 and [agents.md](../applications/agents.md).

---

## 4. Kernel-Side Tool Deep Dives

### 4.1 Rust Compiler — Borrow Checker and Type System

The Rust compiler is AIOS's most powerful static analyzer. Ownership and borrowing eliminate buffer overflow (~35% of kernel CVEs), use-after-free (~20%), and uninitialized memory (~10%) at compile time. For the full breakdown, see [fuzzing-and-hardening.md](fuzzing-and-hardening.md) §3.1.

What remains are `unsafe` blocks, which AIOS requires for MMIO register access, inline assembly, raw pointer manipulation (page table walks), and system register access. Every `unsafe` block follows the documentation standard defined in `CLAUDE.md`: a `// SAFETY:` comment stating the invariant, who maintains it, and what happens if violated. These blocks are the primary target for all tools below.

AIOS additionally enables `#![forbid(unsafe_op_in_unsafe_fn)]` to require explicit `unsafe` blocks even inside `unsafe fn` signatures, ensuring no unsafe operation is invisible.

### 4.2 Clippy — Lint-Based Analysis

Clippy provides 750+ lints covering correctness, performance, and style. AIOS runs Clippy with `-D warnings` (deny all warnings), enforced in CI via `just check`.

Kernel-specific lints to enable beyond the defaults:

| Lint | Why it matters for a kernel |
|---|---|
| `clippy::undocumented_unsafe_blocks` | Enforces the `// SAFETY:` standard at the compiler level |
| `clippy::missing_safety_doc` | Enforces safety docs on `unsafe fn` signatures |
| `clippy::cast_possible_truncation` | Critical for address arithmetic (u64 → u32 truncation) |
| `clippy::cast_sign_loss` | Prevents signed/unsigned confusion in register values |
| `clippy::indexing_slicing` | Prefer `.get()` to prevent panics in kernel code |

Integration: already active in `just clippy` (justfile), `just check`, and CI pipeline (`.github/workflows/ci.yml`).

### 4.3 Miri — MIR Interpreter for Unsafe Code

Miri interprets Rust's Mid-level Intermediate Representation (MIR) under strict memory model rules, detecting undefined behavior that the compiler cannot catch statically:

- Aliasing violations (Stacked Borrows / Tree Borrows model)
- Alignment errors
- Use of uninitialized memory
- Out-of-bounds access
- Invalid enum discriminants
- Dangling references

**Limitation:** Miri cannot interpret inline assembly or MMIO operations. It targets the `shared/` crate and pure-Rust kernel logic extracted into host-testable functions. It cannot run the full `no_std` kernel binary.

**Configuration:** `MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-symbolic-alignment-check"` for maximum strictness.

**Integration:** A `just miri` target running `cargo +nightly miri test -p shared` and any kernel modules with host-testable logic. Runs as a nightly CI job.

### 4.4 Rudra — Unsafe Code Analyzer

Rudra is a research static analyzer from Georgia Tech (SOSP '21 Distinguished Artifact Award) that detects three bug patterns in `unsafe` Rust:

1. **Panic safety** — unwinding through `unsafe` code that has partially established invariants, leaving data structures in an inconsistent state.
2. **Send/Sync variance** — types manually implementing `Send` or `Sync` that violate thread-safety requirements.
3. **Higher-order invariant** — `unsafe` code relying on caller-provided closures or trait implementations to maintain invariants.

Rudra found 264 memory safety bugs across crates.io (76 CVEs, 112 RustSec advisories). For AIOS, any type marked `Send` or `Sync` for cross-core sharing and any `unsafe` block accepting closures or trait objects is a Rudra target.

**Integration:** Periodic manual scans via Docker. Rudra is research-quality and not suitable for blocking CI, but results are triaged as high-priority findings.

### 4.5 Kani — Bit-Precise Model Checker

Kani translates Rust to CBMC and exhaustively explores all execution paths within bounded inputs. Unlike testing (which checks specific inputs) or fuzzing (which checks random inputs), Kani proves properties hold for **all** inputs up to a specified bound.

**Proven in practice:** AWS uses Kani to verify Firecracker VMM security boundaries. Merlin OS uses Kani for kernel atomic primitives and mutexes. The Rust standard library verification effort uses Kani for core data structures.

**AIOS targets:**

| Target | Property to prove | Phase |
|---|---|---|
| Capability attenuation | `child.permissions ⊆ parent.permissions` for all derivations | 3+ |
| Buddy allocator | No double-free; all freed pages return to pool | 2+ |
| Page table flags | No PTE ever has both write and execute bits set (W^X) | 1+ |
| Address space isolation | `mapped_pages(p1) ∩ mapped_pages(p2) = ∅` absent explicit sharing | 3+ |

**Integration:** `#[kani::proof]` harnesses alongside unit tests. A `just kani` target runs `cargo kani` on annotated modules. Initially nightly CI; blocking for PRs touching security-critical code in Phase 13+.

### 4.6 Supply Chain Security

Three tools form a layered defense against dependency-related risks:

**`cargo-audit`** scans `Cargo.lock` against the RustSec Advisory Database. Any finding of severity High or Critical blocks the PR. Runs on every CI build.

**`cargo-deny`** enforces broader policies via a committed `deny.toml`:
- **Licenses:** BSD-2-Clause, MIT, Apache-2.0 only — no GPL in `kernel/` or `shared/` (per `CLAUDE.md` crate rules).
- **Bans:** specific crates blacklisted if known-problematic.
- **Duplicates:** warn on duplicate transitive dependencies.
- **Advisories:** same RustSec database as cargo-audit, configurable severity thresholds.

**`cargo-vet`** tracks human audit provenance. Each dependency is marked as audited or trusted. When a dependency updates, `cargo-vet` flags it for re-audit. AIOS imports audit records from trusted organizations (e.g., Google's published crate audits) to share the audit burden.

### 4.7 Formal Verification — TLA+ and Coq

Formal verification provides mathematical guarantees that static analysis tools cannot. Detailed verification targets are in [security.md](security.md) §8.3; this section describes the approach.

**TLA+ (Phase 13):** Model the capability state machine and IPC message passing as TLA+ specifications. Verify liveness (no permanent capability starvation) and safety (no capability escalation, no cross-address-space memory leak). TLA+ catches design flaws before they become code.

**Coq (Phase 24):** Prove that the Rust implementation of capability attenuation and provenance chain correctly implements the TLA+ specification. Coq proofs are machine-checked and unforgeable.

**Verus (potential):** An SMT-based verification framework that embeds proofs directly in Rust code. Used by the Asterinas OS project to verify page management. Verus could complement or replace Coq for Rust-native verification.

**Relationship to Kani:** Kani is bounded model checking (automated, finds bugs up to a bound). TLA+/Coq is unbounded formal verification (manual, proves correctness for all inputs). They are complementary.

### 4.8 Converos — OS Concurrency Model Checking

Converos (USENIX ATC 2025) is a practical model checker for verifying Rust OS concurrency patterns. Once the scheduler (Phase 3) and multi-core support are implemented, concurrency bugs become the dominant risk in kernel `unsafe` code.

**AIOS targets:** Lock ordering in the scheduler, IPC channel synchronization, and allocator concurrency paths. Converos verifies these modules for deadlock freedom and race-condition absence.

---

## 5. Agent-Side Static Analysis Deep Dive

### 5.1 Language-Specific Analysis

Agents may be written in multiple languages. Each has tailored analysis:

- **Rust agents:** Full Clippy + cargo-audit + cargo-deny pipeline. Any `unsafe` block is rejected — agents must use the SDK for all system interactions.
- **Python agents** (RustPython runtime): AST analysis rejects restricted modules (`os`, `sys`, `subprocess`, `ctypes`, `importlib`) and dangerous builtins (`eval`, `exec`, `compile`, `__import__`).
- **TypeScript agents** (QuickJS runtime): AST analysis rejects `eval`, `Function()` constructor, dynamic `import()`, and Node.js-specific APIs.

### 5.2 Capability Cross-Referencing

The audit tool performs static data-flow analysis — not just pattern matching — to trace capability usage through function calls, closures, and async boundaries. For each declared capability in the manifest, the tool identifies all code paths that exercise it:

- **Capabilities declared but unused:** flagged as over-provisioning (warning). Suggests removing from manifest.
- **Capabilities used but not declared:** flagged as under-declaration (error). The agent will fail at runtime; better to catch at audit time.

### 5.3 AIRS Code Review

AIRS performs an LLM-based semantic review that catches intent-level issues beyond pattern matching: data exfiltration encoded in benign-looking outputs, prompt injection susceptibility when processing untrusted content, and covert channels (timing-based or storage-based side channels). AIRS produces a risk level (Low/Medium/High/Critical) and a natural-language summary. Developers can appeal findings through the Agent Store review process.

Cross-reference: [security.md](security.md) §8.1 (example output), [agents.md](../applications/agents.md) (agent audit developer UX).

---

## 6. Phased Adoption Roadmap

Static analysis is adopted incrementally, aligned with the phase at which each subsystem and tool becomes relevant.

| Phase | Tools added | Targets |
|---|---|---|
| 0–2 | `rustc`, Clippy, `rustfmt`, cargo-audit, cargo-deny, Miri | `shared/` crate, boot code, allocators (host-testable logic) |
| 3–5 | Kani, Converos, cargo-vet | Syscall validation, capability operations, scheduler concurrency |
| 10–12 | `aios agent audit`, AIRS code review | Third-party agent manifests and code bundles |
| 13 | TLA+ models, Rudra full scans, Kani CI enforcement | Capability system, IPC protocol, all `unsafe` blocks |
| 24 | Coq / Verus proofs | Capability no-forge/no-escalate, provenance chain, W^X |

Cross-reference: [fuzzing-and-hardening.md](fuzzing-and-hardening.md) §4 for the parallel fuzzing adoption roadmap.

---

## 7. CI Integration Plan

| Job | Frequency | Tools | Blocks PR? |
|---|---|---|---|
| `just check` | Every commit | Clippy, rustfmt, cargo build | Yes |
| `just audit` | Every PR | cargo-audit, cargo-deny | Yes (High/Critical) |
| `just miri` | Nightly | Miri on `shared/` and host-testable modules | No (findings triaged) |
| `just kani` | Nightly (Phase 3+) | Kani proof harnesses | Yes for security modules (Phase 13+) |
| Rudra scan | Weekly (Phase 2+) | Rudra via Docker | No (findings triaged manually) |
| cargo-vet check | Every PR (Phase 2+) | cargo-vet | Yes (once fully adopted) |
| Agent audit | Agent Store submission | Full `aios agent audit` pipeline | Yes |

---

## 8. Tool Catalog

| Tool | Purpose | Domain | Target | Phase |
|---|---|---|---|---|
| `rustc` | Type, lifetime, and borrow checking | Kernel | All Rust code | 0+ |
| Clippy | Lint-based correctness and style | Kernel | All Rust code | 0+ |
| `rustfmt` | Formatting consistency | Kernel | All Rust code | 0+ |
| `cargo-audit` | CVE scanning of dependencies | Kernel | `Cargo.lock` | 0+ |
| `cargo-deny` | License, ban, duplicate, advisory policy | Kernel | `Cargo.lock` + `deny.toml` | 0+ |
| Miri | UB detection in `unsafe` via MIR interpretation | Kernel | `shared/`, host-testable modules | 0+ |
| Rudra | Panic safety, Send/Sync variance detection | Kernel | `unsafe` blocks | 2+ |
| `cargo-vet` | Human audit tracking for dependencies | Kernel | All dependencies | 2+ |
| Kani | Bit-precise bounded model checking | Kernel | Capability system, allocators, page tables | 3+ |
| Converos | OS concurrency model checking | Kernel | Scheduler, IPC, allocator concurrency | 3+ |
| TLA+ | Protocol-level specification and model checking | Kernel (design) | Capability state machine, IPC protocol | 13+ |
| Coq / Verus | Implementation-level formal proofs | Kernel | Capability derivation, provenance chain | 24 |
| `aios agent audit` | Pre-installation agent analysis | Agent | Agent code bundles | 10+ |
| AIRS code review | LLM-based semantic analysis | Agent | Agent code bundles | 10+ |

---

## 9. Relationship to Fuzzing

Static analysis and fuzzing form complementary layers of a defense-in-depth strategy:

| Layer | What it proves | Cost | Coverage |
|---|---|---|---|
| Static analysis | No known anti-patterns; type-safe; dependencies clean | Low (automated, compile-time) | All code, shallow depth |
| Fuzzing | No crashes on adversarial inputs | Medium (CI compute time) | Input boundaries, deep execution paths |
| Formal verification | Mathematical correctness of invariants | High (manual expert effort) | Critical subsystems, complete |

Static analysis catches bugs that fuzzing cannot find (type errors, license violations, unsafe anti-patterns). Fuzzing catches bugs that static analysis cannot find (input-dependent crashes, race conditions under specific timing). Formal verification proves properties that neither can guarantee. AIOS employs all three.

Cross-reference: [fuzzing-and-hardening.md](fuzzing-and-hardening.md) (companion deep-dive), [security.md](security.md) §8 (parent overview).
