# AIOS Preference Intelligence

Part of: [preferences.md](../preferences.md) — Preference System
**Related:** [inference.md](./inference.md) — Behavioral observer pipeline, [temporal.md](./temporal.md) — Context-driven rules, [security.md](./security.md) — Anomaly detection audit events

-----

The Preference System embeds two tiers of machine intelligence that mirror the split described in the input subsystem's AI architecture (see [input/ai.md](../../platform/input/ai.md) §10): **kernel-internal models** that run at sub-millisecond latency with no external dependencies, and **AIRS-dependent services** that leverage the AI Runtime for semantic understanding and online learning.

This architectural split is load-bearing for preferences. Kernel-internal models handle circadian detection, confidence scoring, and conflict prediction — features that must work even when AIRS has not yet started or is temporarily unavailable. AIRS-dependent services add learning (contextual bandits, NLU disambiguation, cross-preference analysis) that improves the experience but is never required for the system to function correctly.

```text
Preference Intelligence Architecture

┌───────────────────────────────────────────────────────────────┐
│               Kernel-Internal ML (~200KB total)                │
│               Always available, <1ms latency                   │
│                                                                │
│  Temporal Detection    Confidence Scoring    Conflict Predict  │
│  ──────────────────    ─────────────────     ───────────────── │
│  Circadian histogram   Beta distribution     Decision tree     │
│  Weekly histogram      Bayesian interval     Feature extractor │
│  Recency-weighted MA   Min observation gate  Proactive notify  │
└──────────────────────────┬────────────────────────────────────┘
                           │ IPC (PreferenceLearning,
                           │      BehaviorRead, ContextRead)
┌──────────────────────────┴────────────────────────────────────┐
│               AIRS-Dependent Services                          │
│               Available when AIRS is online                    │
│                                                                │
│  Contextual Bandits    Semantic NLU        Dependency Graph    │
│  ──────────────────    ────────────        ────────────────    │
│  LinUCB / Dueling      Transformer NLU     Knowledge graph     │
│  Reward feedback       Multi-pref resolve  Cascade analysis    │
│  Feature-context map   Disambiguation      Proactive suggest   │
│                                                                │
│  Anomaly Detection     Personalization Profile                 │
│  ─────────────────     ───────────────────────                 │
│  Privacy reduction     New-device bootstrap                    │
│  Bulk change detect    Preference embeddings                   │
│  Profile deviation     Federated transfer                      │
└───────────────────────────────────────────────────────────────┘
```

-----

## 16. AI-Native Preference Intelligence (AIRS-Dependent)

### 16.1 Contextual Bandits for Preference Learning

When AIRS is online, it employs a contextual bandit framework to learn which preference suggestions the user accepts. Rather than proposing changes based solely on frequency thresholds (as in the kernel-internal behavioral observer — see [inference.md](./inference.md) §6), the bandit model explores a space of possible suggestions and learns from accept/reject signals.

**Algorithm options:**

| Property | LinUCB | Neural Dueling Bandit |
|---|---|---|
| Model type | Linear UCB (contextual) | Small neural network |
| Parameters | d × K weight matrix (d = context dim, K = actions) | ~500K params |
| Latency | <2ms | 5–10ms |
| Privacy | Context features only; no raw content | Same |
| Update | Online (incremental) | Mini-batch updates |
| Cold start | Good (linear fallback) | Requires warm-up period |

LinUCB is the default. Neural Dueling Bandits are activated when a user has more than 90 days of preference history and the LinUCB exploration bonus has shrunk to near-zero (indicating linear separability is insufficient).

**Context features:**

| Feature | Type | Notes |
|---|---|---|
| `hour_of_day` | Normalized float [0, 1] | Captures circadian patterns |
| `day_of_week` | One-hot [7] | Captures work vs. weekend |
| `activity` | Categorical (embedding dim=8) | From Context Engine |
| `device` | Categorical (embedding dim=4) | Per-device or universal preference scope |
| `battery_level` | Float [0, 1] | Relevant for power preferences |
| `ambient_light` | Float [0, 1] | Relevant for display preferences |
| `recent_app` | Categorical (embedding dim=8) | Active application context |

