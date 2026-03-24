# AIOS Development Plan

## Timeline, Risks, Dependencies, and Decision Gates

**Parent document:** [overview.md](./overview.md)
**Related:** All architecture documents

-----

## 1. Overview

47 phases across 8 tiers. Each tier delivers a usable milestone, building on the previous one to produce something demonstrable. Timeline estimates use a **graduated complexity model** based on observed AI-assisted development velocity from Phases 0–3 (see §8.1 for methodology).

| Estimate Model | Total Duration | Assumption |
|---|---|---|
| AI-assisted (current) | ~55–75 weeks | Solo developer with Claude Code, architecture-first workflow |
| Unassisted baseline | ~220 weeks (~4.2 years) | Solo experienced systems programmer, no AI tooling |

```mermaid
flowchart LR
    T1["`**Tier 1**: Hardware Foundation
    Phases 0-3 | Weeks 1-2 ✅
    *Boot, memory, IPC*`"]
    T1a["`**Kit Foundation**
    Phases 4-5 | Weeks 3-4
    *Storage, Kit API extraction*`"]
    T2["`**Tier 2**: Core System Services
    Phases 6-9 | Weeks 5-11
    *GPU, compositor, input, networking*`"]
    T3["`**Tier 3**: AI & Intelligence
    Phases 10-19 | Weeks 12-23
    *AIRS, search, context, preferences, agents, flow, conversation, identity*`"]
    T4["`**Tier 4**: Platform Maturity
    Phases 20-27 | Weeks 24-34
    *Intent, security, compute, performance, SDK, POSIX*`"]
    T5["`**Tier 5**: Hardware & Connectivity
    Phases 28-33 | Weeks 35-44
    *NTM, USB, wireless, power, thermal*`"]
    T6["`**Tier 6**: Rich Experience
    Phases 34-38 | Weeks 45-56
    *UI toolkit, browser, media, camera, a11y*`"]
    T7["`**Tier 7**: Production OS
    Phases 39-44 | Weeks 57-68
    *Secure boot, Linux compat, multi-device, launch*`"]
    T8["`**Tier 8**: Security Intelligence
    Phases 45-46 | Weeks 69-72
    *Capability profiles, AIRS cap intelligence*`"]

    T1 --> T1a --> T2 --> T3 --> T4 --> T5 --> T6 --> T7 --> T8

    style T1 fill:#228B22,color:white
```

> **Note:** Week ranges assume continued AI-assisted development. Multiply by 3–5x for unassisted solo development. Later tiers have wider ranges because external bridge integration (Servo bridge, candle bridge, hardware drivers) has lower AI leverage than kernel-internal work.

-----

## 2. Tier Milestones

Each tier produces a demonstrable result:

| Tier | Target | Milestone | Demo |
|---|---|---|---|
| 1 | Week 2 | Microkernel boots on QEMU | UART output, memory allocation, IPC ping-pong benchmark |
| 1.5 | Week 4 | Kit Foundation | Storage complete, Kit API traits extracted for Phases 2-4, Kit crate structure established |
| 2 | Week 11 | Graphical desktop with shell | Window compositor, terminal emulator, basic networking (curl works) |
| 3 | Week 23 | AI-enhanced OS | Conversation bar, semantic search, agents running with capability gates, Flow service, identity & credentials |
| 4 | Week 34 | Developer platform | Intent verification, SDK published, compute abstraction, GPU compute, security hardened, POSIX tools |
| 5 | Week 44 | Hardware-ready OS | WiFi, Bluetooth, USB, power management, thermal — runs on Raspberry Pi |
| 6 | Week 56 | Daily-driver OS | Web browser, media player, camera, accessibility, internationalization |
| 7 | Week 68 | Production OS | Secure boot, Linux app compat, enterprise features, real hardware launch |
| 8 | Week 72 | Security Intelligence | Composable capability profiles, AIRS-powered agent audit |

-----

## 3. Phase Dependencies

