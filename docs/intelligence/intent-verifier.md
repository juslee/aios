# AIOS Intent Verifier Architecture

**Audience:** Kernel developers, security engineers, AIRS implementers
**Scope:** Intent verification pipeline, action alignment, capability checking, adversarial resistance
**Related:** [airs.md](./airs.md) — AIRS Runtime, [Security Model](../security/model.md), [Agents](../applications/agents.md), [IPC](../kernel/ipc.md)

---

## §1 Core Insight

Capabilities answer *"is this agent allowed to do this?"* — but they cannot answer *"is this agent supposed to be doing this?"*

An email agent with `ReadSpace("email/")` and `Network(smtp.gmail.com)` has legitimate capabilities for reading and sending mail. If the user asked *"summarize my unread emails"* and the agent starts deleting emails and forwarding them to an external address, every individual action is capability-permitted — but the aggregate behavior violates the user's intent. Layer 2 (capability enforcement) allows it; Layer 1 (intent verification) catches it.

The Intent Verifier is **Security Layer 1** in AIOS's eight-layer defense model. It uses LLM inference through AIRS to semantically compare observed agent actions against the agent's declared task intent. When AIRS is unavailable, intent verification degrades gracefully — Layers 2–8 remain active, and algorithmic pre-checks (structured intent matching) continue to operate without LLM support.

### Design Philosophy

Three principles govern the Intent Verifier:

1. **Algorithmic where possible, LLM where necessary.** Structured intent specifications enable machine-checkable pre-filters that handle ~80% of verification without LLM inference. The LLM is reserved for ambiguous cases requiring semantic understanding.

2. **Conservative fallback.** When uncertain, restrict rather than permit. The system defaults to blocking destructive actions when AIRS is unavailable, rather than silently allowing them.

3. **Defense in depth.** Intent verification is one layer in an eight-layer model. No single layer is sufficient; each catches threats the others miss. Intent verification catches semantic misalignment that capability checks permit; capability checks catch unauthorized access that intent verification might miss.

### Relationship to Other Security Layers

```text
Layer 1: Intent Verification  ← this document
    "Is this action consistent with the declared task?"
    Semantic comparison via AIRS LLM inference

Layer 2: Capability Enforcement  → security/model/capabilities.md
    "Does this agent have a token for this action?"
    Kernel-enforced, O(1) per syscall, hardware-unforgeable

Layer 3: Behavioral Monitoring  → intent-verifier/behavioral.md §6
    "Is this action statistically normal for this agent?"
    Baseline comparison, rate anomaly detection

Layers 4–8: Resource limits, injection defense, audit, cryptographic integrity, user override
    → security/model/layers.md §2.4–§2.8
```

Intent verification and behavioral monitoring operate in tandem: Layer 1 checks *semantic alignment* (does the action match the declared purpose?), while Layer 3 checks *statistical normality* (does the action match historical patterns?). An action can be semantically aligned but statistically anomalous (legitimate but unusual volume), or statistically normal but semantically misaligned (routine action serving a different purpose than declared).

---

## Document Map

| Document | Sections | Content |
|---|---|---|
| **This file** | §1, §14, §15 | Core insight, implementation order, design principles |
| [pipeline.md](./intent-verifier/pipeline.md) | §2, §4, §10 | Architecture, verification pipeline, performance model |
| [specification.md](./intent-verifier/specification.md) | §3 | Intent specification: current DeclaredIntent + structured StructuredIntent |
| [information-flow.md](./intent-verifier/information-flow.md) | §5 | IPC taint labels (DIFC), data flow verification, exfiltration detection |
| [behavioral.md](./intent-verifier/behavioral.md) | §6, §9 | Behavioral monitor coordination, temporal logic monitor |
| [security.md](./intent-verifier/security.md) | §7, §8, §11 | Capability integration, adversarial resistance, graceful degradation |
| [intelligence.md](./intent-verifier/intelligence.md) | §12, §13, §16, §17 | Testing, AI-native intelligence, future directions, references |

---

## §14 Implementation Order

Intent verification is implemented incrementally across multiple phases, building on the capability system (Phase 3) and AIRS inference engine (Phase 10).

