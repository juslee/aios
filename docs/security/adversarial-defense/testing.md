# AIOS Adversarial Defense Testing and Verification

Part of: [adversarial-defense.md](../adversarial-defense.md) — Adversarial Defense

**Related:** [threat-model.md](./threat-model.md) — Adversarial threat taxonomy,
[screening.md](./screening.md) — Input screening, output validation, hint screening,
[intelligence.md](./intelligence.md) — Kernel-internal ML and AIRS intelligence

---

## §13 Testing and Verification

The adversarial defense subsystem requires a testing strategy that goes beyond conventional unit and integration tests. Adversarial attacks are creative, context-dependent, and continuously evolving — a static test suite is insufficient. The strategy combines three complementary approaches: red-team testing finds attacks the system should catch, classifier evaluation measures statistical accuracy across known corpora, and integration testing verifies that containment holds end-to-end when individual components are bypassed or disabled.

These three approaches are designed to be independently valuable. A red-team suite identifies whether specific known attacks are caught; classifier metrics identify statistical regression; integration tests identify whether the 8-layer architecture holds under realistic attack conditions. All three must pass for a release to be considered secure.

### §13.1 Red-Team Testing Framework

The red-team suite is a structured corpus of adversarial test cases organized according to the §2 threat taxonomy. Each test case specifies not just whether detection is expected, but which security layer should catch it — this validates defense-in-depth rather than just aggregate pass/fail rates.

```rust
pub struct RedTeamSuite {
    /// Injection test corpus, categorized by §2 taxonomy
    injection_corpus: Vec<InjectionTestCase>,
    /// Evasion test suite covering techniques from §2.5
    evasion_corpus: Vec<EvasionTestCase>,
    /// Multi-agent attack scenarios from §2.4
    multi_agent_scenarios: Vec<MultiAgentScenario>,
    /// Accumulated results from the current run
    results: Vec<TestResult>,
}

pub struct InjectionTestCase {
    id: TestId,
    /// Category from §2 taxonomy
    category: InjectionCategory,
    input: String,
    expected_detection: bool,
    /// Which layer should catch this — validates defense-in-depth
    expected_layer: SecurityLayer,
    severity: Severity,
}

pub enum InjectionCategory {
    DirectPromptInjection,
    IndirectInjection,
    Jailbreak,
    MultiAgent,
    Evasion,
    HintAbuse,
    SupplyChain,
}

pub struct TestResult {
    case_id: TestId,
    detected: bool,
    detecting_layer: Option<SecurityLayer>,
    latency_us: u64,
    passed: bool,
}
```

Corpus categories map directly to the §2 taxonomy so that a new attack class added to the taxonomy can be immediately reflected in new test cases. Each test case additionally records which layer is expected to fire — this makes it possible to detect regressions where detection moves from an early layer (InputScreener) to a later one (capability enforcement), which may indicate the earlier defense has been weakened even if the overall detection rate is unchanged.

The corpus is maintained under version control alongside the kernel. It is updated with each kernel release to include:

- New attack techniques documented in security literature since the previous release
- Variants of existing attacks discovered through fuzzing (see [fuzzing.md](../fuzzing.md))
- Cases derived from AIRS red-teaming results (§11.3), once those are available

The red-team suite runs on every kernel build in CI. A test run that passes previously-passing cases but fails newly-added ones will fail CI, preventing regressions in both directions.

External corpus references include Promptfoo (prompt injection test harness), Garak (100+ attack modules, open source), and AgenticRed (2026, multi-agent attack scenarios). Cases from these tools are imported into the AIOS corpus and adapted to the AIOS threat model where needed.

### §13.2 Classifier Evaluation

The kernel-internal ML classifier (§10) and AIRS Tier 2 classifier (§11) are evaluated against separate precision/recall metrics. The evaluation set is maintained independently from the training set to prevent overfitting. Both sets are versioned and frozen at build time.

| Metric | Target | Measurement Method |
|---|---|---|
| True positive rate (injection detected) | >95% on known corpus | Run injection corpus through classifier |
| False positive rate (clean data flagged) | <0.1% | Run 10,000-sample clean data corpus |
| F1 score | >0.90 | Harmonic mean of precision and recall |
| Evasion resistance | >70% on evasion corpus | Run GCG/encoding bypass suite |
| Latency, kernel-internal ML | <1ms p99 | Benchmark on target hardware (cortex-a72) |
| Latency, AIRS Tier 2 | <50ms p99 | Benchmark with AIRS running on device |

