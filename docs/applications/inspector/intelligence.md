# AIOS Inspector — AI-Native Intelligence

Part of: [inspector.md](../inspector.md) — Inspector Architecture
**Related:** [views.md](./views.md) — Views, [threat-model.md](./threat-model.md) — Threat Model, [testing.md](./testing.md) — Testing

-----

The Inspector's intelligence operates at two tiers. **Tier 1** (section 13) uses kernel-internal statistical models — frozen decision trees, running statistics, and lightweight anomaly detectors that run without AIRS. **Tier 2** (section 12) uses AIRS-dependent semantic analysis — LLM inference, natural language processing, and cross-agent correlation. Section 14 describes future research directions in federated learning, adversarial robustness, and causal reinforcement learning.

The design principle is **graceful degradation**: every AIRS-dependent feature has a non-AI fallback that preserves core functionality. If AIRS is offline, the Inspector still scores alerts, detects anomalies, and presents provenance data — it loses semantic explanations and natural language queries, but never loses visibility.

-----

## 12. AIRS-Dependent Intelligence

These features require the AIRS inference engine ([airs.md](../../intelligence/airs.md)) for semantic understanding, natural language processing, or cross-agent correlation. Each feature degrades gracefully when AIRS is unavailable — the fallback behavior is specified alongside each capability.

### 12.1 Natural Language Security Queries

Users query the Inspector's security state via the Conversation Bar in natural language. AIRS translates natural language into structured `AuditQuery` objects, enabling non-technical users to interrogate the full provenance chain without learning a query language.

**Query translation pipeline:**

```text
User input (NL)
  |
  v
AIRS NL2Query translator
  |  - Entity extraction: agent names, capability types, time ranges
  |  - Intent classification: search, compare, alert-review, investigate
  |  - Scope inference: single-agent, cross-agent, system-wide
  v
Structured AuditQuery
  |
  v
Provenance Query Engine
  |
  v
Formatted results (tables, timelines, graphs)
```

The NL2Query translator is inspired by NL2KQL, which achieves a 0.964 syntax correctness score on Kusto Query Language translation. AIOS targets comparable accuracy on the simpler `AuditQuery` schema.

**Example queries and their translations:**

| Natural language | Structured query |
|---|---|
| "What has Budget Tracker accessed today?" | `AuditQuery { agent: "Budget Tracker", action: Read\|Write, since: today_start }` |
| "Which agents can see my photos?" | `CapabilityQuery { scope: space("user/photos"), permission: Read }` |
| "Show me anything suspicious in the last hour" | `AuditQuery { anomaly_score: >= 4, since: now - 1h }` |
| "Has any agent been denied network access?" | `AuditQuery { action: NetworkConnect, result: Denied, since: boot }` |
| "Compare Research Assistant to its baseline" | `AgentQuery { agent: "Research Assistant", include: baseline_comparison }` |

**Confidence handling:** Each translated query carries a confidence score. Queries scoring below 0.8 are shown to the user for confirmation before execution: "I think you're asking for X — is that right?"

**Fallback without AIRS:** Keyword search on provenance records. The Inspector parses the input for recognized terms (agent names, capability types, time words like "today" or "hour") and constructs a best-effort filter. Results include a banner: "Natural language search unavailable — showing keyword matches."

### 12.2 Semantic Anomaly Explanation

When the behavioral monitor flags an anomaly ([behavioral-monitor.md](../../intelligence/behavioral-monitor.md)), the raw alert contains statistical data: z-score, action counts, deviation magnitude. AIRS generates a human-readable explanation that contextualizes the anomaly against the agent's declared purpose and behavioral history.

**Explanation generation:**

