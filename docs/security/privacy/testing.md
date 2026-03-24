# AIOS Privacy Testing & Verification

Part of: [privacy.md](../privacy.md) — Privacy Architecture
**Related:** [intelligence.md](./intelligence.md) — Privacy intelligence, [data-lifecycle.md](./data-lifecycle.md) — Data lifecycle privacy

---

## §13 Testing & Verification

Privacy properties must be continuously verified — a privacy guarantee that worked in Phase N can be broken by a change in Phase N+1. The testing strategy covers property-based verification, regression testing, and structured adversarial (red-team) testing.

### §13.1 Privacy Property Testing

Each privacy pillar has **invariant properties** that must hold across all system states. These properties are tested through a combination of unit tests, property-based testing (randomized input generation), and integration tests.

**Agent privacy properties:**

| Property | Test Method | Verification |
|---|---|---|
| Manifest enforcement | Property-based: generate random agent+data flows, verify all undeclared flows are blocked | No agent reads a DataCategory not in its PrivacyManifest |
| Budget enforcement | Exhaustion test: consume budget to zero, attempt access | CapabilityDenied::PrivacyBudgetExhausted returned |
| Budget aggregation | Multi-agent test: deploy N agents from same developer, verify aggregate budget holds | Combined access ≤ single-agent budget |
| Taint propagation | Graph test: create multi-hop IPC path, verify taint reaches all downstream agents | No IPC message loses taint labels in transit |
| Declassification audit | Declassification test: approve declassification, verify audit entry | Every declassification produces a non-suppressible audit entry |

**Sensor privacy properties:**

| Property | Test Method | Verification |
|---|---|---|
| No silent capture | Fuzz test: attempt frame delivery without indicator activation | Anti-silent-capture gate blocks all delivery |
| Kill switch respect | GPIO simulation: engage kill switch during active session | All sessions terminated within 1 timer tick |
| Consent required | First-access test: new agent requests sensor without prior consent | ConsentRequired error returned |
| Consent revocation | Active-session test: revoke consent during active capture | Session terminated, no frames delivered after revocation |
| Multi-sensor coordination | Multi-sensor test: activate camera + microphone simultaneously | Both indicators visible, both require consent |

**Data lifecycle properties:**

| Property | Test Method | Verification |
|---|---|---|
| Scrubbing completeness | Scrub + search test: scrub object, then search all subsystems | Object unreachable from Storage, Indexer, Context, AIRS, Flow |
| Retention enforcement | Time-advance test: create object with ShortTerm retention, advance clock | Object deleted after retention period |
| Classification propagation | Derivation test: create derived object from classified source | Derived object inherits source classification |
| Cross-zone blocking | Flow test: attempt Personal→Untrusted transfer without consent | Transfer blocked by taint system |

**AI privacy properties:**

| Property | Test Method | Verification |
|---|---|---|
| KV cache isolation | Multi-session test: create sessions for agents A and B, verify no cross-read | Agent A cannot access Agent B's KV cache pages |
| Inference output taint | Context taint test: provide PII-tainted context, check output labels | Output message carries PrivacyTaintLabel matching input |
| Model provenance | Tampered model test: modify model weights, attempt load | Model rejected at boot-time integrity check |
| PII screening | Output test: generate response containing PII patterns | PII detected and filtered before delivery to untrusted context |

### §13.2 Privacy Regression Testing

CI pipeline additions to prevent privacy regressions:

**Shared crate unit tests:**

Privacy types (`PrivacyManifest`, `PrivacyBudget`, `PrivacyTaintLabel`, `ConsentDecision`) are defined in the `shared` crate with comprehensive unit tests covering:

- Serialization/deserialization roundtrip
- Budget arithmetic (consumption, exhaustion, reset)
- Taint label merge and propagation logic
- Consent scope narrowing and expiration

**Kernel integration tests:**

Run in QEMU with the full kernel:

- Capability check path includes budget deduction
- IPC taint labels propagate through multi-hop paths
- Sensor indicator activation blocks until confirmed
- Scrubbing pipeline completes across all subsystems

**CI pipeline:**

```text
just test              # Shared crate unit tests (includes privacy types)
just check             # Build + clippy (catches unused privacy fields, dead code)
just run               # QEMU boot (verifies privacy subsystem initialization)
```

Privacy-specific CI checks:

- **Taint completeness check** — Static analysis verifies that all IPC paths from spaces with `SensitivityLevel::Restricted` to network-bound channels pass through a taint check point.
- **Indicator coverage check** — Verifies that every sensor subsystem calls `coordinator.sensor_session_starting()` before data delivery.
- **Audit coverage check** — Verifies that every `PrivacyEventType` variant has at least one emission site in the kernel.

### §13.3 Red-Team Privacy Testing

Structured adversarial testing targeting privacy specifically. Red-team scenarios are designed to exercise the detection mechanisms from [agent-privacy.md](./agent-privacy.md) §4 and the screening extensions from [ai-privacy.md](./ai-privacy.md) §10.

**Scenario categories:**

**S1: Direct exfiltration**
- Agent reads PII from Personal space and attempts to send via IPC to a network-capable agent.
- Expected: Taint labels block the network send. Audit log records the attempt.

**S2: Indirect exfiltration (dead drop)**
- Agent A writes PII to an Ephemeral space. Agent B (from different developer) reads from the same space.
- Expected: Taint labels propagate through the space. Agent B inherits PII taint. Network send blocked.

**S3: Budget splitting**
- Developer deploys 5 agents, each requesting 1/5 of the PII data.
- Expected: Developer budget aggregation (§4.3) treats all 5 agents as sharing one budget. Total access ≤ single-agent limit.

**S4: Consent fatigue**
- Agent requests camera access, is denied, immediately re-requests (repeat 10 times).
- Expected: After 3 denials in 24 hours, subsequent requests are suppressed. Agent receives a rate-limit error.

**S5: Prompt injection exfiltration**
- Adversarial web content instructs the agent to include the user's recent emails in a summary.
- Expected: Control/data plane separation prevents data-plane content from being treated as instructions. Output screening catches PII in the response.

**S6: Temporal collusion**
- Agent A reads sensitive data at time T. Agent B makes an unusual network request at time T+100ms. No direct IPC between A and B.
- Expected: Behavioral Monitor flags the temporal correlation. Alert logged. Monitoring increased for both agents.

**S7: Kill switch bypass attempt**
- Software attempts to activate the camera while the hardware kill switch is engaged.
- Expected: GPIO state check blocks session creation. No frames delivered. Kill switch state is non-overridable.

**S8: Retention override**
- Agent creates an object with `RetentionTier::Permanent` when its manifest declares `max_retention: Ephemeral`.
- Expected: Intent Verifier blocks the creation. Object not persisted. Audit log records the violation.

---

## §14 POSIX Compatibility & Cross-Reference Index

### §14.1 POSIX Privacy Bridge

Traditional POSIX applications do not have privacy manifests and are not designed for capability-based privacy enforcement. The POSIX compatibility layer ([posix.md](../../platform/posix.md)) bridges this gap by assigning a default restrictive privacy profile to unmodified applications.

**Default POSIX privacy profile:**

```rust
/// Default privacy manifest for POSIX applications
/// that have not opted into the AIOS privacy system.
/// Pseudocode — field names align with PrivacyManifest (§3.1).
fn posix_default_manifest(app_id: &str) -> PrivacyManifest {
    PrivacyManifest {
        agent_id: app_id.into(),
        manifest_version: 1,
        signature: None,               // Unsigned (legacy app)
        data_access: Vec::new(),        // No declared sensitive data access
        max_retention: RetentionTier::Ephemeral,  // Session-only retention
        flow_destinations: vec![FlowDestination::LocalOnly],
        // ... other fields set to their most restrictive values
    }
}
```

**Implications for POSIX applications:**

| Privacy Feature | POSIX Behavior | Opt-In Path |
|---|---|---|
| Data access | No sensitive data access by default | Provide `PrivacyManifest` via agent manifest |
| Sensor access | All sensors denied by default | Request capabilities through POSIX bridge |
| Network access | Standard POSIX networking allowed (taint labels still apply to data from Spaces) | N/A |
| Retention | Ephemeral (session-only) | Declare retention in manifest |
| Budget | Minimal (TL4 equivalent) | Agent manifest with trust level declaration |

**POSIX applications and network access:** The default manifest declares `FlowDestination::LocalOnly`, meaning the privacy system does not grant the POSIX application privacy-budget-funded access to send sensitive data off-device. However, POSIX applications retain standard socket-level network access (`socket()`, `connect()`, etc.) because they expect these APIs to work. The distinction: `LocalOnly` restricts *privacy-manifest-governed data flows*, not raw TCP/IP. If a POSIX application reads data from a tainted source (via file descriptor from a Space), taint labels propagate to its network sends, and the taint system blocks transmission of tainted data to off-device destinations. This provides privacy protection even for unmodified applications.

