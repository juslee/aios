# AIOS Context Engine — Context Inference

Part of: [context-engine.md](../context-engine.md) — Context Engine
**Related:** [signals.md](./signals.md) — Signal sources and weights, [overrides.md](./overrides.md) — Override system, [consumers.md](./consumers.md) — State consumers

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

```text
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

```text
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
