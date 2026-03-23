# AIOS Inspector — Threat Model & Security

Part of: [inspector.md](../inspector.md) — Inspector Architecture
**Related:** [architecture.md](./architecture.md) — Architecture, [intelligence.md](./intelligence.md) — Intelligence, [testing.md](./testing.md) — Testing

-----

## 10. Threat Model

The Inspector is the user's primary security visibility tool. An attacker who can degrade, confuse, or blind the Inspector gains a significant advantage: the user loses situational awareness. This section addresses attacks specifically targeting the Inspector itself, distinct from attacks on agents or the kernel.

### 10.1 Provenance Spoofing

**Threat:** A compromised agent injects false provenance records to frame another agent, hide its own actions, or create confusion about the timeline of events.

**Why this matters:** If the provenance chain contains attacker-controlled data, the Inspector's timeline and forensic views become unreliable. A user investigating an incident could be misled by fabricated records.

**Mitigation:**

The provenance chain is a kernel-managed, append-only Merkle-linked data structure ([layers.md](../../security/model/layers.md) §2.7). Agents have no write path to it. Every provenance record is:

1. **Created by the kernel.** The kernel appends records as a side effect of syscall processing. There is no `ProvenanceWrite` syscall. Agents cannot request that a record be added.
2. **Merkle-chained.** Each record's `prev_hash` links to the previous record's `record_hash`. Inserting, removing, or modifying any record breaks the chain.
3. **Kernel-signed.** Each record carries an Ed25519 signature from the kernel's signing key, loaded from secure storage at boot. Forging a signature requires the kernel's private key.

```rust
/// Inspector chain verification on display
pub fn verify_and_render(chain: &[ProvenanceRecord]) -> VerificationResult {
    let mut expected_prev = Hash::zero(); // genesis
    let mut broken_links: Vec<RecordId> = Vec::new();
    let mut bad_signatures: Vec<RecordId> = Vec::new();

    for record in chain {
        // Verify Merkle link
        if record.prev_hash != expected_prev {
            broken_links.push(record.id);
        }
        // Verify record hash matches contents
        let computed = sha256_fields(record);
        if record.record_hash != computed {
            broken_links.push(record.id);
        }
        // Verify kernel signature
        if !kernel_pubkey.verify(&record.record_hash, &record.signature) {
            bad_signatures.push(record.id);
        }
        expected_prev = record.record_hash;
    }

    VerificationResult {
        chain_intact: broken_links.is_empty() && bad_signatures.is_empty(),
        broken_links,
        bad_signatures,
    }
}
```

**Inspector behavior:** When displaying provenance records, the Inspector verifies chain integrity inline. A broken link or invalid signature triggers an immediate critical alert (auto-open, full-screen) because chain corruption implies either kernel compromise or storage corruption — both require investigation.

### 10.2 Audit Flooding

**Threat:** A malicious agent generates a massive volume of legitimate (or borderline) actions to flood the provenance chain, overwhelming the Inspector's display and burying real attacks in noise.

**Attack variants:**

- **Volume flood:** Agent performs thousands of reads per second, each individually legitimate, producing a torrent of provenance records that push real security events off the Inspector's visible timeline.
- **Alert fatigue:** Agent generates many low-severity security events (e.g., repeatedly attempting and failing a borderline capability check) to desensitize the user to alerts.
- **Dilution:** Coordinated agents each generate moderate traffic — no single agent triggers rate limits, but the aggregate buries a third compromised agent's exfiltration.

**Mitigation (three layers):**

| Defense Layer | Mechanism | Effect |
|---|---|---|
| Kernel rate limiting | `HardLimits` per agent ([layers.md](../../security/model/layers.md) §2.3) | Caps provenance record generation rate at the source |
| AIRS pre-triage | Semantic clustering and severity scoring | Groups related events, surfaces high-severity items above noise |
| Kernel-internal ML | Burst pattern detection (decision tree, < 50 us) | Identifies coordinated flooding across agents |

