# AIOS Inspector — Testing & Accessibility

Part of: [inspector.md](../inspector.md) — Inspector Architecture
**Related:** [views.md](./views.md) — Views, [threat-model.md](./threat-model.md) — Threat Model, [intelligence.md](./intelligence.md) — Intelligence

-----

## 18. Testing Strategy

### 18.1 Unit Tests

Unit tests validate each Inspector component in isolation, using mock data sources that simulate kernel syscall responses without requiring a running kernel.

**Query engine tests:**

- `AuditQuery` with various filter combinations (agent ID, time range, event type, severity) returns correctly filtered subsets from a pre-populated mock provenance chain
- Pagination: requesting page N of size M returns exactly M records starting at offset N*M, and the `has_more` flag is correct for the final page
- Merkle verification: given a provenance record and its Merkle proof, `verify_proof()` returns `true` for valid proofs and `false` for proofs with a single bit flipped in any hash
- Empty result sets: queries with impossible filters (future timestamps, nonexistent agent IDs) return empty results without error

**Capability snapshot diff:**

- Given two `CapabilitySnapshot` values taken at different times, `diff_snapshots()` produces the correct `Added`, `Removed`, and `Modified` change sets
- Attenuation changes (same capability token with narrowed scope) are classified as `Modified`, not `Removed + Added`
- Temporal capability expiry is classified as `Expired` rather than `Removed`
- Empty diff between identical snapshots produces zero changes

**Alert scoring:**

- The priority scoring decision tree assigns `Critical` to provenance chain integrity failures
- Capability denials for sensitive resources (camera, microphone, location) score higher than filesystem denials
- Repeated low-severity events from the same agent within a burst window are consolidated into a single medium-severity alert
- Events matching the agent's behavioral baseline score lower than events outside the baseline envelope

**Heatmap computation:**

- EMA matrix updates correctly: a new data point with weight alpha produces `new_value = alpha * sample + (1 - alpha) * old_value` for the relevant cell
- Matrix cells with no recent activity decay toward zero over successive update cycles
- Row and column labels (agent ID x capability type) remain consistent across updates
- Sparse matrix representation correctly handles agents with zero activity in most capability categories

**Temporal pattern detection:**

```rust
#[test]
fn detect_burst_pattern() {
    let events = generate_events_with_burst(
        agent_id: AgentId(42),
        burst_count: 50,
        burst_window_ms: 100,
        baseline_rate_per_sec: 1,
    );
    let patterns = detect_temporal_patterns(&events);
    assert!(patterns.contains(&Pattern::Burst {
        agent: AgentId(42),
        rate_multiplier: 500, // 50 in 100ms = 500/sec vs 1/sec baseline
    }));
}

#[test]
fn detect_periodicity() {
    let events = generate_periodic_events(
        agent_id: AgentId(7),
        interval_ms: 60_000,
        count: 10,
        jitter_ms: 500,
    );
    let patterns = detect_temporal_patterns(&events);
    assert!(patterns.contains(&Pattern::Periodic {
        agent: AgentId(7),
        interval_ms: 60_000,
        confidence: 0.95,
    }));
}

#[test]
fn detect_correlation() {
    // Agent A reads contacts, then Agent B sends network data within 200ms, 5 times
    let events = generate_correlated_events(
        agent_a: AgentId(3),
        action_a: Action::SpaceRead("contacts"),
        agent_b: AgentId(9),
        action_b: Action::NetworkSend,
        delay_ms: 200,
        repetitions: 5,
    );
    let patterns = detect_temporal_patterns(&events);
    assert!(patterns.contains(&Pattern::Correlation {
        agents: (AgentId(3), AgentId(9)),
        confidence: 0.90,
    }));
}
```

-----

### 18.2 Integration Tests

Integration tests verify that the Inspector's components work together correctly, including cross-view consistency, concurrent access, and end-to-end action flows.

**Multi-view consistency:**

Selecting an agent in the Dashboard view triggers a `FocusAgent` event. The test verifies:

1. Agent View scrolls to and highlights the selected agent's detail panel
2. Capability View filters to show only that agent's capabilities
3. Provenance View applies a time-range filter centered on the agent's most recent activity
4. Hardware View highlights resource rows associated with the selected agent
5. Deselecting the agent in any view clears the selection in all views

**Query engine concurrency:**

- Spawn 9 tasks (one per view), each issuing queries at 10 Hz for 5 seconds
- Verify all queries return valid results (no partial reads, no corrupted records)
- Verify the query cache produces correct results: a cache hit for the same query parameters returns data identical to a cache miss
- Verify cache invalidation: when a new provenance record arrives, subsequent queries include it

