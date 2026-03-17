# AIOS Behavioral Monitor — Detection Engine

Part of: [behavioral-monitor.md](../behavioral-monitor.md) — Behavioral Monitor Architecture
**Related:** [data-model.md](./data-model.md) — Data structures, [response.md](./response.md) — Escalation and enforcement, [intelligence.md](./intelligence.md) — ML-powered detection enhancements

-----

## §4. Statistical Detection Engine

The detection engine evaluates agent behavior against baselines using five independent detectors. Each detector produces an `AnomalyType` with a severity score. Detectors run in parallel — an action may trigger multiple detectors simultaneously.

### §4.1 Z-Score Detection

The primary detection mechanism. For each action type (read, write, network, inference, spawn), the detector compares the current observation window's count against the baseline's `RunningStats` for the corresponding hour.

**Algorithm:**

```text
For each action_type in [read, write, network, inference, spawn]:
    current = count of action_type in current 1-minute window
    baseline = hourly_profile[current_hour].{action_type}
    if baseline.count < MIN_OBSERVATIONS (default: 48, i.e., 2 days):
        skip (insufficient data for this hour)
    z = (current - baseline.mean) / sqrt(baseline.variance)
    if z > policy.anomaly_threshold (default: 3.0):
        emit FrequencySpike { action_type, current, baseline.mean, z }
```

**Why z-score 3.0:** A z-score of 3.0 means the observation is 3 standard deviations above the mean. For a normal distribution, this occurs by chance less than 0.3% of the time (one-tailed). In practice, agent behavior distributions are not perfectly normal, but z-score provides a distribution-agnostic threshold that works well empirically:

| Z-Score | Probability of False Positive (Normal) | Practical Meaning |
|---|---|---|
| 2.0 | 2.3% | Noisy — too many false positives |
| 2.5 | 0.6% | Sensitive — useful for Tier 2 triage |
| **3.0** | **0.13%** | **Default — good balance** |
| 3.5 | 0.023% | Conservative — fewer false positives, more false negatives |
| 4.0 | 0.003% | Very conservative — only extreme anomalies |

The threshold is configurable per-agent via `BehavioralPolicy.anomaly_threshold`. Enterprise deployments may lower it (2.5) for high-security agents or raise it (3.5) for agents with naturally variable behavior.

**Hourly bucketing:** Z-scores are computed against the same hour's baseline (e.g., 2 PM current vs. 2 PM historical). This prevents time-of-day effects from causing false positives — an agent that is naturally busier during work hours is not flagged for higher afternoon activity.

### §4.2 Hard Limit Enforcement

Hard limits are absolute ceilings, independent of baselines. They are enforced at all times, including during the warmup period when no baseline exists.

```text
For each rate_metric in [reads/min, writes/min, network_bytes/min, inference/min]:
    current = count in current 1-minute sliding window
    limit = policy.hard_limits.{metric}
    if current > limit:
        immediately set behavioral_state = RateLimited
        emit enforcement event to provenance chain
        rate-limit agent until next window
```

**Sliding window implementation:** Hard limit counters use a 60-second sliding window with 1-second granularity (60 buckets). The current second's bucket accumulates; expired buckets are cleared. Total across all buckets is compared against the hard limit. This prevents burst-then-idle patterns from evading per-minute limits.

**Spawn limit:** The `max_children` limit is enforced differently — it is a concurrent count, not a rate. When an agent attempts to spawn a child beyond the limit, the spawn syscall returns an error immediately.

**Hard limits cannot be shifted by baseline learning.** Even if an agent's baseline shows high activity, hard limits remain fixed. This is the primary defense against baseline manipulation attacks (see [evasion.md §11.2](./evasion.md)).

### §4.3 Temporal Pattern Detection

Detects activity at unusual times by comparing the current hour's activity level against the baseline for that hour:

```text
For each action_type:
    baseline_freq = hourly_profile[current_hour].{action_type}.mean
    if baseline_freq < TEMPORAL_FLOOR (default: 0.1 per window):
        // This agent is rarely active at this hour
        if current > 0:
            emit TemporalAnomaly { hour, action_type, baseline_freq }
```

**Severity scoring:** Temporal anomalies are scored by the ratio of current activity to baseline frequency. An agent with a baseline frequency of 0.0 at 3 AM that suddenly produces 50 reads is a higher-severity anomaly than an agent with a baseline of 2.0 that produces 10.

