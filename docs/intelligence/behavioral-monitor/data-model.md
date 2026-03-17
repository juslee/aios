# AIOS Behavioral Monitor — Data Model

Part of: [behavioral-monitor.md](../behavioral-monitor.md) — Behavioral Monitor Architecture
**Related:** [detection.md](./detection.md) — Detection algorithms, [response.md](./response.md) — Escalation and enforcement, [profiling.md](./profiling.md) — Agent behavior profiling

-----

## §3. Behavioral Data Model

### §3.1 Core Types

The Behavioral Monitor maintains per-agent baselines and per-agent policies. The top-level structure:

```rust
pub struct BehavioralMonitor {
    /// Per-agent learned behavioral baselines
    baselines: HashMap<AgentId, BehavioralBaseline>,
    /// Per-agent enforcement policies (may be agent-specific or trust-level defaults)
    policies: HashMap<AgentId, BehavioralPolicy>,
    /// IPC channel to AIRS for Tier 2 analysis requests
    airs_channel: ChannelId,
}
```

The `baselines` map grows as agents are installed and operates. Agents without a baseline entry are in the warmup period — only hard limits are enforced. The `policies` map is populated from three sources in priority order:

1. **Per-agent override** — set by the user or enterprise policy for a specific agent
2. **Trust-level default** — derived from the agent's trust level (Untrusted, Standard, Trusted, System)
3. **Global default** — hardcoded conservative policy for agents with no other policy source

### §3.2 BehavioralBaseline

The canonical baseline structure tracks per-agent behavior across multiple dimensions:

```rust
pub struct BehavioralBaseline {
    agent: AgentId,

    /// Time-bucketed action frequencies (one profile per hour of day)
    /// Captures diurnal patterns: "this agent is active 9am-5pm"
    hourly_profile: [ActionProfile; 24],

    /// Typical targets — spaces, network endpoints, other agents
    /// Used for target novelty detection (§4.4)
    usual_targets: HashSet<ActionTarget>,

    /// Aggregate data volume statistics (bytes per observation window)
    volume_stats: RunningStats,

    /// Observed action sequences (most common patterns)
    /// Used for sequence anomaly detection (§4.5)
    common_sequences: Vec<ActionSequence>,

    /// How many days of observation have been collected
    observation_days: u32,

    /// Last time this baseline was updated
    updated_at: Timestamp,
}
```

Each hour slot contains an `ActionProfile` with per-action-type running statistics:

```rust
pub struct ActionProfile {
    /// Space read operations per window
    read_count: RunningStats,
    /// Space write operations per window
    write_count: RunningStats,
    /// Network bytes transferred per window
    network_bytes: RunningStats,
    /// AIRS inference calls per window
    inference_calls: RunningStats,
    /// Child agent/process spawn operations per window
    spawn_count: RunningStats,
}
```

The 24-element `hourly_profile` array enables temporal anomaly detection: an agent that reads 100 objects at 2 PM is normal if it always does that; the same agent reading 100 objects at 3 AM is anomalous if it has never been active at night.

### §3.3 RunningStats

All statistical baselines use Welford's online algorithm for numerically stable incremental computation of mean and variance:

```rust
pub struct RunningStats {
    /// Running mean (Welford's algorithm)
    mean: f64,
    /// Running variance (computed from M2 / (count - 1))
    variance: f64,
    /// Number of observations
    count: u64,
    /// Minimum observed value
    min: f64,
    /// Maximum observed value
    max: f64,
}
```

**Update procedure** (O(1) per observation):

```rust
impl RunningStats {
    pub fn update(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = value - self.mean;
        // M2 accumulates sum of squared differences
        // variance = M2 / (count - 1) for sample variance
        self.variance = if self.count > 1 {
            ((self.count - 2) as f64 * self.variance + delta * delta2)
                / (self.count - 1) as f64
        } else {
            0.0
        };
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    pub fn z_score(&self, value: f64) -> f64 {
        if self.variance <= 0.0 || self.count < 2 {
            return 0.0; // insufficient data
        }
        (value - self.mean) / self.variance.sqrt()
    }
}
```

**Why Welford's:** Classic mean/variance computation requires storing all observations or is numerically unstable with floating-point accumulation. Welford's algorithm is:
- O(1) per observation (no storage of history)
- Numerically stable (no catastrophic cancellation)
- Incrementally updatable (no recomputation from scratch)
- Fixed-size state (5 fields, 40 bytes)

See [detection.md §4.1](./detection.md) for how z-scores drive anomaly detection.

### §3.4 BehavioralPolicy

Each agent has a policy that defines hard limits and detection thresholds:

```rust
pub struct BehavioralPolicy {
    /// Absolute limits — never exceeded regardless of baseline
    hard_limits: HardLimits,
    /// Statistical detection threshold (z-score, default 3.0)
    /// A z-score of 3.0 means "this observation is 3 standard deviations
    /// from the mean" — expected to occur by chance < 0.3% of the time
    anomaly_threshold: f64,
    /// How the monitor responds when anomalies are detected
    response: EscalationPolicy,
}

pub struct HardLimits {
    /// Maximum space read operations per minute
    max_reads_per_minute: u32,
    /// Maximum space write operations per minute
    max_writes_per_minute: u32,
    /// Maximum network bytes transferred per minute
    max_network_bytes_per_minute: u64,
    /// Maximum AIRS inference calls per minute
    max_inference_calls_per_minute: u32,
    /// Maximum child agents/processes the agent may spawn
    max_children: u32,
}
```