The evasion resistance target is 70%, not 100%. This is intentional. The architecture accounts for classifier bypass through Layers 2–8 — capability enforcement, behavioral monitoring, security zones, output validation, and blast radius containment all operate independently of the Layer 5 classifier. The classifier's role is to catch most attacks early; the remaining layers handle the rest. Setting an achievable target avoids false confidence from overfitting to the evasion corpus.

Dataset versioning follows this convention:

```text
classifier_v{N}_trained_{YYYY-MM-DD}_eval_{YYYY-MM-DD}
```

Training set changes require a version increment. Evaluation set changes do not increment the version but do require a date update. This convention makes it possible to audit which training data influenced a given classifier version and to reproduce historical evaluation results.

Regression thresholds are enforced in CI: if any metric falls below 90% of its target value compared to the previous tagged release, the build fails. This allows gradual improvement without blocking development on minor fluctuations, while preventing significant regressions from going unnoticed.

### §13.3 Integration Testing

Integration tests run in QEMU with `-nographic` and validate UART output against expected strings. Each test exercises a complete attack scenario end-to-end, including the adversarial input delivery, the detection mechanism, the containment response, and the audit record generation.

The six primary integration scenarios are:

**Scenario 1 — Injection containment.** Inject adversarial content into an agent's incoming IPC message stream. Expected: InputScreener fires at Tier 1, agent process is paused, message is quarantined, audit record is written with `event_type = InjectionDetected`.

```text
PASS: adv-def/injection-containment
  InputScreener detected: true
  Agent paused: true
  Quarantine entry: true
  Audit record: InjectionDetected pid=7 ts=12345
```

**Scenario 2 — Exfiltration prevention.** Agent attempts to send content matching a sensitive data pattern to a network endpoint. Expected: OutputValidator blocks the send, a notification is delivered to the user agent, audit record written with `event_type = ExfiltrationBlocked`.

**Scenario 3 — Multi-agent injection chain.** Agent A writes adversarial content to a shared Space. Agent B reads it. Expected: InputScreener detects the injection on Agent B's read path, Agent A's write is recorded in Space provenance as the origin, both events appear in the audit trail.

**Scenario 4 — Jailbreak with capability containment.** The Tier 1 classifier is disabled for this test to simulate a bypass. Agent attempts an unauthorized syscall that its capability set does not permit. Expected: Layer 2 (capability enforcement) blocks the syscall, audit trail records both the classifier-bypass event and the capability rejection, confirming defense-in-depth held.

**Scenario 5 — Hint abuse with rate limiting.** Agent floods the AIRS hint channel at 10× the normal rate. Expected: HintScreener rate-limits the channel, no system state leaks through hint responses, counter metrics show rate-limit firings.

**Scenario 6 — AIRS fallback to kernel-internal ML.** AIRS is disabled (simulating model unavailability). An injection attack is delivered. Expected: kernel-internal ML classifier still detects the injection, structural enforcement (DataLabel enforcement) still blocks execution of labeled data, containment proceeds normally. This validates that the system does not degrade silently when AIRS is unavailable.

Each scenario produces a PASS/FAIL result and a structured log line to UART. The test harness compares these against expected strings using the same acceptance-criteria pattern as phase implementation tests.

### §13.4 Formal Properties

The following properties are candidates for formal verification in Phase 40, which introduces static analysis and formal verification tooling. These properties are stated informally here for design clarity and will be encoded in the formal verification framework when it becomes available.

**Property 1 — Data integrity.** No code path exists where DATA-labeled content becomes an INSTRUCTION source without explicit user authorization.

```text
Formalization:
  ∀ msg ∈ IPC_messages:
    label(msg) = DATA → msg ∉ ConstraintStore
```

**Property 2 — Screening completeness.** Every IPC message originating from an External or Agent trust level source passes through InputScreener before delivery to the receiving agent.

```text
Formalization:
  ∀ msg: trust_level(sender(msg)) ∈ {External, Agent} →
    screened(msg) = true
```

**Property 3 — Label immutability.** Agents cannot modify the DataLabel on a received message. The label assigned at send time is the label observed at receive time.

```text
Formalization:
  ∀ agent, msg: recv(agent, msg) →
    label(msg) = label_at_send(msg)
```

**Property 4 — Provenance completeness.** Every adversarial event that triggers a defense response produces at least one provenance record in the chain. No adversarial event is silently dropped.

