# AIOS Inspector Architecture

## Security & Capability Management Dashboard

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [model.md §7.1](../security/model.md), [agents.md](agents.md), [compositor.md](../platform/compositor.md), [ui-toolkit.md](ui-toolkit.md)

-----

## 1. Core Insight

macOS has Activity Monitor — a window into process CPU, memory, disk, and network activity. It is the user's escape hatch when something feels wrong. But Activity Monitor shows *resources*, not *intent*. You can see that a process uses 80% CPU, but not *why* it accessed your contacts.

AIOS has richer primitives. Every agent action is capability-gated and provenance-recorded. The kernel knows not just *what* an agent consumed, but *what it tried to do*, *what it was denied*, and *which capability tokens authorized each action*. This is fundamentally more information than any traditional OS exposes.

The Inspector surfaces all of it. It is the single place where users see, understand, and control what agents do on their behalf. It is the "Activity Monitor for agent security" — but because AIOS records intent and provenance, not just resource counters, it can show things no traditional monitor can.

-----

## 2. Design Principles

**Transparency over obscurity.** Every security decision the OS makes is visible. No hidden policies, no silent denials that the user never learns about.

**Comprehensible by default, powerful on demand.** The default view is a simple dashboard: which agents are running, what they recently did, are there any alerts. Drilling down reveals capability tokens, provenance chains, profile resolution traces, and AIRS analysis reports.

**No special kernel backdoors.** The Inspector is a regular Trust Level 2 agent ([model.md §1.2](../security/model.md)). Its elevated visibility comes from having `AuditRead(Scope::All)` capability, granted because it is system-shipped and signed by the AIOS root key. It uses the same syscall interface as any agent.

**Non-blocking.** The Inspector never interferes with agent execution. It is a read-heavy, write-light application. The only writes are user-initiated actions: capability revocation, profile override edits, agent pause/resume.

-----

## 3. Agent Identity

```rust
pub const INSPECTOR_AGENT: AgentManifest = AgentManifest {
    bundle_id: "dev.aios.inspector",
    name: "Inspector",
    version: "1.0.0",
    // Trust Level 2: Native experience agent (see model.md §1.2)
    runtime: RuntimeType::Native,
    profiles: vec![
        ProfileReference {
            profile_id: "os.base.v1",
            version_req: ">=1.0",
            required: true,
        },
        ProfileReference {
            profile_id: "runtime.native.v1",
            version_req: ">=1.0",
            required: true,
        },
    ],
    requested_capabilities: vec![
        // Core: full audit visibility
        CapabilityRequest { capability: Capability::AuditRead(Scope::All), justification: "Full audit visibility for security monitoring", required: true },
        // Read capability tables for all agents
        CapabilityRequest { capability: Capability::CapabilityQuery(Scope::All), justification: "Read capability tables for all agents", required: true },
        // Revoke capabilities on user's behalf
        CapabilityRequest { capability: Capability::CapabilityRevoke(Scope::All), justification: "Revoke capabilities on user's behalf", required: true },
        // Read agent metadata and behavioral baselines
        CapabilityRequest { capability: Capability::AgentQuery(Scope::All), justification: "Read agent metadata and behavioral baselines", required: true },
        // Pause/resume agents on user's behalf
        CapabilityRequest { capability: Capability::AgentControl(Scope::All), justification: "Pause/resume agents on user's behalf", required: true },
        // Profile management (Phase 40)
        CapabilityRequest { capability: Capability::ProfileRead(Scope::All), justification: "Read capability profiles and resolution logs", required: false },
        CapabilityRequest { capability: Capability::ProfileWrite(Scope::User), justification: "Manage user override profiles (Layer 90)", required: false },
        // Compositor surface for its own window
        CapabilityRequest { capability: Capability::Compositor(SurfaceType::Window), justification: "Compositor surface for Inspector window", required: true },
        // Read AIRS analysis results (Phase 41)
        CapabilityRequest { capability: Capability::InferenceQuery(Scope::SecurityAnalysis), justification: "Read AIRS security analysis results", required: false },
    ],
};
```

