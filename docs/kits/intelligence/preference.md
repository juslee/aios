# Preference Kit

**Layer:** Intelligence | **Crate:** `aios_preference` | **Architecture:** [`docs/intelligence/preferences.md`](../../intelligence/preferences.md) + 8 sub-docs

## 1. Overview

The Preference Kit learns and resolves user preferences through a 7-tier source precedence
model -- from enterprise policy (highest authority) down through explicit user choice,
context-driven rules, behavioral inference, agent suggestions, and system defaults (lowest).
Every preference is a typed, auditable value with metadata sufficient for UI generation,
conflict resolution, and cross-device sync.

The Kit provides three distinct interfaces. `PreferenceStore` is the typed key-value store
where preferences live. `PreferenceResolver` evaluates the full precedence stack, accounting
for active context overrides and enterprise policy, to return the single effective value for a
preference key at any point in time. `BehavioralObserver` watches user actions over time and
emits inferred preference updates with confidence scores -- these are never applied silently;
the user must approve behavioral inferences before they take effect. A natural language
settings interface backed by the [AIRS Kit](airs.md) NLU pipeline allows users to say "make
the font bigger" through the Conversation Bar, which the Kit maps to
`display.font_scale = 1.2`.

Use the Preference Kit when your agent needs to read user preferences, expose its own
configurable settings, or register context-driven temporal rules. Do not use it for transient
runtime state (use in-memory data structures) or for persistent data storage (use the
[Storage Kit](../platform/storage.md)).

## 2. Core Traits

