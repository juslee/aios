# AIOS Development Plan

## Timeline, Risks, Dependencies, and Decision Gates

**Parent document:** [overview.md](./overview.md)
**Related:** All architecture documents

-----

## 1. Overview

30 phases across ~138 weeks (~2.7 years). Organized into 8 tiers, each delivering a usable milestone. The plan is designed so that each tier builds on the previous one and produces something demonstrable.

```
Tier 1: Hardware Foundation    (Phases 0–3)    Weeks 1–16     Boot, memory, IPC
Tier 2: Core System Services   (Phases 4–7)    Weeks 17–34    Storage, GPU, compositor, networking
Tier 3: AI & Intelligence      (Phases 8–11)   Weeks 35–54    AIRS, semantic search, agents
Tier 4: Platform Maturity      (Phases 12–15)  Weeks 55–74    SDK, security, performance, POSIX (+3 wk buffer)
Tier 5: Hardware & Connectivity(Phases 16–19)  Weeks 75–92    Full NTM, USB, wireless, power
Tier 6: Rich Experience        (Phases 20–23)  Weeks 93–112   UI toolkit, browser, media, a11y
Tier 7: Production OS          (Phases 24–27)  Weeks 113–130  Secure boot, Linux compat, launch
Tier 8: Security Intelligence  (Phases 28–29)  Weeks 131–138  Capability profiles, AIRS cap intelligence
```

-----

## 2. Tier Milestones

Each tier produces a demonstrable result:

| Tier | Milestone | Demo |
|---|---|---|
| 1 | Microkernel boots on QEMU | UART output, memory allocation, IPC ping-pong benchmark |
| 2 | Graphical desktop with shell | Window compositor, terminal emulator, basic networking (curl works) |
| 3 | AI-enhanced OS | Conversation bar, semantic search, agents running with capability gates |
| 4 | Developer platform | SDK published, BSD tools functional, security hardened, boot <3s |
| 5 | Hardware-ready OS | WiFi, Bluetooth, USB, power management — runs on Raspberry Pi |
| 6 | Daily-driver OS | Web browser, media player, accessibility, internationalization |
| 7 | Production OS | Secure boot, Linux app compat, enterprise features, shipping |
| 8 | Security Intelligence | Composable capability profiles, AIRS-powered agent audit |

-----

## 3. Phase Dependencies

```
Phase 0: Foundation & Tooling
  └─→ Phase 1: Boot & First Pixels
       └─→ Phase 2: Memory Management
            └─→ Phase 3: IPC & Capability System
                 └─→ Phase 4: Block Storage & Object Store
                      └─→ Phase 5: GPU & Display
                           └─→ Phase 6: Window Compositor & Shell
                                └─→ Phase 7: Input, Terminal & Basic Networking
                                     ├─→ Phase 8: AIRS Core
                                     │    ├─→ Phase 9: Space Intelligence & Conversation
                                     │    │    └─→ Phase 10: Agent Framework
                                     │    │         ├─→ Phase 11: Tasks, Flow & Attention
                                     │    │         │    └─→ Phase 12: Developer Experience & SDK
                                     │    │         │
                                     │    │         └─→ Phase 13: Security Hardening (also requires 4)
                                     │    │
                                     │    └─→ Phase 14: Performance & Optimization (also requires 6)
                                     │         └─→ Phase 15: POSIX Compatibility & BSD Userland
                                     │
                                     └─→ Phase 16: Network Translation Module

Phase 16 ──→ Phase 17: USB Stack & Hotplug
              └─→ Phase 18: WiFi, Bluetooth & Wireless
                   └─→ Phase 19: Power Management & Thermal

Phase 12 ──→ Phase 20: Portable UI Toolkit
              └─→ Phase 21: Web Browser (Servo)
Phase 18 ──→ Phase 22: Media, Audio & Camera Subsystems
              └─→ Phase 23: Accessibility & Internationalization

Phase 19 ──→ Phase 24: Secure Boot & Update System
              └─→ Phase 25: Linux Binary & Wayland Compatibility
                   └─→ Phase 26: Enterprise & Multi-Device
                        └─→ Phase 27: Real Hardware, Certification & Launch

Phase 3 + Phase 10 + Phase 12
  └─→ Phase 28: Composable Capability Profiles
       └─→ Phase 29: AIRS Capability Intelligence (also requires Phase 8, Phase 13)
```