**Trust-level defaults:**

| Trust Level | max_reads/min | max_writes/min | max_network_bytes/min | max_inference/min | max_children |
|---|---|---|---|---|---|
| Untrusted | 60 | 10 | 1 MB | 10 | 0 |
| Standard | 300 | 60 | 10 MB | 30 | 2 |
| Trusted | 1000 | 300 | 100 MB | 100 | 8 |
| System | 10000 | 3000 | 1 GB | 1000 | 32 |

Hard limits serve as the safety net even when baselines are absent or manipulated. See [evasion.md §11.2](./evasion.md) for why hard limits are critical for evasion resistance.

### §3.5 AnomalyType

The monitor classifies detected anomalies into five categories:

```rust
pub enum AnomalyType {
    /// Action frequency exceeds baseline by more than threshold standard deviations
    FrequencySpike {
        action: ActionType,
        observed: f64,
        baseline_mean: f64,
        z_score: f64,
    },

    /// Data volume exceeds baseline
    VolumeSpike {
        observed_bytes: u64,
        baseline_mean: f64,
        z_score: f64,
    },

    /// Action at unusual time of day
    TemporalAnomaly {
        hour: u8,
        action: ActionType,
        /// How frequently this action occurs at this hour in the baseline
        baseline_frequency: f64,
    },

    /// Access to a target (space, endpoint) never seen in observation period
    NewTarget {
        target: ActionTarget,
        /// How many days the baseline has been built (more days = more confidence)
        observation_days: u32,
    },

    /// Unusual action sequence (edit distance from nearest known sequence exceeds threshold)
    SequenceAnomaly {
        observed: Vec<ActionType>,
        nearest_known: Vec<ActionType>,
        edit_distance: u32,
    },
}
```

Each anomaly type is handled by a dedicated detector. See [detection.md §4](./detection.md) for the detection algorithms.

### §3.6 Behavioral State Byte

The kernel maintains a per-process `behavioral_state` byte that reflects the Behavioral Monitor's current assessment:

```rust
/// Stored in kernel per-process data, accessed in the IPC fast path.
/// Written by AIRS via lightweight notification (not IPC — avoids recursion).
/// Read by the kernel at IPC entry point (step 3 of the zero-trust enforcement stack).
#[repr(u8)]
pub enum BehavioralState {
    /// Normal operation — no enforcement applied
    Normal = 0,
    /// Elevated monitoring — actions are logged with extra detail
    Elevated = 1,
    /// Rate-limited — IPC processing slowed by a configurable factor
    RateLimited = 2,
    /// Suspended — all IPC blocked, agent paused
    Suspended = 3,
}
```

**Update path:** AIRS writes the behavioral state byte via a lightweight kernel notification (a dedicated syscall, not an IPC message — IPC itself is gated by this byte, so using IPC to update it would create a dependency loop). The kernel applies the state at the IPC entry point as part of the zero-trust enforcement stack ([operations.md §10.4](../../security/model/operations.md)).

**Persistence:** The behavioral state byte is transient — it resets to `Normal` on agent restart. This is intentional: if an agent was rate-limited due to a transient anomaly, restarting it gives it a clean slate. Persistent enforcement (e.g., revoking capabilities) happens through the capability system, not the behavioral state byte.

**AIRS unavailability:** When AIRS is not running, the behavioral state byte retains its last written value. The kernel does not automatically clear it. This means a rate-limited agent remains rate-limited until AIRS recovers and updates the state. The kernel falls back to capability-only checks for agents in `Normal` state.

### §3.7 Storage Model

Behavioral baselines are persisted in the `system/audit/behavioral/` space:

```text
system/audit/behavioral/
├── baselines/
│   ├── {agent_id_1}.baseline    # Serialized BehavioralBaseline
│   ├── {agent_id_2}.baseline
│   └── ...
├── policies/
│   ├── {agent_id_1}.policy      # Per-agent policy override (if any)
│   ├── trust_level_defaults.policy  # Trust-level default policies
│   └── global_default.policy    # Global fallback policy
└── events/
    ├── {date}/                  # Daily event logs
    │   ├── anomalies.log        # Detected anomalies
    │   └── enforcement.log      # Enforcement actions taken
    └── retention.config         # Retention policy settings
```

**Update frequency:** Baselines are updated incrementally every observation window (1 minute for aggregate statistics, 1 hour for hourly profile rotation). Baseline files are written to disk every 15 minutes or on clean shutdown.

**Retention:**
- Baselines: retained as long as the agent is installed, deleted on agent uninstall
- Anomaly events: 7 days full detail, 90 days summarized (count + severity per day), hash-only after 90 days
- Enforcement events: 90 days full detail (for compliance and forensic analysis)

**Bootstrap:** A newly installed agent has no baseline file. The monitor creates an empty baseline and enters `AuditOnly` mode for the warmup period (default: 7 days). During warmup, hard limits are enforced but statistical detection is suppressed. See [detection.md §5.1](./detection.md) for warmup details.
