# AIOS Context Engine — Consumers

Part of: [context-engine.md](../context-engine.md) — Context Engine
**Related:** [inference.md](./inference.md) — Context state production, [overrides.md](./overrides.md) — Override system, [sdk.md](./sdk.md) — SDK API for agents

-----

## 6. Consumers

The Context Engine publishes `ContextState` to consumers via IPC pub/sub. Consumers subscribe at startup and receive updates whenever the state changes significantly (past the hysteresis threshold).

```rust
pub struct StatePublisher {
    subscribers: Vec<ChannelId>,
}

impl StatePublisher {
    pub async fn publish(&self, state: ContextState) {
        for channel in &self.subscribers {
            ipc_send(*channel, ContextUpdate {
                state: state.clone(),
                timestamp: Timestamp::now(),
                source: ContextSource::Inferred, // or Override
            });
        }
    }

    pub fn subscribe(&mut self, channel: ChannelId) {
        self.subscribers.push(channel);
    }
}

pub struct ContextUpdate {
    pub state: ContextState,
    pub timestamp: Timestamp,
    pub source: ContextSource,
}

pub enum ContextSource {
    Inferred,                   // from model or rule-based
    Override(OverrideId),       // from explicit override
    Fallback,                   // from rule-based (AIRS unavailable)
}
```

### 6.1 Scheduler

The scheduler receives `ContextState` and adjusts scheduling weights via context multipliers.

During deep work (`work_engagement` > 0.8, `resource_priority: Balanced`): background agents running compilation, linting, or indexing get a scheduling boost. The foreground agent (IDE) keeps interactive priority. Idle-class agents (sync, maintenance) are deprioritized.