The Inspector's capabilities are broad but bounded. It can *read* everything and *revoke* capabilities, but it cannot *grant* new capabilities, *create* agents, or *write* to agent spaces. It is an auditor, not an administrator.

-----

## 4. Architecture

### 4.1 Component Decomposition

```
┌───────────────────────────────────────────────────────────────────┐
│                        Inspector Agent                            │
│                                                                   │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────────────┐ │
│  │  Dashboard   │  │  Detail      │  │  Action                  │ │
│  │  Controller  │  │  Views       │  │  Handler                 │ │
│  │             │  │              │  │                          │ │
│  │  • Summary  │  │  • Agent     │  │  • Revoke capability     │ │
│  │  • Alerts   │  │  • Provenance│  │  • Pause/resume agent    │ │
│  │  • Status   │  │  • Security  │  │  • Edit user override    │ │
│  │             │  │  • Capability│  │  • Export audit log       │ │
│  │             │  │  • Hardware  │  │  • Acknowledge alert      │ │
│  │             │  │  • Profile   │  │                          │ │
│  │             │  │  • AIRS      │  │                          │ │
│  └──────┬──────┘  └──────┬───────┘  └────────────┬─────────────┘ │
│         │                │                       │               │
│  ┌──────▼────────────────▼───────────────────────▼─────────────┐ │
│  │                    Data Layer                                │ │
│  │                                                             │ │
│  │  ┌─────────────┐ ┌──────────────┐ ┌───────────────────────┐ │ │
│  │  │ Provenance  │ │ Capability   │ │ Agent                 │ │ │
│  │  │ Query       │ │ Query        │ │ Query                 │ │ │
│  │  │ Engine      │ │ Engine       │ │ Engine                │ │ │
│  │  └──────┬──────┘ └──────┬───────┘ └──────────┬────────────┘ │ │
│  └─────────┼───────────────┼────────────────────┼──────────────┘ │
└────────────┼───────────────┼────────────────────┼────────────────┘
             │               │                    │
    ─────────▼───────────────▼────────────────────▼────────
             Kernel syscall interface (AuditRead, CapabilityQuery, etc.)
    ────────────────────────────────────────────────────────
```

### 4.2 Data Sources

The Inspector reads from these kernel-managed data sources:

| Source | Syscall | Contents |
|---|---|---|
| Provenance chain | `AuditRead` | Merkle-chained action records (agent, action, target, result, timestamp) |
| Capability table | `CapabilityQuery` | Active capability tokens per agent (type, scope, expiry, delegation chain) |
| Agent registry | `AgentQuery` | Agent metadata, trust level, runtime, behavioral baseline, anomaly score |
| Security events | `AuditRead(filter: security)` | Real-time feed of denials, anomalies, injection attempts, hardware violations |
| Profile store | `ProfileRead` | Installed capability profiles, resolution logs ([model.md §3.7](../security/model.md)) |
| AIRS analysis | `InferenceQuery` | SecurityAnalysis results for installed agents ([airs.md §5.9](../intelligence/airs.md)) |

All reads are non-blocking. The provenance chain is append-only and immutable — the Inspector can never modify it.

-----

## 5. Views

The Inspector presents eight views, each corresponding to a distinct concern. The user navigates between them via a sidebar or tab bar.

### 5.1 Dashboard (Default View)

The landing page. Shows system-wide security posture at a glance.

