# AIOS Thermal Intelligence

Part of: [thermal.md](../thermal.md) — Thermal Management
**Related:** [zones.md](./zones.md) — Thermal zones & sensors, [cooling.md](./cooling.md) — Governors, [scheduling.md](./scheduling.md) — Thermal-aware scheduling

---

## §12 Kernel-Internal ML

Kernel-internal ML operates without AIRS dependency. These are lightweight, frozen models (decision trees, tiny neural networks) embedded in the kernel that enhance thermal decisions using purely statistical methods. They work even when AIRS is offline or not yet loaded, providing a baseline level of intelligent thermal management from the first moment the kernel boots.

All models are trained offline on thermal traces collected from each supported platform and shipped as static constants in the kernel binary. No runtime training occurs. This ensures predictable performance and eliminates the risk of in-kernel training destabilizing thermal control.

The kernel-internal ML layer complements the governor framework in cooling.md §5. Governors handle the control loop; kernel-internal ML handles model selection, predictive horizon extension, and anomaly seeding.

### §12.1 Decision Tree Governor Selection

A frozen decision tree selects the optimal governor for each thermal zone based on observed conditions at zone initialization and whenever zone topology changes (hotplug, new cooling device attachment).

**Input Features:**

- Zone type (CPU / GPU / NPU / SoC / Storage / Custom)
- Number of active cooling devices
- Energy model availability (yes / no)
- Current load pattern (steady / bursty / idle)
- Platform type identifier

**Output:** Recommended governor variant (step-wise, PID, bang-bang) as defined in cooling.md §5.

```rust
pub struct GovernorSelector {
    /// Pre-trained decision tree (serialized, read-only)
    tree: &'static DecisionTree,
}

impl GovernorSelector {
    pub fn recommend(&self, features: &GovernorFeatures) -> GovernorType {
        self.tree.classify(features)
    }
}

pub struct GovernorFeatures {
    pub zone_type: ThermalZoneType,
    pub cooling_device_count: u32,
    pub has_energy_model: bool,
    pub load_pattern: LoadPattern,
    pub platform: PlatformId,
}

pub enum LoadPattern {
    Steady,  // Sustained compute (inference, compilation)
    Bursty,  // Intermittent spikes (UI interaction, web browsing)
    Idle,    // Mostly idle with occasional wake
}
```

The decision tree is trained offline on thermal traces from each platform and shipped as a `static` in the kernel binary. No runtime training occurs. Tree depth is capped at 8 to bound worst-case classification time, which is always O(depth) regardless of feature values.

Reselection is triggered when:

- A new cooling device is attached (hotplug registration in cooling.md §4.5)
- Load pattern transitions persist for more than 30 seconds
- The current governor exceeds its thermal violation budget (see cooling.md §5.1)

### §12.2 Lightweight NN Thermal Prediction

Inspired by Physics-Informed Neural Networks (PINNs) from MDPI 2025, this tiny neural network predicts zone temperatures 10–60 seconds into the future, enabling proactive thermal management before trip points are reached.

**Architecture:**

- Input: 8 features — current temperature for up to 4 zones (padded with ambient if fewer zones active), plus 3 recent per-zone gradients in millidegrees C per second, plus CPU utilization as a fraction in `[0.0, 1.0]`
- Hidden: 2 fully connected layers × 16 neurons, ReLU activation
- Output: predicted temperatures for each zone at t+10 s, t+30 s, t+60 s
- Total parameters: approximately 800, fits in L1 cache on any supported platform

```rust
pub struct ThermalPredictor {
    weights_l1: [[f32; 8]; 16],   // input → hidden1
    bias_l1: [f32; 16],
    weights_l2: [[f32; 16]; 16],  // hidden1 → hidden2
    bias_l2: [f32; 16],
    weights_out: [[f32; 16]; 12], // hidden2 → output (4 zones × 3 horizons)
    bias_out: [f32; 12],
}

impl ThermalPredictor {
    /// Predict future temperatures for all zones.
    /// Returns temperatures in millidegrees Celsius.
    pub fn predict(&self, features: &[f32; 8]) -> ThermalForecast {
        let h1 = self.forward_relu(&self.weights_l1, &self.bias_l1, features);
        let h2 = self.forward_relu(&self.weights_l2, &self.bias_l2, &h1);
        let out = self.forward_linear(&self.weights_out, &self.bias_out, &h2);
        ThermalForecast::from_raw(&out)
    }
}

pub struct ThermalForecast {
    /// (t+10s, t+30s, t+60s) per zone, millidegrees Celsius
    pub predictions: [(i32, i32, i32); 4],
    /// Model confidence in [0.0, 1.0]; low confidence suppresses proactive action
    pub confidence: f32,
}
```

