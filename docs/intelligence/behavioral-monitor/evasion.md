# AIOS Behavioral Monitor — Evasion Resistance

Part of: [behavioral-monitor.md](../behavioral-monitor.md) — Behavioral Monitor Architecture
**Related:** [detection.md](./detection.md) — Detection algorithms, [security.md](./security.md) — Security layer integration, [intelligence.md](./intelligence.md) — AI-powered detection enhancements

-----

## §11. Evasion Resistance

The Behavioral Monitor must be designed with adversarial agents in mind. An attacker who controls or has compromised an agent will attempt to evade detection while achieving their objective (data exfiltration, resource abuse, privilege escalation). This section analyzes known evasion strategies and the defenses AIOS employs.

### §11.1 Threat Model for Monitor Evasion

**Attacker goals:**
1. Operate below detection thresholds while performing malicious actions
2. Manipulate baselines to make malicious behavior appear normal
3. Exploit warmup periods to establish inflated baselines
4. Time attacks to coincide with legitimate high-activity periods
5. Coordinate across multiple agents to stay individually below thresholds
6. Learn exact detection thresholds from enforcement feedback

**Attacker capabilities by trust level:**

| Trust Level | What the Attacker Can Do | What They Cannot Do |
|---|---|---|
| **Untrusted** | Execute code within agent sandbox, make IPC calls within capability set | Access kernel memory, modify behavioral state byte, forge capabilities |
| **Standard** | Same as Untrusted + spawn limited children | Modify hard limits, access other agents' baselines |
| **Trusted** | Same as Standard + wider capabilities | Modify kernel structures, disable monitoring |
| **Compromised AIRS** | Issue anomalous directives, manipulate Tier 2 analysis | Modify kernel-side monitors, forge provenance records |

**Key assumption:** The kernel is trusted. The attacker cannot modify kernel code, kernel data structures, or the provenance chain. If the kernel is compromised, all bets are off — behavioral monitoring is not the defense layer for kernel compromise (hardware security features like PAC, BTI, and MTE address that).

### §11.2 Gradual Ramp-Up Attack (Boiling Frog)

**Attack:** The agent slowly increases its activity rate over weeks to shift the baseline. Each day, activity increases by a small amount — below the z-score threshold. Over time, the baseline adapts to the higher rate via the decayed Welford update. When the baseline is sufficiently inflated, the attacker performs the actual malicious action within the "new normal."

**Example timeline:**
```text
Week 1: Agent reads 10 objects/min (normal baseline)
Week 2: Agent reads 12 objects/min (z = 0.8, below threshold)
Week 3: Agent reads 15 objects/min (z = 1.2, below threshold)
...
Week 8: Agent reads 50 objects/min (baseline has shifted, z = 1.5)
Week 9: Agent exfiltrates 50 objects/min (within "new normal")
```

**Defenses (layered):**

1. **Hard limits** — The most important defense. Hard limits are fixed absolute ceilings that cannot be shifted by baseline learning. Even if the baseline drifts to 50 objects/min, a hard limit of 300 reads/min prevents unbounded exfiltration. Hard limits are set conservatively based on trust level, not on observed behavior.

2. **Baseline drift rate capping** — The monitor tracks the rate of baseline change (see [profiling.md §8.5](./profiling.md)). If any metric's mean changes faster than the configured drift rate (default: 10% per week), the monitor flags `BaselineDrift` and triggers Tier 2 analysis. This detects gradual ramp-up at any speed above the drift rate cap.

3. **Dual-timescale detection** — The weekly baseline (7-day profile) captures long-term patterns that the daily baseline may miss. An agent that gradually increases its daily rate will eventually diverge from its weekly baseline, triggering a cross-timescale anomaly (see [detection.md §5.3](./detection.md)).

4. **Immutable historical baselines** — The provenance chain records baseline snapshots at enforcement events. Forensic analysis can compare current baselines against historical snapshots to detect drift patterns that occurred over the baseline's update window.

### §11.3 Warmup Period Exploitation

**Attack:** The attacker front-loads malicious behavior during the warmup period (first 7 days) to establish an inflated baseline. Since statistical detection is suppressed during warmup, the agent performs its malicious behavior freely, creating a baseline that considers the malicious behavior "normal."

**Example:**
```text
Day 1-7 (warmup): Agent reads 500 objects/min, sends 100 MB/min to network
Day 8+: Baseline shows mean = 500 reads/min, 100 MB/min (malicious = "normal")
```

**Defenses (layered):**

1. **Hard limits enforced during warmup** — Even though statistical detection is suppressed, hard limits apply from the first second of the agent's life. An Untrusted agent cannot read more than 60 objects/min regardless of warmup status. The inflated baseline never helps because hard limits are independent of baselines.

2. **Predicted behavior as prior** — If AIRS has performed behavioral prediction (Stage 3 of Agent Capability Intelligence), the predicted behavior serves as a prior for the baseline. An email agent predicted to read 10–50 objects/min will have its warmup observations compared against the prediction. A warmup reading of 500/min will diverge dramatically from the prediction, triggering a `HighDivergence` alert even during warmup (see [profiling.md §8.4](./profiling.md)).

