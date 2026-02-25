# AIOS Context Engine

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [airs.md](./airs.md) — AI Runtime Service, [compositor.md](../platform/compositor.md) — Compositor and display, [agents.md](../applications/agents.md) — Agent framework

-----

## 1. Overview

Every traditional OS forces the user to manage their own context. You turn on "Do Not Disturb" when you want to focus. You switch display profiles when you move from coding to watching a movie. You close chat apps when you need to concentrate. You are the context engine, and you are terrible at it — you forget, you get lazy, you leave focus mode on for six hours and miss a call from your kid's school.

The AIOS Context Engine eliminates this. It continuously infers user context from system signals — what space is active, what agents are running, what the keyboard cadence looks like, what time it is, what the calendar says — and publishes a `ContextState` that drives the entire system. AI engagement level, notification threshold, resource scheduling priority, UI layout — all adapt automatically.

There are no toggles. There are no modes. The user never thinks about "switching" to anything. The OS just knows.

When the user does want to express intent — "heads down for two hours" or "I'm done for the day" — the Context Engine accepts that as an override. Overrides are always time-bounded. They expire. The system returns to inference. Forgotten overrides cannot exist.

The Context Engine is an intelligence service within AIRS. When AIRS is available, context inference uses a lightweight classifier model that processes signal vectors in under a millisecond. When AIRS is unavailable (early boot, resource pressure, user disabled), the engine falls back to a rule-based system using time-of-day heuristics and explicit overrides. The system degrades gracefully — it gets dumber, but it never breaks.

-----

## 2. Architecture

```
┌────────────────────────────────────────────────────────────┐
│                     Context Engine                          │
│                  (AIRS intelligence service)                │
│                                                            │
│  ┌────────────────┐  ┌─────────────────┐  ┌────────────┐  │
│  │    Signal       │  │   Context       │  │  Override   │  │
│  │    Collector    │  │   Model         │  │  Manager    │  │
│  │                 │  │                 │  │             │  │
│  │  gather input   │  │  AIRS inference │  │  explicit   │  │
│  │  from 8 signal  │  │  or rule-based  │  │  user       │  │
│  │  sources        │  │  fallback       │  │  intents    │  │
│  └────────┬────────┘  └────────┬────────┘  └──────┬─────┘  │
│           │                    │                   │        │
│           ▼                    ▼                   ▼        │
│  ┌────────────────┐  ┌─────────────────┐  ┌────────────┐  │
│  │    State        │  │   History       │  │  Fallback   │  │
│  │    Publisher    │  │   Store         │  │  Engine     │  │
│  │                 │  │                 │  │             │  │
│  │  notify all     │  │  learned        │  │  rule-based │  │
│  │  consumers      │  │  patterns in    │  │  inference  │  │
│  │  via IPC        │  │  system/context/│  │  (no AIRS)  │  │
│  └────────┬────────┘  └─────────────────┘  └────────────┘  │
│           │                                                 │
└───────────┼─────────────────────────────────────────────────┘
            │ Publishes ContextState
            │
    ┌───────┼──────────────┬──────────────┬──────────────┐
    ▼       ▼              ▼              ▼              ▼
Scheduler  Attention    Compositor    Preference     Agent
(priority  Manager      (UI adapt,   Service        Runtime
 adjust,   (notif       layout,      (theme,        (agent
 context   threshold,   chrome)      brightness)    hints)
 mult.)    digest)
```

The engine runs as a subservice of AIRS, sharing AIRS's privileged access to system state. It reads signals from other system services via IPC, produces a `ContextState`, and publishes that state to all consumers. Consumers subscribe and react — they never poll.

**Data flow is one-directional.** Signal sources push into the engine. The engine publishes state. Consumers read state. No consumer can modify the context. Only the Override Manager (driven by user intent) can force a state change.

-----

## 3. Signal Collection

### 3.1 Signal Sources

The engine collects eight signal types. Each maps to a variant of the `ContextSignal` enum from the architecture document:

```rust
pub enum ContextSignal {
    ActiveSpace(SpaceId),
    RunningAgents(Vec<AgentId>),
    InputPattern(InputActivity),
    TimeOfDay(Time),
    CalendarState(CalendarContext),
    MediaPlayback(MediaState),
    UserHistory(Pattern),
    ExplicitIntent(Option<Intent>),
}
```

**ActiveSpace.** Which space has focus? The compositor reports the active surface, the engine resolves that to a space. Work spaces (code, documents, research) push `work_engagement` high. Media spaces (games, movies, music) push it low. Spaces have a `space_category` field in their metadata — set by the user or inferred by AIRS at creation time.

```rust
pub enum SpaceCategory {
    Work,           // code, documents, research, email
    Communication,  // chat, video calls, social
    Media,          // music, video, podcasts
    Gaming,         // games, game saves, game mods
    Personal,       // photos, notes, journal
    System,         // settings, inspector, diagnostics
    Unknown,        // not yet categorized
}
```

**RunningAgents.** What agents are active? The Agent Runtime provides the full list. The engine maps agents to categories based on their manifest metadata. An IDE agent plus a research agent signals work. A game agent plus a music player signals leisure. Agent combinations matter more than individual agents — a music player alone is ambiguous; a music player alongside a game agent is unambiguously leisure.

