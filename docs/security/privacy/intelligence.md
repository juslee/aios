# AIOS Privacy Intelligence

Part of: [privacy.md](../privacy.md) — Privacy Architecture
**Related:** [ai-privacy.md](./ai-privacy.md) — AI privacy, [agent-privacy.md](./agent-privacy.md) — Agent privacy model

---

## §11 Kernel-Internal ML for Privacy

Kernel-internal ML provides lightweight, always-on privacy monitoring without AIRS dependency. These models run as frozen decision trees, Bloom filters, or simple statistical detectors in kernel space. They have no AIRS dependency — privacy protection operates at full capability even when the intelligence layer is offline.

### §11.1 Privacy Anomaly Detection

Lightweight anomaly detectors that flag unusual privacy-relevant behavior without requiring semantic understanding:

**Access frequency detector:**

Monitors per-agent access rates to sensitive data categories. Uses a Welford online variance estimator (same approach as [behavioral-monitor/detection.md](../../intelligence/behavioral-monitor/detection.md) §4.1) to track the rolling mean and standard deviation of access counts per category per time window.

```rust
/// Privacy access anomaly detector.
/// Runs in kernel space as part of capability check path.
pub struct PrivacyAnomalyDetector {
    /// Per-agent, per-category access statistics.
    pub baselines: BTreeMap<(AgentId, DataCategory), AccessBaseline>,
    /// Z-score threshold for anomaly flagging.
    pub z_threshold: f32,
    /// Minimum observations before baseline is valid.
    pub min_observations: u32,
}

/// Rolling baseline for access frequency.
pub struct AccessBaseline {
    /// Welford online mean.
    pub mean: f64,
    /// Welford online variance (M2 accumulator).
    pub m2: f64,
    /// Number of observations.
    pub count: u32,
    /// Current window access count.
    pub current_count: u32,
    /// Window start timestamp.
    pub window_start: Timestamp,
}
```

An agent that normally reads 10 PII objects per hour but suddenly reads 500 triggers an anomaly alert (z-score > threshold). The alert is logged as `PrivacyEvent::CollusionSuspected` and the Behavioral Monitor is notified for deeper analysis.

**Cross-zone flow detector:**

Monitors data flow volume between security zones. Each zone pair has a baseline flow rate. Sudden increases in Personal-to-Collaborative or Personal-to-Untrusted flow trigger alerts.

```rust
/// Cross-zone flow baseline.
pub struct ZoneFlowBaseline {
    /// Source and destination zones.
    pub from: SecurityZone,
    pub to: SecurityZone,
    /// Rolling mean bytes transferred per window.
    pub mean_bytes: f64,
    /// Variance accumulator.
    pub m2: f64,
    /// Observation count.
    pub count: u32,
}
```

**Budget consumption spike detector:**

Monitors the rate of privacy budget consumption. An agent that consumes 80% of its budget in the first 10% of the window is flagged. This catches "burst exfiltration" patterns where an agent rapidly reads sensitive data before the user can react.

### §11.2 Privacy Budget Prediction

A per-agent model that predicts privacy budget consumption based on historical patterns. This enables:

- **Preemptive warning** — Notify the user before an agent exhausts its budget: "Agent X is likely to run out of PII access in ~15 minutes based on current usage."
- **Anomaly detection** — If predicted consumption diverges from observed consumption (agent is consuming much more or less than expected), flag for review.
- **Budget sizing** — Historical consumption data informs the default budget assignment for new agents from the same developer.

```rust
/// Privacy budget predictor.
/// Uses exponential moving average of per-window consumption.
pub struct BudgetPredictor {
    /// EMA of per-category consumption (fraction of budget used per window).
    pub consumption_ema: [f64; 9],
    /// EMA decay factor (default 0.3 — weighs recent windows heavily).
    pub alpha: f64,
    /// Number of completed windows observed.
    pub windows_observed: u32,
}
```

The predictor runs at the start of each budget window. If `consumption_ema[category] > 0.8` (agent typically uses >80% of budget), a preemptive notification is sent.

### §11.3 Sensitive Data Detection (Lightweight)

A kernel-space system that detects common sensitive data patterns in IPC message payloads. This provides real-time taint label assignment for data that has not yet been classified by the Space Indexer's full classification pipeline.

**Detection methods:**

1. **Regex patterns** — Fast pattern matching for structured sensitive data:
   - Social Security Numbers: `\d{3}-\d{2}-\d{4}`
   - Credit card numbers: Luhn-validated 13-19 digit sequences
   - Email addresses: `[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}`
   - Phone numbers: International format patterns
   - API keys/tokens: High-entropy base64 strings >32 characters

