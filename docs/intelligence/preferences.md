# AIOS Preference System

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [airs.md](./airs.md) — Behavioral inference, [context-engine.md](./context-engine.md) — Context-driven preference adaptation, [experience.md](../experience/experience.md) — Conversation Bar configuration, [spaces.md](../storage/spaces.md) — Preference storage and sync, [agents.md](../applications/agents.md) — Agent preference scoping

-----

## 1. Overview

Settings panels are broken. Every operating system ships a Settings application with hundreds of options organized by developer logic, not user need. Want to change the font size? Settings → Display → Font Size → Advanced → Scaling Factor. Want to stop notifications at night? Settings → Notifications → Do Not Disturb → Schedule → Custom. Want to change the mouse speed? Settings → Input → Mouse → Pointer Speed → (which of the three sliders?).

Users don't find settings. They search the web for "how to make text bigger on [OS name]" and follow a tutorial. The settings panel is a failure of design — it forces the user to learn the developer's mental model of the system instead of meeting the user where they are.

Config files are worse. `.bashrc`, `.vimrc`, `~/.config/appname/settings.json`, `/etc/sysctl.conf`, environment variables, registry keys, plist files, dconf databases. Every application invents its own configuration format, stored in its own location, with its own syntax. There is no discoverability, no history, no explanation of what each setting does or why it's set to its current value.

AIOS replaces all of this with the **Preference System** — a unified, conversational, behavioral, evolving configuration layer.

**How it works:**

- "Make the text bigger" → Preference Service increases font scale → compositor re-renders. Done.
- "I don't like the blue accent color" → theme accent changes. Done.
- "Stop notifications at night" → attention suppression schedule created. Done.
- User always reduces brightness after 8pm → AIRS proposes auto-dim. User approves once.
- Agent suggests a configuration change → user sees the proposal with explanation, accepts or rejects.

No settings panel required. No config files to edit. No documentation to read. The computer adapts to the user, not the other way around.

-----

## 2. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                                                                   │
│  User                                                             │
│  ├── Conversation Bar: "Make the text bigger"                    │
│  ├── Settings UI: visual preference browser                      │
│  └── Agent UI: agent-specific preference controls                │
│                                                                   │
│         │              │                │                         │
│         ▼              ▼                ▼                         │
│  ┌──────────────────────────────────────────────────────────────┐│
│  │                  Preference Service                          ││
│  │                (privileged system service)                    ││
│  │                                                              ││
│  │  ┌──────────────┐  ┌──────────────┐  ┌───────────────────┐ ││
│  │  │ NLU Resolver  │  │ Preference   │  │ Behavioral        │ ││
│  │  │               │  │ Store        │  │ Observer          │ ││
│  │  │ Natural lang  │  │              │  │                   │ ││
│  │  │ → preference  │  │ user/        │  │ Watches user      │ ││
│  │  │   change      │  │ preferences/ │  │ patterns, infers  │ ││
│  │  │               │  │ space        │  │ preference changes│ ││
│  │  └──────────────┘  └──────────────┘  └───────────────────┘ ││
│  │                                                              ││
│  │  ┌──────────────┐  ┌──────────────┐  ┌───────────────────┐ ││
│  │  │ Change        │  │ Conflict     │  │ History           │ ││
│  │  │ Propagator    │  │ Resolver     │  │ Manager           │ ││
│  │  │               │  │              │  │                   │ ││
│  │  │ Notifies      │  │ UserExplicit │  │ Every change      │ ││
│  │  │ affected      │  │ wins, but    │  │ recorded with     │ ││
│  │  │ components    │  │ explains     │  │ timestamp, source │ ││
│  │  │ via IPC       │  │ tradeoffs    │  │ and reason        │ ││
│  │  └──────────────┘  └──────────────┘  └───────────────────┘ ││
│  └──────────────────────────────────────────────────────────────┘│
│         │              │              │              │            │
│         ▼              ▼              ▼              ▼            │
│    Compositor      Audio         Attention      Network          │
│    (display,       Service       Manager        Service          │
│     theme,         (volume,      (thresholds,   (metered,        │
│     density)       output)       schedule)      VPN)             │
│                                                                   │
└──────────────────────────────────────────────────────────────────┘
```

-----

## 3. The Preference

### 3.1 Data Model

```rust
pub struct Preference {
    /// Unique identifier (hierarchical: "display.font_scale")
    pub id: PreferenceId,

    /// Human-readable description
    pub description: String,

    /// Current value
    pub value: PreferenceValue,

    /// Who/what set this value
    pub source: PreferenceSource,

