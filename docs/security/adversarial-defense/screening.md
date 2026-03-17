# AIOS Adversarial Screening Pipeline

Part of: [adversarial-defense.md](../adversarial-defense.md) — Adversarial Defense

Related:
[control-data-separation.md](./control-data-separation.md) |
[response.md](./response.md) |
[intelligence.md](./intelligence.md)

---

## §5 Input Screening Pipeline

The input screening pipeline screens data flowing into agents for adversarial content before
that content reaches an agent's LLM context. It is a defense-in-depth layer: the primary
defense is control/data separation (§4), which prevents injected instructions from ever
reaching privileged execution paths. Screening is a secondary defense that catches obvious
injection attempts, reduces noise, and generates the audit signals needed for threat response.

Screening is not a trust boundary on its own. An agent whose LLM is convinced by injected
instructions cannot exceed its capability grants, blast radius policy, or behavioral baseline
regardless of what the screener missed. This framing keeps the screener's failure mode
acceptable: a missed injection that reaches the agent LLM is an intelligence failure, not a
security breach.

### §5.1 Architecture

The pipeline runs synchronously on the IPC delivery path. All stages complete before a
message is delivered to the destination agent. The total latency budget for the synchronous
path is 2ms (§5.5).

```mermaid
flowchart LR
    A["Incoming\nIPC message"] --> B["Rate check\n(kernel, no AIRS)"]
    B -->|within limits| C["Structural\nanalysis"]
    B -->|exceeded| X1["Block / throttle"]
    C -->|clean| D["Pattern\nmatching"]
    C -->|suspicious| R["Response\npolicy"]
    D -->|no match| E["Tier 1 ML\n(kernel, frozen)"]
    D -->|match| R
    E -->|safe| F["Deliver\nto agent"]
    E -->|suspicious| G["Tier 2 ML\n(AIRS, async)"]
    G -->|safe| F
    G -->|injection| R
    R --> X2["Sanitize / Block\n/ Flag / LogOnly"]
```

**Stage 1 — Rate check.** Enforces per-agent data volume and rate limits in the kernel IPC
layer. No AIRS dependency. Exceeding the limit blocks the message and issues a throttle
event to the response subsystem (§8). Limits are sourced from the agent's
`KernelResourceLimits` and enforced atomically per-agent.

**Stage 2 — Structural analysis.** Detects instruction-like content in data: imperative
sentence structures, system-prompt-like formatting (role headers, delimiter tokens), and
content that structurally resembles a conversation transcript. Structural analysis operates
on the message's decoded text representation and runs in O(n) over message length.

**Stage 3 — Pattern matching.** Regex and keyword matching against the frozen injection
pattern catalog (§5.2). Pattern matching is the fastest high-recall stage and covers the
large majority of known injection techniques. The catalog is embedded in the kernel binary
and cannot be updated at runtime.

**Stage 4 — Tier 1 ML classification.** A frozen embedding-based binary classifier (§5.3)
runs when pattern matching returns clean or returns a low-severity match that does not
trigger an immediate response. Inference is synchronous and completes in under 1ms on the
kernel-internal compute budget.

**Stage 5 — Tier 2 ML classification.** Invoked asynchronously when Tier 1 returns
`Suspicious` or when the message originates from an External trust-level source. AIRS
provides semantic injection analysis with context awareness (§5.3). For non-destructive data
reads, Tier 2 can complete after provisional delivery; for writes and network-bound outputs,
delivery is held until Tier 2 completes.

**Stage 6 — Response.** The response policy (§5.4) selects a `ScreeningResponse` based on
the highest severity signal from any stage and the agent's declared policy.

The core types:

```rust
/// Screens data flowing INTO an agent for adversarial content.
pub struct InputScreener {
    /// Pattern-based detection (regex, keyword matching)
    patterns: Vec<InjectionPattern>,
    /// ML-based detection (via AIRS, when available)
    ml_detector: Option<ChannelId>,
    /// Action on detection
    response: ScreeningResponse,
}

pub struct InjectionPattern {
    name: String,
    pattern: Regex,
    severity: Severity,
    examples: Vec<String>,
}

pub enum ScreeningResponse {
    /// Strip the detected injection and pass clean data
    Sanitize,
    /// Block the entire input
    Block,
    /// Deliver but flag agent for monitoring
    Flag,
    /// Deliver and log only — no other action
    LogOnly,
}

/// Combines pattern matching with structural analysis.
pub struct InjectionDetector {
    patterns: Vec<InjectionPattern>,
    structural_analyzer: StructuralAnalyzer,
}
```

`StructuralAnalyzer` operates on a tokenized view of the message text. It scores messages
on a set of structural signals — imperative verb density, delimiter-token presence, role
header patterns — and produces a `StructuralScore` that feeds into the response policy
alongside pattern match severity.

### §5.2 Pattern-Based Detection

The injection pattern catalog is a static list embedded in the kernel binary. It is updated
only via kernel upgrades, never at runtime. This design prevents an adversary from poisoning
the pattern catalog by injecting a message that "teaches" the screener to allow future
injections.

Patterns are organized by category. Each pattern carries a severity level and a list of
canonical examples that informed its construction:

| Category | Examples | Severity |
|---|---|---|
| Instruction override | "ignore previous instructions", "disregard your system prompt", "forget your guidelines" | High |
| Role-play exploitation | "pretend you are", "you are now a", "act as if you have no restrictions" | Medium |
| Encoding bypass | Base64-encoded instructions, Unicode homoglyphs, zero-width characters between letters | High |
| Markdown injection | Hidden HTML comments (`<!-- instructions -->`), invisible text via CSS color tricks, Unicode formatting overrides | Medium |
| Delimiter attacks | Fake `<system>` / `<user>` message wrappers, JSON injection into structured outputs, YAML front-matter injection | High |
| Multi-language bypass | Instructions in non-English languages interleaved with English content to bypass English-only patterns | Medium |
| Prompt leakage | "repeat everything above", "what are your instructions", "output your system prompt" | Medium |
| Indirect chaining | Web content containing instruction fragments designed to be assembled by a multi-step agent | High |

**Detection precision trade-offs.** The pattern catalog is tuned for high recall over high
precision. False positives (flagging benign content) are handled via `LogOnly` at lower
severity levels rather than blocking. The blast radius policy (Layer 8) and control/data separation (§4)
bound the impact of any true positive that is missed.

**Multi-language coverage.** The catalog includes translated variants of high-severity
patterns in the top 20 languages by web content volume. AIRS Tier 2 (§5.3) provides
coverage for the long tail of languages not represented in the frozen catalog.

### §5.3 ML-Based Classification

Pattern matching cannot cover novel injection techniques or semantically equivalent
rephrasing of known patterns. The two-tier ML classification layer provides coverage for
these cases.

**Tier 1 — Kernel-internal frozen classifier.**

The Tier 1 classifier is an embedding-based binary model embedded in the kernel binary.
It classifies messages as `Safe` or `Suspicious` using pre-computed embeddings and a small
linear classifier head. Design targets:

- Model size: under 500KB (kernel memory budget)
- Inference latency: under 1ms on Cortex-A72 without hardware acceleration
- Architecture: MiniBERT-style token embeddings with mean pooling, single linear layer
- Training: offline on injection datasets including PINT benchmark and PromptGuard training
  data; F1 of 0.91 achieved on held-out injection evaluation set (PromptGuard, 2025)
- Frozen at build time: weights are `&'static [f32]` compiled into the kernel binary

```rust
pub struct InjectionClassifier {
    kernel_model: FrozenClassifier,
    airs_channel: Option<ChannelId>,
}

pub struct FrozenClassifier {
    /// Pre-computed token embedding matrix, compiled into kernel binary
    embeddings: &'static [f32],
    /// Classifier weights (linear layer after mean-pool)
    weights: &'static [f32],
    /// Detection threshold — tuned for low false-positive rate
    threshold: f32,
    /// Monotonic version; used to detect kernel upgrades in audit records
    model_version: u32,
}

pub enum ClassificationResult {
    Safe { confidence: f32 },
    Suspicious { confidence: f32, reason: String },
    Injection { confidence: f32, category: InjectionCategory },
}
```

