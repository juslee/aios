# Security Kit

**Layer:** Application | **Crate:** `aios_security` | **Architecture:** [`docs/security/model.md`](../../security/model.md)

## 1. Overview

Security Kit is the user-facing layer over AIOS's capability and audit systems. It surfaces
the Security Inspector, permission prompts, and the security dashboard -- translating
kernel-level capability tables and audit rings into forms a user can understand, review,
and act on. While Capability Kit (kernel layer) enforces the rules, Security Kit makes
those rules visible, explainable, and manageable.

The Kit's central principle is transparency without complexity. Every active capability
grant, every audit event, and every behavioral anomaly is available for inspection, but
the default presentation is a trust posture summary rather than a raw capability table.
Users who need deeper visibility can drill down into per-agent capability graphs, audit
timelines, and intent verification results. The Security Inspector application, built
entirely on Security Kit, serves as the reference implementation for this graduated
disclosure approach.

Security Kit also owns the permission prompt flow -- the synchronous UI that appears when
an agent requests a capability the user has not yet granted. Permission prompts are
designed to be specific, contextual, and non-habituating: they explain *why* the capability
is being requested (via Intent Kit's declared intent), *what* the agent will be able to do,
and *how long* the grant will last. The Kit tracks prompt fatigue and adjusts presentation
to avoid training users to click "Allow" reflexively.

## 2. Core Traits

```rust
use aios_capability::{Capability, CapabilityHandle, CapabilityToken};
use aios_intent::{DeclaredIntent, VerificationResult};
use aios_identity::{Did, IdentityProvider};
use aios_interface::View;

/// Live view of all active capability grants across agents and applications.
///
/// SecurityInspector provides read access to the kernel's capability tables,
/// translating raw tokens into human-readable grant descriptions.
pub trait SecurityInspector {
    /// Return a summary of the system's trust posture.
    fn trust_posture(&self) -> Result<TrustPosture, SecurityError>;

    /// List all active capability grants, optionally filtered by agent.
    fn active_grants(
        &self,
        filter: Option<&AgentFilter>,
    ) -> Result<Vec<GrantSummary>, SecurityError>;

    /// Return the full capability graph for a specific agent.
    fn agent_capabilities(&self, agent: &AgentId) -> Result<CapabilityGraph, SecurityError>;

    /// Return the delegation chain for a specific capability handle.
    fn delegation_chain(
        &self,
        handle: CapabilityHandle,
    ) -> Result<Vec<DelegationLink>, SecurityError>;

    /// List capabilities that will expire within the given duration.
    fn expiring_grants(&self, within: Duration) -> Result<Vec<GrantSummary>, SecurityError>;

    /// List all anomalies detected by the behavioral monitor.
    fn anomalies(&self) -> Result<Vec<AnomalyReport>, SecurityError>;

    /// Check whether a specific capability is currently granted to an agent.
    fn is_granted(&self, agent: &AgentId, cap: &Capability) -> Result<bool, SecurityError>;
}

/// Browsable presentation of the kernel audit ring with filtering and search.
///
/// The audit viewer transforms raw audit entries (64-byte ring buffer records)
/// into structured, searchable events with agent attribution.
pub trait AuditViewer {
    /// Query audit events with optional filters.
    fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>, SecurityError>;

    /// Return the most recent N audit events.
    fn recent(&self, count: usize) -> Result<Vec<AuditEvent>, SecurityError>;

    /// Return audit events for a specific agent.
    fn for_agent(&self, agent: &AgentId) -> Result<Vec<AuditEvent>, SecurityError>;

    /// Return audit events for a specific capability type.
    fn for_capability(&self, cap: &Capability) -> Result<Vec<AuditEvent>, SecurityError>;

    /// Search audit event descriptions with a text query.
    fn search(&self, query: &str) -> Result<Vec<AuditEvent>, SecurityError>;

    /// Export audit events in a structured format for external analysis.
    fn export(&self, filter: AuditFilter, format: ExportFormat) -> Result<Vec<u8>, SecurityError>;

    /// Return aggregate statistics about audit events.
    fn statistics(&self, period: Duration) -> Result<AuditStatistics, SecurityError>;
}

/// Synchronous capability request UI presented to the user.
///
/// When an agent requests a capability not yet granted, Security Kit
/// presents a permission prompt that explains the request in context.
/// The prompt is modal -- the requesting agent blocks until the user
/// responds.
pub trait PermissionPrompt {
    /// Show a permission prompt for the given capability request.
    /// Blocks until the user responds with Allow, Deny, or AllowOnce.
    fn prompt(
        &mut self,
        request: CapabilityRequest,
    ) -> Result<PermissionDecision, SecurityError>;

    /// Show a permission prompt with additional context from Intent Kit.
    fn prompt_with_intent(
        &mut self,
        request: CapabilityRequest,
        intent: &DeclaredIntent,
        verification: &VerificationResult,
    ) -> Result<PermissionDecision, SecurityError>;

    /// Return prompt fatigue statistics for the current session.
    fn fatigue_stats(&self) -> PromptFatigueStats;

    /// Configure prompt behavior (auto-deny after N prompts, etc.).
    fn configure(&mut self, config: PromptConfig) -> Result<(), SecurityError>;
}

/// Capability management operations exposed to the user.
///
/// Wraps Capability Kit's kernel API with user-facing semantics:
/// confirmation dialogs, batch operations, and undo support.
pub trait CapabilityManager {
    /// Revoke a specific capability grant. Triggers cascade revocation
    /// for any derived grants.
    fn revoke(&mut self, handle: CapabilityHandle) -> Result<RevocationResult, SecurityError>;

    /// Revoke all capabilities for an agent. The agent will need to
    /// re-request each capability through the permission prompt flow.
    fn revoke_all_for_agent(&mut self, agent: &AgentId) -> Result<RevocationResult, SecurityError>;

    /// Temporarily suspend a capability (can be re-enabled without re-prompting).
    fn suspend(&mut self, handle: CapabilityHandle) -> Result<(), SecurityError>;

    /// Re-enable a previously suspended capability.
    fn resume(&mut self, handle: CapabilityHandle) -> Result<(), SecurityError>;

    /// Set an expiration time on an existing grant.
    fn set_expiry(
        &mut self,
        handle: CapabilityHandle,
        expires: Duration,
    ) -> Result<(), SecurityError>;

    /// Attenuate a capability (reduce its scope without revoking it).
    fn attenuate(
        &mut self,
        handle: CapabilityHandle,
        new_scope: Capability,
    ) -> Result<CapabilityHandle, SecurityError>;

    /// Undo the most recent revocation (within a grace period).
    fn undo_revocation(&mut self) -> Result<Option<CapabilityHandle>, SecurityError>;
}

/// Aggregate trust posture view for the security dashboard.
pub trait TrustDashboard {
    /// Return the overall trust score (0.0 to 1.0).
    fn trust_score(&self) -> f32;

    /// List factors contributing to the trust score.
    fn trust_factors(&self) -> Vec<TrustFactor>;

    /// Return a timeline of trust-affecting events.
    fn trust_timeline(&self, period: Duration) -> Result<Vec<TrustEvent>, SecurityError>;

    /// List agents sorted by risk score (highest risk first).
    fn agents_by_risk(&self) -> Result<Vec<AgentRiskProfile>, SecurityError>;

    /// Return recommendations for improving trust posture.
    fn recommendations(&self) -> Vec<SecurityRecommendation>;
}
```

## 3. Usage Patterns

**Minimal -- check if an agent has a capability:**

```rust
use aios_security::SecurityKit;

let inspector = SecurityKit::inspector()?;
let has_camera = inspector.is_granted(
    &AgentId::new("com.example.videoapp"),
    &Capability::CameraAccess { origin: None },
)?;
println!("Camera access: {}", if has_camera { "granted" } else { "denied" });
```

**Realistic -- build a permission settings screen:**

```rust
use aios_security::{SecurityKit, GrantSummary};

let inspector = SecurityKit::inspector()?;

// List all agents with active capabilities
let grants = inspector.active_grants(None)?;

// Group by agent for display
let mut by_agent: BTreeMap<AgentId, Vec<GrantSummary>> = BTreeMap::new();
for grant in grants {
    by_agent.entry(grant.agent.clone()).or_default().push(grant);
}

// Render in UI
for (agent, agent_grants) in &by_agent {
    println!("=== {} ===", agent);
    for grant in agent_grants {
        println!(
            "  {} (expires: {}, delegated by: {})",
            grant.capability_description,
            grant.expires.map_or("never".into(), |e| format!("{:?}", e)),
            grant.delegated_by,
        );
    }
}

// User taps "Revoke" on a grant
let manager = SecurityKit::capability_manager()?;
let result = manager.revoke(selected_grant.handle)?;
println!("Revoked {} grants (including {} derived)", result.direct, result.cascaded);
```

**Advanced -- security dashboard with anomaly monitoring:**

```rust
use aios_security::{SecurityKit, TrustDashboard, AuditViewer};

let dashboard = SecurityKit::trust_dashboard()?;

// Overall trust posture
let score = dashboard.trust_score();
let factors = dashboard.trust_factors();
println!("Trust score: {:.0}%", score * 100.0);
for factor in &factors {
    println!("  {}: {} ({:+.1}%)", factor.name, factor.status, factor.impact * 100.0);
}

// Agents ranked by risk
let risky = dashboard.agents_by_risk()?;
for agent in risky.iter().take(5) {
    println!(
        "{}: risk={:.2}, anomalies={}, grants={}",
        agent.agent_id, agent.risk_score, agent.anomaly_count, agent.grant_count
    );
}

// Drill into audit log for the riskiest agent
let audit = SecurityKit::audit_viewer()?;
let events = audit.for_agent(&risky[0].agent_id)?;
for event in events.iter().take(10) {
    println!("[{}] {}: {}", event.timestamp, event.event_type, event.description);
}

// Act on recommendations
let recommendations = dashboard.recommendations();
for rec in &recommendations {
    println!("Recommendation: {} (severity: {:?})", rec.description, rec.severity);
    // e.g., "Revoke unused camera access for com.example.oldapp"
}
```

> **Common Mistakes**
>
> - **Bypassing `PermissionPrompt` for capability grants.** All user-facing capability
>   requests must go through the prompt flow. Direct Capability Kit grants from application
>   code are kernel-level operations reserved for system agents.
> - **Polling `active_grants()` in a tight loop.** Capability tables are relatively static.
>   Use event-based notification (via IPC Kit) for grant/revoke events instead of polling.
> - **Ignoring cascade revocation.** When you revoke a capability, all capabilities derived
>   from it through delegation chains are also revoked. Always check `RevocationResult` to
>   understand the blast radius.
> - **Displaying raw capability tokens in UI.** Capability tokens are opaque identifiers.
>   Use `GrantSummary.capability_description` for human-readable presentation.

## 4. Integration Examples

**Security Kit + Capability Kit -- permission prompt with intent verification:**

```rust
use aios_security::{SecurityKit, PermissionPrompt};
use aios_intent::IntentKit;
use aios_capability::CapabilityKit;

// An agent requests camera access. The prompt includes the agent's
// declared intent, verified by Intent Kit, so the user can make an
// informed decision.

let request = CapabilityRequest {
    agent: AgentId::new("com.example.scanner"),
    capability: Capability::CameraAccess { origin: None },
    reason: "Scan a QR code to import settings".into(),
};

let intent = IntentKit::declared_intent(&request.agent)?;
let verification = IntentKit::verify(&intent)?;

let mut prompt = SecurityKit::permission_prompt()?;
let decision = prompt.prompt_with_intent(request, &intent, &verification)?;

match decision {
    PermissionDecision::Allow { duration } => {
        CapabilityKit::grant_with_expiry(
            request.capability,
            request.agent,
            duration,
        )?;
    }
    PermissionDecision::AllowOnce => {
        CapabilityKit::grant_one_shot(request.capability, request.agent)?;
    }
    PermissionDecision::Deny => {
        // Agent receives CapabilityDenied error
    }
}
```

**Security Kit + Identity Kit -- identity-attributed audit trail:**

```rust
use aios_security::{SecurityKit, AuditViewer};
use aios_identity::IdentityKit;

let audit = SecurityKit::audit_viewer()?;
let events = audit.recent(50)?;

for event in &events {
    // Each audit event is attributed to the identity that caused it
    let identity = IdentityKit::resolve_did(&event.actor_did)?;
    println!(
        "[{}] {} ({}) -- {}",
        event.timestamp,
        identity.display_name(),
        event.actor_did,
        event.description
    );
}

// Export the last 24 hours for external SIEM integration
let export = audit.export(
    AuditFilter::since(Duration::from_secs(24 * 60 * 60)),
    ExportFormat::JsonLines,
)?;
```

**Security Kit + Interface Kit -- Security Inspector application:**

```rust
use aios_security::SecurityKit;
use aios_interface::{View, TabView, ListView};

/// The Security Inspector is built entirely on Security Kit's public API.
/// Any application can build equivalent security UIs.
struct SecurityInspectorApp {
    inspector: Box<dyn SecurityInspector>,
    audit: Box<dyn AuditViewer>,
    dashboard: Box<dyn TrustDashboard>,
    manager: Box<dyn CapabilityManager>,
}

impl SecurityInspectorApp {
    fn build_ui(&self) -> TabView {
        TabView::new(vec![
            Tab::new("Dashboard", self.build_dashboard_view()),
            Tab::new("Agents", self.build_agents_view()),
            Tab::new("Audit Log", self.build_audit_view()),
            Tab::new("Anomalies", self.build_anomaly_view()),
        ])
    }

    fn build_dashboard_view(&self) -> Box<dyn View> {
        let score = self.dashboard.trust_score();
        let factors = self.dashboard.trust_factors();
        let recommendations = self.dashboard.recommendations();
        // Render trust score gauge, factor breakdown, and action items
        Box::new(DashboardView { score, factors, recommendations })
    }
}
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `SecurityInspector::trust_posture` | `CapabilityInspect` | Read-only system overview |
| `SecurityInspector::active_grants` | `CapabilityInspect` | Can filter to own grants without cap |
| `SecurityInspector::agent_capabilities` | `CapabilityInspect` | Cross-agent visibility |
| `SecurityInspector::anomalies` | `CapabilityInspect` + `AuditRead` | Anomalies are audit-derived |
| `AuditViewer::query` | `AuditRead` | Full audit log access |
| `AuditViewer::export` | `AuditRead` + `SecurityAdmin` | Export requires elevated privilege |
| `PermissionPrompt::prompt` | `SecurityAdmin` | Only system prompt service |
| `CapabilityManager::revoke` | `SecurityAdmin` | Destructive operation |
| `CapabilityManager::revoke_all_for_agent` | `SecurityAdmin` | High-impact; audit-logged |
| `CapabilityManager::attenuate` | `SecurityAdmin` | Scope reduction |
| `TrustDashboard::trust_score` | `CapabilityInspect` | Read-only posture view |
| `TrustDashboard::agents_by_risk` | `CapabilityInspect` | Risk scoring is read-only |

## 6. Error Handling & Degradation

```rust
/// Errors returned by Security Kit operations.
#[derive(Debug)]
pub enum SecurityError {
    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// The capability handle was not found (already revoked or expired).
    HandleNotFound(CapabilityHandle),

    /// The agent was not found in the system.
    AgentNotFound(AgentId),

    /// The audit ring is temporarily unavailable.
    AuditUnavailable,

    /// The revocation would break a critical system service.
    RevocationBlocked { reason: String },

    /// The undo grace period has expired.
    UndoExpired,

    /// The prompt was dismissed without a decision (timeout or user cancelled).
    PromptDismissed,

    /// Intent verification is unavailable; prompt shown without intent context.
    IntentUnavailable,

    /// The behavioral monitor detected an inconsistency.
    AnomalyDetected(AnomalyReport),

    /// Internal error reading capability tables.
    Internal(String),
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| Intent Kit unavailable | Permission prompt shown without intent context (less informative) |
| Behavioral monitor unavailable | Anomaly tab empty; trust score excludes behavioral factors |
| AIRS unavailable | No AI-generated recommendations; static rule-based suggestions |
| Audit ring full | Oldest entries overwritten; export covers available window only |
| Capability table read fails | Inspector shows cached data with staleness indicator |
| Cross-device sync fails | Revocations apply locally; sync retried on reconnection |
| Prompt fatigue threshold hit | Auto-deny new prompts; show consolidated prompt at next interaction |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Trust score | ML-weighted scoring across behavioral, capability, and audit signals | Simple count-based heuristic |
| Anomaly detection | Behavioral monitor identifies unusual capability usage patterns | No anomaly detection |
| Recommendations | Context-aware suggestions for improving security posture | Static checklist |
| Prompt enrichment | Natural language explanation of why an agent needs a capability | Technical capability name only |
| Risk profiling | Per-agent risk scores based on behavioral history | No risk scoring |
| Audit search | Semantic search across audit events | Keyword matching only |

Security Kit's core functionality (inspection, revocation, prompts) works fully without
AIRS. AI features enhance the user experience -- making security information more
understandable and actionable -- but the enforcement layer is entirely kernel-based and
operates independently.

**Platform availability:**

| Platform | Inspector UI | Permission Prompts | Audit Log | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | Text-based (UART) | UART prompt | Full (256 entries) | Testing only |
| Raspberry Pi 4 | Compositor UI | Modal dialog | Full | Standard experience |
| Raspberry Pi 5 | Compositor UI | Modal dialog | Full | Same as Pi 4 |
| Apple Silicon | Full compositor UI | Biometric-enhanced prompts | Full | Secure Enclave integration |

**Implementation phase:** Phase 17+. Security Kit depends on Capability Kit (Phase 3+),
Interface Kit (Phase 6+ for prompts), Intent Kit (Phase 15+), and the behavioral monitor
(Phase 16+) for anomaly detection.

---

*See also: [Capability Kit](../kernel/capability.md) | [Intent Kit](../intelligence/intent.md) | [Identity Kit](identity.md) | [Interface Kit](interface.md)*
