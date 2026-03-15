# AIOS Context Engine — Signal Collection

Part of: [context-engine.md](../context-engine.md) — Context Engine
**Related:** [inference.md](./inference.md) — Context inference pipeline, [learning.md](./learning.md) — Pattern learning from signals

-----

## 3. Signal Collection

### 3.1 Signal Sources

The engine collects eight signal types. Each maps to a variant of the `ContextSignal` enum from the architecture document:

```rust
pub enum ContextSignal {
    ActiveSpace(SpaceId),
    RunningAgents(Vec<AgentId>),
    InputPattern(InputActivity),
    TimeOfDay(TimeSignal),
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