**Physics Constraints (PINNs):**

The training loss function includes a heat equation residual term to enforce physical plausibility:

```text
dT/dt = α∇²T + Q/(ρc)
```

Where `α` is thermal diffusivity, `Q` is heat generation rate, `ρ` is density, and `c` is specific heat capacity. This constraint:

- Prevents physically impossible predictions (e.g., a 50 000 millidegrees C jump in 10 seconds)
- Ensures predictions degrade gracefully when extrapolating beyond the training distribution
- Reduces prediction error variance at the 60-second horizon by approximately 30% vs. unconstrained training

**Governor Integration:**

The PID governor (cooling.md §5.3) and step-wise governor (cooling.md §5.2) both consume the forecast:

- If `predictions[zone].1 > passive_trip_mdegc` (t+30 s prediction exceeds Passive): begin proactive cooling now
- If `confidence < 0.4`: suppress proactive action, fall back to reactive control
- Reduces frequency of actually reaching trip points by approximately 40%, consistent with Linux IPA research findings

### §12.3 Control-Theoretic Core Idling (MPC)

Model Predictive Control for thermal regulation via core idling, inspired by work presented at RTSS 2021 on thermal-aware real-time scheduling.

Rather than reducing frequency uniformly, MPC selectively idles cores while keeping active cores at full clock speed. This trades parallelism for single-thread performance headroom and is particularly effective for bursty, latency-sensitive workloads.

```rust
pub struct MpcThermalController {
    /// Per-core thermal conductance to ambient (W per °C)
    conductance: [f32; MAX_CORES],
    /// Ambient temperature estimate, millidegrees Celsius
    ambient_mdegc: i32,
    /// Prediction horizon in control steps (1 step = 1 second)
    horizon: usize,
    /// Control horizon: number of steps for which a new mask is computed
    control_horizon: usize,
}
```

**Algorithm:**

1. At each control interval (1 second):
   - Read per-core temperatures from sensors (zones.md §3.2)
   - Predict temperatures for the next `horizon` steps given the current active core mask
   - Solve the optimization: minimize `Σ(predicted_temp - target_mdegc)²` subject to `active_cores ≥ min_active_cores`
   - Output: optimal active core mask for the next control interval
2. The thermal-aware scheduler (scheduling.md §6) applies the mask: threads are placed only on eligible cores
3. Idled cores are sent to the deepest available idle state by the power management layer (power-management.md §8)

**Comparison with pure DVFS:**

- Active cores run at their full rated frequency, avoiding the latency uncertainty of frequency scaling
- More predictable execution timing for real-time tasks
- Better for bursty workloads: full speed for short bursts, maximum thermal dissipation between bursts
- Combined with DVFS (not exclusive): MPC handles coarse control, DVFS handles fine-grained adjustment within each active core

### §12.4 Thermal Signature Fingerprinting

The system learns the characteristic thermal profile of common workloads over time. Each workload type — model inference, software compilation, video playback, sustained web browsing, idle — produces a repeatable thermal signature on a given hardware platform. Deviations from the expected signature indicate either a change in hardware health or an unusual workload pattern.

**Signature Schema:**

```rust
pub struct ThermalSignature {
    /// Hash of workload type and platform identifiers
    pub workload_hash: u64,
    /// Expected temperature rise rate under this workload, °C/s
    pub expected_gradient: f32,
    /// Expected steady-state temperature, millidegrees Celsius
    pub expected_steady_state: i32,
    /// Acceptable deviation from expected steady state, in millidegrees Celsius
    pub variance_mdegc: i32,
    /// Number of observations used to compute this signature
    pub sample_count: u32,
}
```

**Storage and Lifecycle:**

- Signatures are stored in the `system/thermal/signatures` space (spaces.md §3.1)
- Updated via exponential moving average: `new_value = α * observation + (1 - α) * stored_value`, with `α = 0.1`
- Maximum 256 signatures; LRU eviction removes the least recently observed workload signature
- Signatures persist across reboots, enabling long-term trend detection (§13.5)

**Anomaly Seeding:**