```rust
pub struct AgentSignalData {
    agents: Vec<AgentId>,
    categories: Vec<AgentCategory>,
    foreground: Option<AgentId>,
    agent_count_by_category: HashMap<AgentCategory, usize>,
}

pub enum AgentCategory {
    Productivity,   // IDE, document editor, spreadsheet
    Communication,  // email, chat, video call
    Media,          // music player, video player, podcast
    Gaming,         // game, game launcher
    System,         // compositor, AIRS, inspector
    Utility,        // calculator, file manager, terminal
    Unknown,
}
```

**InputPattern.** How is the user interacting with the device? The compositor reports aggregated input statistics — never raw keystrokes (privacy). The engine sees cadence, not content.

```rust
pub struct InputActivity {
    /// Characters per minute (smoothed over 30s window)
    typing_cadence: f32,
    /// Mouse distance per second (smoothed)
    mouse_velocity: f32,
    /// Seconds since last input event
    idle_duration: f32,
    /// Dominant input type in last 60s
    dominant_input: InputType,
    /// Editing vs browsing ratio (keyboard/mouse balance)
    edit_ratio: f32,
}

pub enum InputType {
    Keyboard,       // typing-dominant (coding, writing)
    Mouse,          // pointing-dominant (browsing, design)
    Gamepad,        // game controller active
    Touch,          // touchscreen input
    Idle,           // no input for > 30s
}
```

Fast, sustained typing (high `typing_cadence`, high `edit_ratio`) → focused work. Mouse-dominant browsing with intermittent typing → casual work or leisure. Gamepad input → gaming. Extended idle → user stepped away.

**TimeOfDay.** What time is it? Morning hours typically correlate with work; late evening with leisure. But this is the weakest signal — it is easily overridden by stronger signals and exists primarily as a tiebreaker. The engine never assumes "it's 9 AM therefore the user is working" when the user is playing a game.

```rust
pub struct TimeSignal {
    local_time: Time,
    day_of_week: DayOfWeek,
    is_holiday: bool,           // from calendar data
}
```

**CalendarState.** Does the calendar have anything to say? Calendar events provide strong context. A "Team Standup" event means the user is in a meeting. A "Focus Time" block means the user wants concentration. Calendar-sourced context is high-confidence because the user explicitly scheduled it.

```rust
pub struct CalendarContext {
    current_event: Option<CalendarEvent>,
    next_event: Option<(CalendarEvent, Duration)>,  // event + time until start
    event_type: Option<CalendarEventType>,
}

pub enum CalendarEventType {
    Meeting,        // → suppress notifications, communication context
    FocusTime,      // → deep work, suppress everything non-critical
    Personal,       // → leisure-leaning
    Travel,         // → mobile context, reduced resource expectations
    Deadline,       // → high work engagement
}
```

**MediaPlayback.** Is media playing? Music alone is a weak signal — people code with music. But active video playback or a game running in the foreground is a strong leisure indicator. The media subsystem reports playback state.

```rust
pub struct MediaState {
    audio_playing: bool,
    video_playing: bool,
    media_type: Option<MediaType>,
    fullscreen: bool,
}

pub enum MediaType {
    Music,          // weak signal (ambiguous)
    Podcast,        // weak signal (ambiguous)
    Video,          // moderate signal (leisure-leaning)
    Game,           // strong signal (leisure)
    VideoCall,      // strong signal (communication/work)
}
```

**UserHistory.** What has the engine learned about this user over time? The History Store accumulates patterns: "this user codes 9-12 every weekday," "this user games after 8 PM on weekends," "switching to the research space usually means 2+ hours of focused work." History modifies the confidence of other signals. It does not generate context on its own.

```rust
pub struct Pattern {
    /// Time-based activity distribution
    time_distribution: Vec<TimeBucket>,
    /// Learned space-to-context mappings
    space_patterns: HashMap<SpaceId, ContextBias>,
    /// Learned agent-combination patterns
    agent_patterns: Vec<AgentCombinationPattern>,
    /// Average session durations by context
    session_durations: HashMap<ContextBucket, Duration>,
}

pub struct ContextBias {
    work_bias: f32,         // -1.0 (strong leisure) to 1.0 (strong work)
    confidence: f32,        // 0.0 (no data) to 1.0 (many observations)
}
```

**ExplicitIntent.** Did the user say something? "Heads down for two hours." "I'm done for the day." "Gaming time." Explicit intent has the highest weight — it overrides everything. It flows through the Override Manager, not through the inference model.

### 3.2 Signal Weights

Not all signals are equal. The engine assigns a base weight to each signal type. These weights determine influence on the final `ContextState` when using the rule-based fallback. When using the AIRS classifier, these weights serve as feature importance priors.

| Signal          | Base Weight | Rationale                                           |
|-----------------|-------------|-----------------------------------------------------|
| ExplicitIntent  | 1.0         | User said what they want. Overrides everything.     |
| CalendarState   | 0.8         | User scheduled it. High confidence, time-bounded.   |
| ActiveSpace     | 0.7         | Strong indicator — spaces have clear categories.    |
| RunningAgents   | 0.6         | Agent combinations reveal intent well.              |
| InputPattern    | 0.5         | Typing cadence is informative but noisy.            |
| MediaPlayback   | 0.5         | Video/game strong. Music alone ambiguous.           |
| UserHistory     | 0.4         | Modifier, not primary. Adjusts other signals.       |
| TimeOfDay       | 0.3         | Weakest. Tiebreaker only. Easily wrong.             |