```mermaid
flowchart TD
    P0["Phase 0: Foundation & Tooling"] --> P1["Phase 1: Boot & First Pixels"]
    P1 --> P2["Phase 2: Memory Management"]
    P2 --> P3["Phase 3: IPC & Capability System"]
    P3 --> P4["Phase 4: Block Storage & Object Store"]
    P4 --> P5["Phase 5: Kit Foundation"]
    P5 --> P6["Phase 6: GPU & Display"]
    P6 --> P7["Phase 7: Window Compositor & Shell"]
    P7 --> P8["Phase 8: Input & Terminal"]
    P8 --> P9["Phase 9: Basic Networking"]

    P9 --> P10["Phase 10: Agent Framework"]
    P9 --> P11["Phase 11: AIRS Inference Engine"]
    P11 --> P12["Phase 12: AIRS Intelligence Services"]
    P12 --> P13["Phase 13: Space Indexer & Search"]
    P12 --> P14["Phase 14: Context Engine"]
    P14 --> P15["Phase 15: Preference System"]
    P15 --> P16["Phase 16: Flow & Translation"]
    P7 --> P16
    P16 --> P17["Phase 17: Attention, Task Manager & Notification"]
    P13 --> P18["Phase 18: Conversation Manager"]
    P14 --> P18
    P17 --> P18
    P10 --> P19["Phase 19: Identity & Credentials"]

    P19 --> P20["Phase 20: Intent Verification"]
    P12 --> P20
    P20 --> P21["Phase 21: Security Architecture"]
    P4 --> P21
    P21 --> P22["Phase 22: Security Testing & Verification"]

    P12 --> P23["Phase 23: Kernel Compute Abstraction"]
    P6 --> P23
    P23 --> P24["Phase 24: GPU Compute & Accelerator Drivers"]
    P6 --> P24
    P23 --> P25["Phase 25: Performance & Optimization"]
    P7 --> P25
    P25 --> P27["Phase 27: POSIX Compatibility & BSD Userland"]

    P10 --> P26["Phase 26: Developer Experience & SDK"]
    P22 --> P26

    P9 --> P28["Phase 28: Network Translation Module"]
    P28 --> P29["Phase 29: USB Stack & Hotplug"]
    P29 --> P30["Phase 30: WiFi Stack"]
    P30 --> P31["Phase 31: Bluetooth & Wireless Integration"]
    P31 --> P32["Phase 32: Power Management"]
    P32 --> P33["Phase 33: Thermal Management"]
    P23 --> P33

    P26 --> P34["Phase 34: Interface Kit"]
    P34 --> P35["Phase 35: Browser Kit"]
    P24 --> P36["Phase 36: Media Pipeline & Codecs"]
    P11 --> P36
    P36 --> P37["Phase 37: Streaming, RTC & Camera"]
    P29 --> P37
    P37 --> P38["Phase 38: Accessibility & Internationalization"]

    P33 --> P39["Phase 39: Secure Boot & Update System"]
    P39 --> P40["Phase 40: Linux Binary Loader"]
    P40 --> P41["Phase 41: Wayland Bridge & Sandbox"]
    P41 --> P42["Phase 42: Multi-Device Sync & Pairing"]
    P42 --> P43["Phase 43: Enterprise Management"]
    P24 --> P44["Phase 44: Real Hardware, Certification & Launch"]
    P43 --> P44

    P3 --> P45["Phase 45: Composable Capability Profiles"]
    P10 --> P45
    P26 --> P45
    P45 --> P46["Phase 46: AIRS Capability Intelligence"]
    P12 --> P46
    P21 --> P46

    style P0 fill:#228B22,color:white
    style P1 fill:#228B22,color:white
    style P2 fill:#228B22,color:white
    style P3 fill:#228B22,color:white
    style P4 fill:#228B22,color:white
    style P5 fill:#228B22,color:white
```

**Critical path:** 0 → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 26 → 34 → 35. Browser Kit (Phase 35) is the last item on the critical path before the OS can be someone's daily driver. Every phase on the critical path is a potential bottleneck.

Note: Phase 27 (POSIX/BSD Userland) is not on the critical path because the Daily Driver gate (Gate 3, after Phase 35) depends on the Interface Kit + Browser Kit chain (26 → 34 → 35), not POSIX tools. Phase 27 is a parallel workstream that enhances the developer experience but is not a prerequisite for the Gate 3 decision.

**Parallel workstreams:** Several phase chains can proceed concurrently once their dependencies are met:

- **Compute chain:** 23 → 24 → 36 (kernel compute abstraction → GPU compute → media pipeline)
- **Connectivity chain:** 28 → 29 → 30 → 31 → 32 → 33 (networking → USB → wireless → power → thermal)
- **Security chain:** 21 → 22 (security architecture → security testing)
- **Intelligence chain:** 14 → 15 (context engine → preferences)
- **Search/Conversation chain:** 13 → 18 (search → conversation manager)

-----

## 4. Risk Register

### 4.1 Technical Risks

| Risk | Impact | Likelihood | Status | Mitigation |
|---|---|---|---|---|
| IPC performance < 5μs target | High — microkernel viability | Medium | **Resolved** — avg=4μs, p99=8μs (Gate 1 passed) | Prototype IPC in Phase 3, benchmark before proceeding. Fallback: hybrid kernel with in-kernel filesystem |
| Inference bridge performance (candle default, GGML alternative) | High — AI features unusable | Low | Open | candle is pure Rust on aarch64; GGML alternative bridge available via FFI if hand-tuned NEON required. Mitigation: quantization, smaller models, NPU offload |
| Engine bridge complexity (Servo default, Gecko/Blink via Linux compat) | High — browser delayed | Medium | Open | Servo is modular but massive (1.5M+ LoC). Firefox/Chrome available via Linux compat layer as alternative. Mitigation: start integration early (Phase 35 prep in Phase 26), maintain Servo fork |
| smoltcp limitations | Medium — networking incomplete | Low | Open | smoltcp handles TCP/UDP well. Missing: advanced congestion control, some edge cases. Mitigation: contribute upstream, fork if needed |
| GPU driver complexity (real hardware) | High — no display on Pi | Medium | Open | VirtIO-GPU works in QEMU. Pi GPU (VC4/V3D) has open-source driver but is complex. Mitigation: start with framebuffer fallback |
| Memory pressure on 2GB devices | Medium — degraded experience | Medium | Open | Models need 4+ GB RAM. Mitigation: aggressive quantization, model eviction, swap space, 4GB minimum recommended |
| Firmware blob licensing | Medium — WiFi/BT unusable | Low | Open | Most WiFi/BT chips need proprietary firmware. Mitigation: redistribute under manufacturer license (standard practice), document clearly |
| Kernel memory safety despite unsafe Rust | High — security compromise | Low-Medium | Mitigated | 14.6K lines of kernel code with all `unsafe` blocks documented. 275 unit tests, zero memory corruption observed. Ongoing: minimize unsafe, fuzz all syscalls, formal verification for critical paths |
| AI tooling dependency | Medium — velocity drops 3–5x | Low | Open | Development velocity depends on Claude Code. Mitigation: 58K lines of architecture docs serve as specification regardless of tooling; any developer can continue from current state |
| QEMU version compatibility | Low — CI breaks | Low | Open | QEMU 10.x changed edk2 MMIO mapping behavior post-ExitBootServices (discovered Phase 4 M13). Mitigation: pin QEMU version in CI (pending), test against multiple versions |
| Driver ecosystem time sink | High — hardware phases delayed | High | Open | Every comparable OS project (seL4, Fuchsia, Redox, Hubris) reports drivers as the single biggest time sink. Real hardware (Pi GPU, USB, WiFi) has undocumented quirks that resist AI acceleration. Mitigation: budget 2–3x estimated time for Phases 28–33 and 44; start with QEMU virtual devices before real hardware |