**Reward signal:**

```text
User accepts suggestion          →  +1.0
User rejects suggestion          →  -0.5
User accepts then reverts        →  -1.0  (within 10 minutes)
User does not respond (timeout)  →   0.0  (no update)
```

The asymmetric reward structure penalizes revert-after-accept more than immediate rejection, since accepted-then-reverted changes reveal that the suggestion timing or value was wrong even though the preference direction was not entirely incorrect.

**Capability chain:**

The bandit model runs inside AIRS and requires the `PreferenceLearning` capability. It never receives raw preference values — only context feature vectors and binary accept/reject signals forwarded by the Preference Service after user interaction.

-----

### 16.2 Semantic NLU Enhancement

The Preference Service includes a keyword-based NLU pipeline described in [resolution.md](./resolution.md) §5.1 that handles common requests like "make text bigger" or "dark mode". When AIRS is online, a second-tier transformer model handles ambiguous and multi-preference utterances that the keyword pipeline cannot resolve.

**Tier 2 NLU model:**

| Property | Value |
|---|---|
| Model type | 6-layer transformer encoder (distilled) |
| Parameters | 10–20M (INT8 quantized) |
| Input | User utterance + current preference context |
| Output | Ranked (preference_id, value, confidence) tuples |
| Latency | 8–15ms on Cortex-A72 (NEON SIMD via GGML Runtime) |
| Fallback | Tier 1 keyword pipeline when AIRS offline |

**Ambiguity examples that require Tier 2:**

| User says | Tier 1 result | Tier 2 resolution |
|---|---|---|
| "make it easier to read" | Ambiguous — multiple targets | `display.font_scale` ↑, `display.high_contrast` on, `display.reduce_motion` on |
| "I find this too cluttered" | Unknown | `display.density` → compact |
| "something feels slow" | Unknown | `display.animation_speed` ↓ + prompt for more detail |
| "less distracting" | Unknown | `attention.night_suppress` schedule, suppress specific agent |
| "more comfortable at night" | Ambiguous | `display.night_shift` + `display.brightness` ↓ |

**Multi-preference changes from a single utterance:**

When Tier 2 maps a single utterance to multiple preference changes, each change is presented as a grouped proposal rather than separate suggestions. The user accepts or rejects the group atomically, with the option to expand and adjust individual items.

**Disambiguation with follow-up questions:**

When confidence is below a threshold (0.65), AIRS generates a clarifying question rather than applying a low-confidence change:

```text
User: "make it more comfortable"
AIRS confidence: 0.52 (ambiguous between display, audio, input categories)

Response: "Did you mean display brightness and colors, or something about
           sounds, or input feel? I can adjust any of these."
```

The clarification flow is handled via the Conversation Bar interface described in [resolution.md](./resolution.md) §5.1. The NLU model receives the follow-up response and re-resolves with the additional context, typically reaching confidence > 0.9.

-----

### 16.3 Cross-Preference Dependency Analysis

Changing one preference often has cascade effects on related preferences that the user may not anticipate. AIRS maintains a **preference knowledge graph** that encodes these relationships and uses it to generate proactive suggestions when a primary preference changes.

**Knowledge graph structure:**

Nodes are preference IDs. Edges encode dependency types:

| Edge type | Meaning | Example |
|---|---|---|
| `Suggests` | Changing A often benefits from also changing B | `display.font_scale` ↑ → suggests `display.density` adjust |
| `Requires` | Changing A to a specific value requires B to be compatible | `display.high_contrast` on → requires `display.theme` = dark or light (not auto) |
| `Conflicts` | Changing A may conflict with the current value of B | `display.animation_speed` = 0 conflicts with `display.reduce_motion` = false |
| `Enhances` | Changing A to value X is enhanced by also setting B | `display.theme` = dark → enhances `display.accent_color` suggestions |

**Example cascade analysis:**