When a measured profile diverges from its stored signature by more than 3σ, kernel-internal fingerprinting raises a `SignatureDeviation` event. This event seeds the AIRS anomaly detector (§13.5) with the specific zone and deviation magnitude, allowing AIRS to investigate root cause with broader context.

---

## §13 AIRS Thermal Intelligence

AIRS-dependent thermal features require the AI Runtime Service (airs.md §2) to be operational. These features provide strategic, context-aware thermal intelligence that extends beyond what kernel-internal ML can achieve alone: workload-aware scheduling, reinforcement-learned DVFS policy, spatial multi-zone prediction, and coordinated power budget allocation.

AIRS thermal intelligence communicates with the kernel thermal subsystem through a dedicated capability-gated channel. The kernel never blocks on AIRS; if an AIRS advisory has not arrived by the next governor tick, the governor proceeds with its last known policy or falls back to kernel-internal ML.

### §13.1 AIRS Thermal Advisor

The Thermal Advisor is an AIRS sub-service that builds workload-thermal correlation models across agent activities and coordinates with the context engine (context-engine.md §3) to predict and defer thermally expensive work.

```rust
pub struct ThermalAdvisor {
    /// Per-agent thermal impact profiles, keyed by AgentId
    agent_profiles: HashMap<AgentId, AgentThermalProfile>,
    /// Ambient temperature estimator (exponential averaging of idle steady state)
    ambient_estimator: AmbientEstimator,
    /// Deferred tasks awaiting a thermal window
    deferred_queue: Vec<DeferredThermalTask>,
}

pub struct AgentThermalProfile {
    pub agent_id: AgentId,
    /// Average temperature increase during this agent's activity, millidegrees Celsius
    pub avg_temp_impact_mdegc: i32,
    /// Worst-case temperature increase observed, millidegrees Celsius
    pub peak_temp_impact_mdegc: i32,
    /// Typical duration of the thermal impact, seconds
    pub typical_duration_s: u32,
}

pub struct DeferredThermalTask {
    pub agent_id: AgentId,
    pub task_id: u64,
    /// Latest acceptable start time (deadline), Unix seconds
    pub deadline_s: u64,
    /// Estimated thermal budget required, milliwatts
    pub budget_mw: u32,
}
```

**Scheduling Policy:**

- AIRS observes which agent activities correlate with temperature spikes by correlating kernel thermal events with agent task logs
- Tasks flagged as thermally expensive with sufficient deadline slack are deferred to thermal windows: low-ambient periods, device idle periods, or times when the user is away (context-engine.md §4.2)
- The deferred queue is drained when all zones are below their Passive trip points and no foreground activity is detected
- Tasks past 80% of their deadline are promoted to immediate regardless of thermal state

### §13.2 DRL-Based DVFS Optimization

Inspired by FiDRL (IEEE TC 2024), this component applies deep reinforcement learning to DVFS control while accounting for the overhead of invoking the DRL agent itself when making frequency decisions.

**Key Innovation:** Naive DRL DVFS loops can thrash frequency levels if the control interval is shorter than the combined latency of inference + frequency transition. FiDRL-style agents measure their own invocation cost and enforce a minimum stable interval.

```rust
pub struct DrlDvfsAgent {
    /// Frozen policy network; outputs frequency level indices
    policy: FrozenPolicy,
    /// Measured invocation overhead, microseconds
    invocation_cost_us: u32,
    /// Minimum interval between frequency decisions
    min_interval_ms: u32,
}
```

**State Space:**

- Current zone temperatures, millidegrees Celsius (all zones from zones.md §3)
- Current frequency level indices for all DVFS domains
- Per-core CPU utilization over the last 5 control intervals
- Active AIRS agent count and priority class distribution

**Action Space:**

- Discrete frequency level selection for each DVFS domain (as enumerated in cooling.md §4.2)
- Active core mask recommendation forwarded to the MPC controller (§12.3)

**Reward Function:**

```text
R = -α × thermal_violation - β × perf_loss + γ × energy_saved
```

Where:

- `thermal_violation` = integral of (temperature above Passive trip) × (degrees above trip), in millidegrees·seconds
- `perf_loss` = throughput reduction compared to maximum frequency, as a fraction in `[0.0, 1.0]`
- `energy_saved` = energy reduction compared to maximum frequency, joules
- Coefficients `α`, `β`, `γ` are tuned per platform to balance thermal safety vs. performance vs. efficiency

**Deployment Constraints:**