### 4.2 Schedule Risks

| Risk | Impact | Status | Mitigation |
|---|---|---|---|
| Phase 3 (IPC) takes longer than 6 weeks | Delays everything downstream | **Resolved** — completed in 3 days | IPC was flagged as the hardest kernel component. Architecture-first approach eliminated design uncertainty. |
| Phase 11 (AIRS) underestimated | AI features delayed | Open | candle bridge integration is well-understood (pure Rust). Context management and indexing are the unknowns. |
| Phase 35 (Browser) underestimated | Daily-driver delayed | Open | Servo bridge integration is the highest-risk single phase. External codebase integration has the lowest AI leverage. Budget extra slack. |
| External dependency integration ceiling | Later phases slow down | Open | Kernel-internal phases (0–3) benefit most from AI assistance (10–45x speedup). External library integration phases (6, 8, 28, 35) involve coordination costs that resist acceleration. Expect 2–4x speedup at best. |
| Scope creep in any phase | Timeline extends | Open | Each phase has a strict deliverable. Feature requests go to future phases. |

### 4.3 Ecosystem Risks

| Risk | Impact | Mitigation |
|---|---|---|
| No developers build agents | Empty ecosystem | App Ecosystem Tier 2 (web apps, see architecture.md §8) covers most use cases. Build compelling demo agents. |
| Model quality/size tradeoff | Poor AI experience | Model ecosystem is improving rapidly. Today's 8B models were impossible 2 years ago. |
| Hardware vendor engagement | No partner hardware | Pi and QEMU are sufficient for years. Pine64 is developer-friendly. |

-----

## 5. Decision Gates

Major decisions that must be made during development:

### Gate 1: Kernel Architecture (after Phase 3)

**Decision:** Is IPC performance acceptable? Does the microkernel architecture work?

**Criteria:**

- IPC round-trip < 10 μs (target: < 5 μs). Note: architecture.md §6.9 lists the optimized target (< 5 μs); this gate uses the acceptance threshold (< 10 μs) that determines whether the architecture is viable at all.
- Context switch < 20 μs (gate threshold). Note: architecture.md §6.9 lists the optimized target (< 10 μs); this gate uses a relaxed threshold since post-Phase 3 optimization (Phase 25) has not yet occurred.
- No pathological performance cliffs

**If NO:** Consider hybrid kernel (move Space Storage into kernel space). This is a significant architectural change but recoverable at this stage because no external users depend on the IPC interface yet, and only kernel and storage code has been written.

#### Gate 1 Retrospective (Phase 3 M12, 2026-03-13)

**Result: PASS** — microkernel architecture confirmed viable. Hybrid kernel fallback is not needed.

| Metric | Gate Threshold | Optimized Target | Actual (avg) | Actual (p99) |
|---|---|---|---|---|
| IPC round-trip (same core) | < 10 μs | < 5 μs | 4 μs | 8 μs |
| Context switch | < 20 μs | < 10 μs | 2 μs | 5 μs |
| Shared memory throughput (write) | — | — | 569 MB/s | — |
| Shared memory throughput (read) | — | — | 298 MB/s | — |

**Key findings:**

- IPC avg (4 μs) meets the *optimized* target (< 5 μs) before Phase 25 optimization, indicating headroom for capability check overhead, auditing, and cross-core IPC.
- Context switch avg (2 μs) is 5x below the optimized target, suggesting the save/restore path is well-optimized.
- Direct switch fast path (bypassing the scheduler when the receiver is already waiting) is the primary IPC optimization — co-locating caller and receiver on the same core enables sub-5μs round-trips.
- P99 latencies (8 μs IPC, 5 μs context switch) indicate occasional scheduling jitter but no pathological cliffs.

**Benchmark methodology:** 10,000 IPC iterations and 1,000 context switch iterations on QEMU virt (cortex-a72, 4 cores, 2 GiB RAM). Timer resolution: 16 ns (62.5 MHz CNTFRQ_EL0). IRQs masked during measurement to prevent timer preemption skew. Server and client co-located on CPU 0 for direct switch path. Production latencies with unmasked IRQs will be slightly higher.

### Gate 2: AI Viability (after Phase 12)

**Decision:** Can we run useful LLM inference on target hardware?

**Criteria:**

