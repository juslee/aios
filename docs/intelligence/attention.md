# AIOS Attention Management

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [airs.md](./airs.md) — Urgency inference and summarization, [context-engine.md](./context-engine.md) — Context-aware filtering, [experience.md](../experience/experience.md) — Attention Panel UI, [agents.md](../applications/agents.md) — Agent attention posting, [security.md](../security/security.md) — Capability enforcement

-----

## 1. Overview

Notifications are broken. Every application believes its messages are important. A promotional email has the same visual weight as a server outage. A game achievement badge interrupts a deep coding session. Users receive hundreds of notifications per day and learn to ignore them all — which means they also miss the ones that matter.

The problem is structural: traditional notification systems let **senders** decide importance. The app that sends the notification chooses the title, the sound, the badge count, the priority level. A social media app has every incentive to mark everything as urgent. The user is the victim of a race to the bottom where every notification screams for attention.

AIOS inverts this. **The AI decides importance, not the sender.** Agents post attention items — structured descriptions of events — and the Attention Manager uses AIRS to assess urgency based on the user's context, relationships, history, and the actual content. An agent cannot set its own urgency. It can declare what happened. The system decides whether the user cares.

**Key principles:**

1. **AI-assessed urgency.** The sender describes the event. AIRS determines importance.
2. **Context-aware delivery.** What gets through depends on what the user is doing right now.
3. **Never interruptive unless genuinely urgent.** During leisure, only a system error or a critical person breaks through.
4. **Summarized, not listed.** Five Slack messages become one line: "5 messages in #engineering (none urgent)."
5. **Actionable, not just dismissible.** Every attention item has concrete actions, not just "OK."

-----

## 2. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                                                                   │
│  Agent A ──┐                                                      │
│             │   IPC (PostAttention capability required)            │
│  Agent B ──┼──→ ┌──────────────────────────────────────────────┐ │
│             │    │           Attention Manager                   │ │
│  Agent C ──┘    │          (system service)                     │ │
│                  │                                               │ │
│                  │  ┌─────────────┐    ┌──────────────────────┐│ │
│                  │  │ Intake Queue │───→│ AIRS Triage          ││ │
│                  │  │ (rate-limited│    │                      ││ │
│                  │  │  per-agent)  │    │ • Urgency assessment ││ │
│                  │  └─────────────┘    │ • Content analysis   ││ │
│                  │                      │ • Relationship lookup││ │
│                  │                      │ • History patterns   ││ │
│                  │                      └──────────┬───────────┘│ │
│                  │                                 │            │ │
│                  │                      ┌──────────▼───────────┐│ │
│                  │                      │ Context Filter       ││ │
│                  │                      │                      ││ │
│                  │                      │ • Current context    ││ │
│                  │                      │ • User preferences   ││ │
│                  │                      │ • Override state     ││ │
│                  │                      └──────────┬───────────┘│ │
│                  │                                 │            │ │
│                  │                      ┌──────────▼───────────┐│ │
│                  │                      │ Grouping & Summary   ││ │
│                  │                      │                      ││ │
│                  │                      │ • Cluster related    ││ │
│                  │                      │ • Generate summaries ││ │
│                  │                      │ • Merge duplicates   ││ │
│                  │                      └──────────┬───────────┘│ │
│                  │                                 │            │ │
│                  │  ┌──────────────┐    ┌──────────▼───────────┐│ │
│                  │  │ Audit Log    │◄───│ Presentation Queue   ││ │
│                  │  │ (all items)  │    │                      ││ │
│                  │  └──────────────┘    │ • Interrupt → now    ││ │
│                  │                      │ • NextBreak → wait   ││ │
│                  │                      │ • Digest → batch     ││ │
│                  │                      │ • Silent → log only  ││ │
│                  │                      └──────────┬───────────┘│ │
│                  └──────────────────────────────────┼───────────┘ │
│                                                     │             │
│                                          ┌──────────▼──────────┐ │
│                                          │  Presentation Layer  │ │
│                                          │                      │ │
│                                          │  • Status Strip badge│ │
│                                          │  • Attention Panel   │ │
│                                          │  • Interrupt overlay │ │
│                                          │  • Conversation Bar  │ │
│                                          │  • Toast (NextBreak) │ │
│                                          └──────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

-----

## 3. The Attention Item

### 3.1 Data Model

```rust
pub struct AttentionItem {
    /// Unique identifier
    pub id: AttentionId,

    /// Which agent posted this item
    pub source: AgentId,

    /// Structured content — not a free-form string
    pub content: TypedContent,

    /// AI-assessed urgency (set by AIRS, not by the agent)
    pub urgency: Urgency,

    /// Relevance to the user's current activity (0.0 - 1.0)
    pub relevance: f32,

    /// Proposed action the user can take with one click
    pub auto_actionable: Option<ProposedAction>,

    /// Group ID for clustering related items
    pub group: Option<GroupId>,

    /// When this item was posted
    pub timestamp: SystemTime,

    /// When this item expires (no longer relevant)
    pub expiry: Option<SystemTime>,

    /// Whether the user has seen this item
    pub seen: bool,

    /// Whether the user has acted on this item
    pub acted: bool,

    /// Triage metadata from AIRS
    pub triage: TriageMetadata,
}

pub enum TypedContent {
    /// Message from a person
    PersonMessage {
        sender: IdentityId,
        channel: String,
        preview: String,
        service: ServiceId,
    },
    /// System event (build, deploy, error)
    SystemEvent {
        event_type: SystemEventType,
        summary: String,
        details: Option<String>,
    },
    /// Agent report (task complete, results ready)
    AgentReport {
        agent: AgentId,
        task: Option<TaskId>,
        summary: String,
        results: Option<Vec<SpaceObjectId>>,
    },
    /// Calendar/scheduling
    Schedule {
        event_name: String,
        time: SystemTime,
        change: Option<ScheduleChange>,
    },
    /// External service update
    ServiceUpdate {
        service: ServiceId,
        summary: String,
        url: Option<String>,
    },
}

pub enum Urgency {
    /// Show immediately — system error, critical person, safety alert
    Interrupt,
    /// Show when the user next pauses — colleague message, build result
    NextBreak,
    /// Batch into periodic summary — newsletters, non-urgent updates
    Digest,
    /// Log but never show — telemetry, routine confirmations
    Silent,
}
```

