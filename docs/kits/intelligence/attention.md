# Attention Kit

**Layer:** Intelligence | **Crate:** `aios_attention` | **Architecture:** [`docs/intelligence/attention.md`](../../intelligence/attention.md)

## 1. Overview

The Attention Kit manages the flow of notifications and interruptions to the user. In
traditional operating systems, the sending application decides notification importance -- a
promotional email has the same visual weight as a server outage. AIOS inverts this: agents
post structured attention items describing events, and the Attention Manager uses AIRS to
assess urgency based on the user's context, relationships, history, and actual content. An
agent cannot set its own urgency. It describes what happened. The system decides whether the
user cares.

The Attention Manager routes items through a triage pipeline: intake (rate-limited per agent),
AIRS urgency assessment, context filtering (via the [Context Kit](context.md)), grouping and
summarization (five Slack messages become "5 messages in #engineering, none urgent"), and
finally delivery to the presentation layer with one of four delivery modes: Interrupt (now),
NextBreak (toast when user pauses), Digest (batch for later), or Silent (log only). Focus
sessions allow the user to enter heightened suppression periods, and per-agent attention
budgets prevent notification spam.

Use the Attention Kit when your agent needs to notify the user of an event or query the
current focus state before generating an interruption. Do not use it for agent-to-agent
messaging (use IPC channels) or for persistent data storage (use the
[Storage Kit](../platform/storage.md)).

## 2. Core Traits

