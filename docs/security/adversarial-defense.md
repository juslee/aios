# AIOS Adversarial Defense

**Audience:** Kernel developers (enforcement), intelligence developers (detection), security researchers (threat model)
**Phase:** 13b (Agent Framework — Adversarial Defense + hint screening)
**Related:** [model.md](./model.md) — Security model (8 layers),
[model/layers.md](./model/layers.md) — Layer 5 summary,
[../intelligence/airs/intelligence-services.md](../intelligence/airs/intelligence-services.md) — AIRS intelligence services (§5.6),
[../intelligence/airs/security.md](../intelligence/airs/security.md) — AIRS security path isolation

---

## §1 Core Insight

Adversarial defense in AIOS is **structural, not heuristic**. The fundamental protection against prompt injection is an OS-level architectural guarantee: agent instructions come from the kernel (signed manifests, capability tokens, user constraints), and data from spaces, network, and user input is permanently labeled as DATA. This control/data plane separation means that even a fully "jailbroken" agent — one whose LLM has been convinced to ignore its system prompt — cannot escalate beyond its kernel-enforced boundaries.

This is fundamentally different from application-level prompt injection defenses, which attempt to make the LLM itself resist adversarial inputs. AIOS's position is that **LLM-level defenses are necessary but insufficient**. Research consistently shows that adaptive attacks bypass detection-only defenses at rates exceeding 50% (Zhan et al., NAACL 2025), and universal adversarial suffixes can evade all probes simultaneously at 93–99% success rates (Mršić et al., 2026). The only reliable defense is defense-in-depth where multiple independent layers — most of them AI-independent — contain the blast radius of any single compromise.

AIOS implements this through eight security layers (see [model.md](./model.md)), of which adversarial defense is Layer 5. Even if Layer 5 fails entirely:

- **Layer 2** (Capability enforcement) prevents unauthorized resource access
- **Layer 3** (Behavioral monitoring) detects anomalous action patterns
- **Layer 4** (Security zones) prevents cross-zone data flow
- **Layer 6** (Cryptographic enforcement) prevents reading encrypted spaces
- **Layer 7** (Provenance recording) ensures all actions are auditable
- **Layer 8** (Blast radius containment) limits damage from any single agent

**Design philosophy:**

- **Structural over heuristic** — control/data separation is an architectural invariant, not a pattern-matching heuristic that can be evaded
- **Defense in depth** — eight independent layers, five of which are AI-independent and immune to adversarial ML attacks
- **Fail closed on critical paths** — when AIRS is unavailable, agents operate with restricted permissions (read-only or allowlist-only), not with reduced detection
- **No feedback to adversaries** — screening decisions, hint acceptance, and detection results are never exposed to agents, preventing adaptive probing
- **Forensic completeness** — every adversarial event is recorded in a tamper-evident provenance chain for post-incident reconstruction

---

## Document Map

| Document | Sections | Content |
|---|---|---|
| **This file** | §1, §16–§17 | Overview, implementation order, design principles |
| [threat-model.md](./adversarial-defense/threat-model.md) | §2–§3 | Adversarial threat taxonomy, attack surface map |
| [control-data-separation.md](./adversarial-defense/control-data-separation.md) | §4 | Control/data plane separation protocol |
| [screening.md](./adversarial-defense/screening.md) | §5–§7 | Input screening, output validation, hint screening |
| [response.md](./adversarial-defense/response.md) | §8–§9 | Detection/response pipeline, forensics, incident reconstruction |
| [intelligence.md](./adversarial-defense/intelligence.md) | §10–§12 | Kernel-internal ML, AIRS-dependent intelligence, future directions |
| [testing.md](./adversarial-defense/testing.md) | §13–§15 | Red-team testing, classifier evaluation, formal properties, POSIX, cross-references |

---

## §16 Implementation Order

### Preparatory Work (Phases 3–10)

