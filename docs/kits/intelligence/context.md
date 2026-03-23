# Context Kit

**Layer:** Intelligence | **Crate:** `aios_context` | **Architecture:** [`docs/intelligence/context-engine.md`](../../intelligence/context-engine.md) + 6 sub-docs

## 1. Overview

The Context Kit infers what the user is currently doing and delivers context-aware adaptation
to every subsystem in AIOS. It collects eight signal types -- active Space, running agents,
input patterns, time of day, calendar state, media playback, user history, and explicit
intent -- and feeds them through a classifier that outputs a `ContextState` describing the
user's engagement level, activity type, and interruptibility. Results are stabilized with
hysteresis to prevent context thrashing (e.g., briefly checking email during a coding session
does not switch the context from "deep work" to "communication").

Consumers subscribe to context transitions and receive push notifications when the state
changes. The scheduler uses context to adjust priority classes, the compositor adapts layout
and surface hints, the [Attention Kit](attention.md) uses context for notification filtering,
and the [Preference Kit](preference.md) evaluates context-driven temporal rules. Two inference
backends exist: an AIRS classifier model (primary, ~1ms per inference) and a rule-based
weighted-average fallback (secondary, always available).

Use the Context Kit when your agent needs to adapt behavior to the user's current activity
or subscribe to context transitions. Do not use it for raw sensor data (use the
[Input Kit](../platform/input.md) or sensor subsystems directly) or for user preference
values (use the [Preference Kit](preference.md), which consumes context internally).

## 2. Core Traits

```rust
use aios_context::{
    ContextEngine, ContextConsumer, ContextState,
    ContextSignal, ContextOverride,
};
use aios_capability::CapabilityHandle;

/// Read the current inferred context state.
///
/// The context state is a struct describing the user's activity, engagement
/// level, and interruptibility. It is updated at most once per second and
/// stabilized with hysteresis to prevent rapid oscillation.
pub trait ContextConsumer {
    /// Get the current inferred context state.
    ///
    /// This is a cheap read from a cached value -- no inference runs on each
    /// call. The state is updated asynchronously by the context engine.
    fn current_state(&self) -> Result<ContextState, ContextError>;

    /// Subscribe to context transitions. The callback is invoked whenever
    /// the context state changes (after hysteresis filtering).
    fn on_transition(
        &self,
        callback: Box<dyn Fn(&ContextState, &ContextState) + Send>,
        cap: &CapabilityHandle,
    ) -> Result<ContextSubscription, ContextError>;

    /// Unsubscribe from context transitions.
    fn unsubscribe(&self, sub: ContextSubscription) -> Result<(), ContextError>;

    /// Query the transition history for the last N transitions.
    fn recent_transitions(
        &self,
        limit: usize,
    ) -> Result<Vec<ContextTransition>, ContextError>;
}

/// The inferred context state.
///
/// Updated by the context engine at most once per second. All fields are
/// normalized to [0.0, 1.0] ranges. The `activity` field is the primary
/// classification; the numeric fields provide granularity.
pub struct ContextState {
    /// Primary activity classification.
    pub activity: ActivityType,
    /// Confidence in the activity classification (0.0 - 1.0).
    pub confidence: f32,
    /// How deeply engaged the user appears (0.0 = idle, 1.0 = deep focus).
    pub engagement_level: f32,
    /// How interruptible the user is (0.0 = do not disturb, 1.0 = open).
    pub interruptibility: f32,
    /// Time spent in this state (since last transition).
    pub duration: core::time::Duration,
    /// Whether an explicit override is currently active.
    pub override_active: bool,
}

/// Broad activity classification.
pub enum ActivityType {
    /// Coding, writing, design -- high-focus creative work.
    DeepWork,
    /// Email, chat, video calls -- communication-oriented.
    Communication,
    /// Movies, music, games -- leisure and entertainment.
    Media,
    /// Browsing, reading, light file management.
    Browsing,
    /// Meeting (calendar-driven, video/audio active).
    Meeting,
    /// No recent input, screen may be off.
    Idle,
    /// Not enough signal data to classify.
    Unknown,
}

/// Apply manual or rule-based overrides on top of inferred context.
///
/// Overrides take precedence over inference. They can be temporary
/// (expire after a duration) or persistent (until explicitly cleared).
pub trait ContextOverrideManager {
    /// Set a manual override. This immediately changes the context state
    /// for all consumers. The override persists until `clear_override`
    /// is called or the optional duration expires.
    fn set_override(
        &self,
        state: ContextState,
        duration: Option<core::time::Duration>,
        cap: &CapabilityHandle,
    ) -> Result<OverrideId, ContextError>;

    /// Clear an active override, allowing inference to resume.
    fn clear_override(
        &self,
        id: OverrideId,
        cap: &CapabilityHandle,
    ) -> Result<(), ContextError>;

    /// List currently active overrides.
    fn active_overrides(&self) -> Result<Vec<ActiveOverride>, ContextError>;
}

/// Report signals to the context engine (for system services only).
///
/// Most agents do not use this trait. Signals are reported by system
/// services (compositor, agent runtime, input subsystem) automatically.
/// This trait is exposed for agents that provide additional signal sources
/// (e.g., a calendar agent reporting meeting state).
pub trait ContextSignalReporter {
    /// Report a context signal. Signals are weighted and combined by the
    /// context engine's inference model.
    fn report_signal(
        &self,
        signal: ContextSignal,
        cap: &CapabilityHandle,
    ) -> Result<(), ContextError>;
}
```