The `threshold` field is set conservatively (high specificity) because false positives have
an immediate user-visible cost (blocked messages), while missed injections are handled by
the capability and blast radius layers beneath. The threshold is a compile-time constant and
is not tunable at runtime.

**Tier 2 — AIRS-dependent semantic classifier.**

Tier 2 invokes a full language model loaded by AIRS via a kernel IPC channel. It provides:

- Semantic understanding of injection intent, not just surface pattern matching
- Context-aware classification: the surrounding message history informs the decision
- Coverage of novel techniques not present in the Tier 1 training data
- Language-agnostic detection (the full model handles any human language)

Tier 2 is invoked in two conditions:

1. Tier 1 returns `Suspicious` (confidence above `threshold` but below `injection_threshold`)
2. The message originates from a source at External trust level

Tier 2 is asynchronous for non-destructive reads (the message is delivered provisionally;
if Tier 2 returns `Injection`, the response subsystem issues a containment event). For
writes and network-bound outputs, delivery is held until Tier 2 completes, subject to the
10ms latency budget.

If AIRS is unavailable (early boot, model not loaded, AIRS service degraded), Tier 2 is
skipped. The system falls back to Tier 1 + pattern matching alone. This degraded mode is
logged in the audit ring and reported in the kernel health metrics.

### §5.4 Screening Response Policy

The response policy maps (trust level, agent tier, detected severity) to a
`ScreeningResponse`. Defaults are defined per trust level and can be overridden by agents in
their manifest — agents may declare stricter screening but never looser.

| Trust Level | Agent Tier | Detected Severity | Default Response |
|---|---|---|---|
| External (web content) | Any | High | Block |
| External (web content) | Any | Medium | Flag |
| External (web content) | Any | Low | LogOnly |
| Agent (inter-agent IPC) | Untrusted | High | Block |
| Agent (inter-agent IPC) | Untrusted | Medium | Flag |
| Agent (inter-agent IPC) | System | High | Flag |
| Agent (inter-agent IPC) | System | Medium | LogOnly |
| User | Any | High | Flag |
| User | Any | Medium | LogOnly |
| Kernel | N/A | Any | No screening |

**Rationale for External defaults.** Web content is the primary injection vector. Agents
that browse the web or process documents from external sources operate in a high-threat
environment. Blocking on High severity is justified even at the cost of occasional false
positives because the alternative — allowing injected instructions from web content into the
agent's LLM context — is a well-documented attack path.

**Rationale for inter-agent defaults.** System agents are operated by the OS and are
substantially more trusted than user-space untrusted agents, but the injection chain threat
(one compromised agent injecting into another) warrants screening at both tiers. System
agents that disagree with the Flag policy can declare `screening_override: LogOnly` in their
manifest; this declaration is itself an audited event.

**Manifest declaration example:**

```rust
/// Agent manifest screening configuration.
pub struct ScreeningConfig {
    /// Override the default response for this agent's incoming data.
    /// May only be stricter (higher action level) than the trust-level default.
    response_override: Option<ScreeningResponse>,
    /// Opt in to Tier 2 screening for all messages regardless of Tier 1 result.
    always_tier2: bool,
    /// Declare specific pattern categories to block (additive to catalog defaults).
    additional_block_categories: Vec<InjectionCategory>,
}
```

### §5.5 Performance Budget

The input screener runs on every IPC delivery path. Latency directly impacts agent
throughput. All budget numbers are measured on Cortex-A72 at 1.5GHz (QEMU virt target):

