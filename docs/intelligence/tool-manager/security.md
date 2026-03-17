# AIOS Tool Security & Observability

Part of: [tool-manager.md](../tool-manager.md) — Tool Manager
**Related:** [execution.md](./execution.md) — Execution pipeline, [sandboxing.md](./sandboxing.md) — Crash containment, [intelligence.md](./intelligence.md) — AI-native anomaly detection

---

## 11. Capability Enforcement

The Tool Manager's capability enforcement builds on the kernel capability system and the AIRS security layers. This section provides a deep dive into the three-level validation introduced in [execution.md](./execution.md) §5.3.

### 11.1 Three-Level Validation Deep Dive

**Level 1 — Kernel IPC capability (mandatory, always enforced):**

```rust
/// Checked by the kernel IPC subsystem, not by the Tool Manager.
/// The Tool Manager simply sends an IPC message to the provider;
/// if the caller lacks ChannelAccess, the kernel rejects the send.
pub fn ipc_send(
    caller_pid: ProcessId,
    target_channel: ChannelId,
    message: &RawMessage,
) -> Result<(), IpcError> {
    // Kernel checks caller's capability table for ChannelAccess(target_channel)
    let cap_table = get_process_caps(caller_pid)?;
    if !cap_table.has_capability(Capability::ChannelAccess(target_channel)) {
        return Err(IpcError::CapabilityDenied);
    }
    // ... proceed with IPC
}
```

Cross-reference: [cap/mod.rs](../../kernel/cap/mod.rs) for `CapabilityToken` and `CapabilityTable`.

**Level 2 — Intent verification (AIRS-powered, fallback-safe):**

The Intent Verifier compares the tool call against the caller's declared task context:

```rust
pub struct ToolCallIntentCheck {
    /// The caller's current task description (from Agent Manifest)
    pub caller_task: String,
    /// The tool being called
    pub tool_name: ToolName,
    /// The tool's description
    pub tool_description: String,
    /// Parameter summary (redacted for sensitive fields)
    pub param_summary: String,
    /// Verification mode
    pub mode: VerificationMode,
}
```

When AIRS is unavailable (model not loaded, AIRS crashed), Level 2 is skipped and the call proceeds with Levels 1 and 3 only. This is the "graceful degradation" principle from [airs.md](../airs.md) §9.3.

Cross-reference: [layers.md](../../security/model/layers.md) §2.1 for `IntentVerifier` and `IntentPolicy`.

**Level 3 — Tool-specific capability (declared by provider):**

```rust
/// Checked by the Tool Manager against the caller's capability set.
pub fn check_tool_capability(
    caller_caps: &CapabilityTable,
    tool: &RegisteredTool,
) -> Result<(), ToolError> {
    if let Some(required) = &tool.capability_required {
        if !caller_caps.has_capability(required.clone()) {
            return Err(ToolError::ToolCapabilityDenied {
                required: required.clone(),
            });
        }
    }
    Ok(())
}
```

### 11.2 Tool-Specific Capabilities

Tools can require specific capabilities from callers. Common capability patterns:

| Tool Category | Required Capability | Rationale |
|---|---|---|
| File operations | `ReadSpace(space_id)` / `WriteSpace(space_id)` | Tool accesses user data |
| Network tools | `Network(endpoint)` | Tool makes external requests |
| System tools | `SystemInfo` | Tool reads system state |
| Agent management | `SpawnAgent` | Tool spawns child agents |
| Pure computation | `None` | No resource access needed |

**Delegation chains:** Agent A can grant Agent B a capability, and Agent B can declare a tool that requires that capability. When Agent C calls the tool, Agent C must hold the capability — not Agent A. Capabilities flow through grant chains, not through tool calls.

Cross-reference: [capabilities.md](../../security/model/capabilities.md) §3.6 for attenuation and delegation.

### 11.3 Trust Levels for Tool Providers

Tool providers are classified by trust level, which affects their default resource limits and capability ceiling:

| Trust Level | Provider Type | Default Limits | Capability Ceiling |
|---|---|---|---|
| **System** (TL3) | Kernel-spawned services, AIRS | Highest | Full system access |
| **Verified** (TL2) | Signed by known developer, reviewed | Standard | User-granted capabilities |
| **Community** (TL1) | User-installed, marketplace | Restricted | Subset of user-granted capabilities |
| **Untrusted** (TL0) | Unknown origin, first run | Minimal | Read-only, no network, no spawn |

Trust levels are assigned during agent installation (see [agents.md](../../applications/agents.md) §3.1 for the installation flow) and stored in the agent manifest. The Tool Manager uses trust levels for:

- **Multi-provider ranking** (§5.2.1): Higher trust = preferred provider
- **Rate limit defaults**: Lower trust = stricter rate limits
- **Audit verbosity**: Lower trust = more detailed audit (parameter logging, not just hashes)

### 11.4 Rate Limiting

Rate limiting prevents tool call flooding from compromised or buggy agents:

```rust
pub struct ToolRateLimiter {
    /// Per-caller limits (calls per second by AgentId)
    per_caller: HashMap<AgentId, RateCounter>,
    /// Per-tool limits (calls per second by ToolId, declared by provider)
    per_tool: HashMap<ToolId, RateCounter>,
    /// Global limit (total calls per second across all tools)
    global: RateCounter,
    /// Configuration
    config: RateLimitConfig,
}

pub struct RateLimitConfig {
    /// Default per-caller rate (calls/second)
    pub default_caller_rate: u32,     // default: 100
    /// Maximum per-caller rate (even if provider allows more)
    pub max_caller_rate: u32,         // default: 1000
    /// Global rate across all tools
    pub global_rate: u32,             // default: 10000
    /// Rate limit window (seconds)
    pub window_seconds: u32,          // default: 1
}

pub struct RateCounter {
    /// Number of calls in the current window
    count: u32,
    /// Window start timestamp
    window_start: Timestamp,
    /// Limit for this window
    limit: u32,
}
```

