# AIOS Intent Verifier — Behavioral Integration

Part of: [intent-verifier.md](../intent-verifier.md) — Intent Verifier Architecture
**Related:** [pipeline.md](./pipeline.md) — Verification Pipeline, [security.md](./security.md) — Adversarial Resistance, [specification.md](./specification.md) — Intent Specification

---

## §6 Behavioral Monitor Coordination

The Intent Verifier (Security Layer 1) and Behavioral Monitor (Security Layer 3) operate as a tandem defense. Neither layer is sufficient alone: intent verification catches semantic misalignment that statistical checks miss, while behavioral monitoring catches volume anomalies and pattern deviations that semantic checks cannot detect. This section defines how the two layers coordinate to produce a single enforcement decision for each observed action.

### §6.1 Layer 1 + Layer 3 Tandem Operation

Layer 1 (Intent Verifier) answers: *does this action match the declared purpose?* It compares the observed action against the agent's `StructuredIntent` using algorithmic pre-checks and, when necessary, LLM semantic verification through AIRS.

Layer 3 (Behavioral Monitor) answers: *does this action match historical patterns?* It compares the observed action against the agent's learned behavioral baseline — typical access rates, space access patterns, network targets, and resource consumption.

These two dimensions are orthogonal. An action can be semantically aligned but statistically anomalous (a legitimate batch export that is 10x normal volume), or statistically normal but semantically misaligned (routine email reads performed by an agent whose declared task is "compose a report"). The four combinations produce distinct interpretations and enforcement actions:

| Intent (L1) | Behavior (L3) | Interpretation | Action |
|---|---|---|---|
| Aligned | Normal | Expected operation | Allow |
| Aligned | Anomalous | Legitimate but unusual (e.g., large batch job) | Allow + Log |
| Misaligned | Normal | Routine action serving wrong purpose | Block + Alert |
| Misaligned | Anomalous | Active threat | Block + Suspend + Notify user |

The coordination logic combines verdicts from both layers into a single `CombinedVerdict`:

```rust
pub struct CombinedVerdict {
    /// Layer 1 result: semantic alignment check
    intent_result: VerificationResult,
    /// Layer 3 result: statistical normality check
    behavior_result: BehaviorAssessment,
    /// Combined enforcement action
    combined_action: CombinedAction,
    /// Timestamp of the verdict
    timestamp: Timestamp,
    /// Agent that triggered the check
    agent_id: AgentId,
}

pub enum BehaviorAssessment {
    /// Action falls within learned baseline parameters
    Normal,
    /// Action deviates from baseline but within tolerance
    Elevated { deviation: f32 },
    /// Action significantly deviates from baseline
    Anomalous {
        deviation: f32,
        conditions: Vec<BehaviorCondition>,
    },
}

pub enum CombinedAction {
    /// Both layers agree: action is expected
    Allow,
    /// Intent aligned but behavior unusual: permit with audit trail
    AllowWithLog,
    /// Intent misaligned: block and alert security subsystem
    Block { reason: String },
    /// Both layers flag problems: block, suspend agent, notify user
    Suspend { reason: String, notify_user: bool },
}
```

The combination logic follows a conservative merge rule: the most restrictive verdict wins. If Layer 1 returns `Aligned` but Layer 3 returns `Anomalous`, the action is allowed but logged. If Layer 1 returns `Violation`, the action is blocked regardless of Layer 3's assessment. The `Suspend` action is reserved for the case where both layers independently flag the action as problematic — this dual-trigger requirement prevents false positives from either layer alone from suspending an agent.

```rust
pub fn combine_verdicts(
    intent: &VerificationResult,
    behavior: &BehaviorAssessment,
) -> CombinedAction {
    match (intent, behavior) {
        (VerificationResult::Aligned, BehaviorAssessment::Normal) => CombinedAction::Allow,
        (VerificationResult::Aligned, BehaviorAssessment::Elevated { .. }) => CombinedAction::Allow,
        (VerificationResult::Aligned, BehaviorAssessment::Anomalous { .. }) => {
            CombinedAction::AllowWithLog
        }
        (VerificationResult::Violation { explanation }, BehaviorAssessment::Normal) => {
            CombinedAction::Block {
                reason: format!("semantic misalignment: {}", explanation),
            }
        }
        (VerificationResult::Suspicious { .. } | VerificationResult::Violation { .. },
         BehaviorAssessment::Elevated { .. }) => {
            CombinedAction::Block {
                reason: format!("semantic misalignment with elevated behavior"),
            }
        }
        (VerificationResult::Violation { .. }, BehaviorAssessment::Anomalous { conditions, .. }) => {
            CombinedAction::Suspend {
                reason: format!(
                    "dual-layer threat: intent misaligned + {} anomalous conditions",
                    conditions.len()
                ),
                notify_user: true,
            }
        }
    }
}
```