```text
User action: "dark mode"
  → display.theme = dark (applied)

AIRS graph traversal:
  display.theme → Suggests → display.accent_color
    current: #4A90D9 (blue, designed for light backgrounds)
    AIRS: "Your current accent color was chosen for light mode.
           Would you also like to update it to work well on dark backgrounds?
           [Suggest colors] [Keep current] [Skip]"

  display.theme → Enhances → display.night_shift
    if current time is evening and night_shift = false:
    AIRS: "Dark mode is set. Night Shift (warm color filter) also reduces
           eye strain at night. Enable it? [Yes] [No]"
```

The knowledge graph is built from a combination of static design rules (encoded at build time, kernel-internal) and dynamic patterns learned from the AIRS bandit model. The static component covers structural constraints (e.g., high_contrast requires explicit theme). The dynamic component covers soft suggestions that vary by user preference profile.

-----

### 16.4 Preference Anomaly Detection

AIRS monitors preference change streams for patterns that indicate security threats or unauthorized access. These checks complement the capability-gated access control described in [security.md](./security.md) §15 — they detect attacks that hold valid capabilities but exhibit anomalous behavior patterns.

**Anomaly categories:**

| Category | Detection Method | Severity |
|---|---|---|
| Privacy reduction | Single change: `privacy.ai_data_local_only` true → false, or `privacy.identity_disclosure` → `full` | High |
| Bulk change burst | >5 system preference changes from a single agent within 60 seconds | Medium |
| Profile deviation | Change vector cosine similarity to user's historical profile < threshold | Medium |
| Capability scope creep | Agent reads preferences outside its declared manifest scope | High |
| Rejected suggestion retry | Agent re-suggests a preference the user rejected within rejection cooldown | Low (rate-limited per §15.6) |

**Privacy reduction detection:**

Privacy-category preferences (`privacy.*`) are classified as `CategoryRestricted` (see [data-model.md](./data-model.md)). Any change that reduces privacy protection — enabling data sharing, expanding disclosure level, or disabling local-only AI processing — triggers an anomaly event regardless of whether the change has valid capability authorization.

```text
Anomaly event → PreferenceAuditEvent::AnomalyDetected (see security.md §15.4)
             → Security model event log (see security/model/operations.md §6)
             → User-visible notification: "A preference change may have reduced
               your privacy settings. Review recent changes? [Yes] [Dismiss]"
```

**Profile deviation detection:**

AIRS maintains a learned embedding of the user's preference change history. Each change is represented as a vector in preference space. Significant deviations from the user's historical patterns are flagged for review — not blocked, but surfaced.

The deviation threshold is set conservatively to minimize false positives: the 95th percentile of the user's own historical change vectors defines the normal boundary. Changes beyond the 99th percentile generate a notification.

**Integration with security audit:**

All anomaly detection events are forwarded to the audit trail described in [security.md](./security.md) §15.4. This allows the Inspector application (see [applications/inspector.md](../../applications/inspector.md)) to display preference anomalies in the security timeline alongside other security events.

-----

### 16.5 Personalization Profile Learning

When a user sets up a new device, the Preference System needs an initial configuration. AIRS learns a low-dimensional representation of the user's preference space that enables two capabilities: **new-device bootstrapping** (predicting preferences from a few initial choices) and **federated transfer** (learning from similar users without centralizing personal data).

**Preference embeddings:**

AIRS learns a per-user preference embedding by training a small autoencoder on the user's preference change history. The embedding captures the user's stylistic tendencies:

| Component | Technique | Dimension |
|---|---|---|
| Preference style | Autoencoder on change history | 32-dim |
| Temporal patterns | LSTM on time-series of changes | 16-dim |
| Context sensitivity | Attention over context features | 16-dim |
| Combined profile | Concatenated, normalized | 64-dim |

**New-device bootstrap:**

During first boot or new-device setup, the user makes a small number of initial preference choices (guided by the first-boot experience described in `boot/accessibility.md §21`). AIRS uses these initial choices as a query against the preference embedding space to predict remaining preferences.