```rust
/// Kernel-side rate limiter for provenance generation
pub struct ProvenanceRateLimiter {
    /// Per-agent sliding window counters
    per_agent: HashMap<AgentId, SlidingWindow>,
    /// Maximum provenance records per agent per minute
    max_records_per_minute: u32,    // default: 500
    /// Alert threshold: agent generating > N records/min triggers monitoring
    alert_threshold: u32,           // default: 200
}

/// Inspector-side flood mitigation
pub struct FloodMitigation {
    /// AIRS semantic clustering groups related events
    /// (DEMIST-2 inspired event correlation)
    cluster_engine: EventClusterEngine,
    /// High-severity events always surface above noise
    severity_filter: SeverityPrioritizer,
    /// Burst detection: flag agents with sudden activity spikes
    burst_detector: BurstDetector,
}
```

**Inspector behavior:** When flood conditions are detected, the Inspector switches to a **triage view** that groups events by semantic cluster rather than raw chronological order. A banner informs the user: "High event volume detected — showing grouped view. N events from [agent] suppressed." The user can expand suppressed groups on demand.

### 10.3 UI Confusion Attacks

**Threat:** An agent renders a surface that visually mimics the Inspector's UI — fake security alerts, fake "all clear" dashboards, or fake capability prompts — to deceive the user into trusting a malicious agent or ignoring real threats.

**Why this matters:** If users cannot distinguish the real Inspector from an imitation, an attacker can present a clean security posture while actively exfiltrating data.

**Mitigation: Compositor-enforced trust borders.**

AIOS assigns each agent a trust level ([model.md](../../security/model.md) §1.2). The compositor renders a **trust-level border** around every agent's surface that the agent itself cannot control:

| Trust Level | Border Color | Agents |
|---|---|---|
| 0 | None (kernel only) | Kernel |
| 1 | Gold | Compositor, service manager |
| 2 | Blue | Inspector, Settings, system experience agents |
| 3 | Green | Third-party agents (sandboxed) |
| 4 | Red | Untrusted / ephemeral agents |

```rust
/// Compositor-rendered trust border (agent cannot modify)
pub struct TrustBorder {
    /// Trust level determines color — set by compositor from agent manifest
    trust_level: TrustLevel,
    /// Border width in logical pixels (scaled by display DPI)
    width: u32,                     // 3px at 1x, 6px at 2x
    /// Rendered by compositor AFTER agent surface compositing
    /// Agent's framebuffer never overlaps the border region
    render_order: RenderPhase::PostComposite,
}
```

**Key properties (Qubes OS inspired):**

1. **Compositor-rendered.** The trust border is drawn by the compositor after the agent's surface is composited. The agent's framebuffer is clipped to exclude the border region. An agent cannot draw over, hide, or modify its own border.
2. **Color is derived from trust level.** A Trust Level 3 agent always gets a green border. It cannot request or fake a blue border (Trust Level 2). The compositor reads the trust level from the kernel's agent registry, not from the agent.
3. **Always visible.** The border persists even in fullscreen mode. A fullscreen third-party agent shows a thin green border at the screen edges.
4. **Inspector is always blue.** Users learn to look for the blue border when verifying they are interacting with the real Inspector.

**Inspector behavior:** If a user opens what they believe is the Inspector but the surface has a green or red border, the compositor's trust coloring immediately reveals the deception. No action is required from the Inspector itself — the compositor prevents confusion at the rendering level.

### 10.4 Compromised Inspector Agent

**Threat:** The Inspector agent itself is compromised — through a supply chain attack on its signed binary, a vulnerability in its code, or a hostile build environment.

**Why this matters:** A compromised Inspector could suppress alerts, hide provenance records from the user, or present a false security posture. It is the user's primary security visibility tool — compromising it blinds the user.

**Mitigation (defense in depth):**

```text
Boot chain verification:
  Firmware → UEFI Secure Boot → Kernel image → System agent signatures
                                                     ↓
                                              Inspector binary
                                              (signed by AIOS root key,
                                               verified at load time)
```

1. **Signed by AIOS root key.** The Inspector binary is signed during the AIOS build process. The kernel verifies the signature against the root public key (embedded in the kernel image) before loading the Inspector agent. A modified binary fails verification and is not loaded. Cross-reference: [secure-boot.md](../../security/secure-boot.md) §3.

2. **Measured boot.** The Inspector's binary hash is recorded in the measured boot log. Remote attestation can verify that the loaded Inspector matches the expected hash. Cross-reference: [trust-chain.md](../../security/secure-boot/trust-chain.md) §3.7.

