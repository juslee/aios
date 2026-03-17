# AIOS Behavioral Monitor — AI-Powered Detection Intelligence

Part of: [behavioral-monitor.md](../behavioral-monitor.md) — Behavioral Monitor Architecture
**Related:** [detection.md](./detection.md) — Statistical detection engine, [evasion.md](./evasion.md) — Evasion resistance, [ai-native.md](../airs/ai-native.md) — AIRS AI-native intelligence (§13.5 TokenAnomalyDetector), [intelligence-services.md](../airs/intelligence-services.md) — Agent Capability Intelligence (§5.9)

-----

The Behavioral Monitor's detection intelligence operates at two tiers. **Tier 1** (§12) uses kernel-internal statistical models — fixed-size state, O(1) per observation, no LLM dependency. **Tier 2** (§13) uses AIRS-dependent semantic analysis — LLM inference, retrieval-augmented generation, and cross-agent correlation. §16 describes future directions informed by research in adversarial ML, graph neural networks, and formal verification.

-----

## §12. Kernel-Internal ML

These techniques run without AIRS. They are compiled into the AIRS binary (or kernel, where noted) as fixed-size statistical models. Total state overhead: under 10 KB for 64 agents.

### §12.1 TokenAnomalyDetector (Welford's Algorithm)

The primary kernel-internal detection mechanism. For each agent, the detector maintains running mean and variance using Welford's online algorithm (1962), enabling z-score anomaly detection with O(1) per observation and fixed state per agent.

```rust
pub struct TokenAnomalyDetector {
    /// Per-agent running statistics (Welford's algorithm)
    agents: [Option<AgentTokenStats>; 64],
}

pub struct AgentTokenStats {
    agent_id: AgentId,
    /// Running mean of actions per observation window
    mean: f64,
    /// Running M2 (sum of squared deviations from mean)
    m2: f64,
    /// Number of observation windows
    count: u64,
    /// EWMA of recent activity (faster response to sudden change)
    recent_ewma: f64,
    /// Anomaly flag: set when z-score exceeds threshold
    anomaly: bool,
}

impl AgentTokenStats {
    /// Update with a new observation window.
    pub fn observe(&mut self, actions_this_window: u64) {
        let x = actions_this_window as f64;
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;

        self.recent_ewma = 0.3 * x + 0.7 * self.recent_ewma;

        // Z-score anomaly detection (after warmup period)
        if self.count > 48 {
            let variance = self.m2 / (self.count - 1) as f64;
            let stddev = variance.sqrt();
            let z_score = (x - self.mean) / stddev.max(1.0);
            self.anomaly = z_score > 3.0;
        }
    }
}
```

**Integration with detection engine:** The `TokenAnomalyDetector` provides the statistical backbone for the z-score detection described in [detection.md §4.1](./detection.md). The `RunningStats` type in the data model ([data-model.md §3.3](./data-model.md)) generalizes this pattern across five action dimensions (read, write, network, inference, spawn), each with independent mean/variance tracking.

**State overhead:** ~2.3 KB total (64 agents × ~36 bytes each).

### §12.2 EWMA Fast Anomaly Detection

Z-score detection against hourly baselines has a response time measured in minutes — it requires enough observations to build statistical significance. For faster response to sudden behavioral changes (within seconds), the monitor uses an Exponentially Weighted Moving Average (EWMA):

```rust
pub struct EwmaDetector {
    /// Current EWMA value
    value: f64,
    /// Smoothing factor (default: 0.3 — recent observations weighted heavily)
    alpha: f64,
    /// EWMA of squared deviations (for variance estimation)
    variance_ewma: f64,
    /// Minimum observations before alerting
    min_observations: u32,
    /// Current observation count
    observations: u32,
}

impl EwmaDetector {
    /// Update with a new 1-second micro-window observation.
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.observations += 1;
        let prev = self.value;
        self.value = self.alpha * x + (1.0 - self.alpha) * self.value;
        let deviation = (x - prev).powi(2);
        self.variance_ewma = self.alpha * deviation + (1.0 - self.alpha) * self.variance_ewma;

        if self.observations >= self.min_observations {
            let stddev = self.variance_ewma.sqrt().max(1.0);
            let z = (x - self.value) / stddev;
            if z > 4.0 {
                return Some(z);
            }
        }
        None
    }
}
```