During gaming (`work_engagement` < 0.2, `resource_priority: Foreground`): the foreground game agent gets maximum CPU and GPU allocation. All background agents drop to `Idle` priority. The compositor minimizes its own overhead. AIRS inference is deprioritized (inference requests from background agents queue behind the game's frame deadline).

```rust
pub struct ContextMultiplier {
    interactive: f32,       // weight for Interactive priority agents
    normal: f32,            // weight for Normal priority agents
    idle: f32,              // weight for Idle priority agents
}

impl Scheduler {
    fn context_multipliers(&self, ctx: &ContextState) -> ContextMultiplier {
        match ctx.resource_priority {
            ResourcePriority::Foreground => ContextMultiplier {
                interactive: 2.0,
                normal: 0.5,
                idle: 0.1,
            },
            ResourcePriority::Balanced => ContextMultiplier {
                interactive: 1.0,
                normal: 1.0,
                idle: 0.5,
            },
            ResourcePriority::BackgroundWork => ContextMultiplier {
                interactive: 1.0,
                normal: 1.5,
                idle: 0.3,
            },
        }
    }
}
```

### 6.2 Attention Manager

The Attention Manager is the most important consumer. It decides what the user sees and when. The full pipeline:

```text
Agent posts AttentionItem
  │
  ▼
Attention Manager receives item
  │
  ▼
Step 1: AIRS re-assesses urgency
  │  Agent says "Interrupt" — but is it really?
  │  AIRS examines content, sender context, user context.
  │  A Slack message from a bot is not Interrupt, even if the
  │  agent declared it so. A message from the user's manager
  │  during a meeting might be.
  │
  ▼
Step 2: Filter against notification_threshold from ContextState
  │  notification_threshold = Interrupt → only Interrupt items pass
  │  notification_threshold = NextBreak → Interrupt + NextBreak pass
  │  notification_threshold = Digest → all items pass (but batched)
  │
  ├── Item passes threshold → Step 3
  │
  └── Item does not pass → queued
        │
        ├── Urgency Digest → batched for periodic summary
        └── Urgency Silent → logged, never shown
  │
  ▼
Step 3: Grouping
  │  Related items batched: 10 Slack messages from one channel
  │  become 1 grouped notification: "12 new messages in #engineering"
  │  Grouping key: GroupId from AttentionItem, or AIRS-inferred
  │
  ▼
Step 4: Route to display
  │  Compositor shows notification via overlay surface
  │  For Interrupt: immediate, with sound
  │  For NextBreak: queued, shown when user pauses (idle > 10s)
  │
  ▼
Step 5: Auto-action
  If auto_actionable is Some: show proposed action button
  "Meeting in 5 minutes — Open Calendar"
  User taps → action executes. User ignores → dismissed after 30s.
```

```rust
pub struct AttentionManager {
    incoming: PriorityQueue<AttentionItem>,
    model: AttentionModel,
    context: ContextState,
    digest_queue: Vec<AttentionItem>,
    groups: HashMap<GroupId, Vec<AttentionItem>>,
    digest_interval: Duration,              // default: 30 minutes
    next_digest: Timestamp,
}

/// See attention.md §3 for the full 12-field definition.
/// Key fields used by the Context Engine's attention integration:
pub struct AttentionItem {
    pub id: AttentionId,
    pub source: AgentId,
    pub content: AttentionContent,
    pub urgency: Urgency,                   // AI-assessed, not app-declared
    pub relevance: f32,                     // 0.0-1.0, AIRS-computed
    pub auto_actionable: Option<ProposedAction>,
    pub group: Option<GroupId>,
    pub timestamp: SystemTime,
    pub expiry: Option<SystemTime>,
    pub seen: bool,
    pub acted: bool,
    pub triage: TriageMetadata,
}

impl AttentionManager {
    pub async fn process(&mut self, mut item: AttentionItem) {
        // Step 1: AIRS re-assessment
        if let Some(ref model) = self.model.classifier {
            item.urgency = model.assess_urgency(&item, &self.context).await;
            item.relevance = model.assess_relevance(&item, &self.context).await;
        }

        // Step 2: Filter against threshold
        if !self.passes_threshold(&item) {
            match item.urgency {
                Urgency::Digest => self.digest_queue.push(item),
                Urgency::Silent => { /* log only */ }
                _ => self.digest_queue.push(item),
            }
            return;
        }

        // Step 3: Grouping
        if let Some(group_id) = &item.group {
            let group = self.groups.entry(group_id.clone()).or_default();
            group.push(item.clone());
            if group.len() > 1 {
                // Show grouped notification instead
                self.show_grouped(group_id).await;
                return;
            }
        }

        // Step 4: Route to display
        self.show(item).await;
    }

    fn passes_threshold(&self, item: &AttentionItem) -> bool {
        match self.context.notification_threshold {
            Urgency::Interrupt => item.urgency == Urgency::Interrupt,
            Urgency::NextBreak => matches!(item.urgency, Urgency::Interrupt | Urgency::NextBreak),
            Urgency::Digest => item.urgency != Urgency::Silent,
            Urgency::Silent => false, // Silent items are logged only, never displayed
        }
    }
}
```

**Digest delivery.** Every 30 minutes (configurable), the Attention Manager bundles queued Digest items into a summary and presents it as a single notification: "You have 3 messages, 2 email threads, and 1 PR review waiting." AIRS generates the summary text. The user can expand for details or dismiss.

### 6.3 Compositor

The compositor adapts the visual environment based on context.

```text
Work context (work_engagement > 0.7, ai_engagement: Available):
  ┌──────────────────────────────────────────────┐
  │  [Conversation Bar — prominent, ready]        │
  │                                               │
  │  ┌─────────────────┐  ┌───────────────────┐  │
  │  │                 │  │                   │  │
  │  │   Code Editor   │  │   Terminal        │  │
  │  │   (60% width)   │  │   (40% width)     │  │
  │  │                 │  │                   │  │
  │  └─────────────────┘  └───────────────────┘  │
  │                                               │
  │  [Task bar — showing active tasks/agents]     │
  └──────────────────────────────────────────────┘

Leisure context (work_engagement < 0.3, ai_engagement: Invisible):
  ┌──────────────────────────────────────────────┐
  │                                               │
  │         ┌───────────────────────┐             │
  │         │                       │             │
  │         │    Media Player       │             │
  │         │    (centered, large)  │             │
  │         │                       │             │
  │         └───────────────────────┘             │
  │                                  [·] ← bar   │
  │  [Task bar — minimal, auto-hide]              │
  └──────────────────────────────────────────────┘

Focus mode (override active, work_engagement = 1.0):
  ┌──────────────────────────────────────────────┐
  │  [Conversation Bar — available but unobtrusive│
  │                                               │
  │  ┌──────────────────────────────────────────┐│
  │  │                                          ││
  │  │          Document Editor                 ││
  │  │          (fullscreen, minimal chrome)    ││
  │  │                                          ││
  │  └──────────────────────────────────────────┘│
  │                              [Focus: 1h 42m] │
  └──────────────────────────────────────────────┘
```

The compositor subscribes to `ContextState` updates and adjusts:

- **Conversation bar visibility.** `Available` → prominent bar with suggestion chips. `Ambient` → subtle bar, no suggestions. `Invisible` → minimized to a single dot (still invokable by gesture).
- **Layout mode.** Work context → tiling layout. Leisure context → floating layout with centered primary window. Focus override → fullscreen primary window.
- **Chrome density.** Work → standard chrome (menus, tabs, toolbars visible). Focus → minimal chrome (distractions removed). Leisure → adaptive (media player shows controls, browser shows bar).
- **Animation style.** Work → snappy, minimal transitions. Leisure → smooth, relaxed animations.

### 6.4 Preference Service

The Preference Service adjusts system preferences based on context:

```rust
pub struct ContextPreferences {
    /// Brightness adjustment (relative to user's base setting)
    brightness_offset: f32,         // -1.0 to 1.0
    /// Color temperature adjustment
    color_temp_offset: f32,         // -1.0 (cool) to 1.0 (warm)
    /// Theme variant
    theme_variant: Option<ThemeVariant>,
}

pub enum ThemeVariant {
    Light,
    Dark,
    ReducedMotion,
    HighContrast,
}

impl PreferenceService {
    fn context_preferences(&self, ctx: &ContextState) -> ContextPreferences {
        ContextPreferences {
            // Evening + leisure → warmer, dimmer
            brightness_offset: if ctx.work_engagement < 0.3 { -0.1 } else { 0.0 },
            color_temp_offset: if ctx.work_engagement < 0.3 { 0.2 } else { 0.0 },
            theme_variant: None, // only if user has enabled context-based themes
        }
    }
}
```

Preference adjustments are subtle. The user sets their base preferences. The Context Engine modulates them slightly — a touch dimmer in the evening during leisure, a touch warmer color temperature. If the user has explicitly configured a theme schedule, context overrides do not interfere.

-----