3. **Read-only for provenance.** Even a compromised Inspector cannot alter the provenance chain. It holds `AuditRead` capability and is not granted any capability that would allow writing or mutating kernel-created provenance records. `AuditWrite` exists for emitting audit log events but cannot rewrite existing provenance entries. The kernel exposes no syscall for modifying existing provenance records. The worst a compromised Inspector can do is **suppress display** — not alter the underlying data.

4. **Bounded capabilities.** The Inspector's capability set is fixed in its manifest (§3 of [inspector.md](../inspector.md)). It can read audit data, read capability tables, revoke capabilities, and pause/resume agents. It cannot grant new capabilities, create agents, write to agent spaces, or modify the provenance chain.

5. **Redundant detection paths.** A compromised Inspector that suppresses alerts does not prevent:
   - The kernel from enforcing capability checks (Layer 2 operates independently)
   - The behavioral monitor from detecting anomalies (Layer 3 operates independently)
   - AIRS from pausing agents on critical anomalies (AIRS acts directly, not through Inspector)
   - The Conversation Bar from reporting security events when queried by the user

```rust
/// What a compromised Inspector CAN do (damage ceiling)
pub enum CompromisedInspectorCapability {
    /// Suppress alert display — user does not see notifications
    SuppressDisplay,
    /// Show false "all clear" — user believes system is secure
    FalsePositive,
    /// Misrepresent provenance — show wrong agent for an action
    MisattributeProvenance,
}

/// What a compromised Inspector CANNOT do (kernel-enforced)
pub enum CompromisedInspectorLimit {
    /// Cannot modify provenance chain (no write syscall exists)
    CannotAlterProvenance,
    /// Cannot grant capabilities (no CapabilityGrant in manifest)
    CannotGrantCapabilities,
    /// Cannot create agents (no AgentCreate in manifest)
    CannotCreateAgents,
    /// Cannot write to agent spaces (no SpaceWrite in manifest)
    CannotWriteSpaces,
    /// Cannot disable kernel enforcement (capability checks continue)
    CannotBypassKernel,
}
```

**Recovery:** If Inspector compromise is suspected, the user can verify the Inspector's binary hash via the Conversation Bar ("verify Inspector integrity") or by booting into recovery mode. The kernel can re-verify the Inspector's signature on demand using the same secure-boot / measured-boot verification path it uses at startup, without requiring a dedicated syscall.

### 10.5 Event Overwhelming

**Threat:** Multiple agents coordinate to generate correlated events that individually appear normal but collectively create a false narrative — either masking a real attack (false negative) or triggering so many alerts that the user dismisses a real one (alert fatigue).

**Attack example:** Three agents each generate 50% of their baseline activity, staying within behavioral norms. Meanwhile, a fourth compromised agent performs slow exfiltration at 10% of its normal volume — well below the anomaly threshold. The noise from the three cooperating agents consumes the user's attention budget.

**Mitigation (three detection layers):**

```text
Layer 1: Kernel-internal temporal pattern detection
  - Detects synchronized activity bursts across agents
  - Decision tree classifier, < 50 us per evaluation
  - Flags: "3 agents increased activity within 30s window"

Layer 2: AIRS semantic clustering
  - Groups events by causal relationships, not just timestamps
  - Identifies events that are semantically correlated
    even if temporally separated
  - Accuracy target: ~94% (DEMIST-2 inspired clustering)

Layer 3: Behavioral monitor cross-agent correlation
  - Tracks inter-agent communication patterns
  - Detects coordinated baseline shifts across agent groups
  - Identifies covering patterns (noise + exfiltration)
```

```rust
/// Cross-agent correlation analysis
pub struct CrossAgentCorrelator {
    /// Sliding window of per-agent activity rates
    activity_windows: HashMap<AgentId, SlidingWindow>,
    /// Correlation matrix: agents with synchronized activity patterns
    correlation_matrix: CorrelationMatrix,
    /// Threshold for flagging coordinated behavior
    coordination_threshold: f64,     // default: 0.85 Pearson correlation
    /// Minimum number of agents to consider coordination
    min_coordinated_agents: usize,   // default: 2
}

pub struct CorrelationAlert {
    /// Agents with correlated activity patterns
    correlated_agents: Vec<AgentId>,
    /// Correlation coefficient (0.0 = independent, 1.0 = synchronized)
    correlation: f64,
    /// Time window over which correlation was observed
    window: Duration,
    /// Suggested focus: which other agents were active during this window
    /// but NOT correlated (potential targets of cover)
    uncorrelated_active: Vec<AgentId>,
}
```