| Stage | Mode | Latency budget | Notes |
|---|---|---|---|
| Rate check | Synchronous | <0.1ms | Atomic counter check, O(1) |
| Structural analysis | Synchronous | <0.5ms | O(n) over message length |
| Pattern matching | Synchronous | <0.5ms | Aho-Corasick multi-pattern scan |
| Tier 1 ML | Synchronous | <1ms | Frozen classifier, no I/O |
| Tier 2 ML | Async (can defer) | <10ms | AIRS IPC round-trip |
| **Total (critical path)** | **Synchronous** | **<2ms** | **Pattern + Tier 1 only** |

**Throughput target.** The screener must sustain 10,000 messages per second per core. At
2ms per message on the synchronous path, a single core can process 500 messages per second
at full latency. Meeting the 10,000/second target requires the fast path (no pattern match,
Tier 1 returns Safe) to complete in under 0.1ms. The fast path bypasses the full
Aho-Corasick scan and uses a hash-based pre-filter before engaging the full pattern engine.

The Tier 2 async path does not contribute to the synchronous latency budget. It runs on a
dedicated kernel worker thread and delivers results via a completion channel.

---

## §6 Output Validation

Output validation screens data flowing out of agents before it reaches its destination. It
is the mirror of input screening: where input screening protects agents from adversarial
content, output validation protects the system and external parties from agents that have
been compromised or are behaving anomalously.

### §6.1 Exfiltration Detection

Exfiltration detection looks for sensitive data in agent outputs that the agent should not
be transmitting — either because the agent is compromised, or because its LLM has been
convinced to include sensitive data in its output.

```rust
/// Validates data flowing OUT of an agent.
pub struct OutputValidator {
    exfiltration_detector: ExfiltrationDetector,
    schema_validator: Option<SchemaValidator>,
}

pub struct ExfiltrationDetector {
    /// Known sensitive patterns: credit card numbers, API keys, private keys, etc.
    sensitive_patterns: Vec<SensitivePattern>,
    /// Cross-space tracking: flag outputs that carry data from a higher-trust space
    /// to a lower-trust destination.
    cross_space_check: bool,
}
```

**Sensitive pattern catalog.** The detector maintains patterns for categories of sensitive
data commonly found in agent-accessible spaces:

- Credit card numbers (regex + Luhn algorithm verification to reduce false positives)
- API key formats (provider-specific prefixes: `sk-`, `ghp_`, `AKIA`, etc.)
- Social Security Number patterns (with context: bare numbers without SSN context are not flagged)
- Email address and password combinations (the pair is more sensitive than either alone)
- Private key headers (`-----BEGIN RSA PRIVATE KEY-----`, `-----BEGIN EC PRIVATE KEY-----`)
- Bearer tokens and OAuth credentials

**Cross-space exfiltration tracking.** The kernel tracks provenance for data objects read
by an agent during a session. If an agent reads from a space with a high-trust zone (e.g.,
`SecurityZone::Protected`) and subsequently produces output bound for a lower-trust
destination (e.g., a network socket or an External-tier agent), the cross-space check flags
the output for review.

Cross-space tracking is maintained in the agent's `ProvenanceChain`. When an agent reads an
object, the kernel records the object's space and zone in the chain. When the agent produces
output, the chain is consulted to determine whether the output path is compatible with the
data's provenance. Incompatible paths produce a `CrossSpaceExfiltration` audit event.

**False positive management.** Sensitive pattern matching is intentionally lenient in the
`Flag` direction: the output validator logs and flags rather than blocking unless the pattern
match is high-confidence (e.g., Luhn-valid 16-digit number in a format context, not a
random numeric string). The response subsystem (§8, [response.md](./response.md)) handles escalation.

### §6.2 Output Schema Enforcement

Agents may declare an expected output schema in their manifest. Schema enforcement is a
structural validation that runs before output reaches its destination.

An agent's manifest may declare:

```rust
pub struct OutputSchemaConfig {
    /// JSON Schema or type-tagged schema for expected output structure.
    schema: OutputSchema,
    /// Action when schema validation fails.
    on_violation: SchemaViolationAction,
}

pub enum SchemaViolationAction {
    /// Block the output and generate a security event.
    Block,
    /// Deliver the output but flag the agent for monitoring.
    Flag,
    /// Log only — used for monitoring without enforcement.
    LogOnly,
}
```