**Critical path:** 0 → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 20 → 21. Note: Phase 15 (POSIX/BSD Userland) is not on the critical path because the Daily Driver gate (Gate 3, after Phase 21) depends on the browser and UI toolkit chain (12 → 20 → 21), not POSIX tools. Phase 15 is a parallel workstream that enhances the developer experience but is not a prerequisite for the Gate 3 decision.

The web browser (Phase 21) is the last item on the critical path before the OS can be someone's daily driver. Every phase on the critical path is a potential bottleneck.

-----

## 4. Risk Register

### 4.1 Technical Risks

| Risk | Impact | Likelihood | Mitigation |
|---|---|---|---|
| IPC performance < 5μs target | High — microkernel viability | Medium | Prototype IPC in Phase 3, benchmark before proceeding. Fallback: hybrid kernel with in-kernel filesystem |
| GGML/llama.cpp aarch64 performance insufficient | High — AI features unusable | Low | GGML is proven on aarch64 (runs on phones). Mitigation: quantization, smaller models, NPU offload |
| Servo build complexity | High — browser delayed | Medium | Servo is modular but massive. Mitigation: start integration early (Phase 21 prep in Phase 16), maintain Servo fork |
| smoltcp limitations | Medium — networking incomplete | Low | smoltcp handles TCP/UDP well. Missing: advanced congestion control, some edge cases. Mitigation: contribute upstream, fork if needed |
| GPU driver complexity (real hardware) | High — no display on Pi | Medium | VirtIO-GPU works in QEMU. Pi GPU (VC4/V3D) has open-source driver but is complex. Mitigation: start with framebuffer fallback |
| Memory pressure on 2GB devices | Medium — degraded experience | Medium | Models need 4+ GB RAM. Mitigation: aggressive quantization, model eviction, swap space, 4GB minimum recommended |
| Firmware blob licensing | Medium — WiFi/BT unusable | Low | Most WiFi/BT chips need proprietary firmware. Mitigation: redistribute under manufacturer license (standard practice), document clearly |
| Kernel memory safety despite unsafe Rust | High — security compromise | Medium | Kernel code requires `unsafe` for hardware access. Mitigation: minimize unsafe, document all unsafe blocks, fuzz all syscalls, formal verification for critical paths |

### 4.2 Schedule Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Phase 3 (IPC) takes longer than 6 weeks | Delays everything downstream | IPC is the hardest kernel component. Budget 2 weeks of slack. Prototype early. |
| Phase 8 (AIRS) underestimated | AI features delayed | GGML integration is well-understood. Context management and indexing are the unknowns. |
| Phase 21 (Browser) underestimated | Daily-driver delayed | Servo integration is the highest-risk single phase. 5 weeks may not be enough. Budget 2 weeks slack. |
| Scope creep in any phase | Timeline extends | Each phase has a strict deliverable. Feature requests go to future phases. |

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
- Context switch < 20 μs (gate threshold). Note: architecture.md §6.9 lists the optimized target (< 10 μs); this gate uses a relaxed threshold since post-Phase 3 optimization (Phase 14) has not yet occurred.
- No pathological performance cliffs

**If NO:** Consider hybrid kernel (move Space Storage into kernel space). This is a significant architectural change but recoverable at this stage because no external users depend on the IPC interface yet, and only kernel and storage code has been written.

### Gate 2: AI Viability (after Phase 8)

**Decision:** Can we run useful LLM inference on target hardware?

**Criteria:**
- 7B model runs at > 5 tokens/second on Pi 4 (4GB)
- Time to first token < 2 seconds (gate threshold). Note: architecture.md §6.9 lists the optimized target (< 500 ms); this gate uses a relaxed threshold since pre-optimization hardware (Pi 4 on SD card) has higher latency than the final target.
- Memory usage within budget (leaves >1 GB for OS + apps)