**Why α = 0.3:** This gives a half-life of approximately 2 observations — the EWMA forgets 50% of its history every 2 seconds. For behavioral detection, this means a sudden spike is detected within 2–3 seconds, compared to the z-score detector's reliance on minute-level aggregation. The trade-off is higher false positive rate — hence the higher threshold (4.0 vs 3.0) and the requirement that EWMA anomalies trigger investigation (Tier 2 escalation) rather than immediate enforcement.

**Integration with multi-timescale baselines:** The EWMA detector is the "fast" timescale in the three-timescale system described in [detection.md §5.3](./detection.md). It complements the hourly (medium) and weekly (slow) baselines by catching sudden spikes that the longer timescales would smooth out.

**State overhead:** ~40 bytes per agent per action type. For 64 agents × 5 action types: ~12.8 KB.

### §12.3 Lightweight Markov Sequence Model

Statistical detection (z-score, EWMA) catches rate anomalies — agents doing too much or too little. But rate-normal attacks with unusual *sequences* (e.g., read → send → read → send in alternation, suggesting data exfiltration) require sequence analysis. The kernel-internal solution is a lightweight Markov transition matrix:

```rust
pub struct MarkovSequenceModel {
    /// Transition counts: from ActionType → to ActionType
    /// 8×8 matrix (8 action types defined in detection.md §4.5)
    transitions: [[u32; 8]; 8],
    /// Total transitions from each state (for probability computation)
    row_totals: [u32; 8],
    /// Previous action (for transition recording)
    prev_action: Option<ActionType>,
    /// Anomaly threshold: log-probability below which a transition is flagged
    threshold: f64,
}

impl MarkovSequenceModel {
    /// Record a new action and check if the transition is anomalous.
    pub fn observe(&mut self, action: ActionType) -> Option<f64> {
        if let Some(prev) = self.prev_action {
            let from = prev as usize;
            let to = action as usize;

            // Check probability of this transition
            let total = self.row_totals[from] as f64;
            if total > 100.0 {
                let count = self.transitions[from][to] as f64;
                let prob = (count + 1.0) / (total + 8.0); // Laplace smoothing
                let log_prob = prob.ln();
                if log_prob < self.threshold {
                    self.prev_action = Some(action);
                    self.transitions[from][to] += 1;
                    self.row_totals[from] += 1;
                    return Some(-log_prob); // Return surprise score
                }
            }

            // Update transition counts
            self.transitions[from][to] += 1;
            self.row_totals[from] += 1;
        }
        self.prev_action = Some(action);
        None
    }
}
```

**Relationship to sequence analysis:** This complements the edit-distance sequence analysis in [detection.md §4.5](./detection.md). The Markov model catches anomalous individual transitions (pairs of actions), while the edit-distance approach catches anomalous longer sequences (windows of 8 actions). The Markov model is faster (O(1) per action) but less expressive; the edit-distance approach is more expressive but runs only on aggregated windows.

**Why Laplace smoothing:** Without smoothing, a never-seen transition has probability 0 and infinite surprise. Laplace smoothing (+1 to each count, +8 to total) ensures every transition has a nonzero probability while still flagging rare transitions as anomalous after sufficient observations.

**State overhead:** ~296 bytes per agent (64 counts × 4 bytes + 8 totals × 4 bytes + overhead). For 64 agents: ~18.5 KB.

### §12.4 Kernel-Internal ML Summary

| Component | What It Detects | State Per Agent | Update Cost | Alert Latency |
|---|---|---|---|---|
| **TokenAnomalyDetector** | Rate anomalies (z-score) | ~36 B | O(1) per window | Minutes (hourly baseline) |
| **EwmaDetector** | Sudden spikes | ~40 B × 5 = 200 B | O(1) per second | 2–3 seconds |
| **MarkovSequenceModel** | Unusual transitions | ~296 B | O(1) per action | Immediate |

**Total Tier 1 state for 64 agents:** ~2.3 KB + 12.8 KB + 18.5 KB ≈ **33.6 KB**

All three models run in kernel context (or AIRS without LLM inference). They have no dependencies on model loading, LLM availability, or AIRS health. If AIRS crashes, Tier 1 continues operating unchanged.

-----

## §13. AIRS-Dependent Intelligence

These techniques require the AIRS inference engine and a loaded language model. They provide higher-fidelity detection than Tier 1 but are optional — the system is safe without them.

### §13.1 Semantic Sequence Classification

Tier 1 detects *statistical* anomalies — a z-score or an unusual Markov transition. But statistical anomaly is not the same as semantic anomaly. An agent that reads 50 objects/min during a user-initiated "organize my email" command is statistically anomalous but semantically normal.