## 3. Usage Patterns

### Adapting agent behavior to current context

```rust
use aios_context::{ContextConsumer, ActivityType};

fn should_show_tips(context: &dyn ContextConsumer) -> bool {
    match context.current_state() {
        Ok(state) => {
            // Only show tips during browsing or idle, not during deep work
            matches!(state.activity, ActivityType::Browsing | ActivityType::Idle)
                && state.interruptibility > 0.5
        }
        Err(_) => true, // Default to showing tips if context unavailable
    }
}
```

### Subscribing to context transitions

```rust
use aios_context::{ContextConsumer, ContextState, ActivityType};

fn setup_work_timer(
    context: &dyn ContextConsumer,
    cap: &CapabilityHandle,
) -> Result<ContextSubscription, ContextError> {
    context.on_transition(
        Box::new(|old, new| {
            if !matches!(old.activity, ActivityType::DeepWork)
                && matches!(new.activity, ActivityType::DeepWork)
            {
                // User just started deep work -- begin a focus timer
                start_focus_timer();
            }

            if matches!(old.activity, ActivityType::DeepWork)
                && !matches!(new.activity, ActivityType::DeepWork)
            {
                // User left deep work -- stop the timer, log duration
                stop_focus_timer(old.duration);
            }
        }),
        cap,
    )
}
```

### Setting an explicit context override

```rust
use aios_context::{ContextOverrideManager, ContextState, ActivityType};

fn enter_focus_mode(
    overrides: &dyn ContextOverrideManager,
    cap: &CapabilityHandle,
    hours: u64,
) -> Result<OverrideId, ContextError> {
    let focus_state = ContextState {
        activity: ActivityType::DeepWork,
        confidence: 1.0,
        engagement_level: 1.0,
        interruptibility: 0.0, // Do not disturb
        duration: core::time::Duration::from_secs(0),
        override_active: true,
    };

    overrides.set_override(
        focus_state,
        Some(core::time::Duration::from_secs(hours * 3600)),
        cap,
    )
}
```

## 4. Integration Examples

### Context Kit + Attention Kit: context-driven notification filtering

```rust
use aios_context::ContextConsumer;
use aios_attention::AttentionManager;

fn should_deliver_notification(
    context: &dyn ContextConsumer,
    attention: &dyn AttentionManager,
    item: &AttentionItem,
) -> bool {
    let state = context.current_state().unwrap_or_default();

    // During deep work with low interruptibility, only urgent items
    // break through. During idle, everything is delivered.
    match state.interruptibility {
        i if i < 0.2 => item.urgency == Urgency::Critical,
        i if i < 0.5 => item.urgency >= Urgency::High,
        _ => true,
    }
}
```

### Context Kit + Preference Kit: context-driven preferences