```rust
use aios_preference::{
    PreferenceStore, PreferenceResolver, PreferenceValue,
    PreferenceSource, PreferenceId, BehavioralObserver,
};
use aios_capability::CapabilityHandle;

/// Typed key-value store for preference values.
///
/// Preferences use hierarchical dot-separated keys (e.g.,
/// "display.font_scale", "audio.output_device"). Each preference has a
/// schema defining its type, valid range, default, and human-readable
/// description. Agent-specific preferences are namespaced under the
/// agent's ID (e.g., "com.example.notes.auto_save").
pub trait PreferenceStore {
    /// Read a preference value. Returns the resolved effective value
    /// after evaluating the full 7-tier precedence stack.
    fn get(
        &self,
        key: &PreferenceId,
        cap: &CapabilityHandle,
    ) -> Result<PreferenceValue, PreferenceError>;

    /// Set a preference value explicitly (UserExplicit source, tier 2).
    ///
    /// This is the highest non-enterprise authority. The value persists
    /// until the user explicitly changes it or enterprise policy overrides.
    fn set(
        &self,
        key: &PreferenceId,
        value: PreferenceValue,
        cap: &CapabilityHandle,
    ) -> Result<(), PreferenceError>;

    /// Register a new preference schema for the calling agent's namespace.
    ///
    /// Schemas define the preference type, valid values, default, and
    /// localized description. Once registered, the preference appears in
    /// the Settings UI under the agent's section.
    fn register_schema(
        &self,
        schema: PreferenceSchema,
        cap: &CapabilityHandle,
    ) -> Result<(), PreferenceError>;

    /// List all preference keys in a namespace (e.g., "display.*").
    fn list_keys(
        &self,
        namespace: &str,
        cap: &CapabilityHandle,
    ) -> Result<Vec<PreferenceId>, PreferenceError>;

    /// Subscribe to changes on a specific preference key.
    fn on_change(
        &self,
        key: &PreferenceId,
        callback: Box<dyn Fn(&PreferenceValue, &PreferenceValue) + Send>,
        cap: &CapabilityHandle,
    ) -> Result<PreferenceSubscription, PreferenceError>;
}

/// Evaluate the 7-tier precedence stack for a preference key.
///
/// The resolver checks, in order: EnterpriseLocked > UserExplicit >
/// EnterpriseRecommended > ContextDriven > UserBehaviorInferred >
/// AgentSuggested > SystemDefault. The first tier with a value wins.
/// Context-driven overrides are evaluated against the current context
/// state from the Context Kit.
pub trait PreferenceResolver {
    /// Resolve the effective value, returning both the value and the
    /// source tier that produced it.
    fn resolve(
        &self,
        key: &PreferenceId,
        cap: &CapabilityHandle,
    ) -> Result<ResolvedPreference, PreferenceError>;

    /// Explain why a preference has its current value -- returns the
    /// full precedence chain showing which tiers were evaluated and why
    /// each was accepted or overridden.
    fn explain(
        &self,
        key: &PreferenceId,
        cap: &CapabilityHandle,
    ) -> Result<PrecedenceExplanation, PreferenceError>;

    /// Check if a preference is locked by enterprise policy (tier 1).
    fn is_locked(
        &self,
        key: &PreferenceId,
    ) -> bool;
}

/// A resolved preference value with source metadata.
pub struct ResolvedPreference {
    /// The effective value after precedence resolution.
    pub value: PreferenceValue,
    /// Which tier produced this value.
    pub source: PreferenceSource,
    /// Whether this value is temporary (context-driven override active).
    pub temporary: bool,
    /// When the value will revert (if temporary).
    pub expires: Option<Timestamp>,
}

/// Typed preference values.
pub enum PreferenceValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Enum { value: String, options: Vec<String> },
    Color(Color),
    Duration(core::time::Duration),
    Range { value: f64, min: f64, max: f64, step: f64 },
}

/// Source tiers ordered from highest to lowest authority.
pub enum PreferenceSource {
    /// Enterprise policy that the user cannot override.
    EnterpriseLocked { policy_id: PolicyId },
    /// User directly stated this preference.
    UserExplicit,
    /// Enterprise recommendation that the user can override.
    EnterpriseRecommended { policy_id: PolicyId },
    /// Context-driven temporal rule activated automatically.
    ContextDriven { rule_id: RuleId, trigger: String },
    /// AIRS observed a behavioral pattern, user approved.
    UserBehaviorInferred { confidence: f32 },
    /// An agent proposed this value; requires user approval.
    AgentSuggested { agent: AgentId, rationale: String },
    /// Factory default.
    SystemDefault,
}

/// Watch user behavior and emit inferred preference updates.
///
/// The BehavioralObserver monitors user actions (not keystrokes -- only
/// aggregate patterns like "user consistently enables dark mode after
/// 8pm") and emits inferred preference values. Inferences are never
/// applied silently. They are presented to the user for approval first.
pub trait BehavioralObserver {
    /// Query pending behavioral inferences (awaiting user approval).
    fn pending_inferences(
        &self,
        cap: &CapabilityHandle,
    ) -> Result<Vec<BehavioralInference>, PreferenceError>;

    /// Approve a behavioral inference, applying it as tier 5.
    fn approve(
        &self,
        inference_id: InferenceId,
        cap: &CapabilityHandle,
    ) -> Result<(), PreferenceError>;

    /// Reject a behavioral inference permanently (never suggest again).
    fn reject(
        &self,
        inference_id: InferenceId,
        cap: &CapabilityHandle,
    ) -> Result<(), PreferenceError>;
}

/// A behavioral inference pending user approval.
pub struct BehavioralInference {
    pub id: InferenceId,
    pub key: PreferenceId,
    pub proposed_value: PreferenceValue,
    pub confidence: f32,
    pub evidence: String,
    pub observed_since: Timestamp,
}

/// Schema definition for registering agent-specific preferences.
pub struct PreferenceSchema {
    pub key: PreferenceId,
    pub description: String,
    pub value_type: PreferenceValueType,
    pub default: PreferenceValue,
    pub validation: Option<ValidationRule>,
}
```

## 3. Usage Patterns

### Reading a system preference

```rust
use aios_preference::PreferenceStore;

fn get_font_scale(prefs: &dyn PreferenceStore, cap: &CapabilityHandle) -> f64 {
    match prefs.get(&"display.font_scale".into(), cap) {
        Ok(PreferenceValue::Float(scale)) => scale,
        _ => 1.0, // system default
    }
}
```