### §6.2 Cumulative Volume Tracking

A naive behavioral monitor that only checks per-minute rates is vulnerable to slow exfiltration. An attacker reading 1 object per hour stays below any reasonable per-minute threshold, yet accumulates thousands of reads over days. To catch this class of attack, the Behavioral Monitor maintains rolling window counters across multiple time scales.

```rust
pub struct VolumeTracker {
    /// Counters for different time windows
    windows: [RollingWindow; 3],
}

pub struct RollingWindow {
    /// Window duration
    duration: Duration,
    /// Current count within window
    count: u64,
    /// Threshold for this window
    threshold: u64,
    /// Oldest event timestamp in window
    window_start: Timestamp,
}

impl VolumeTracker {
    pub fn new() -> Self {
        Self {
            windows: [
                RollingWindow::new(Duration::from_secs(3600), 500),      // 1 hour
                RollingWindow::new(Duration::from_secs(86400), 5000),     // 1 day
                RollingWindow::new(Duration::from_secs(604800), 20000),   // 7 days
            ],
        }
    }

    pub fn record_and_check(&mut self, timestamp: Timestamp) -> Option<VolumeAlert> {
        for window in &mut self.windows {
            window.expire_old_events(timestamp);
            window.count += 1;
            if window.count > window.threshold {
                return Some(VolumeAlert {
                    window: window.duration,
                    count: window.count,
                    threshold: window.threshold,
                });
            }
        }
        None
    }
}

pub struct VolumeAlert {
    /// Which time window was exceeded
    pub window: Duration,
    /// Current count in that window
    pub count: u64,
    /// Threshold that was exceeded
    pub threshold: u64,
}
```

The multi-window approach catches three distinct attack patterns:

- **Slow exfiltration**: 1 read/hour sustained over 24 hours produces 24 reads — individually invisible per hour, but the daily window detects it if the agent's `ResourceBounds` declares `max_reads: 10` for a task expected to read a few files. The thresholds are agent-specific, not absolute.
- **Burst-then-pause**: 50 reads in a single hour from an agent with `max_reads: 20` per hour — caught by the hourly window even if the daily and weekly totals remain below threshold.
- **Gradual accumulation**: 30 reads/day over 7 days produces 210 reads/week — caught by the weekly window when the agent's declared weekly budget is 100, even though no single day exceeds its daily threshold.

Each `VolumeTracker` is per-agent and per-operation-class (reads, writes, network sends, inference requests). Thresholds are derived from the agent's `ResourceBounds` in its `StructuredIntent` specification, not from hardcoded constants. An agent declaring `max_reads: 50` gets tighter windows than one declaring `max_reads: 10000`.

### §6.3 Fixed Baseline Snapshots

Adaptive baselines that continuously update from observed behavior are vulnerable to gradual escalation. An attacker slowly increasing their activity over weeks causes the baseline to drift upward, eventually normalizing what would have been flagged as anomalous against the original baseline.

The Behavioral Monitor maintains dual baselines to detect this drift:

```rust
pub struct DualBaseline {
    /// Fixed snapshot from first 7 days of operation (never updated after freeze)
    frozen: BehaviorBaseline,
    /// Adaptive baseline from recent behavior (continuously updated)
    adaptive: BehaviorBaseline,
    /// Maximum allowed drift between frozen and adaptive
    max_drift_factor: f32,  // default: 3.0
    /// Whether the frozen baseline has been finalized
    frozen_complete: bool,
}

pub struct BehaviorBaseline {
    /// Typical reads per hour
    typical_read_rate: f32,
    /// Typical writes per hour
    typical_write_rate: f32,
    /// Typical network sends per hour
    typical_network_rate: f32,
    /// Typical inference requests per hour
    typical_inference_rate: f32,
    /// Set of spaces typically accessed
    typical_spaces: Vec<SpaceId>,
    /// Set of network endpoints typically contacted
    typical_endpoints: Vec<EndpointId>,
}

impl DualBaseline {
    pub fn check_drift(&self) -> Option<DriftAlert> {
        if !self.frozen_complete {
            return None; // Still in learning period
        }

        let read_drift = self.adaptive.typical_read_rate
            / self.frozen.typical_read_rate.max(0.001);
        let write_drift = self.adaptive.typical_write_rate
            / self.frozen.typical_write_rate.max(0.001);
        let network_drift = self.adaptive.typical_network_rate
            / self.frozen.typical_network_rate.max(0.001);

        let max_observed = read_drift.max(write_drift).max(network_drift);

        if max_observed > self.max_drift_factor {
            return Some(DriftAlert {
                dimension: if read_drift >= write_drift && read_drift >= network_drift {
                    "read_rate"
                } else if write_drift >= network_drift {
                    "write_rate"
                } else {
                    "network_rate"
                },
                frozen_value: self.frozen.typical_read_rate,
                adaptive_value: self.adaptive.typical_read_rate,
                drift_factor: max_observed,
            });
        }
        None
    }
}

pub struct DriftAlert {
    /// Which behavioral dimension drifted
    pub dimension: &'static str,
    /// Original frozen value
    pub frozen_value: f32,
    /// Current adaptive value
    pub adaptive_value: f32,
    /// Ratio of adaptive to frozen
    pub drift_factor: f32,
}
```

The frozen baseline captures the agent's behavior during its initial 7-day learning period. Once frozen, it is never updated. The adaptive baseline continues to track recent behavior using an exponential moving average. When the drift between the two exceeds `max_drift_factor` (default 3.0x), a `DriftAlert` is raised. This alert does not automatically block the agent — it triggers a re-evaluation by AIRS to determine whether the behavioral shift is legitimate (e.g., the user expanded the agent's responsibilities) or malicious.

The `max_drift_factor` is configurable per trust level. Higher trust levels (e.g., system agents) receive tighter drift bounds (2.0x), while lower trust levels (e.g., newly installed third-party agents) receive wider bounds (5.0x) to accommodate initial settling behavior.

### §6.4 Intent Anchoring

Baselines alone — whether frozen or adaptive — describe *what the agent has done*. They do not describe *what the agent should be doing*. Intent anchoring closes this gap by periodically re-validating the adaptive baseline against the agent's declared `StructuredIntent`.

AIRS performs intent anchoring checks every 24 hours (configurable per trust level):

1. **Source verification**: Does the adaptive baseline's `typical_spaces` set remain a subset of the `IntentPurpose.sources` declared in the agent's `StructuredIntent`? If the baseline shows the agent accessing spaces outside its declared scope, an alert is raised even if the access rate is statistically normal.

2. **Volume verification**: Does the cumulative volume tracked by `VolumeTracker` remain within the `ResourceBounds` declared in the `StructuredIntent`? An agent declaring `max_writes: 100` that has written 95 objects over 6 days is approaching its declared bounds — the intent anchoring check flags this for review before the limit is reached.

3. **Endpoint verification**: Does the adaptive baseline's `typical_endpoints` set match the network targets declared in the agent's capabilities? An agent with `Network(smtp.gmail.com)` capability whose baseline shows connections to `unknown-server.example.com` triggers an alert even if the connection rate is normal.

4. **Temporal verification**: Does the agent's activity pattern match any `TemporalSpec` constraints in its `StructuredIntent`? An agent declared as `OneShot` (single task, then done) whose baseline shows sustained activity beyond the expected task duration triggers review.

```rust
pub struct IntentAnchor {
    /// The agent's declared structured intent
    intent: StructuredIntent,
    /// Current adaptive baseline for comparison
    baseline: BehaviorBaseline,
    /// Last anchoring check timestamp
    last_check: Timestamp,
    /// Check interval (default: 24 hours)
    check_interval: Duration,
}

pub enum AnchorViolation {
    /// Agent accessing spaces not in declared sources
    UndeclaredSpaceAccess { space: SpaceId },
    /// Cumulative volume approaching declared bounds
    VolumeBoundApproaching { resource: &'static str, usage: u64, bound: u64 },
    /// Network targets outside declared endpoints
    UndeclaredEndpoint { endpoint: EndpointId },
    /// Activity pattern inconsistent with temporal spec
    TemporalViolation { expected: TemporalSpec, observed: &'static str },
}
```

Intent anchoring catches behavioral drift where the agent's behavior is statistically smooth — no single action triggers the behavioral monitor — but semantically drifting away from its declared purpose. A compromised email agent that gradually starts accessing calendar spaces, then contacts spaces, then documents, maintains smooth statistical behavior at each step but has fundamentally departed from its declared intent of "summarize unread emails."