**Weight adjustment.** UserHistory does not contribute a raw score — it adjusts the effective weight of other signals. If the engine has learned that this user always works in the "research" space, the ActiveSpace signal's effective weight increases when the research space is active. If the user sometimes games in the evening but sometimes codes, TimeOfDay's weight for that period decreases.

### 3.3 Signal Collection Frequency

Signals arrive through two mechanisms: event-driven push and periodic poll.

| Signal          | Mechanism    | Frequency / Trigger                             |
|-----------------|--------------|--------------------------------------------------|
| ActiveSpace     | Event-driven | Compositor reports focus change immediately      |
| RunningAgents   | Event-driven | Agent Runtime reports start/stop immediately     |
| InputPattern    | Polled       | Compositor publishes aggregate every 5 seconds   |
| TimeOfDay       | Polled       | Checked every 60 seconds                         |
| CalendarState   | Event-driven | Calendar agent reports event start/end           |
| MediaPlayback   | Event-driven | Media subsystem reports state change             |
| UserHistory     | On inference  | Read from History Store at each inference cycle   |
| ExplicitIntent  | Event-driven | Conversation Bar or Override Manager reports      |

**Event coalescing.** Rapid signal changes (user switching between spaces quickly) are coalesced. The engine waits 500ms after the last event before running inference. This prevents unnecessary computation during rapid transitions like Alt-Tab cycling.

```rust
pub struct SignalCollector {
    /// Accumulated signals since last inference
    pending_signals: Vec<TimestampedSignal>,
    /// Time of last signal arrival
    last_signal_time: Timestamp,
    /// Coalescing window
    coalesce_window: Duration,         // default: 500ms
    /// Subscriptions to event sources
    subscriptions: Vec<ChannelId>,
    /// Poll timer for periodic signals
    poll_timer: TimerId,
}

pub struct TimestampedSignal {
    signal: ContextSignal,
    received_at: Timestamp,
    source: SignalSource,
}

pub enum SignalSource {
    Compositor,
    AgentRuntime,
    CalendarAgent,
    MediaSubsystem,
    ConversationBar,
    InternalClock,
    HistoryStore,
}
```

-----

## 4. Context Inference

### 4.1 The Context Model

The Context Model transforms a vector of signals into a `ContextState`. Two implementations exist:

**AIRS classifier (primary).** A small classifier model — not a full LLM, not generative. It takes a fixed-length feature vector extracted from the current signals and outputs a `ContextState`. The model is trained on labeled context data and runs in under 1ms on CPU. It ships as a GGUF model in the system model registry.

```rust
pub struct ContextClassifier {
    /// GGML model handle
    model: ModelHandle,
    /// Feature extraction pipeline
    feature_extractor: FeatureExtractor,
    /// Output post-processor (raw logits → ContextState)
    post_processor: PostProcessor,
}

pub struct FeatureExtractor {
    /// Converts ContextSignal variants into a fixed-length f32 vector
    feature_dim: usize,     // 32 features
}

impl FeatureExtractor {
    /// Extracts a normalized feature vector from current signals
    pub fn extract(&self, signals: &[ContextSignal]) -> Vec<f32> {
        let mut features = vec![0.0f32; self.feature_dim];

        // Features 0-3: ActiveSpace category (one-hot encoded)
        // Features 4-7: RunningAgents category distribution
        // Features 8-11: InputPattern (cadence, velocity, idle, edit_ratio)
        // Features 12-14: TimeOfDay (hour_sin, hour_cos, is_weekend)
        // Features 15-17: CalendarState (in_meeting, in_focus, has_deadline)
        // Features 18-20: MediaPlayback (audio, video, fullscreen)
        // Features 21-27: UserHistory (bias adjustments per signal)
        // Features 28-31: ExplicitIntent (has_override, work_intent, leisure_intent, ttl)

        // ... normalization to [0.0, 1.0] range ...
        features
    }
}
```

**Rule-based fallback (secondary).** A weighted average computation that does not require AIRS. Each signal produces a score for each `ContextState` field. Scores are combined using the base weights from Section 3.2. Simpler, less nuanced, but functional.