### 3.2 How Items Differ from Notifications

|Property|Traditional Notification|AIOS Attention Item|
|--------|----------------------|-------------------|
|Urgency|Set by sending app|Set by AI based on content + context|
|Content|Free-form string|Typed, structured data|
|Grouping|None (chronological list)|AI clusters related items|
|Summarization|None|AI generates one-line summaries|
|Actions|Dismiss / Open app|Context-specific (Reply, Accept, Snooze, etc.)|
|Filtering|Per-app toggle|Per-context, per-relationship, AI-triaged|
|History|Scrollback until cleared|Queryable audit log with analytics|

-----

## 4. Urgency Assessment

### 4.1 How AIRS Determines Urgency

When an attention item arrives, AIRS evaluates multiple signals to assign urgency:

```rust
pub struct UrgencyAssessment {
    /// Final urgency level
    pub urgency: Urgency,
    /// Confidence in the assessment (0.0 - 1.0)
    pub confidence: f32,
    /// Signals that contributed to the decision
    pub signals: Vec<UrgencySignal>,
}

pub enum UrgencySignal {
    /// Sender is in user's relationships with high trust
    RelationshipPriority { identity: IdentityId, trust: TrustLevel },
    /// Content contains urgency markers ("ASAP", "urgent", "down", "broken")
    ContentUrgencyMarkers { markers: Vec<String> },
    /// Content sentiment indicates distress or emergency
    SentimentAnalysis { sentiment: Sentiment, confidence: f32 },
    /// User historically engages with items from this source quickly
    HistoricalEngagement { source: AgentId, avg_response_time: Duration },
    /// Item is time-sensitive (meeting in 10 minutes, deployment window)
    TimeSensitivity { deadline: SystemTime },
    /// Source agent trust level (system agents rank higher)
    AgentTrustLevel { trust: TrustLevel },
    /// Content type inherently urgent (system errors, security alerts)
    InherentUrgency { event_type: SystemEventType },
}
```

### 4.2 Assessment Pipeline

```rust
impl AttentionManager {
    async fn assess_urgency(&self, item: &mut AttentionItem) -> UrgencyAssessment {
        let mut signals = Vec::new();

        // 1. Check if sender is a known identity with high trust
        if let TypedContent::PersonMessage { sender, .. } = &item.content {
            if let Some(rel) = self.identity_service.get_relationship(sender).await {
                let priority = match rel.kind {
                    RelationshipKind::Family => UrgencySignal::RelationshipPriority {
                        identity: *sender,
                        trust: TrustLevel::Trusted,
                    },
                    RelationshipKind::Colleague => UrgencySignal::RelationshipPriority {
                        identity: *sender,
                        trust: rel.trust_level,
                    },
                    _ => UrgencySignal::RelationshipPriority {
                        identity: *sender,
                        trust: TrustLevel::Known,
                    },
                };
                signals.push(priority);
            }
        }

        // 2. Content analysis via AIRS inference
        let content_text = item.content.to_text();
        let analysis = self.airs.analyze_urgency(&content_text).await;
        if !analysis.urgency_markers.is_empty() {
            signals.push(UrgencySignal::ContentUrgencyMarkers {
                markers: analysis.urgency_markers,
            });
        }
        signals.push(UrgencySignal::SentimentAnalysis {
            sentiment: analysis.sentiment,
            confidence: analysis.confidence,
        });

        // 3. Historical engagement patterns
        let history = self.audit_log.engagement_stats(&item.source).await;
        if history.avg_response_time < Duration::from_secs(60) {
            signals.push(UrgencySignal::HistoricalEngagement {
                source: item.source,
                avg_response_time: history.avg_response_time,
            });
        }

        // 4. Time sensitivity
        if let TypedContent::Schedule { time, .. } = &item.content {
            let until = time.duration_since(SystemTime::now()).unwrap_or_default();
            if until < Duration::from_secs(600) {
                signals.push(UrgencySignal::TimeSensitivity { deadline: *time });
            }
        }

        // 5. Inherent urgency from system events
        if let TypedContent::SystemEvent { event_type, .. } = &item.content {
            match event_type {
                SystemEventType::Error | SystemEventType::SecurityAlert => {
                    signals.push(UrgencySignal::InherentUrgency {
                        event_type: *event_type,
                    });
                }
                _ => {}
            }
        }

        // 6. Compute final urgency from signals
        let urgency = Self::compute_urgency(&signals);
        let confidence = Self::compute_confidence(&signals);

        UrgencyAssessment { urgency, confidence, signals }
    }

    fn compute_urgency(signals: &[UrgencySignal]) -> Urgency {
        // Any inherent urgency signal → Interrupt
        if signals.iter().any(|s| matches!(s, UrgencySignal::InherentUrgency { .. })) {
            return Urgency::Interrupt;
        }

        // Family + urgency markers → Interrupt
        let has_family = signals.iter().any(|s| matches!(s,
            UrgencySignal::RelationshipPriority { trust: TrustLevel::Trusted, .. }
        ));
        let has_urgency_markers = signals.iter().any(|s| matches!(s,
            UrgencySignal::ContentUrgencyMarkers { .. }
        ));
        if has_family && has_urgency_markers {
            return Urgency::Interrupt;
        }

        // Time-sensitive → NextBreak (or Interrupt if < 5 min)
        if let Some(UrgencySignal::TimeSensitivity { deadline }) =
            signals.iter().find(|s| matches!(s, UrgencySignal::TimeSensitivity { .. }))
        {
            let until = deadline.duration_since(SystemTime::now()).unwrap_or_default();
            if until < Duration::from_secs(300) {
                return Urgency::Interrupt;
            }
            return Urgency::NextBreak;
        }

        // Known person with fast historical response → NextBreak
        let has_known_person = signals.iter().any(|s| matches!(s,
            UrgencySignal::RelationshipPriority { .. }
        ));
        let fast_response = signals.iter().any(|s| matches!(s,
            UrgencySignal::HistoricalEngagement { .. }
        ));
        if has_known_person && fast_response {
            return Urgency::NextBreak;
        }

        // Default for known persons
        if has_known_person {
            return Urgency::NextBreak;
        }

        // Everything else → Digest
        Urgency::Digest
    }
}
```