```rust
use aios_context::ContextConsumer;
use aios_preference::PreferenceStore;

fn get_theme_for_context(
    context: &dyn ContextConsumer,
    prefs: &dyn PreferenceStore,
    cap: &CapabilityHandle,
) -> String {
    let state = context.current_state().unwrap_or_default();

    // The Preference Kit evaluates context-driven temporal rules (§14)
    // internally. This example shows how an agent might query the resolved
    // preference, which already accounts for context.
    prefs.get("display.theme", cap)
        .unwrap_or_else(|_| "system-default".into())
}
```

### Context Kit + Compositor: adaptive layout

```rust
use aios_context::{ContextConsumer, ActivityType};

fn compositor_layout_hint(context: &dyn ContextConsumer) -> LayoutHint {
    let state = context.current_state().unwrap_or_default();

    match state.activity {
        ActivityType::DeepWork => LayoutHint::FocusSingle,
        ActivityType::Communication => LayoutHint::SplitWithSidebar,
        ActivityType::Media => LayoutHint::Fullscreen,
        ActivityType::Meeting => LayoutHint::VideoCallLayout,
        _ => LayoutHint::Default,
    }
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
| --- | --- | --- |
| `ContextRead` | Reading the current context state | Granted to all agents |
| `ContextSubscribe` | Subscribing to context transitions | Granted to all agents |
| `ContextOverride` | Setting manual context overrides | User-initiated actions only |
| `ContextSignalReport` | Reporting custom context signals | System services only |
| `ContextHistoryRead` | Reading transition history | Granted to all agents |
| `ContextAdmin` | Modifying inference model, signal weights | System agents only |

## 6. Error Handling

```rust
/// Errors returned by the Context Kit.
pub enum ContextError {
    /// The agent lacks the required context capability.
    /// Recovery: declare the capability in the agent manifest.
    CapabilityDenied(String),

    /// The context engine is not yet initialized (early boot).
    /// Recovery: wait for the `ContextReady` boot phase event.
    NotInitialized,

    /// The subscription limit has been reached (32 per agent).
    /// Recovery: unsubscribe from unused transition watchers.
    SubscriptionLimitReached {
        current: usize,
        max: usize,
    },

    /// The specified override ID does not exist or has already expired.
    /// Recovery: check `active_overrides()` for valid IDs.
    OverrideNotFound(OverrideId),

    /// An invalid signal was reported (malformed data, out-of-range values).
    /// Recovery: validate signal data before reporting.
    InvalidSignal(String),

    /// The context engine's inference backend encountered an error.
    /// The rule-based fallback is used automatically in this case.
    /// Recovery: check AIRS status; the engine will recover on its own.
    InferenceError(String),
}
```

## 7. Platform & AI Availability

The Context Kit has two inference backends and degrades gracefully:

**AIRS classifier (primary):**

- Small classifier model (~2 MB), not a full LLM.
- Runs in under 1ms on CPU. No GPU required.
- 32-feature input vector extracted from the eight signal sources.
- Outputs `ContextState` with calibrated confidence scores.
- Trains on labeled context data; ships as a GGUF model.
- Provides nuanced multi-factor context inference.

**Rule-based fallback (secondary, always available):**

- Weighted average computation using fixed signal weights.
- No model loading required. Works from first boot.
- Less nuanced: may misclassify ambiguous contexts (e.g., music playing
  during coding is classified as "media" instead of "deep work with music").
- Hysteresis and transition smoothing still apply.

**Feature detection:**

```rust
use aios_context::ContextConsumer;

fn is_ai_context_available(context: &dyn ContextConsumer) -> bool {
    context.current_state()
        .map(|s| s.confidence > 0.8) // AI classifier has higher confidence
        .unwrap_or(false)
}
```

**Signal availability:**

Not all signals are available on all devices. The context engine adapts:

| Signal | Desktop | Tablet | Server |
| --- | --- | --- | --- |
| ActiveSpace | Yes | Yes | No (headless) |
| RunningAgents | Yes | Yes | Yes |
| InputPattern | Yes | Yes (touch) | No |
| TimeOfDay | Yes | Yes | Yes |
| CalendarState | If synced | If synced | If synced |
| MediaPlayback | Yes | Yes | Rarely |
| UserHistory | After learning period | After learning period | N/A |
| ExplicitIntent | Yes | Yes | Via API |

When signals are missing, the context engine increases weight on available signals
and reduces confidence proportionally. On a headless server, context defaults to
"system service" mode with high interruptibility.