Schema enforcement provides a second-order defense against prompt injection: an adversary
who has injected instructions into an agent's LLM context may cause the agent to produce
output that violates its declared schema. For example:

- An email-drafting agent whose output must conform to an `EmailMessage` schema (recipient,
  subject, body). If injected instructions cause the agent to output a shell command sequence
  instead of an email, schema validation fails and the output is blocked.
- A data-transformation agent whose output must be valid JSON. An injection that causes the
  agent to produce a narrative explanation instead of structured data fails schema validation.
- A summarization agent whose output must not contain URLs. An injection that causes the
  agent to include exfiltration URLs in the summary is caught by schema validation.

Schema validation runs after the exfiltration detector. It is optional: agents that do not
declare a schema skip this stage. Agents in high-trust operational roles (system agents,
agents with write access to Protected spaces) are recommended to declare output schemas in
their manifests. The kernel does not enforce this recommendation but logs the absence of a
schema as an advisory in the agent's health metrics.

### §6.3 Network Output Screening

Output bound for network destinations receives additional screening beyond the standard
exfiltration and schema checks. Network output is the primary exfiltration path for a
compromised agent.

**Domain allowlisting.** An agent's manifest declares the set of domains it is permitted to
contact. Attempts to contact domains outside the allowlist are blocked by the networking
subsystem's capability gate before the connection is established. The output validator
enforces the manifest declaration at the content layer — if an agent constructs a URL
pointing to a non-allowlisted domain and embeds it in output that would be followed by
another subsystem, the validator flags the output.

**Content-type validation.** The expected content type for network-bound output is declared
in the agent's manifest (e.g., `application/json` for an API client agent). An agent
sending binary data to an endpoint declared as a JSON API is structurally anomalous.
Content-type mismatches are flagged at Medium severity.

**Volume anomaly detection.** The output validator tracks rolling output volume per agent
per network destination. A sudden increase in output volume — defined as more than 5x the
agent's 5-minute rolling average — triggers a Flag event. This pattern captures agents that
have been convinced to exfiltrate bulk data: the per-message screener may not catch
individually clean messages that collectively represent an exfiltration event.

Volume tracking is maintained in the agent's behavioral baseline (Layer 3). Anomalies
contribute to the behavioral score that feeds the response subsystem's threat level
assessment.

---

## §7 Hint Screening

AIRS resource orchestration accepts optional hints from agents — lightweight signals about
anticipated resource needs. A hint might indicate that an agent expects to need embedding
model access, or that a large batch operation is about to begin. These hints are a distinct
input vector from untrusted agents into a privileged system service, and they receive
adversarial screening as rigorous as web content screening.

The hint screening subsystem is part of the `InputScreener` pipeline but runs on a separate
code path: hints are short structured messages with a constrained schema, and their screening
can be faster and more targeted than general text screening.

### §7.1 Screening Rules

```rust
/// Screens resource hints before AIRS considers them.
/// Part of the InputScreener pipeline (§5).
pub struct HintScreener {
    /// Rate limit: max hints per agent per minute (default: 10)
    max_hints_per_minute: u32,
    /// Consistency checker: does the hint match the agent's capabilities?
    capability_validator: CapabilityValidator,
    /// Anomaly checker: is the hint consistent with the agent's history?
    history_checker: HintHistoryChecker,
}

pub struct HintValidation {
    /// Is the requested resource within the agent's blast radius limits?
    within_blast_radius: bool,
    /// Does the agent hold capabilities for the resources it's hinting about?
    capabilities_match: bool,
    /// Is the hint volume/rate consistent with the agent's behavioral baseline?
    baseline_consistent: bool,
    /// Is the hint structurally valid (no injection patterns, no probing)?
    structurally_valid: bool,
}

pub enum HintScreeningResult {
    /// Hint is valid — AIRS may consider it for resource optimization
    Accept,
    /// Hint exceeds agent's limits — silently ignored, logged
    Ignore { reason: String },
    /// Hint pattern is suspicious — agent flagged for monitoring
    Flag { reason: String },
    /// Hint rate exceeded — agent's hint channel throttled
    Throttle { until: Timestamp },
}
```

