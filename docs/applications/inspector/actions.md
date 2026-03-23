# AIOS Inspector — Actions & Integration

Part of: [inspector.md](../inspector.md) — Inspector Architecture
**Related:** [views.md](./views.md) — Views, [architecture.md](./architecture.md) — Architecture, [threat-model.md](./threat-model.md) — Threat Model

-----

## 6. User Actions

The Inspector is primarily a read-only dashboard, but it exposes controlled write operations for security management. Every destructive action requires user confirmation.

### 6.1 Action Table

| Action | Operation / API | Confirmation | Reversible |
|---|---|---|---|
| Revoke capability token | `CapabilityRevoke` (kernel syscall) | "Remove [agent]'s access to [resource]?" | Agent must re-request |
| Revoke all agent capabilities | Inspector loop: list tokens for agent → `CapabilityRevoke` each | "Remove all permissions for [agent]? It will not be able to function." | Agent must be re-authorized |
| Pause agent | Service Manager IPC: `service_control(agent_id, Pause)` | "Pause [agent]? It will stop all activity." | Resume available |
| Resume agent | Service Manager IPC: `service_control(agent_id, Resume)` | Immediate (no confirmation for resume) | -- |
| Add user override (Layer 90) | Profile service IPC (write user override layer) | "Add denial for [capability] on [agent]?" | Override can be removed |
| Remove user override | Profile service IPC (remove user override entry) | "Remove your override? Agent will regain [capability]." | Can re-add |
| Apply AIRS recommendation | Varies per recommendation | Shows specific effect before confirming | Depends on action type |
| Export audit log | `AuditRead` capability + local file write | "Export [N] records to [path]?" | File can be deleted |
| Acknowledge alert | Inspector local state (clears badge) | Immediate | Alert remains in history |

### 6.2 Confirmation Flow

Every destructive action presents an expanded confirmation panel before execution. The panel contains four fields:

```text
┌────────────────────────────────────────────────────────────┐
│  Confirm: Revoke Capability                                │
│                                                            │
│  What changes:                                             │
│    Research Assistant loses access to SpaceRead(research/*) │
│                                                            │
│  Agents affected:                                          │
│    Research Assistant                                       │
│    + 1 delegated token (Tab Agent via tok_d412)            │
│                                                            │
│  Reversible:  No — agent must re-request this capability   │
│                                                            │
│  [Cancel]                              [Revoke Access]     │
└────────────────────────────────────────────────────────────┘
```

**What changes** describes the concrete effect in human-readable terms ("loses access to your research papers" rather than "SpaceRead(research/*)"). Technical identifiers appear in a collapsible detail row for advanced users.

**Agents affected** lists every agent impacted, including those holding delegated tokens derived from the capability being revoked. Cascade revocation ([capabilities.md §3.5](../../security/model/capabilities.md)) propagates through the delegation chain, so the confirmation panel enumerates the full cascade before the user commits.

**Reversible** states whether the action can be undone and how. Three categories:

| Category | Examples | Undo mechanism |
|---|---|---|
| Immediately reversible | Pause agent, add user override | Explicit undo button or counterpart action |
| Re-requestable | Revoke single capability | Agent re-requests; user re-grants |
| Irreversible | Revoke all + uninstall agent | No automatic recovery; reinstallation required |

**Undo window** appears for immediately reversible actions. For a configurable period (default 30 seconds), the Inspector displays a toast notification with an "Undo" button. Pressing undo reverses the action atomically:

- **Pause** undo: resumes the agent immediately
- **Add override** undo: removes the override, restoring the previous capability set
- **Acknowledge alert** undo: marks the alert as unacknowledged, restoring the badge count

After the undo window expires, the action is considered committed. The user can still reverse it through the normal counterpart action (resume, remove override), but the one-click undo toast disappears.

### 6.3 Action Audit Trail

Every user action through the Inspector is itself recorded in the provenance chain ([layers.md §2.7](../../security/model/layers.md)). The provenance record includes:

```rust
pub struct InspectorAction {
    /// Which user initiated the action
    pub identity: IdentityId,
    /// The action taken
    pub action: ActionType,
    /// Target agent
    pub target_agent: AgentId,
    /// Target capability (if applicable)
    pub target_capability: Option<CapabilityTokenId>,
    /// Confirmation panel was shown and accepted
    pub confirmed: bool,
    /// Whether the undo window was used
    pub undone: bool,
    /// Timestamp
    pub timestamp: Timestamp,
}
```

This ensures the Inspector itself is auditable. An administrator reviewing the provenance chain can see not just agent actions, but also every security decision the user made through the Inspector.

-----

## 7. Conversation Bar Integration

The Inspector works in concert with AIRS and the Conversation Bar ([conversation-manager.md](../../intelligence/conversation-manager.md), [model.md §7.2](../../security/model.md)). Natural language security queries route through AIRS, which queries the same data sources the Inspector uses. The Conversation Bar is the *conversational* interface; the Inspector is the *visual* interface. Both show the same data.

### 7.1 Natural Language Queries

```text
User: "What has the Budget Tracker been doing?"

-> AIRS queries provenance chain (same AuditRead the Inspector uses)
-> Conversation Bar: "Budget Tracker read 15 objects from finances/budget
   today. It made 3 API calls to plaid.com. No anomalies detected."
-> The response includes a link: "Open in Inspector ->"
-> Clicking opens Inspector's Agent View filtered to Budget Tracker
```

When AIRS detects an anomaly and pauses an agent, the notification includes "Review in Inspector" which opens directly to that agent's view with the relevant security event highlighted.

### 7.2 Query Type Examples

The Conversation Bar handles five categories of security queries, each routed to the appropriate query engine:

**Provenance queries** retrieve agent activity history:
- "What has the Budget Tracker been doing?" -- retrieves recent provenance records filtered by agent
- "Show me everything the Email Agent accessed yesterday" -- time-bounded provenance scan

**Capability queries** inspect the permission landscape:
- "Which agents can access my photos?" -- reverse capability lookup across all agents for `SpaceRead(user/photos/*)`
- "Does the Research Assistant have network access?" -- single-agent capability check

**Anomaly queries** surface behavioral deviations:
- "Show me anything unusual in the last hour" -- queries the behavioral monitor ([behavioral-monitor.md](../../intelligence/behavioral-monitor.md)) for anomaly scores above threshold
- "Is any agent behaving strangely?" -- system-wide anomaly scan with ranked results

**Denial explanation queries** explain why an action was blocked:
- "Why was the Research Assistant denied access?" -- retrieves the most recent denial event, identifies the blocking layer and missing capability, and explains in plain language
- "What would it take for the Code Editor to access my documents?" -- hypothetical capability gap analysis

**Temporal comparison queries** detect behavioral drift:
- "Compare today's agent activity to last week" -- statistical comparison of provenance record counts, capability usage patterns, and anomaly scores across time windows
- "Has the Email Agent's network usage changed recently?" -- trend analysis on specific resource categories

Each query response includes an "Open in Inspector" link that navigates to the relevant view with appropriate filters pre-applied.

### 7.3 Security Copilot Workflow

AIRS functions as an investigation partner for security concerns, following a structured workflow inspired by the investigative copilot pattern:

```text
Step 1 — User describes concern
  "Something feels off about the Research Assistant"

Step 2 — AIRS queries relevant data
  - Provenance: last 24h activity (47 actions)
  - Behavioral baseline: reads/day avg=34, today=67 (1.97 sigma)
  - Capability usage: Network(arxiv.org) used 31 times (vs avg 8)
  - Security events: 0 denials, 0 anomaly alerts triggered

Step 3 — AIRS presents findings
  "The Research Assistant's read count is nearly double its average
   (67 vs 34). Its arxiv.org network calls are 4x normal. No policy
   violations, but the spike is unusual. This pattern is consistent
   with a bulk download operation."

Step 4 — AIRS suggests actions
  "Suggested actions:
   1. Review the 31 arxiv.org requests in Provenance View [open ->]
   2. Add a rate limit override for Network(arxiv.org) [apply ->]
   3. No action — activity is within capabilities, just elevated"

Step 5 — User confirms
  User selects option 2. Inspector opens confirmation panel.
  Override applied with 30-second undo window.
```

