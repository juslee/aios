# AIOS Context Engine — SDK API and Diagnostics

Part of: [context-engine.md](../context-engine.md) — Context Engine
**Related:** [consumers.md](./consumers.md) — System-level consumers, [overrides.md](./overrides.md) — Override capabilities, [learning.md](./learning.md) — Learned patterns in diagnostics

-----

## 9. SDK API

Agents interact with the Context Engine through the SDK's `AgentContext` trait. Three operations are available: reading current context, posting attention items, and subscribing to context changes.

### 9.1 Reading Context (for agents)

Agents with the `ContextRead` capability can read the current context state. This is a read-only, non-blocking operation.

```rust
// Agent reads current context and adapts its behavior
let context = ctx.context().current().await?;

match context.ai_engagement {
    AiEngagement::Available => {
        // Full AI features: show suggestions, enable conversation,
        // proactively offer help
        self.enable_suggestions();
        self.show_ai_panel();
    }
    AiEngagement::Ambient => {
        // Subtle AI: improve search results, auto-complete,
        // but don't show conversational UI
        self.enable_smart_defaults();
        self.hide_ai_panel();
    }
    AiEngagement::Invisible => {
        // No visible AI: pure manual mode
        // AI still works (search, indexing) but nothing is shown
        self.disable_suggestions();
        self.hide_ai_panel();
    }
}

// Use work_engagement for fine-grained adaptation
if context.work_engagement > 0.8 {
    // Deep focus: minimize distractions in the UI
    self.set_minimal_chrome();
} else if context.work_engagement < 0.2 {
    // Deep leisure: relax constraints, show fun features
    self.set_relaxed_chrome();
}
```

### 9.2 Posting Attention Items

Agents post attention items through the Attention Manager. The agent declares the content and an initial urgency hint. AIRS always determines the final urgency — the agent's hint is advisory only and may be overridden based on actual content analysis and current context (see [attention.md](../attention.md) for the authoritative urgency assignment model).

```rust
ctx.attention().post(AttentionItem {
    content: AttentionContent::text("Meeting in 5 minutes: Team Standup"),
    urgency: Urgency::NextBreak,  // hint only; AIRS determines final urgency
    relevance: 0.8,
    auto_actionable: Some(ProposedAction::OpenCalendar),
    group: Some(GroupId::from("calendar-reminders")),
    ..Default::default()
}).await?;
```

The agent's declared `urgency` is a hint. AIRS may upgrade or downgrade it. An email agent that declares every message as `Interrupt` will find its messages consistently downgraded to `Digest` by AIRS. An agent that accurately declares urgency builds a better track record and its declarations are trusted more over time.

### 9.3 Subscribing to Context Changes

Agents can subscribe to a stream of context updates. This is the preferred mechanism for agents that need to adapt continuously (as opposed to checking context once at startup).

```rust
let mut context_stream = ctx.context().subscribe().await?;

while let Some(update) = context_stream.next().await {
    let new_state = update.state;

    // Adapt UI based on new context
    self.adapt_ui(&new_state);

    // Adjust agent behavior
    match new_state.ai_engagement {
        AiEngagement::Available => self.start_proactive_analysis(),
        AiEngagement::Ambient => self.stop_proactive_analysis(),
        AiEngagement::Invisible => {
            self.stop_proactive_analysis();
            self.reduce_background_work();
        }
    }

    // Respect resource priority
    match new_state.resource_priority {
        ResourcePriority::Foreground => {
            // Another agent has priority — reduce our resource usage
            self.throttle_background_tasks();
        }
        ResourcePriority::Balanced => {
            self.resume_normal_operation();
        }
        ResourcePriority::BackgroundWork => {
            // Background work is boosted — good time for indexing
            self.start_background_indexing();
        }
    }
}
```

### 9.4 Capability Requirements

| Operation              | Required Capability | Notes                                |
|------------------------|---------------------|--------------------------------------|
| Read current context   | `ContextRead`       | Read-only, most agents should have   |
| Subscribe to changes   | `ContextRead`       | Same capability, streaming variant   |
| Post attention item    | `AttentionPost`     | AIRS re-assesses urgency             |
| Create override        | Not available to agents | User-only via Conversation Bar   |

