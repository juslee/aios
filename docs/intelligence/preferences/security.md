# AIOS Preference Security and Capabilities

Part of: [preferences.md](../preferences.md) — Preference System
**Related:** [data-model.md](./data-model.md) — SecurityClassification and PreferenceSource types, [resolution.md](./resolution.md) — Enterprise policy precedence, [intelligence.md](./intelligence.md) — Anomaly detection for preference changes

-----

## 15. Security and Capabilities

Preferences are a high-value attack surface. A malicious agent that gains write access to `privacy.ai_data_local_only` can exfiltrate user data. A compromised MDM channel that overrides `attention.night_suppress` can disrupt user focus. A preference injection attack that silently lowers `display.brightness` to zero can denial-of-service the user interface.

AIOS addresses these threats through four complementary mechanisms: capability-based access control that restricts who can read and modify preferences, enterprise policy signing that prevents unauthorized organizational overrides, rate-limiting and anomaly detection that catch malicious agent behavior, and comprehensive audit logging that records every preference operation.

-----

### 15.1 Capability-Gated Preference Access

All preference operations require explicit capability tokens. The capability system follows the standard AIOS model described in `security/model/capabilities.md §3`.

```rust
pub enum PreferenceCapability {
    /// Read preference values and metadata.
    /// Most agents need this for basic operation.
    PreferenceRead,

    /// Read preference change history and audit trail.
    /// Useful for analytics agents and user-facing explain features.
    PreferenceHistoryRead,

    /// Write agent-scoped preferences (agent.weather.temperature_unit).
    /// Agents can only modify their own namespace.
    PreferenceAgentWrite,

    /// Suggest system preference changes (requires user approval).
    /// The suggestion goes through the conflict resolution pipeline (§10).
    PreferenceSuggest,

    /// Write system preferences directly (bypasses approval flow).
    /// Reserved for the Conversation Bar, Settings UI, and Preference Service.
    PreferenceSystemWrite,

    /// Create, modify, or delete context rules (§14).
    PreferenceRuleManage,

    /// Manage cross-device sync policy (§11).
    PreferenceSyncControl,

    /// Manage enterprise policies (EnterpriseLocked/Recommended).
    /// Requires signed policy payloads from a trusted MDM server.
    EnterprisePolicy,

    /// Access behavioral inference data and train preference models (§16, §17).
    /// Used by AIRS bandit model; receives context features, not raw values.
    PreferenceLearning,

    /// Full administrative access (schema registration, bulk operations).
    PreferenceAdmin,
}
```

**Trust Level Mapping:**

| Capability | Min Trust Level | Typical Holder |
|---|---|---|
| `PreferenceRead` | 1 (User) | Any agent |
| `PreferenceHistoryRead` | 1 (User) | Settings UI, analytics agents |
| `PreferenceAgentWrite` | 1 (User) | Any agent (own namespace only) |
| `PreferenceSuggest` | 1 (User) | System agents (power, network) |
| `PreferenceSystemWrite` | 2 (Privileged) | Conversation Bar, Settings UI |
| `PreferenceRuleManage` | 2 (Privileged) | Context Engine, Settings UI |
| `PreferenceSyncControl` | 3 (System) | Sync service |
| `EnterprisePolicy` | 3 (System) | MDM agent (with signed payloads) |
| `PreferenceLearning` | 3 (System) | AIRS bandit model |
| `PreferenceAdmin` | 4 (Kernel) | Preference Service itself |

The Preference Service checks capability tokens at every API entry point. A missing or insufficiently trusted token results in `PreferenceError::CapabilityDenied` and an audit entry (see §15.4).

### 15.2 Capability Attenuation

Capability tokens can be attenuated before delegation, narrowing the scope of access while preserving the least-privilege principle:

- `PreferenceRead` can be attenuated to a **specific category**: a weather agent receives `PreferenceRead` attenuated to `PreferenceCategory::Network`, allowing it to check metered-connection status but not read privacy settings.
- `PreferenceRead` can be attenuated to a **specific preference**: an accessibility agent receives `PreferenceRead` attenuated to `display.font_scale` only.
- `PreferenceAgentWrite` is inherently attenuated to the agent's own namespace — the capability token embeds the `AgentId` and the Preference Service enforces namespace isolation.