**Action confirmation flow:**

```text
Test: capability_revoke_with_undo

1. Select agent "test-agent" in Agent View
2. Click "Revoke" on SpaceRead("photos") capability
3. Verify confirmation dialog appears with:
   - Agent name and capability description in human-readable form
   - Warning about consequences ("test-agent will no longer be able to read your photos")
   - "Revoke" and "Cancel" buttons
4. Click "Revoke"
5. Verify capability no longer appears in CapabilityQuery results for test-agent
6. Verify undo banner appears with 30-second countdown
7. Trigger undo within the window
8. Verify capability is restored in CapabilityQuery results
9. Wait 30 seconds without undo
10. Repeat revocation
11. Verify undo window expires and revocation is permanent
```

**Event subscription:**

- Register Inspector as a subscriber for security events via the kernel event channel
- From a separate test agent, trigger a capability denial (attempt an unauthorized SpaceRead)
- Measure the time from kernel denial to Inspector displaying the event in Security Events view
- Assert latency is below 1 second under normal load (fewer than 100 events/second)
- Verify the displayed event includes: agent ID, denied capability, denial reason, timestamp, and the specific security layer that blocked it

-----

### 18.3 Security Red-Team Tests

Red-team tests validate that the Inspector correctly handles adversarial scenarios and cannot be deceived or subverted by malicious agents.

**Provenance spoofing:**

- A Trust Level 3 agent attempts to write a fabricated provenance record via `AuditWrite` syscall
- Verify the kernel rejects the write (only the kernel appends to the provenance chain)
- A compromised agent attempts to replay a valid provenance record with a modified payload but identical Merkle hash
- Verify the Inspector detects the hash mismatch and raises a chain integrity alert

**UI confusion attacks:**

- A Trust Level 3 agent requests a compositor surface with the Inspector's trust-level border color (blue, indicating TL2)
- Verify the compositor enforces trust-level-appropriate borders regardless of the agent's request ([compositor security §10.5](../../platform/compositor/security.md))
- A malicious agent creates a window visually mimicking the Inspector's confirmation dialog
- Verify the compositor's trust-level indicator distinguishes the fake dialog from the real Inspector

**Audit flooding:**

- A test agent generates 10,000 audit events per second for 30 seconds
- Verify the Inspector remains responsive: frame rate stays above 30 fps, input latency stays below 100ms
- Verify alert fatigue mitigation activates: the flooding agent's events are consolidated rather than displayed individually
- Verify the flooding agent itself triggers a behavioral anomaly alert (burst pattern detection)
- Verify that legitimate alerts from other agents are not suppressed by the flood

**Inspector compromise simulation:**

- Simulate a scenario where the Inspector agent crashes or is suppressed
- Verify the kernel audit subsystem continues recording provenance independently
- Verify an alternative access path exists: kernel audit data is exportable via a recovery shell or secondary diagnostic agent
- Verify the kernel detects the Inspector's absence and logs a system health event

**Toxic combination injection:**

- Create three agents, each with individually benign capabilities:
  - Agent A: `SpaceRead("contacts")`
  - Agent B: `NetworkSend(destination: "*.example.com")`
  - Agent C: `IpcSend(target: AgentB)`
- Verify the Inspector's toxic combination detection flags the chain A-reads-contacts -> C-relays-to-B -> B-exfiltrates as a potential collusion vector
- Verify the alert includes all three agents and the specific data flow path

-----

### 18.4 QEMU Validation

QEMU tests verify end-to-end Inspector behavior on the target platform, using the standard `just run` boot sequence.

**Boot and capability acquisition:**

```text
Expected UART output:
  [inspector] Registered with AuditRead(Scope::All)
  [inspector] Registered with CapabilityQuery(Scope::All)
  [inspector] Dashboard ready
```

- Boot the system and verify the Inspector agent starts automatically
- Verify it obtains `AuditRead(Scope::All)` capability via the standard capability grant flow
- Verify it is registered as a Trust Level 2 agent

**Provenance display:**

- Generate 10 test provenance records from a test agent (file reads, capability checks, IPC calls)
- Verify all 10 records appear in the Dashboard's recent activity feed within 1 second
- Verify each record displays: agent name, action description, timestamp, and outcome (allowed/denied)

**Security event auto-open:**

- With the Inspector in background, trigger a security event matching the auto-open trigger table ([actions.md §8](./actions.md))
- Verify the Inspector's window is brought to foreground by the compositor
- Verify the Security Events view is auto-selected with the triggering event highlighted