The copilot workflow is conversational and iterative. The user can ask follow-up questions ("What was it downloading from arxiv?"), and AIRS drills deeper into the provenance records. Each round of investigation uses the same query engines backing the Inspector views, ensuring consistency between what the copilot reports and what the Inspector displays.

Key design constraints for the security copilot:

- **AIRS never takes autonomous action.** It suggests; the user confirms through the standard confirmation flow (see [section 6.2](#62-confirmation-flow)).
- **Investigation queries use the Inspector's query budget.** The copilot does not bypass the 100 queries/second rate limit ([section 9](#9-performance-characteristics)).
- **Findings are grounded in data.** Every claim the copilot makes links to specific provenance records, capability tokens, or behavioral baseline metrics. No unsubstantiated assessments.

-----

## 8. Auto-Open Triggers

The Inspector normally runs in the background (no visible window). It auto-opens in these scenarios:

| Trigger | Severity | Inspector Behavior |
|---|---|---|
| Level 4 incident (chain integrity, suspected compromise) | Critical | Opens immediately, full-screen, blocks until acknowledged |
| Level 3 incident (agent terminated, spaces locked) | Critical | Opens at next user interaction, Agent View focused |
| Repeated anomalies from same agent | High | Badge on dock/taskbar icon; opens if user clicks |
| User says "open Inspector" or "show security" | -- | Opens Dashboard view |
| Agent installation (AIRS analysis complete) | -- | Opens AIRS Analysis View for the new agent |

Auto-open behavior respects the 4-level escalation policy defined in [layers.md §2.8](../../security/model/layers.md). Only Level 3 and Level 4 incidents force the Inspector open without user initiation. Lower-severity events use badge notifications and are available on demand.

The compositor ([compositor.md](../../platform/compositor.md)) handles the Inspector's surface lifecycle. When a Level 4 auto-open fires, the compositor grants the Inspector's surface top-level focus and blocks input to other surfaces until the user acknowledges the incident. This is the only scenario where the Inspector takes focus away from the user's current task.

-----

## 9. Performance Characteristics

The Inspector must never degrade system performance. It is a monitoring tool -- if monitoring itself becomes expensive, it defeats the purpose.

### 9.1 Query Budget

The Inspector limits itself to soft rate caps that prevent audit subsystem contention:

| Query type | Rate limit | Notes |
|---|---|---|
| Provenance queries | 100/sec | Covers activity feed, search, chain verification |
| Capability table scans | 10/sec | Full table scan across all agents |
| Agent metadata queries | 50/sec | Metadata, trust level, behavioral baseline |
| AIRS analysis requests | 1/sec | Inference queries are compute-heavy |
| Security event subscriptions | Continuous | Push-based via kernel event notifications |

These are soft limits enforced by the Inspector itself. The kernel rate-limits `AuditRead` independently ([operations.md §7.3](../../security/model/operations.md)), providing a hard backstop if the Inspector's self-regulation fails.

### 9.2 Memory Budget

The Inspector maintains tiered caches optimized for its access patterns:

| Data | Strategy | Size | Rationale |
|---|---|---|---|
| Provenance records | LRU cache, most recent 1000 | ~64 KB | Activity feed shows recent records; older records fetched on scroll |
| Capability tokens | Full materialization | ~256 KB (256 tokens x 32 agents) | Always needed for Capability View; small enough to hold entirely |
| Agent metadata | Full materialization | ~32 KB (32 agents) | Always needed for Dashboard; rarely changes |
| Security events | Ring buffer, last 500 | ~32 KB | Live feed; older events in provenance chain |
| AIRS analysis results | On-demand, cached per agent | ~16 KB per agent | Only loaded when user opens AIRS Analysis View |

Total memory budget: under 512 KB resident. The Inspector does not pre-load provenance history beyond the most recent 1000 records. Scrolling or searching triggers on-demand fetches that pass through the query budget rate limiter.

### 9.3 Render Budget

Views refresh at different cadences based on their data freshness requirements:

| View | Refresh rate | Method |
|---|---|---|
| Dashboard | 1 Hz | Kernel event subscription + 1-second polling fallback |
| Security Events | 1 Hz | Kernel event subscription (push-based) |
| Agent View | On demand | Refreshes when user navigates to it |
| Provenance View | On demand | Refreshes on scroll or filter change |
| Capability View | On demand | Refreshes on navigation or after action |
| Profile View | On demand | Refreshes on navigation or after override edit |
| AIRS Analysis View | On demand | Refreshes on navigation |
| Hardware View | 2 Hz | Polling (hardware state changes are infrequent) |

The Inspector does not continuously poll for views that are not visible. When the user switches views, the outgoing view's refresh timer stops and the incoming view's timer starts.

### 9.4 Concurrency Model

The Inspector runs three thread classes:

- **Main thread (UI):** Handles user input, view rendering, and action confirmation flows. Runs on the compositor's frame schedule. Never blocks on query results -- uses async callbacks.
- **Query threads (async pool):** Execute provenance, capability, and agent queries against the kernel syscall interface. Results are delivered to the main thread via an internal message channel. Pool size: 4 threads, matching the query diversity (provenance, capability, agent, security events).
- **Event subscription thread:** Maintains a persistent subscription to kernel security event notifications. Receives push notifications for new security events and anomaly alerts. Forwards to the main thread for display and badge updates.

### 9.5 Scalability Targets

The Inspector handles the following workloads without degradation:

| Dimension | Target | Constraint |
|---|---|---|
| Concurrent agents | 32 | Matches `MAX_PROCESSES` system limit |
| Provenance records | 1M+ total, 1000 cached | On-demand fetch for records outside cache |
| Capability tokens per agent | 256 | Matches `MAX_CAPS_PER_PROCESS` system limit |
| Security events per day | 10,000+ raw, <50 displayed after triage | Kernel-internal alert scoring reduces volume |
| Delegation chains | 8 levels deep | Matches `MAX_INHERITANCE_DEPTH` |
| Simultaneous views | 2 (split-pane) | Multi-view linking supports two concurrent views |

### 9.6 Startup

Cold start completes in under 500 ms. The startup sequence:

1. **Surface allocation** (compositor handshake): <50 ms
2. **Agent metadata + capability token fetch**: <100 ms (small, fully materialized)
3. **Recent provenance cache prime** (1000 records): <200 ms
4. **Security event subscription**: <50 ms
5. **Dashboard render**: <100 ms

The Inspector pre-caches agent metadata and capability tokens at launch. Provenance history beyond the initial 1000 records loads incrementally as the user navigates to the Provenance View or scrolls the activity feed.

-----

## Cross-References

| Topic | Document | Sections |
|---|---|---|
| Capability revocation mechanics | [capabilities.md](../../security/model/capabilities.md) | §3.5 Cascade revocation, §3.6 Temporal capabilities |
| Escalation policy | [layers.md](../../security/model/layers.md) | §2.8 Four-level escalation |
| Audit trail format | [operations.md](../../security/model/operations.md) | §7 Audit subsystem |
| Conversation Bar design | [conversation-bar.md](../../intelligence/conversation-manager/conversation-bar.md) | §9 Bar invocation, §10 Structured output |
| Behavioral baselines | [detection.md](../../intelligence/behavioral-monitor/detection.md) | §4.1 Welford/z-score, §5 Baseline learning |
| Compositor surface lifecycle | [protocol.md](../../platform/compositor/protocol.md) | §3.1 Surface creation, §3.4 Damage tracking |
| Agent manifest format | [agents.md](../agents.md) | §3.1 Installation flow |