```rust
pub struct RuleBasedModel {
    weights: SignalWeights,
}

pub struct SignalWeights {
    explicit_intent: f32,   // 1.0
    calendar_state: f32,    // 0.8
    active_space: f32,      // 0.7
    running_agents: f32,    // 0.6
    input_pattern: f32,     // 0.5
    media_playback: f32,    // 0.5
    user_history: f32,      // 0.4
    time_of_day: f32,       // 0.3
}

impl RuleBasedModel {
    pub fn infer(&self, signals: &[ContextSignal]) -> ContextState {
        let mut work_score = 0.0f32;
        let mut total_weight = 0.0f32;

        for signal in signals {
            let (score, weight) = match signal {
                ContextSignal::ActiveSpace(id) => {
                    let s = match self.space_category(id) {
                        SpaceCategory::Work => 0.9,
                        SpaceCategory::Communication => 0.6,
                        SpaceCategory::Media => 0.1,
                        SpaceCategory::Gaming => 0.0,
                        _ => 0.5,
                    };
                    (s, self.weights.active_space)
                }
                ContextSignal::InputPattern(activity) => {
                    let s = (activity.typing_cadence / 200.0)
                        .min(1.0)
                        .max(0.0)
                        * activity.edit_ratio;
                    (s, self.weights.input_pattern)
                }
                // ... other signal types ...
                _ => continue,
            };
            work_score += score * weight;
            total_weight += weight;
        }

        let work_engagement = if total_weight > 0.0 {
            (work_score / total_weight).clamp(0.0, 1.0)
        } else {
            0.5 // no signals → neutral
        };

        ContextState {
            work_engagement,
            ai_engagement: Self::derive_ai_engagement(work_engagement),
            notification_threshold: Self::derive_notification_threshold(work_engagement),
            resource_priority: Self::derive_resource_priority(work_engagement, signals),
        }
    }

    fn derive_ai_engagement(work: f32) -> AiEngagement {
        if work >= 0.7 {
            AiEngagement::Available
        } else if work >= 0.3 {
            AiEngagement::Ambient
        } else {
            AiEngagement::Invisible
        }
    }

    fn derive_notification_threshold(work: f32) -> Urgency {
        if work >= 0.8 {
            Urgency::Interrupt       // deep work: only urgent items
        } else if work >= 0.5 {
            Urgency::NextBreak       // moderate work: show at pause
        } else {
            Urgency::Digest          // leisure: batch everything
        }
    }
}
```

### 4.2 Inference Pipeline

The full inference pipeline runs every time a significant signal change is detected:

```
Signal event arrives (or poll timer fires)
  │
  ▼
Signal Collector: append to pending_signals
  │
  ▼
Coalescing check: has 500ms passed since last signal?
  │
  ├── No  → wait (more signals may arrive)
  │
  └── Yes → proceed to inference
        │
        ▼
  Assemble signal vector (all current signals, not just pending)
        │
        ▼
  Override check: is an active override in effect?
        │
        ├── Yes → use override's ContextState directly, skip model
        │
        └── No  → run model inference
              │
              ├── AIRS available → FeatureExtractor → Classifier (~1ms)
              │
              └── AIRS unavailable → RuleBasedModel → weighted average (~0.1ms)
              │
              ▼
        New ContextState produced
              │
              ▼
        Hysteresis check: compare with current state
              │
              ├── Significant change → publish update to all consumers
              │
              └── Minor change → accumulate, do not publish
                    (debounce threshold: 0.1 change in work_engagement,
                     or any change in ai_engagement tier)
```

```rust
pub struct InferencePipeline {
    collector: SignalCollector,
    classifier: Option<ContextClassifier>,   // None if AIRS unavailable
    fallback: RuleBasedModel,
    override_manager: OverrideManager,
    current_state: ContextState,
    publisher: StatePublisher,
    hysteresis: HysteresisConfig,
}

pub struct HysteresisConfig {
    /// Minimum change in work_engagement to trigger publish
    work_engagement_threshold: f32,          // default: 0.1
    /// Minimum time between state transitions
    min_transition_interval: Duration,       // default: 10s
    /// Time sustained signals must persist before work→leisure transition
    work_to_leisure_delay: Duration,         // default: 5 min
    /// Time sustained signals must persist before leisure→work transition
    leisure_to_work_delay: Duration,         // default: 2 min
}

impl InferencePipeline {
    pub async fn run_inference(&mut self) -> Option<ContextState> {
        let signals = self.collector.take_pending();
        let all_signals = self.collector.current_snapshot();

        // Check overrides first
        if let Some(active_override) = self.override_manager.active() {
            let state = active_override.effect.clone();
            if state != self.current_state {
                self.current_state = state.clone();
                self.publisher.publish(state.clone()).await;
                return Some(state);
            }
            return None;
        }

        // Run model inference
        let candidate = if let Some(ref classifier) = self.classifier {
            let features = classifier.feature_extractor.extract(&all_signals);
            classifier.model.classify(&features)
        } else {
            self.fallback.infer(&all_signals)
        };

        // Hysteresis: only publish if change is significant
        if self.is_significant_change(&candidate) {
            self.current_state = candidate.clone();
            self.publisher.publish(candidate.clone()).await;
            Some(candidate)
        } else {
            None
        }
    }
}
```

### 4.3 State Transitions

Context does not flip instantly. Hysteresis prevents flickering.

The problem: a user is deep in a coding session (work_engagement: 0.95). They briefly open a browser to check a game score. Without hysteresis, the context flips to leisure, notifications flood in, the compositor rearranges, and three seconds later when the user returns to coding, everything flips back. This is worse than no context engine at all.

The solution: directional transition delays.

```
Work → Leisure transition:
  Sustained leisure signals required for 5 minutes.
  Why 5 min: quick breaks (checking phone, grabbing coffee,
  glancing at social media) should not trigger a full context switch.

Leisure → Work transition:
  Sustained work signals required for 2 minutes.
  Why 2 min: when the user sits down to work, the system should
  respond quickly. 2 minutes is long enough to filter accidental
  signals but short enough to feel responsive.

Tier changes within work or leisure:
  Available → Ambient: 3 minutes of reduced engagement
  Ambient → Invisible: 5 minutes of deep leisure signals
  Invisible → Ambient: 1 minute of moderate engagement
  Ambient → Available: 2 minutes of strong work signals
```