### Registering agent-specific preferences

```rust
use aios_preference::{PreferenceStore, PreferenceSchema, PreferenceValue, PreferenceValueType};

fn register_note_preferences(
    prefs: &dyn PreferenceStore,
    cap: &CapabilityHandle,
) -> Result<(), PreferenceError> {
    prefs.register_schema(PreferenceSchema {
        key: "com.example.notes.auto_save".into(),
        description: "Automatically save notes every N seconds".into(),
        value_type: PreferenceValueType::Integer,
        default: PreferenceValue::Integer(30),
        validation: Some(ValidationRule::Range { min: 5, max: 300 }),
    }, cap)?;

    prefs.register_schema(PreferenceSchema {
        key: "com.example.notes.spell_check".into(),
        description: "Enable real-time spell checking".into(),
        value_type: PreferenceValueType::Bool,
        default: PreferenceValue::Bool(true),
        validation: None,
    }, cap)?;

    Ok(())
}
```

### Subscribing to preference changes

```rust
use aios_preference::{PreferenceStore, PreferenceValue};

fn watch_theme_changes(
    prefs: &dyn PreferenceStore,
    cap: &CapabilityHandle,
) -> Result<PreferenceSubscription, PreferenceError> {
    prefs.on_change(
        &"display.theme".into(),
        Box::new(|old, new| {
            // Re-render UI when the theme preference changes.
            // This fires for all source tiers: user explicit change,
            // context-driven override (e.g., dark mode after sunset),
            // or enterprise policy push.
            apply_theme(new);
        }),
        cap,
    )
}
```

## 4. Integration Examples

### Preference Kit + Context Kit: temporal preference rules

```rust
use aios_preference::{PreferenceStore, PreferenceValue};
use aios_context::ContextConsumer;

// The Preference Kit evaluates context-driven rules (tier 4) internally.
// Agents just read the resolved preference -- they do not need to check
// context themselves.
fn get_effective_brightness(
    prefs: &dyn PreferenceStore,
    cap: &CapabilityHandle,
) -> f64 {
    // If a temporal rule is active (e.g., "reduce brightness after 10pm"),
    // the resolved value already reflects it. The agent sees the final value.
    match prefs.get(&"display.brightness".into(), cap) {
        Ok(PreferenceValue::Float(brightness)) => brightness,
        _ => 0.8,
    }
}
```

### Preference Kit + AIRS Kit: natural language settings

```rust
use aios_preference::{PreferenceStore, PreferenceValue};

// Internal to the Preference Kit's NLU pipeline, shown for illustration.
// Users interact via the Conversation Bar; agents do not call this directly.
fn parse_natural_language_setting(
    nlu_result: &NluParseResult,
    prefs: &dyn PreferenceStore,
    cap: &CapabilityHandle,
) -> Result<(), PreferenceError> {
    // "Make the font bigger" -> display.font_scale += 0.1
    // "Turn on dark mode" -> display.theme = "dark"
    // "Don't show tips" -> experience.show_tips = false

    let key = &nlu_result.preference_key;
    let value = &nlu_result.proposed_value;

    prefs.set(key, value.clone(), cap)
}
```

### Preference Kit + Storage Kit: cross-device sync

```rust
use aios_preference::PreferenceStore;

// Preferences sync automatically via Space Mesh when the preference store
// is backed by the `system/preferences/` Space. Agents do not need to
// manage sync explicitly. This example shows how to check sync status.
fn check_preference_sync_status(
    prefs: &dyn PreferenceStore,
    cap: &CapabilityHandle,
) -> SyncStatus {
    // The preference store reports its sync state through the Storage Kit's
    // standard Space sync interface.
    prefs.sync_status(cap).unwrap_or(SyncStatus::Unknown)
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
| --- | --- | --- |
| `PreferenceRead` | Reading system and agent preferences | Granted to all agents |
| `PreferenceWrite` | Setting preferences in the agent's namespace | Granted to all agents |
| `PreferenceWriteSystem` | Setting system-wide preferences (display, audio) | User-initiated actions only |
| `PreferenceSchema` | Registering new preference schemas | Granted to all agents |
| `PreferenceSubscribe` | Subscribing to preference change notifications | Granted to all agents |
| `PreferenceBehavior` | Approving or rejecting behavioral inferences | User-initiated actions only |
| `PreferenceExplain` | Reading the full precedence explanation | Granted to all agents |
| `PreferenceAdmin` | Enterprise policy management, bulk operations | Enterprise MDM only |

## 6. Error Handling

```rust
/// Errors returned by the Preference Kit.
pub enum PreferenceError {
    /// The agent lacks the required preference capability.
    /// Recovery: declare the capability in the agent manifest.
    CapabilityDenied(String),