```rust
pub struct AnomalyExplanation {
    /// The anomaly being explained
    anomaly_id: AuditRecordId,
    /// Human-readable summary (1-3 sentences)
    summary: String,
    /// Causal factors ranked by contribution
    factors: Vec<CausalFactor>,
    /// Comparison to agent's manifest-declared behavior
    manifest_deviation: Option<ManifestDeviation>,
    /// Suggested user actions
    suggested_actions: Vec<SuggestedAction>,
}

pub struct CausalFactor {
    /// What contributed to the anomaly
    description: String,
    /// Relative importance (0.0 - 1.0)
    weight: f32,
    /// Link to the specific provenance record
    evidence: AuditRecordId,
}
```

**Example output:** "Research Assistant accessed 47 files in user/documents — 4x its daily average. This is unusual because it normally only reads research/papers/*. The burst started at 14:32 and correlates with a new tool invocation (web-scraper) that was installed 10 minutes earlier."

The explanation approach is inspired by LogRESP-Agent, which achieves 99.97% detection accuracy on log-based anomaly classification by combining statistical signals with semantic understanding of log content.

**Causal graph:** Each explanation includes a DAG showing the chain of events leading to the anomaly. Nodes represent provenance records; edges represent temporal or causal relationships (e.g., "agent acquired capability X" -> "agent accessed resource Y"). The graph is rendered in the Provenance View during investigation workflows.

**Fallback without AIRS:** The Inspector displays the raw anomaly data — z-score, action counts, deviation percentage — with a template-based summary: "Agent X performed Y actions (Z% above average)." No causal graph or manifest comparison.

### 12.3 Proactive Behavior Prediction

AIRS predicts likely next actions for each active agent based on static code analysis of the agent bundle, runtime behavioral history, and manifest-declared capabilities. Predictions surface in the Agent View as forward-looking indicators.

**Prediction model:**

```text
Inputs:
  - Agent manifest (declared capabilities, tool registrations)
  - Historical action sequences (last 24h, windowed)
  - Current active tools and open resources
  - Similar-agent corpus behavior (anonymized)

Output:
  - Ranked list of predicted next capabilities/actions
  - Time horizon estimate (within next session, next hour, next day)
  - Confidence score per prediction
```

This approach is inspired by Pro2Guard, which achieves 93.6% early enforcement accuracy by predicting permission requests before they occur, enabling preemptive capability review.

**Example prediction:** "This agent will likely request Network access to api.openai.com within the next session, based on its tool manifest declaring an OpenAI integration and its historical pattern of making API calls after document analysis."

**User-facing integration:** Predictions appear in the Agent View as a collapsible "Predicted Activity" panel. Users can pre-approve or pre-deny predicted capability requests, reducing runtime interruptions. Pre-decisions are stored as temporal capability rules ([capabilities.md §3.6](../../security/model/capabilities.md)).

**Fallback without AIRS:** No predictions displayed. The "Predicted Activity" panel shows: "Behavior prediction requires AIRS. Enable AIRS for proactive security insights."

### 12.4 Security Investigation Copilot

AIRS acts as an interactive investigation partner, guiding users through security incidents. Inspired by Microsoft Security Copilot, which enables analysts to identify 6.5x more security alerts in the same time window.

**Investigation workflow:**

```text
1. User describes concern
   "Something feels off with the Weather agent"
       |
       v
2. AIRS queries relevant data across all layers
   - Provenance: recent actions, denied requests
   - Capabilities: active tokens, recent grants/revocations
   - Behavioral baseline: deviation from norm
   - Cross-agent: interactions with other agents
       |
       v
3. AIRS presents findings with evidence
   "Weather Agent has been accessing user/contacts (3 reads in last hour).
    This is outside its declared scope (weather data only).
    It also received a new IPC message from Unknown Agent #7."
       |
       v
4. AIRS suggests response actions
   "Recommended: Pause Weather Agent, revoke ContactsRead capability,
    investigate Unknown Agent #7's provenance chain."
       |
       v
5. User confirms or modifies
   Actions execute through the Action Handler with full audit logging.
```

The investigation copilot is integrated into the Conversation Bar with a dedicated "Investigate" mode. When activated, the Conversation Bar context switches to security investigation, with auto-complete suggestions drawn from agent names, capability types, and recent security events.

