# AIOS Preference Data Model

Part of: [preferences.md](../preferences.md) — Preference System
**Related:** [resolution.md](./resolution.md) — Source precedence and conflict resolution, [security.md](./security.md) — Capability-gated access and audit, [temporal.md](./temporal.md) — Context-driven preference rules

-----

## 3. The Preference

### 3.1 Core Data Model

Every preference in AIOS is a typed, auditable, propagating value with metadata sufficient for UI generation, conflict resolution, and cross-device sync.

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

    /// Metadata for UI generation and validation
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

/// Authority ranking for who/what set a preference value.
/// Higher authority sources override lower ones. See §4 for precedence rules.
pub enum PreferenceSource {
    /// Enterprise policy that the user CANNOT override.
    /// Highest authority — set by MDM or organizational policy engine.
    /// Follows the ChromeOS mandatory policy model.
    EnterpriseLocked {
        policy_id: PolicyId,
        organization: OrganizationId,
        timestamp: SystemTime,
    },

    /// User directly stated this preference.
    /// Second-highest authority — never overridden silently by anything
    /// except enterprise-locked policy.
    UserExplicit {
        method: ExplicitMethod,
        timestamp: SystemTime,
    },

    /// Enterprise policy recommendation that the user CAN override.
    /// Sets the default but respects user choice.
    /// Follows the ChromeOS recommended policy model.
    EnterpriseRecommended {
        policy_id: PolicyId,
        organization: OrganizationId,
        timestamp: SystemTime,
    },

    /// Context-driven rule activated by environmental conditions.
    /// Applied automatically when context matches; user can override at any time.
    /// See §14 for the context rule engine.
    ContextDriven {
        rule: ContextRuleId,
        trigger: String,
        timestamp: SystemTime,
    },

    /// AIRS inferred this from user behavior.
    /// Medium authority — can be overridden, user was informed.
    UserBehaviorInferred {
        observation: String,
        confidence: f32,
        timestamp: SystemTime,
    },

    /// An agent suggested this change.
    /// Low authority — requires user approval.
    AgentSuggested {
        agent: AgentId,
        reason: String,
        approved: bool,
        timestamp: SystemTime,
    },