**If NO:** Scale down AI features. Use smaller models (1-3B). Focus on embedding/classification rather than generation. Conversation bar becomes a search interface rather than a conversational one.

### Gate 3: Daily Driver (after Phase 21)

**Decision:** Can someone use AIOS as their only computer for basic tasks?

**Criteria:**
- Web browsing works (Gmail, YouTube, basic sites)
- Terminal works (development possible)
- Files are accessible (spaces + POSIX bridge)
- System is stable (no crashes in 8-hour session)

**If NO:** Identify blocking issues and allocate additional time before Tier 7.

-----

## 6. Staffing Model

### Solo Developer (Realistic)

All 30 phases are designed to be achievable by a single experienced systems programmer:
- Average phase: 4-5 weeks
- Total: ~138 weeks (~2.7 years)
- Assumes full-time, focused work

### Small Team (Accelerated)

With 2-3 developers, phases can be parallelized:
- Developer A: kernel (Phases 0-3), then performance (14), then security (13)
- Developer B: storage + GPU (Phases 4-6), then input/networking (7), then UI toolkit (20), then browser (21) — ordered to respect dependency chain (Phase 7 → 8, Phase 12 → 20 → 21)
- Developer C: AI (Phases 8-11), then networking (16), then agent ecosystem (12)
- Note: Remaining phases (15, 17-19, 22-27) are assigned based on availability as earlier phases complete.

Estimated timeline with 3 developers: ~50-60 weeks (~1 year).

-----

## 7. Technology Stack Summary