**Multi-turn context:** The copilot maintains investigation context across multiple exchanges, building a running hypothesis and evidence set. Users can fork an investigation ("also check agent X") without losing the original thread.

**Fallback without AIRS:** Manual investigation using the existing nine views. The Conversation Bar reverts to keyword search (section 12.1 fallback). The Inspector displays a suggestion: "For guided investigation, enable AIRS."

### 12.5 Capability Recommendation Engine

AIRS suggests least-privilege capability sets for each agent, identifying capabilities that can be safely removed and capabilities that should be added based on observed behavior.

**Recommendation sources:**

| Source | Signal | Weight |
|---|---|---|
| Static code analysis | Agent bundle declares tool X but never invokes it | 0.3 |
| Runtime behavioral data | Capability Y granted but unused for 30+ days | 0.4 |
| Corpus comparison | 95% of similar agents (same category) do not hold capability Z | 0.2 |
| Manifest declaration | Agent declares needing capability W but has not been granted it | 0.1 |

**Recommendation output:**

```rust
pub struct CapabilityRecommendation {
    /// The agent this recommendation applies to
    agent_id: AgentId,
    /// Recommended action
    action: RecommendedAction,
    /// The capability in question
    capability: Capability,
    /// Confidence score (0.0 - 1.0)
    confidence: f32,
    /// Human-readable rationale
    rationale: String,
    /// Evidence supporting the recommendation
    evidence: Vec<EvidenceItem>,
}

pub enum RecommendedAction {
    /// Capability can be safely revoked (unused or unnecessary)
    Revoke,
    /// Capability scope can be narrowed (e.g., All -> specific space)
    Narrow { suggested_scope: Scope },
    /// Capability should be granted (agent needs it but lacks it)
    Grant,
    /// Capability should be time-limited (only needed during specific contexts)
    TemporalLimit { suggested_window: Duration },
}
```

Recommendations are displayed in the AIRS Analysis View with "Apply" buttons. Each recommendation shows its confidence score and a one-sentence rationale. Applying a recommendation routes through the Action Handler with the standard confirmation dialog.

**Fallback without AIRS:** The Inspector displays a simpler "Unused Capabilities" list based on the kernel-internal capability usage heatmap (section 13.6). No confidence scores, no scope narrowing suggestions, no corpus comparison.

### 12.6 Semantic Event Clustering

AIRS groups related security events by semantic meaning rather than timestamp proximity. Raw security event feeds often generate many individual alerts that represent facets of a single underlying situation. Semantic clustering collapses these into coherent incident groups.

**Clustering approach:**

```text
Input: stream of security events (denials, anomalies, warnings)
    |
    v
Feature extraction:
  - Agent identity (same agent? related agents?)
  - Resource target (same space? same object type?)
  - Temporal proximity (within same 5-minute window?)
  - Capability type (same permission class?)
  - Causal linkage (event A's output is event B's input?)
    |
    v
Semantic similarity scoring (AIRS embeddings)
    |
    v
Agglomerative clustering (merge when similarity > 0.7)
    |
    v
Output: incident groups with summary labels
```

Inspired by DEMIST-2, which achieves approximately 94% accuracy on security event clustering by combining structural features with semantic understanding.

**Example:** Three separate "access denied" events — agent A denied ContactsRead, agent B denied ContactsRead, agent C denied ContactsWrite — all targeting the same space within a 2-minute window. Instead of three independent alerts, the Inspector presents a single cluster: "Coordinated access attempt on user/contacts by 3 agents (all denied)."

**Alert reduction:** Semantic clustering reduces the visible alert count by collapsing related events. Each cluster shows a count badge ("3 events") and expands to reveal individual events on click.

**Fallback without AIRS:** Time-window grouping. Events within a 60-second window targeting the same resource are grouped. No semantic similarity, no cross-agent correlation. Cluster labels use templates: "N events on resource X in time window Y."

### 12.7 Explainable Root Cause Analysis