```rust
use aios_attention::{
    AttentionManager, AttentionItem, AttentionContent,
    NotificationFilter, FocusSession, AttentionBudget,
    DeliveryMode, Urgency,
};
use aios_capability::CapabilityHandle;

/// Post attention items and query delivery state.
///
/// The central interface for agents. Agents post structured attention items
/// describing events. The Attention Manager runs them through the triage
/// pipeline and delivers them according to the assessed urgency and current
/// context. Agents never set urgency directly.
pub trait AttentionManager {
    /// Post an attention item for triage.
    ///
    /// The item enters the intake queue, is assessed by AIRS for urgency,
    /// filtered by current context and focus sessions, grouped with related
    /// items, and delivered to the presentation layer. The returned ID can
    /// be used to update or withdraw the item.
    fn post(
        &self,
        item: AttentionContent,
        cap: &CapabilityHandle,
    ) -> Result<AttentionId, AttentionError>;

    /// Withdraw a previously posted attention item before delivery.
    ///
    /// If the item has already been delivered, this marks it as resolved
    /// in the attention panel. If still queued, it is removed.
    fn withdraw(
        &self,
        id: AttentionId,
        cap: &CapabilityHandle,
    ) -> Result<(), AttentionError>;

    /// Update a posted item with new content (e.g., unread count changed).
    ///
    /// The item is re-triaged with the updated content. If urgency changes,
    /// the delivery mode may change accordingly.
    fn update(
        &self,
        id: AttentionId,
        content: AttentionContent,
        cap: &CapabilityHandle,
    ) -> Result<(), AttentionError>;

    /// Query the current delivery mode that would be applied to a
    /// hypothetical item. Useful for deciding whether to post at all.
    fn current_delivery_mode(&self) -> DeliveryMode;

    /// Query whether the user is currently in a focus session.
    fn is_focus_active(&self) -> bool;
}

/// Structured content for an attention item.
///
/// Agents describe the event; they do not set urgency. AIRS assesses
/// urgency from the content, the user's relationship with the source,
/// and the current context.
pub struct AttentionContent {
    /// Category of the event (message, alert, reminder, progress, social).
    pub category: AttentionCategory,
    /// Primary text describing the event.
    pub title: String,
    /// Optional detail text (e.g., message preview).
    pub body: Option<String>,
    /// The entity this event relates to (person, service, system).
    pub source_entity: Option<EntityRef>,
    /// Concrete actions the user can take (max 3).
    pub actions: Vec<AttentionAction>,
    /// Whether this item can be grouped with similar items.
    pub groupable: bool,
    /// Optional expiration -- item is auto-dismissed after this duration.
    pub expires_after: Option<core::time::Duration>,
}

/// Event categories used by AIRS for urgency assessment.
pub enum AttentionCategory {
    /// Person-to-person message (chat, email, mention).
    Message { sender: Option<String>, channel: Option<String> },
    /// System or service alert (error, warning, status change).
    Alert { severity: AlertSeverity },
    /// Time-based reminder or deadline.
    Reminder { due_at: Option<Timestamp> },
    /// Long-running task progress update.
    Progress { percent: f32, task_name: String },
    /// Social activity (likes, follows, shares).
    Social { interaction_type: String },
}

/// How the Attention Manager delivers an item to the user.
pub enum DeliveryMode {
    /// Show immediately as an interrupt overlay. Reserved for critical items.
    Interrupt,
    /// Show as a toast when the user next takes a break (input idle > 3s).
    NextBreak,
    /// Batch into a digest summary delivered at context transitions.
    Digest,
    /// Log silently. Visible in the Attention Panel but no notification.
    Silent,
}

/// AI-assessed urgency level (set by AIRS, never by the posting agent).
pub enum Urgency {
    /// Life-safety, system failure, or critical-person message.
    Critical,
    /// Important but not emergency. Timely delivery expected.
    High,
    /// Standard notification. Delivery can wait for a natural break.
    Medium,
    /// Low importance. Batch into digest or log silently.
    Low,
}

/// Per-agent and system-wide notification filter rules.
///
/// Filters can suppress, delay, or reclassify items. System-wide filters
/// apply to all agents. Per-agent filters are set by the user in Settings.
pub trait NotificationFilter {
    /// Add a filter rule for a specific agent or category.
    fn add_rule(
        &self,
        rule: FilterRule,
        cap: &CapabilityHandle,
    ) -> Result<FilterId, AttentionError>;

    /// Remove a filter rule.
    fn remove_rule(
        &self,
        id: FilterId,
        cap: &CapabilityHandle,
    ) -> Result<(), AttentionError>;

    /// List active filter rules.
    fn active_rules(&self) -> Result<Vec<FilterRule>, AttentionError>;
}

/// User-initiated focus session with heightened interruption suppression.
///
/// During a focus session, only items exceeding the session's threshold
/// break through. Everything else is deferred to Digest or Silent.
pub trait FocusSession {
    /// Start a focus session with the given parameters.
    fn start(
        &self,
        config: FocusConfig,
        cap: &CapabilityHandle,
    ) -> Result<FocusId, AttentionError>;

    /// End the current focus session early.
    fn end(
        &self,
        id: FocusId,
        cap: &CapabilityHandle,
    ) -> Result<FocusSummary, AttentionError>;

    /// Get the current focus session status (if any).
    fn current(&self) -> Option<FocusStatus>;
}

/// Focus session configuration.
pub struct FocusConfig {
    /// Minimum urgency level that breaks through during focus.
    pub breakthrough_threshold: Urgency,
    /// How long the focus session lasts (None = until manually ended).
    pub duration: Option<core::time::Duration>,
    /// Specific agents that always break through (e.g., a VIP contact).
    pub allowed_agents: Vec<AgentId>,
    /// Label for the focus session (shown in status strip).
    pub label: String,
}

/// Per-agent attention budget limiting notification frequency.
pub trait AttentionBudget {
    /// Query the remaining budget for the calling agent.
    fn remaining(&self, cap: &CapabilityHandle) -> Result<BudgetStatus, AttentionError>;

    /// Query the budget for a specific agent (requires admin capability).
    fn query_agent(
        &self,
        agent: AgentId,
        cap: &CapabilityHandle,
    ) -> Result<BudgetStatus, AttentionError>;
}

/// Budget status for a single agent.
pub struct BudgetStatus {
    /// Items posted in the current period.
    pub used: u32,
    /// Maximum items allowed per period.
    pub limit: u32,
    /// When the budget resets.
    pub resets_in: core::time::Duration,
    /// Whether the agent is currently rate-limited.
    pub throttled: bool,
}
```

