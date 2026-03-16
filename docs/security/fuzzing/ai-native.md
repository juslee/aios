# Fuzzing: AI-Native Strategies

Part of: [fuzzing.md](../fuzzing.md) — Fuzzing and Input Hardening
**Related:** [strategies.md](./strategies.md) — Hardening strategies, [adoption-roadmap.md](./adoption-roadmap.md) — Phased adoption, [tooling.md](./tooling.md) — Tooling and catalog

---

## 7. AI-Native Fuzzing and Defense

AIOS is an AI-first operating system. This section describes how AI techniques improve both the development-time fuzzing process and the runtime defense posture. Techniques are categorized by their deployment model:

- **Development-time** — AI assists during CI/development; no runtime component
- **Kernel-internal** — frozen statistical models embedded in the kernel; no AIRS dependency
- **AIRS-dependent** — requires the AI Runtime Service for semantic understanding and adaptation

### 7.1 Development-Time AI Tools

Tools that assist kernel developers during implementation and CI but do not ship as runtime components.

#### 7.1.1 Automated Syscall Specification Generation

**KernelGPT** (Yang et al., ASPLOS 2025) uses LLMs to auto-generate syscall fuzzer specifications from kernel source code. Applied to Linux, it found 24 bugs (12 fixed, 11 CVEs) by generating specifications for 532 syscalls in 4.7 hours.

For AIOS, the same technique generates fuzzer descriptions for all 31 syscalls from `kernel/src/syscall/mod.rs` and `shared/src/syscall.rs`. The LLM extracts argument types (`ChannelId`, `SharedMemoryId`, capability handles), valid ranges, state preconditions, and expected error returns — eliminating manual specification writing.

**SyzForge** (2025) extends this with a 4-stage pipeline: static analysis → symbolic execution for argument constraints → fuzzing with coverage assessment → LLM-driven specification repair. This achieves 13.3% more coverage than KernelGPT alone and found 19 unreported vulnerabilities.

#### 7.1.2 Autonomous Fuzzing Agents

**SyzAgent** (2025) provides real-time LLM guidance during fuzzing campaigns. The LLM analyzes crash reports and coverage data mid-campaign and suggests targeted mutations: "this IPC sequence reached the priority inheritance path — try extending the chain to depth > `MAX_INHERITANCE_DEPTH=8`."

**FuzzGPT** (ISSTA 2023) mines historical bug-triggering code patterns and uses in-context learning to generate edge-case inputs targeting known vulnerability classes. For AIOS, this means mining crash logs to generate syscall sequences targeting specific patterns: use-after-free in channel cleanup, double-free in shared memory unmap, refcount underflow in capability revocation.

#### 7.1.3 Vulnerability Prediction

**Graph Neural Networks** (Springer 2024, Wiley STVR 2024) construct code property graphs from kernel source and predict vulnerability-prone functions. Applied to AIOS, GNN analysis models relationships between syscall handlers, capability checks, and lock acquisitions, predicting which functions (`cascade_revoke`, `process_exit` cleanup, `ipc_direct_switch`) are most likely to contain bugs.

This prioritizes fuzzing effort: allocate more cycles to high-risk functions (those with `unsafe` blocks, MMIO access, complex lock ordering) and fewer cycles to simple accessor functions.

#### 7.1.4 Intelligent Crash Deduplication

**GPTrace** (2025) uses LLM embeddings to semantically cluster crash stack traces, grouping crashes by root cause rather than surface-level stack similarity. This reduces triage overhead when continuous fuzzing produces hundreds of crash reports.

**ECHO** (MDPI 2024) provides lightweight call-stack-based deduplication using longest common subsequence between normalized kernel stack traces. For AIOS, stack traces are normalized by stripping KASLR offsets before comparison.

### 7.2 Kernel-Internal AI (Frozen Models, Phase 17+)

Lightweight inference models that run inside the kernel or as privileged system services for runtime anomaly detection. These do NOT depend on AIRS — they are frozen, small models loaded at boot and updated only through kernel upgrades.

#### 7.2.1 Syscall Anomaly Detection

A frozen CNN+LSTM model (Du et al., CCS 2017; CCSW 2024) monitors per-agent syscall patterns in real time. The CNN extracts spatial patterns from syscall argument distributions; the LSTM models temporal sequences.

**What it detects:**

- Rapid channel creation/destruction cycles (resource exhaustion attempt)
- Unusual capability query patterns followed by IPC floods (privilege escalation probe)
- Syscall argument distributions that deviate from the agent's established baseline (compromised agent)

**Model constraints:**

- Size target: <1 MB (fits in kernel pool on 2 GB devices)
- Inference: triggered per-syscall or sampled (e.g., every 100th syscall) depending on overhead budget
- Action: raises an anomaly flag in the process's `SchedEntity`, which feeds into AIRS behavioral assessment (§7.3) or triggers rate limiting if AIRS is unavailable

#### 7.2.2 Statistical Distribution Invariants

Lightweight statistical monitors that require no neural network — pure arithmetic on counters already maintained by the observability subsystem (`LogRing`, `TraceRing`).

**Monitored distributions:**

- Syscall frequency per agent: exponential moving average baseline; anomaly = deviation > 3σ
- IPC message rate per channel: baseline established during first 1000 messages
- Memory allocation rate per process: sudden spikes indicate potential resource exhaustion
- Lock hold times per lock: Z-score monitoring for contention anomalies

These monitors have zero ML inference cost — they use counters that the kernel already tracks for observability. The anomaly detection is a simple comparison against a running baseline.

#### 7.2.3 Learned Lock-Order Prediction