    /// System components affected by this preference
    pub affects: Vec<SystemComponent>,

    /// Full change history
    pub history: Vec<PreferenceChange>,

    /// Metadata for UI generation
    pub metadata: PreferenceMetadata,
}

pub enum PreferenceValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Enum { value: String, options: Vec<String> },
    Color(Color),
    Duration(Duration),
    Size { width: f64, height: f64 },
    Range { value: f64, min: f64, max: f64, step: f64 },
    Complex(serde_json::Value),
}

pub enum PreferenceSource {
    /// User directly stated this preference
    /// Highest authority — never overridden silently
    UserExplicit {
        method: ExplicitMethod,
        timestamp: SystemTime,
    },

    /// AIRS inferred this from user behavior
    /// Medium authority — can be overridden, user was informed
    UserBehaviorInferred {
        observation: String,
        confidence: f32,
        timestamp: SystemTime,
    },

    /// An agent suggested this change
    /// Low authority — requires user approval
    AgentSuggested {
        agent: AgentId,
        reason: String,
        approved: bool,
        timestamp: SystemTime,
    },

    /// Factory default
    /// Lowest authority — overridden by everything
    SystemDefault,
}

pub enum ExplicitMethod {
    /// Set via Conversation Bar ("make text bigger")
    ConversationBar,
    /// Set via Settings UI
    SettingsUI,
    /// Set via agent-specific UI
    AgentUI { agent: AgentId },
    /// Set via SDK API
    ProgrammaticApi,
}

pub struct PreferenceMetadata {
    /// Category for UI grouping
    pub category: PreferenceCategory,
    /// Whether this is device-specific or universal
    pub scope: PreferenceScope,
    /// Minimum system version that supports this preference
    pub since_version: Version,
    /// Whether changing this requires restart
    pub requires_restart: bool,
    /// Related preferences (e.g., font_scale relates to display.density)
    pub related: Vec<PreferenceId>,
    /// Validation constraints
    pub constraints: Option<ValueConstraints>,
}

pub enum PreferenceCategory {
    Display,
    Audio,
    Input,
    Network,
    Privacy,
    Agents,
    Accessibility,
    Power,
    Attention,
    Context,
    Storage,
}

pub enum PreferenceScope {
    /// Same value across all devices
    Universal,
    /// Different value per device (e.g., display brightness)
    PerDevice,
}

/// System components that a preference can affect (referenced by architecture.md §2.8).
pub enum SystemComponent {
    Compositor,
    Scheduler,
    AttentionManager,
    ContextEngine,
    FlowService,
    Airs,
    SpaceStorage,
    NetworkTranslation,
    AudioService,
    InputService,
    PowerManager,
    IdentityService,
    PosixLayer,
}

/// Named collection of preferences associated with an identity profile
/// (referenced by architecture.md §6.3 and agents.md).
pub struct PreferenceSet {
    pub preferences: Vec<Preference>,
}
```

### 3.2 Preference Change Record

Every change is recorded:

```rust
pub struct PreferenceChange {
    /// What changed
    pub preference_id: PreferenceId,
    /// Previous value
    pub old_value: PreferenceValue,
    /// New value
    pub new_value: PreferenceValue,
    /// Who/what made the change
    pub source: PreferenceSource,
    /// Why the change was made (human-readable)
    pub reason: String,
    /// When
    pub timestamp: SystemTime,
    /// Whether this change was reverted
    pub reverted: bool,
}
```

-----

## 4. Preference Sources and Precedence

### 4.1 Authority Ranking

```
┌──────────────────────────────────────────────────────────────────┐
│  HIGHEST AUTHORITY                                                │
│                                                                   │
│  1. UserExplicit                                                  │
│     "I said dark mode." This is never overridden silently.       │
│     If a system component wants to change an explicit pref,      │
│     it must explain why and get approval.                        │
│                                                                   │
│  2. UserBehaviorInferred                                          │
│     "You always enable dark mode after 8pm."                     │
│     AIRS observed a pattern and the user approved the inference. │
│     Overridable by explicit preference at any time.              │
│                                                                   │
│  3. AgentSuggested                                                │
│     "Power agent suggests reducing brightness to 40%."           │
│     Requires explicit user approval. Never applied silently.     │
│     Can be rejected permanently.                                  │
│                                                                   │
│  4. SystemDefault                                                 │
│     Factory defaults. Applied when nothing else has been set.    │
│     Overridden by any other source.                              │
│                                                                   │
│  LOWEST AUTHORITY                                                 │
└──────────────────────────────────────────────────────────────────┘
```

### 4.2 Precedence Resolution

```rust
impl PreferenceService {
    pub fn resolve_value(&self, id: &PreferenceId) -> ResolvedPreference {
        let pref = self.store.get(id);

        // Check for temporary overrides (e.g., "heads down for 2 hours")
        if let Some(temp_override) = self.temporary_overrides.get(id) {
            if temp_override.expires > SystemTime::now() {
                return ResolvedPreference {
                    value: temp_override.value.clone(),
                    source: PreferenceSource::UserExplicit {
                        method: ExplicitMethod::ConversationBar,
                        timestamp: temp_override.set_at,
                    },
                    temporary: true,
                    expires: Some(temp_override.expires),
                };
            }
        }

        // Return the current value with its source
        ResolvedPreference {
            value: pref.value.clone(),
            source: pref.source.clone(),
            temporary: false,
            expires: None,
        }
    }
}
```

### 4.3 Source Conflict Example

```
Scenario: Battery is at 15%