Agents cannot create overrides. Only the user (through the Conversation Bar, keyboard shortcuts, or calendar events) can override the inferred context. This prevents agents from manipulating the context to get more resources or bypass notification filtering.

-----

## 10. Diagnostics

The Context Engine exposes its full internal state through the Inspector. Every aspect of context inference is visible to the user.

### 10.1 Inspector View

```text
┌─────────────────────────────────────────────────────────┐
│  Context Engine — Inspector                              │
│                                                          │
│  Current State:                                         │
│    work_engagement:      0.82  ████████░░  (deep work)  │
│    ai_engagement:        Available                      │
│    notification_threshold: Interrupt                     │
│    resource_priority:    Balanced                        │
│    source:               AIRS classifier                │
│                                                          │
│  Active Override: None                                   │
│                                                          │
│  Signal Values:                                         │
│  ┌───────────────────┬────────┬────────┬──────────────┐ │
│  │ Signal            │ Value  │ Weight │ Contribution │ │
│  ├───────────────────┼────────┼────────┼──────────────┤ │
│  │ ActiveSpace       │ 0.90   │ 0.70   │ 0.630        │ │
│  │ RunningAgents     │ 0.85   │ 0.60   │ 0.510        │ │
│  │ InputPattern      │ 0.78   │ 0.50   │ 0.390        │ │
│  │ CalendarState     │ —      │ 0.80   │ (no event)   │ │
│  │ MediaPlayback     │ 0.30   │ 0.50   │ 0.150        │ │
│  │ TimeOfDay         │ 0.70   │ 0.30   │ 0.210        │ │
│  │ UserHistory       │ +0.05  │ 0.40   │ (modifier)   │ │
│  │ ExplicitIntent    │ —      │ 1.00   │ (no intent)  │ │
│  └───────────────────┴────────┴────────┴──────────────┘ │
│                                                          │
│  Override Stack: (empty)                                 │
│                                                          │
│  Recent Transitions:                                    │
│    14:32  Leisure → Work  (opened IDE, started typing)  │
│    12:15  Work → Leisure  (lunch break, media playing)  │
│    09:02  — → Work        (boot, morning routine)       │
│                                                          │
│  Hysteresis:                                            │
│    Pending transition: None                              │
│    Last transition: 2h 14m ago                          │
│                                                          │
│  Inference Stats:                                       │
│    Model: AIRS classifier (context-v1.gguf, 2.1 MB)    │
│    Avg inference time: 0.8ms                            │
│    Inferences today: 847                                │
│    Overrides today: 1 ("heads down", 09:30-11:30)      │
└─────────────────────────────────────────────────────────┘
```

### 10.2 Diagnostic API

System agents and the Inspector access diagnostics through a dedicated IPC endpoint:

```rust
pub enum ContextDiagnostic {
    /// Full current state with signal breakdown
    CurrentState {
        state: ContextState,
        signals: Vec<SignalSnapshot>,
        override_stack: Vec<Override>,
        source: ContextSource,
    },

    /// History of context transitions
    TransitionHistory {
        transitions: Vec<ContextTransition>,
    },

    /// Inference performance statistics
    InferenceStats {
        model_name: String,
        model_size: usize,
        avg_inference_ms: f32,
        total_inferences: u64,
        total_overrides: u64,
    },

    /// Learned patterns summary
    LearnedPatterns {
        work_hours: Vec<TimeRange>,
        space_associations: Vec<(SpaceId, SpaceCategory)>,
        override_frequency: f32,
    },
}

pub struct SignalSnapshot {
    signal_type: String,
    raw_value: f32,
    weight: f32,
    contribution: f32,
    last_updated: Timestamp,
}

pub struct ContextTransition {
    from: ContextState,
    to: ContextState,
    trigger: String,            // human-readable reason
    timestamp: Timestamp,
}
```