| Layer | Technology | License |
|---|---|---|
| Language | Rust | MIT/Apache-2.0 |
| Build system | just + cargo | MIT |
| Bootloader | UEFI (custom) | BSD-2-Clause |
| TCP/IP | smoltcp | BSD-2-Clause |
| TLS | rustls | Apache-2.0/MIT |
| HTTP | h2, hyper | MIT |
| QUIC | quinn | Apache-2.0/MIT |
| DNS | hickory-dns | Apache-2.0/MIT |
| GPU | wgpu | Apache-2.0/MIT |
| UI toolkit | iced | MIT |
| Font rendering | fontdue or ab_glyph | MIT |
| Browser engine | Servo (Servo's layout + SpiderMonkey) | MPL-2.0 |
| AI inference | GGML / llama.cpp | MIT |
| Model format | GGUF | MIT |
| C library | musl | MIT |
| Userland tools | FreeBSD | BSD-2-Clause |
| Shell | FreeBSD /bin/sh | BSD-2-Clause |
| Compiler | LLVM/clang | Apache-2.0 |
| Certificates | webpki-roots | MPL-2.0 |

All permissively licensed. No GPL dependencies in the core OS.

-----

## 7.1 Target Application: OpenFang

[OpenFang](https://github.com/RightNow-AI/openfang) is an open-source Agent Operating System built in Rust (MIT/Apache-2.0, 137K lines, 14 crates). It provides autonomous agent orchestration, 40 channel adapters, 53 built-in tools, 27 LLM providers, and 16 security layers — all in a single ~32MB binary. AIOS adopts OpenFang as a first-class target application starting at Phase 10.

### Why OpenFang

Rather than reinvent agent orchestration, channel adapters, and LLM routing, AIOS provides the kernel-level primitives (capability isolation, hardware-enforced sandboxing, kernel-scheduled inference) and lets OpenFang provide the userspace agent runtime. This gives AIOS a production-tested agent ecosystem from day one.

### Integration Points by Phase

| Phase | Integration | What AIOS Provides | What OpenFang Provides |
|---|---|---|---|
| 8 (AIRS Core) | LLM routing compatibility | Kernel-level inference with hardware scheduling | Model routing patterns, cost-aware metering, 27 provider configs |
| 10 (Agent Framework) | Hand → AIOS agent mapping | AgentManifest with scheduled execution support | HAND.toml manifest format as candidate packaging standard, 7 bundled Hands |
| 10 (Agent Framework) | Agent-to-Agent protocol | IPC channels with capability gates | A2A + OFP protocol patterns for inter-agent delegation |
| 13 (Security) | Security layer mapping | Hardware capability tokens, MMU isolation | Taint tracking patterns, Merkle audit chain, prompt injection scanning |
| 15 (POSIX) | Full binary compatibility | POSIX syscalls, musl libc | Unmodified OpenFang binary runs as AIOS process |
| 16 (Network Translation Module) | Channel adapter support | TCP/IP + TLS stack | 40 channel adapters (Telegram, Discord, Slack, etc.) |

### OpenFang Concepts Adopted into AIOS

- **Scheduled autonomous agents (Hands):** AIOS AgentManifest gains `schedule` field for cron-like autonomous execution — agents that wake, perform work, and sleep without user prompting. Modeled after OpenFang's HAND.toml.
- **HAND.toml as candidate manifest format:** The AIOS `manifest.toml` format references OpenFang's HAND.toml for the `[agent.schedule]`, `[[agent.approval_gates]]`, and `[[agent.dashboard_metrics]]` sections. See [agents.md §2.4](../applications/agents.md).
- **Cost-aware inference metering:** AIRS tracks per-model token costs and enforces budgets, inspired by OpenFang's per-model cost tracking and GCRA rate limiting. See [airs.md](../intelligence/airs.md).
- **Information flow taint tracking:** The capability system labels secret data at introduction and tracks propagation, inspired by OpenFang's taint tracking system.
- **Merkle hash-chain audit trail:** Every capability invocation is cryptographically chained for tamper-evident audit logs, inspired by OpenFang's audit system.

-----

## 8. Phase Detail Reference

Each phase has an implementation doc in `docs/phases/` containing objectives, milestone steps with acceptance criteria, decision points, and references to the existing architecture documents (which hold the technical design). This avoids duplicating architecture content while providing a clear implementation sequence.

| Phase | Name | Document | Status |
|---|---|---|---|
| 0 | Foundation & Tooling | [`00-foundation-and-tooling.md`](../phases/00-foundation-and-tooling.md) | Planned |
| 1 | Boot & First Pixels | [`01-boot-and-first-pixels.md`](../phases/01-boot-and-first-pixels.md) | Planned |
| 2 | Memory Management | [`02-memory-management.md`](../phases/02-memory-management.md) | Planned |
| 3 | IPC & Capability System | `03-ipc-and-capability-system.md` | Planned |
| 4 | Block Storage & Object Store | `04-block-storage-and-object-store.md` | Planned |
| 5 | GPU & Display | `05-gpu-and-display.md` | Planned |
| 6 | Window Compositor & Shell | `06-window-compositor-and-shell.md` | Planned |
| 7 | Input, Terminal & Basic Networking | `07-input-terminal-and-basic-networking.md` | Planned |
| 8 | AIRS Core | `08-airs-core.md` | Planned |
| 9 | Space Intelligence & Conversation | `09-space-intelligence-and-conversation.md` | Planned |
| 10 | Agent Framework | `10-agent-framework.md` | Planned |
| 11 | Tasks, Flow & Attention | `11-tasks-flow-and-attention.md` | Planned |
| 12 | Developer Experience & SDK | `12-developer-experience-and-sdk.md` | Planned |
| 13 | Security Hardening | `13-security-hardening.md` | Planned |
| 14 | Performance & Optimization | `14-performance-and-optimization.md` | Planned |
| 15 | POSIX Compatibility & BSD Userland | `15-posix-compatibility-and-bsd-userland.md` | Planned |
| 16 | Network Translation Module | `16-network-translation-module.md` | Planned |
| 17 | USB Stack & Hotplug | `17-usb-stack-and-hotplug.md` | Planned |
| 18 | WiFi, Bluetooth & Wireless | `18-wifi-bluetooth-and-wireless.md` | Planned |
| 19 | Power Management & Thermal | `19-power-management-and-thermal.md` | Planned |
| 20 | Portable UI Toolkit | `20-portable-ui-toolkit.md` | Planned |
| 21 | Web Browser | `21-web-browser.md` | Planned |
| 22 | Media, Audio & Camera | `22-media-audio-and-camera.md` | Planned |
| 23 | Accessibility & Internationalization | `23-accessibility-and-internationalization.md` | Planned |
| 24 | Secure Boot & Update System | `24-secure-boot-and-update-system.md` | Planned |
| 25 | Linux Binary & Wayland Compatibility | `25-linux-binary-and-wayland-compatibility.md` | Planned |
| 26 | Enterprise & Multi-Device | `26-enterprise-and-multi-device.md` | Planned |
| 27 | Real Hardware, Certification & Launch | `27-real-hardware-certification-and-launch.md` | Planned |
| 28 | Composable Capability Profiles | `28-composable-capability-profiles.md` | Planned |
| 29 | AIRS Capability Intelligence | `29-airs-capability-intelligence.md` | Planned |

-----

## 9. Success Metrics Per Tier

### Tier 1 Complete (Week 16)
- [ ] Kernel boots on QEMU aarch64
- [ ] Virtual memory with W^X and KASLR
- [ ] IPC round-trip < 10 μs (target < 5 μs)
- [ ] Capability system enforces access control
- [ ] Service manager spawns and monitors services

### Tier 2 Complete (Week 34)
- [ ] Persistent spaces with content-addressing and versioning
- [ ] GPU-accelerated compositor at 60 fps
- [ ] Window management (floating + tiling)
- [ ] Terminal emulator with keyboard/mouse input
- [ ] TCP/IP networking (curl works from terminal)

### Tier 3 Complete (Week 54)
- [ ] Local LLM inference with streaming responses
- [ ] Semantic search across spaces
- [ ] Conversation bar for natural language interaction
- [ ] Capability-gated agents with intent verification
- [ ] Task decomposition and Flow (smart clipboard)
- [ ] AI-triaged attention management

### Tier 4 Complete (Week 74)
- [ ] Multi-language SDK (Rust, Python, TypeScript)
- [ ] `aios agent dev/test/audit/publish` workflow
- [ ] All syscalls fuzzed, ARM PAC/BTI/MTE enabled
- [ ] Boot to desktop < 3 seconds
- [ ] FreeBSD tools, musl libc, self-hosting capability

### Tier 5 Complete (Week 92)
- [ ] Full NTM: space resolver, shadow engine, capability gate
- [ ] USB: xHCI, HID, mass storage, hotplug
- [ ] WiFi (WPA2/WPA3) and Bluetooth (audio, HID)
- [ ] Power management: sleep, hibernate, thermal throttling
- [ ] Runs on Raspberry Pi 4/5

### Tier 6 Complete (Week 112)
- [ ] Cross-platform UI toolkit (iced on AIOS/Linux/macOS)
- [ ] Servo-based browser with tab-per-agent isolation
- [ ] Media player, audio subsystem, camera subsystem
- [ ] Screen reader, keyboard navigation, Unicode/i18n

### Tier 7 Complete (Week 130)
- [ ] Verified boot chain with A/B updates
- [ ] Linux binary compatibility (ELF)
- [ ] Wayland compatibility (Linux GUI apps)
- [ ] MDM, fleet management, cross-device sync
- [ ] Pi 4/5 certified, Pine64 support, VM images published

### Tier 8 Complete (Week 138)
- [ ] Composable capability profiles: OS Base, Runtime, and Subsystem profiles shipped
- [ ] Agent manifests reference profiles instead of duplicating capabilities
- [ ] Profile resolution algorithm produces flat CapabilitySet for kernel enforcement
- [ ] AIRS 5-stage agent capability analysis pipeline operational
- [ ] `aios agent audit` provides profile suggestions, missing/unused capability detection
- [ ] Corpus-based outlier detection for agent capability review
- [ ] Feedback loop: user override tracking improves AIRS accuracy over time