```rust
pub struct TransitionState {
    /// Direction of pending transition
    pending: Option<PendingTransition>,
    /// Timestamp when sustained signal was first detected
    sustained_since: Option<Timestamp>,
}

pub struct PendingTransition {
    target: ContextState,
    direction: TransitionDirection,
    required_duration: Duration,
    started_at: Timestamp,
}

pub enum TransitionDirection {
    WorkToLeisure,       // requires 5 min sustained
    LeisureToWork,       // requires 2 min sustained
    EngagementIncrease,  // requires 2 min sustained
    EngagementDecrease,  // requires 3 min sustained
}
```

If the sustained signal is interrupted (user returns to coding during a pending work-to-leisure transition), the pending transition is cancelled and the timer resets. The current state remains unchanged.

### 4.4 The ContextState

The `ContextState` is the engine's output. Every consumer reads this struct. It contains four fields, each driving different system behaviors.

```rust
/// Discretized context mode, derived from the continuous work_engagement score.
/// Used by the Attention Manager and compositor for coarse-grained decisions.
pub enum ContextMode {
    /// work_engagement < 0.3: gaming, media, casual browsing
    Leisure,
    /// work_engagement 0.3–0.7: mixed activity, light tasks
    Focus,
    /// work_engagement > 0.7: deep work, writing, coding
    Work,
    /// Detected via active game process or gamepad input
    Gaming,
}

pub struct ContextState {
    /// 0.0 = deep leisure, 1.0 = deep work
    work_engagement: f32,

    /// How visible AI should be to the user
    ai_engagement: AiEngagement,

    /// What level of notification gets through
    notification_threshold: Urgency,

    /// Which scheduling class gets weight boost
    resource_priority: ResourcePriority,
}

impl ContextState {
    /// Derive the discrete ContextMode from continuous work_engagement.
    pub fn mode(&self) -> ContextMode {
        match self.work_engagement {
            x if x < 0.3 => ContextMode::Leisure,
            x if x > 0.7 => ContextMode::Work,
            _ => ContextMode::Focus,
        }
    }
}

pub enum AiEngagement {
    /// Pure infrastructure. AI does scheduling, security, indexing.
    /// User sees no AI. Conversation bar hidden or minimized.
    /// Triggered by: gaming, media playback, casual browsing.
    Invisible,

    /// Results visible, process hidden. Search works better,
    /// defaults adapt, suggestions appear in context but are
    /// not conversational. Conversation bar is subtle.
    /// Triggered by: light work, mixed activity.
    Ambient,

    /// Conversation bar prominent and responsive. Suggestions
    /// actively offered. AI-driven features front and center.
    /// Triggered by: active work in spaces, explicit invocation.
    Available,
}

pub enum ResourcePriority {
    /// Boost foreground interactive agent (gaming, media)
    Foreground,
    /// Balanced allocation (normal work)
    Balanced,
    /// Boost background work agents (compilation, indexing)
    BackgroundWork,
}
```

**work_engagement** (f32, 0.0-1.0). The primary output dimension. This drives downstream decisions in every consumer. A value of 0.0 means the user is in deep leisure — gaming, watching a movie, relaxing. A value of 1.0 means deep focused work — writing code, analyzing data, composing a document. Values in between represent mixed or transitional contexts.

| Range       | Interpretation       | System behavior                              |
|-------------|----------------------|----------------------------------------------|
| 0.0 - 0.2  | Deep leisure         | Invisible AI, minimal notifications, foreground priority |
| 0.2 - 0.4  | Light leisure        | Invisible AI, batched notifications          |
| 0.4 - 0.6  | Mixed / transitional | Ambient AI, notifications at pause           |
| 0.6 - 0.8  | Active work          | Available AI, selective notifications        |
| 0.8 - 1.0  | Deep focus           | Available AI, only urgent interrupts         |

**ai_engagement** (enum). Controls AIRS visibility. At `Invisible`, the conversation bar is hidden or minimized to a dot. AIRS still runs — it does scheduling, security, indexing — but the user sees none of it. At `Ambient`, AIRS results appear in context (search results are better, defaults adapt) but there is no conversational interface. At `Available`, the conversation bar is prominent and ready for interaction.

**notification_threshold** (Urgency). The minimum urgency an `AttentionItem` must have to be shown to the user. During deep focus, only `Interrupt`-level items get through — system errors, calls from starred contacts, calendar alarms. During leisure, everything is batched into periodic digests.

**resource_priority** (ResourcePriority). Hint to the scheduler. During gaming, the foreground game agent gets maximum CPU/GPU allocation. During background compilation, background work agents get a boost.

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
    /// Active override stack (most recent first)
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

```
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

```
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
            Urgency::Digest => true,
            Urgency::Silent => true,
        }
    }
}
```

**Digest delivery.** Every 30 minutes (configurable), the Attention Manager bundles queued Digest items into a summary and presents it as a single notification: "You have 3 messages, 2 email threads, and 1 PR review waiting." AIRS generates the summary text. The user can expand for details or dismiss.

### 6.3 Compositor

The compositor adapts the visual environment based on context.

```
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

## 7. Learning and Personalization

### 7.1 Pattern Learning

Over time, the Context Engine learns user-specific patterns that improve inference accuracy. Learning is passive — the engine observes context transitions and their outcomes, not user content.

What the engine learns:

- **Typical work hours.** "This user codes Monday-Friday 9 AM to 12 PM and 1 PM to 5 PM. On weekends, they sometimes code in the morning but not reliably."
- **Space-context associations.** "The 'research' space is always work. The 'music' space is ambiguous. The 'game-saves' space is always leisure."
- **Agent combination patterns.** "When the IDE agent and terminal agent are both active, this user is in deep work for an average of 2.3 hours."
- **Override patterns.** "This user activates 'heads down' about 3 times per week, always in the afternoon, always for 1-2 hours."
- **Notification response patterns.** "During high work_engagement, this user ignores Digest notifications for 2+ hours. During leisure, they respond to NextBreak notifications within 5 minutes."

Learning happens through observation of context transitions and their stability:

```rust
pub struct LearningEngine {
    /// Accumulated observations
    observations: Vec<ContextObservation>,
    /// Derived patterns
    patterns: Pattern,
    /// Learning rate (how quickly patterns update)
    learning_rate: f32,
    /// Minimum observations before a pattern is used
    min_observations: usize,         // default: 20
}

pub struct ContextObservation {
    /// What signals were active
    signals: Vec<ContextSignal>,
    /// What context was inferred
    inferred_state: ContextState,
    /// How long the context was stable
    stability_duration: Duration,
    /// Did the user override?
    was_overridden: bool,
    /// Timestamp
    timestamp: Timestamp,
}

impl LearningEngine {
    pub fn observe(&mut self, obs: ContextObservation) {
        self.observations.push(obs.clone());

        // If the user overrode an inferred state, that's a correction signal.
        // The inference was wrong. Adjust patterns to prevent the same mistake.
        if obs.was_overridden {
            self.adjust_for_correction(&obs);
        }

        // If the inferred state was stable for > 10 minutes without override,
        // that's a confirmation signal. The inference was right. Reinforce.
        if obs.stability_duration > Duration::from_secs(600) && !obs.was_overridden {
            self.reinforce(&obs);
        }

        // Periodically recompute derived patterns
        if self.observations.len() % 50 == 0 {
            self.recompute_patterns();
        }
    }
}
```

**Override correction.** If the user frequently overrides the inferred context for a specific signal combination, the engine learns to infer differently next time. Example: the engine infers leisure because it's 9 PM, but the user says "heads down." After several such corrections, the engine reduces the weight of TimeOfDay for this user's evening hours.

### 7.2 Privacy

All learning data stays local. Nothing leaves the device.

- **Storage.** Patterns are stored in `system/context/` space. This is a system space, not readable by third-party agents.
- **Inspection.** The user can open the Inspector and see exactly what the Context Engine has learned: time-based patterns, space associations, agent combination mappings. No black boxes.
- **Deletion.** The user can delete all learned patterns at any time. The engine resets to base weights and starts learning from scratch.
- **Disabling.** The user can disable learning entirely. The engine will still function using base weights and explicit overrides. Inference quality will be lower but the system will work.
- **No content access.** The engine never sees document content, message text, or browsing history. It sees structural signals: which space, which agents, what input cadence, what time. The engine knows "the user is typing fast in the 'research' space" but never "the user is writing about quantum computing."

```rust
pub struct PrivacyControls {
    /// Is learning enabled?
    learning_enabled: bool,
    /// Maximum observation retention period
    retention_period: Duration,         // default: 90 days
    /// Space where patterns are stored
    storage_space: SpaceId,            // system/context/
}

impl PrivacyControls {
    pub fn delete_all_patterns(&mut self) {
        self.storage_space.delete_all();
        // Engine continues with base weights
    }

    pub fn export_patterns(&self) -> PatternExport {
        // User can export learned patterns for inspection
        // Returns human-readable summary, not raw model weights
        PatternExport {
            work_hours: self.describe_work_hours(),
            space_associations: self.describe_space_patterns(),
            common_overrides: self.describe_override_patterns(),
        }
    }
}
```

-----

## 8. Fallback (Without AIRS)

### 8.1 Rule-Based Fallback

When AIRS is unavailable — during early boot before AIRS loads, during resource pressure when the inference engine is paused, or if the user has disabled AIRS — the Context Engine falls back to the `RuleBasedModel` described in Section 4.1.

The fallback uses the same signal collection, the same override system, and the same state publishing. Only the inference step changes: instead of a classifier model, a weighted average computes the `ContextState`.

```
Fallback inference:

1. For each signal, compute a work_engagement score (0.0-1.0)
   based on hard-coded rules:

   ActiveSpace:
     Work/System → 0.9
     Communication → 0.6
     Personal → 0.5
     Media → 0.2
     Gaming → 0.0
     Unknown → 0.5

   RunningAgents:
     Productivity agents > 50% of active → 0.8
     Gaming agents present → 0.1
     Mixed → 0.5

   TimeOfDay (weekday):
     06:00-09:00 → 0.6 (morning, probably work)
     09:00-17:00 → 0.7 (business hours)
     17:00-21:00 → 0.4 (evening, ambiguous)
     21:00-06:00 → 0.2 (night, probably leisure)

   TimeOfDay (weekend):
     All hours → 0.3 (leisure bias)

2. Multiply each score by its base weight
3. Sum and normalize → work_engagement
4. Derive ai_engagement, notification_threshold,
   resource_priority from work_engagement using
   the same thresholds as the classifier
```