| Phase | Component | Dependencies | Deliverable |
|---|---|---|---|
| **Phase 14a** | Core IntentVerifier + Behavioral Monitor | Phase 10 (AIRS inference), Phase 11 (context engine) | `IntentVerifier` struct, `DeclaredIntent`, `VerificationResult`, security path IPC, synchronous/async verification modes, `BehavioralMonitor` with baseline learning |
| **Phase 14b** | Structured Intent Specs + Algorithmic Pre-Check | Phase 14a | `StructuredIntent`, `IntentPurpose` enum, algorithmic pre-filter (no LLM for ~80% of checks), `TemporalSpec` formulas |
| **Phase 14c** | Adversarial Defense integration | Phase 14a | `InjectionClassifier`, control/data separation enforcement, multi-round adversarial self-testing for high-risk actions |
| **Phase 16+** | IPC Taint Labels | Phase 3 (IPC), Phase 14a | `LabelSet` on IPC messages, kernel-enforced DIFC, declassification protocol |
| **Phase 16+** | MTL Evaluator | Phase 14b | Compact in-kernel temporal logic evaluator, rules loaded from agent manifests |
| **Phase 17+** | Capability Flow Graph | Phase 3 (capabilities) | Periodic delegation chain analysis, confused deputy detection, escalation path detection |
| **Phase 42** | Agent Capability Intelligence | Phase 14a, Phase 41 (capability profiles) | 5-stage analysis pipeline, behavioral prediction, corpus comparison, profile suggestion |

### Dependency Chain

```text
Phase 3 (IPC + Caps) ──→ Phase 10 (AIRS) ──→ Phase 14a (Core Verifier)
                                              ├──→ Phase 14b (Structured Intent)
                                              ├──→ Phase 14c (Adversarial Defense)
                                              ├──→ Phase 16+ (Taint Labels, MTL)
                                              ├──→ Phase 17+ (Cap Flow Graph)
                                              └──→ Phase 42 (Capability Intelligence)
```

---

## §15 Design Principles

1. **Security is not optional.** Intent verification and behavioral monitoring are always active when AIRS is available. Agents cannot disable, bypass, or influence the verification process. The IntentVerifier's own instructions come from its manifest, not from agent messages.

2. **Algorithmic where possible, LLM where necessary.** Structured intent specifications (`IntentPurpose`, `TemporalSpec`, `DataFlowSpec`) enable machine-checkable pre-filters that operate in <0.01ms with no AIRS dependency. The LLM is consulted only for actions that fail or are ambiguous under algorithmic checking — approximately 20% of all verifications.

3. **Conservative fallback.** When AIRS is unavailable, the system does not silently permit all actions. Fallback policies are configurable per trust level: `Skip` (rely on Layers 2–8), `ReadOnly` (allow reads, block writes), or `BlockAll` (block all non-allowlisted actions). Lower trust levels default to more restrictive fallbacks — untrusted/sandboxed agents default to `BlockAll`, while system/verified agents default to `Skip` since they have the strongest Layer 2 capability constraints.

4. **Defense in depth.** Intent verification is necessary but not sufficient. It operates alongside capability enforcement (algorithmic, always-on), behavioral monitoring (statistical, always-on), resource limits, injection defense, audit trails, and cryptographic integrity. Each layer catches threats the others miss.

5. **Separation of security and resource paths.** Intent verification runs on a dedicated security code path within AIRS, isolated from resource optimization operations. A prefetch or compression operation never delays or influences an intent verification check. The security path has a hard <10ms SLA.

6. **Provenance over permission.** Beyond checking whether an action is permitted, the system tracks *where data came from* and *where it flows*. IPC taint labels enable the kernel to enforce information flow policies that capability checks alone cannot express — preventing cross-agent data exfiltration even when each individual agent's actions are capability-permitted.

---

## Cross-Reference Index