```text
Formalization:
  ∀ event ∈ adversarial_events →
    ∃ record ∈ provenance_chain: record.event_id = event.id
```

**Property 5 — Capability containment.** A jailbroken agent — one whose LLM constraint system has been bypassed — cannot perform actions outside its assigned capability set. Capability enforcement is independent of LLM behavior.

```text
Formalization:
  ∀ agent, action:
    execute(agent, action) → action ∈ capabilities(agent)
```

These five properties collectively encode the core security invariants of the adversarial defense subsystem. Properties 1–3 address the control/data separation layer (§4). Property 4 addresses the provenance and forensics layer (§9). Property 5 addresses the capability enforcement layer (Layer 2), confirming that structural enforcement holds even when semantic defenses are bypassed.

VeriGuard (2025) and FIDES (Microsoft, 2025) demonstrate formal verification approaches for LLM agent safety properties using similar formulations. The AIOS approach will draw on these prior results when Phase 40 formal verification tooling is integrated.

---

## §14 POSIX Compatibility

Linux binaries run under the ELF loader sandbox defined in [linux-compat.md](../../platform/linux-compat.md) §9. The sandbox is treated as a Trust Level 4 (Untrusted) agent for adversarial defense purposes. This means the full adversarial defense pipeline applies to it without special exceptions.

The mapping from Linux binary execution to adversarial defense components is as follows:

All input to the sandbox — data read from AIOS Spaces via the POSIX bridge, data received from IPC channels, and data fetched from the network — is screened by InputScreener using External trust level screening rules. This is the most restrictive screening profile, appropriate for content that may originate from untrusted external sources.

System calls issued by the sandbox are translated through the syscall translation layer ([linux-compat/syscall-translation.md](../../platform/linux-compat/syscall-translation.md) §5), which enforces AIOS capability checks on each translated syscall. A Linux binary cannot bypass capability enforcement by issuing Linux syscalls — every translated syscall goes through the same capability gate as native AIOS calls.

Network connections initiated by the sandbox go through the OutputValidator pipeline, with the same exfiltration detection rules that apply to native agents. A Linux binary that attempts to send sensitive data to a network endpoint will be blocked by the same mechanism that blocks a native agent attempting the same action.

POSIX file descriptor reads from AIOS Spaces receive DATA labels transparently. The POSIX bridge applies DataLabel assignment at the read boundary, so content read from a Space by a Linux binary carries the same label it would carry if read by a native agent. This ensures that the control/data separation invariant (Property 1, §13.4) holds across the POSIX compatibility boundary.

The sandbox itself is recorded in the provenance chain as a process with trust level Untrusted. Any adversarial event originating from a Linux binary is attributable to the sandbox process and traceable through the same audit mechanisms used for native agents.

---

## §15 Cross-Reference Index

| Section | External Reference | Content |
|---|---|---|
| §1 | [model.md](../model.md) §1 | 8-layer security model overview |
| §2 | [model/layers.md](../model/layers.md) §2.5 | Layer 5 adversarial defense summary |
| §3 | [model/operations.md](../model/operations.md) §6 | Detection-response pipeline |
| §4 | [airs/intelligence-services.md](../../intelligence/airs/intelligence-services.md) §5.6 | AdversarialDefense service and ControlDataSeparator |
| §5–§7 | [model/layers.md](../model/layers.md) §2.5–§2.5.1 | InputScreener, OutputValidator, HintScreener |
| §8 | [model/operations.md](../model/operations.md) §6.1–§6.3 | Escalation policy and response actions |
| §10 | [airs/ai-native.md](../../intelligence/airs/ai-native.md) §13 | Kernel-internal ML patterns |
| §11 | [airs/ai-native.md](../../intelligence/airs/ai-native.md) §14 | AIRS-dependent intelligence patterns |
| §11.2 | [airs/intelligence-services.md](../../intelligence/airs/intelligence-services.md) §5.4 | Intent Verifier |
| §11.4 | [airs/intelligence-services.md](../../intelligence/airs/intelligence-services.md) §5.5 | Behavioral Monitor |
| §14 | [linux-compat.md](../../platform/linux-compat.md) §9 | ELF loader sandbox security |
| — | [applications/agents.md](../../applications/agents.md) | Agent manifest and lifecycle |
| — | [applications/inspector.md](../../applications/inspector.md) | Security dashboard and audit UI |
| — | [security/fuzzing.md](../fuzzing.md) | Fuzz testing of the screening pipeline |