**Merkle chain integrity (clean):**

- Boot the system, let it accumulate 100+ provenance records
- Trigger the Inspector's chain integrity check
- Verify it reports: "Chain integrity: verified, N records, no breaks"

**Merkle chain integrity (corrupted):**

- Boot the system with a pre-corrupted audit chain (one record with an intentionally wrong hash)
- Trigger the Inspector's chain integrity check
- Verify it detects the corruption and reports the specific record index where the chain breaks
- Verify a Level 4 alert is raised

-----

### 18.5 Performance Tests

Performance tests verify that the Inspector meets its resource budgets and latency targets under realistic workloads.

**Query latency under load:**

| Scenario | Target | Measurement |
|---|---|---|
| 32 agents, 1M provenance records, unfiltered query | < 50ms | Time from `AuditQuery` issue to first result byte |
| Filtered query (single agent, 1-hour window) | < 10ms | Time from query issue to complete result set |
| Capability snapshot for all 32 agents | < 20ms | Time to build full snapshot from `CapabilityQuery` |
| Merkle proof verification (80M record chain) | < 5ms | Time to verify a single logarithmic proof (~3 KB) |

**Rendering frame rate:**

- All 9 views active simultaneously (multi-view layout) with 32 agents generating events at 10 events/second each
- Target: sustained 60 fps with no frame drops lasting more than 2 consecutive frames
- Measurement: compositor frame timing telemetry over a 60-second window

**Memory budget:**

| Component | Budget | Measurement |
|---|---|---|
| Cached provenance records | 1000 records maximum | Count records in LRU cache |
| Capability token snapshots | 32 agents x 256 tokens maximum | Measure snapshot memory |
| Heatmap EMA matrix | 32 agents x 16 capability types | Verify matrix fits in 2 KiB |
| Alert queue | 256 entries maximum | Count entries in ring buffer |
| Total Inspector heap | < 8 MiB | Measure via `MemoryQuery` syscall |

**Cold start time:**

- From Inspector agent spawn to Dashboard rendering its first frame
- Target: < 500ms on QEMU with 2 GiB RAM, 4 cores
- Breakdown budget: agent init (50ms) + capability acquisition (100ms) + initial query (100ms) + first render (250ms)

-----

## 19. Accessibility

The Inspector is a security-critical application. Accessibility is not optional — users who rely on assistive technology must have full access to every security signal and every action the Inspector provides.

Cross-reference: [accessibility.md](../../experience/accessibility.md), [experience.md §15](../../experience/experience.md)

### 19.1 Screen Reader Support

All views expose accessibility tree nodes with semantic labels following the AIOS accessibility tree protocol ([system-integration.md §9](../../experience/accessibility/system-integration.md)).

**View announcements:**

- Dashboard: "Security Dashboard. [N] agents active. [M] alerts requiring attention. Last event: [description] [time ago]."
- Provenance timeline: "Timeline from [start time] to [end time]. [N] events displayed. [M] alerts. Use arrow keys to navigate events."
- Security Events: "[N] security events. [M] unacknowledged. Highest severity: [level]. Press Enter to view details."

**Event announcements:**

- Security events are announced with severity level first for rapid triage: "Critical alert: agent [name] violated capability [description]"
- Capability changes: "Capability [action]: [agent name] [gained/lost] [capability description]"
- Provenance records: "[agent name] [action description] at [time]. Outcome: [allowed/denied]."

**Agent list:**

- Announced with current sort order: "Agent list, sorted by [field], [ascending/descending]. [N] agents."
- Each agent row: "[name], trust level [N], [status]. [M] active capabilities. Anomaly score: [value]."
- Sort changes are announced: "Sorted by anomaly score, descending."

**Action confirmations:**

- Full dialog content is read including consequences: "Confirm revocation. Revoking [capability] from [agent]. This agent will no longer be able to [human-readable consequence]. Press Enter to confirm or Escape to cancel. Undo available for 30 seconds after confirmation."

-----

### 19.2 Keyboard Navigation

All views are fully navigable via keyboard. No Inspector functionality requires a pointing device.

**Navigation keys:**

| Key | Action |
|---|---|
| Tab / Shift+Tab | Move focus between UI regions (view panels, action bar, alert list) |
| Arrow Up/Down | Navigate items within a list (agents, events, capabilities) |
| Arrow Left/Right | Navigate timeline, expand/collapse tree nodes |
| Enter | Activate focused element (open detail, confirm action) |
| Escape | Dismiss dialog, cancel action, return focus to previous element |
| Ctrl+1 through Ctrl+9 | Switch directly to view 1 (Dashboard) through view 9 (Multi-Device) |
| Ctrl+F | Open query/filter bar for the active view |
| Ctrl+Shift+A | Jump to highest-severity unacknowledged alert |