    /// Factory default.
    /// Lowest authority — overridden by everything.
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
```

### 3.2 Preference Metadata

Metadata enables dynamic UI generation, validation, and cross-device sync decisions without hardcoding preference knowledge into any component.

```rust
pub struct PreferenceMetadata {
    /// Category for UI grouping
    pub category: PreferenceCategory,
    /// Whether this is device-specific, universal, or context-dependent
    pub scope: PreferenceScope,
    /// Minimum system version that supports this preference
    pub since_version: Version,
    /// Whether changing this requires restart
    pub requires_restart: bool,
    /// Related preferences (e.g., font_scale relates to display.density)
    pub related: Vec<PreferenceId>,
    /// Validation constraints
    pub constraints: Option<ValueConstraints>,
    /// Security classification for capability-gated access (see §15)
    pub security_classification: SecurityClassification,
    /// Schema definition for typed validation (see §3.5)
    pub schema: PreferenceSchema,
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
    /// Value varies by active context rule (e.g., "at work" vs "at home")
    PerContext,
}

/// Security classification determines which capability tokens are required
/// to read or modify this preference. See §15 for the full capability model.
pub enum SecurityClassification {
    /// Any agent with basic PreferenceRead can access
    Public,
    /// Requires category-specific capability (e.g., PreferenceRead attenuated to Privacy)
    CategoryRestricted { category: PreferenceCategory },
    /// Requires elevated trust level (system services only)
    SystemOnly,
    /// Enterprise-managed; requires EnterprisePolicy capability
    EnterpriseLocked,
}

/// System components that a preference can affect.
/// Referenced by architecture.md §2.8.
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

/// Named collection of preferences associated with an identity profile.
/// Referenced by architecture.md §6.3 and agents.md.
pub struct PreferenceSet {
    pub preferences: Vec<Preference>,
}
```

### 3.3 Preference Change Record

Every change is recorded with full provenance. This enables the explainability features described in §9 and the audit trail described in §15.

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

### 3.4 Context Rule Model

Context rules enable preferences that adapt to environmental conditions. The full context rule evaluation engine is described in §14.

```rust
/// A rule that activates a preference override when context conditions are met.
pub struct ContextRule {
    /// Unique identifier
    pub id: ContextRuleId,
    /// Human-readable name ("At work", "Evening mode", "Low battery")
    pub name: String,
    /// Conditions that must ALL be true for the rule to activate
    pub conditions: Vec<ContextCondition>,
    /// Preference overrides to apply when active
    pub overrides: Vec<ContextOverride>,
    /// Priority when multiple rules conflict (higher wins)
    pub priority: u32,
    /// Whether the user approved this rule
    pub approved: bool,
    /// Source: user-created, AIRS-suggested, or enterprise-defined
    pub source: ContextRuleSource,
}

pub enum ContextCondition {
    /// Time-of-day range (e.g., 20:00-07:00)
    TimeOfDay { start: NaiveTime, end: NaiveTime },
    /// Day of week (e.g., weekdays only)
    DayOfWeek { days: Vec<Weekday> },
    /// Sunrise/sunset relative (e.g., "after sunset")
    SolarEvent { event: SolarEvent, offset: Duration },
    /// Geographic location (e.g., within 500m of office)
    Location { center: GeoPoint, radius_meters: f64, name: String },
    /// Connected device presence (e.g., external monitor connected)
    DevicePresent { device_class: DeviceClass },
    /// Activity detection (e.g., in a meeting, exercising)
    Activity { activity: ActivityType },
    /// Power state (e.g., battery below 20%)
    PowerState { condition: PowerCondition },
    /// Network state (e.g., on metered connection)
    NetworkState { condition: NetworkCondition },
}

pub struct ContextOverride {
    /// Which preference to override
    pub preference_id: PreferenceId,
    /// Value to set when rule is active
    pub value: PreferenceValue,
}

pub enum ContextRuleSource {
    /// User explicitly created this rule
    UserCreated,
    /// AIRS observed patterns and suggested this rule (user approved)
    AirsSuggested { confidence: f32 },
    /// Enterprise policy defined this rule
    EnterprisePolicy { policy_id: PolicyId },
}
```

### 3.5 Preference Schema Registry

Every preference is declared via a typed schema before it can be used. The schema registry ensures type safety, enables dynamic UI generation, and provides self-documenting configuration. This follows the GNOME dconf/gsettings pattern of compiled, validated schemas.

```rust
pub struct PreferenceSchema {
    /// Preference identifier this schema defines
    pub id: PreferenceId,
    /// Human-readable summary (for Settings UI tooltips)
    pub summary: String,
    /// Detailed description (for Settings UI help panels)
    pub description: String,
    /// Type and constraints
    pub value_type: SchemaValueType,
    /// Default value
    pub default: PreferenceValue,
    /// Components affected when this preference changes
    pub affects: Vec<SystemComponent>,
    /// Category for UI grouping
    pub category: PreferenceCategory,
    /// Sync scope
    pub scope: PreferenceScope,
    /// Security classification
    pub security: SecurityClassification,
}

pub enum SchemaValueType {
    Bool,
    Integer { min: Option<i64>, max: Option<i64> },
    Float { min: Option<f64>, max: Option<f64>, step: Option<f64> },
    String { max_length: Option<usize>, pattern: Option<String> },
    Enum { options: Vec<SchemaEnumOption> },
    Color,
    Duration { min: Option<Duration>, max: Option<Duration> },
    Range { min: f64, max: f64, step: f64 },
}

pub struct SchemaEnumOption {
    pub value: String,
    pub label: String,
    pub description: String,
}
```

**Schema registration** happens at subsystem initialization. When a subsystem registers with the Preference Service, it provides its schemas:

```rust
impl PreferenceService {
    /// Register a batch of preference schemas from a subsystem.
    /// Panics if any schema ID conflicts with an existing registration.
    pub fn register_schemas(&mut self, schemas: &[PreferenceSchema]) {
        for schema in schemas {
            assert!(
                !self.schema_registry.contains_key(&schema.id),
                "Duplicate preference schema: {}",
                schema.id
            );
            self.schema_registry.insert(schema.id.clone(), schema.clone());

            // Initialize the preference with its default value if not already set
            if !self.store.contains(&schema.id) {
                self.store.set(
                    &schema.id,
                    Preference {
                        id: schema.id.clone(),
                        description: schema.summary.clone(),
                        value: schema.default.clone(),
                        source: PreferenceSource::SystemDefault,
                        affects: schema.affects.clone(),
                        history: Vec::new(),
                        metadata: PreferenceMetadata::from_schema(schema),
                    },
                );
            }
        }
    }
}
```

The Settings UI (§13) reads schemas to generate controls dynamically — no hardcoded preference knowledge is required. When a new subsystem registers preferences, they appear in Settings automatically.