---

## §9 Temporal Logic Monitor

The Behavioral Monitor's hardcoded rules (`BehaviorCondition::ReadRateExceeds(f32)`, `WriteRateExceeds(f32)`) are effective for simple rate-based checks but cannot express temporal relationships between events. Rules like "a bulk deletion must be preceded by user confirmation within the last 5 minutes" or "after reading credentials, the next network send must target a declared endpoint" require reasoning about event ordering and time intervals.

The Temporal Logic Monitor extends behavioral monitoring with a compact in-kernel evaluator for Metric Temporal Logic (MTL) formulas, enabling declarative behavioral rules that are loaded from agent manifests rather than compiled into kernel code.

### §9.1 MTL Evaluator Design

Metric Temporal Logic adds time bounds to temporal operators (`always`, `eventually`, `previously`, `until`), enabling precise specification of behavioral constraints that reference both event ordering and elapsed time.

The evaluator operates on pre-compiled automata rather than parsing MTL formulas at runtime. Rules are compiled from MTL formulas at agent install time by AIRS, producing frozen state machines that the kernel evaluator steps through as events arrive. This design keeps the in-kernel evaluator small (~1-2KB of code) and deterministic — no allocation, no parsing, no LLM dependency at evaluation time.

```rust
pub struct MtlEvaluator {
    /// Compiled rules from agent manifest + system policy
    rules: Vec<CompiledRule>,
    /// Event trace buffer (ring buffer, last N events)
    trace: RingBuffer<TraceEvent, 1024>,
}

pub struct CompiledRule {
    /// Rule identifier for audit trail references
    id: RuleId,
    /// Human-readable description (from manifest)
    description: &'static str,
    /// Compiled automaton (states + transitions)
    automaton: MtlAutomaton,
    /// Current automaton state
    state: AutomatonState,
    /// Action to take on rule violation
    on_violation: BehaviorAction,
}

pub struct MtlAutomaton {
    /// Number of states (max 64 for bounded formulas)
    num_states: u8,
    /// Transition table: (current_state, event_class) -> next_state
    transitions: Vec<Transition>,
    /// Accepting states (rule satisfied — no action needed)
    accepting: BitSet<64>,
    /// Rejecting states (rule violated — trigger on_violation)
    rejecting: BitSet<64>,
    /// Time-bounded transitions: must fire within duration or reject
    timed_transitions: Vec<TimedTransition>,
}

pub struct Transition {
    from_state: u8,
    event_class: EventClass,
    to_state: u8,
}

pub struct TimedTransition {
    from_state: u8,
    to_state: u8,
    /// Maximum time allowed in from_state before forced transition
    deadline: Duration,
    /// State to transition to on deadline expiry
    timeout_state: u8,
}

pub struct TraceEvent {
    timestamp: Timestamp,
    agent_id: AgentId,
    event_class: EventClass,
    /// Optional payload for parameterized events
    parameter: u64,
}
```

When an event arrives, the evaluator feeds it to each active rule's automaton, advancing the state machine. If any automaton reaches a rejecting state, the associated `BehaviorAction` is triggered (log, block, or suspend). Timed transitions implement MTL deadline semantics — if the automaton remains in a state with a timed transition beyond the deadline, it automatically transitions to the timeout state (typically a rejecting state).

### §9.2 Rule Examples

MTL rules express temporal behavioral constraints in a declarative notation. Each rule is shown in MTL formula syntax and plain English:

```text
# Rule 1: No bulk deletion without recent user confirmation
always (bulk_delete(count > 10) -> previously(user_confirm, 300s))
"Deleting more than 10 objects requires user confirmation within the last 5 minutes"

# Rule 2: Network sends after credential reads must target declared endpoints
always (credential_read -> next(network_send).target in declared_endpoints)
"After reading credentials, the next network send must go to a declared endpoint"

# Rule 3: Inference rate cannot exceed 3x baseline for more than 2 minutes
always (inference_rate > 3 * baseline -> eventually(inference_rate <= baseline, 120s))
"Inference rate spikes must return to baseline within 2 minutes"

# Rule 4: Space writes must be preceded by related space reads
always (space_write(X) -> previously(space_read(X), 30s) OR previously(space_read(parent(X)), 30s))
"Writing to a space requires having read from it or its parent within 30 seconds"

# Rule 5: No concurrent access to more than 3 spaces
always (concurrent_space_access <= 3)
"Agent cannot have more than 3 spaces open simultaneously"

# Rule 6: Sensitive data reads must be followed by local processing, not network send
always (sensitive_read -> not(next(network_send) until(local_process)))
"After reading sensitive data, the agent must process it locally before any network activity"

# Rule 7: Agent must not restart more than 3 times in 10 minutes
always (agent_restart -> count(agent_restart, 600s) <= 3)
"Rapid restart loops (potential retry-based attacks) are capped at 3 per 10-minute window"
```