    /// The preference key does not exist and no schema is registered.
    /// Recovery: register a schema first, or check the key spelling.
    KeyNotFound(PreferenceId),

    /// The value does not match the preference schema (wrong type,
    /// out of range, invalid enum variant).
    /// Recovery: validate the value against the schema before setting.
    ValidationFailed {
        key: PreferenceId,
        reason: String,
    },

    /// The preference is locked by enterprise policy (tier 1).
    /// Recovery: contact the organization administrator.
    EnterpriseLocked {
        key: PreferenceId,
        policy_id: PolicyId,
    },

    /// The agent attempted to write outside its namespace.
    /// Recovery: only set preferences under your agent's namespace
    /// (e.g., "com.example.myagent.*"), or request PreferenceWriteSystem.
    NamespaceViolation {
        key: PreferenceId,
        agent_namespace: String,
    },

    /// Too many subscriptions per agent (limit: 64 per agent).
    /// Recovery: unsubscribe from unused watchers.
    SubscriptionLimitReached {
        current: usize,
        max: usize,
    },

    /// The behavioral inference ID does not exist or has already been
    /// processed (approved or rejected).
    InferenceNotFound(InferenceId),

    /// The AIRS NLU pipeline failed to parse a natural language setting.
    /// Recovery: use the explicit `set()` API instead.
    NluParseFailed(String),

    /// Storage error during preference persistence or sync.
    StorageError(String),
}
```

## 7. Platform & AI Availability

The Preference Kit separates its core key-value store from AI-powered features:

**Always available (no AIRS dependency):**

- Reading and writing all preference values.
- Full 7-tier precedence resolution (tiers 1-3 and 6-7 are non-AI).
- Schema registration and validation.
- Change subscription notifications.
- Cross-device sync via Space Mesh.
- Enterprise policy enforcement (tier 1 and 3).

**Available when AIRS is loaded:**

- Natural language settings parsing via NLU ("make the font bigger").
- Behavioral observation and inference (tier 5) -- pattern detection that
  proposes preference changes based on observed behavior.
- Contextual bandit preference optimization -- AIRS learns which preference
  values lead to better user engagement.
- Conflict detection and resolution assistance when multiple tiers disagree.

**Feature detection:**

```rust
use aios_preference::PreferenceResolver;

fn nlu_settings_available(resolver: &dyn PreferenceResolver) -> bool {
    // When AIRS is loaded, the NLU pipeline can parse natural language.
    // Check by attempting to resolve a system preference -- the method
    // itself always works, but NLU features require AIRS.
    resolver.is_nlu_available()
}
```

**Context-driven temporal rules (tier 4):**

Temporal rules activate preferences based on environmental conditions. They
always evaluate (no AIRS dependency) since they use the [Context Kit](context.md)'s
output, which has its own rule-based fallback:

| Trigger Type | Example | AIRS Required |
| --- | --- | --- |
| Time of day | Dark mode after 10pm | No |
| Location | Mute audio at work | No (GPS signal) |
| Activity | Larger font during reading | No (context inference) |
| Device presence | Higher volume when Bluetooth speaker connected | No |
| Calendar | DND during meetings | No (calendar sync) |
| Learned pattern | Auto-brightness based on usage history | Yes (AIRS) |