3. **Trust-level-based maximum baseline values** — Even if an Untrusted agent's warmup observations are all at 59 reads/min (just below the hard limit), the baseline mean is capped at a trust-level maximum. For Untrusted agents, the baseline mean cap might be 30 reads/min — any observations above this contribute to variance but not to mean inflation.

4. **Post-warmup review** — At the end of the warmup period, the monitor performs a one-time review comparing the established baseline against trust-level norms and predicted behavior. If the baseline is unusually permissive (e.g., higher than 90th percentile of agents in the same category), the monitor logs a `SuspiciousWarmup` event for Tier 2 review.

### §11.4 Mimicry Attacks

**Attack:** The agent learns what "normal" looks like by observing enforcement feedback. When rate-limited at 100 reads/min, the agent learns the threshold is near 100 and operates at 95 reads/min — just below the detection boundary. The agent "mimics" normal behavior while maximizing its malicious throughput.

**Defenses (layered):**

1. **No threshold feedback** — The behavioral state byte communicates enforcement state (`Normal`, `Elevated`, `RateLimited`, `Suspended`) but not the exact threshold values, z-scores, or baseline parameters. An agent knows it has been rate-limited but not at what exact rate the limit was triggered.

2. **Randomized threshold jitter** — The effective anomaly threshold is jittered by ±10% per observation window (drawn from a uniform distribution). An agent that probes the boundary by incrementally increasing its rate cannot reliably determine the exact threshold because it shifts each window. The jitter is seeded per-agent from the kernel's entropy pool.

3. **Multi-dimensional detection** — Even if an agent perfectly mimics the rate of one action type, the Behavioral Monitor evaluates five independent dimensions (frequency, volume, temporal, target, sequence). Mimicking all five simultaneously while performing a useful attack is exponentially harder than mimicking one.

4. **Tier 2 semantic analysis** — When an agent consistently operates near its statistical boundaries (within 0.5σ of the threshold for extended periods), Tier 2 flags it for semantic review. AIRS evaluates whether the agent's behavior matches its declared purpose: "This weather agent is consistently using 95% of its allowed network bandwidth — is it fetching weather data or exfiltrating?"

### §11.5 Time-of-Check to Time-of-Use (TOCTOU)

**Attack:** The behavioral check occurs at IPC entry point (when the agent calls a syscall), but the actual action occurs later (when the kernel or service processes the request). The agent passes the behavioral check with a benign state, then somehow alters its request before execution.

**Why this attack fails in AIOS:**

1. **Behavioral state is persistent, not per-action** — The behavioral state byte reflects the agent's cumulative behavior, not a single action's characteristics. The state byte is set by AIRS based on aggregate analysis and persists across IPC calls. There is no per-action "check" that an individual action can pass or fail.

2. **IPC messages are immutable after send** — Once an agent sends an IPC message, the message is in kernel memory and cannot be modified by the agent. There is no window between "check" and "use" where the message content could change.

3. **Capability enforcement is separate** — Even if the behavioral check didn't exist, capability enforcement (Level 1 of the zero-trust stack) independently validates each action. TOCTOU on the behavioral state byte alone is insufficient to bypass other security layers.

### §11.6 Colluding Agents

**Attack:** Multiple agents coordinate to stay individually below detection thresholds while collectively achieving the attacker's goal. Agent A reads 30 objects/min (below its threshold), passes them via IPC to Agent B, which sends 30 MB/min to the network (below its threshold). Neither agent is individually anomalous, but together they exfiltrate data.

**Defenses (layered):**

1. **Cross-agent IPC monitoring** — The behavioral monitor tracks not just action rates but also inter-agent communication patterns. If Agent A and Agent B suddenly start communicating at an unusual rate or volume, the inter-agent channel itself triggers a target novelty or frequency spike anomaly.

2. **Aggregate space access monitoring** — The monitor tracks aggregate access patterns to sensitive spaces. If `personal/email/` is being read at 100 objects/min total (by any combination of agents), this triggers an aggregate alarm even if no individual agent exceeds its threshold. Aggregate monitoring is implemented as a separate `SpaceAccessMonitor` that tracks per-space rates across all agents.

3. **Tier 2 cross-agent behavioral correlation** (AIRS-dependent) — AIRS analyzes the temporal correlation between agents' activities. If Agent A's read bursts consistently precede Agent B's network bursts by 2 seconds, AIRS flags the correlation as a potential collusion pattern. This analysis uses cross-correlation functions on per-agent activity time series. See [intelligence.md §13.4](./intelligence.md).

4. **Capability delegation audit** — If Agent A delegates capabilities to Agent B (or vice versa), the delegation is recorded in the provenance chain. Unusual delegation patterns (agents that never communicated before suddenly sharing capabilities) trigger an anomaly.

**Limitations:** Cross-agent collusion detection is primarily a Tier 2 (AIRS-dependent) capability. Tier 1 kernel-internal detection can catch obvious collusion (sudden new IPC channels between agents) but cannot perform the sophisticated temporal correlation analysis needed to detect subtle coordination. On resource-constrained devices without AIRS, collusion detection relies on aggregate space access monitoring and capability delegation auditing.