**Weekend vs. weekday:** The hourly profile has 24 entries (one per hour), which conflates weekday and weekend patterns. For agents with strong weekday/weekend differences, AIOS supports an extended 168-entry profile (24 hours × 7 days). The extended profile activates automatically when the monitor detects significant weekday/weekend divergence (z-score > 2.0 between weekday and weekend means for the same hour, sustained over 4 weeks).

### §4.4 Target Novelty Detection

Detects access to spaces, network endpoints, or other agents that the agent has never accessed during the observation period:

```text
target = extract target from current action (space_id, endpoint, agent_id)
if target NOT IN baseline.usual_targets:
    if baseline.observation_days >= MIN_OBSERVATION_DAYS (default: 7):
        severity = target_sensitivity(target)
        emit NewTarget { target, observation_days }
```

**Target sensitivity weighting:** Not all new targets are equally alarming. Accessing a new `Untrusted` zone space is less concerning than accessing a new `Personal` or `Core` zone space. Sensitivity is derived from the target's security zone:

| Security Zone | Novelty Severity |
|---|---|
| Untrusted | Low |
| Ephemeral | Low |
| Personal | Medium |
| Core | High |
| Network endpoint (internal) | Low |
| Network endpoint (external, new domain) | High |

**Target set aging:** The `usual_targets` set grows over time as the agent legitimately accesses new targets. To prevent unbounded growth (which would reduce detection sensitivity), targets that haven't been accessed in 30 days are aged out of the set. This means an agent that accessed a space once 60 days ago will trigger a `NewTarget` anomaly if it accesses it again — the system treats it as a new pattern.

### §4.5 Sequence Analysis

Detects unusual patterns of actions — not just individual action rates, but the *order* in which actions occur:

```text
observed_sequence = last N actions (default: N=8, sliding window)
for each known_sequence in baseline.common_sequences:
    distance = edit_distance(observed_sequence, known_sequence)
    if distance <= SEQUENCE_THRESHOLD (default: 3):
        // Close enough to a known pattern
        return
// No known sequence within threshold
nearest = argmin(edit_distance(observed_sequence, s) for s in common_sequences)
emit SequenceAnomaly {
    observed: observed_sequence,
    nearest_known: nearest,
    edit_distance: edit_distance(observed_sequence, nearest),
}
```

**Action sequence representation:** Actions are abstracted to an `ActionType` enum for sequence comparison:

```rust
pub enum ActionType {
    SpaceRead,
    SpaceWrite,
    SpaceDelete,
    NetworkSend,
    NetworkReceive,
    InferenceCall,
    AgentSpawn,
    AgentCommunicate,
}
```

With 8 action types and a window of 8, the sequence space is 8⁸ = ~16 million possible sequences. In practice, agents follow a small number of repeated patterns (typically 5–20 common sequences), making deviation highly informative.

**Edit distance:** Levenshtein distance (insertions, deletions, substitutions). A threshold of 3 means up to 3 edits are tolerated before flagging. This accommodates minor variations in execution order (e.g., reading two spaces in reversed order) without flagging normal jitter.

**Memory budget:** The monitor stores at most 32 common sequences per agent (sorted by frequency). Sequences below a frequency threshold are evicted to keep memory bounded. Total memory per agent: 32 sequences × 8 actions × 1 byte = 256 bytes.

-----

## §5. Baseline Learning Algorithms

### §5.1 Warmup Period

A newly installed agent has no behavioral history. The warmup period allows the monitor to observe and build a baseline before activating statistical detection.

**Warmup configuration:**

| Parameter | Default | Range | Description |
|---|---|---|---|
| Duration | 7 days | 1–30 days | How long to observe before activating detection |
| Mode | `AuditOnly` | — | During warmup: log anomalies but don't enforce (except hard limits) |
| Min observations per hour | 48 | — | Require at least 48 windows (2 days) of data for an hour before using that hour's profile |
| Early activation | Yes | — | If all 24 hourly profiles have sufficient data before 7 days, activate early |

**During warmup:**
- Hard limits are enforced immediately (no warmup exemption)
- Statistical detectors run but their output is logged, not acted upon
- The user can view warmup anomalies in Inspector (labeled as "warmup — not enforced")
- Tier 2 (AIRS) analysis is active if available, providing early semantic classification