### 4.3 Why Agents Cannot Set Urgency

If agents controlled their own urgency, every agent would set `Interrupt`. This is exactly the problem with traditional notifications. In AIOS:

- The agent's `PostAttention` IPC message has no urgency field.
- The agent provides `TypedContent` — a structured description of what happened.
- AIRS assesses urgency based on content, sender, context, and history.
- The agent never knows what urgency was assigned to its item.

This removes the incentive for urgency inflation. An agent that says "server is down" will be assessed as urgent because the content analysis detects a critical event, not because the agent claimed it was urgent.

-----

## 5. Context Filtering

After urgency assessment, items pass through the Context Filter. The user's current context determines what gets through:

### 5.1 Context-Based Thresholds

```rust
pub struct ContextFilter {
    context: ContextState,
    user_overrides: AttentionPreferences,
}

impl ContextFilter {
    pub fn should_present(&self, item: &AttentionItem) -> PresentationDecision {
        let threshold = self.threshold_for_context();

        match item.urgency {
            Urgency::Interrupt => {
                // Interrupt always gets through unless user explicitly suppressed
                if self.user_overrides.suppress_all {
                    PresentationDecision::Queue
                } else {
                    PresentationDecision::Immediate
                }
            }
            Urgency::NextBreak => {
                if threshold <= UrgencyThreshold::NextBreak {
                    PresentationDecision::WaitForBreak
                } else {
                    PresentationDecision::Digest
                }
            }
            Urgency::Digest => {
                PresentationDecision::Digest
            }
            Urgency::Silent => {
                PresentationDecision::LogOnly
            }
        }
    }

    fn threshold_for_context(&self) -> UrgencyThreshold {
        match self.context.mode() {
            ContextMode::Work => UrgencyThreshold::NextBreak,
            ContextMode::Leisure => UrgencyThreshold::InterruptOnly,
            ContextMode::Focus => UrgencyThreshold::InterruptOnly,
            ContextMode::Gaming => UrgencyThreshold::InterruptOnly,
        }
    }
}

pub enum PresentationDecision {
    /// Show now (interrupt overlay or toast)
    Immediate,
    /// Queue until user pauses activity
    WaitForBreak,
    /// Include in next digest
    Digest,
    /// Log to audit, never present
    LogOnly,
    /// Queue until context changes
    Queue,
}
```

### 5.2 Break Detection

For `NextBreak` items, the Attention Manager detects user pauses:

```rust
pub struct BreakDetector {
    last_input_time: Instant,
    break_threshold: Duration,    // default: 30 seconds of no input
    context_engine: ContextEngineClient,
}

impl BreakDetector {
    pub fn is_user_on_break(&self) -> bool {
        let idle_duration = Instant::now() - self.last_input_time;
        idle_duration > self.break_threshold
    }

    pub fn on_input_event(&mut self) {
        self.last_input_time = Instant::now();
    }

    pub fn on_break_detected(&self) -> Vec<AttentionItem> {
        // Return all queued NextBreak items
        self.next_break_queue.drain(..).collect()
    }
}
```

When the user pauses (30 seconds of no input), queued `NextBreak` items appear as subtle toasts — visible but not blocking.

### 5.3 Context Transition Flush

When the Context Engine detects a transition (work → leisure), the Attention Manager re-evaluates all queued items:

```rust
impl AttentionManager {
    pub fn on_context_transition(&mut self, old: ContextMode, new: ContextMode) {
        // Re-filter all queued items with new context
        let mut to_present = Vec::new();
        for item in self.queued_items.drain(..) {
            let decision = self.context_filter.should_present(&item);
            match decision {
                PresentationDecision::Immediate | PresentationDecision::WaitForBreak => {
                    to_present.push(item);
                }
                PresentationDecision::Digest => {
                    self.digest_buffer.push(item);
                }
                PresentationDecision::LogOnly => {
                    self.audit_log.record(&item);
                }
                PresentationDecision::Queue => {
                    self.queued_items.push(item);
                }
            }
        }

        // Present accumulated items as a digest on transition
        if !to_present.is_empty() {
            self.present_transition_digest(to_present);
        }
    }
}
```

-----

## 6. Grouping and Summarization

### 6.1 Grouping Algorithm

Related items are clustered before presentation. Grouping reduces noise — instead of 12 individual Slack messages, the user sees "12 Slack messages in 3 channels."