```rust
impl RuleBasedModel {
    fn time_of_day_score(time: &TimeSignal) -> f32 {
        let hour = time.local_time.hour();
        let weekend = matches!(time.day_of_week, DayOfWeek::Saturday | DayOfWeek::Sunday)
            || time.is_holiday;

        if weekend {
            return 0.3;
        }

        match hour {
            6..=8   => 0.6,
            9..=16  => 0.7,
            17..=20 => 0.4,
            _       => 0.2,
        }
    }
}
```

### 8.2 Fallback Quality

The rule-based fallback is functional but noticeably less nuanced than the AIRS classifier:

| Scenario                                    | AIRS classifier | Rule-based fallback |
|---------------------------------------------|-----------------|---------------------|
| Coding at 10 PM on a weekday                | Work (correct)  | Leisure (wrong — TimeOfDay dominates) |
| Browsing documentation during gaming break  | Leisure (correct, sustained gaming context) | Mixed (unstable — ActiveSpace flickers) |
| Video call that is actually a social hangout | Communication (adapts from conversation signals) | Work (wrong — CalendarState says "meeting") |
| User's personal coding style (very slow typing) | Work (learned pattern) | Mixed (InputPattern says low cadence = leisure) |

The fallback works well for clear-cut situations: 9 AM on a Monday with an IDE open is obviously work. A game running fullscreen on a Saturday night is obviously leisure. It struggles with ambiguous or unusual situations where the AIRS classifier would use learned patterns and cross-signal correlation.

**When AIRS is unavailable, explicit overrides become more important.** The system should suggest overrides more proactively: if the fallback keeps getting it wrong, the user learns to say "heads down" or "gaming." This is still better than no context engine at all — the override system alone is more useful than manual mode-switching.

### 8.3 Boot-Time Context Behavior

During boot, the Context Engine faces a unique situation: it must publish a `ContextState` before any meaningful user signals exist. This section specifies exactly what happens between the Context Engine starting (Phase 3, after AIRS — see [boot.md §4.5](../kernel/boot.md) dependency graph) and the first real user activity.

**Boot-time signal availability:**

| Signal | Available at boot? | Value during boot |
|---|---|---|
| ActiveSpace | No (compositor not running) | `None` — no space is active |
| RunningAgents | Partial (system agents only) | System agents starting; no user agents yet |
| InputPattern | No (no user input yet) | `InputActivity::Idle` |
| TimeOfDay | Yes | Current wall-clock time |
| CalendarState | No (calendar agent not started) | `CalendarContext::Unknown` |
| LocationContext | No (location service not started) | `LocationContext::Unknown` |
| BatteryState | Yes (HAL provides at Phase 2) | Current battery level and AC state |
| ExternalDisplay | Yes (HAL provides at Phase 2) | Whether external display is connected |

**Boot-time inference.** With only TimeOfDay and hardware signals available, the Context Engine produces a conservative initial state:

```rust
impl ContextEngine {
    /// Generate the initial ContextState during boot.
    /// Called once during Phase 3 initialization, before any user signals.
    fn boot_context(&self) -> ContextState {
        let time_signal = self.signal_collector.time_of_day();
        let battery = self.signal_collector.battery_state();

        // Use time-of-day as the primary signal
        let work_engagement = RuleBasedModel::time_of_day_score(&time_signal);

        // Battery-aware adjustment: if battery is critical, reduce resource priority
        let resource_priority = if battery.level < 0.10 && !battery.ac_connected {
            ResourcePriority::Minimal
        } else {
            ResourcePriority::from_engagement(work_engagement)
        };

        ContextState {
            context_mode: ContextMode::Default,
            work_engagement,
            ai_engagement: AiEngagement::Ambient, // conservative: don't pop up AI UI
            notification_threshold: NotificationThreshold::Medium,
            resource_priority,
            confidence: ContextConfidence::Low,    // we know we're guessing
            source: ContextSource::BootHeuristic,
            active_override: None,
        }
    }
}
```

**Key design decisions for boot context:**

1. **`AiEngagement::Ambient`, not `Available`.** During boot, the system should not proactively show AI UI (Conversation Bar, suggestion panels). The user may be waiting for the desktop to appear. Once the user interacts and signals accumulate, the engagement level adjusts naturally.