```
┌──────────────────────────────────────────────────────────────────┐
│  Inspector — Dashboard                                           │
│                                                                  │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │  7 Agents   │  │  0 Alerts   │  │  142 Actions │             │
│  │  Running    │  │  ✓ All Clear│  │  Today       │             │
│  └─────────────┘  └─────────────┘  └─────────────┘             │
│                                                                  │
│  Recent Activity                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  12:04  Research Assistant  read  research/papers/ml.pdf │   │
│  │  12:03  Email Agent         net   imap.gmail.com         │   │
│  │  12:01  Budget Tracker      read  finances/budget/q1     │   │
│  │  11:58  Code Editor         write workspace/src/main.rs  │   │
│  │  11:55  Research Assistant  net   api.anthropic.com      │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Alerts (last 24h)                                               │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  (none)                                                   │   │
│  └──────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

**Data**: Aggregates from all query engines. Activity feed is the most recent N provenance records. Alert count is unacknowledged security events of severity >= Medium.

### 5.2 Agent View

Per-agent deep dive. Select an agent from a list to see everything about it.

```
┌──────────────────────────────────────────────────────────────────┐
│  Inspector — Agent: Research Assistant                            │
│                                                                  │
│  Trust Level: 3 (Third-party)     Runtime: WASM                  │
│  Installed: 2025-01-15            Author: research-tools.dev     │
│  Anomaly Score: 0.12 (normal)     Status: Running                │
│                                                                  │
│  Capabilities (4 active)                              [Revoke ▾] │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  SpaceRead    research/*              expires: never      │   │
│  │  SpaceWrite   research/papers/*       expires: never      │   │
│  │  Network      api.anthropic.com       expires: never      │   │
│  │  Network      arxiv.org               expires: 24h        │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Behavioral Baseline                                             │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Reads/day:  avg 34  today 41  ▓▓▓▓▓▓▓▓░░ (within 2σ)  │   │
│  │  Writes/day: avg 8   today 5   ▓▓▓▓░░░░░░ (normal)      │   │
│  │  Net calls:  avg 12  today 15  ▓▓▓▓▓▓▓░░░ (within 2σ)  │   │
│  │  Inference:  avg 6   today 8   ▓▓▓▓▓▓░░░░ (normal)      │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Recent Actions (provenance)                                     │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  12:04  read   research/papers/ml.pdf         ✓ allowed  │   │
│  │  12:03  read   research/papers/transformers   ✓ allowed  │   │
│  │  12:01  net    api.anthropic.com/v1/messages  ✓ allowed  │   │
│  │  11:58  write  research/papers/summary.md     ✓ allowed  │   │
│  │  11:42  read   user/documents/notes.md        ✗ DENIED   │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  [Pause Agent]  [Revoke All Capabilities]  [View Full Provenance]│
└──────────────────────────────────────────────────────────────────┘
```

**Key interaction**: Clicking a denied action shows *why* it was denied (which layer blocked it, which capability was missing). Clicking "Revoke" on a specific capability token shows a confirmation dialog and immediately revokes via `CapabilityRevoke` syscall.

### 5.3 Provenance View

Full Merkle chain browser. The forensic view for investigating what happened and when.

```
┌──────────────────────────────────────────────────────────────────┐
│  Inspector — Provenance                                          │
│                                                                  │
│  Filter: [All agents ▾] [All actions ▾] [Last 24h ▾] [Search…]  │
│                                                                  │
│  Timeline                                                        │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  ──●──●──●──●────●──●────────●──●──●──●──●──            │   │
│  │  9am       10am       11am       12pm                    │   │
│  │  ▲ density = action frequency                            │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Records                                                         │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  #4521  12:04:33  Research Assistant  read               │   │
│  │         target: research/papers/ml.pdf                    │   │
│  │         result: allowed                                   │   │
│  │         capability: tok_a3f2 (SpaceRead research/*)       │   │
│  │         prev_hash: 0x8a3f…  this_hash: 0x2b7c…           │   │
│  │                                                           │   │
│  │  #4520  12:03:17  Email Agent  net                        │   │
│  │         target: imap.gmail.com:993                         │   │
│  │         result: allowed                                   │   │
│  │         capability: tok_b712 (Network imap.gmail.com)     │   │
│  │         prev_hash: 0x7e21…  this_hash: 0x8a3f…           │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Chain Integrity: ✓ Verified (4521 records, no breaks)           │
│                                                                  │
│  [Export…]                                                        │
└──────────────────────────────────────────────────────────────────┘
```

**Key feature**: Chain integrity verification. The Inspector can walk the Merkle chain and confirm no records have been tampered with. A broken chain triggers an alert.

### 5.4 Security Events View

Real-time feed of security-relevant events, filtered by severity.

```
┌──────────────────────────────────────────────────────────────────┐
│  Inspector — Security Events                                     │
│                                                                  │
│  Filter: [All severities ▾] [All agents ▾] [Live ◉]             │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  ⚠ MEDIUM  11:42  Research Assistant                      │   │
│  │  Capability violation: read user/documents/notes.md       │   │
│  │  Missing capability: SpaceRead(user/documents)            │   │
│  │  Blocked at: Layer 1 (capability check)                   │   │
│  │  [Acknowledge]  [View Agent]  [View Provenance]           │   │
│  │                                                           │   │
│  │  ● LOW     10:15  Budget Tracker                          │   │
│  │  Expired capability used: Network(plaid.com)              │   │
│  │  Token tok_c891 expired at 10:00                          │   │
│  │  Auto-cleaned. No action needed.                          │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Event Stats (24h)                                               │
│  Critical: 0  High: 0  Medium: 1  Low: 3                        │
└──────────────────────────────────────────────────────────────────┘
```

**Response levels**: Maps to the 4-level escalation policy defined in [model.md §6.3](../security/model.md). Critical events auto-open the Inspector (Level 4 response).

### 5.5 Capability View

System-wide view of all active capability tokens across all agents.

```
┌──────────────────────────────────────────────────────────────────┐
│  Inspector — Capabilities                                        │
│                                                                  │
│  Total tokens: 23    Delegated: 2    Expiring <1h: 1             │
│                                                                  │
│  By Agent                                                        │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Research Assistant     4 tokens   ▓▓▓▓                  │   │
│  │  Email Agent            5 tokens   ▓▓▓▓▓                 │   │
│  │  Code Editor            6 tokens   ▓▓▓▓▓▓                │   │
│  │  Budget Tracker         3 tokens   ▓▓▓                   │   │
│  │  Browser Shell          5 tokens   ▓▓▓▓▓                 │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  By Type                                                         │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  SpaceRead     8   SpaceWrite   4   Network     5        │   │
│  │  Compositor    3   Inference    2   AgentControl 1       │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Delegation Chains                                               │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  tok_d412: Browser Shell → Tab Agent (site-a)             │   │
│  │           Network(site-a.com), attenuated: read-only      │   │
│  │  tok_e523: Code Editor → Language Server                  │   │
│  │           SpaceRead(workspace/src/*), non-delegatable     │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  [Revoke Selected]  [Export Token Report]                         │
└──────────────────────────────────────────────────────────────────┘
```

### 5.6 Profile View (Phase 40+)

Capability profile management. Shows how profiles compose into resolved capability sets.

```
┌──────────────────────────────────────────────────────────────────┐
│  Inspector — Capability Profiles                                 │
│                                                                  │
│  Installed Profiles: 8                                           │
│                                                                  │
│  System Profiles                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Layer 0   OS Base          v1.0.0  grants: 3  denials: 0│   │
│  │  Layer 10  Native Runtime   v1.0.0  grants: 5  denials: 1│   │
│  │  Layer 10  WASM Runtime     v1.0.0  grants: 3  denials: 2│   │
│  │  Layer 30  Network Subsys   v1.0.0  grants: 2  denials: 0│   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Agent: Research Assistant — Profile Resolution                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Layer 0:  OS Base           → 3 grants                   │   │
│  │  Layer 10: WASM Runtime      → 3 grants, 2 denials        │   │
│  │  Layer 50: Agent manifest    → 4 grants                   │   │
│  │  Layer 90: User override     → (none)                     │   │
│  │  ─────────────────────────────────────────────────────    │   │
│  │  Resolved: 8 grants, 2 denials → 6 effective capabilities │   │
│  │                                                           │   │
│  │  Resolution trace:                                        │   │
│  │  ✓ SpaceRead(research/*) — granted by Layer 50            │   │
│  │  ✓ Network(arxiv.org) — granted by Layer 50               │   │
│  │  ✗ SpaceRead(system/*) — denied by Layer 10 (WASM deny)  │   │
│  │  ✗ RawDevice(*) — denied by Layer 10 (WASM deny)         │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  User Overrides (Layer 90)                                       │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  (No overrides configured for this agent)                 │   │
│  │  [Add Override…]                                          │   │
│  └──────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

**Key interaction**: The user can add Layer 90 overrides (deny or attenuate) to any agent's resolved set. Overrides are stored in `user/preferences/capability-overrides/` ([model.md §3.7.7](../security/model.md)).

The Profile View implements the visual equivalent of `aios agent audit --show-resolution` — showing exactly how each layer contributes to the final capability set.

### 5.7 AIRS Analysis View (Phase 41+)

Displays AIRS capability intelligence results for installed agents.

```
┌──────────────────────────────────────────────────────────────────┐
│  Inspector — AIRS Analysis: Research Assistant                    │
│                                                                  │
│  Analysis Confidence: 0.87           Last analyzed: 2h ago       │
│                                                                  │
│  Capability Assessment                                           │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  ✓ SpaceRead(research/*)      — matches code behavior     │   │
│  │  ✓ Network(api.anthropic.com) — API client detected       │   │
│  │  ✓ Network(arxiv.org)         — HTTP fetch in code        │   │
│  │  ⚠ SpaceWrite(research/*)     — broader than code needs   │   │
│  │    Suggestion: narrow to research/papers/* only            │   │
│  │  ✗ Inference(normal)          — not declared in manifest   │   │
│  │    Suggestion: add — code calls AIRS inference API         │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Behavioral Prediction                                           │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Predicted access: research/papers/ (high), arxiv.org     │   │
│  │  (medium), api.anthropic.com (high)                       │   │
│  │  Resource usage: moderate memory, low CPU, moderate net    │   │
│  │  Risk factors: none detected                              │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Corpus Comparison                                               │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Similar agents: 12 in corpus                             │   │
│  │  Capability set: typical for research-assistant category   │   │
│  │  Outliers: none                                           │   │
│  │  Corpus risk score: 0.15 (low)                            │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Recommendations                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  1. Narrow SpaceWrite to research/papers/* [Apply]        │   │
│  │  2. Add Inference(normal) capability     [Apply]          │   │
│  │  3. Consider using wasm-research profile [Apply Profile]  │   │
│  └──────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

**Key interaction**: "Apply" buttons translate AIRS recommendations into concrete actions — adding a user override, modifying the manifest, or switching to a suggested profile. Each action goes through the standard capability change confirmation flow.

### 5.8 Hardware View

Cross-subsystem audit of hardware access patterns.

```
┌──────────────────────────────────────────────────────────────────┐
│  Inspector — Hardware Access                                     │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  🎤 Microphone    No active sessions                      │   │
│  │  📷 Camera        No active sessions                      │   │
│  │  📍 Location      Email Agent (last: 2h ago, 1 request)   │   │
│  │  🌐 Network       4 agents active                         │   │
│  │  💾 Storage       3 agents (12 reads, 4 writes today)     │   │
│  │  🖥 GPU           Browser Shell (2 tabs), Code Editor     │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Network Connections (live)                                       │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Email Agent        → imap.gmail.com:993    TLS ✓  active│   │
│  │  Research Assistant → api.anthropic.com:443 TLS ✓  idle  │   │
│  │  Browser (tab-1)   → github.com:443        TLS ✓  active│   │
│  │  Browser (tab-2)   → docs.rs:443           TLS ✓  active│   │
│  └──────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

-----

## 6. User Actions

The Inspector is primarily a read-only dashboard, but it exposes controlled write operations for security management. Every action requires user confirmation.

| Action | Syscall | Confirmation | Reversible |
|---|---|---|---|
| Revoke capability token | `CapabilityRevoke` | "Remove [agent]'s access to [resource]?" | Agent must re-request |
| Revoke all agent capabilities | `CapabilityRevoke(agent, all)` | "Remove all permissions for [agent]? It will not be able to function." | Agent must be re-authorized |
| Pause agent | `AgentControl(pause)` | "Pause [agent]? It will stop all activity." | Resume available |
| Resume agent | `AgentControl(resume)` | Immediate (no confirmation for resume) | — |
| Add user override (Layer 90) | `ProfileWrite(User)` | "Add denial for [capability] on [agent]?" | Override can be removed |
| Remove user override | `ProfileWrite(User)` | "Remove your override? Agent will regain [capability]." | Can re-add |
| Apply AIRS recommendation | Varies per recommendation | Shows specific effect before confirming | Depends on action type |
| Export audit log | `AuditRead` + local file write | "Export [N] records to [path]?" | File can be deleted |
| Acknowledge alert | `AuditRead(acknowledge)` | Immediate (clears badge) | Alert remains in history |

-----

## 7. Conversation Bar Integration

The Inspector works in concert with AIRS and the Conversation Bar ([model.md §7.2](../security/model.md)). Natural language security queries route through AIRS, which queries the same data sources the Inspector uses. The Conversation Bar is the *conversational* interface; the Inspector is the *visual* interface. Both show the same data.

```
User: "What has the Budget Tracker been doing?"

→ AIRS queries provenance chain (same AuditRead the Inspector uses)
→ Conversation Bar: "Budget Tracker read 15 objects from finances/budget
   today. It made 3 API calls to plaid.com. No anomalies detected."
→ The response includes a link: "Open in Inspector →"
→ Clicking opens Inspector's Agent View filtered to Budget Tracker
```

When AIRS detects an anomaly and pauses an agent, the notification includes "Review in Inspector" which opens directly to that agent's view with the relevant security event highlighted.

-----

## 8. Auto-Open Triggers

The Inspector normally runs in the background (no visible window). It auto-opens in these scenarios:

| Trigger | Severity | Inspector Behavior |
|---|---|---|
| Level 4 incident (chain integrity, suspected compromise) | Critical | Opens immediately, full-screen, blocks until acknowledged |
| Level 3 incident (agent terminated, spaces locked) | Critical | Opens at next user interaction, Agent View focused |
| Repeated anomalies from same agent | High | Badge on dock/taskbar icon; opens if user clicks |
| User says "open Inspector" or "show security" | — | Opens Dashboard view |
| Agent installation (AIRS analysis complete) | — | Opens AIRS Analysis View for the new agent |

-----

## 9. Performance Characteristics

The Inspector must never degrade system performance. It is a monitoring tool — if monitoring itself becomes expensive, it defeats the purpose.

**Query budget**: The Inspector limits itself to 100 provenance queries/second and 10 capability table scans/second. These are soft limits — the kernel rate-limits audit reads independently.

**Memory budget**: The Inspector maintains an in-memory cache of the most recent 1000 provenance records and all active capability tokens. Older records are fetched on-demand when the user scrolls or searches.

**Render budget**: Views refresh at 1 Hz for the Dashboard and Security Events (live feeds), and on-demand for detail views. The Inspector does not continuously poll — it subscribes to kernel event notifications where available, falling back to 1-second polling.

**Startup**: Cold start < 500ms. The Inspector pre-caches agent metadata and capability tokens at launch. Provenance history loads incrementally as the user navigates.

-----

## 10. Relationship to Existing Docs

The Inspector is referenced throughout the security architecture but defined in detail here. Cross-reference map:

| Document | Section | Inspector Role |
|---|---|---|
| [model.md §1.2](../security/model.md) | Trust Boundaries | Listed as Trust Level 2 native experience agent |
| [model.md §6.3](../security/model.md) | Escalation Policy | Level 4 auto-opens Inspector; user reviews in Inspector |
| [model.md §7.1](../security/model.md) | Inspector (overview) | 5-view summary — **this doc supersedes with full architecture** |
| [model.md §7.2](../security/model.md) | Conversation Bar | Inspector linked from AIRS natural language responses |
| [model.md §3.7](../security/model.md) | Composable Profiles | Inspector Profile View shows resolution traces |
| [airs.md §5.9](../intelligence/airs.md) | Capability Intelligence | Inspector AIRS View displays analysis pipeline results |
| [agents.md §3.1](../applications/agents.md) | Installation Flow | Inspector opens after AIRS analysis of new agent |

-----

## 11. Implementation Phases

The Inspector is built incrementally across multiple phases:

| Phase | Capabilities Added | Views Available |
|---|---|---|
| Phase 17 (Security Architecture) | Core Inspector: AuditRead, CapabilityQuery, AgentQuery, CapabilityRevoke, AgentControl | Dashboard, Agent, Provenance, Security Events, Capability, Hardware |
| Phase 40 (Composable Profiles) | ProfileRead, ProfileWrite(User) | + Profile View with resolution trace and user overrides |
| Phase 41 (AIRS Intelligence) | InferenceQuery(SecurityAnalysis) | + AIRS Analysis View with recommendations and "Apply" actions |

The Phase 17 Inspector is fully functional for security monitoring and management. Phases 40 and 41 add profile management and AI-assisted analysis as progressive enhancements — they do not change the core architecture.

-----

## 12. Comparison: AIOS Inspector vs. macOS Activity Monitor

| Dimension | macOS Activity Monitor | AIOS Inspector |
|---|---|---|
| **Shows** | CPU, memory, disk, network per process | Capabilities, actions, provenance, behavioral baselines per agent |
| **Granularity** | Process-level resource counters | Action-level intent records (what was accessed, why, by whom) |
| **Denials** | Not shown (kernel denials are invisible to users) | Every denial visible with reason and blocking layer |
| **History** | None (live snapshot only) | Full provenance chain (Merkle-linked, tamper-evident) |
| **User control** | Kill process (all-or-nothing) | Revoke specific capabilities, pause agent, add overrides |
| **AI analysis** | None | AIRS behavioral prediction, corpus comparison, profile suggestions |
| **Composition** | N/A | Profile resolution trace showing how layers combine |
| **Integrity** | None | Merkle chain verification, tamper detection |
| **Conversational** | None | Linked from Conversation Bar natural language queries |

The Inspector shows *intent* where Activity Monitor shows *resources*. This is the fundamental difference between an agent-aware OS and a process-based OS.

-----

## 13. Comparison: AIOS Inspector vs. Agent Safehouse

[Agent Safehouse](https://agent-safehouse.dev/) provides sandbox policy management for LLM coding agents on macOS, using `sandbox-exec` profiles. The AIOS Inspector provides analogous functionality but operates at the OS level rather than as an application-level wrapper.

| Dimension | Agent Safehouse | AIOS Inspector |
|---|---|---|
| **Sandbox enforcement** | macOS `sandbox-exec` (application-level) | Kernel capability table (OS-level) |
| **Policy format** | SBPL numbered layers (00-base, 30-toolchains, 60-agents) | Capability profiles with 5 named layers (OsBase through UserOverride) |
| **Deny semantics** | Later rules can override earlier denials | Deny-always-wins across all layers |
| **Visualization** | CLI output, log files | Native GUI with 8 interactive views |
| **Audit trail** | Log files (mutable) | Merkle-chained provenance (tamper-evident) |
| **AI analysis** | None | AIRS 5-stage pipeline with behavioral prediction |
| **User overrides** | Edit SBPL files manually | Layer 90 visual editor in Profile View |
| **Runtime control** | Process kill | Granular: revoke single capability, pause, resume |
| **Scope** | Coding agents only | All agents (any category, any runtime) |
| **Deployment** | Third-party tool installed per-agent | Ships with the OS; automatic for all agents |

The AIOS Inspector subsumes Agent Safehouse's functionality. An Agent Safehouse-equivalent on AIOS would be the Inspector's Profile View + Capability View + AIRS Analysis View — all of which are built into the OS rather than bolted on.
