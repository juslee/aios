# AIOS Preference History, Sync, and Settings UI

Part of: [preferences.md](../preferences.md) — Preference System
**Related:** [data-model.md](./data-model.md) — PreferenceChange records, [resolution.md](./resolution.md) — Conflict resolution for sync, [security.md](./security.md) — Audit trail and privacy protection

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
                PreferenceSource::EnterpriseLocked { organization, .. } => {
                    format!("Locked by your organization ({}). Contact IT for changes.",
                        organization)
                }
                PreferenceSource::UserExplicit { method, timestamp } => {
                    format!("You set this to {} via {} on {}",
                        pref.value, method, timestamp)
                }
                PreferenceSource::EnterpriseRecommended { organization, .. } => {
                    format!("Recommended by your organization ({}). You can override this.",
                        organization)
                }
                PreferenceSource::ContextDriven { rule, trigger, .. } => {
                    format!("Automatically set by context rule: {}", trigger)
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
            // Show active context rules that would affect this preference
            active_context_rules: self.context_engine.rules_for(id),
            // Show suppressed context rules (overridden by higher-authority source)
            suppressed_rules: self.context_engine.suppressed_rules_for(id),
        }
    }
}
```

### 9.2 Undo

Users can revert any preference change:

```rust
impl PreferenceService {
    pub fn undo_last_change(&mut self, id: &PreferenceId) -> Result<(), PreferenceError> {
        let pref = self.store.get_mut(id);

        // Cannot undo enterprise-locked preferences
        if matches!(pref.source, PreferenceSource::EnterpriseLocked { .. }) {
            return Err(PreferenceError::EnterpriseLocked);
        }

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
            Err(PreferenceError::NothingToUndo)
        }
    }
}
```

-----

## 11. Cross-Device Preferences

### 11.1 Sync Strategy

Preferences are stored in `user/preferences/` space and sync via Space Mesh (see [spaces/sync.md](../../storage/spaces/sync.md) §8) like all spaces. The sync policy determines which preferences travel between devices:

```rust
pub struct PreferenceSyncPolicy {
    /// Universal preferences sync everywhere
    pub universal: Vec<PreferenceId>,
    /// Per-device preferences stay local
    pub per_device: Vec<PreferenceId>,
    /// Per-context preferences sync rules but not active state
    pub per_context: Vec<PreferenceId>,
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
                "audio.master_volume".into(),
                "input.mouse_speed".into(),
                "input.keyboard_layout".into(),
                "power.*".into(),
                "network.wifi.*".into(),
            ],
            per_context: vec![
                // Context rules sync across devices but activation state is local
                // e.g., "at work" rule definition syncs, but whether it's currently
                // active depends on each device's actual context
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

Cross-device sync integrates with the multi-device experience layer (see [multi-device/experience.md](../../platform/multi-device/experience.md) §4) for intelligence continuity — when a user moves between devices, their preference context follows them.

**Context rule sync:** Context rules (§14) sync their *definitions* across devices, but activation state is local. A "work hours" rule defined on a laptop also appears on a phone, but the phone's context engine independently evaluates whether the rule's conditions are met based on its own sensors.

-----

## 12. Preference Categories and Defaults

### 12.1 Display

| Preference | Type | Default | Scope |
|---|---|---|---|
| `display.theme` | enum: light, dark, auto | auto | Universal |
| `display.accent_color` | color | #4A90D9 | Universal |
| `display.font_scale` | range: 0.5-3.0 | 1.0 | Universal |
| `display.density` | enum: compact, normal, relaxed | normal | Universal |
| `display.animation_speed` | range: 0.0-2.0 | 1.0 | Universal |
| `display.reduce_motion` | bool | false | Universal |
| `display.high_contrast` | bool | false | Universal |
| `display.brightness` | range: 0-100 | 80 | PerDevice |
| `display.night_shift` | bool + schedule | false | PerDevice |
| `display.resolution` | size | native | PerDevice |

### 12.2 Audio

| Preference | Type | Default | Scope |
|---|---|---|---|
| `audio.master_volume` | range: 0-100 | 50 | PerDevice |
| `audio.ui_sounds` | bool | true | Universal |
| `audio.output_device` | enum (detected) | default | PerDevice |
| `audio.alert_sound` | enum | default | Universal |
| `audio.media_volume` | range: 0-100 | 70 | PerDevice |

### 12.3 Input

| Preference | Type | Default | Scope |
|---|---|---|---|
| `input.keyboard_layout` | enum | us | PerDevice |
| `input.key_repeat_rate` | range: 1-50 | 30 | Universal |
| `input.key_repeat_delay` | duration | 250ms | Universal |
| `input.mouse_speed` | range: 0.1-3.0 | 1.0 | PerDevice |
| `input.natural_scrolling` | bool | true | Universal |
| `input.tap_to_click` | bool | true | PerDevice |

### 12.4 Attention

| Preference | Type | Default | Scope |
|---|---|---|---|
| `attention.digest_interval` | duration | 2h | Universal |
| `attention.night_suppress` | bool + schedule | false | Universal |
| `attention.interrupt_sound` | bool | true | Universal |
| `attention.toast_duration` | duration | 5s | Universal |
| `attention.break_threshold` | duration | 30s | Universal |

### 12.5 Privacy

| Preference | Type | Default | Scope |
|---|---|---|---|
| `privacy.crash_reports` | bool | false | Universal |
| `privacy.usage_analytics` | bool | false | Universal |
| `privacy.ai_data_local_only` | bool | true | Universal |
| `privacy.identity_disclosure` | enum: full, minimal, anonymous | minimal | Universal |

### 12.6 Power

| Preference | Type | Default | Scope |
|---|---|---|---|
| `power.sleep_timeout` | duration | 15min | PerDevice |
| `power.performance_mode` | enum: balanced, performance, battery | balanced | PerDevice |
| `power.low_battery_threshold` | range: 5-30 | 20 | Universal |
| `power.auto_brightness` | bool | true | PerDevice |

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
                let source_badge = self.render_source_badge(&pref.source);
                let locked = matches!(pref.source, PreferenceSource::EnterpriseLocked { .. });

                row![
                    column![
                        text(&pref.description).size(15),
                        text(&self.explain_source(&pref.source))
                            .size(12)
                            .color(Color::from_rgb(0.5, 0.5, 0.5)),
                    ],
                    source_badge,
                    if locked {
                        icon(Icon::Lock).tooltip("Locked by your organization")
                    } else {
                        self.render_control(pref)
                    },
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

The Settings UI is **not hardcoded**. It reads preference schemas (§3.5) from the Preference Service. When a new subsystem registers new preferences, they appear in Settings automatically. No Settings UI code change needed.

**Enterprise indicators:** When an organization manages the device, the Settings UI shows:

- **Lock icon** for `EnterpriseLocked` preferences with the organization's rationale
- **Badge** for `EnterpriseRecommended` preferences showing "Recommended by [org]"
- **Organization section** listing all enterprise-managed preferences in one view

**Context rule indicators:** Active context rules (§14) show in the Settings UI:

- **Context badge** next to preferences currently overridden by a context rule
- **"Why?"** link showing which rule is active and what context triggered it
- **"Override"** button to set an explicit value that supersedes the context rule