1. User previously set: display.brightness = 100% (UserExplicit)
2. Power agent suggests: display.brightness = 30% (AgentSuggested)

Resolution:
- UserExplicit wins — brightness stays at 100%
- BUT: Power agent's suggestion is presented to the user:
  "Battery at 15%. Power agent suggests reducing brightness to 30%.
   At 100%, battery will last ~20 minutes.
   At 30%, battery will last ~90 minutes.
   [Accept] [Keep 100%] [Set to 70%]"

The system respects the user's explicit choice but proactively
informs them of the tradeoff. The user decides.
```

-----

## 5. Conversational Configuration

### 5.1 NLU Resolution Pipeline

When the user speaks a preference change via the Conversation Bar:

```
User input: "Make the text bigger"
         │
         ▼
┌─────────────────────────────────────────┐
│  AIRS Natural Language Understanding     │
│                                          │
│  Intent: change_preference               │
│  Target: display.font_scale              │
│  Direction: increase                     │
│  Amount: unspecified (use default step)   │
└────────────────────┬────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────┐
│  Preference Service                      │
│                                          │
│  Current: display.font_scale = 1.0       │
│  Step: 0.1 (from metadata)               │
│  New value: 1.1                          │
│  Affected: Compositor (re-render all)    │
└────────────────────┬────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────┐
│  Change Propagator                       │
│                                          │
│  IPC → Compositor: font_scale = 1.1      │
│  Compositor re-renders all surfaces      │
│  User sees larger text immediately       │
└─────────────────────────────────────────┘
```

```rust
impl PreferenceService {
    pub async fn handle_natural_language(
        &mut self,
        input: &str,
    ) -> PreferenceChangeResult {
        // 1. AIRS interprets the natural language input
        let intent = self.airs.interpret_preference(input).await;

        match intent {
            PreferenceIntent::Change { target, direction, amount } => {
                let pref = self.store.get(&target);
                let new_value = self.compute_new_value(
                    &pref.value, direction, amount,
                );

                // 2. Apply the change
                self.set_preference(
                    &target,
                    new_value.clone(),
                    PreferenceSource::UserExplicit {
                        method: ExplicitMethod::ConversationBar,
                        timestamp: SystemTime::now(),
                    },
                    &format!("User said: \"{}\"", input),
                ).await;

                PreferenceChangeResult::Applied {
                    preference: target,
                    old_value: pref.value,
                    new_value,
                    description: format!("Text size increased to {:.0}%",
                        new_value.as_float().unwrap() * 100.0),
                }
            }
            PreferenceIntent::Query { target } => {
                let pref = self.store.get(&target);
                PreferenceChangeResult::Info {
                    preference: target,
                    value: pref.value.clone(),
                    source: pref.source.clone(),
                    description: pref.description.clone(),
                }
            }
            PreferenceIntent::Ambiguous { candidates } => {
                PreferenceChangeResult::Clarification {
                    question: "Which setting did you mean?".into(),
                    options: candidates,
                }
            }
            PreferenceIntent::Unknown => {
                PreferenceChangeResult::NotUnderstood {
                    input: input.to_string(),
                }
            }
        }
    }