```text
Bootstrap flow:
1. User makes 5–8 initial choices (theme, font size, language, etc.)
2. AIRS encodes choices → query embedding (64-dim)
3. Nearest-neighbor lookup in user's existing profile embedding
   (or population embedding for new users without history)
4. Top-K preferences ranked by confidence are pre-populated
5. Remaining preferences use system defaults
6. Profile embedding updates incrementally as user accepts or changes predictions
```

**Federated transfer (opt-in):**

Users who opt in to federated preference sharing contribute gradient updates (not raw data) to a population preference model. This model enables better cold-start predictions for new users and improves cross-device bootstrapping. The federated protocol uses:

- Local training on device (AIRS never sends raw preference values off-device)
- Gradient aggregation via ANM Mesh Protocol (see [networking/protocols.md](../../platform/networking/protocols.md) §5.1)
- Differential privacy (Gaussian noise addition before gradient upload)
- Explicit opt-in per preference category, with granular control

The federated model is a complement to per-user personalization — it improves cold-start accuracy from ~60% to ~75% on common preference predictions, without any centralized data collection.

-----

## 17. Kernel-Internal ML

### 17.1 Time-Series Pattern Detection

The Behavioral Observer described in [inference.md](./inference.md) §6 collects observations and forms hypotheses. The kernel-internal pattern detector determines whether a hypothesis has sufficient statistical support to be proposed to the user — without any AIRS dependency.

**Circadian pattern detection:**

The primary time-series model is a pair of histograms per preference hypothesis:

```text
hour_histogram:  [f32; 24]    // observation count per hour-of-day, normalized
dow_histogram:   [f32; 7]     // observation count per day-of-week, normalized
recency_weight:  f32          // exponential decay factor (λ = 0.1 per day)
```

The detector computes a **circadian score** as the peak-to-mean ratio of the hour histogram. A circadian score above 2.5 (peak hour has 2.5× the average) indicates a strong daily pattern. A weekly score above 1.8 (two or more days have 1.8× average) indicates a weekly pattern.

| Pattern type | Detection criterion | Example |
|---|---|---|
| Daily | Peak hour ≥ 2.5× mean across 7+ days | Dark mode at 20:00 every evening |
| Weekly | ≥2 day-of-week bins ≥ 1.8× mean | Volume increase on weekday mornings |
| Event-triggered | Context tag co-occurrence rate ≥ 0.8 | Volume mute when calendar shows meeting |

**Recency-weighted moving average:**

Older observations are weighted less than recent ones. The effective window is the last 30 days with exponential decay. This prevents stale patterns from persisting after a user's routine changes.

**Model properties:**

| Property | Value |
|---|---|
| Model type | Histogram + moving average (no ML inference) |
| Size | ~200 bytes per hypothesis (2 histograms + metadata) |
| Latency | <0.01ms per update |
| Update strategy | Incremental; each observation updates relevant bins |
| Privacy | No raw timestamps stored; only aggregated counts |

-----

### 17.2 Confidence Scoring

Before the Behavioral Observer proposes a preference hypothesis (see [inference.md](./inference.md) §6.2), the confidence model determines whether the statistical evidence is sufficient. The kernel-internal confidence models are frozen statistical estimators — no neural inference, no AIRS dependency.

**Binary preferences (Beta distribution):**

For binary preferences (dark mode on/off, notification suppress on/off), confidence is modeled as a Beta distribution:

```rust
pub struct BinaryConfidenceModel {
    /// Beta distribution parameter: successes (observed activations)
    pub alpha: f32,
    /// Beta distribution parameter: non-activations
    pub beta: f32,
    /// Minimum total observations before proposal
    pub min_observations: u32,
    /// Minimum credible interval lower bound before proposal
    pub min_ci_lower: f32,
}

impl BinaryConfidenceModel {
    /// Returns (mean, credible_interval_lower_95)
    pub fn estimate(&self) -> (f32, f32) {
        let mean = self.alpha / (self.alpha + self.beta);
        // Approximate 95% CI lower bound using normal approximation
        let n = self.alpha + self.beta;
        let std = (mean * (1.0 - mean) / n).sqrt();
        let ci_lower = mean - 1.645 * std;
        (mean, ci_lower)
    }

    pub fn ready_to_propose(&self) -> bool {
        let total = (self.alpha + self.beta) as u32;
        let (_, ci_lower) = self.estimate();
        total >= self.min_observations && ci_lower >= self.min_ci_lower
    }
}
```