```rust
pub struct AttentionGroup {
    pub id: GroupId,
    pub items: Vec<AttentionItem>,
    pub summary: String,           // AI-generated one-liner
    pub highest_urgency: Urgency,  // max urgency in group
    pub source_agent: AgentId,     // common source
    pub category: GroupCategory,
}

pub enum GroupCategory {
    /// Messages from one channel/thread
    MessageThread { channel: String, count: usize },
    /// Multiple events from same agent
    AgentBatch { agent: AgentId, count: usize },
    /// CI/build results
    BuildResults { passed: usize, failed: usize },
    /// Email digest
    EmailBatch { count: usize, important: usize },
    /// Uncategorized cluster
    Mixed { count: usize },
}

impl AttentionManager {
    fn group_items(&self, items: &[AttentionItem]) -> Vec<AttentionGroup> {
        let mut groups: HashMap<GroupKey, Vec<&AttentionItem>> = HashMap::new();

        for item in items {
            let key = self.group_key(item);
            groups.entry(key).or_default().push(item);
        }

        groups.into_iter().map(|(key, items)| {
            let highest_urgency = items.iter()
                .map(|i| &i.urgency)
                .max()
                .cloned()
                .unwrap_or(Urgency::Silent);

            let summary = self.airs.summarize_group(&items);

            AttentionGroup {
                id: GroupId::new(),
                items: items.into_iter().cloned().collect(),
                summary,
                highest_urgency,
                source_agent: items[0].source,
                category: Self::categorize(&key, &items),
            }
        }).collect()
    }

    fn group_key(&self, item: &AttentionItem) -> GroupKey {
        match &item.content {
            TypedContent::PersonMessage { channel, service, .. } => {
                GroupKey::Channel(service.clone(), channel.clone())
            }
            TypedContent::SystemEvent { event_type, .. } => {
                GroupKey::SystemEvent(*event_type)
            }
            TypedContent::AgentReport { agent, .. } => {
                GroupKey::Agent(*agent)
            }
            TypedContent::ServiceUpdate { service, .. } => {
                GroupKey::Service(service.clone())
            }
            TypedContent::Schedule { .. } => {
                GroupKey::Schedule
            }
        }
    }
}
```

### 6.2 AI Summarization

AIRS generates natural language summaries for groups:

```rust
impl AirsClient {
    pub async fn summarize_group(&self, items: &[&AttentionItem]) -> String {
        let context = SummarizationContext {
            item_count: items.len(),
            content_previews: items.iter()
                .take(5)
                .map(|i| i.content.to_text())
                .collect(),
            source_info: items[0].source.display_name(),
            time_span: TimeSpan::from_items(items),
        };

        // AIRS inference call
        let prompt = format!(
            "Summarize {} attention items from {} in one sentence. \
             Items span {}. Previews: {}",
            context.item_count,
            context.source_info,
            context.time_span,
            context.content_previews.join("; "),
        );

        self.infer(&prompt, ModelProfile::FastSummary).await
    }
}
```

Example summaries:

|Items|Summary|
|-----|-------|
|5 Slack messages in #engineering|"5 messages in #engineering about deployment timing (none urgent)"|
|3 CI builds|"3 builds completed: 2 passed, 1 failed (main branch)"|
|8 emails|"8 emails: 1 from Alex about meeting, 5 newsletters, 2 automated"|
|4 agent reports|"research-agent finished processing 4 papers, results in research/"|

-----

## 7. The Attention Digest

### 7.1 Digest Structure

The digest is a periodic summary of all attention activity since the user's last check:

```rust
pub struct AttentionDigest {
    /// Time range this digest covers
    pub since: SystemTime,
    pub until: SystemTime,

    /// Items requiring action, grouped by urgency
    pub urgent: Vec<AttentionGroup>,
    pub summary: Vec<AttentionGroup>,
    pub deferred: Vec<AttentionGroup>,

    /// Aggregate statistics
    pub stats: DigestStats,
}

pub struct DigestStats {
    pub total_items: usize,
    pub items_by_source: HashMap<AgentId, usize>,
    pub items_actioned: usize,
    pub items_auto_resolved: usize,
}
```

### 7.2 When Digests Are Presented

Digests are presented at natural transition points, never arbitrarily:

```rust
pub enum DigestTrigger {
    /// User tapped the attention badge in the Status Strip
    UserRequested,
    /// Context transition (work → leisure, focus → work)
    ContextTransition { from: ContextMode, to: ContextMode },
    /// Long break detected (> 5 minutes idle)
    LongBreak,
    /// Conversation Bar query ("what did I miss?")
    ConversationQuery,
    /// Scheduled digest time (if user configured, e.g., every 2 hours)
    Scheduled,
}
```

### 7.3 Digest Rendering

```
┌─ ATTENTION ──────────────────────────────────────────────┐
│                                                           │
│  ── Since 13:00 (2 hours ago) ──                         │
│                                                           │
│  URGENT                                                   │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  Alex: "Server is down, need your help ASAP"        │ │
│  │  Slack · 15 min ago                                  │ │
│  │  [Reply] [Open Slack]                                │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                           │
│  SUMMARY                                                  │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  5 Slack messages in #engineering (none urgent)      │ │
│  │  2 emails: 1 newsletter, 1 meeting confirmation     │ │
│  │  CI: 3 builds passed, 0 failed                      │ │
│  │  backup-agent: daily backup completed (312 objects)  │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                           │
│  DEFERRED                                                 │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  System update available (non-urgent)                │ │
│  │  Weather: rain expected tomorrow                     │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                           │
│  Total: 14 items · 1 needs action · 13 informational    │
│  [Mark all seen] [Settings]                               │
│                                                           │
└───────────────────────────────────────────────────────────┘
```