- 7B model runs at > 5 tokens/second on Pi 4 (4GB)
- Time to first token < 2 seconds (gate threshold). Note: architecture.md §6.9 lists the optimized target (< 500 ms); this gate uses a relaxed threshold since pre-optimization hardware (Pi 4 on SD card) has higher latency than the final target.
- Memory usage within budget (leaves >1 GB for OS + apps)

**If NO:** Scale down AI features. Use smaller models (1-3B). Focus on embedding/classification rather than generation. Conversation bar becomes a search interface rather than a conversational one.

**Industry context:** The emerging standard (Apple Intelligence, Copilot+ PCs) is hybrid on-device/cloud — small models (1–3B) run locally for low-latency tasks while larger models are accessed via cloud fallback. AIRS should plan for this hybrid architecture regardless of Gate 2 outcome. On-device-only inference remains the primary target for privacy and offline capability, with cloud as an opt-in enhancement.

### Gate 3: Daily Driver (after Phase 35)

**Decision:** Can someone use AIOS as their only computer for basic tasks?

**Criteria:**

- Web browsing works (Gmail, YouTube, basic sites)
- Terminal works (development possible)
- Files are accessible (spaces + POSIX bridge)
- System is stable (no crashes in 8-hour session)

**If NO:** Identify blocking issues and allocate additional time before Tier 7.

### Gate 4: Intelligence Stack (after Phase 19)

**Decision:** Is the full intelligence stack operational end-to-end?

**Criteria:**

- Conversation bar invokes semantic search across spaces
- Agents execute with intent verification and capability gates
- Identity system issues PQC keys and signs agent manifests
- Context engine drives preference resolution and attention triage

**If NO:** Intelligence subsystems may need tighter integration. Review cross-service interfaces (Search ↔ Context ↔ Conversation ↔ Attention) before proceeding to security hardening.

### Gate 5: Hardware-Ready (after Phase 33)

**Decision:** Does AIOS run on real hardware with wireless connectivity and thermal management?

**Criteria:**

- Boots on Raspberry Pi 4/5 with WiFi and Bluetooth functional
- USB peripherals (keyboard, mouse, storage) work via hotplug
- Thermal throttling prevents overheating under sustained load
- Power management supports sleep/wake cycles

**If NO:** Identify hardware-specific issues. Budget additional time for driver debugging before Tier 6 (which assumes stable hardware).

-----

## 6. Staffing Model

### AI-Assisted Solo Developer (Current)

All 47 phases are being implemented by a single developer using AI-assisted development (Claude Code):

- Phases 0–3 completed in 11 calendar days (planned: 16 weeks)
- Observed speedup over unassisted baseline: 5–45x depending on phase complexity
- Average throughput: ~1 phase per 2–3 days for kernel-internal work
- 14.6K lines of kernel code + 275 unit tests in 11 days

**Why AI assistance is effective for this project:**

- 58K lines of architecture docs written before code, providing detailed specifications
- Phase docs with atomic steps and mechanical acceptance criteria
- Rust's type system catches errors at compile time, reducing debug cycles
- QEMU provides deterministic test environment with fast feedback loops
- Architecture docs serve as shared context between developer and AI

**Graduated complexity model** — later tiers will have lower AI leverage:

| AI Leverage | Tiers | Speedup | Reason |
|---|---|---|---|
| High | 1–2 (Phases 0–9) | 10–20x | Pure kernel code, well-specified by architecture docs |
| Medium | 3–4 (Phases 10–27) | 3–8x | External library integration (candle bridge, smoltcp), compute abstraction. AI helps with glue code and testing. |
| Lower | 5–7 (Phases 28–44) | 2–4x | Hardware drivers, engine bridges (Servo), certification. External constraints dominate. |

### Solo Developer (Unassisted Baseline)

Without AI tools, the 47 phases would take ~220 weeks (~4.2 years) for an experienced systems programmer working full-time. This remains the reference estimate for unassisted development.

### Small Team (Accelerated)

With 2–3 AI-assisted developers, phases can be parallelized:

- Developer A: kernel (Phases 0–3), then compute abstraction (23–24), then performance (25), then security (21–22)
- Developer B: storage + GPU (Phases 4–6), then input/networking (8–9), then UI toolkit (34), then browser (35) — ordered to respect dependency chain (Phase 26 → 34 → 35)
- Developer C: AI (Phases 10–19), then networking (28), then agent ecosystem (26)
- Note: Remaining phases (27, 29–33, 36–44) are assigned based on availability as earlier phases complete.

Estimated timeline with 3 AI-assisted developers: ~15–25 weeks.

-----

## 7. Technology Stack Summary

| Layer | Technology | License | Verified |
|---|---|---|---|
| Language | Rust | MIT/Apache-2.0 | Phases 0–4 |
| Build system | just + cargo | MIT | Phases 0–4 |
| Bootloader | UEFI (custom) | BSD-2-Clause | Phase 1 |
| Synchronization | spin (Mutex, Once) | MIT | Phase 1+ |
| Device tree | fdt-parser | MIT | Phase 1+ |
| Cryptographic hash | sha2 (SHA-256, no_std) | MIT/Apache-2.0 | Phase 4 |
| Authenticated encryption | aes-gcm (AES-256-GCM, no_std) | MIT/Apache-2.0 | Phase 4 |
| Compression | lz4_flex (LZ4, no_std) | MIT | Phase 4 (planned) |
| TCP/IP | smoltcp | BSD-2-Clause | — |
| TLS | rustls | Apache-2.0/MIT | — |
| HTTP | h2, hyper | MIT | — |
| QUIC | quinn | Apache-2.0/MIT | — |
| DNS | hickory-dns | Apache-2.0/MIT | — |
| GPU | wgpu | Apache-2.0/MIT | — |
| UI toolkit | Interface Kit (iced bridge) | MIT (iced) | — |
| Font rendering | fontdue or ab_glyph | MIT | — |
| Browser engine | Browser Kit (Servo bridge) | MPL-2.0 (Servo) | — |
| AI inference | Compute Kit Tier 3 (candle bridge) | MIT/Apache-2.0 (candle) | — |
| Model format | GGUF | MIT | — |
| C library | musl | MIT | — |
| Userland tools | FreeBSD | BSD-2-Clause | — |
| Shell | FreeBSD /bin/sh | BSD-2-Clause | — |
| Compiler | LLVM/clang | Apache-2.0 | — |
| Certificates | webpki-roots | MPL-2.0 | — |