2. **Bloom filter** — A pre-computed Bloom filter of known sensitive keywords (configurable, default includes medical terms, financial terms, legal terms). Used for fast "might contain sensitive content" pre-screening.

```rust
/// Lightweight PII detector for IPC message screening.
/// Runs in capability check path — must be fast (<1ms).
pub struct LightweightPiiDetector {
    /// Compiled regex patterns for structured PII.
    pub patterns: Vec<CompiledPattern>,
    /// Bloom filter for sensitive keywords.
    pub keyword_filter: BloomFilter,
    /// False positive rate target for Bloom filter.
    pub fp_rate: f64,
}

/// Detection result.
pub struct PiiDetectionResult {
    /// Whether any pattern matched.
    pub detected: bool,
    /// Matched categories (may be multiple).
    pub categories: DataCategorySet,
    /// Confidence level.
    pub confidence: PiiConfidence,
}

/// Confidence of PII detection.
pub enum PiiConfidence {
    /// Regex pattern match (high confidence).
    PatternMatch,
    /// Bloom filter match only (may be false positive).
    BloomFilterMatch,
    /// Both pattern and Bloom filter match.
    HighConfidence,
}
```

**Performance budget:** The PII detector runs in the capability check hot path. It must complete within 1ms for typical IPC message sizes (≤256 bytes). Regex patterns are compiled at boot time. The Bloom filter uses 64 KiB of memory per core (loaded into L1 cache).

**Taint label assignment:** When the PII detector flags a message, a `PrivacyTaintLabel` is attached with the detected categories. This provides immediate privacy protection while the Space Indexer's full classification (which may take seconds) completes asynchronously.

---

## §12 AIRS-Dependent Privacy Intelligence

When AIRS is active, it provides sophisticated privacy capabilities that go beyond what kernel-internal ML can achieve. These features require semantic understanding, context awareness, and multi-signal correlation.

### §12.1 Contextual Privacy Adaptation

AIRS analyzes user context from the Context Engine ([context-engine/inference.md](../../intelligence/context-engine/inference.md) §4) to dynamically adjust privacy posture. The privacy system becomes **context-aware** — stricter in risky environments, relaxed in trusted environments.

**Context signals that affect privacy:**

| Context Signal | Privacy Adjustment | Rationale |
|---|---|---|
| Public WiFi detected | Increase network-bound budget cost 4x | Higher exfiltration risk on untrusted networks |
| Home location (learned) | Relax location precision constraints | Lower risk of location-based targeting |
| Work hours (enterprise device) | Apply enterprise DLP policies | Compliance requirement during work |
| Multiple users nearby (audio) | Increase microphone consent requirements | Privacy of bystanders |
| Device physically secured | Reduce consent prompt frequency | Lower risk of unauthorized access |
| Unknown Bluetooth devices nearby | Tighten wireless data transfer policies | Potential surveillance devices |

**Adaptation mechanism:**

The Context Engine publishes context state changes to the `PrivacyCoordinator` via a `PrivacyContextSubscriber`. The coordinator adjusts privacy parameters:

```rust
/// AIRS-driven privacy adaptation.
pub struct PrivacyContextAdaptation {
    /// Current environment risk level (0.0 = safe, 1.0 = high risk).
    pub environment_risk: f32,
    /// Budget cost multipliers per category (1.0 = normal).
    pub budget_multipliers: [f32; 9],
    /// Consent level override (None = use default for trust level).
    pub consent_override: Option<ConsentStrictness>,
    /// Network privacy mode.
    pub network_mode: NetworkPrivacyMode,
}

pub enum ConsentStrictness {
    /// Relaxed — cached consent decisions are honored for longer.
    Relaxed,
    /// Normal — default consent behavior.
    Normal,
    /// Strict — all sensor access requires fresh consent.
    Strict,
    /// Lockdown — all non-system sensor access denied.
    Lockdown,
}

pub enum NetworkPrivacyMode {
    /// Normal — standard taint label enforcement.
    Normal,
    /// Cautious — increased budget cost for network operations.
    Cautious,
    /// Restricted — only essential network traffic allowed.
    Restricted,
}
```

**User override:** The user can override AIRS-driven adaptation at any time. A toggle in Settings: "Let AIRS adjust privacy based on context" (default: on). When off, static privacy parameters from the trust level table (§2.5) apply.