    fn compute_new_value(
        &self,
        current: &PreferenceValue,
        direction: ChangeDirection,
        amount: Option<f64>,
    ) -> PreferenceValue {
        match (current, direction) {
            (PreferenceValue::Float(v), ChangeDirection::Increase) => {
                let step = amount.unwrap_or(0.1);
                PreferenceValue::Float(v + step)
            }
            (PreferenceValue::Float(v), ChangeDirection::Decrease) => {
                let step = amount.unwrap_or(0.1);
                PreferenceValue::Float((v - step).max(0.0))
            }
            (PreferenceValue::Bool(_), ChangeDirection::Enable) => {
                PreferenceValue::Bool(true)
            }
            (PreferenceValue::Bool(_), ChangeDirection::Disable) => {
                PreferenceValue::Bool(false)
            }
            (PreferenceValue::Range { value, min, max, step }, dir) => {
                let delta = amount.unwrap_or(*step);
                let new = match dir {
                    ChangeDirection::Increase => (value + delta).min(*max),
                    ChangeDirection::Decrease => (value - delta).max(*min),
                    _ => *value,
                };
                PreferenceValue::Range { value: new, min: *min, max: *max, step: *step }
            }
            _ => current.clone(),
        }
    }
}
```

### 5.2 Conversational Examples

|User says|AIRS interprets|Preference change|
|---------|--------------|----------------|
|"Make the text bigger"|display.font_scale, increase|1.0 → 1.1|
|"Dark mode"|display.theme, set to dark|light → dark|
|"I don't like the blue"|display.accent_color, change|#4A90D9 → (prompt for new color)|
|"Stop notifications at night"|attention.schedule.night_suppress, enable|false → true, 22:00-07:00|
|"Turn off the click sounds"|audio.ui_sounds, disable|true → false|
|"Make the mouse faster"|input.mouse_speed, increase|0.5 → 0.7|
|"I'm heads down for 2 hours"|context.override, focus mode|2h temporary override|
|"Why is my screen so dim?"|display.brightness, query|Returns: "Set to 30% by power agent because battery was at 15%"|

-----

## 6. Behavioral Inference

### 6.1 The Observation Loop

AIRS observes user behavior patterns and proposes preference changes:

```rust
pub struct BehavioralObserver {
    /// Observed patterns (time series of user actions)
    observations: Vec<Observation>,
    /// Hypothesized preferences
    hypotheses: Vec<PreferenceHypothesis>,
    /// AIRS inference client
    airs: AirsClient,
}

pub struct Observation {
    /// What the user did
    pub action: UserAction,
    /// When
    pub timestamp: SystemTime,
    /// Current context at the time
    pub context: ContextState,
}

pub struct PreferenceHypothesis {
    /// Which preference this hypothesis is about
    pub preference: PreferenceId,
    /// Proposed value
    pub proposed_value: PreferenceValue,
    /// Natural language description
    pub description: String,
    /// How many times the pattern was observed
    pub observation_count: usize,
    /// Minimum observations before proposing (default: 5)
    pub threshold: usize,
    /// Confidence (0.0 - 1.0)
    pub confidence: f32,
}
```

### 6.2 Inference Pipeline

```
Observe → Hypothesize → Confirm threshold → Propose → User approves/rejects → Learn