```rust
pub enum PreferenceAttenuation {
    /// Restrict to a single preference category
    Category(PreferenceCategory),
    /// Restrict to a specific preference ID
    Specific(PreferenceId),
    /// Restrict to a prefix (e.g., "display.*")
    Prefix(String),
    /// No attenuation (full access within the capability level)
    Full,
}
```

### 15.3 Agent Permission Model

Agents declare their preference requirements in their manifest. The capability gate evaluates these declarations at agent installation and runtime:

```toml
# Agent manifest — preference declarations
[capabilities.preferences]
# System preferences this agent needs to READ
read = ["display.font_scale", "display.theme", "network.metered"]

# System preferences this agent may SUGGEST changes to
suggest = ["attention.weather_agent.max_urgency"]

# Agent-scoped preferences this agent defines and manages
[preferences]
temperature_unit = { type = "enum", options = ["celsius", "fahrenheit"], default = "celsius" }
update_interval = { type = "duration", default = "30m", min = "5m", max = "24h" }
show_humidity = { type = "bool", default = true }
```

At installation, the system:

1. Validates that requested read preferences exist in the schema registry
2. Creates attenuated `PreferenceRead` tokens for each declared system preference
3. Creates a `PreferenceAgentWrite` token scoped to the agent's namespace
4. If the agent declares `suggest` preferences, creates an attenuated `PreferenceSuggest` token
5. Stores the capability tokens in the agent's capability table

At runtime, every preference API call is checked:

```rust
impl PreferenceService {
    pub fn read_preference(
        &self,
        caller: &AgentId,
        id: &PreferenceId,
    ) -> Result<PreferenceValue, PreferenceError> {
        // Check capability
        let cap = self.capability_gate.check(
            caller,
            PreferenceCapability::PreferenceRead,
        )?;

        // Check attenuation scope
        if !cap.attenuation.permits(id) {
            self.audit.log(PreferenceAuditEvent::AccessDenied {
                agent: caller.clone(),
                preference: id.clone(),
                operation: "read",
            });
            return Err(PreferenceError::CapabilityDenied);
        }

        Ok(self.store.get(id).value.clone())
    }
}
```

### 15.4 Audit Trail

Every preference operation generates an audit event. The audit trail integrates with the kernel audit ring (see `service/mod.rs`) and the security model's event system (see `security/model/operations.md §6`).

```rust
pub enum PreferenceAuditEvent {
    /// Preference value changed
    Changed {
        preference: PreferenceId,
        old_value: PreferenceValue,
        new_value: PreferenceValue,
        source: PreferenceSource,
        agent: AgentId,
    },

    /// Preference read (logged for sensitive categories only)
    Read {
        preference: PreferenceId,
        agent: AgentId,
        classification: SecurityClassification,
    },

    /// Access denied (always logged)
    AccessDenied {
        agent: AgentId,
        preference: PreferenceId,
        operation: &'static str,
    },

    /// Agent suggestion submitted
    SuggestionSubmitted {
        agent: AgentId,
        preference: PreferenceId,
        proposed_value: PreferenceValue,
        reason: String,
    },

    /// Agent suggestion accepted or rejected by user
    SuggestionResolved {
        agent: AgentId,
        preference: PreferenceId,
        accepted: bool,
    },

    /// Context rule activated or deactivated
    ContextRuleTriggered {
        rule: ContextRuleId,
        activated: bool,
        overrides_applied: Vec<PreferenceId>,
    },

    /// Enterprise policy applied
    EnterprisePolicyApplied {
        policy_id: PolicyId,
        preferences_affected: Vec<PreferenceId>,
    },

    /// Agent suggestion silently dropped due to rate limiting (§15.6)
    SuggestionRateLimited {
        agent: AgentId,
        preference: PreferenceId,
    },

    /// Anomaly detected in preference change patterns (§16.4)
    AnomalyDetected {
        agent: AgentId,
        category: String,
        severity: String,
        details: String,
    },

    /// Schema registered by subsystem
    SchemaRegistered {
        subsystem: String,
        preference_count: usize,
    },
}
```

**Audit verbosity tiers:**