**Inspector behavior:** When cross-agent correlation is detected, the Inspector displays a **coordination alert** that shows:

1. Which agents are acting in concert
2. The correlation coefficient and time window
3. Other agents that were active during the same window but **not** correlated — these are potential targets being masked by the coordinated noise
4. A recommendation: "Review uncorrelated agents for unusual low-volume activity"

Cross-reference: [behavioral-monitor.md](../../intelligence/behavioral-monitor.md) §11 for evasion resistance, [adversarial-defense.md](../../security/adversarial-defense.md) §2 for threat taxonomy.

-----

## 11. Security Layer Positioning

The Inspector reads from all security layers (0–8) defined in [layers.md](../../security/model/layers.md). It does not enforce any of them — enforcement is the kernel's responsibility. The Inspector provides visibility into what each layer is doing.

### 11.1 How Inspector Reads from Each Layer

| Layer | Name | What Inspector Reads | Syscall | Inspector View |
|---|---|---|---|---|
| 0 | Hardware Root of Trust | Hardware security state, secure boot measurements, trust anchor status | `AuditRead(filter: hardware)` | Hardware View |
| 1 | Intent Verification | DeclaredIntent vs observed behavior gaps, verification confidence scores | `AuditRead(filter: intent)` | Agent View (anomaly section) |
| 2 | Capability Check | Active capability tokens, grant/revoke history, denial events | `CapabilityQuery` | Capability View |
| 3 | Behavioral Boundary | Anomaly scores, behavioral baselines, z-score deviations, hard limit violations | `AgentQuery` | Agent View (baseline section) |
| 4 | Security Zone | Zone assignments, cross-zone access attempts, zone promotion events | `AuditRead(filter: zone)` | Security Events View |
| 5 | Adversarial Defense | Injection attempt logs, input/output screening results, hint screening | `AuditRead(filter: adversarial)` | Security Events View |
| 6 | Cryptographic Enforcement | Encryption zone status, key rotation events, decryption failures | `AuditRead(filter: crypto)` | Hardware View |
| 7 | Provenance Recording | Full Merkle chain, resource directives, chain integrity status | `AuditRead` | Provenance View |
| 8 | Blast Radius Containment | Write/delete budget usage, auto-snapshot triggers, rate limit activations | `AuditRead(filter: blast_radius)` | Agent View (limits section) |

**Data flow:** Each layer writes provenance records as actions pass through it. The Inspector reads these records via the `AuditRead` syscall with layer-specific filters. All reads are non-blocking and use the same `AuditQuery` API defined in [layers.md](../../security/model/layers.md) §2.7.

### 11.2 Provenance Integrity

The Inspector performs chain integrity verification as both a background task and an on-demand user action.

**Verification algorithm:**

```rust
/// Walk the Merkle chain backwards from head, verify each link
pub fn verify_chain_integrity(
    chain: &[ProvenanceRecord],
    kernel_pubkey: &Ed25519PublicKey,
) -> ChainIntegrityReport {
    let mut report = ChainIntegrityReport::new();

    // Walk forward from genesis
    let mut expected_prev = Hash::zero();
    for (i, record) in chain.iter().enumerate() {
        // 1. Verify Merkle link continuity
        if record.prev_hash != expected_prev {
            report.add_gap(i, expected_prev, record.prev_hash);
        }

        // 2. Verify record hash matches serialized fields
        let computed_hash = sha256(
            &record.agent_id, &record.action, &record.target,
            &record.timestamp, &record.result,
            &record.capability_used, &record.prev_hash,
        );
        if record.record_hash != computed_hash {
            report.add_tampered_record(i, computed_hash, record.record_hash);
        }

        // 3. Verify kernel signature
        if !kernel_pubkey.verify(&record.record_hash, &record.signature) {
            report.add_invalid_signature(i);
        }

        // 4. Verify timestamp monotonicity
        if i > 0 && record.timestamp < chain[i - 1].timestamp {
            report.add_timestamp_violation(i);
        }

        // 5. Verify sequence number continuity
        if i > 0 && record.id.sequence() != chain[i - 1].id.sequence() + 1 {
            report.add_sequence_gap(i);
        }

        expected_prev = record.record_hash;
    }

    report
}
```