The semantic classifier takes flagged windows from Tier 1 and evaluates them with contextual understanding:

```text
Input to AIRS:
  Agent: email-organizer (declared purpose: email management)
  Context: User issued "organize by date" command 2 min ago
  Flagged behavior: 50 reads/min (baseline: 5 reads/min)
  Tier 1 verdict: FrequencySpike (z=4.2)

AIRS analysis:
  The elevated read rate is consistent with the active "organize"
  command. Email agents are expected to batch-read during
  organization. Classify as: FALSE_POSITIVE.

  Confidence: 0.92
  Recommendation: Suppress enforcement, log for baseline update
```

**Implementation:** The semantic classifier uses constrained generation (JSON mode) to produce structured verdicts:

```rust
pub enum SemanticVerdict {
    /// Tier 1 anomaly is a genuine threat — escalate
    TruePositive {
        confidence: f32,
        reasoning: String,
    },
    /// Tier 1 anomaly is explained by context — suppress
    FalsePositive {
        confidence: f32,
        explanation: String,
    },
    /// Tier 1 anomaly is suspicious but ambiguous — investigate
    Uncertain {
        confidence: f32,
        recommended_action: InvestigationAction,
    },
}

pub enum InvestigationAction {
    /// Monitor more closely but don't enforce
    IncreasedMonitoring,
    /// Ask the user for context
    UserQuery { question: String },
    /// Wait for more data before deciding
    WaitAndObserve { duration: Duration },
}
```

**When invoked:** The semantic classifier runs only on windows flagged by Tier 1 (z-score > `anomaly_threshold` or Markov surprise > `threshold`). It does not run on every observation window — that would be prohibitively expensive. Typical invocation rate: 1–5 times per hour across all agents.

**Latency:** ~50–200ms per classification (depends on model size and context length). This is acceptable because semantic classification is asynchronous — it does not block IPC or agent execution. The agent continues operating while the classifier runs, subject to Tier 1 enforcement (rate limiting if z-score exceeds the threshold).

### §13.2 RAG Behavioral Lookup

When the semantic classifier encounters unfamiliar agent behavior, it can compare against a corpus of known-good behavioral profiles using retrieval-augmented generation:

```text
Query: "email agent reading calendar data at 30 objects/min"

Retrieved similar profiles:
  1. email-plus-calendar agent (corpus score: 0.87):
     Reads calendar for meeting scheduling. Normal rate: 20-40/min.
     Verdict: LIKELY BENIGN

  2. email-data-harvest agent (corpus score: 0.23, FLAGGED):
     Known malicious pattern: email agent accessing non-email data.
     Verdict: SUSPICIOUS — requires user confirmation
```

**Corpus sources:**

1. **Local corpus** — Behavioral profiles from all agents that have run on this device. Built from warmup observations and post-warmup baselines. Contains normal profiles only (agents that were never flagged for true positive anomalies).

2. **Agent Capability Intelligence predictions** — PredictedBehavior records from the 5-stage install-time analysis pipeline (see [profiling.md §8.1](./profiling.md) and [intelligence-services.md §5.9](../airs/intelligence-services.md)). These provide a reference for what the agent *should* do.

3. **Federated corpus** (future, see §16.1) — Anonymized behavioral statistics from other AIOS devices, shared with consent. Provides a broader basis for comparison, especially for new agents with no local history.

**Embedding:** Behavioral profiles are embedded using a compact feature vector:
- 5 rate dimensions (mean, stddev for each action type)
- 24 hourly activity levels (normalized)
- Top 32 sequence frequencies
- Target diversity score

Total: ~150-dimensional vector per agent. Embedded with a lightweight encoder (distilled model, ~10 MB) into a 64-dimensional space for nearest-neighbor lookup.

**State overhead:** ~4 KB per agent in corpus. For 64 agents: ~256 KB. The HNSW index adds ~2× overhead for graph structure.

### §13.3 LoRA Fine-Tuning for Deployment Adaptation

Enterprise deployments have distinct behavioral norms — a financial trading platform's agents behave very differently from a personal productivity device's agents. The base model's understanding of "normal agent behavior" may not match the deployment's reality.

**LoRA (Low-Rank Adaptation)** adapts the semantic classifier to the deployment without retraining the full model:

```rust
pub struct BehavioralLoRA {
    /// LoRA adapter weights (rank 8, ~2-4 MB for 7B model)
    adapter: LoraAdapter,
    /// Training data: labeled behavioral windows
    training_data: Vec<LabeledWindow>,
    /// Last training timestamp
    last_trained: Timestamp,
    /// Model performance metrics (precision, recall on held-out set)
    metrics: ClassifierMetrics,
}

pub struct LabeledWindow {
    /// The observed behavioral window
    window: ObservationWindow,
    /// Ground truth label (from user feedback or Tier 1 consensus)
    label: WindowLabel,
    /// Confidence in the label (higher for user-confirmed, lower for auto-labeled)
    confidence: f32,
}

pub enum WindowLabel {
    Normal,
    Anomalous { anomaly_type: AnomalyType },
    FalsePositive,
}
```

**Training trigger:** LoRA retraining occurs when:
1. The deployment has accumulated > 100 labeled windows (from user feedback via Inspector)
2. The classifier's false positive rate exceeds 5% (measured from user dismissals)
3. The classifier's false negative rate exceeds 1% (measured from escalations that bypassed Tier 2)

**Training schedule:** Background training during device idle time. Uses the last 7 days of labeled windows. Training time: ~5–15 minutes for rank-8 LoRA on a 7B model with 100–500 labeled examples.

**Safety guardrail:** The LoRA adapter cannot reduce hard limits or change escalation policy. It only affects the semantic classifier's sensitivity — whether a flagged window is classified as `TruePositive`, `FalsePositive`, or `Uncertain`. Enforcement boundaries remain kernel-controlled.

### §13.4 Cross-Agent Behavioral Correlation

Individual agent monitoring has a fundamental blind spot: **collusion attacks** where multiple agents coordinate to stay individually below detection thresholds while collectively achieving a malicious goal (see [evasion.md §11.6](./evasion.md)).

Cross-agent correlation detects these patterns by analyzing temporal relationships between agents' activities:

```rust
pub struct CrossAgentCorrelator {
    /// Per-agent activity time series (last 1 hour, 1-second resolution)
    activity_series: HashMap<AgentId, TimeSeries>,
    /// Known correlated pairs (updated every 5 minutes)
    correlations: Vec<AgentCorrelation>,
    /// Correlation threshold for flagging (default: 0.7)
    threshold: f64,
}

pub struct AgentCorrelation {
    /// The two agents with correlated behavior
    agent_a: AgentId,
    agent_b: AgentId,
    /// Pearson correlation coefficient (-1.0 to 1.0)
    correlation: f64,
    /// Time lag at peak correlation (seconds)
    lag: i32,
    /// Whether these agents have an IPC channel (expected correlation)
    has_ipc_channel: bool,
    /// Whether this correlation is new (not seen in baseline period)
    is_novel: bool,
}
```

**Detection logic:**

1. **Compute pairwise correlations** — For each pair of agents, compute the cross-correlation function over their activity time series. The peak correlation and its lag indicate how strongly and how quickly one agent's activity follows another's.

2. **Filter expected correlations** — Agents with established IPC channels are expected to have correlated activity. Only flag correlations that are:
   - Novel (not present in the first 7 days of observation), OR
   - Extremely strong (correlation > 0.9 with lag < 5 seconds), OR
   - Between agents that have never communicated before

3. **Semantic analysis** — For flagged correlations, AIRS evaluates whether the correlated behavior is consistent with the agents' declared purposes: "The email agent's read bursts correlate with the network agent's send bursts — this could be legitimate email forwarding or data exfiltration."

**Computational cost:** Pairwise correlation for N agents is O(N²). For 64 agents, this is 2,016 pairs — computed every 5 minutes using 1-minute resolution time series (60 data points per series). Total cost: ~10ms. This runs in AIRS, not kernel context.

**Limitations:** Cross-agent correlation is effective for detecting temporal coordination but cannot detect semantic coordination (agents passing messages through shared spaces with no temporal correlation). Semantic coordination detection requires content inspection (Layer 4, Layer 6) and is beyond the behavioral monitor's scope.

### §13.5 Semantic Cache for Behavioral Verdicts

Semantic classification is expensive (~100ms per invocation). Many behavioral patterns recur — the same agent exhibits the same anomaly at the same time of day. The semantic cache stores recent classification results and reuses them for similar patterns:

```rust
pub struct SemanticCache {
    /// Cache entries keyed by behavioral pattern hash
    entries: HashMap<u64, CacheEntry>,
    /// Maximum cache size (entries)
    max_size: usize,                    // default: 1024
    /// Cache hit/miss statistics
    stats: CacheStats,
}

pub struct CacheEntry {
    /// Hash of the behavioral pattern (agent category + anomaly type + context)
    pattern_hash: u64,
    /// Cached verdict
    verdict: SemanticVerdict,
    /// When this entry was created
    timestamp: Timestamp,
    /// How many times this entry has been reused
    hit_count: u32,
    /// Time-to-live (entries expire after 24 hours)
    ttl: Duration,
}
```

**Cache key:** The pattern hash combines:
- Agent category (from manifest or Agent Capability Intelligence stage 4)
- Anomaly type (from Tier 1 classification)
- Hourly bucket (which hour of the day)
- Severity bucket (low/medium/high)

This allows the cache to match "email agent with a FrequencySpike of medium severity at 3 PM" without requiring an exact match on all behavioral dimensions.

**Cache invalidation:** Entries expire after 24 hours. Entries are also invalidated when:
- The agent is updated (new version → new behavior expectations)
- The user provides contradictory feedback (dismissed a cached `TruePositive`)
- The LoRA adapter is retrained (model behavior may have changed)

**Expected hit rate:** 40–60% for mature deployments (agents with stable behavior patterns). New agent installations or after agent updates, the hit rate drops to near zero until patterns stabilize.

-----

## §16. Future Directions

### §16.1 Federated Behavioral Intelligence

Currently, each AIOS device builds behavioral baselines in isolation. Federated learning enables cross-device behavioral intelligence without sharing raw behavioral data:

**Architecture:**
- Each device computes gradient updates from its local behavioral data
- Updates are anonymized (differential privacy with ε=1.0) before sharing
- A federated aggregation server (optional, can be any device in the mesh) combines updates
- The aggregated model is distributed back to all devices

**Benefits:**
- New devices get meaningful baselines from day one (instead of 7-day warmup with no priors)
- Rare attack patterns detected on one device improve detection on all devices
- Agent category norms (e.g., "email agents typically read 10–50 objects/minute") are learned from fleet-wide data

**Privacy guarantees:**
- No raw behavioral data leaves the device
- Differential privacy ensures individual agent behavior cannot be reconstructed from shared gradients
- Participation is opt-in and anonymous
- Enterprise deployments can restrict federation to within the organization

### §16.2 Graph Neural Networks for Agent Interaction Analysis

Agent collusion detection (§13.4) currently uses pairwise temporal correlation. Graph Neural Networks (GNNs) enable richer analysis of multi-agent interaction patterns:

**Approach:**
- Model agents as nodes and IPC channels as edges in a dynamic graph
- Node features: behavioral metrics (rate, volume, target diversity, sequence entropy)
- Edge features: communication frequency, message sizes, temporal correlation
- GNN architecture: Graph Attention Network (GAT) with temporal attention
- Training: Self-supervised on normal interaction patterns, flagging structural anomalies

**What GNNs catch that pairwise correlation misses:**
- **Three-way coordination** — Agent A reads data, passes to Agent B for processing, Agent B passes to Agent C for exfiltration. No individual pair (A-B, B-C) is anomalous; the chain is.
- **Community structure changes** — A previously isolated group of agents suddenly forming connections with another group may indicate lateral movement.
- **Role reversal** — An agent that was always a data consumer (receiving IPC) suddenly becoming a data producer (sending IPC) changes the graph topology.

**Computational budget:** GNN inference on a 64-node graph: ~5ms per evaluation. Run every 5 minutes in AIRS context. Total overhead: negligible.

### §16.3 Reinforcement Learning for Adaptive Thresholds

Static z-score thresholds (default 3.0) are a compromise — too low causes false positives, too high misses subtle attacks. Reinforcement learning (RL) can adapt thresholds per-agent based on feedback:

**Formulation:**
- **State:** Agent's behavioral profile (mean, variance, hourly pattern, target set size)
- **Action:** Anomaly threshold for each detection dimension (continuous, range 2.0–5.0)
- **Reward:** Negative for false positives (user dismissals), positive for true positives (confirmed threats), penalty for false negatives (attacks that bypassed detection)

**Approach:** Contextual bandits (rather than full RL) — each threshold decision is independent, avoiding the need for long-horizon planning. The bandit uses Thompson sampling with a Gaussian prior, updated from user feedback via Inspector.

**Safety constraint:** The RL agent cannot set thresholds above 5.0 (would miss obvious attacks) or below 2.0 (would cause excessive false positives). Hard limits are unaffected by RL — they remain kernel-controlled and fixed.

