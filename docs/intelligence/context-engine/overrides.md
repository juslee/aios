# AIOS Context Engine — Override System

Part of: [context-engine.md](../context-engine.md) — Context Engine
**Related:** [inference.md](./inference.md) — Inference pipeline (overrides bypass inference), [consumers.md](./consumers.md) — State consumers, [sdk.md](./sdk.md) — SDK API

-----

## 5. Override System

### 5.1 Explicit Overrides

The user can always tell the system what they want. Overrides are created through the Conversation Bar (natural language), keyboard shortcuts (system-defined), or calendar events (automatic).

```rust
pub struct Override {
    /// What the user said or what triggered this
    intent: String,            // "heads down for 2 hours"
    /// The context state to enforce
    effect: ContextState,
    /// When this override was created
    created_at: Timestamp,
    /// When this override expires — always set, never permanent
    expires: Timestamp,
    /// Source of the override
    source: OverrideSource,
    /// Unique identifier for stack management
    id: OverrideId,
}

pub enum OverrideSource {
    /// User spoke or typed the intent
    UserExplicit,
    /// Calendar event triggered it
    Calendar(CalendarEventId),
    /// Agent requested it (requires user approval)
    Agent(AgentId),
    /// System condition (e.g., low battery)
    System,
}
```

**Overrides are always time-bounded.** This is a hard invariant. The system will not create an override without an expiration time. If the user says "heads down" without a duration, the system defaults to 2 hours. If the user says "I'm done for the day," the override expires at midnight. There is no "permanent focus mode" because permanent overrides are how you miss your kid's school calling.

The Override Manager enforces this:

```rust
pub struct OverrideManager {
    /// Active override stack (most recent last; top-of-stack = last element)
    stack: Vec<Override>,
    /// Maximum override duration (configurable, default: 12 hours)
    max_duration: Duration,
    /// Default duration when user doesn't specify
    default_duration: Duration,            // 2 hours
}

impl OverrideManager {
    pub fn create(&mut self, intent: &str, effect: ContextState, duration: Option<Duration>) -> Override {
        let duration = duration
            .unwrap_or(self.default_duration)
            .min(self.max_duration);

        let ovr = Override {
            intent: intent.to_string(),
            effect,
            created_at: Timestamp::now(),
            expires: Timestamp::now() + duration,
            source: OverrideSource::UserExplicit,
            id: OverrideId::generate(),
        };

        self.stack.push(ovr.clone());
        self.gc_expired();
        ovr
    }

    /// Returns the active override (top of stack, if not expired)
    pub fn active(&mut self) -> Option<&Override> {
        self.gc_expired();
        self.stack.last()
    }

    /// Remove expired overrides
    fn gc_expired(&mut self) {
        let now = Timestamp::now();
        self.stack.retain(|o| o.expires > now);
    }
}
```

When an override is active, the compositor shows a subtle indicator: a small context badge in the status area showing the override intent and time remaining. "Heads down — 1h 42m remaining." The user can dismiss the override at any time.

### 5.2 Override Examples

Common overrides and their effects:

| User says                | work_engagement | ai_engagement | notification_threshold | resource_priority | expires        |
|--------------------------|-----------------|---------------|------------------------|-------------------|----------------|
| "Heads down"             | 1.0             | Available     | Interrupt              | Balanced          | 2h (default)   |
| "Heads down for 4 hours" | 1.0             | Available     | Interrupt              | Balanced          | 4h             |
| "I'm done for the day"  | 0.0             | Invisible     | Digest                 | Foreground        | End of day     |
| "Gaming"                 | 0.0             | Invisible     | Interrupt              | Foreground        | Until explicit end (max 12h) |
| "In a meeting"           | 0.7             | Ambient       | NextBreak              | Balanced          | Meeting end    |
| "Focus time"             | 1.0             | Available     | Interrupt              | BackgroundWork    | 2h (default)   |
| "Light browsing"         | 0.2             | Ambient       | Digest                 | Foreground        | 1h (default)   |
| "Presenting"             | 0.8             | Invisible     | Interrupt              | Foreground        | 1h (default)   |

Calendar-sourced overrides are created automatically when a calendar event starts. The calendar agent posts the override with the event's duration. When the event ends, the override expires and inference resumes.

### 5.3 Override Stacking

Multiple overrides can be active simultaneously. The stack is ordered by creation time. The most recent override takes precedence.

```text
Stack (top = active):
  3. "Quick break" (expires in 10 min)     ← ACTIVE
  2. "Heads down for 4 hours" (expires in 3h 50m)
  1. Calendar: "Focus afternoon" (expires at 5 PM)
```

When override #3 expires in 10 minutes, override #2 resumes automatically. When #2 expires, #1 resumes. When #1 expires, inference resumes.

This handles the common case: user is in "heads down" mode, takes a 10-minute break ("quick break"), and returns to "heads down" without having to re-activate it.

```rust
impl OverrideManager {
    /// Cancel the top override and fall back to the next one
    pub fn cancel_top(&mut self) {
        self.stack.pop();
    }

    /// Cancel all overrides and return to inference
    pub fn cancel_all(&mut self) {
        self.stack.clear();
    }

    /// Get the full stack for display in the UI
    pub fn stack(&self) -> &[Override] {
        &self.stack
    }
}
```