-----

## 8. Auto-Actionable Items

### 8.1 Proposed Actions

Some attention items arrive with actions the user can take immediately:

```rust
pub struct ProposedAction {
    /// Human-readable description of what this action does
    pub description: String,
    /// The actual action to execute
    pub action: ActionType,
    /// Required capabilities (verified before presenting)
    pub required_capabilities: Vec<Capability>,
    /// Whether this action is reversible
    pub reversible: bool,
}

pub enum ActionType {
    /// Reply to a message
    Reply { channel: IpcChannel, draft: Option<String> },
    /// Open a URL or space object
    Open { target: OpenTarget },
    /// Accept/decline a calendar event
    CalendarResponse { event_id: String, response: CalendarRsvp },
    /// Dismiss or snooze
    Snooze { duration: Duration },
    /// Run an agent task
    AgentTask { agent: AgentId, task: TaskSpec },
    /// Custom action defined by the posting agent
    Custom { agent: AgentId, action_id: String, params: serde_json::Value },
}
```

### 8.2 Action Verification

Before presenting an auto-actionable item, the Attention Manager verifies that the proposed action's required capabilities are available:

```rust
impl AttentionManager {
    fn verify_action(&self, action: &ProposedAction) -> bool {
        for cap in &action.required_capabilities {
            if !self.capability_manager.check(cap) {
                return false;
            }
        }
        true
    }

    fn present_item(&self, item: &AttentionItem) -> PresentableItem {
        let actions = if let Some(proposed) = &item.auto_actionable {
            if self.verify_action(proposed) {
                vec![
                    PresentableAction::primary(&proposed.description),
                    PresentableAction::secondary("Snooze"),
                    PresentableAction::secondary("Dismiss"),
                ]
            } else {
                vec![PresentableAction::secondary("Dismiss")]
            }
        } else {
            vec![PresentableAction::secondary("Dismiss")]
        };

        PresentableItem { item: item.clone(), actions }
    }
}
```

### 8.3 Examples

|Event|Proposed Actions|
|-----|---------------|
|"Alex asks: can you review PR #427?"|[Accept and open PR] [Decline] [Remind in 1hr]|
|"Meeting moved to 16:00"|[Acknowledge] [Suggest different time] [Decline]|
|"CI build failed on feature branch"|[View logs] [Rerun] [Dismiss]|
|"backup-agent completed daily backup"|[View details] — auto-dismiss after 1 hour|
|"System update available"|[Install at next reboot] [Remind tomorrow] [Details]|

-----

## 9. Agent Interaction

### 9.1 Posting Attention Items

Agents post attention items via IPC. The agent needs the `PostAttention` capability in its manifest:

```toml
# Agent manifest
[capabilities]
attention = "post"  # can post attention items
```

```rust
// Agent code — posting an attention item
use aios_sdk::attention;

pub async fn notify_results_ready(results: &[SpaceObjectId]) {
    attention::post(AttentionRequest {
        content: TypedContent::AgentReport {
            agent: self_agent_id(),
            task: current_task_id(),
            summary: format!("Found {} papers matching your criteria", results.len()),
            results: Some(results.to_vec()),
        },
        // Note: no urgency field. The agent cannot set urgency.
        expiry: Some(SystemTime::now() + Duration::from_hours(24)),
        auto_action: Some(ProposedAction {
            description: "View results in Space Navigator".into(),
            action: ActionType::Open {
                target: OpenTarget::Space("research/papers/".into()),
            },
            required_capabilities: vec![],
            reversible: true,
        }),
    }).await;
}
```

### 9.2 Rate Limiting

To prevent attention flooding, the Attention Manager rate-limits per agent:

```rust
pub struct RateLimiter {
    /// Maximum items per minute per agent
    per_minute: HashMap<AgentId, u32>,
    /// Maximum items per hour per agent
    per_hour: HashMap<AgentId, u32>,
    /// Default limits
    default_per_minute: u32,  // 10
    default_per_hour: u32,    // 100
}

impl RateLimiter {
    pub fn check(&mut self, agent: &AgentId) -> RateLimitResult {
        let minute_count = self.per_minute.entry(*agent).or_insert(0);
        let hour_count = self.per_hour.entry(*agent).or_insert(0);

        if *minute_count >= self.default_per_minute {
            return RateLimitResult::Throttled {
                retry_after: Duration::from_secs(60),
            };
        }
        if *hour_count >= self.default_per_hour {
            return RateLimitResult::Throttled {
                retry_after: Duration::from_secs(3600),
            };
        }

        *minute_count += 1;
        *hour_count += 1;
        RateLimitResult::Allowed
    }
}
```

If an agent exceeds rate limits, its items are silently demoted to `Silent` urgency and logged for review. Repeated violations trigger a behavioral anomaly flag in AIRS.

-----

## 10. Presentation Layer

### 10.1 Presentation Channels

Attention items reach the user through different channels depending on urgency and context:

```rust
pub enum PresentationChannel {
    /// Status Strip badge — count of unseen items
    /// Always visible (except Gaming mode)
    StatusBadge,

    /// Attention Panel — full digest view
    /// Opened by tapping badge or asking "what did I miss?"
    AttentionPanel,

    /// Interrupt overlay — urgent items
    /// Slides in from edge, requires acknowledgment
    InterruptOverlay,

    /// Toast — NextBreak items during detected pause
    /// Subtle, auto-dismisses after 5 seconds, non-blocking
    Toast,

    /// Conversation Bar — queryable
    /// "What notifications did I get?" / "Any messages from Alex?"
    ConversationBar,
}
```

### 10.2 Routing Logic