2. **`ContextConfidence::Low`.** The boot context explicitly marks itself as low-confidence. Consumers that check confidence (e.g., the scheduler's context multiplier) use a conservative default instead of the inferred value when confidence is low.

3. **`ContextSource::BootHeuristic`.** The audit log records that this context state came from boot heuristics, not from real signal analysis. Useful for debugging context transitions.

**Transition to real context.** Once the compositor starts (Phase 5) and the user begins interacting, real signals flow in. The Context Engine transitions from boot heuristics to normal inference:

```
Boot context lifecycle:

Phase 3: Context Engine starts (after AIRS, non-critical path)
  → Publishes BootHeuristic context (Low confidence)
  → All consumers receive conservative defaults

Phase 5: Compositor + agents start
  → ActiveSpace signal arrives (user's last workspace)
  → RunningAgents signal populates as agents launch
  → Confidence rises to Medium

Phase 5 + 30s: User interacts
  → InputPattern signal activates
  → Confidence rises to High
  → Context Engine switches from rule-based to AIRS classifier
    (if AIRS model is loaded by now)

Phase 5 + 5min: Steady state
  → All 8 signal sources active
  → AIRS classifier running
  → Learning engine observing
  → Full confidence
```

**Semantic Resume integration.** When the system restores a previous session via Semantic Resume (see [boot-lifecycle.md §15.3](../kernel/boot-lifecycle.md)), the Context Engine receives a hint about the user's pre-reboot context from the resume state. If the user was in deep work before a crash, the Context Engine initializes with a work-biased context rather than a neutral boot context. This reduces the jarring transition of "I was coding, the system crashed, and now it thinks I'm leisuring."

```rust
pub enum BootContextHint {
    /// Clean boot — no prior context information
    ColdBoot,
    /// Semantic Resume — we know what the user was doing
    SemanticResume { previous_context: ContextState },
    /// Recovery mode — system is in a degraded state
    RecoveryMode,
    /// Proactive wake — system woke up for a scheduled task
    ProactiveWake { scheduled_task: TaskDescription },
}
```

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

Agents post attention items through the Attention Manager. The agent declares the content and a suggested urgency. AIRS re-assesses the urgency based on the actual content and current context.

```rust
ctx.attention().post(AttentionItem {
    content: AttentionContent::text("Meeting in 5 minutes: Team Standup"),
    urgency: Urgency::NextBreak,
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

```
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

-----

## 11. Implementation Order

The Context Engine is built incrementally. Each development phase (numbered per the project development plan in [development-plan.md](../project/development-plan.md), not to be confused with boot phases) delivers testable functionality. Later phases add intelligence.

```
Dev Phase 8: Basic Context Engine
  ├── ContextState struct and IPC publishing
  ├── Signal Collector (ActiveSpace, RunningAgents, TimeOfDay)
  ├── Rule-based inference (weighted average, no AIRS)
  ├── Override Manager (explicit overrides, time-bounded)
  ├── State Publisher (pub/sub to consumers)
  ├── Hysteresis (transition delays, debouncing)
  ├── Scheduler integration (context multipliers)
  └── Inspector diagnostics (current state, signal values)

Dev Phase 8: AIRS Classifier Integration
  ├── Feature extractor (signals → fixed-length vector)
  ├── Context classifier model (small GGUF, ~2 MB)
  ├── AIRS inference integration (~1ms per inference)
  ├── Fallback detection (AIRS unavailable → rule-based)
  └── Classifier training pipeline (offline, ship with OS)

Dev Phase 8: Attention Manager Integration
  ├── AttentionItem processing pipeline
  ├── AIRS urgency re-assessment
  ├── Notification threshold filtering
  ├── Grouping and digest batching
  ├── Compositor notification routing
  └── Auto-action support

Dev Phase 14: Learning and Personalization
  ├── Observation recording (context transitions, stability)
  ├── Pattern extraction (work hours, space associations)
  ├── Override correction learning
  ├── UserHistory signal integration
  ├── Privacy controls (inspect, delete, disable)
  └── Pattern export for Inspector

Dev Phase 19: Power-Aware Context
  ├── Battery level as signal (low battery → resource conservation)
  ├── Thermal state as signal (throttled → reduce background work)
  ├── Power source as signal (plugged in → less conservative)
  └── Integration with scheduler power management
```

**Critical dependencies:**

- Context Engine requires IPC (dev phase 3) — all signal collection and state publishing is IPC-based.
- Context Engine requires Compositor (dev phase 6) — ActiveSpace and InputPattern signals come from the compositor.
- Context Engine requires Agent Runtime (dev phase 7) — RunningAgents signal comes from the Agent Runtime.
- AIRS classifier requires AIRS inference engine (dev phase 8) — the classifier runs on AIRS.
- Attention Manager requires AIRS (dev phase 8) — urgency re-assessment needs inference.
- Learning requires History Store in spaces (dev phase 4) — patterns stored in `system/context/` space.

**Testing strategy.** The rule-based fallback is tested first and serves as the reference implementation. The AIRS classifier must match or exceed the fallback's accuracy on a labeled test set before it replaces the fallback as the primary inference path. Both paths are always available — the system can switch between them at runtime.

-----

## 12. Design Principles

1. **No toggles, no modes.** The user never switches between "work mode" and "play mode." The OS infers context continuously. The user can override, but they never have to manage.

2. **Overrides expire.** Every override is time-bounded. Forgotten overrides cannot exist. The system always returns to inference.

3. **Hysteresis over responsiveness.** A context engine that flickers between states is worse than no context engine. Transitions are delayed, debounced, and directional. The system may be slow to react, but it is never wrong for long.

4. **Signals, not surveillance.** The engine sees structural metadata: which space, which agents, what input cadence, what time. It never sees content. The user can inspect everything the engine knows, delete it, or disable learning entirely.

5. **Graceful degradation.** Without AIRS, the engine falls back to rules. Without rules, explicit overrides still work. Without overrides, the system defaults to `Ambient` AI and `NextBreak` notifications. There is always a functional floor.

6. **Consumers react, they don't control.** Consumers subscribe to `ContextState` and adapt. They cannot modify the context. Only the user can override. This prevents agents from gaming the context system to get more resources or attention.

7. **AIRS enhances, rules suffice.** The AIRS classifier makes the engine smarter — it catches edge cases, learns personal patterns, correlates signals that the rule-based model misses. But the rule-based model works. Shipping the rule-based model alone delivers value. AIRS makes it great.

8. **Transparency.** The Inspector shows every signal, every weight, every transition, every learned pattern. The context engine is not a black box. The user can always understand why the system is behaving the way it is.
