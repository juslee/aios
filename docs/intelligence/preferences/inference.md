# AIOS Preference Inference and Propagation

Part of: [preferences.md](../preferences.md) — Preference System
**Related:** [data-model.md](./data-model.md) — PreferenceSource and PreferenceChange types, [resolution.md](./resolution.md) — Source precedence, [security.md](./security.md) — Capability-gated agent access and rate limiting, [intelligence.md](./intelligence.md) — ML models powering behavioral inference

-----

## 6. Behavioral Inference

### 6.1 The Observation Loop

AIRS observes user behavior patterns and proposes preference changes. The statistical models powering behavioral inference are described in §17 (Kernel-Internal ML). AIRS-enhanced inference that uses semantic understanding is described in §16 (AI-Native Intelligence).

```rust
pub struct BehavioralObserver {
    /// Observed patterns (time series of user actions)
    observations: Vec<Observation>,
    /// Hypothesized preferences
    hypotheses: Vec<PreferenceHypothesis>,
    /// AIRS inference client
    airs: AirsClient,
    /// Rate limiter for suggestions (§15.6)
    rate_limiter: SuggestionRateLimiter,
    /// Preference Service for applying accepted hypotheses
    preference_service: PreferenceServiceHandle,
    /// Hypotheses rejected by the user (suppressed until rejection cooldown expires)
    rejected_hypotheses: HashSet<PreferenceId>,
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
    /// Confidence (0.0 - 1.0), computed by §17.2 models
    pub confidence: f32,
}
```

### 6.2 Inference Pipeline

```text
Observe → Hypothesize → Confirm threshold → Propose → User approves/rejects → Learn

Example:
1. OBSERVE: User enables dark mode at 8pm (day 1)
2. OBSERVE: User enables dark mode at 8:30pm (day 2)
3. OBSERVE: User enables dark mode at 7:45pm (day 3)
4. OBSERVE: User enables dark mode at 8:15pm (day 4)
5. OBSERVE: User enables dark mode at 8pm (day 5)
6. HYPOTHESIZE: "User prefers dark mode after ~8pm"
7. CONFIRM: 5 observations, confidence 0.92 (§17.2 Beta distribution model)
8. PROPOSE: "I've noticed you switch to dark mode around 8pm most evenings.
             Would you like me to do this automatically?
             [Yes, auto-switch at 8pm] [No, I'll do it manually] [Customize time]"
9. USER: [Yes, auto-switch at 8pm]
10. LEARN: Create context rule (§14): TimeOfDay { start: 20:00, end: 07:00 }
           → display.theme = dark
           Source: ContextRuleSource::AirsSuggested { confidence: 0.92 }
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
        // Rate limiting check (§15.6) — prevents agent spam
        if !self.rate_limiter.allow_suggestion(&hypothesis.preference) {
            return;
        }

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
        let hypothesis = self.hypotheses.iter()
            .find(|h| h.preference.as_str() == hypothesis_id)
            .expect("hypothesis not found");

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
            // User rejected — don't propose this again (§15.6 rejection cooldown)
            self.rejected_hypotheses.insert(hypothesis.preference.clone());
        }
    }
}
```

### 6.3 Observable Patterns

| Observed behavior | Inferred preference |
|---|---|
| User enables dark mode every evening | Auto dark mode at sunset or fixed time (→ context rule §14) |
| User always increases volume when music starts | Higher default media volume |
| User always maximizes terminal windows | Default terminal to fullscreen |
| User increases font size on every new device | Larger system font scale default |
| User mutes during calendar meetings | Calendar-aware auto-mute (→ activity rule §14) |
| User always dismisses weather notifications | Suppress weather agent attention |
| User types slowly and corrects often | Slower key repeat rate |
| User reduces brightness after 8pm | Auto-brightness reduction (→ time-of-day rule §14) |

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

        // 1. Validate the new value against schema
        if let Some(schema) = self.schema_registry.get(id) {
            if !schema.value_type.validate(&value) {
                return; // or return error
            }
        }

        // 2. Check capability (§15)
        // Caller must hold PreferenceSystemWrite or be the Preference Service itself

        // 3. Record the change
        pref.history.push(PreferenceChange {
            preference_id: id.clone(),
            old_value: old_value.clone(),
            new_value: value.clone(),
            source: source.clone(),
            reason: reason.to_string(),
            timestamp: SystemTime::now(),
            reverted: false,
        });

        // 4. Update the value
        pref.value = value.clone();
        pref.source = source.clone();

        // 5. Persist to space
        space::write(
            &format!("user/preferences/{}", id.to_path()),
            pref,
        );

        // 6. Audit log (§15.4)
        self.audit.log(PreferenceAuditEvent::Changed {
            preference: id.clone(),
            old_value: old_value.clone(),
            new_value: value.clone(),
            source,
            agent: self.current_caller(),
        });

        // 7. Notify all affected components
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

Agents define their own preferences in their manifest. These are scoped to the agent's namespace and stored separately from system preferences. Agents hold `PreferenceAgentWrite` tokens (§15.1) that are inherently attenuated to their own namespace.

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

When an agent is installed, the Preference Service creates schemas for each declared agent preference and initializes them with default values. These appear in the Settings UI under the agent's section.

### 8.3 Reading System Preferences

Agents can read system preferences relevant to their function using attenuated `PreferenceRead` capability tokens (§15.2). They can never modify system preferences directly — only suggest changes through the approval flow:

```rust
// Agent code
use aios_sdk::preferences;

pub async fn setup(&mut self) {
    // Read system preference (read-only, requires PreferenceRead capability)
    let font_scale = preferences::system("display.font_scale").await;
    self.apply_font_scale(font_scale.as_float().unwrap());

    // Read agent-specific preference (read-write via PreferenceAgentWrite)
    let unit = preferences::agent("temperature_unit").await;
    self.temperature_unit = unit.as_string().unwrap().parse().unwrap();

    // Subscribe to system preference changes
    preferences::on_system_change("display.font_scale", |new_value| {
        self.apply_font_scale(new_value.as_float().unwrap());
    });
}

pub async fn suggest_system_change(&self) {
    // Agents can SUGGEST system preference changes (requires PreferenceSuggest)
    // These go through the approval flow and are rate-limited (§15.6)
    preferences::suggest_system_change(
        "attention.weather_agent.max_urgency",
        PreferenceValue::Enum {
            value: "digest".into(),
            options: vec![
                "interrupt".into(), "next_break".into(),
                "digest".into(), "silent".into(),
            ],
        },
        "Weather updates are informational, suggest digest-level attention",
    ).await;
}
```