```rust
impl PresentationRouter {
    pub fn route(&self, item: &AttentionItem, decision: PresentationDecision)
        -> Vec<PresentationChannel>
    {
        match decision {
            PresentationDecision::Immediate => {
                vec![
                    PresentationChannel::InterruptOverlay,
                    PresentationChannel::StatusBadge,
                ]
            }
            PresentationDecision::WaitForBreak => {
                vec![
                    PresentationChannel::Toast,  // when break detected
                    PresentationChannel::StatusBadge,
                ]
            }
            PresentationDecision::Digest => {
                vec![PresentationChannel::StatusBadge]
                // Appears in AttentionPanel when user opens it
            }
            PresentationDecision::LogOnly => {
                vec![] // audit log only
            }
            PresentationDecision::Queue => {
                vec![] // held until context changes
            }
        }
    }
}
```

### 10.3 Interrupt Overlay

For `Interrupt`-level items, a non-dismissible overlay appears:

```
┌─────────────────────────────────────────────────────────┐
│                                                          │
│  ⚠ Alex (Slack):                                        │
│  "Server is down, need your help ASAP"                  │
│                                                          │
│  [Reply] [Open Slack] [Snooze 15min]                    │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

The overlay is:
- Positioned at the top of the screen, not fullscreen
- Semi-transparent background so context isn't lost
- Requires one action (reply, open, snooze, or dismiss)
- Plays a subtle audio cue (configurable, can be silenced)

-----

## 11. User Controls

### 11.1 Per-Agent Settings

```rust
pub struct AgentAttentionConfig {
    /// Override urgency for this agent (e.g., "never interrupt me for this")
    pub max_urgency: Option<Urgency>,
    /// Whether this agent's items appear in digests
    pub include_in_digest: bool,
    /// Custom rate limits
    pub rate_limit: Option<RateLimit>,
    /// Whether to auto-dismiss after expiry
    pub auto_dismiss: bool,
}
```

### 11.2 Relationship Overrides

```rust
pub struct RelationshipAttentionConfig {
    /// Always interrupt for this person, regardless of context
    pub always_interrupt: bool,
    /// Never show items from this person (blocked)
    pub blocked: bool,
    /// Custom sound for this person
    pub custom_sound: Option<SoundId>,
}
```

### 11.3 Conversational Configuration

All attention settings are configurable via the Conversation Bar:

- "Never interrupt me for the weather agent" → sets max_urgency to Digest
- "Always show messages from Alex immediately" → sets always_interrupt for Alex
- "Suppress notifications until 7am" → sets time-based override
- "I'm heads down for 2 hours" → Context Engine override, InterruptOnly threshold
- "How many notifications did I get today?" → audit log query

### 11.4 Do Not Disturb

```rust
pub struct DoNotDisturb {
    pub active: bool,
    pub until: Option<SystemTime>,
    pub exceptions: Vec<DndException>,
}

pub enum DndException {
    /// Allow interrupts from specific identities
    Identity(IdentityId),
    /// Allow interrupts from specific agents
    Agent(AgentId),
    /// Allow system-level emergencies
    SystemEmergency,
    /// Allow phone calls (if telephony agent exists)
    PhoneCalls,
}
```

-----

## 12. Relationship-Aware Priority

The Identity system feeds into urgency assessment. Items from people the user has relationships with receive priority adjustments:

```rust
pub struct RelationshipPriorityBoost {
    pub relationship_kind: RelationshipKind,
    pub urgency_boost: i8,  // -2 to +2
}

impl RelationshipPriorityBoost {
    pub fn for_kind(kind: RelationshipKind) -> Self {
        match kind {
            RelationshipKind::Family => Self { relationship_kind: kind, urgency_boost: 2 },
            RelationshipKind::Friend => Self { relationship_kind: kind, urgency_boost: 1 },
            RelationshipKind::Colleague => Self { relationship_kind: kind, urgency_boost: 1 },
            RelationshipKind::Acquaintance => Self { relationship_kind: kind, urgency_boost: 0 },
            RelationshipKind::Service => Self { relationship_kind: kind, urgency_boost: -1 },
            RelationshipKind::Unknown => Self { relationship_kind: kind, urgency_boost: -2 },
        }
    }
}
```

A `Digest`-level message from a family member gets boosted to `NextBreak`. A `NextBreak`-level marketing message from an unknown service gets demoted to `Digest`. The user's relationship graph is the signal, not the sender's self-declared importance.

-----

## 13. Audit and History

### 13.1 Audit Log

Every attention item is recorded in `system/audit/attention/`:

```rust
pub struct AuditEntry {
    pub item: AttentionItem,
    pub triage: TriageMetadata,
    pub presentation: PresentationDecision,
    pub user_action: Option<UserAction>,
    pub response_time: Option<Duration>,
    pub timestamp: SystemTime,
}