| Phase | Adversarial Defense Preparatory | Dependency |
|---|---|---|
| 3 | Capability system (Layer 2 enforcement) | IPC complete |
| 4 | Space storage with provenance chain (Layer 7) | Block engine |
| 5 | Security zones (Layer 4) and blast radius (Layer 8) | Spaces + capabilities |
| 10 | AIRS intelligence services framework | AIRS inference engine |

### Phase 14: Agent Framework

| Sub-phase | Steps | Dependency | Observable Result |
|---|---|---|---|
| 13a | Intent Verifier + Behavioral Monitor (Layers 1, 3) | Phase 11 | Agent actions verified against declared intent; behavioral baselines established |
| **13b** | **Adversarial Defense + hint screening (Layer 5)** | **Phase 14a** | **Input screening pipeline operational; control/data separation enforced; hint screening active** |
| 13c | Tool Manager + Agent Lifecycle | Phase 14b | Full agent framework operational with adversarial protection |

### Phase 14b Milestones

| Milestone | Steps | Target | Observable Result |
|---|---|---|---|
| M40 | ConstraintStore + ControlDataSeparator + data labeling | Week 1 | Agent instructions kernel-only; data labeled at IPC boundary |
| M41 | InputScreener + InjectionDetector + ML classifier stub | Week 2 | Pattern-based injection detection; screening responses enforced |
| M42 | OutputValidator + HintScreener + forensic logging | Week 3 | Exfiltration detection; hint screening active; full audit trail |

### Post-Phase 14b

| Phase | Enhancement |
|---|---|
| 14 | AIRS-dependent semantic injection detection (§11.1) |
| 17 | Inspector adversarial event dashboard |
| 39 | Formal verification of control/data separation invariants (§13.4) |
| 41 | AIRS adversarial red-teaming and adaptive pattern updates (§11.3, §11.5) |

---

## §17 Design Principles

1. **Instructions are kernel objects.** Agent constraints (manifest, capabilities, user preferences) live in kernel memory. No agent action, data flow, or adversarial input can modify them. This is the non-negotiable foundation.

2. **Data never becomes instructions.** All data entering an agent (space reads, network receives, user input) is labeled as DATA at the IPC boundary. The label is kernel metadata, not message payload. Labels cannot be removed or upgraded by agents.

3. **Detection is defense in depth, not primary defense.** Input screening (Layer 5) catches obvious injection patterns. But the primary defense is structural: Layers 2, 4, 6, and 8 are AI-independent and enforce constraints regardless of whether Layer 5 detects anything.

4. **No single detection layer is sufficient.** Research shows adaptive attacks bypass any individual detector. AIOS combines pattern matching, ML classification, behavioral anomaly detection, and semantic analysis — each catching different attack classes.

5. **Screening latency is bounded.** Pattern matching: <1ms (synchronous). ML classification: <10ms (async for non-destructive data). Screening must not bottleneck IPC message delivery.

6. **No feedback to agents about security decisions.** Agents never learn whether their input was screened, flagged, or sanitized. Hint acceptance is opaque. This prevents adaptive probing where an adversary iteratively refines attacks based on system responses.

7. **Forensic completeness.** Every adversarial event — detection, classification, response, quarantine — is recorded in the provenance chain with full content preservation. Post-incident reconstruction can replay the entire attack timeline.

8. **Graceful degradation.** When AIRS is unavailable, adversarial defense degrades to kernel-internal ML (frozen classifiers) plus structural enforcement (Layers 2, 4, 6, 8). Security never depends entirely on LLM availability.

9. **Classifier robustness is assumed incomplete.** The architecture explicitly accounts for classifier evasion (§2.5). Defense does not depend on classifiers being unbeatable — it depends on the combination of detection and enforcement layers.

10. **Self-monitoring.** AIRS resource orchestration directives are monitored by the kernel (see [model/layers.md](./model/layers.md) §2.3.1). If AIRS itself exhibits anomalous behavior, the kernel falls back to static heuristics.