For complex multi-agent incidents, AIRS constructs a causal graph showing how events relate and which factors triggered the anomaly. This goes beyond the single-anomaly explanations of section 12.2 to cover incidents spanning multiple agents, capabilities, and time windows.

**Causal graph construction:**

```rust
pub struct CausalGraph {
    /// Nodes: individual security events or actions
    nodes: Vec<CausalNode>,
    /// Edges: causal or temporal relationships
    edges: Vec<CausalEdge>,
    /// Root cause: the node(s) identified as the origin
    root_causes: Vec<NodeId>,
    /// Overall incident summary
    summary: String,
}

pub struct CausalNode {
    id: NodeId,
    /// The underlying provenance record
    record: AuditRecordId,
    /// Feature attribution: which factors made this node relevant
    attribution: Vec<FeatureAttribution>,
}

pub struct FeatureAttribution {
    /// The feature that contributed to the anomaly score
    feature_name: String,
    /// SHAP value indicating contribution direction and magnitude
    shap_value: f32,
}
```

The analysis uses SHAP (SHapley Additive exPlanations) and LIME (Local Interpretable Model-agnostic Explanations) style feature attribution to explain which factors triggered the anomaly detection models. For each node in the causal graph, users can inspect which input features (action count, capability scope, time-of-day, agent trust level) contributed most to its anomaly score.

**Presentation:** The causal graph renders as an interactive DAG in the Provenance View. Nodes are color-coded by severity (green=normal, yellow=suspicious, red=anomalous). Clicking a node shows its feature attribution breakdown. Root cause nodes are highlighted with a distinct border.

**Investigation integration:** The root cause analysis is triggered automatically when the Security Investigation Copilot (section 12.4) identifies a multi-agent incident. It can also be triggered manually from any security event by selecting "Analyze root cause" in the context menu.

**Fallback without AIRS:** The Inspector shows the provenance chain as a flat timeline without causal relationships or feature attribution. Events are linked only by temporal ordering. No root cause identification.

-----

## 13. Kernel-Internal ML

These techniques run as frozen decision trees and lightweight statistical models within the kernel. They require no AIRS dependency and operate even when AIRS is offline or unavailable. Total state overhead targets under 128 KiB for 32 agents across all models.

### 13.1 Alert Priority Scoring

A frozen decision tree scores each security event on a priority scale of 1-10, determining which events surface prominently in the Inspector and which are logged but not displayed.

**Decision tree features:**

| Feature | Type | Source |
|---|---|---|
| Event severity | Categorical (info/warn/error/critical) | Audit record |
| Agent trust level | Integer (0-3) | Agent registry |
| Capability scope | Categorical (single/space/all) | Capability token |
| Time-of-day context | Categorical (active/idle/night) | Context engine |
| Historical frequency | Float (events/hour, 24h average) | Per-agent counter |
| Denial count (recent) | Integer (last 5 minutes) | Rolling counter |

**Scoring rules (simplified):**

```text
IF severity == critical AND trust_level <= 1:
    score = 10   (always surface)
ELIF denial_count_recent > 5 AND time_context == idle:
    score = 8    (burst of denials during user absence)
ELIF severity == error AND historical_freq < 0.1:
    score = 7    (rare error from this agent)
ELIF severity == warn AND capability_scope == all:
    score = 6    (broad-scope warning)
ELIF severity == info AND trust_level == 3:
    score = 2    (routine info from trusted native agent)
ELSE:
    score = 4    (default — display in detailed view)
```

Inspired by production SOAR (Security Orchestration, Automation, and Response) systems that reduce alert fatigue by 62% through priority-based filtering.

The Inspector displays only events scoring above a configurable threshold (default: 4) in the main dashboard. All events remain accessible in the Security Events View regardless of score. Users adjust the threshold via Settings.

**State overhead:** ~2 KiB (feature cache per agent + rolling counters).

### 13.2 Behavioral Baseline Comparison