## 3. Usage Patterns

### Posting a simple notification

```rust
use aios_attention::{AttentionManager, AttentionContent, AttentionCategory, AttentionAction};

fn notify_new_message(
    attention: &dyn AttentionManager,
    cap: &CapabilityHandle,
    sender: &str,
    preview: &str,
) -> Result<AttentionId, AttentionError> {
    let content = AttentionContent {
        category: AttentionCategory::Message {
            sender: Some(sender.to_string()),
            channel: None,
        },
        title: format!("New message from {sender}"),
        body: Some(preview.to_string()),
        source_entity: None,
        actions: vec![
            AttentionAction { label: "Reply".into(), action_id: "reply".into() },
            AttentionAction { label: "Mark read".into(), action_id: "mark_read".into() },
        ],
        groupable: true,
        expires_after: None,
    };

    attention.post(content, cap)
}
```

### Checking focus state before posting

```rust
use aios_attention::{AttentionManager, DeliveryMode};

fn post_if_appropriate(
    attention: &dyn AttentionManager,
    cap: &CapabilityHandle,
    content: AttentionContent,
) -> Result<Option<AttentionId>, AttentionError> {
    // Check if notifications would even be shown right now
    match attention.current_delivery_mode() {
        DeliveryMode::Silent => {
            // User is in deep focus -- skip low-importance items entirely
            Ok(None)
        }
        _ => {
            // Post the item; the Attention Manager handles delivery timing
            let id = attention.post(content, cap)?;
            Ok(Some(id))
        }
    }
}
```

### Starting a focus session

```rust
use aios_attention::{FocusSession, FocusConfig, Urgency};

fn start_coding_session(
    focus: &dyn FocusSession,
    cap: &CapabilityHandle,
) -> Result<FocusId, AttentionError> {
    focus.start(
        FocusConfig {
            breakthrough_threshold: Urgency::Critical,
            duration: Some(core::time::Duration::from_secs(2 * 3600)),
            allowed_agents: vec![], // Only critical items break through
            label: "Coding session".into(),
        },
        cap,
    )
}
```

## 4. Integration Examples

### Attention Kit + Context Kit: auto-focus on deep work

```rust
use aios_context::{ContextConsumer, ActivityType};
use aios_attention::{FocusSession, FocusConfig, Urgency};

fn auto_focus_on_deep_work(
    context: &dyn ContextConsumer,
    focus: &dyn FocusSession,
    cap: &CapabilityHandle,
) -> Result<(), AttentionError> {
    context.on_transition(
        Box::new(move |_old, new| {
            if matches!(new.activity, ActivityType::DeepWork)
                && new.engagement_level > 0.8
                && focus.current().is_none()
            {
                // Auto-enter focus when deep work is detected
                let _ = focus.start(
                    FocusConfig {
                        breakthrough_threshold: Urgency::High,
                        duration: None, // Ends when context transitions
                        allowed_agents: vec![],
                        label: "Auto-focus (deep work detected)".into(),
                    },
                    cap,
                );
            }
        }),
        cap,
    )?;

    Ok(())
}
```

### Attention Kit + AIRS Kit: urgency assessment pipeline