- Policy network is frozen after platform-specific fine-tuning; no online weight updates in production
- AIRS runs the policy inference on the NPU where available, reducing CPU thermal load from the control loop itself
- If inference latency exceeds `min_interval_ms / 2`, AIRS automatically falls back to the PID governor (cooling.md §5.3)

### §13.3 GNN Spatial Thermal Prediction

A Graph Neural Network models thermal coupling between zones explicitly, enabling accurate multi-zone temperature prediction for platforms with complex die topologies such as Apple Silicon or multi-chiplet server processors.

**Graph Structure:**

- Nodes: thermal zones (CPU cores, GPU, NPU, DRAM, skin) as defined in zones.md §3.1
- Edges: thermal coupling coefficients between adjacent zones (zones.md §3.4), weighted by physical proximity and thermal resistance
- Message passing: each node aggregates heat transfer contributions from neighbors, updates its hidden state, and outputs a temperature prediction

**Architecture vs. Simple NN (§12.2):**

- §12.2 treats zones independently with gradient features; GNN explicitly represents spatial coupling
- GNN handles arbitrary zone topologies without retraining — only the graph structure changes per platform
- Accurate for multi-zone platforms with 4+ zones where zone interactions dominate prediction error
- Prediction error targets: less than 1 000 millidegrees C at 1-minute horizon, less than 3 000 millidegrees C at 5-minute horizon

**Inference Schedule:**

- AIRS runs GNN inference every 30 seconds
- Output feeds the MPC controller (§12.3) as a long-horizon temperature trajectory
- Also feeds the PID governor (cooling.md §5.3) as a feedforward term, reducing overshoot

**Model Size:**

- Node embedding dimension: 16
- Message passing rounds: 3
- Total parameters: approximately 8 000 (32 KB float32)
- Inference time on Cortex-A72 at 1.5 GHz: approximately 2 ms; negligible on NPU

### §13.4 Multi-Agent RL Power Budget Coordination

Inspired by ARDF (Computing Journal 2025), this component applies adversarial reinforcement learning to jointly optimize DVFS decisions and cooling device actuation across all AIRS sub-agents under a shared power budget constraint.

**Concept:**

Multiple AIRS sub-agents bid for power budget based on their current priority and deadline urgency. A centralized thermal allocator solves the allocation problem using a policy trained with safe RL constraints (Lagrangian relaxation) to ensure the total allocated power never exceeds the Thermal Sustainable Power (TSP) envelope.

```rust
pub struct ThermalBudgetAllocator {
    /// Per-agent power bids for the upcoming control interval
    bids: Vec<PowerBid>,
    /// Total sustainable power budget, milliwatts
    total_budget_mw: u32,
    /// Lagrangian safety multiplier; increases when budget is violated
    lambda: f32,
}

pub struct PowerBid {
    pub agent_id: AgentId,
    /// Power requested for the upcoming interval, milliwatts
    pub requested_mw: u32,
    /// Agent priority derived from scheduler class (scheduling.md §6.2)
    pub priority: f32,
    /// Time until agent deadline, milliseconds; lower slack → higher urgency
    pub deadline_slack_ms: u32,
}
```

**Allocation Algorithm:**

1. Each active AIRS agent submits a `PowerBid` at the start of each control interval (1 second)
2. Allocator sorts bids by urgency: `urgency = priority / max(deadline_slack_ms, 1)`
3. Greedy allocation with Lagrangian penalty: agents with higher urgency receive budget first, subject to `Σ allocated_mw ≤ total_budget_mw × (1 - λ_margin)`
4. `λ` is updated online: increases after budget violations, decreases after safe intervals

**Adversarial Training:**

- One agent role attempts to maximize task throughput; the opposing role attempts to minimize thermal violations
- Nash equilibrium of the adversarial game produces a robust allocation policy resilient to both benign and adversarial workload distributions
- Benchmarks consistent with ARDF show approximately 7% energy improvement over static thermal power capping, with zero increase in thermal violations

**Safety Guarantee:**

The Lagrangian constraint is enforced in the kernel, not in AIRS. Even if AIRS submits allocations that sum beyond `total_budget_mw`, the kernel thermal subsystem clips each allocation proportionally before forwarding to DVFS and cooling devices. AIRS never has the ability to exceed the hardware-derived TSP limit.

### §13.5 Thermal Anomaly Detection

AIRS detects cooling system degradation, thermal interface material aging, and environmental changes by correlating observations from kernel-internal fingerprinting (§12.4) with long-term trend analysis and cross-zone consistency checks.