pub enum UserAction {
    Seen,
    Dismissed,
    Acted(ActionType),
    Snoozed(Duration),
    Never, // user never saw this item
}
```

### 13.2 Queryable History

Users can query attention history via the Conversation Bar:

- "What notifications did I get last week?" → time-range query
- "How many items did the weather agent post?" → per-agent stats
- "Show me everything Alex sent" → identity-filtered query
- "What's my average notification rate?" → analytics query
- "Did I miss anything important yesterday?" → urgency-filtered query with AI assessment

### 13.3 Pattern Analysis

AIRS periodically analyzes attention patterns to improve triage:

```rust
pub struct AttentionAnalytics {
    /// Items per day, trending up or down
    pub daily_volume: TimeSeries,
    /// Average response time per urgency level
    pub response_times: HashMap<Urgency, Duration>,
    /// Agents with highest post volume
    pub top_agents: Vec<(AgentId, usize)>,
    /// Items the user consistently ignores
    pub ignored_patterns: Vec<IgnorePattern>,
    /// Items the user consistently acts on quickly
    pub engaged_patterns: Vec<EngagePattern>,
}
```

If the user consistently ignores items from a particular agent, AIRS suggests reducing that agent's urgency or suppressing it entirely. If the user always responds quickly to a particular person, AIRS boosts that person's priority.

-----

## 14. Performance

### 14.1 Latency Targets

|Operation|Target|
|---------|------|
|Intake (receive IPC message)|< 1ms|
|AIRS urgency assessment|< 50ms for single item|
|Context filter check|< 1ms|
|Grouping (batch of 10)|< 10ms|
|Summarization (AIRS)|< 200ms for group summary|
|Presentation routing|< 1ms|
|Total intake-to-presentation (Interrupt)|< 100ms|
|Total intake-to-presentation (NextBreak)|< 500ms|
|Digest generation|< 2 seconds|

### 14.2 Batch Processing

Non-urgent items are batched to amortize AIRS inference costs:

```rust
impl AttentionManager {
    async fn process_batch(&mut self) {
        // Collect items from intake queue (up to 50)
        let batch: Vec<AttentionItem> = self.intake_queue
            .drain(..self.intake_queue.len().min(50))
            .collect();

        if batch.is_empty() {
            return;
        }

        // Batch urgency assessment (single AIRS call for efficiency)
        let assessments = self.airs.batch_assess_urgency(&batch).await;

        // Apply assessments and route
        for (mut item, assessment) in batch.into_iter().zip(assessments) {
            item.urgency = assessment.urgency;
            item.triage = assessment.into();
            let decision = self.context_filter.should_present(&item);
            self.route(item, decision);
        }
    }
}
```

### 14.3 Caching

Urgency assessments for recurring patterns are cached:

```rust
pub struct TriageCache {
    /// Cache key: (source_agent, content_type_hash)
    cache: LruCache<(AgentId, u64), CachedAssessment>,
    ttl: Duration, // 1 hour
}

pub struct CachedAssessment {
    pub urgency: Urgency,
    pub created: Instant,
}
```

If the same agent posts the same type of content repeatedly (e.g., hourly build results), the cached assessment is used instead of calling AIRS again.

-----

## 15. Boot-Time Initialization

The Attention Manager is a system service that starts during boot Phase 4 (Intelligence Services). Unlike most services, the Attention Manager must handle a bootstrapping problem: it depends on AIRS for urgency assessment, but AIRS may still be loading its model when the first attention items arrive. This section specifies the Attention Manager's initialization sequence, its minimal startup state, and how it connects to the compositor notification pipeline.

### 15.1 Initialization Sequence

```
Boot Phase 4 — Attention Manager startup:

1. Service Manager spawns Attention Manager process
   Capabilities granted: PostAttention (receive), ContextRead,
   CompositorNotify, AIRSInference (optional at this point)

2. Load user preferences from system/preferences/attention/
   - Per-agent suppression rules
   - Per-person priority boosts
   - Quiet hours schedule
   - Digest frequency setting
   If preferences don't exist (first boot): use built-in defaults

3. Initialize intake queue
   - Per-agent rate limiters (default: 10 items/minute/agent)
   - Total queue depth: 1000 items
   - Items arriving before AIRS is ready are queued, not dropped

4. Initialize audit log writer
   - Connect to system/audit/attention/ space
   - All items logged regardless of AIRS availability

5. Connect to Context Engine (if available)
   - Subscribe to ContextState changes
   - If Context Engine not yet ready: assume ContextMode::Default
     (medium notification threshold, no context filtering)

6. Probe AIRS availability
   - Send a lightweight health check to AIRS inference endpoint
   - If AIRS responds: enter AI-triage mode (normal operation)
   - If AIRS not yet ready: enter rule-based triage mode (§15.2)

7. Connect to Compositor notification pipeline
   - Register as the notification source for Status Strip badge
   - Register as the source for interrupt overlays
   - If Compositor not yet ready (Phase 4 runs before Phase 5):
     buffer presentation events until compositor connects

8. Signal Service Manager: Attention Manager ready
   - Other services can now post attention items via IPC
```

### 15.2 Pre-AIRS Triage (Rule-Based Mode)

Before AIRS loads its model (which may take several seconds on slow storage), the Attention Manager uses rule-based urgency assessment. This is a simpler, faster evaluation that doesn't require LLM inference.

```rust
pub struct RuleBasedTriage {
    /// Keyword patterns that indicate high urgency
    urgent_keywords: Vec<&'static str>,
    /// Agent categories with default urgency levels
    agent_urgency_defaults: HashMap<AgentCategory, UrgencyLevel>,
    /// Person priority from identity system (if available)
    relationship_boosts: HashMap<PersonId, f32>,
}