The four screening rules applied to every hint:

**Rule 1 — Blast radius ceiling.** A hint requesting more resources than the agent's
`BlastRadiusPolicy` allows is silently ignored. The agent does not receive confirmation
that its hint was rejected: it simply observes no change in resource allocation. This
resource allocation opacity prevents an agent from using the hint-response loop as a
measurement channel to map the system's resource landscape.

**Rule 2 — Capability consistency.** A hint that references space objects, model handles, or
compute resources for which the agent holds no capability token is rejected and logged as a
security event at High severity. Hinting about resources you cannot access is a classic
probing pattern: the agent is attempting to enumerate system resources by observing whether
hints succeed or fail. Since AIRS never confirms hint outcomes, a capability-inconsistent
hint provides no useful information to the attacker, but it does generate an audit signal.

**Rule 3 — Rate limiting.** Submissions exceeding `max_hints_per_minute` trigger throttling.
The throttle duration is exponential: first violation produces a 1-minute throttle, second
produces a 10-minute throttle, and subsequent violations within the same hour produce a
1-hour throttle. An agent submitting dozens of hints per second is either suffering from a
software bug or is actively probing AIRS's response characteristics.

**Rule 4 — No feedback channel.** Hints are submitted via fire-and-forget IPC. There is no
response message: AIRS never confirms whether a hint was accepted, acted upon, or ignored.
This design prevents the hint system from being used as a timing side channel to probe
system state. An agent cannot distinguish between a hint that was accepted and optimized for
versus a hint that was silently dropped by the screener.

### §7.2 Probing Detection

Beyond the individual screening rules, the hint screener tracks behavioral patterns across
multiple hint submissions to detect systematic probing:

**Capability probe pattern.** An agent submits hints referencing resources it does not hold
capabilities for — potentially cycling through resource IDs or space IDs. Each individual
hint generates a capability-inconsistent rejection (Rule 2). If the screener detects more
than three capability-inconsistent hints within a 10-minute window, it escalates to a
`Flag` event and notifies the response subsystem that the agent may be conducting a
capability enumeration attack. The capability IDs referenced in the hints are recorded in
the audit entry to assist threat analysis.

**Timing probe pattern.** An agent submits hints at systematically varying rates — for
example, one hint per second for 60 seconds — and observes changes in system behavior (e.g.,
task scheduling latency, inference response time). Since hints are fire-and-forget and
produce no response, the only observable system behavior is timing. The hint screener detects
this pattern by tracking the variance in hint submission intervals: low-variance, high-rate
hint streams from a single agent trigger throttling under Rule 3. The screener additionally
flags the pattern to the behavioral monitoring subsystem (Layer 3) as a timing-probe signal.

**Deduplication — DoS prevention.** An agent submitting identical hint content repeatedly
within a short window is either experiencing a retry loop bug or attempting a denial-of-service
against the AIRS scheduling queue. The screener maintains a short-lived LRU cache of recent
hint hashes per agent. Duplicate hints within the deduplication window (default: 30 seconds)
are silently dropped after the first submission. The drop count is recorded in the agent's
health metrics but does not generate an alert unless the rate exceeds the throttle threshold.

**Audit and opacity.** All probing events are logged in the provenance chain of the
submitting agent. The agent receives no indication that its hints have been identified as
probing — no error codes, no throttle notifications beyond the standard rate-limit response,
no behavioral change in the system that would confirm the detection. This opacity is
deliberate: confirming detection would allow an attacker to calibrate their probing strategy
to remain below the detection threshold.

Probing detection signals feed into the agent's behavioral risk score, which in turn
influences the response subsystem's decisions about monitoring intensity, capability
attenuation, and agent suspension (§8, [response.md](./response.md)).