Train a small decision tree on observed lock acquisition sequences from `TraceRing` data. The model learns the documented lock order (PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE > CHANNEL_TABLE > SELECT_WAITERS > BLOCK_ENGINE > VIRTIO_BLK) and predicts whether a lock acquisition will violate the ordering.

**Deployment model:**

- Training: offline, on trace data collected from test runs
- Runtime: frozen decision tree, <10 KB, checked on lock acquisition in debug builds
- Production: disabled by default (overhead); enabled via kernel boot parameter for diagnostics

Cross-reference: [static-analysis.md](static-analysis.md) §4.8 (Converos for compile-time concurrency verification).

### 7.3 AIRS-Dependent Security (Phase 10+)

These features require the AI Runtime Service (AIRS) and are part of the broader intelligence layer. They represent the convergence of fuzzing insights and runtime AI capabilities.

#### 7.3.1 Continuous Vulnerability Analysis

**Big Sleep** (Google Project Zero, 2024) demonstrated that LLM agents can discover real-world vulnerabilities that traditional fuzzing misses — including an exploitable stack buffer underflow in SQLite. The agent navigates codebases, reasons about code semantics, and discovers logic bugs through understanding, not random mutation.

AIRS deploys a Big Sleep-style analysis agent that continuously reviews kernel code changes. The agent understands AIOS architecture — capability semantics, IPC state machines, lock ordering contracts — and identifies semantic vulnerabilities:

- Missing capability checks on IPC paths
- Lock ordering violations in new code
- Use-after-revoke patterns in capability cleanup
- Missing bounds checks in `unsafe` blocks

This complements Rudra (pattern-based) and Kani (bounded model checking) by catching intent-level bugs that no static tool can detect.

#### 7.3.2 Self-Healing Intrusion Detection

AIRS monitors anomaly flags from §7.2.1 and behavioral state from the context engine to implement graduated autonomous response:

1. **Rate limiting**: reduce suspicious agent's scheduling priority and IPC throughput
2. **Capability attenuation**: restrict the agent's capabilities to a safe subset via `cap_attenuate`
3. **Isolation**: suspend the agent's channels and shared memory mappings
4. **Termination**: invoke `process_exit` with cleanup of all kernel resources

The response is graduated — not binary. A slightly anomalous agent gets rate-limited; a clearly malicious agent gets terminated. The graduation threshold adapts based on the agent's trust level and behavioral history.

Cross-reference: [security.md](model.md) §2 (eight security layers), [airs.md](../../intelligence/airs.md) §5.9 (capability intelligence).

#### 7.3.3 LLM-Assisted Invariant Verification

LLMs generate Kani proof harnesses from natural-language invariant descriptions (Quokka, 2025; ASE 2024). A developer writes: "The buddy allocator never returns a page that overlaps with an already-allocated page." The LLM translates this into a Kani harness with appropriate bounds, the human reviews, and Kani proves it.

This reduces the manual effort barrier for expanding formal verification coverage. Combined with Rudra's pattern detection, it creates a pipeline:

```text
Rudra scan → find unsafe patterns → LLM generates proof harnesses → Kani proves → verified
```

Cross-reference: [static-analysis.md](static-analysis.md) §4.5 (Kani), §4.7 (formal verification).

---

## References

### Development-Time AI

- KernelGPT — Yang et al., "KernelGPT: Enhanced Kernel Fuzzing via Large Language Models" (ASPLOS 2025)
- SyzForge — Multi-stage specification pipeline (Springer 2025)
- SyzAgent — Real-time LLM-guided fuzzing (arXiv 2025)
- FuzzGPT — Bug-pattern mining with in-context learning (ISSTA 2023)
- ChatAFL — LLM-guided protocol fuzzing (NDSS 2024)
- MOCK — Context-aware syscall dependency mutation (NDSS 2024)
- T-Scheduler — Hyperparameter-free MAB seed scheduling (AsiaCCS 2024)
- Psyzkaller — N-gram syscall dependency mining (arXiv 2024)
- GPTrace — LLM-based crash deduplication (arXiv 2025)
- OZZ — Out-of-order concurrency bug detection (SOSP 2024, Best Paper)

### Kernel-Internal AI

- DeepLog — LSTM-based anomaly detection (Du et al., CCS 2017)
- CNN+LSTM syscall anomaly — CCSW 2024
- Statistical distribution invariants — VORTEX 2024

### AIRS-Dependent

- Big Sleep — LLM vulnerability discovery (Google Project Zero, 2024)
- Quokka — LLM invariant synthesis (arXiv 2025)
- LLM+BMC — Invariant generation and verification (ASE 2024)

### Rust-Specific

- Rudra — Ecosystem-scale unsafe Rust analysis (Bae et al., SOSP 2021)
- FourFuzz — Selective unsafe instrumentation (EASE 2025)
- deepSURF — LLM-generated fuzzing harnesses for Rust (IEEE S&P 2026)
- LibAFL — Modular fuzzing framework (Fioraldi et al., CCS 2022)
- Kani — Bit-precise bounded model checking (AWS)
- Loom — Exhaustive concurrency testing (tokio-rs)
- LACE — eBPF-powered controlled concurrency testing (arXiv 2025)

### Hardware-Assisted

- ARM MTE — Memory Tagging Extension (ARM whitepaper)
- TIKTAG — MTE tag leakage via speculation (S&P 2025)
- ARM CoreSight ETM — Hardware execution tracing (Ricerca Security; ARMOR, IEEE TIFS 2024)
- Snapchange — Snapshot-based QEMU fuzzing (AWS, 2023+)