| Section | Sub-Document | Topic |
|---|---|---|
| §1 | This file | Core insight and overview |
| §2 | [pipeline.md](./intent-verifier/pipeline.md) | Architecture and component placement |
| §3 | [specification.md](./intent-verifier/specification.md) | Intent specification (DeclaredIntent + StructuredIntent) |
| §3.1 | [specification.md](./intent-verifier/specification.md) | DeclaredIntent (free-text) |
| §3.2 | [specification.md](./intent-verifier/specification.md) | StructuredIntent (machine-checkable) |
| §3.3 | [specification.md](./intent-verifier/specification.md) | Intent in agent manifests |
| §3.4 | [specification.md](./intent-verifier/specification.md) | Intent registration at task start |
| §4 | [pipeline.md](./intent-verifier/pipeline.md) | Verification pipeline |
| §4.1 | [pipeline.md](./intent-verifier/pipeline.md) | Action observation |
| §4.2 | [pipeline.md](./intent-verifier/pipeline.md) | Algorithmic pre-check |
| §4.3 | [pipeline.md](./intent-verifier/pipeline.md) | LLM semantic verification |
| §4.4 | [pipeline.md](./intent-verifier/pipeline.md) | Verification modes (sync/async) |
| §4.5 | [pipeline.md](./intent-verifier/pipeline.md) | Result caching |
| §4.6 | [pipeline.md](./intent-verifier/pipeline.md) | Multi-round adversarial self-testing |
| §5 | [information-flow.md](./intent-verifier/information-flow.md) | Information flow verification |
| §5.1 | [information-flow.md](./intent-verifier/information-flow.md) | IPC taint labels (DIFC) |
| §5.2 | [information-flow.md](./intent-verifier/information-flow.md) | Data flow graph construction |
| §5.3 | [information-flow.md](./intent-verifier/information-flow.md) | Cross-agent exfiltration detection |
| §6 | [behavioral.md](./intent-verifier/behavioral.md) | Behavioral monitor coordination |
| §6.1 | [behavioral.md](./intent-verifier/behavioral.md) | Layer 1 + Layer 3 tandem operation |
| §6.2 | [behavioral.md](./intent-verifier/behavioral.md) | Cumulative volume tracking |
| §6.3 | [behavioral.md](./intent-verifier/behavioral.md) | Fixed baseline snapshots |
| §6.4 | [behavioral.md](./intent-verifier/behavioral.md) | Intent anchoring |
| §7 | [security.md](./intent-verifier/security.md) | Capability system integration |
| §7.1 | [security.md](./intent-verifier/security.md) | Layer 1 + Layer 2 coordination |
| §7.2 | [security.md](./intent-verifier/security.md) | Capability flow graph analysis |
| §7.3 | [security.md](./intent-verifier/security.md) | Approval gates |
| §7.4 | [security.md](./intent-verifier/security.md) | Capability budget per trust level |
| §8 | [security.md](./intent-verifier/security.md) | Adversarial resistance |
| §8.1 | [security.md](./intent-verifier/security.md) | Threat model |
| §8.2 | [security.md](./intent-verifier/security.md) | Evasion techniques and defenses |
| §8.3 | [security.md](./intent-verifier/security.md) | AIRS self-protection |
| §9 | [behavioral.md](./intent-verifier/behavioral.md) | Temporal logic monitor |
| §9.1 | [behavioral.md](./intent-verifier/behavioral.md) | MTL evaluator design |
| §9.2 | [behavioral.md](./intent-verifier/behavioral.md) | Rule examples |
| §9.3 | [behavioral.md](./intent-verifier/behavioral.md) | Rule composition |
| §10 | [pipeline.md](./intent-verifier/pipeline.md) | Performance model |
| §10.1 | [pipeline.md](./intent-verifier/pipeline.md) | Latency budget |
| §10.2 | [pipeline.md](./intent-verifier/pipeline.md) | Throughput under load |
| §10.3 | [pipeline.md](./intent-verifier/pipeline.md) | Cache hit rates |
| §11 | [security.md](./intent-verifier/security.md) | Graceful degradation |
| §11.1 | [security.md](./intent-verifier/security.md) | Fallback policies |
| §11.2 | [security.md](./intent-verifier/security.md) | Per-trust-level configuration |
| §11.3 | [security.md](./intent-verifier/security.md) | AIRS unavailability scenarios |
| §12 | [intelligence.md](./intent-verifier/intelligence.md) | Testing and validation |
| §13 | [intelligence.md](./intent-verifier/intelligence.md) | AI-native intelligence |
| §14 | This file | Implementation order |
| §15 | This file | Design principles |
| §16 | [intelligence.md](./intent-verifier/intelligence.md) | Future directions |
| §17 | [intelligence.md](./intent-verifier/intelligence.md) | References |