### §16.4 Causal Inference for Root Cause Analysis

When an anomaly is detected, the current system reports *what* is anomalous but not *why*. Causal inference techniques can trace anomalies to root causes:

**Approach:** Granger causality analysis on behavioral time series. If Agent A's read rate spike Granger-causes Agent B's network spike (A's spike consistently precedes B's with a stable lag), the system can report:

```text
Root cause analysis:
  Agent B's network anomaly was likely caused by Agent A's
  read spike (Granger causality p < 0.01, lag = 2 seconds).
  Recommendation: Investigate Agent A's behavior first.
```

**Integration with Inspector:** Root cause chains are visualized in the Inspector timeline view, connecting related anomalies across agents with directed arrows showing causal relationships.

### §16.5 Formal Verification of Detection Invariants

The behavioral monitor's safety properties should be formally verifiable:

1. **Hard limit invariance:** "No agent can exceed its hard limit for more than one observation window" — provable from the enforcement code structure
2. **Escalation termination:** "The escalation state machine always terminates" — provable from the finite state machine with bounded timeouts
3. **Baseline bounded drift:** "The baseline mean cannot change by more than `drift_rate` per week" — provable from the decayed Welford update formula
4. **Provenance integrity:** "Every enforcement action has a corresponding provenance record" — provable from the enforcement→provenance call chain

**Tooling:** TLA+ specifications for the escalation state machine. Kani model checking for Rust enforcement code. CBMC bounded model checking for kernel-internal path correctness.

### §16.6 Behavioral Contracts

Extend Agent Capability Intelligence (§5.9) with formal behavioral contracts — agents declare not just what they *can* do (capabilities) but what they *will* do (behavioral bounds):

```rust
pub struct BehavioralContract {
    /// Maximum expected rates by action type
    declared_rates: HashMap<ActionType, RateRange>,
    /// Declared active hours (e.g., "business hours only")
    declared_schedule: Option<TimeRange>,
    /// Declared target spaces
    declared_targets: Vec<SpacePattern>,
    /// Hash of the contract (signed by the agent developer)
    contract_hash: ContentHash,
    /// Developer's signature
    signature: Signature,
}
```

**Enforcement:** The behavioral monitor treats contract bounds as a tighter version of hard limits. An agent violating its own contract faces immediate escalation (Level 2 — Pause) rather than the gradual escalation applied to baseline-derived anomalies. Contract violations indicate either a bug in the agent or a compromise.

**Benefit:** Contracts shift the detection paradigm from "anomalous compared to history" to "violating declared intent." This eliminates the cold start problem entirely — the contract serves as an immediate, developer-provided baseline.

### §16.7 Neuromorphic Behavioral Processing

For resource-constrained devices (wearables, IoT), neuromorphic computing offers an alternative to traditional statistical processing:

**Spiking neural network (SNN) detector:** Replace the EWMA and z-score detectors with a small SNN (64–256 neurons) that processes behavioral events as spike trains. Neuromorphic hardware (Intel Loihi 2, SynSense Xylo) computes these with sub-milliwatt power consumption.

**Advantages:**
- Power: ~100× more efficient than CPU-based statistical detection
- Latency: Sub-microsecond spike-to-spike processing
- Adaptation: Spike-timing-dependent plasticity (STDP) provides online learning without explicit Welford updates

**Limitations:** Requires neuromorphic hardware. The SNN approach is complementary to, not a replacement for, the statistical Tier 1 — it adds a power-efficient detection layer for always-on monitoring on battery-constrained devices.

-----

## §12–§13 Cross-Reference Index

| Section | Sub-file | Topic |
|---|---|---|
| §12.1 | This file | TokenAnomalyDetector (Welford) |
| §12.2 | This file | EWMA fast anomaly detection |
| §12.3 | This file | Markov sequence model |
| §12.4 | This file | Kernel-internal ML summary |
| §13.1 | This file | Semantic sequence classification |
| §13.2 | This file | RAG behavioral lookup |
| §13.3 | This file | LoRA deployment adaptation |
| §13.4 | This file | Cross-agent correlation |
| §13.5 | This file | Semantic cache |
| §16.1 | This file | Federated behavioral intelligence |
| §16.2 | This file | GNN agent interaction analysis |
| §16.3 | This file | RL adaptive thresholds |
| §16.4 | This file | Causal inference root cause |
| §16.5 | This file | Formal verification |
| §16.6 | This file | Behavioral contracts |
| §16.7 | This file | Neuromorphic processing |