Each formula compiles to an automaton with at most 64 states. Formulas exceeding this bound are rejected at install time — the complexity limit ensures bounded evaluation cost per event.

### §9.3 Rule Composition and Conflict Resolution

An agent's active rule set is the union of three rule sources, evaluated in parallel:

1. **System rules**: Defined by the AIOS kernel security policy. These encode fundamental invariants that apply to all agents (e.g., "no bulk deletion without user confirmation"). System rules cannot be overridden or relaxed by any other source.

2. **Enterprise policy rules**: Defined by organizational policy through the MDM/policy engine. These encode organization-specific constraints (e.g., "agents in the finance department cannot access personal spaces"). Enterprise rules can only be more restrictive than system rules.

3. **Agent manifest rules**: Declared by the agent developer in the agent's manifest. These encode agent-specific behavioral constraints (e.g., "this email agent should never access the calendar space"). Manifest rules can only be more restrictive than system and enterprise rules.

This layering enforces monotonic restriction — each layer can only narrow the set of permitted behaviors, never widen it.

```rust
pub fn compose_rules(
    system_rules: &[CompiledRule],
    policy_rules: &[CompiledRule],
    agent_rules: &[CompiledRule],
) -> Vec<CompiledRule> {
    // All rules are evaluated in parallel against each event.
    // A violation in ANY rule triggers the associated action.
    // If multiple rules trigger on the same event, the most
    // restrictive action wins (Suspend > Block > Log > Allow).
    let mut all_rules = Vec::with_capacity(
        system_rules.len() + policy_rules.len() + agent_rules.len(),
    );
    all_rules.extend_from_slice(system_rules);
    all_rules.extend_from_slice(policy_rules);
    all_rules.extend_from_slice(agent_rules);
    all_rules
}

pub fn most_restrictive_action(actions: &[BehaviorAction]) -> BehaviorAction {
    let mut worst = BehaviorAction::Allow;
    for action in actions {
        worst = match (&worst, action) {
            (_, BehaviorAction::Suspend { .. }) => action.clone(),
            (BehaviorAction::Suspend { .. }, _) => worst,
            (_, BehaviorAction::Block { .. }) => action.clone(),
            (BehaviorAction::Block { .. }, _) => worst,
            (_, BehaviorAction::Log { .. }) => action.clone(),
            (BehaviorAction::Log { .. }, _) => worst,
            _ => worst,
        };
    }
    worst
}
```

When an event triggers multiple rule violations simultaneously, the `most_restrictive_action` function selects the strongest enforcement response. The audit trail records all violated rules, not just the one that determined the final action — this provides complete forensic context for security investigations.

Rule conflicts between sources are resolved at install time, not at runtime:

- If an agent manifest rule contradicts a system rule by being less restrictive, the manifest rule is rejected during agent installation. AIRS flags the conflict and the agent developer must correct the manifest.
- If an enterprise policy rule contradicts a system rule, the system rule prevails. The policy engine logs a warning for the administrator.
- Duplicate rules across sources are deduplicated. If the same constraint appears in both system rules and agent manifest rules, only one automaton is instantiated.

---

## Cross-References

| Topic | Document | Section |
|---|---|---|
| VerificationResult, StructuredIntent types | [specification.md](./specification.md) | §3.2 |
| Verification pipeline flow | [pipeline.md](./pipeline.md) | §4 |
| Adversarial resistance and evasion defense | [security.md](./security.md) | §8 |
| Graceful degradation when AIRS unavailable | [security.md](./security.md) | §11 |
| Security layer model (8 layers) | [layers.md](../../security/model/layers.md) | §2 |
| Agent manifests and lifecycle | [agents.md](../../applications/agents.md) | All |
| AIRS intelligence services | [intelligence-services.md](../airs/intelligence-services.md) | §5.5 (Behavioral Monitor) |
| Capability system | [capabilities.md](../../security/model/capabilities.md) | §3 |