Default thresholds: `min_observations = 5`, `min_ci_lower = 0.75`. This matches the threshold described in [inference.md](./inference.md) §6.1 (`observation_count >= threshold && confidence >= 0.85`).

**Continuous preferences (Bayesian credible interval):**

For continuous preferences (brightness level at a given time, font scale adjustment), confidence uses a conjugate normal-normal model:

```rust
pub struct ContinuousConfidenceModel {
    /// Posterior mean (running estimate of preferred value)
    pub posterior_mean: f32,
    /// Posterior variance
    pub posterior_variance: f32,
    /// Number of observations
    pub n: u32,
    /// Minimum observations before proposal
    pub min_observations: u32,
    /// Maximum acceptable posterior standard deviation before proposal
    pub max_std: f32,
}
```

The model is ready to propose when the 95% credible interval width (approximately `2 × 1.96 × posterior_std`) is narrow enough that the proposed value is precise. `max_std = 0.05` for normalized [0, 1] preferences means the estimate is within ±0.1 of the true preference at 95% confidence.

-----

### 17.3 Conflict Prediction

Before applying or proposing a preference change, the Preference Service runs a lightweight conflict predictor to check whether the change will generate a conflict with an existing rule or a previously-explicit preference. This is a proactive gate that prevents the user from seeing unexpected conflict dialogs after accepting a suggestion.

**Feature extraction:**

The conflict predictor operates on a fixed feature vector extracted from the pending change:

| Feature | Description |
|---|---|
| `preference_category` | One-hot [11 categories] |
| `change_magnitude` | Normalized distance from current value |
| `source_authority` | Numeric rank (SystemDefault=0 → EnterpriseLocked=6) |
| `has_explicit_history` | Bool: has user ever explicitly set this preference? |
| `n_active_rules` | Count of active context rules targeting this preference |
| `n_agent_suggestions_pending` | Count of pending suggestions for same preference |

**Decision tree classifier:**

| Property | Value |
|---|---|
| Model type | Decision tree (depth ≤ 5) |
| Size | ~4KB (serialized tree) |
| Latency | <0.01ms |
| Output | `ConflictProbability: f32` in [0, 1] |
| Threshold | >0.7 → proactive user notification before applying change |

The tree is frozen at build time and trained on a synthetic dataset of preference change scenarios with known conflict outcomes. It is not updated at runtime.

**Proactive notification:**

When `ConflictProbability > 0.7` for a proposed change:

```text
Before: "Would you also like to enable dark mode automatically at 8pm?"

After conflict prediction check finds an existing explicit rule for
display.theme at 20:00:

Modified proposal: "I noticed you previously set dark mode to start
at 8pm manually. Would you like to make that automatic instead?
[Yes, automate it] [Keep doing it manually] [Change the time]"
```

The notification incorporates the reason for the predicted conflict, sourced from the preference history (see [history.md](./history.md) §9).

-----

### 17.4 Feature Importance

The kernel-internal feature importance model answers the question: "which context features most strongly correlate with this preference changing?" This enables two downstream capabilities — targeted context rule suggestions (see [temporal.md](./temporal.md)) and preference change explainability (see [history.md](./history.md) §9.1).

**Shapley value approximation:**

For each preference hypothesis with sufficient observations, the system computes an approximate feature importance vector using a permutation-based Shapley approximation over the context features collected at observation time:

```rust
pub struct FeatureImportance {
    /// Preference this importance vector belongs to
    pub preference_id: PreferenceId,
    /// Importance score per context feature (higher = more predictive)
    /// Feature order: [hour, dow, battery, ambient_light, activity, app]
    pub scores: [f32; 6],
    /// Number of observations used to compute this estimate
    pub n_observations: u32,
}
```