Example:
1. OBSERVE: User enables dark mode at 8pm (day 1)
2. OBSERVE: User enables dark mode at 8:30pm (day 2)
3. OBSERVE: User enables dark mode at 7:45pm (day 3)
4. OBSERVE: User enables dark mode at 8:15pm (day 4)
5. OBSERVE: User enables dark mode at 8pm (day 5)
6. HYPOTHESIZE: "User prefers dark mode after ~8pm"
7. CONFIRM: 5 observations, confidence 0.92
8. PROPOSE: "I've noticed you switch to dark mode around 8pm most evenings.
             Would you like me to do this automatically?
             [Yes, auto-switch at 8pm] [No, I'll do it manually] [Customize time]"
9. USER: [Yes, auto-switch at 8pm]
10. LEARN: Create preference display.auto_dark_mode = { enabled: true, time: "20:00" }
           Source: UserBehaviorInferred
```

```rust
impl BehavioralObserver {
    pub async fn check_hypotheses(&mut self) {
        for hypothesis in &mut self.hypotheses {
            if hypothesis.observation_count >= hypothesis.threshold
                && hypothesis.confidence >= 0.85
            {
                // Propose to user via Attention system
                self.propose_preference_change(hypothesis).await;
            }
        }
    }

    async fn propose_preference_change(&self, hypothesis: &PreferenceHypothesis) {
        attention::post(AttentionRequest {
            content: AttentionContent::AgentReport {
                agent: self_agent_id(),
                task: None,
                summary: hypothesis.description.clone(),
                results: None,
            },
            auto_action: Some(ProposedAction {
                description: format!("Enable: {}", hypothesis.description),
                action: ActionType::Custom {
                    agent: self_agent_id(),
                    action_id: "accept_preference".into(),
                    params: serde_json::json!({
                        "preference": hypothesis.preference,
                        "value": hypothesis.proposed_value,
                    }),
                },
                required_capabilities: vec![],
                reversible: true,
            }),
        }).await;
    }

    pub fn on_user_response(&mut self, hypothesis_id: &str, accepted: bool) {
        if accepted {
            // Apply the inferred preference
            self.preference_service.set_preference(
                &hypothesis.preference,
                hypothesis.proposed_value.clone(),
                PreferenceSource::UserBehaviorInferred {
                    observation: hypothesis.description.clone(),
                    confidence: hypothesis.confidence,
                    timestamp: SystemTime::now(),
                },
                &hypothesis.description,
            );
        } else {
            // User rejected — don't propose this again
            self.rejected_hypotheses.insert(hypothesis.preference.clone());
        }
    }
}
```

### 6.3 Observable Patterns

|Observed behavior|Inferred preference|
|----------------|-------------------|
|User enables dark mode every evening|Auto dark mode at sunset or fixed time|
|User always increases volume when music starts|Higher default media volume|
|User always maximizes terminal windows|Default terminal to fullscreen|
|User increases font size on every new device|Larger system font scale default|
|User mutes during calendar meetings|Calendar-aware auto-mute|
|User always dismisses weather notifications|Suppress weather agent attention|
|User types slowly and corrects often|Slower key repeat rate|

-----

## 7. Preference Propagation

### 7.1 Change Notification

When a preference changes, all affected components are notified via IPC:

```rust
impl PreferenceService {
    pub async fn set_preference(
        &mut self,
        id: &PreferenceId,
        value: PreferenceValue,
        source: PreferenceSource,
        reason: &str,
    ) {
        let pref = self.store.get_mut(id);
        let old_value = pref.value.clone();

        // 1. Validate the new value
        if let Some(constraints) = &pref.metadata.constraints {
            if !constraints.validate(&value) {
                return; // or return error
            }
        }

        // 2. Record the change
        pref.history.push(PreferenceChange {
            preference_id: id.clone(),
            old_value: old_value.clone(),
            new_value: value.clone(),
            source: source.clone(),
            reason: reason.to_string(),
            timestamp: SystemTime::now(),
            reverted: false,
        });

        // 3. Update the value
        pref.value = value.clone();
        pref.source = source;

        // 4. Persist to space
        space::write(
            &format!("user/preferences/{}", id.to_path()),
            pref,
        );

        // 5. Notify all affected components
        for component in &pref.affects {
            self.notify_component(component, id, &value).await;
        }
    }

    async fn notify_component(
        &self,
        component: &SystemComponent,
        pref_id: &PreferenceId,
        value: &PreferenceValue,
    ) {
        let channel = self.component_channels.get(component);
        if let Some(ch) = channel {
            ch.send(&PreferenceChangedMsg {
                preference: pref_id.clone(),
                value: value.clone(),
            }).await;
        }
    }
}
```

### 7.2 Component Subscription

System components subscribe to preferences they care about:

```rust
// In the Compositor
impl Compositor {
    pub fn setup_preference_subscriptions(&mut self) {
        self.preference_service.subscribe(&[
            "display.theme",
            "display.font_scale",
            "display.density",
            "display.accent_color",
            "display.animation_speed",
            "display.reduce_motion",
            "display.high_contrast",
        ], |pref_id, value| {
            match pref_id.as_str() {
                "display.font_scale" => {
                    self.set_global_font_scale(value.as_float().unwrap());
                    self.invalidate_all_surfaces();
                }
                "display.theme" => {
                    let theme_name = value.as_string().unwrap();
                    self.apply_theme(Theme::from_name(theme_name));
                }
                "display.reduce_motion" => {
                    self.set_animations_enabled(!value.as_bool().unwrap());
                }
                _ => {}
            }
        });
    }
}
```

-----

## 8. Agent Preferences

### 8.1 Agent-Scoped Preferences

Agents can define their own preferences. These are scoped to the agent and stored separately from system preferences:

```rust
pub struct AgentPreference {
    /// Preference ID (scoped: "agent.weather.temperature_unit")
    pub id: PreferenceId,
    /// Same data model as system preferences
    pub value: PreferenceValue,
    pub source: PreferenceSource,
    pub metadata: PreferenceMetadata,
    pub history: Vec<PreferenceChange>,
}
```

### 8.2 Agent Manifest Preference Declaration

```toml
# Agent manifest
[preferences]
temperature_unit = { type = "enum", options = ["celsius", "fahrenheit"], default = "celsius" }
update_interval = { type = "duration", default = "30m", min = "5m", max = "24h" }
show_humidity = { type = "bool", default = true }
```

### 8.3 Reading System Preferences

Agents can read system preferences relevant to their function, but never modify them directly:

```rust
// Agent code
use aios_sdk::preferences;

pub async fn setup(&mut self) {
    // Read system preference (read-only)
    let font_scale = preferences::system("display.font_scale").await;
    self.apply_font_scale(font_scale.as_float().unwrap());

    // Read agent-specific preference (read-write)
    let unit = preferences::agent("temperature_unit").await;
    self.temperature_unit = unit.as_string().unwrap().parse().unwrap();

    // Subscribe to system preference changes
    preferences::on_system_change("display.font_scale", |new_value| {
        self.apply_font_scale(new_value.as_float().unwrap());
    });
}

pub async fn suggest_system_change(&self) {
    // Agents can SUGGEST system preference changes
    // These go through the approval flow
    preferences::suggest_system_change(
        "attention.weather_agent.max_urgency",
        PreferenceValue::Enum {
            value: "digest".into(),
            options: vec!["interrupt".into(), "next_break".into(), "digest".into(), "silent".into()],
        },
        "Weather updates are informational, suggest digest-level attention",
    ).await;
}
```

-----

## 9. Preference History and Explainability

### 9.1 "Why Is My Screen Dim?"

Every preference has a full audit trail. Users can ask why any setting has its current value:

```rust
impl PreferenceService {
    pub fn explain(&self, id: &PreferenceId) -> PreferenceExplanation {
        let pref = self.store.get(id);

        PreferenceExplanation {
            current_value: pref.value.clone(),
            current_source: pref.source.clone(),
            reason: match &pref.source {
                PreferenceSource::UserExplicit { method, timestamp } => {
                    format!("You set this to {} via {} on {}",
                        pref.value, method, timestamp)
                }
                PreferenceSource::UserBehaviorInferred { observation, .. } => {
                    format!("Automatically set based on observed pattern: {}",
                        observation)
                }
                PreferenceSource::AgentSuggested { agent, reason, .. } => {
                    format!("Suggested by {} because: {}", agent, reason)
                }
                PreferenceSource::SystemDefault => {
                    "Factory default — you haven't changed this yet".into()
                }
            },
            history: pref.history.last_n(5),
            related_preferences: self.get_related(id),
        }
    }
}
```

### 9.2 Undo

Users can revert any preference change:

```rust
impl PreferenceService {
    pub fn undo_last_change(&mut self, id: &PreferenceId) -> Result<(), Error> {
        let pref = self.store.get_mut(id);

        if let Some(last_change) = pref.history.last() {
            let old_value = last_change.old_value.clone();

            // Mark the change as reverted
            pref.history.last_mut().unwrap().reverted = true;

            // Restore old value
            self.set_preference(
                id,
                old_value,
                PreferenceSource::UserExplicit {
                    method: ExplicitMethod::ConversationBar,
                    timestamp: SystemTime::now(),
                },
                "Reverted previous change",
            );

            Ok(())
        } else {
            Err(Error::NothingToUndo)
        }
    }
}
```

-----

## 10. Preference Conflicts

### 10.1 Conflict Detection

Conflicts arise when two sources disagree:

```rust
pub struct PreferenceConflict {
    pub preference: PreferenceId,
    pub current: ConflictSide,
    pub proposed: ConflictSide,
    pub tradeoff: String,
}

pub struct ConflictSide {
    pub value: PreferenceValue,
    pub source: PreferenceSource,
    pub rationale: String,
}
```

### 10.2 Resolution Strategy

```rust
impl PreferenceService {
    pub fn resolve_conflict(&self, conflict: &PreferenceConflict) -> ConflictResolution {
        // UserExplicit always wins over everything else
        if matches!(conflict.current.source, PreferenceSource::UserExplicit { .. }) {
            // Don't silently override. Instead, inform the user.
            return ConflictResolution::KeepCurrent {
                inform_user: true,
                message: conflict.tradeoff.clone(),
            };
        }

        // BehaviorInferred wins over AgentSuggested and SystemDefault
        if matches!(conflict.current.source, PreferenceSource::UserBehaviorInferred { .. })
            && matches!(conflict.proposed.source, PreferenceSource::AgentSuggested { .. })
        {
            return ConflictResolution::KeepCurrent {
                inform_user: false,
                message: String::new(),
            };
        }

        // AgentSuggested vs SystemDefault → propose to user
        ConflictResolution::AskUser {
            question: format!(
                "{} wants to change {} from {} to {}. {}",
                conflict.proposed.source,
                conflict.preference,
                conflict.current.value,
                conflict.proposed.value,
                conflict.tradeoff,
            ),
            options: vec![
                ConflictOption::Accept(conflict.proposed.value.clone()),
                ConflictOption::Keep(conflict.current.value.clone()),
                ConflictOption::Custom,
            ],
        }
    }
}
```

-----

## 11. Cross-Device Preferences

### 11.1 Sync Strategy

Preferences are stored in `user/preferences/` space and sync via Space Mesh like all spaces:

```rust
pub struct PreferenceSyncPolicy {
    /// Universal preferences sync everywhere
    pub universal: Vec<PreferenceId>,
    /// Per-device preferences stay local
    pub per_device: Vec<PreferenceId>,
}

impl PreferenceSyncPolicy {
    pub fn default() -> Self {
        PreferenceSyncPolicy {
            universal: vec![
                "display.theme".into(),
                "display.accent_color".into(),
                "display.font_scale".into(),
                "audio.ui_sounds".into(),
                "attention.*".into(),
                "privacy.*".into(),
                "agents.*".into(),
                "context.*".into(),
            ],
            per_device: vec![
                "display.brightness".into(),
                "display.resolution".into(),
                "display.refresh_rate".into(),
                "audio.output_device".into(),
                "audio.volume".into(),
                "input.mouse_speed".into(),
                "input.keyboard_layout".into(),
                "power.*".into(),
                "network.wifi.*".into(),
            ],
        }
    }
}
```

### 11.2 Conflict Resolution Across Devices

When the same universal preference is changed on two devices simultaneously:

```rust
pub enum SyncConflictResolution {
    /// Most recent change wins (default for most preferences)
    LastWriteWins,
    /// Prompt user to choose
    AskUser,
    /// Merge (for list-type preferences)
    Merge,
}
```

-----

## 12. Preference Categories and Defaults

### 12.1 Display

|Preference|Type|Default|Scope|
|---|---|---|---|
|`display.theme`|enum: light, dark, auto|auto|Universal|
|`display.accent_color`|color|#4A90D9|Universal|
|`display.font_scale`|range: 0.5-3.0|1.0|Universal|
|`display.density`|enum: compact, normal, relaxed|normal|Universal|
|`display.animation_speed`|range: 0.0-2.0|1.0|Universal|
|`display.reduce_motion`|bool|false|Universal|
|`display.high_contrast`|bool|false|Universal|
|`display.brightness`|range: 0-100|80|PerDevice|
|`display.night_shift`|bool + schedule|false|PerDevice|
|`display.resolution`|size|native|PerDevice|

### 12.2 Audio

|Preference|Type|Default|Scope|
|---|---|---|---|
|`audio.master_volume`|range: 0-100|50|PerDevice|
|`audio.ui_sounds`|bool|true|Universal|
|`audio.output_device`|enum (detected)|default|PerDevice|
|`audio.alert_sound`|enum|default|Universal|
|`audio.media_volume`|range: 0-100|70|PerDevice|

### 12.3 Input

|Preference|Type|Default|Scope|
|---|---|---|---|
|`input.keyboard_layout`|enum|us|PerDevice|
|`input.key_repeat_rate`|range: 1-50|30|Universal|
|`input.key_repeat_delay`|duration|250ms|Universal|
|`input.mouse_speed`|range: 0.1-3.0|1.0|PerDevice|
|`input.natural_scrolling`|bool|true|Universal|
|`input.tap_to_click`|bool|true|PerDevice|

### 12.4 Attention

|Preference|Type|Default|Scope|
|---|---|---|---|
|`attention.digest_interval`|duration|2h|Universal|
|`attention.night_suppress`|bool + schedule|false|Universal|
|`attention.interrupt_sound`|bool|true|Universal|
|`attention.toast_duration`|duration|5s|Universal|
|`attention.break_threshold`|duration|30s|Universal|

### 12.5 Privacy

|Preference|Type|Default|Scope|
|---|---|---|---|
|`privacy.crash_reports`|bool|false|Universal|
|`privacy.usage_analytics`|bool|false|Universal|
|`privacy.ai_data_local_only`|bool|true|Universal|
|`privacy.identity_disclosure`|enum: full, minimal, anonymous|minimal|Universal|

### 12.6 Power

|Preference|Type|Default|Scope|
|---|---|---|---|
|`power.sleep_timeout`|duration|15min|PerDevice|
|`power.performance_mode`|enum: balanced, performance, battery|balanced|PerDevice|
|`power.low_battery_threshold`|range: 5-30|20|Universal|
|`power.auto_brightness`|bool|true|PerDevice|

-----

## 13. The Settings UI

For users who prefer a visual interface, the Settings UI provides a structured view of all preferences:

```rust
pub struct SettingsUI {
    /// All preferences, grouped by category
    categories: Vec<PreferenceCategory>,
    /// Search index for finding preferences
    search_index: SearchIndex,
    /// Currently selected category
    selected_category: Option<PreferenceCategory>,
}

impl Application for SettingsUI {
    fn view(&self) -> Element<Message> {
        let sidebar = column(
            self.categories.iter().map(|cat| {
                button(cat.display_name())
                    .on_press(Message::SelectCategory(cat.clone()))
                    .style(if Some(cat) == self.selected_category.as_ref() {
                        Style::Active
                    } else {
                        Style::Default
                    })
            }).collect()
        );

        let content = if let Some(cat) = &self.selected_category {
            self.render_category(cat)
        } else {
            self.render_search()
        };

        row![
            sidebar.width(Length::FillPortion(1)),
            vertical_rule(1),
            container(content).width(Length::FillPortion(3)).padding(16),
        ].into()
    }

    fn render_category(&self, category: &PreferenceCategory) -> Element<Message> {
        let prefs = self.preferences_for_category(category);

        column(
            prefs.iter().map(|pref| {
                row![
                    column![
                        text(&pref.description).size(15),
                        text(&self.explain_source(&pref.source))
                            .size(12)
                            .color(Color::from_rgb(0.5, 0.5, 0.5)),
                    ],
                    self.render_control(pref),
                ]
                .spacing(8)
                .align_items(Alignment::Center)
            }).collect()
        )
        .spacing(16)
        .into()
    }
}
```

The Settings UI is **not hardcoded**. It reads preference metadata from the Preference Service. When a new subsystem registers new preferences, they appear in Settings automatically. No Settings UI code change needed.

-----

## 14. Implementation Order

Development plan phases (see development-plan.md — not to be confused with boot phases):

```
Dev Phase 5a:   Preference data model              → PreferenceId, PreferenceValue, storage
Dev Phase 5b:   Preference Service (basic)         → get/set/persist, system defaults
Dev Phase 5c:   Change propagation                 → IPC notification to components

Dev Phase 9a:   NLU resolver                       → Conversation Bar → preference changes
Dev Phase 9b:   Preference history                 → change records, explain, undo
Dev Phase 9c:   Conflict resolution                → source precedence, tradeoff dialogs

Dev Phase 13a:  Behavioral observer                → pattern detection, hypothesis generation
Dev Phase 13b:  Behavioral proposals               → AIRS-driven preference suggestions
Dev Phase 13c:  Agent preferences                  → manifest declaration, scoped storage

Dev Phase 16a:  Settings UI                        → visual preference browser
Dev Phase 16b:  Cross-device sync                  → universal vs per-device, conflict resolution
Dev Phase 16c:  Preference analytics               → usage patterns, recommendation engine

Dev Phase 20:   Full NLU coverage                  → handle ambiguous, complex, multi-preference changes
Dev Phase 23:   Accessibility preferences          → screen reader, high contrast, reduced motion
```

-----

## 15. Design Principles

1. **Conversation first, panels last.** The primary interface for preferences is natural language. "Make the text bigger" is always easier than Settings → Display → Font Size → Scale Factor. The Settings UI is a fallback for browsing, not the default.

2. **Every change has a reason.** No silent changes. Every preference change records who changed it, when, why, and what the previous value was. "Why is my screen dim?" always has an answer.

3. **User explicit wins.** When the user directly sets a preference, nothing overrides it silently. System components can explain tradeoffs and suggest alternatives, but the user's explicit choice is respected.

4. **Observe, hypothesize, propose, confirm.** Behavioral inference follows a strict pipeline. The system observes patterns, forms hypotheses, waits for statistical confidence, proposes changes with explanation, and only applies them after user approval. No silent behavior modification.

5. **Preferences are data, not code.** Preferences are typed values stored in spaces, not hardcoded constants scattered across configuration files. They're queryable, syncable, versionable, and auditable.

6. **Agents suggest, users decide.** Agents can propose preference changes with rationale. Users see the proposal, the explanation, and the tradeoffs. The agent never makes the decision.

7. **Sync what makes sense.** Theme syncs everywhere. Brightness doesn't. The system knows which preferences are universal and which are hardware-specific.

8. **No settings archaeology.** If a user can't find a setting within 10 seconds, the system has failed. Natural language search, category browsing, and "why is X set to Y?" queries ensure every preference is discoverable.