**Security action shortcuts:**

| Shortcut | Action | Requires Confirmation |
|---|---|---|
| Ctrl+R | Revoke focused capability | Yes — confirmation dialog |
| Ctrl+P | Pause focused agent | Yes — confirmation dialog |
| Ctrl+U | Resume (unpause) focused agent | No |
| Ctrl+Z | Undo last action (within undo window) | No |

**Focus management:**

- When Security Events view opens (manually or via auto-open), focus moves to the highest-severity unacknowledged alert
- After completing an action (revoke, pause), focus returns to the element that was focused before the confirmation dialog appeared
- When a new critical alert arrives while the user is in another view, an aria-live region announces it without stealing focus

-----

### 19.3 Color-Blind-Safe Alert Visualization

All severity indicators use redundant encoding: color, shape, icon, and text label. No security information is conveyed through color alone.

**Alert severity indicators:**

| Severity | Color | Shape + Icon | Text Label |
|---|---|---|---|
| Critical | Red (#D32F2F) | Circle with exclamation mark + screen flash | "CRITICAL" |
| High | Orange (#F57C00) | Triangle with exclamation mark | "HIGH" |
| Medium | Yellow (#FBC02D) | Diamond with question mark | "MEDIUM" |
| Low | Blue (#1976D2) | Square with info icon | "LOW" |

**Trust-level borders:**

| Trust Level | Color | Pattern |
|---|---|---|
| TL1 (System) | Gold (#FFD700) | Solid border, 3px |
| TL2 (Native) | Blue (#1976D2) | Solid border, 2px |
| TL3 (Sandboxed) | Green (#4CAF50) | Dashed border, 2px |
| TL4 (Untrusted) | Red (#D32F2F) | Dotted border, 2px |

**Charts and heatmaps:**

- All color palettes use the Okabe-Ito palette (8 colors distinguishable under all common forms of color vision deficiency)
- Heatmap cells include numeric values displayed on hover/focus, not just color intensity
- Graph edges use distinct line styles (solid, dashed, dotted, dash-dot) in addition to colors

**High contrast mode:**

- Available as a system-wide preference ([preferences.md](../../intelligence/preferences.md))
- Inspector responds to high contrast mode by: increasing border widths to 4px, using pure black/white backgrounds, and increasing text contrast ratios to 7:1 minimum (WCAG AAA)

-----

### 19.4 Trust Level Audio Cues

Optional audio indicators provide non-visual feedback for trust level transitions and security events. These are supplementary — they never replace visual or screen reader announcements.

**Trust level transition sounds:**

| Event | Sound | Description |
|---|---|---|
| Agent elevated (TL increased) | Ascending two-tone chime | Brief, non-intrusive, positive valence |
| Agent restricted (TL decreased) | Descending two-tone chime | Brief, attention-drawing, neutral valence |
| Critical alert | Distinct alert tone | Urgent, repeats once, high priority |
| High alert | Single alert tone | Moderately urgent, plays once |
| Chain integrity failure | Alarm sequence | Three short pulses, demands immediate attention |

**Configuration:**

- Audio cues are disabled by default and enabled via Preferences ([preferences.md](../../intelligence/preferences.md))
- Individual event categories can be enabled/disabled independently
- Volume follows the system accessibility audio channel, separate from media volume
- Audio cues respect the system-wide "Reduce audio distractions" accessibility setting

-----

### 19.5 Reduced Motion Support

All Inspector animations respect the system-wide `prefers-reduced-motion` preference. When reduced motion is active, the Inspector provides equivalent information through static presentation.

**Animation replacements:**

| Normal Mode | Reduced Motion Mode |
|---|---|
| Timeline scrubbing with animated cursor | Static cursor jump to selected time |
| View transition slide/fade (300ms) | Instant view swap (0ms) |
| Graph layout spring animation | Immediate final-position layout |
| Provenance timeline flowing dots | Static event markers at fixed positions |
| Alert pulse animation | Static bold border |
| Heatmap color transition | Instant color update |

**Implementation constraints:**

- No animation exceeds 300ms duration even in normal mode
- All animations use `requestAnimationFrame` equivalent — never CSS transitions that cannot be intercepted
- The reduced motion setting is read once at Inspector start and re-read on preference change events; no polling

Cross-reference: [accessibility.md](../../experience/accessibility.md), [experience.md §15](../../experience/experience.md)