**Tiered retention handling:**

The provenance chain uses three retention tiers ([layers.md](../../security/model/layers.md) §2.7.2):

| Tier | Window | Detail Level | Integrity Guarantee |
|---|---|---|---|
| Hot (Full) | 7 days | Every field of every record | Full Merkle chain verification |
| Warm (Summarized) | 90 days | Hourly aggregates per agent, chain hashes preserved | Hash chain verification (individual records gone) |
| Cold (Hash-only) | Indefinite | Hash anchors only | Anchor chain verification (proves nothing deleted) |

The Inspector verifies integrity at each tier boundary. At the hot-warm transition, it confirms that the summary records' hash anchors match the last full record's hash in the preceding hot window. At the warm-cold transition, it confirms anchor continuity.

**Consistency checks performed:**

- **Gap detection:** Missing sequence numbers indicate deleted records
- **Timestamp monotonicity:** Non-increasing timestamps indicate reordering or injection
- **Sequence number continuity:** Gaps in sequence numbers indicate deletion
- **Cross-tier anchor matching:** Tier boundary hashes must chain correctly
- **Signature validity:** Every record (or summary) must carry a valid kernel signature

### 11.3 Trust Model

The Inspector operates as a Trust Level 2 agent — elevated but bounded. It has no kernel backdoors, no privileged syscalls, and no capability that other Trust Level 2 agents could not theoretically hold. Its special status comes from being system-shipped and AIOS-root-key-signed, which allows the kernel to grant it broad `Scope::All` capabilities at boot.

**What Inspector CAN do:**

| Action | Capability | Scope |
|---|---|---|
| Read all audit/provenance data | `AuditRead` | `Scope::All` |
| Read all capability tables | `CapabilityQuery` | `Scope::All` |
| Revoke any capability token | `CapabilityRevoke` | `Scope::All` |
| Read agent metadata and baselines | `AgentQuery` | `Scope::All` |
| Pause or resume any agent | `AgentControl` | `Scope::All` |
| Read capability profiles | `ProfileRead` | `Scope::All` |
| Manage user override profiles | `ProfileWrite` | `Scope::User` (Layer 90 only) |
| Read AIRS security analysis | `InferenceQuery` | `Scope::SecurityAnalysis` |

**What Inspector CANNOT do:**

| Prohibited Action | Why |
|---|---|
| Grant new capabilities | No `CapabilityGrant` in manifest |
| Create agents | No `AgentCreate` in manifest |
| Write to agent spaces | No `SpaceWrite` in manifest |
| Modify provenance chain | No write syscall exists for provenance |
| Bypass kernel enforcement | Capability checks are in-kernel; no agent can skip them |
| Read raw kernel memory | No `MemoryRead` capability; runs in user address space |
| Escalate its own trust level | Trust level is set by kernel from signed manifest |

**Trust boundary:**

The Inspector trusts two things:

1. **The kernel's provenance chain.** If the kernel is compromised, provenance records may be false — but so is everything else. A compromised kernel can lie to all agents, not just the Inspector.
2. **The kernel's capability table.** The Inspector reads capability tables via syscall. If the kernel returns false data, the Inspector's view is wrong — but again, kernel compromise breaks all security guarantees, not just the Inspector's.

This is an intentional design choice: the Inspector does not attempt to verify kernel integrity from user space. Kernel integrity is ensured by secure boot ([trust-chain.md](../../security/secure-boot/trust-chain.md)) and measured boot attestation. The Inspector's role is to present kernel-provided data honestly, not to second-guess the kernel.

```text
Trust hierarchy:

  Hardware root of trust
       ↓ (measured boot)
  Kernel (trusted computing base)
       ↓ (signed provenance, capability tables)
  Inspector (Trust Level 2)
       ↓ (visual presentation)
  User (makes decisions based on Inspector's display)

If the kernel is compromised, the Inspector's view is unreliable.
But if the kernel is compromised, ALL agents' views are unreliable.
The Inspector does not add a new trust dependency — it depends on
the same kernel that every other agent depends on.
```

Cross-reference: [model.md](../../security/model.md) §1.2 for trust level definitions, [capabilities.md](../../security/model/capabilities.md) for capability token lifecycle, [operations.md](../../security/model/operations.md) for security event response.