Per-agent running mean and variance using Welford's online algorithm, as designed in [behavioral-monitor.md](../../intelligence/behavioral-monitor.md). The Inspector consumes these baselines to render comparison visualizations.

**Tracked dimensions:**

```rust
pub struct AgentBaseline {
    /// Five action dimensions, each with independent Welford state
    dimensions: [WelfordState; 5],
}

pub struct WelfordState {
    mean: f64,
    m2: f64,
    count: u64,
}
```

The five dimensions are: read operations, write operations, network requests, inference calls, and agent spawns. Each dimension independently tracks a running mean and variance.

**Inspector integration:** The Agent View renders baseline comparisons as horizontal bar charts. Each bar shows the agent's current-session activity (solid fill) against its historical mean (outline) with standard deviation whiskers. Bars exceeding 2 standard deviations from the mean are highlighted yellow; bars exceeding 3 standard deviations are highlighted red.

The z-score threshold for anomaly flagging is configurable per dimension. Default: |z| > 3.0 for all dimensions.

**State overhead:** ~2.4 KiB total (32 agents x 5 dimensions x 3 floats x 8 bytes).

### 13.3 Provenance Graph Feature Scoring

Structural features extracted from the provenance chain feed a frozen decision tree that identifies suspicious subgraphs. This operates on the graph topology, not the semantic content of individual events.

**Structural features:**

| Feature | Definition | Suspicious when |
|---|---|---|
| Fan-out | Number of distinct resources accessed by one agent in a window | > 3x historical mean |
| Fan-in | Number of distinct agents accessing one resource in a window | > 5 agents on a non-shared resource |
| Temporal density | Actions per minute within a burst | > 10x agent's average rate |
| Depth | Length of delegation chain (agent A -> agent B -> agent C) | > 3 hops |
| Breadth | Number of distinct capability types used in a window | > agent's historical maximum |

The decision tree maps feature vectors to a suspiciousness score (0.0-1.0). Subgraphs scoring above 0.6 are highlighted in the Provenance View with a colored border and a tooltip showing which features contributed most.

**State overhead:** ~4 KiB (feature accumulators per agent, sliding window state).

### 13.4 Isolation Forest for Capability Abuse

A lightweight isolation forest detects agents whose capability usage patterns deviate from the population. Unlike per-agent baselines (section 13.2), which compare an agent to its own history, the isolation forest compares each agent to all other agents, identifying population-level outliers.

**Feature vector per agent:**

```rust
pub struct CapabilityUsageFeatures {
    /// Distribution across capability types (normalized histogram)
    type_distribution: [f32; 16],
    /// Scope breadth: fraction of capabilities with Scope::All
    scope_breadth: f32,
    /// Delegation depth: maximum depth of capability delegation chain
    max_delegation_depth: u8,
    /// Temporal variance: coefficient of variation of usage times
    temporal_cv: f32,
}
```

The isolation forest uses a fixed set of 32 binary trees, each with a maximum depth of 8. Trees are pre-trained on a representative agent population and frozen. The anomaly score for each agent is the average path length across all trees — shorter paths indicate more anomalous behavior.

Agents scoring in the top 5% (configurable) are flagged as outliers. The Capability View displays an outlier indicator next to flagged agents, with a tooltip explaining which features drove the anomaly score.

**State overhead:** ~16 KiB (32 trees x 255 nodes x ~2 bytes per split).

### 13.5 Temporal Pattern Detection

Three temporal pattern detectors run continuously on the security event stream, identifying patterns that statistical baselines alone miss.

**Pattern 1: Burst detection**

Detects sudden spikes in action frequency within a sliding window. A burst is defined as N actions within T seconds, where N exceeds 5x the agent's historical rate for that window duration. Burst detection targets data exfiltration scenarios where an agent rapidly reads many objects before attempting a network send.

```text
Window: 30 seconds (sliding)
Threshold: 5x historical rate
Action: flag event stream, increment burst_count in alert scoring (section 13.1)
```

**Pattern 2: Periodicity detection**

