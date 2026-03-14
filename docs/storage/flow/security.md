# AIOS Flow Security

Part of: [flow.md](../flow.md) — Flow System
**Related:** [data-model.md](./data-model.md) — Capability types, [integration.md](./integration.md) — Cross-agent flow, [model.md](../security/model.md) — System security model

-----

## 11. Security

### 11.1 Capability Enforcement

Flow security is built on the same capability model as every other AIOS subsystem:

```text
Agent wants to push content to Flow:
  1. Agent calls ctx.flow().push(content, options)
  2. SDK sends IPC to Flow Service (sys.flow channel)
  3. Kernel IPC handler checks: does this agent hold FlowWrite capability?
     NO  → IPC rejected, agent gets PermissionDenied
     YES → message delivered to Flow Service
  4. Flow Service checks: is the transfer target valid?
     If target is Agent(id): does target agent exist and have FlowRead?
  5. Transfer proceeds

Agent wants to pull content from Flow:
  1. Agent calls ctx.flow().pull(filter)
  2. SDK sends IPC to Flow Service
  3. Kernel checks FlowRead capability
  4. Flow Service checks: is there a transfer visible to this agent?
     - Transfers with target: Any → visible to all agents with FlowRead
     - Transfers with target: Agent(this_agent) → visible
     - Transfers with target: Agent(other_agent) → NOT visible
  5. Content delivered (with transform if needed)
```

**Isolation guarantee:** An agent with FlowRead cannot read transfers targeted at other agents. The Flow Service enforces this. The kernel enforces that only agents with FlowRead can even talk to the Flow Service's read endpoint.

### 11.2 Content Screening

> **Note:** Content sanitization (§15.6 in [extensions.md](./extensions.md)) operates as a defense-in-depth layer *before* the AIRS-based screening described here. Sanitization validates content structure (strips scripts from HTML, validates image headers); screening detects sensitive data patterns (credit cards, passwords, PII).

AIRS screens Flow content for sensitive data before delivery to untrusted agents:

```rust
pub struct FlowContentScreen {
    /// Patterns that indicate sensitive content
    sensitive_patterns: Vec<SensitivePattern>,

    /// Agent trust levels (from security framework)
    trust_levels: HashMap<AgentId, TrustLevel>,
}

pub struct SensitivePattern {
    /// Human-readable name
    name: String,
    /// Detection method
    detector: SensitiveDetector,
    /// What to do when detected
    action: ScreenAction,
}

pub enum SensitiveDetector {
    /// Regex pattern (credit card numbers, SSNs, API keys)
    Regex(String),
    /// AIRS classification (passwords, credentials, PII)
    AirsClassifier(String),
}

pub enum ScreenAction {
    /// Allow but warn user
    Warn,
    /// Block transfer, require user confirmation
    Block,
    /// Redact the sensitive portion
    Redact,
}
```

**Screening rules:**

| Pattern | Detection | Action on untrusted agent |
|---|---|---|
| Credit card number | Regex (Luhn check) | Block, require confirmation |
| Social Security Number | Regex | Block, require confirmation |
| API key / token | Regex (`sk-`, `ghp_`, `AKIA`) | Warn |
| Password | AIRS classifier | Block |
| PII (address, phone) | AIRS classifier | Warn |
| Private key material | Regex (BEGIN PRIVATE KEY) | Block |

**Trust-based screening:** Screening is only applied when content flows to agents with lower trust than the source. System agents are fully trusted. Native experience agents are trusted. Third-party agents and tab agents are screened. Transfer between two system agents is never screened (performance optimization).

**Inspector audit trail:** Every Flow transfer is logged to the audit space (`system/audit/flow/`). The Inspector shows the full trail: timestamp, source agent, destination agent, content type, intent, any transforms applied, any screening actions taken. This is the transparency mechanism — the user can always see what data moved where.

### 11.3 Rate Limiting and Abuse Prevention

A misbehaving or compromised agent could flood the Flow Service with transfers, consuming shared memory and filling history storage. Flow enforces per-agent rate limits:

```rust
pub struct FlowRatePolicy {
    /// Maximum transfers an agent can initiate per minute
    max_transfers_per_minute: u32,          // default: 120

    /// Maximum total bytes an agent can stage concurrently
    /// (across all in-flight transfers, before delivery)
    max_staged_bytes: u64,                  // default: 256 MB

    /// Maximum number of concurrent in-flight transfers per agent
    max_concurrent_transfers: u32,          // default: 32

    /// Maximum history entries an agent can create per hour
    /// (prevents history spam from rapid automated copy/paste)
    max_history_entries_per_hour: u32,      // default: 500

    /// Cooldown period after hitting a rate limit
    /// (agent must wait before retrying)
    cooldown: Duration,                     // default: 5 seconds
}

pub enum RateLimitAction {
    /// Transfer rejected, agent receives FlowError::RateLimited
    Reject,
    /// Transfer queued and delivered when budget allows
    Queue,
}
```

**Enforcement rules:**

| Limit | Action when exceeded | System agents exempt? |
|---|---|---|
| Transfers per minute | Reject (FlowError::RateLimited) | Yes |
| Staged bytes | Reject until existing transfers complete | Yes |
| Concurrent transfers | Queue (deliver in order when slots free) | Yes |
| History entries per hour | Entries still created but marked low-priority for pruning | Yes |

**Escalation:** If an agent repeatedly hits rate limits (>10 rejections in 5 minutes), the Flow Service emits a `FlowAbuse` event to the Inspector and the Attention Panel. The user sees: "[Agent X] is making unusually frequent clipboard operations." The user can revoke the agent's FlowWrite capability from the Attention Panel.

System agents (compositor, POSIX bridge) are exempt from rate limits because they are trusted and mediate on behalf of the user's direct actions.