```rust
use aios_attention::{AttentionContent, Urgency, AttentionCategory};

// This is internal to the Attention Manager, shown for illustration.
// Agents do not call AIRS directly for urgency assessment.
fn assess_urgency(
    content: &AttentionContent,
    context: &ContextState,
    airs_available: bool,
) -> Urgency {
    if !airs_available {
        // Fallback: use category-based heuristics
        return match &content.category {
            AttentionCategory::Alert { severity: AlertSeverity::Critical } => Urgency::Critical,
            AttentionCategory::Alert { .. } => Urgency::High,
            AttentionCategory::Reminder { .. } => Urgency::Medium,
            _ => Urgency::Low,
        };
    }

    // With AIRS: semantic analysis of content, relationship scoring,
    // and context-weighted urgency assessment. The AI considers:
    // - Is the sender in the user's "critical contacts" list?
    // - Does the content mention deadlines, emergencies, or action items?
    // - Is this related to the user's current task?
    // This logic runs inside the Attention Manager, not in agent code.
    Urgency::Medium // placeholder
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
| --- | --- | --- |
| `AttentionPost` | Posting attention items | Granted to all agents |
| `AttentionWithdraw` | Withdrawing or updating posted items | Granted to posting agent |
| `FocusManage` | Starting and ending focus sessions | User-initiated actions only |
| `FilterManage` | Adding and removing notification filter rules | User-initiated actions only |
| `BudgetQuery` | Querying own attention budget | Granted to all agents |
| `BudgetAdmin` | Querying or modifying other agents' budgets | System agents only |
| `AttentionAdmin` | System-wide attention configuration | System agents only |

## 6. Error Handling

```rust
/// Errors returned by the Attention Kit.
pub enum AttentionError {
    /// The agent lacks the required attention capability.
    /// Recovery: declare `AttentionPost` in the agent manifest.
    CapabilityDenied(String),

    /// The agent has exceeded its per-period attention budget.
    /// Recovery: reduce posting frequency or wait for budget reset.
    BudgetExhausted {
        used: u32,
        limit: u32,
        resets_in: core::time::Duration,
    },

    /// The attention item ID does not exist (already delivered and dismissed,
    /// or withdrawn). Recovery: post a new item if needed.
    ItemNotFound(AttentionId),

    /// Too many actions specified in the attention content (max 3).
    /// Recovery: reduce to three or fewer actions.
    TooManyActions {
        provided: usize,
        max: usize,
    },

    /// The content title or body exceeds size limits (title: 256 chars, body: 4096).
    /// Recovery: truncate the text.
    ContentTooLarge(String),

    /// A focus session is already active. Only one at a time.
    /// Recovery: end the current focus session before starting a new one.
    FocusAlreadyActive(FocusId),

    /// No focus session is currently active (attempted to end a non-existent session).
    NoActiveFocus,

    /// Internal error in the attention pipeline.
    /// Recovery: retry; if persistent, report via Inspector.
    InternalError(String),
}
```

## 7. Platform & AI Availability

The Attention Kit operates in two modes depending on AIRS availability:

**With AIRS (full intelligence):**

- Semantic urgency assessment: AIRS analyzes content, sender relationships, and context.
- Notification grouping and summarization: "5 messages in #engineering, none urgent."
- Relationship-aware scoring: messages from critical contacts score higher.
- Content relevance: items related to the current task score higher.
- Learned patterns: AIRS learns which items the user acts on vs. dismisses.

**Without AIRS (heuristic fallback):**

- Category-based urgency: `Alert(Critical)` maps to `Urgency::Critical`, messages
  default to `Urgency::Medium`, social items default to `Urgency::Low`.
- Simple grouping: items from the same agent with the same category are grouped by
  count. No AI summarization.
- No relationship scoring: all senders are treated equally.
- No content analysis: urgency is derived from category alone.

**Feature detection:**

```rust
use aios_attention::AttentionManager;

fn has_ai_triage(attention: &dyn AttentionManager) -> bool {
    // When AIRS is available, delivery mode decisions are more nuanced.
    // Without AIRS, the manager falls back to category-based heuristics.
    // There is no explicit API to check; agents should always post and
    // let the Attention Manager decide. This check is informational only.
    attention.current_delivery_mode() != DeliveryMode::Silent
        || attention.is_focus_active()
}
```

**Attention budget defaults (per agent, per hour):**

| Trust Level | Budget (items/hour) | Burst Limit |
| --- | --- | --- |
| System agent | Unlimited | Unlimited |
| Trusted (user-installed) | 60 | 10 in 60s |
| Verified (store-distributed) | 30 | 5 in 60s |
| Unknown (sideloaded) | 10 | 2 in 60s |
