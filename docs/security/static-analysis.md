# Static Analysis and Formal Verification

This document provides a technical deep-dive into how AIOS uses static analysis, model checking, and formal verification to prevent bugs before they reach runtime — both in kernel development and in agent pre-installation auditing.

For fuzzing and runtime hardening, see [fuzzing-and-hardening.md](fuzzing-and-hardening.md). For the security model overview and formal verification targets, see [security.md](security.md) §8.

---

## 1. Why Static Analysis Matters for a Kernel

**Static analysis** examines code without executing it, using techniques such as type checking, data-flow analysis, pattern matching on AST/MIR, bounded model checking, concurrency model checking, refinement types, and abstract interpretation to detect bugs, security vulnerabilities, and policy violations at compile time or CI time. For a kernel, every bug that reaches runtime is a potential privilege escalation. Static analysis is the first line of defense, complementing fuzzing (which finds runtime bugs on adversarial inputs) and formal verification (which proves absence of entire bug classes).

| Strategy | How it works | Best for |
|---|---|---|
| **Type-system enforcement** | Compiler rejects code violating type, lifetime, and borrow rules | Memory safety, data races, use-after-free |
| **Lint-based analysis** | Pattern-matching on AST/MIR for known anti-patterns | Style, correctness, performance pitfalls |
| **MIR interpretation** | Stepwise execution of Rust MIR under strict memory model rules | Undefined behavior in `unsafe` blocks |
| **Abstract interpretation** | Approximate execution over all possible inputs | Information flow, integer overflow, tag analysis |
| **Model checking** | Exhaustive exploration of bounded state space | Concurrency bugs, invariant violations, panic reachability |
| **Concurrency testing** | Systematic exploration of thread interleavings | Lock ordering, data races, deadlocks |
| **Refinement types** | Compile-time verification of value predicates via SMT | Numeric invariants, alignment, bounds |
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
| `unsafe` blocks | UB in code Miri cannot run (FFI, inline asm) | cargo-careful | 0+ |
| `unsafe` blocks | Panic safety, Send/Sync variance, higher-order invariants | Rudra | 2+ |
| `shared/` crate API | Breaking API changes between kernel and stub | cargo-semver-checks | 0+ |
| All dependencies | `unsafe` usage surface audit | cargo-geiger | 0+ |
| All dependencies | Known CVEs, yanked crates | `cargo-audit` | 0+ |
| All dependencies | License violations, banned crates, duplicates | `cargo-deny` | 0+ |
| All dependencies | Human audit provenance tracking | `cargo-vet` | 2+ |
| Host-testable test suite | Test suite effectiveness (mutation coverage) | cargo-mutants | 2+ |
| Capability system, allocators | State-space invariant violations, panic reachability | Kani model checker | 3+ |
| Scheduler, IPC, allocator concurrency | Lock ordering, deadlock, data races in unsafe concurrent code | Converos | 3+ |
| Scheduler, IPC, allocator concurrency | Exhaustive interleaving exploration | Loom | 3+ |
| Scheduler, IPC, allocator concurrency | Randomized scheduling for large state spaces | Shuttle | 3+ |
| All kernel code | AIOS-specific anti-patterns (MMIO without volatile, TTBR without barriers) | Semgrep (custom rules) | 3+ |
| Capability system, allocators | Capability flow tracking, integer overflow | MIRAI | 3+ |
| Capability system, IPC | Design-level correctness proofs | TLA+ / Coq | 13+ / 24 |
| Page table management | W^X enforcement proof | Kani / exhaustive path analysis | 13+ |
| Kernel numeric invariants | Compile-time value predicates (alignment, bounds) | Prusti / Flux | 13+ |
| `unsafe` abstractions | Implementation-level formal proofs | Verus / RefinedRust | 24 |

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

For example output and developer UX, see [security.md](security.md) §8.1 and [agents.md](../applications/agents.md). For expanded AI-assisted analysis techniques, see §10.

---

## 4. Kernel-Side Tool Deep Dives

### 4.1 Rust Compiler — Borrow Checker and Type System

The Rust compiler is AIOS's most powerful static analyzer. Ownership and borrowing eliminate buffer overflow (~35% of kernel CVEs), use-after-free (~20%), and uninitialized memory (~10%) at compile time. For the full breakdown, see [fuzzing-and-hardening.md](fuzzing-and-hardening.md) §3.1.