| Event | Logged When |
|---|---|
| `Changed` | Always |
| `Read` | Only for `CategoryRestricted` and `SystemOnly` preferences |
| `AccessDenied` | Always |
| `SuggestionSubmitted` | Always |
| `SuggestionResolved` | Always |
| `ContextRuleTriggered` | Always |
| `EnterprisePolicyApplied` | Always |
| `SuggestionRateLimited` | Always |
| `AnomalyDetected` | Always |
| `SchemaRegistered` | At subsystem initialization |

### 15.5 Enterprise Policy Security

Enterprise policies (`EnterpriseLocked` and `EnterpriseRecommended`) must be cryptographically signed to prevent unauthorized organizational overrides. This prevents a compromised agent from injecting fake enterprise policies.

```rust
pub struct EnterprisePolicy {
    /// Policy payload
    pub preferences: Vec<EnterprisePolicyEntry>,
    /// Organization that issued the policy
    pub organization: OrganizationId,
    /// Timestamp of policy issuance
    pub issued_at: SystemTime,
    /// Ed25519 signature over the serialized payload
    pub signature: Signature,
    /// Public key certificate chain to organization root
    pub certificate_chain: Vec<Certificate>,
}

pub struct EnterprisePolicyEntry {
    /// Which preference this policy controls
    pub preference_id: PreferenceId,
    /// Locked (user cannot change) or Recommended (user can override)
    pub enforcement: PolicyEnforcement,
    /// Value to set
    pub value: PreferenceValue,
    /// Human-readable rationale shown to the user
    pub rationale: String,
}

pub enum PolicyEnforcement {
    /// User cannot change this value. Settings UI shows it as locked with rationale.
    Locked,
    /// Sets the default. User can override but sees the org recommendation.
    Recommended,
}
```

The Preference Service validates the signature chain before applying any enterprise policy. An invalid or expired signature results in `PreferenceError::PolicySignatureInvalid` and an audit event.

### 15.6 Agent Suggestion Rate-Limiting

To prevent agents from overwhelming the user with preference suggestions or attempting brute-force preference manipulation:

```rust
pub struct SuggestionRateLimiter {
    /// Maximum suggestions per agent per hour
    pub max_per_hour: u32,
    /// Maximum suggestions per agent per day
    pub max_per_day: u32,
    /// Minimum time between suggestions for the same preference
    pub min_interval_per_preference: Duration,
    /// Minimum observation window before an agent can suggest
    /// behavioral inference (prevents snap judgments)
    pub min_observation_window: Duration,
    /// Cooldown after user rejects a suggestion for the same preference
    pub rejection_cooldown: Duration,
}

impl Default for SuggestionRateLimiter {
    fn default() -> Self {
        SuggestionRateLimiter {
            max_per_hour: 3,
            max_per_day: 10,
            min_interval_per_preference: Duration::from_secs(3600),
            min_observation_window: Duration::from_secs(86400 * 3), // 3 days
            rejection_cooldown: Duration::from_secs(86400 * 7),     // 7 days
        }
    }
}
```

Suggestions that exceed rate limits are silently dropped (not queued) and logged as `PreferenceAuditEvent::SuggestionRateLimited`.

### 15.7 Privacy Protection

Preferences can contain personally identifiable information (PII) — location rules reveal home/work addresses, behavioral observations reveal daily routines, and preference history reveals personality traits.

**Privacy controls:**

- **Encryption at rest:** Preference values classified as `SecurityClassification::CategoryRestricted { category: PreferenceCategory::Privacy }` or `SecurityClassification::SystemOnly` are stored encrypted in the `user/preferences/` space using the per-space encryption described in `storage/spaces/encryption.md §6`.
- **Minimal behavioral logging:** The Behavioral Observer (§6) records observation *summaries*, not raw interaction data. "User enabled dark mode at 8pm" is stored; "User was browsing news articles about climate change at 8pm" is not.
- **Location anonymization:** Context rules (§14) store geofence *names* ("office") and center points, but never continuous location tracks. Location data is processed on-device and never synced.
- **Data retention:** Preference history older than 90 days is automatically compacted: individual change records are replaced with monthly summaries unless the user explicitly enables full history retention.
- **Export and deletion:** Users can export all preference data in a standard format and request complete deletion, including behavioral observations and context rules.