The computation is a simple accumulated permutation test run incrementally as observations arrive — not a full Shapley computation, but an approximation that converges within ~20 observations.

**Targeted context rule suggestions:**

When the feature importance vector has a dominant feature (score > 0.6), the Preference Service uses it to suggest a context rule rather than a fixed preference change:

| Dominant feature | Suggested rule type |
|---|---|
| `hour_of_day` | Time-of-day rule (e.g., "after 20:00") |
| `day_of_week` | Day-of-week rule (e.g., "on weekdays") |
| `activity` | Activity-triggered rule (e.g., "when focusing") |
| `ambient_light` | Sensor-triggered rule (e.g., "when ambient light < 50 lux") |
| `battery_level` | Power-state rule (e.g., "when battery < 30%") |

This connects the inference pipeline to the context rules system described in [temporal.md](./temporal.md), enabling the system to propose the right kind of automation based on the observed data rather than a fixed template.

**Explainability output:**

When the user asks "why did this change?" (see [history.md](./history.md) §9.1), the Preference Service includes the feature importance vector in the explanation:

```text
"I suggested enabling dark mode automatically because you've switched to
 dark mode yourself most evenings (80% of changes happen between 19:00
 and 21:00). Time of day is the strongest signal for this preference."
```

-----

### 17.5 Kernel-Internal Model Budget

Total model footprint for the full kernel-internal preference ML suite:

| Component | Model | Size |
|---|---|---|
| Circadian pattern detector | Per-hypothesis histograms (~200B each, max 64 hypotheses) | ~12KB |
| Binary confidence models | Beta distribution state (~24B each, max 64) | ~1.5KB |
| Continuous confidence models | Normal-normal state (~32B each, max 32) | ~1KB |
| Conflict prediction tree | Frozen decision tree | ~4KB |
| Feature importance accumulators | Per-hypothesis Shapley state (~48B each, max 64) | ~3KB |
| **Total frozen models** | | **~4KB** |
| **Per-user adaptive state** | All accumulators + histograms at runtime | **~22KB** |

All models are loaded once at Preference Service startup. Per-user adaptive state is loaded on user login from `user/preferences/_ml/` and saved on session close.

-----

## Future Directions

These capabilities are under research consideration for later development phases:

**Differential privacy for behavioral inference:** The behavioral observer (see [inference.md](./inference.md) §6) could apply DP-SGD (differentially private stochastic gradient descent) to the AIRS bandit model training, providing formal privacy guarantees on the learned preference patterns. The main challenge is calibrating the privacy budget (ε) against utility loss — early experiments suggest ε=2.0 preserves >90% of suggestion accuracy while providing meaningful privacy guarantees.

**Federated preference learning across devices:** The opt-in federated transfer described in §16.5 currently covers new-device bootstrap. A deeper version would allow the population model to improve continuously across devices without centralizing raw data, using secure aggregation and the ANM Mesh Protocol. This requires solving the heterogeneous-device federated learning problem (different device capabilities, intermittent connectivity, varying update frequencies).

**Safe RLHF for preference optimization:** The contextual bandit framework (§16.1) could be extended to reinforcement learning from human feedback (RLHF), where the reward model is trained on accept/reject signals and the policy is optimized subject to a safety constraint that prevents suggestions that reverse privacy settings or reduce accessibility features. The key design challenge is decoupling helpfulness optimization from safety — ensuring the RL objective cannot find adversarial suggestion strategies that technically maximize reward while violating user trust.

**Formal verification of conflict resolution with timed automata:** The conflict detection pipeline (§17.3) uses a heuristic classifier. A longer-term direction is to express the preference schema and conflict rules as a timed automaton and use model checking (e.g., UPPAAL) to verify that the conflict resolution algorithm always terminates and produces a deterministic outcome given the authority ranking defined in [resolution.md](./resolution.md) §4.1. This would provide formal correctness guarantees for the most security-sensitive preference operations.