What remains are `unsafe` blocks, which AIOS requires for MMIO register access, inline assembly, raw pointer manipulation (page table walks), and system register access. Every `unsafe` block follows the documentation standard defined in `CLAUDE.md`: a `// SAFETY:` comment stating the invariant, who maintains it, and what happens if violated. These blocks are the primary target for all tools below.

The kernel targets enabling `#![forbid(unsafe_op_in_unsafe_fn)]` to require explicit `unsafe` blocks even inside `unsafe fn` signatures, ensuring no unsafe operation is invisible. This prevents the common anti-pattern where an `unsafe fn` contains dozens of lines of safe code with a single unsafe operation buried in the middle.

### 4.2 Clippy — Lint-Based Analysis

Clippy provides 750+ lints covering correctness, performance, and style. AIOS runs Clippy with `-D warnings` (deny all warnings), enforced in CI via `just check`.

Kernel-specific lints targeted for explicit enablement beyond the defaults:

| Lint | Why it matters for a kernel |
|---|---|
| `clippy::undocumented_unsafe_blocks` | Enforces the `// SAFETY:` standard at the compiler level |
| `clippy::missing_safety_doc` | Enforces safety docs on `unsafe fn` signatures |
| `clippy::cast_possible_truncation` | Critical for address arithmetic (u64 → u32 truncation) |
| `clippy::cast_sign_loss` | Prevents signed/unsigned confusion in register values |
| `clippy::indexing_slicing` | Prefer `.get()` to prevent panics in kernel code |

Integration: already active in `just clippy` (justfile), `just check`, and CI pipeline (`.github/workflows/ci.yml`). Custom AIOS-specific lint rules are additionally planned via Semgrep (see §4.14).

### 4.3 Miri — MIR Interpreter for Unsafe Code

Miri interprets Rust's Mid-level Intermediate Representation (MIR) under strict memory model rules, detecting undefined behavior that the compiler cannot catch statically:

- Aliasing violations (Stacked Borrows / Tree Borrows model)
- Alignment errors
- Use of uninitialized memory
- Out-of-bounds access
- Invalid enum discriminants
- Dangling references

**Aliasing model evolution.** Miri supports two aliasing models: the original Stacked Borrows and the newer Tree Borrows (`-Zmiri-tree-borrows`). Tree Borrows is more permissive for patterns common in OS `unsafe` code — notably, raw pointers derived from references that are then used alongside the original reference. AIOS targets running Miri with both models to catch the widest range of issues while avoiding false positives from Stacked Borrows' stricter rules.

**Weak memory emulation.** Miri supports weak memory model emulation (`-Zmiri-weak-memory-emulation`), detecting bugs that only manifest on ARM/RISC-V but not on x86. This is directly relevant to AIOS's aarch64 target, where weak memory ordering bugs are a real concern in the atomic operations used for inter-core synchronization.

**Limitation:** Miri cannot interpret inline assembly or MMIO operations. It targets the `shared/` crate and pure-Rust kernel logic extracted into host-testable functions. It cannot run the full `no_std` kernel binary. See §4.9 (cargo-careful) for a lightweight complement that handles code Miri cannot run.

**Configuration:** `MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-symbolic-alignment-check -Zmiri-weak-memory-emulation"` for maximum strictness. Run with `-Zmiri-tree-borrows` as a secondary pass to identify false positives in the Stacked Borrows run.

**Integration:** A `just miri` target running `cargo +nightly miri test -p shared` and any kernel modules with host-testable logic. Runs as a CI job.

### 4.4 Rudra — Unsafe Code Analyzer