**Rate limit enforcement point:** Rate limiting is checked in the Tool Manager (Stage 2.5, between registry lookup and capability validation). This ensures rate-limited calls don't waste capability check resources.

**Provider-declared rate limits:** A provider can declare a per-tool rate limit in the tool metadata (e.g., "this tool can handle 10 calls/second"). The Tool Manager respects these limits and returns `RateLimited` errors when exceeded.

---

## 12. Audit and Observability

### 12.1 Audit Log Format

Every tool call produces an audit record, regardless of outcome:

```rust
pub struct ToolCallAuditEntry {
    /// Monotonic timestamp
    pub timestamp: Timestamp,
    /// Caller agent
    pub caller: AgentId,
    /// Provider agent
    pub provider: AgentId,
    /// Tool name
    pub tool_name: ToolName,
    /// SHA-256 hash of parameters (not raw params, for privacy)
    pub param_hash: ContentHash,
    /// Whether raw parameters were logged (TL0/TL1 providers only)
    pub params_logged: bool,
    /// Call outcome
    pub outcome: AuditOutcome,
    /// End-to-end latency (microseconds)
    pub latency_us: u64,
    /// Capability tokens used (IDs only)
    pub capabilities_used: Vec<CapabilityTokenId>,
    /// Delegation chain (if tool call triggered nested calls)
    pub delegation_chain: Vec<AgentId>,
}

pub enum AuditOutcome {
    Success,
    ProviderError(String),
    ProviderCrashed,
    ProviderTimeout,
    CapabilityDenied,
    IntentMisaligned,
    SchemaValidationFailed,
    RateLimited,
    Cancelled,
}
```

Cross-reference: [service/mod.rs](../../kernel/service/mod.rs) for the kernel audit ring pattern. Tool call audit entries share the same ring buffer infrastructure.

**Privacy considerations:**

- Parameters are hashed by default (SHA-256). Only the hash is stored in the audit trail.
- For untrusted providers (TL0/TL1), raw parameters are also logged for forensic analysis.
- For system providers (TL3), only hashes are logged (system tools handle sensitive data).
- Users can configure per-agent audit verbosity through the preference system.

### 12.2 Metrics

The Tool Manager exports metrics using the kernel observability framework:

```rust
pub struct ToolMetrics {
    /// Total tools registered (gauge)
    pub tools_registered: Gauge,
    /// Tool calls per second (counter, per-tool)
    pub tool_calls: Counter,
    /// Tool call latency (histogram, per-tool)
    pub tool_call_latency: Histogram<16>,
    /// Tool call errors by type (counter, per-error-type)
    pub tool_call_errors: Counter,
    /// Tool call timeout rate (counter)
    pub tool_call_timeouts: Counter,
    /// Schema validation failures (counter)
    pub schema_validation_failures: Counter,
    /// Rate limit rejections (counter)
    pub rate_limit_rejections: Counter,
    /// Provider crashes (counter, per-provider)
    pub provider_crashes: Counter,
}
```

Cross-reference: [metrics.rs](../../kernel/observability/metrics.rs) for `Counter`, `Gauge`, `Histogram<N>`.

**Key dashboards:**

| Metric | Alert Threshold | Action |
|---|---|---|
| `tool_call_errors` / `tool_calls` > 10% | Error rate spike | Investigate provider health |
| `tool_call_latency` p99 > 10× p50 | Latency outliers | Check provider resource contention |
| `provider_crashes` > 3/minute | Unstable provider | Circuit breaker activates |
| `rate_limit_rejections` sustained | Flood attempt | Review caller behavior |

### 12.3 Tool Call Tracing

Tool calls participate in the kernel distributed tracing system:

```rust
pub struct ToolCallTrace {
    /// Trace ID (propagated across tool call chains)
    pub trace_id: TraceId,
    /// Span ID for this specific call
    pub span_id: SpanId,
    /// Parent span (the caller's current span)
    pub parent_span: Option<SpanId>,
    /// Tool call stages with timestamps
    pub stages: Vec<TraceStage>,
}

pub struct TraceStage {
    pub stage: u8,           // 1–7
    pub stage_name: &'static str,
    pub start_us: u64,
    pub end_us: u64,
    pub metadata: Option<String>,
}
```

**Trace propagation:** When Agent A calls Agent B's tool, and Agent B internally calls Agent C's tool, the same `trace_id` propagates through the entire chain. This creates a complete call tree visible in the Inspector.

Cross-reference: [trace.rs](../../kernel/observability/trace.rs) for `TraceEvent` and `TraceRing`.

### 12.4 Inspector Integration

The Inspector security dashboard ([inspector.md](../../applications/inspector.md)) provides a dedicated Tool Manager view:

**Registry view:**
- All registered tools with provider, trust level, and capability requirements
- Tool registration/deregistration timeline
- Schema viewer for each tool

**Live call view:**
- Currently in-flight tool calls with stage progress
- Real-time latency and error rate graphs
- Active circuit breakers

**Historical analysis:**
- Tool call patterns over time (which agents call which tools)
- Anomaly flags from behavioral monitoring ([intelligence.md](./intelligence.md) §16.1)
- Delegation chain visualization (A → B → C call trees)
