# AIOS AI-Native Identity Intelligence

Part of: [identity.md](../identity.md) — Identity & Relationships
**Related:** [core.md](./core.md) — Key management, [relationships.md](./relationships.md) — Trust model, [privacy.md](./privacy.md) — Recovery design, [credentials.md](./credentials.md) — Credential isolation

**Cross-references:** [behavioral-monitor](../../intelligence/behavioral-monitor.md), [intent-verifier](../../intelligence/intent-verifier.md), [context-engine](../../intelligence/context-engine.md)

-----

## 17. AI-Native Identity Intelligence

This section describes how AI enhances identity operations. Capabilities are split into two categories: **kernel-internal ML** (no AIRS dependency, frozen statistical models that run even when AIRS is offline) and **AIRS-dependent** (requires semantic understanding from the AI Runtime).

### 17.1 Kernel-Internal ML

These models run as frozen decision trees or statistical estimators within the kernel identity subsystem. They have no AIRS dependency and operate with fixed memory budgets.

#### 17.1.1 Keystroke Timing Model

Continuous authentication via keystroke dynamics. A ~10KB frozen model captures the user's typing rhythm (inter-key intervals, hold times, flight times).

- **Input:** Keystroke event stream from input subsystem
- **Output:** Confidence score `[0.0, 1.0]` — probability that the current typist is the enrolled user
- **Equal Error Rate (EER):** 3–7% (competitive with dedicated keystroke biometric systems)
- **Update:** Model parameters refreshed during authenticated sessions; frozen when unauthenticated
- **Use:** Gates sensitive operations (key rotation, device addition) when session confidence drops

#### 17.1.2 Session Confidence Score

Exponentially decaying confidence that the current session is operated by the identity owner:

```text
c(t) = c₀ * exp(-λt)
```

Where `c₀` is the confidence at last authentication event, `λ` is the decay rate (configurable per trust level), and `t` is time since last authentication signal. Authentication signals that reset `c₀` to 1.0: passphrase entry, keystroke model match, hardware key touch.

- **Budget:** ~100 bytes (3 floats + timestamp)
- **Use:** Gating function for sensitive identity operations. When `c(t) < threshold`, the system requires re-authentication before proceeding.

#### 17.1.3 Trust Anomaly Detection

Z-score anomaly detection on trust relationship changes:

- **Input:** Trust delta events (trust score changes between peers)
- **Model:** Welford running mean + variance (~100 bytes per tracked peer)
- **Detection:** Alert when `|trust_delta| > 3σ` from historical mean
- **Use:** Flags suspicious trust changes (e.g., a Colleague suddenly elevated to Family without interaction history). Feeds into the behavioral monitor.

#### 17.1.4 Key Rotation Prediction

Decision tree (~1KB) that predicts when key rotation should be recommended based on:

- Time since last rotation
- Number of signatures issued
- Device count changes
- Trust graph changes (new high-trust relationships)
- Anomalous signing patterns

The model outputs a rotation urgency score. When the score exceeds a threshold, the system proactively suggests key rotation to the user.

#### 17.1.5 Recovery Risk Scorer

Estimates the current recoverability of the user's identity based on share availability:

- **Input:** Device reachability timestamps, share distribution state, dead man's switch status
- **Model:** Weighted sum estimator (~500 bytes) — weights per shareholder type (primary device, secondary device, trusted peer)
- **Output:** Recovery risk score `[0.0, 1.0]` where 1.0 = fully recoverable, 0.0 = identity at risk
- **Threshold:** When score drops below 0.5 (e.g., too few reachable shareholders), proactively warn the user
- **Use:** Triggers share redistribution suggestions, warns when devices holding VSS shares haven't been seen recently
- **Cross-reference:** [privacy.md §14.4–§14.7](./privacy.md) for the VSS share distribution and refresh mechanisms

#### 17.1.6 Key Usage Anomaly Detection

Welford running mean + z-score on signing frequency per key:

- **Input:** Signing operation timestamps and key IDs
- **Model:** Per-key running statistics (~32 bytes per key)
- **Detection:** Alert when signing frequency deviates >3σ from baseline
- **Use:** Detects compromised keys being used for mass signing, or dormant keys suddenly becoming active

### 17.2 AIRS-Dependent Intelligence

These capabilities require the AI Runtime (AIRS) for semantic understanding. They enhance identity operations but are not required for core functionality.

#### 17.2.1 Behavioral Biometric Fusion

Combines multiple biometric signals for stronger continuous authentication:

- Keystroke dynamics (kernel-internal, §17.1.1)
- Application usage patterns (AIRS — requires semantic understanding of app context)
- Interaction timing patterns (AIRS — requires understanding of user workflow context)
- **Combined EER:** 1–3% (significantly better than keystroke alone)
- **Cross-reference:** [behavioral-monitor/profiling.md §8](../../intelligence/behavioral-monitor/profiling.md) for the profiling pipeline

#### 17.2.2 Intent Verification for Identity Operations

Natural language understanding of identity operation requests:

- "Revoke my old laptop" → maps to `revoke_device()` with device identification
- "Share photos with Mom" → maps to `share_space()` with relationship lookup + space identification
- "I think my key was compromised" → triggers `emergency_rekey()` workflow with guided ceremony
- **Cross-reference:** [intent-verifier](../../intelligence/intent-verifier.md) for the verification pipeline

#### 17.2.3 Guardian Health Monitoring

For multi-device recovery (§14), AIRS proactively monitors recovery share availability:

- Periodic reachability checks of devices holding VSS shares
- Predict guardian availability based on interaction patterns
- Suggest share redistribution when guardians become unreachable
- Natural language guidance through recovery ceremonies

#### 17.2.4 Trust Context Inference

AIRS analyzes interaction metadata to suggest trust level adjustments:

- Detects when a Colleague relationship has evolved to Friend-level interaction patterns
- Identifies dormant relationships that should have trust decay applied
- Suggests relationship kind upgrades/downgrades based on communication patterns

#### 17.2.5 Collusion Detection in Trust Graph

Graph neural network analysis of the trust relationship graph:

- Detects coordinated trust elevation attacks (multiple colluding identities)
- Identifies Sybil patterns (many low-interaction identities with similar behavior)
- Flags trust graph topology anomalies (isolated clusters, star patterns around a single node)

#### 17.2.6 Natural Language Recovery Ceremony Guidance

Conversational guidance through complex identity operations:

- Step-by-step passphrase change with confirmation
- Device addition ceremony with security explanations
- Emergency rekey process with relationship notification status
- Recovery share redistribution with guardian selection advice

### 17.3 Comparative Analysis

| System | Identity Model | Key Storage | Recovery | Trust Model |
|--------|---------------|-------------|----------|-------------|
| **AIOS** | Ed25519 key pair, local-first | Kernel Crypto Core (never in userspace) | Graduated 3-tier (session → device VSS → identity VSS) | Graduated (Family→Unknown), multi-signal, EigenTrust |
| **Fuchsia** | Account-based (Google) | Userspace KMS | Cloud-based (Google account) | Binary (authenticated/not) |
| **seL4** | No kernel crypto | Application-level | Application-level | Capability-only |
| **Signal** | Per-device keys (Sesame) | OS keychain | Phone number + PIN | Binary (verified/not) |
| **GrapheneOS** | Android keystore | TEE (Titan M2) | Google account + duress key | Android permissions |
| **Apple** | iCloud Keychain | Secure Enclave | iCloud recovery | Binary + Face/Touch ID |

**Key differentiators:**

- AIOS is the only system where private keys never leave the kernel address space
- AIOS's graduated trust model (6 tiers) is more nuanced than any compared system
- Prevention-first recovery avoids the custodial burden that plagues all compared systems
- AI-native continuous authentication (keystroke + behavioral fusion) is unique to AIOS