impl RuleBasedTriage {
    pub fn assess(&self, item: &AttentionItem) -> UrgencyAssessment {
        let mut score: f32 = 0.0;

        // 1. Agent category baseline
        score += match item.source_agent.category {
            AgentCategory::System => 0.8,       // system alerts are usually important
            AgentCategory::Communication => 0.5, // messages vary
            AgentCategory::Productivity => 0.3,  // typically low urgency
            AgentCategory::Media => 0.1,         // almost never urgent
            AgentCategory::Game => 0.05,         // never urgent
            _ => 0.3,
        };

        // 2. Keyword scan (no AI, just pattern matching)
        if self.urgent_keywords.iter().any(|kw| item.content.contains(kw)) {
            score += 0.3;
        }

        // 3. Relationship boost (if identity service is up)
        if let Some(person) = &item.sender_identity {
            if let Some(boost) = self.relationship_boosts.get(&person.id) {
                score += boost;  // e.g., +0.4 for family members
            }
        }

        // 4. Declared urgency hint (agents can suggest, but this is just a hint)
        score += match item.declared_urgency {
            DeclaredUrgency::Critical => 0.2,
            DeclaredUrgency::High => 0.1,
            DeclaredUrgency::Normal => 0.0,
            DeclaredUrgency::Low => -0.1,
        };

        score = score.clamp(0.0, 1.0);

        UrgencyAssessment {
            score,
            delivery: if score > 0.8 {
                DeliveryMode::Interrupt
            } else if score > 0.5 {
                DeliveryMode::NextBreak
            } else if score > 0.2 {
                DeliveryMode::Digest
            } else {
                DeliveryMode::Silent
            },
            confidence: Confidence::Low, // rule-based is always low confidence
            method: TriageMethod::RuleBased,
        }
    }
}
```

**Rule-based triage limitations:**
- No content understanding — cannot distinguish "server is on fire" from "server deployed successfully"
- No behavioral pattern learning — treats every notification from an app the same way
- No cross-item correlation — cannot group "5 messages from the same thread" without reading them
- Higher false-interrupt rate — more things get through that shouldn't

**Transition to AI triage.** When AIRS becomes available, the Attention Manager:
1. Switches to AI-triage mode for all new items
2. Does **not** re-triage queued items that were already delivered (that would cause duplicate notifications)
3. Re-triages queued items that are still in the intake queue (not yet delivered)
4. Logs the mode transition in the audit log

### 15.3 Minimal Startup State

The Attention Manager is functional with zero configuration:

| Dependency | Available at startup? | Fallback |
|---|---|---|
| AIRS | Maybe (loading model) | Rule-based triage (§15.2) |
| Context Engine | Maybe (starting concurrently) | Assume ContextMode::Default — medium threshold |
| Identity Service | Maybe (starting concurrently) | No relationship boosts; all senders treated equally |
| Compositor | No (Phase 5) | Buffer presentation events; deliver when compositor connects |
| User Preferences | Yes (loaded from space) | Built-in defaults if first boot |
| Audit Log | Yes (space writer) | Always available after Phase 2 (storage) |

**First-boot behavior.** On first boot, no user preferences exist. The Attention Manager uses conservative defaults: only system agents can interrupt, everything else goes to the digest. The user configures preferences through the Conversation Bar ("make messages from Mom always interrupt") or the Settings agent. Preferences are stored in `system/preferences/attention/` and loaded on subsequent boots.

### 15.4 Compositor Connection

The Attention Manager connects to the compositor's notification pipeline during Phase 5, when the compositor starts. Before that connection:

- Items that would be interrupts are buffered (max 10 buffered interrupts)
- Items that would be digest or silent are stored normally
- When the compositor connects, buffered interrupts are delivered in chronological order
- If more than 10 interrupts buffered: oldest are demoted to digest (the user wasn't looking at the screen anyway)

The connection uses a dedicated IPC channel with the `CompositorNotify` capability. The Attention Manager sends structured presentation commands:

```rust
pub enum PresentationCommand {
    /// Update the Status Strip badge count
    UpdateBadge { unseen_count: u32, highest_urgency: UrgencyLevel },
    /// Show an interrupt overlay (urgent item)
    ShowInterrupt { item: PresentableItem, timeout: Duration },
    /// Show a toast notification (NextBreak item, user is at a break)
    ShowToast { item: PresentableItem, timeout: Duration },
    /// Update the Attention Panel content (digest items changed)
    RefreshPanel { items: Vec<PresentableItem> },
}
```

-----

## 16. Implementation Order

```
Phase 9a:   Attention Manager service          → intake queue, audit log
Phase 9b:   AIRS urgency assessment            → basic content analysis
Phase 9c:   Context filtering                  → context-aware thresholds
Phase 9d:   Status Strip badge                 → unseen count visible

Phase 11a:  Attention Panel UI                 → digest view with grouping
Phase 11b:  Interrupt overlay                  → urgent items break through
Phase 11c:  Toast notifications                → NextBreak delivery
Phase 11d:  Grouping and summarization         → AI-generated summaries

Phase 14a:  Auto-actionable items              → one-click actions
Phase 14b:  Relationship-aware priority        → identity integration
Phase 14c:  User controls                      → per-agent, per-person settings
Phase 14d:  Conversational configuration       → Conversation Bar integration

Phase 17:   Break detection                    → idle-based NextBreak delivery
Phase 19:   Pattern analysis                   → AIRS learns from engagement
Phase 21:   Cross-device attention sync        → Space Mesh attention state
Phase 24:   Attention analytics                → queryable history, trends
```

-----

## 17. Design Principles

1. **The AI decides importance, not the sender.** Agents describe events. AIRS assesses urgency. No agent can inflate its own priority.

2. **Context determines delivery.** The same item might interrupt during work but get digested during leisure. What matters is what the user is doing right now.

3. **Summarize, don't list.** Five related items become one summary. The user sees patterns, not individual alerts.

4. **Every item is actionable.** "Dismiss" is never the only option. Items carry proposed actions the user can take with one tap.

5. **Silence is the default.** Most of what happens in the background should stay in the background. The audit log captures everything; the user sees only what matters.

6. **Relationships are the signal.** A message from a family member is inherently more important than one from an unknown service. The identity system provides the context that traditional notification systems lack.

7. **The user is always in control.** Override any decision. Suppress any agent. Boost any person. The AI's assessment is a default, not a mandate.

8. **History is queryable.** Nothing is lost. Everything is in the audit log. "What did I miss?" has a real answer.