**Detection Methods:**

1. **Signature deviation** (seeded by §12.4): measured steady-state temperature for a known workload exceeds the stored signature by more than 3σ across at least 3 consecutive observations
2. **Degradation trend**: rolling 7-day linear regression of idle steady-state temperature shows an increase exceeding 5 000 millidegrees C, indicating progressive cooling efficiency loss (thermal paste drying, fan bearing wear)
3. **Cross-zone inconsistency**: one zone diverges from the GNN-predicted temperature (§13.3) by more than 4 000 millidegrees C while adjacent zones remain within prediction bounds, suggesting a localized sensor fault or blocked vent
4. **Recovery time regression**: the time for a zone to return from Passive trip to Normal increases by more than 20% compared to the baseline profile, indicating reduced cooling capacity

```rust
pub enum ThermalAnomaly {
    SignatureDeviation {
        zone: &'static str,
        /// Expected steady-state temperature, millidegrees Celsius
        expected_mdegc: i32,
        /// Measured steady-state temperature, millidegrees Celsius
        measured_mdegc: i32,
    },
    DegradationTrend {
        zone: &'static str,
        /// Temperature increase rate, millidegrees Celsius per day
        rate_mdegc_per_day: i32,
    },
    CrossZoneInconsistency {
        zone_a: &'static str,
        zone_b: &'static str,
    },
    SlowRecovery {
        zone: &'static str,
        expected_recovery_s: u32,
        measured_recovery_s: u32,
    },
}
```

**Response Actions:**

- User notification with a plain-language diagnosis: "CPU cooling efficiency has declined — thermal interface material may need replacement" or "Fan speed is lower than expected for current temperature"
- Conservative trip point adjustment: lower the Passive trip for affected zones by 5 000 millidegrees C as a safety margin until the anomaly is resolved
- Audit log entry written to `system/audit/thermal/anomalies` (security.md §11.3) with full diagnostic context
- If `SlowRecovery` persists and `measured_recovery_s > 2 × expected_recovery_s`, escalate to Critical alert and engage maximum cooling regardless of user preference

**Fleet Analysis:**

Anomaly records written to `system/audit/thermal/anomalies` are available to AIRS context engine (context-engine.md §5) for cross-device fleet analysis. Patterns observable across many devices (e.g., a specific platform model uniformly developing `DegradationTrend` after 18 months) can inform predictive maintenance scheduling.

### §13.6 Future Directions

The following capabilities are under research consideration for future AIOS thermal intelligence phases. None are committed to a specific phase schedule.

**Liquid cooling integration:** Support for liquid-cooled workstation and server platforms with coolant flow rate sensors, coolant inlet/outlet temperature sensors, and pump speed actuators. The governor framework (cooling.md §5) is extensible to new `CoolingDevice` types; liquid cooling adds a `PumpSpeed` variant. GNN spatial prediction (§13.3) would include coolant loop nodes.

**Chiplet thermal management:** Next-generation processors expose per-chiplet power and temperature telemetry over PCIe DVSEC or proprietary management buses. Each chiplet would register as a distinct thermal zone (zones.md §3.1) with explicit coupling edges to the package and substrate. The GNN (§13.3) naturally handles this topology extension.

**Cross-device thermal coordination:** When multiple AIOS devices are physically co-located — a laptop and a phone on the same desk, for example — ambient heat from one device raises the ambient temperature sensed by the other. AIRS peer protocol (networking/protocols.md §5.1) could carry thermal ambient advisories between devices, allowing coordinated workload scheduling that avoids simultaneous peak thermal loads.

**Thermal digital twin:** A full 3D finite-element thermal simulation of the device, calibrated in real time from sensor readings, would enable sub-millimeter thermal hotspot detection and prediction. The digital twin would run on the NPU between inference tasks, with AIRS managing its scheduling priority relative to user workloads.

**Formal thermal safety proofs:** Extending the model checking approach used for security properties (security.md §11.4) to cover the full thermal control stack — governors, ML predictors, MPC controller, and AIRS Advisor — would provide machine-verified safety guarantees for all reachable thermal states. Initial scope: prove that the step-wise governor (cooling.md §5.2) combined with MPC core idling (§12.3) never allows a Critical trip-point violation given bounded sensor latency.

See power-management.md §14 for related future directions covering system-wide power policy. See intelligence/airs.md §5 for the AIRS architecture that hosts the sub-services described in this section.