Detects regular recurring access patterns using autocorrelation on the event timestamp series. Periodic access with intervals shorter than 1 second may indicate a covert timing channel. Periodicity detection uses a fixed-point autocorrelation over a 60-second window with 100ms resolution.

```text
Window: 60 seconds
Resolution: 100ms bins
Threshold: autocorrelation coefficient > 0.7 at any lag
Action: flag as potential covert channel, score 7+ in alert scoring
```

**Pattern 3: Cross-agent correlation**

Detects multiple agents acting in temporal concert — a potential indicator of collusion or coordinated attack. The detector maintains a per-resource activity vector and flags when more than 2 agents access the same resource within a 5-second window, provided those agents have no declared relationship (e.g., not in the same agent group).

```text
Window: 5 seconds per resource
Threshold: > 2 unrelated agents
Action: flag as potential collusion, score 8+ in alert scoring
```

**State overhead:** ~8 KiB (sliding window buffers + autocorrelation state per monitored agent).

### 13.6 Capability Usage Heatmap

An Exponential Moving Average (EMA) matrix tracks per-agent, per-capability-type usage frequency, providing a visual summary of how the system's capability surface is being used.

**Matrix dimensions:**

```text
Rows: 32 agents (MAX_PROCESSES)
Columns: 32 capability types
Cell value: EMA of hourly usage count (alpha = 0.1)
Window: 24 hours (implicit via EMA decay)
```

The matrix is updated on every capability check (allow or deny). Each cell stores a single `f32` EMA value, updated as: `ema = alpha * current_count + (1 - alpha) * ema`.

**Inspector integration:** The Capability View renders this matrix as a color-coded heatmap. Color intensity maps to usage frequency: white (never used) through blue (moderate) to red (heavily used). Two callouts are highlighted:

- **Cold capabilities** (EMA < 0.01 for 72+ hours): candidates for revocation. Displayed with a snowflake indicator and a tooltip: "This capability has not been used in 3+ days. Consider revoking."
- **Hot capabilities** (EMA > 95th percentile of population): candidates for increased monitoring. Displayed with a flame indicator.

This heatmap provides the non-AIRS fallback for the capability recommendation engine (section 12.5): instead of semantic recommendations with confidence scores, users see raw usage patterns and make their own revocation decisions.

**State overhead:** ~4 KiB (32 x 32 x 4 bytes).

-----

## 14. Future Directions

Research directions informed by advances in federated learning, adversarial ML, differential privacy, and reinforcement learning for security automation. These are not planned for specific phases.

### 14.1 Federated Behavioral Learning

Learn behavioral baselines across multiple AIOS devices without sharing raw provenance data. Inspired by FLADEN (Federated Learning for Anomaly Detection in Edge Networks), which achieves 99.85% accuracy while preserving data privacy.

**Architecture:**

```text
Device A                     Device B                     Device C
  |                            |                            |
  v                            v                            v
Local behavioral model     Local behavioral model     Local behavioral model
  |                            |                            |
  +------------- Gradient aggregation server ---------------+
                               |
                               v
                    Updated global model
                               |
                    +---------++---------+
                    v          v          v
                Device A   Device B   Device C
```

Only model gradients — not raw provenance records — cross device boundaries. Each device trains locally on its own behavioral data and shares only the model update. The aggregation server (which can be any device in the mesh, or a dedicated server for enterprise deployments) combines gradients using federated averaging.

**Benefit:** Devices with rare agent types benefit from behavioral baselines learned across the fleet. An agent installed on only one device still gets meaningful anomaly detection because similar behavioral patterns have been observed across other devices.

**Privacy guarantee:** Raw provenance records never leave the device. Gradient updates are clipped and noised (section 14.3) before transmission.

### 14.2 Adversarial Robustness

Train anomaly detection models against adversarial examples — agents deliberately crafted to evade behavioral detection. Research on GAN-augmented training shows 10-15% F1 improvement on adversarial datasets.

**Continuous red-team loop:**