Rudra is a research static analyzer from Georgia Tech (SOSP '21 Distinguished Artifact Award) that detects three bug patterns in `unsafe` Rust:

1. **Panic safety** — unwinding through `unsafe` code that has partially established invariants, leaving data structures in an inconsistent state.
2. **Send/Sync variance** — types manually implementing `Send` or `Sync` that violate thread-safety requirements.
3. **Higher-order invariant** — `unsafe` code relying on caller-provided closures or trait implementations to maintain invariants.

Rudra found 264 memory safety bugs across crates.io (76 CVEs, 112 RustSec advisories). For AIOS, any type marked `Send` or `Sync` for cross-core sharing and any `unsafe` block accepting closures or trait objects is a Rudra target.

**Maintenance status.** Rudra has seen minimal updates since the 2021 publication and is pinned to older Rust compiler internals. It may require effort to run on newer nightly versions. As a long-term mitigation, the three core analysis patterns (panic safety, Send/Sync variance, higher-order invariants) could be re-implemented as custom Clippy lints or Semgrep rules (§4.14) if Rudra becomes unmaintainable.

**Integration:** Periodic manual scans via Docker. Rudra is research-quality and not suitable for blocking CI, but results are triaged as high-priority findings.

### 4.5 Kani — Bit-Precise Model Checker

Kani translates Rust to CBMC and exhaustively explores all execution paths within bounded inputs. Unlike testing (which checks specific inputs) or fuzzing (which checks random inputs), Kani proves properties hold for **all** inputs up to a specified bound.

**Proven in practice:** AWS uses Kani to verify Firecracker VMM security boundaries and s2n-tls. The Rust standard library verification initiative uses Kani for core data structures — this effort has driven improvements in Kani's handling of generic code, trait objects, and complex type hierarchies, validating Kani's maturity for production use.

**Key capabilities:**

- **Contract-based verification:** `#[kani::requires]` and `#[kani::ensures]` enable modular verification — each function's proof obligations are checked independently without full program analysis, making verification of large codebases practical.
- **Stub/mock support:** `#[kani::stub]` allows replacing complex functions with simpler models during verification, isolating the property under test from unrelated complexity.
- **Concrete playback:** When Kani finds a counterexample, it generates a concrete test case that reproduces the bug, bridging model checking and unit testing.

**AIOS targets:**

| Target | Property to prove | Phase |
|---|---|---|
| Capability attenuation | `child.permissions ⊆ parent.permissions` for all derivations | 3+ |
| Buddy allocator | No double-free; all freed pages return to pool | 2+ |
| Page table flags | No PTE ever has both write and execute bits set (W^X) | 1+ |
| Address space isolation | `mapped_pages(p1) ∩ mapped_pages(p2) = ∅` absent explicit sharing | 3+ |

**Integration:** `#[kani::proof]` harnesses alongside unit tests. A `just kani` target runs `cargo kani` on annotated modules. Initially nightly CI; blocking for PRs touching security-critical code in Phase 13+.

### 4.6 Supply Chain Security

Five tools form a layered defense against dependency-related risks:

**`cargo-audit`** scans `Cargo.lock` against the RustSec Advisory Database. Any RustSec advisory (of any severity) blocks the PR under the current CI configuration. Runs on every CI build.

**`cargo-deny`** enforces broader policies via a committed `deny.toml`:

- **Licenses:** BSD-2-Clause, MIT, Apache-2.0, MPL-2.0 only — no GPL in `kernel/` or `shared/` (per `CLAUDE.md` crate rules).
- **Bans:** specific crates blacklisted if known-problematic.
- **Duplicates:** warn on duplicate transitive dependencies.
- **Advisories:** same RustSec database as cargo-audit; CI denies vulnerabilities at all severities (thresholds adjustable if policy changes).

**`cargo-vet`** tracks human audit provenance. Each dependency is marked as audited or trusted. When a dependency updates, `cargo-vet` flags it for re-audit. AIOS imports audit records from trusted organizations (e.g., Google's published crate audits) to share the audit burden.

**`cargo-geiger`** scans the dependency tree and counts `unsafe` usage in each crate. This quantifies the attack surface of the dependency graph — a crate with zero `unsafe` blocks is lower-risk than one with dozens. Combined with `cargo-vet`, this gives a complete picture of dependency risk: which dependencies use `unsafe` (geiger) and whether those `unsafe` blocks have been audited (vet).

**`cargo-semver-checks`** lints crate APIs for semver violations — removed public items, changed function signatures, altered type bounds. For AIOS, the `shared/` crate is the API boundary between kernel and uefi-stub; breaking changes to `BootInfo`, `RawMessage`, `Syscall` enum, etc. could silently break the stub. Running cargo-semver-checks on `shared/` in CI catches these before they reach integration testing. Note: cargo-semver-checks is integrated into `cargo` itself as an experimental feature since Rust 1.74 (`cargo publish --check-semver`).

### 4.7 Formal Verification — Verus, TLA+, Coq, and RefinedRust

Formal verification provides mathematical guarantees that static analysis tools cannot. Detailed verification targets are in [security.md](security.md) §8.3; this section describes the approach and tool selection.

**Verus (Phase 24 — primary recommendation).** Verus is an SMT-based verification framework that embeds proofs directly in Rust code. Pre/postconditions and invariants are written as Rust expressions, and Verus discharges proof obligations via the Z3 solver. Unlike Coq or Isabelle, proofs are mostly automated — the programmer writes specifications, not proof scripts.

Verus has been validated on real systems code:

- The Asterinas OS project uses Verus for verified page table management and introduced the "framekernel" pattern (§11.1).
- VeriSMo is a formally verified SMM monitor written in Rust and verified with Verus, demonstrating that Verus handles MMIO, page tables, and hardware register interactions — the same patterns AIOS uses.
- Verus's `tracked` and `ghost` types provide a natural way to express resource ownership proofs (e.g., "this page frame is owned by exactly one address space") with zero runtime cost.

**TLA+ (Phase 13).** Model the capability state machine and IPC message passing as TLA+ specifications. Verify liveness (no permanent capability starvation) and safety (no capability escalation, no cross-address-space memory leak). TLA+ catches design flaws before they become code. The Apalache model checker provides an alternative SMT-based backend to the standard TLC model checker, offering faster verification for certain spec patterns.

**Coq / RefinedRust (Phase 24).** RefinedRust (PLDI 2024, MPI-SWS) extends the RustBelt/Iris separation logic framework with semi-automated verification of `unsafe` Rust code. It is the most rigorous approach to proving soundness of `unsafe` abstractions — MMIO wrappers, page table manipulation, context switch code. RefinedRust's automation is significantly more practical than manual Coq proofs while maintaining the same level of rigor.

**Relationship to Kani:** Kani is bounded model checking (automated, finds bugs up to a bound). Verus/TLA+/Coq provide unbounded formal verification (proves correctness for all inputs). They are complementary — Kani is adopted earlier (Phase 3+) because it requires less expertise, while formal verification (Phase 13+/24) provides stronger guarantees for critical subsystems.

### 4.8 Converos — OS Concurrency Model Checking

Converos (USENIX ATC 2025) is a practical model checker for verifying Rust OS concurrency patterns. Once the scheduler (Phase 3) and multi-core support are established, concurrency bugs become the dominant risk in kernel `unsafe` code.

**AIOS targets:** Lock ordering in the scheduler, IPC channel synchronization, allocator concurrency paths, interrupt handler safety (timer tick handler re-entrancy), and per-CPU data access patterns (run queues, log rings, trace rings). Converos verifies these modules for deadlock freedom and race-condition absence.

**Kernel-specific capabilities:** Converos handles the complexity of OS concurrency patterns that general-purpose tools miss: interrupt disable as mutual exclusion, per-CPU data accessed without locks (safe only on the local core), and the interaction between spinlocks and interrupt masking. These patterns are pervasive in AIOS (e.g., `IN_SCHEDULER` guard, per-core `LogRing` and `TraceRing`).

### 4.9 cargo-careful — Lightweight UB Detection

cargo-careful runs Rust programs and tests with extra undefined behavior detection enabled — a lighter-weight complement to Miri for code that Miri cannot run (FFI, inline assembly paths). Created by Ralf Jung (Miri maintainer), it builds the standard library with debug assertions and extra checks enabled (`-Zextra-const-ub-checks`, debug assertions in `core`/`alloc`).

**Relevance to AIOS:** Miri cannot run the kernel binary (inline assembly, MMIO). cargo-careful can run host-side tests with extra UB checks that are cheaper than full Miri interpretation, catching a different subset of UB in `shared/` crate and any host-testable kernel logic.

**Integration:** A `just careful` target running `cargo +nightly careful test -p shared`. Runs alongside `just miri` in CI.

### 4.10 Concurrency Testing — Loom and Shuttle

Two tools complement Converos (§4.8) for concurrency verification, each with a more mature ecosystem:

**Loom** (Tokio project) systematically explores thread interleavings using a controlled scheduler. It provides exhaustive (within bounds) exploration of concurrent code paths, detecting lock ordering violations, data races, and deadlocks. Loom requires test harnesses that use `loom` types instead of `std` types, so it targets extracted concurrency logic rather than the full kernel.

**Shuttle** (AWS) provides randomized concurrency testing as a complement to Loom's exhaustive exploration. When the concurrent state space exceeds Loom's exhaustive exploration budget (common for scheduler and IPC interaction testing), Shuttle provides probabilistic coverage with significantly lower execution time. AWS uses it for verifying concurrent data structures in production services.

**Integration:** Loom and Shuttle harnesses in the test suite for scheduler, IPC, and allocator concurrency paths. Phase 3+ adoption alongside Converos. Loom for small state spaces (proof of correctness), Shuttle for large state spaces (probabilistic assurance).

### 4.11 Deductive Verification — Prusti, Creusot, and Flux

Three research tools offer deductive verification approaches that complement Verus and Kani:

**Prusti** (ETH Zurich, OOPSLA 2022) enables pre/postconditions and loop invariants as Rust attributes (`#[requires(...)]`, `#[ensures(...)]`), discharging verification conditions via the Viper/Z3 infrastructure. Prusti supports a significant subset of safe Rust and could verify pure-logic kernel functions — allocator arithmetic, capability permission checks, address translation math — without the full weight of Coq proofs. Annotations live in Rust source, reducing proof maintenance burden vs. external proof files. Limitation: limited `unsafe` support and no inline assembly.

**Creusot** (Inria, ICFEM 2022) translates Rust to WhyML (Why3 framework) with strong support for Rust's ownership semantics via a prophecy-based approach to mutable borrows. The Why3 backend provides flexibility in proof strategy (SMT solvers or interactive provers). Currently a research tool with a smaller community; monitor for maturity.

**Flux** (UC San Diego, 2023-2024) adds liquid/refinement types to Rust, allowing compile-time verification of value predicates (e.g., `i32{v: v > 0}`). Refinement types could express kernel invariants like "this address is page-aligned", "this capability permission set is a subset of parent", "this pool index is within bounds" — all checked at compile time with zero runtime cost. Currently an early research prototype; monitor for maturity.

**Integration:** Phase 13+ evaluation. Prusti is the most practical near-term; Flux has the highest potential payoff if it matures. All three are alternatives/complements to Verus for different verification needs.

### 4.12 Abstract Interpretation — MIRAI

MIRAI (Meta) is an abstract interpretation engine for Rust MIR that performs inter-procedural analysis to detect unreachable code, integer overflow, precondition violations, and information flow via tag analysis. The tag analysis capability is directly relevant to capability flow tracking — it could statically verify that capabilities don't flow to unauthorized code paths, and that kernel address arithmetic doesn't overflow.

MIRAI supports user-defined contracts via the `mirai_annotations` crate, enabling annotation of kernel invariants that are checked across function boundaries. Contract verification could annotate capability propagation rules at the type level.

**Maintenance note:** MIRAI was developed at Meta for the Libra/Diem blockchain project. Maintenance has slowed since 2023. Evaluate current status before adoption; if unmaintained, the capability flow analysis use case could be addressed by Semgrep custom rules (§4.14) or Kani contracts (§4.5).

**Integration:** Phase 3+ evaluation, contingent on maintenance status. Target: capability system and allocator integer arithmetic.

### 4.13 Test Quality — cargo-mutants

cargo-mutants performs mutation testing — automatically modifying code (replacing `+` with `-`, `true` with `false`, removing function calls) and checking if tests catch the mutations. This measures test suite effectiveness beyond code coverage: a test suite with 90% coverage but 50% mutation score is weaker than one with 80% coverage and 85% mutation score.

**Relevance to AIOS:** With 275+ tests in `shared/` and growing kernel host tests, mutation testing validates that the tests actually catch real bugs, not just achieve coverage. Particularly important for verifying that Kani proof harnesses and property tests are strong enough — a proof harness that doesn't actually exercise the property is worse than no proof harness (false confidence).

**Integration:** A `just mutants` target running `cargo mutants -p shared`. Periodic runs (weekly or per-milestone), not every PR — mutation testing is too slow for per-commit CI.

### 4.14 Custom Pattern Rules — Semgrep

Semgrep is a pattern-matching static analysis engine that supports custom rules written in YAML. Unlike Clippy (Rust-specific, AST-level), Semgrep works across languages and supports semantic patterns that span multiple lines and files.

**AIOS-specific rules:** Custom Semgrep rules encode kernel-specific anti-patterns that no general-purpose tool catches:

- MMIO register access without `read_volatile`/`write_volatile`
- TTBR write without subsequent `DSB` + `ISB` barrier sequence
- Capability check missing before IPC channel operation
- `spin::Mutex` usage in code paths reachable from Non-Cacheable memory (Phase 1 identity map)
- Interrupt-disabled critical section exceeding length threshold
- System register write without `ISB`

**Integration:** Phase 3+ CI integration. Rules committed to `tools/semgrep/` and run via `just semgrep`. Blocks PR if a rule fires. Rules are added incrementally as new kernel patterns emerge.

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

For expanded AI-assisted analysis techniques including multi-model consensus, LLM-guided fuzzing, and GNN vulnerability detection, see §10.

Cross-reference: [security.md](security.md) §8.1 (example output), [agents.md](../applications/agents.md) (agent audit developer UX).

---

## 6. Phased Adoption Roadmap

Static analysis is adopted incrementally, aligned with the phase at which each subsystem and tool becomes relevant.

| Phase | Tools added | Targets |
|---|---|---|
| 0–2 | `rustc`, Clippy, `rustfmt`, cargo-audit, cargo-deny, Miri, cargo-careful, cargo-geiger, cargo-semver-checks | `shared/` crate, boot code, allocators (host-testable logic), dependency audit |
| 2+ | cargo-vet, cargo-mutants, Rudra | Dependency provenance, test quality, `unsafe` patterns |
| 3–5 | Kani, Converos, Loom, Shuttle, Semgrep, MIRAI | Syscall validation, capability operations, scheduler concurrency, kernel patterns |
| 10–12 | `aios agent audit`, AIRS code review, AI-assisted techniques (§10) | Third-party agent manifests and code bundles |
| 13+ | TLA+ models, Rudra full scans, Kani CI enforcement, Prusti, Flux | Capability state machine, IPC protocol, all `unsafe` blocks, numeric invariants |
| 24 | Verus proofs, RefinedRust / Coq / Creusot proofs | Capability no-forge/no-escalate, provenance chain, W^X, unsafe abstraction soundness |

Cross-reference: [fuzzing-and-hardening.md](fuzzing-and-hardening.md) §4 for the parallel fuzzing adoption roadmap.

---

## 7. CI Integration Plan

| Job | Frequency | Tools | Blocks PR? |
|---|---|---|---|
| `just check` | Every commit | Clippy, rustfmt, cargo build | Yes |
| `just audit` | Every PR | cargo-audit | Yes (any severity) |
| `just deny` | Every PR | cargo-deny | Yes (any severity) |
| `just miri` | Every PR | Miri on `shared/` and host-testable modules | Yes |
| `just careful` | Every PR (Phase 0+) | cargo-careful on `shared/` | No (findings triaged) |
| `just semver` | Every PR (Phase 0+) | cargo-semver-checks on `shared/` | Yes |
| `just geiger` | Weekly (Phase 0+) | cargo-geiger dependency scan | No (report only) |
| `just kani` | Nightly (Phase 3+) | Kani proof harnesses | Yes for security modules (Phase 13+) |
| `just semgrep` | Every PR (Phase 3+) | Semgrep custom kernel rules | Yes |
| `just loom` | Nightly (Phase 3+) | Loom concurrency tests | No (findings triaged) |
| `just mutants` | Weekly (Phase 2+) | cargo-mutants on `shared/` | No (findings triaged) |
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
| cargo-careful | Lightweight UB detection for non-Miri code | Kernel | `shared/`, host-testable modules | 0+ |
| cargo-geiger | Dependency `unsafe` usage audit | Kernel | All dependencies | 0+ |
| cargo-semver-checks | API compatibility checking | Kernel | `shared/` crate | 0+ |
| Rudra | Panic safety, Send/Sync variance detection | Kernel | `unsafe` blocks | 2+ |
| `cargo-vet` | Human audit tracking for dependencies | Kernel | All dependencies | 2+ |
| cargo-mutants | Mutation testing for test quality | Kernel | Test suites | 2+ |
| Kani | Bit-precise bounded model checking | Kernel | Capability system, allocators, page tables | 3+ |
| Converos | OS concurrency model checking | Kernel | Scheduler, IPC, allocator concurrency | 3+ |
| Loom | Exhaustive thread interleaving exploration | Kernel | Concurrent data structures, lock ordering | 3+ |
| Shuttle | Randomized concurrency testing | Kernel | Large-state-space concurrent modules | 3+ |
| Semgrep | Custom kernel-specific lint rules | Kernel | All kernel code (AIOS-specific patterns) | 3+ |
| MIRAI | Abstract interpretation, tag analysis | Kernel | Capability flow, integer arithmetic | 3+ |
| Prusti | Deductive verification (pre/postconditions) | Kernel | Pure kernel logic, `shared/` crate | 13+ |
| Flux | Refinement type checking | Kernel | Numeric invariants, alignment, bounds | 13+ |
| TLA+ | Protocol-level specification and model checking | Kernel (design) | Capability state machine, IPC protocol | 13+ |
| Verus | SMT-based implementation verification | Kernel | Page tables, capability derivation, unsafe abstractions | 24 |
| RefinedRust | Separation-logic formal proofs of `unsafe` | Kernel | Unsafe abstraction soundness proofs | 24 |
| Coq | General-purpose interactive theorem prover | Kernel | Provenance chain, deep properties | 24 |
| Creusot | Deductive verification via Why3 | Kernel | Monitor for maturity | 24 |
| `aios agent audit` | Pre-installation agent analysis | Agent | Agent code bundles | 10+ |
| AIRS code review | LLM-based semantic analysis | Agent | Agent code bundles | 10+ |

---

## 9. Relationship to Fuzzing

Static analysis and fuzzing form complementary layers of a defense-in-depth strategy:

| Layer | What it proves | Cost | Coverage |
|---|---|---|---|
| Static analysis | No known anti-patterns; type-safe; dependencies clean | Low (automated, compile-time) | All code, shallow depth |
| Fuzzing | No crashes on adversarial inputs | Medium (CI compute time) | Input boundaries, deep execution paths |
| Formal verification | Mathematical correctness of invariants | High (manual expert effort, reducible via AI — see §10) | Critical subsystems, complete |

Static analysis catches bugs that fuzzing cannot find (type errors, license violations, unsafe anti-patterns). Fuzzing catches bugs that static analysis cannot find (input-dependent crashes, race conditions under specific timing). Formal verification proves properties that neither can guarantee. AIOS employs all three.

Cross-reference: [fuzzing-and-hardening.md](fuzzing-and-hardening.md) (companion deep-dive), [security.md](security.md) §8 (parent overview).

---

## 10. AI-Assisted Analysis

AI techniques augment traditional static analysis in two categories: AIRS-dependent techniques that require the full AI runtime (Phase 10+), and kernel-internal techniques that run in CI without AIRS.

### 10.1 AIRS-Dependent Techniques (Phase 10+)

These techniques require semantic understanding provided by the AIRS system and are primarily used for agent auditing.

**LLM code security review.** The existing AIRS code review (§5.3) analyzes agent code for behavioral threats. Key enhancements from industry practice:

- **Multi-model consensus:** Run multiple LLMs (e.g., different model sizes or providers) on the same agent code and vote on findings. This reduces false positives — a finding flagged by multiple models independently is higher confidence than one flagged by a single model.
- **Retrieval-augmented review:** Give the LLM access to the AIOS security policy, capability specification, and past audit findings for context-aware review that catches violations specific to AIOS's security model.
- **Fine-tuning on kernel-specific patterns:** Train or fine-tune models on known kernel vulnerability databases (Linux CVEs, Syzkaller findings) to improve detection of kernel-specific anti-patterns in agent syscall usage.

**LLM-guided fuzzing.** LLMs generate targeted fuzz inputs and harnesses based on code understanding. Rather than random mutation, the LLM reasons about code paths and generates inputs likely to trigger edge cases. Google's OSS-Fuzz integration with LLMs demonstrated 30%+ coverage improvement over traditional fuzzing. For AIOS, AIRS generates targeted fuzz harnesses for the syscall interface, IPC message parser, and agent manifest validator.

**GNN vulnerability detection.** Graph neural networks trained on code property graphs (AST + CFG + DFG) detect vulnerability patterns learned from CVE databases. Research tools (Devign, LineVul, ReVeal) show promise but have high false-positive rates (30-60%). Not suitable for blocking CI but useful for periodic deep scans of `unsafe` blocks, where findings are triaged by AIRS with human oversight.

### 10.2 Kernel-Internal ML (CI-Safe, No AIRS)

These techniques run without the AI runtime and can be integrated into CI pipelines directly.

**Custom Semgrep rules.** Described in §4.14. While not ML-based, Semgrep rules encode domain-specific patterns that capture the same class of bugs that simple ML classifiers target, without the false-positive overhead.

**LLM-assisted proof and harness generation.** LLMs generate Kani proof harnesses, Verus pre/postconditions, and TLA+ spec drafts from code context. Even imperfect LLM-generated proofs reduce expert review burden:

- Auto-generate Kani `#[kani::proof]` harnesses from function signatures and `// SAFETY:` comments.
- Auto-suggest Verus `requires`/`ensures` clauses from code patterns and documentation.
- Auto-generate TLA+ spec drafts from Rust module interfaces and IPC protocol definitions.

These can be generated offline (by any LLM, not AIRS) and committed as starting points for expert refinement. The LLM does the scaffolding; the expert verifies correctness. This addresses the main barrier to formal verification adoption — the cost of writing proofs from scratch.

---

## 11. Novel Architectural Patterns

Research and production OS projects have developed architectural patterns that strengthen static analysis effectiveness. These patterns influence AIOS's design rather than adding a tool.

### 11.1 Framekernel Pattern (Asterinas)

The Asterinas OS project introduced the "framekernel" architecture: all `unsafe` code is isolated into a verified "frame" layer, while the rest of the OS uses only safe Rust. The frame layer provides safe abstractions over hardware (MMIO, page tables, system registers), and formal verification (via Verus) proves these abstractions are sound. The safe kernel code above the frame layer is then protected by Rust's type system — no further verification needed.

**Relevance to AIOS:** AIOS already follows this pattern informally — `unsafe` blocks are concentrated in `kernel/src/arch/aarch64/` (MMIO, assembly, system registers) and `kernel/src/mm/` (page table manipulation). Formalizing this as an explicit framekernel boundary would make the verification scope clear: verify the frame layer (Phase 24), then the safe layer is guaranteed by construction.

### 11.2 Dual Aliasing Model Testing

The Rust aliasing model is still being finalized. Running Miri with both Stacked Borrows (default) and Tree Borrows (`-Zmiri-tree-borrows`) on the same code catches the widest range of issues:

- Stacked Borrows violations that are genuine bugs (caught by both models)
- Stacked Borrows violations that are false positives (caught only by Stacked Borrows, accepted by Tree Borrows)
- Patterns that both models reject (definitely UB)

For AIOS, the `unsafe` MMIO abstractions and page table code should pass both models. Patterns that only pass under Tree Borrows should be documented and monitored as the Rust aliasing model is finalized.

### 11.3 Ferrocene — Safety-Critical Rust

Ferrocene is a qualified Rust compiler toolchain for safety-critical systems (ISO 26262, IEC 61508), developed by Ferrous Systems. If AIOS ever targets safety-critical applications (automotive, medical, aerospace), Ferrocene provides the qualified compiler needed for certification. Ferrocene's formal specification of Rust semantics also informs AIOS's verification strategy — properties proven against Ferrocene's spec are guaranteed by a qualified compiler.

### 11.4 seL4 Rust Rewrite

The seL4 team is working on a Rust rewrite of the seL4 microkernel, aiming to maintain the formal verification guarantees of the original C implementation while leveraging Rust's memory safety. Their experience with what can and cannot be verified in Rust kernels is directly applicable to AIOS's Phase 24 verification strategy. Key lessons: Rust's ownership model helps with some proofs (resource management) but complicates others (interior mutability, shared state).

### 11.5 Rust for Linux — Custom Clippy Practices

The Rust-for-Linux project (merged into mainline Linux 6.1+) has developed practices for static analysis of Rust kernel code:

- Custom Clippy lints for kernel-specific patterns
- `#[vtable]` macro for safe C-Rust FFI boundaries
- Kernel-specific safety abstractions validated by the compiler

AIOS can adapt their custom Clippy lint approach for AIOS-specific rules, complementing the Semgrep rules in §4.14 with compiler-integrated checks.

---

## 12. Cross-References

| Topic | Document | Relevant Sections |
|---|---|---|
| Fuzzing deep dive | [fuzzing-and-hardening.md](fuzzing-and-hardening.md) | §2 attack surface, §4 fuzzing roadmap |
| Security model overview | [security.md](security.md) | §8 verification, §8.1 agent audit, §8.3 formal targets |
| Agent framework | [agents.md](../applications/agents.md) | Agent audit developer UX |
| AIRS architecture | [airs.md](../intelligence/airs.md) | AI-assisted analysis context |
| Capability system | [security.md](security.md) | §§2-3 capability model |
| Memory hardening | [fuzzing-and-hardening.md](fuzzing-and-hardening.md) | §3.3 W^X, guard pages, KASLR, PAC/BTI/MTE |
| IPC security | [ipc.md](../kernel/ipc.md) | §13 AI-native IPC, §14 future directions |