Mostly permissively licensed (MIT, Apache-2.0, BSD-2-Clause) with some MPL-2.0 dependencies (Servo bridge, webpki-roots). No GPL dependencies in the core OS.

-----

## 7.1 Target Application: OpenFang

[OpenFang](https://github.com/RightNow-AI/openfang) is an open-source Agent Operating System built in Rust (MIT/Apache-2.0, 137K lines, 14 crates). It provides autonomous agent orchestration, 40 channel adapters, 53 built-in tools, 27 LLM providers, and 16 security layers — all in a single ~32MB binary. AIOS adopts OpenFang as a first-class target application starting at Phase 10.

### Why OpenFang

Rather than reinvent agent orchestration, channel adapters, and LLM routing, AIOS provides the kernel-level primitives (capability isolation, hardware-enforced sandboxing, kernel-scheduled inference) and lets OpenFang provide the userspace agent runtime. This gives AIOS a production-tested agent ecosystem from day one.

### Integration Points by Phase

| Phase | Integration | What AIOS Provides | What OpenFang Provides |
|---|---|---|---|
| 11 (AIRS Inference Engine) | LLM routing compatibility | Kernel-level inference with hardware scheduling | Model routing patterns, cost-aware metering, 27 provider configs |
| 10 (Agent Framework) | Hand → AIOS agent mapping | AgentManifest with scheduled execution support | HAND.toml manifest format as candidate packaging standard, 7 bundled Hands |
| 10 (Agent Framework) | Agent-to-Agent protocol | IPC channels with capability gates | A2A + OFP protocol patterns for inter-agent delegation |
| 21 (Security Architecture) | Security layer mapping | Hardware capability tokens, MMU isolation | Taint tracking patterns, Merkle audit chain, prompt injection scanning |
| 27 (POSIX Compatibility) | Full binary compatibility | POSIX syscalls, musl libc | Unmodified OpenFang binary runs as AIOS process |
| 28 (Network Translation Module) | Channel adapter support | TCP/IP + TLS stack | 40 channel adapters (Telegram, Discord, Slack, etc.) |

### OpenFang Concepts Adopted into AIOS

- **Scheduled autonomous agents (Hands):** AIOS AgentManifest gains `schedule` field for cron-like autonomous execution — agents that wake, perform work, and sleep without user prompting. Modeled after OpenFang's HAND.toml.
- **HAND.toml as candidate manifest format:** The AIOS `manifest.toml` format references OpenFang's HAND.toml for the `[agent.schedule]`, `[[agent.approval_gates]]`, and `[[agent.dashboard_metrics]]` sections. See [agents.md §2.4](../applications/agents.md).
- **Cost-aware inference metering:** AIRS tracks per-model token costs and enforces budgets, inspired by OpenFang's per-model cost tracking and GCRA rate limiting. See [airs.md](../intelligence/airs.md).
- **Information flow taint tracking:** The capability system labels secret data at introduction and tracks propagation, inspired by OpenFang's taint tracking system.
- **Merkle hash-chain audit trail:** Every capability invocation is cryptographically chained for tamper-evident audit logs, inspired by OpenFang's audit system.

-----

## 8. Phase Detail Reference

Each phase has an implementation doc in `docs/phases/` containing objectives, milestone steps with acceptance criteria, decision points, and references to the existing architecture documents (which hold the technical design). This avoids duplicating architecture content while providing a clear implementation sequence.

| Phase | Name | Kit(s) | Arch Docs | Document | Status | Planned | Actual |
|---|---|---|---|---|---|---|---|
| 0 | Foundation & Tooling | — | boot.md | [`00-foundation-and-tooling.md`](../phases/00-foundation-and-tooling.md) | Complete | 2w | 2.5d |
| 1 | Boot & First Pixels | — | boot.md, hal.md | [`01-boot-and-first-pixels.md`](../phases/01-boot-and-first-pixels.md) | Complete | 4w | 1.5d |
| 2 | Memory Management | Memory Kit | memory.md (5) | [`02-memory-management.md`](../phases/02-memory-management.md) | Complete | 4w | 5d |
| 3 | IPC & Capability System | IPC Kit, Capability Kit | ipc.md, scheduler.md | [`03-ipc-and-capability-system.md`](../phases/03-ipc-and-capability-system.md) | Complete | 6w | 3d |
| 4 | Block Storage & Object Store | Storage Kit | spaces.md (8) | [`04-block-storage-and-object-store.md`](../phases/04-block-storage-and-object-store.md) | Complete | 5w | — |
| 5 | Kit Foundation | Memory Kit, IPC Kit, Capability Kit, Storage Kit | — | [`05-kit-foundation.md`](../phases/05-kit-foundation.md) | Complete | 3w | — |
| 6 | GPU & Display | Compute Kit (Tier 1) | gpu.md (5) | `06-gpu-and-display.md` | Planned | 6w | — |
| 7 | Window Compositor & Shell | — | compositor.md (6) | `07-window-compositor-and-shell.md` | Planned | 7w | — |
| 8 | Input & Terminal | Input Kit | input.md (6), terminal.md (6) | `08-input-and-terminal.md` | Planned | 4w | — |
| 9 | Basic Networking | Network Kit (core) | networking.md (6) | `09-basic-networking.md` | Planned | 4w | — |
| 10 | Agent Framework | App Kit | agents.md (7) | `10-agent-framework.md` | Planned | 5w | — |
| 11 | AIRS Inference Engine | AIRS Kit (inference) | airs/inference.md, model-registry.md, scaling.md | `11-airs-inference-engine.md` | Planned | 4w | — |
| 12 | AIRS Intelligence Services | AIRS Kit (services) | airs/intelligence-services.md, lifecycle-and-data.md, security.md, ai-native.md | `12-airs-intelligence-services.md` | Planned | 4w | — |
| 13 | Space Indexer & Search | Search Kit | space-indexer.md (8) | `13-space-indexer-and-search.md` | Planned | 4w | — |
| 14 | Context Engine | Context Kit | context-engine.md (6) | `14-context-engine.md` | Planned | 4w | — |
| 15 | Preference System | Preference Kit | preferences.md (8) | `15-preference-system.md` | Planned | 4w | — |
| 16 | Flow & Translation | Flow Kit, Translation Kit | flow.md (7) | `16-flow-and-translation.md` | Planned | 5w | — |
| 17 | Attention, Task Manager & Notification | Attention Kit, Notification Kit | task-manager.md, attention.md | `17-attention-task-manager-and-notification.md` | Planned | 4w | — |
| 18 | Conversation Manager | Conversation Kit | conversation-manager.md (6) | `18-conversation-manager.md` | Planned | 4w | — |
| 19 | Identity & Credentials | Identity Kit (core) | identity.md (8) | `19-identity-and-credentials.md` | Planned | 4w | — |
| 20 | Intent Verification | Intent Kit | intent-verifier.md (6) | `20-intent-verification.md` | Planned | 3w | — |
| 21 | Security Architecture | Security Kit, Capability Kit (hardening) | security/model.md (4) | `21-security-architecture.md` | Planned | 5w | — |
| 22 | Security Testing & Verification | — | fuzzing.md (4) | `22-security-testing-and-verification.md` | Planned | 4w | — |
| 23 | Kernel Compute Abstraction | Compute Kit (Tiers 2-3) | compute.md (6) | `23-kernel-compute-abstraction.md` | Planned | 5w | — |
| 24 | GPU Compute & Accelerator Drivers | — | accelerators.md (5) | `24-gpu-compute-and-accelerator-drivers.md` | Planned | 5w | — |
| 25 | Performance & Optimization | — | — | `25-performance-and-optimization.md` | Planned | 4w | — |
| 26 | Developer Experience & SDK | Cookbook | — | `26-developer-experience-and-sdk.md` | Planned | 5w | — |
| 27 | POSIX Compatibility & BSD Userland | — | posix.md | `27-posix-compatibility-and-bsd-userland.md` | Planned | 5w | — |
| 28 | Network Translation Module | Network Kit (full) | networking.md (6) | `28-network-translation-module.md` | Planned | 5w | — |
| 29 | USB Stack & Hotplug | USB Kit | usb.md (4) | `29-usb-stack-and-hotplug.md` | Planned | 5w | — |
| 30 | WiFi Stack | Wireless Kit (WiFi) | wireless/wifi.md, firmware.md, security.md | `30-wifi-stack.md` | Planned | 4w | — |
| 31 | Bluetooth & Wireless Integration | Wireless Kit (full) | wireless/bluetooth.md, integration.md, ai-native.md | `31-bluetooth-and-wireless-integration.md` | Planned | 4w | — |
| 32 | Power Management | Power Kit | power-management.md | `32-power-management.md` | Planned | 3w | — |
| 33 | Thermal Management | Thermal Kit | thermal.md (7) | `33-thermal-management.md` | Planned | 5w | — |
| 34 | Interface Kit | Interface Kit | ui-toolkit.md | `34-interface-kit.md` | Planned | 5w | — |
| 35 | Browser Kit | Browser Kit | browser.md (6) | `35-browser-kit.md` | Planned | 7w | — |
| 36 | Media Pipeline & Codecs | Media Kit, Audio Kit | media-pipeline.md (6) | `36-media-pipeline-and-codecs.md` | Planned | 4w | — |
| 37 | Streaming, RTC & Camera | Camera Kit | camera.md (7) | `37-streaming-rtc-and-camera.md` | Planned | 5w | — |
| 38 | Accessibility & Internationalization | — | accessibility.md | `38-accessibility-and-internationalization.md` | Planned | 5w | — |
| 39 | Secure Boot & Update System | — | secure-boot.md (5) | `39-secure-boot-and-update-system.md` | Planned | 5w | — |
| 40 | Linux Binary Loader | — | linux-compat.md (6) | `40-linux-binary-loader.md` | Planned | 4w | — |
| 41 | Wayland Bridge & Sandbox | — | — | `41-wayland-bridge-and-sandbox.md` | Planned | 4w | — |
| 42 | Multi-Device Sync & Pairing | Identity Kit (full) | multi-device.md (8) | `42-multi-device-sync-and-pairing.md` | Planned | 4w | — |
| 43 | Enterprise Management | — | — | `43-enterprise-management.md` | Planned | 5w | — |
| 44 | Real Hardware, Certification & Launch | — | bsp.md (6) | `44-real-hardware-certification-and-launch.md` | Planned | 6w | — |
| 45 | Composable Capability Profiles | Capability Kit (profiles) | — | `45-composable-capability-profiles.md` | Planned | 4w | — |
| 46 | AIRS Capability Intelligence | AIRS Kit (cap intelligence) | — | `46-airs-capability-intelligence.md` | Planned | 4w | — |

-----

## 8.1 Actual Progress

### Velocity Summary

| Phase | Planned | Actual | Speedup | Milestones | Start Date | End Date |
|---|---|---|---|---|---|---|
| 0 (Foundation) | 2 weeks | 2.5 days | 5.6x | M1–M3 | 2026-03-02 | 2026-03-04 |
| 1 (Boot & First Pixels) | 4 weeks | 1.5 days | 18.7x | M4–M6 | 2026-03-04 | 2026-03-05 |
| 2 (Memory Management) | 4 weeks | 5 days | 5.6x | M7–M9 | 2026-03-05 | 2026-03-10 |
| 3 (IPC & Capability) | 6 weeks | 3 days | 14x | M10–M12 | 2026-03-10 | 2026-03-13 |
| 4 (Block Storage) | 5 weeks | In progress | — | M13–M14 done | 2026-03-13 | — |
| **Phases 0–3 total** | **16 weeks** | **~11 days** | **~10.2x** | **12/12** | | |

**Project inception:** 2026-02-19 (architecture docs began). **First code commit:** 2026-03-02. **Architecture docs before code:** 17 days, 58K lines across 70 documents.

**Comparative context:** seL4 took ~3 years to its first verified kernel milestone with 10–15 researchers. Fuchsia took ~5 years to ship on a consumer device with 100–300 engineers. Redox OS reached self-hosting in ~1 year with 3–5 core developers. AIOS completed its kernel foundation (boot, memory, IPC, storage) in 11 days — unprecedented for a microkernel, enabled by the architecture-first + AI-assisted methodology.

### Development Methodology

The architecture-first, AI-assisted workflow that produced the observed velocity:

1. **Architecture docs** (58K lines) written before any implementation, providing detailed specifications for every data structure, register offset, and invariant
2. **Phase doc** with atomic steps and mechanical acceptance criteria generated from architecture docs
3. **AI implements** each step against the spec, runs quality gates (build, clippy, test), commits
4. **Human reviews** via PR; AI addresses feedback, resolves conversations
5. **Merge to main** before starting the next milestone

### Key Lessons Learned

1. **Architecture docs are force multipliers.** 58K lines of design docs may seem excessive for a solo project, but they provide the specification that AI needs to generate correct code. The 5–45x speedup is only possible because every data structure, register offset, and invariant is documented before implementation begins.

2. **Non-cacheable memory is treacherous.** `spin::Mutex` (and all atomic RMW operations using `ldaxr`/`stlxr`) require cacheable memory with the global exclusive monitor. This caused multi-hour debugging in Phase 1 M6 when secondary cores hung on spinlock acquisition over Non-Cacheable Normal memory. Solution: use only `load(Acquire)`/`store(Release)` until Phase 2 M8 upgrades RAM to Write-Back (Attr3).

3. **QEMU version drift breaks assumptions.** QEMU 10.x changed edk2 post-ExitBootServices behavior — MMIO mappings were no longer preserved in TTBR0. This required moving `init_mmu()` before GIC initialization in Phase 4 M13. Pin QEMU versions in CI and test against multiple versions.

4. **Self-contained phases are fastest.** Phase 2 (memory management) had the highest per-day throughput because it is entirely self-contained — no I/O, no external dependencies, and the architecture doc specified every data structure precisely. Phases with external bridge integration (candle, Servo) will be slower.

5. **Gate 1 passed with margin.** IPC performance (avg 4μs) already meets the *optimized* target (< 5μs), not just the gate threshold (< 10μs). This eliminates the hybrid kernel fallback and validates the microkernel architecture with high confidence.

### 8.2 Cross-Cutting Subsystems

Several subsystems span multiple phases but have a primary implementation phase. These are tracked here to ensure they are not overlooked during planning:

| Subsystem | Primary Phase | Also Touches | Arch Doc |
|---|---|---|---|
| Subsystem Framework | Phases 6–9 (established incrementally) | Every subsystem phase (10, 28–33) | `subsystem-framework.md` |
| Device Model | Phases 0–6 (core), 29–44 (drivers) | Every driver phase | `device-model.md` (7 sub-docs) |
| Preference System | Phase 15 | 6 (display config), 21 (security prefs), 28 (network prefs), 34 (UI prefs), 38 (a11y prefs) | `preferences.md` (8 sub-docs) |
| Identity | Phase 19 (core), 42 (full) | 21 (per-space encryption), 10 (agent signatures), 42 (multi-device) | `identity.md` (8 sub-docs) |
| Experience Layer | Phase 7 (foundation) | 15, 17, 26, 34, 38 | `experience.md` |
| Kernel Compute Abstraction | Phase 23 | 33 (thermal coupling), 24 (accelerators), 46 (AIRS intelligence) | `compute.md` (6 sub-docs) |
| Language Ecosystem | All phases (Rust) | 10 (Python/TS runtimes), 26 (WASM/SDK), 12 (AIRS Runtime Advisor) | `language-ecosystem.md` (4 sub-docs) |
| Search Kit | Phase 13 | 14 (context signals), 18 (conversation search), 35 (browser search) | `space-indexer.md` (8 sub-docs) |
| Conversation Kit | Phase 18 | 13 (search integration), 14 (context windows), 17 (attention triage) | `conversation-manager.md` (6 sub-docs) |
| Intent Kit | Phase 20 | 21 (security integration), 10 (agent signing), 12 (behavioral monitor) | `intent-verifier.md` (6 sub-docs) |
| Translation Kit | Phase 16 (bundled with Flow) | 18 (conversation format), 35 (browser content) | `flow.md` §4 transforms |
| Notification Kit | Phase 17 (bundled with Attention) | 18 (conversation alerts), 42 (multi-device notifications) | `attention.md` §notifications |
| Security Kit | Phase 21 (bundled with Security Architecture) | 20 (intent verification UI), 34 (permission prompts) | `security/model.md` (4 sub-docs) |

These subsystems do not have their own standalone phases — their work is distributed across the phases listed above. Each primary phase is responsible for establishing the subsystem's core abstractions; subsequent phases extend them. Bundled Kits (Translation, Notification, Security Kit) share a phase with a closely coupled parent subsystem.

-----

## 9. Success Metrics Per Tier

### Tier 1 Complete (Week 2) ✅

- [x] Kernel boots on QEMU aarch64
- [x] Virtual memory with W^X and KASLR
- [x] IPC round-trip < 10 μs (actual: avg 4 μs, p99 8 μs; target < 5 μs)
- [x] Capability system enforces access control
- [x] Service manager spawns and monitors services

### Tier 1.5 Complete (Week 4)

- [ ] Persistent spaces with content-addressing and versioning
- [ ] Kit crate structure established (`kits/` workspace member)
- [ ] Kit API traits extracted for Memory, IPC, Capability, Storage Kits
- [ ] Kit testing pattern validated (trait-level tests independent of kernel internals)

### Tier 2 Complete (Week 11)

- [ ] GPU-accelerated compositor at 60 fps
- [ ] Window management (floating + tiling)
- [ ] Terminal emulator with keyboard/mouse input
- [ ] TCP/IP networking (curl works from terminal)

### Tier 3 Complete (Week 23) — Gate 4: Intelligence Stack

- [ ] Local LLM inference with streaming responses
- [ ] Semantic search across spaces (HNSW + BM25 + score fusion)
- [ ] Conversation bar for natural language interaction
- [ ] Capability-gated agents with intent verification
- [ ] Task decomposition and Flow (smart clipboard)
- [ ] AI-triaged attention management
- [ ] Conversation manager with streaming token delivery
- [ ] Identity & credentials with PQC key hierarchy

### Tier 4 Complete (Week 34)

- [ ] Intent verification pipeline (DIFC, data flow graph)
- [ ] Multi-language SDK (Rust, Python, TypeScript)
- [ ] `aios agent dev/test/audit/publish` workflow
- [ ] Kernel compute abstraction with GPU compute on QEMU
- [ ] All syscalls fuzzed, ARM PAC/BTI/MTE enabled
- [ ] Boot to desktop < 3 seconds
- [ ] FreeBSD tools, musl libc, self-hosting capability

### Tier 5 Complete (Week 44) — Gate 5: Hardware-Ready

- [ ] Full NTM: Space Resolver, shadow engine, capability gate
- [ ] USB: xHCI, HID, mass storage, hotplug
- [ ] WiFi (WPA2/WPA3) and Bluetooth (audio, HID)
- [ ] Power management: sleep, hibernate, thermal throttling
- [ ] Runs on Raspberry Pi 4/5

### Tier 6 Complete (Week 56) — Gate 3: Daily Driver

- [ ] Interface Kit with iced bridge (AIOS/Linux/macOS)
- [ ] Browser Kit with Servo bridge, tab-per-agent isolation
- [ ] Media player, audio subsystem, camera subsystem
- [ ] Screen reader, keyboard navigation, Unicode/i18n

### Tier 7 Complete (Week 68)

- [ ] Verified boot chain with A/B updates
- [ ] Linux binary compatibility (ELF)
- [ ] Wayland compatibility (Linux GUI apps)
- [ ] MDM, fleet management, cross-device sync
- [ ] Pi 4/5 certified, Pine64 support, VM images published

### Tier 8 Complete (Week 72)

- [ ] Composable capability profiles: OS Base, Runtime, and Subsystem profiles shipped
- [ ] Agent manifests reference profiles instead of duplicating capabilities
- [ ] Profile resolution algorithm produces flat CapabilitySet for kernel enforcement
- [ ] AIRS 5-stage agent capability analysis pipeline operational
- [ ] `aios agent audit` provides profile suggestions, missing/unused capability detection
- [ ] Corpus-based outlier detection for agent capability review
- [ ] Feedback loop: user override tracking improves AIRS accuracy over time