**Warmup duration adaptation:** Agents that are used infrequently (e.g., once a week) need longer warmup periods to collect meaningful baselines. The monitor tracks `observation_density` (windows with activity / total windows) and extends warmup if density is below 5%. The maximum extension is 30 days, after which the monitor activates with whatever baseline it has.

### §5.2 Incremental Updates

After warmup, baselines are updated continuously using the same Welford's algorithm that built them. Every 1-minute observation window produces a data point that updates the relevant `RunningStats` instances.

**Exponential decay:** To adapt to legitimate behavioral evolution (agent updates, user workflow changes), old observations are weighted less than recent ones. The monitor uses a **decayed Welford** variant:

```text
For each new observation x:
    effective_count = min(count, DECAY_WINDOW)  // default: 10080 (7 days of minutes)
    If count > DECAY_WINDOW:
        // Gradually forget old observations
        mean += (x - mean) / effective_count
        variance = (1 - 1/effective_count) * variance + (x - mean)^2 / effective_count
    Else:
        // Standard Welford update during initial baseline building
        standard Welford update
```

This ensures the baseline reflects the agent's behavior over the last ~7 days, not its entire lifetime. An agent that legitimately changes behavior (e.g., after an update that adds new features) will see its baseline adapt within one decay window.

**Decay window tuning:** The 7-day default balances two concerns:
- Too short (1 day): baselines shift too quickly, reducing detection sensitivity
- Too long (30 days): baselines adapt too slowly, causing persistent false positives after legitimate changes

Enterprise deployments can configure the decay window per trust level.

### §5.3 Multi-Timescale Baselines

A single timescale is insufficient for agents with complex usage patterns. The monitor maintains baselines at three timescales:

| Timescale | Window | Purpose | Detection Catches |
|---|---|---|---|
| **Fast** (EWMA) | ~5 minutes (α = 0.3) | Rapid anomaly response | Sudden spikes within a session |
| **Medium** (hourly) | 1 hour × 24 hours | Diurnal pattern matching | Off-hours activity, temporal anomalies |
| **Slow** (weekly) | 1 day × 7 days | Weekly pattern matching | Weekend attacks, periodic behavior |

The fast timescale (EWMA) is detailed in [intelligence.md §12.2](./intelligence.md). The medium timescale is the primary `hourly_profile` in `BehavioralBaseline`. The slow timescale adds a `daily_profile: [ActionProfile; 7]` that captures day-of-week patterns.

**Cross-timescale detection:** An anomaly is more severe if flagged by multiple timescales. The severity scorer applies a multiplier:

| Timescales Flagging | Severity Multiplier |
|---|---|
| 1 of 3 | 1.0× |
| 2 of 3 | 1.5× |
| 3 of 3 | 2.0× |

### §5.4 Cold Start Problem

Brand new agents have zero history — no baseline, no observations, no priors. The cold start problem asks: how should the monitor behave for an agent it has never seen?

**Cold start strategies (applied in priority order):**

1. **Hard limits** — Always enforced immediately. Provide a safety floor regardless of baseline state.

2. **Agent Capability Intelligence priors** — If AIRS has performed behavioral prediction for this agent (Stage 3 of the 5-stage pipeline, see [intelligence-services.md §5.9](../airs/intelligence-services.md)), the predicted behavior serves as a prior for the baseline. The monitor initializes `RunningStats` with the predicted mean and a high variance (low confidence).

3. **Category-based priors** — If no prediction is available, the monitor uses category-based defaults from the agent corpus (Stage 4 of the 5-stage pipeline). For example, "email agents typically read 10–50 objects/minute and use 1–5 MB/minute of network." These priors are looser than predictions but better than nothing.

4. **Trust-level defaults** — If no corpus data is available, fall back to the hard limits as implicit behavioral bounds. The agent operates under hard limits only until enough observations accumulate for statistical detection.

**Prior confidence decay:** Priors from predictions and corpus data are initialized with high variance (low confidence). As real observations accumulate, the variance decreases and the baseline transitions from prior-dominated to observation-dominated. After one full decay window (7 days), prior influence is negligible.

See [profiling.md §8.4](./profiling.md) for how predicted behavior is compared against observed behavior over time.