```text
1. Generator: synthesize agent behavior traces that evade current detectors
   (e.g., slow-drip exfiltration below burst threshold, mimicry of normal agents)
       |
       v
2. Discriminator: current anomaly detection models attempt to classify
       |
       v
3. Retrain: update detection models on adversarial examples
       |
       v
4. Repeat: generator adapts to updated models
```

The generator is constrained to produce only physically realizable behavior traces — it cannot synthesize actions that the capability system would prevent. This ensures the adversarial training targets realistic evasion strategies.

**Integration with Inspector:** Adversarial robustness scores for each detection model are displayed in a diagnostic view. Security-conscious users can see which detection capabilities are most vulnerable to evasion and prioritize model updates.

### 14.3 Differential Privacy for Audit Analysis

Apply differential privacy guarantees to aggregate audit statistics shared across devices in multi-device deployments. Ensures that individual provenance records cannot be reconstructed from aggregate reports.

**Mechanism:** Laplace noise addition to aggregate counts before sharing. Privacy budget (epsilon) is tracked per device per reporting period. When the budget is exhausted, no further aggregate reports are generated until the next period.

**Integration with Inspector:** The Inspector's Export View includes a "Privacy-preserving export" option that applies differential privacy to exported aggregate statistics. The privacy budget remaining is displayed in the export dialog. Individual provenance records are never exported — only aggregates with calibrated noise.

### 14.4 Causal RL for Automated Response

A reinforcement learning agent that learns optimal response policies for security incidents, reducing the time between detection and containment.

**MDP formulation:**

| Component | Definition |
|---|---|
| State | Current security posture: agent states, capability configuration, recent events, alert queue |
| Actions | Revoke capability, pause agent, send alert, escalate to user, do nothing |
| Reward | +1 for true positive containment, -1 for false positive disruption, -10 for missed true positive |
| Transition | Determined by agent responses to containment actions |

**Human-in-the-loop:** The RL agent suggests actions but never executes autonomously. Suggestions appear in the Security Events View as "Recommended response" with a confidence score. The user confirms, modifies, or dismisses each suggestion. User decisions feed back into the RL agent's reward signal, enabling it to learn the user's risk tolerance over time.

**Safety constraint:** The RL agent is prohibited from suggesting actions that would brick the system (e.g., pausing all agents simultaneously, revoking all capabilities). A hard-coded safety policy vetoes any suggestion that would reduce the system to fewer than 2 active agents or remove capabilities from Trust Level 3 (native) agents.

### 14.5 Runtime Guardrail Frameworks

Integration with application-level guardrail frameworks (NeMo Guardrails, Guardrails AI) that operate above the OS capability layer. The Inspector displays guardrail violation events alongside OS-level capability violations, providing a unified security view spanning both layers.

**Unified event model:**

```text
OS-level events:
  - Capability denials (kernel-enforced)
  - Behavioral anomalies (behavioral monitor)
  - Provenance chain violations (intent verifier)

Application-level events:
  - Guardrail violations (content safety, topic restrictions)
  - Output validation failures (format, factuality checks)
  - Input screening triggers (injection detection)

Unified view:
  Both event types appear in the Security Events View with a source indicator
  (kernel vs. guardrail). Semantic clustering (section 12.6) groups related
  events across both layers.
```

This integration requires agents to forward guardrail events to the kernel audit system via a dedicated syscall. The Inspector treats guardrail events as advisory (they represent application-level policy, not OS-level enforcement) but includes them in anomaly scoring and investigation workflows.

-----

**Cross-references:**

- Behavioral monitor intelligence: [behavioral-monitor.md](../../intelligence/behavioral-monitor.md)
- Adversarial defense: [adversarial-defense.md](../../security/adversarial-defense.md)
- AIRS inference engine: [airs.md](../../intelligence/airs.md)
- Capability system: [capabilities.md](../../security/model/capabilities.md)
- Intent verifier: [intent-verifier.md](../../intelligence/intent-verifier.md)
- Privacy architecture: [privacy.md](../../security/privacy.md)