### §12.2 Privacy-Aware Agent Scoring

AIRS maintains a per-agent **privacy score** based on historical behavior. The score influences default budget assignment, monitoring intensity, and user warnings.

**Scoring dimensions:**

| Dimension | Weight | Measurement |
|---|---|---|
| Budget compliance | 0.3 | Fraction of windows where budget was not exhausted |
| Consent respect | 0.2 | Whether the agent honors consent revocations promptly |
| Data flow compliance | 0.2 | Fraction of data flows that match the privacy manifest |
| Retention compliance | 0.15 | Whether the agent respects retention policies |
| Anomaly history | 0.15 | Number of privacy anomalies attributed to this agent |

```rust
/// Per-agent privacy score maintained by AIRS.
pub struct AgentPrivacyScore {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Overall privacy score (0.0 = poor, 1.0 = excellent).
    pub score: f32,
    /// Per-dimension scores.
    pub dimensions: [f32; 5],
    /// Number of scoring windows observed.
    pub windows_observed: u32,
    /// Score trend (positive = improving, negative = degrading).
    pub trend: f32,
    /// Last update timestamp.
    pub last_updated: Timestamp,
}
```

**Score consequences:**

| Score Range | Label | Consequence |
|---|---|---|
| 0.8–1.0 | Excellent | Default budgets; standard monitoring |
| 0.6–0.8 | Good | Default budgets; standard monitoring |
| 0.4–0.6 | Fair | Reduced budgets (0.75x); increased monitoring |
| 0.2–0.4 | Poor | Restricted budgets (0.5x); user warning at launch |
| 0.0–0.2 | Critical | Minimal budgets (0.25x); user warning + confirmation required |

Scores are stored in `system/airs/privacy-scores/` and are visible to the user through the Inspector app. Agents cannot read their own privacy score (to prevent gaming).

### §12.3 Natural Language Privacy Queries

AIRS enables users to query their privacy state through natural language, routed through the Conversation Manager ([conversation-manager.md](../../intelligence/conversation-manager.md)):

**Supported query types:**

| Query Pattern | Example | Data Source |
|---|---|---|
| Agent access history | "What data has agent X accessed this week?" | Audit ring + capability logs |
| Sensor usage | "Which agents used my camera today?" | Sensor session audit logs |
| Data sharing | "What has been sent off-device this month?" | Network egress audit logs |
| Privacy score | "How trustworthy is agent X with my data?" | Agent privacy score (§12.2) |
| Classification | "What sensitive data do I have?" | Space Indexer classification |
| Retention | "What will be deleted soon?" | Retention enforcer schedule |

**Implementation:**

1. The Conversation Manager receives the natural language query.
2. AIRS classifies it as a privacy query and routes it to the Privacy Query Handler.
3. The handler translates the query into audit log searches, Space Indexer queries, and/or privacy score lookups.
4. Results are formatted as a structured privacy report.
5. The report itself is classified as `SensitivityLevel::Confidential` and stored in the user's Personal space with `RetentionTier::ShortTerm`.

**Privacy of privacy queries:** The query itself and its results are privacy-sensitive (they reveal what the user cares about protecting). Queries are not logged in the agent audit trail — only in the user's personal conversation history.

### §12.4 Future Directions

**On-device differential privacy for cross-device learning:**

When multi-device support enables fleet-wide model improvement (Phase 37+), differential privacy at the gradient level (DP-SGD) would ensure that individual device contributions cannot be reverse-engineered from the aggregate model. Target privacy budget: ε ≤ 8 per training round.

**Privacy-aware model selection:**

AIRS could choose between models based on privacy properties for sensitive tasks. A user asking about health topics could trigger selection of a model with `dp_trained: true` and `memorization_tested: true`, even if a larger non-DP model would produce better quality output.

**Homomorphic encryption for multi-device inference:**

For future multi-device scenarios where inference is split across devices (e.g., small model on phone, large model on desktop), homomorphic encryption could protect intermediate results during transfer. This is computationally expensive with current techniques but may become feasible with specialized hardware (ARM CCA, hardware FHE accelerators).

**Formal verification of privacy properties:**

Key privacy invariants could be formally verified:
- "No IPC path exists from a Personal-zone space to a network egress without a declassification gate" (information flow property).
- "No sensor session can deliver data without an active indicator" (sensor privacy property).
- "No agent can access data beyond its declared categories" (manifest compliance property).

These would use the same formal verification infrastructure planned for security properties ([static-analysis.md](../static-analysis.md)).