**Linux binary compatibility:** For Linux binaries running under the Linux compatibility layer ([linux-compat.md](../../platform/linux-compat.md)), the sandbox profile ([linux-compat/sandbox.md](../../platform/linux-compat/sandbox.md) §9) provides the privacy boundary. The default sandbox profile restricts access to Personal spaces and requires capability grants for sensor access.

### §14.2 Cross-Reference Index

Complete mapping of privacy mechanisms to their primary implementation location and the existing docs that define them.

| Privacy Mechanism | Primary Location | Existing Doc Reference | Phase |
|---|---|---|---|
| Capability system | `kernel/src/cap/` | [model/capabilities.md](../model/capabilities.md) §3.1–§3.7 | Phase 3 |
| DIFC taint labels | `shared/src/ipc.rs` | [intent-verifier/information-flow.md](../../intelligence/intent-verifier/information-flow.md) §5 | Phase 18 |
| Camera LED enforcement | `kernel/src/drivers/camera/` | [camera/security.md](../../platform/camera/security.md) §8.1 | Phase 33 |
| Camera consent | `kernel/src/drivers/camera/` | [camera/security.md](../../platform/camera/security.md) §8.4 | Phase 33 |
| Microphone indicator | `kernel/src/drivers/audio/` | [audio/integration.md](../../platform/audio/integration.md) §11.4 | Phase 11 |
| Audio audit | `kernel/src/drivers/audio/` | [audio/integration.md](../../platform/audio/integration.md) §11.2 | Phase 11 |
| DLP classification | `kernel/src/storage/` | [multi-device/data-protection.md](../../platform/multi-device/data-protection.md) §9.1 | Phase 39 |
| DLP enforcement | `kernel/src/storage/` | [multi-device/data-protection.md](../../platform/multi-device/data-protection.md) §9.2 | Phase 39 |
| Encryption zones | `kernel/src/storage/crypto.rs` | [storage/spaces/encryption.md](../../storage/spaces/encryption.md) §6 | Phase 4 |
| Embedding privacy | `kernel/src/intelligence/` | [space-indexer/security.md](../../intelligence/space-indexer/security.md) §10.4 | Phase 15 |
| Session isolation | `kernel/src/intelligence/` | [conversation-manager/security.md](../../intelligence/conversation-manager/security.md) §14.3 | Phase 15 |
| Preference privacy | `kernel/src/intelligence/` | [preferences/security.md](../../intelligence/preferences/security.md) §15 | Phase 13 |
| Prompt injection defense | `kernel/src/security/` | [adversarial-defense/screening.md](../adversarial-defense/screening.md) §5–§7 | Phase 18 |
| Behavioral monitoring | `kernel/src/intelligence/` | [behavioral-monitor/detection.md](../../intelligence/behavioral-monitor/detection.md) §4 | Phase 15 |
| Model registry | `kernel/src/intelligence/` | [airs/model-registry.md](../../intelligence/airs/model-registry.md) §4 | Phase 15 |
| Secure boot attestation | `kernel/src/security/` | [secure-boot/trust-chain.md](../secure-boot/trust-chain.md) §3 | Phase 35 |
| Enterprise policies | `kernel/src/intelligence/` | [multi-device/policy.md](../../platform/multi-device/policy.md) §7 | Phase 39 |
| POSIX sandbox | `kernel/src/platform/posix/` | [linux-compat/sandbox.md](../../platform/linux-compat/sandbox.md) §9 | Phase 23 |
| Privacy manifests | `shared/src/privacy.rs` | [agent-privacy.md](./agent-privacy.md) §3.1 | Phase 14 |
| Privacy budgets | `kernel/src/cap/` | [agent-privacy.md](./agent-privacy.md) §3.2 | Phase 14 |
| Privacy coordinator | `kernel/src/security/` | [privacy.md](../privacy.md) §2.2 | Phase 14 |
| Consent flow | `kernel/src/security/` | [sensor-privacy.md](./sensor-privacy.md) §6.2 | Phase 13 |
| Scrubbing pipeline | `kernel/src/storage/` | [data-lifecycle.md](./data-lifecycle.md) §7.3 | Phase 18 |
| Privacy anomaly detection | `kernel/src/intelligence/` | [intelligence.md](./intelligence.md) §11.1 | Phase 15 |
| Agent privacy scoring | `kernel/src/intelligence/` | [intelligence.md](./intelligence.md) §12.2 | Phase 15 |
