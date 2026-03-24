---
tags: [experience, agents, intelligence]
type: architecture
---

# AIOS Scriptable Agent Protocol

**Parent document:** [architecture.md](../project/architecture.md)
**ADR:** [BeOS Lessons — Lesson 1](../knowledge/decisions/2026-03-23-jl-beos-haiku-redox-lessons.md)
**Kit overview:** [App Kit](../kits/application/app.md) — Every agent implements Scriptable as part of App Kit
**Related:** [agents/sdk.md](../applications/agents/sdk.md) — Agent SDK, [tool-manager.md](../intelligence/tool-manager.md) — Tool Manager builds on Scriptable, [airs.md](../intelligence/airs.md) — AIRS uses Scriptable for discovery

-----

## 1. Core Insight

BeOS made every application scriptable. Every `BHandler` in the system published a self-describing schema of properties and verbs. The `hey` command-line tool could address any running application, traverse its object hierarchy, read and write properties, and invoke actions — with no special integration work. This was not an optional feature: it was the architecture. Every BHandler got basic scripting for free; applications extended with domain-specific suites.

AIOS makes every agent scriptable for the same reason, amplified by the AI-first nature of the OS. In BeOS, scriptability was primarily a developer convenience — power users and shell scripts could automate applications without dedicated CLI support. In AIOS, scriptability is the foundation of AIRS's ability to compose agents into multi-agent workflows. AIRS can only discover what agents can do, chain operations across agents, and explain its actions in human-readable terms if every agent exposes a uniform, introspectable protocol. An opt-in Scriptable trait would collapse this model — AIRS would need per-agent API knowledge to do anything useful.

The Scriptable trait extends BeOS's original 6 verbs (GET/SET/CREATE/DELETE/COUNT/EXECUTE) with two additions motivated by the AI-first context: `SUBSCRIBE` for reactive queries (agents can watch properties for changes, feeding the Attention Manager and Context Engine) and `DESCRIBE` for schema introspection (AIRS calls DESCRIBE to build a capability map of every running agent before composing workflows). Together, these 8 verbs and a capability-scoped hierarchical addressing model give AIRS — and human users via CLI tools — uniform control over the entire running agent population.

---

## 2. The Scriptable Trait

The `Scriptable` trait is defined in the `aios_app` crate and is mandatory for all native AIOS applications. The App Kit `Application` trait requires `Scriptable` as a supertrait — an agent that does not implement `Scriptable` cannot implement `Application` and cannot run on AIOS.

```rust
use aios_app::{AppError, PropertyInfo, PropertyValue, ScriptVerb, Specifier, Suite};

/// Mandatory for all native AIOS applications. The base derive macro provides
/// the "aios:app:lifecycle" suite (Name, State, Version, Capabilities) automatically.
/// Applications extend with domain-specific suites.
pub trait Scriptable {
    /// Return all suites this object publishes. AIRS calls this to build a
    /// capability map of the agent before constructing multi-step workflows.
    fn suites(&self) -> &[Suite];

    /// Execute a scripting request. The runtime checks capabilities at each
    /// specifier traversal step before dispatching to this method.
    fn script(
        &mut self,
        verb: ScriptVerb,
        property: &str,
        specifiers: &[Specifier],
        value: Option<PropertyValue>,
    ) -> Result<PropertyValue, AppError>;

    /// Resolve a child scriptable object for hierarchical traversal.
    /// For example: `GET Title of Window 0` resolves `Window 0` (this method),
    /// then reads `Title` on the returned child.
    /// Default returns PropertyNotFound — override for nested objects.
    fn resolve_specifier(
        &self,
        property: &str,
        specifier: &Specifier,
    ) -> Result<&dyn Scriptable, AppError> {
        let _ = (property, specifier);
        Err(AppError::PropertyNotFound)
    }
}

/// A suite is a named group of properties with declared verb support and
/// capability requirements. Suites are self-describing — DESCRIBE returns them.
pub struct Suite {
    /// Reverse-DNS suite identifier. e.g. "com.example:editor:document"
    pub id: &'static str,
    /// Human-readable name for UI tools and AIRS explanations.
    pub name: &'static str,
    /// The properties in this suite.
    pub properties: &'static [PropertyInfo],
}

/// Metadata for a single scriptable property.
pub struct PropertyInfo {
    /// Property name as used in scripting requests. e.g. "Title", "Selection"
    pub name: &'static str,
    /// The value type returned or accepted by this property.
    pub value_type: ValueType,
    /// Which verbs are valid for this property.
    pub supported_verbs: &'static [ScriptVerb],
    /// Capability required to access this property. None = accessible to any
    /// caller with a channel to this agent.
    pub required_capability: Option<CapabilityKind>,
    /// Human-readable description for AIRS and UI tools.
    pub description: &'static str,
}
```

The `Specifier` type encodes how a property is addressed within a hierarchy. AIOS supports four specifier forms inherited from BeOS:

```rust
pub enum Specifier {
    /// By zero-based index. e.g. `Window 0`
    Index(i64),
    /// By reverse index from end. e.g. `Window -1` (last window)
    ReverseIndex(i64),
    /// By name. e.g. `Window "main"`
    Name(String),
    /// A contiguous range. e.g. `Entries 0 through 9`
    Range { start: i64, end: i64 },
}
```

---

## 3. Verb Semantics

| Verb | Meaning | Example | Returns |
|---|---|---|---|
| `GET` | Read a property value | `GET Title of Window 0 of Agent "editor"` | `PropertyValue` |
| `SET` | Write a property value | `SET Selection of View 0 of Window 0 of Agent "editor" to Range(10, 20)` | `PropertyValue::Unit` |
| `CREATE` | Instantiate a new entity | `CREATE Window of Agent "editor"` | `PropertyValue::ObjectRef` (new object's name/id) |
| `DELETE` | Remove an entity | `DELETE Entry "draft.txt" of Agent "file-manager"` | `PropertyValue::Unit` |
| `COUNT` | Count entities in a collection | `COUNT Entry of Agent "file-manager"` | `PropertyValue::Int` |
| `EXECUTE` | Invoke a named action | `EXECUTE SaveAll of Agent "editor"` | `PropertyValue` (action-defined) |
| `SUBSCRIBE` | Register for property change notifications | `SUBSCRIBE State of Agent "downloader"` | `PropertyValue::SubscriptionId` |
| `DESCRIBE` | Return suite schema | `DESCRIBE of Agent "editor"` | `PropertyValue::SuiteList` |

`SUBSCRIBE` delivers change events via the caller's existing IPC channel to the agent. Subscriptions are scoped to the calling channel's lifetime — when the channel closes, all subscriptions on it are automatically cancelled. This is the same lifetime model as BeOS node monitoring.

`DESCRIBE` is the introspection verb. It takes no specifiers and always returns a `PropertyValue::SuiteList` containing all suites with full `PropertyInfo` metadata. It is the only verb that ignores per-property capability requirements — if a caller can establish a channel to an agent, they can call DESCRIBE. This is intentional: knowing what an agent can do is not the same as being allowed to do it.

---

## 4. Property Discovery

AIRS calls `DESCRIBE` at agent startup to build a capability map of every registered agent. The result is cached in the Tool Manager's `ToolRegistry` and refreshed when an agent emits a `SuitesChanged` notification (a special system-level `SUBSCRIBE` event that agents fire when they add or remove suites at runtime).

```rust
/// The runtime representation of a DESCRIBE response, stored in ToolRegistry.
pub struct AgentScriptableMap {
    pub agent_id: AgentId,
    pub suites: Vec<SuiteDescriptor>,
    pub captured_at: Timestamp,
}

pub struct SuiteDescriptor {
    pub suite_id: String,
    pub suite_name: String,
    pub properties: Vec<PropertyDescriptor>,
}

pub struct PropertyDescriptor {
    pub name: String,
    pub value_type: ValueType,
    pub supported_verbs: Vec<ScriptVerb>,
    /// Present if access requires a specific capability.
    pub required_capability: Option<CapabilityKind>,
    pub description: String,
}
```

AIRS uses the `AgentScriptableMap` to answer questions like: "Which agents expose a writable `Selection` property of type `TextRange`?" or "Which agents support the `EXECUTE SaveAs` verb?" This is how AIRS composes multi-agent workflows without per-agent API knowledge — it queries the live capability map rather than consulting a static registry.

The Conversation Manager surfaces this to users: if a user asks "what can the editor do?", the Conversation Manager issues a `DESCRIBE` request and formats the result as a natural-language capability summary.

---

## 5. Hierarchical Addressing

Scriptable requests use hierarchical specifiers to address nested objects. The path `GET Password of Account "admin" of Agent "identity"` traverses three levels:

1. `Agent "identity"` — resolves the top-level agent by name via the Service Manager
2. `Account "admin"` — calls `resolve_specifier("Account", Name("admin"))` on the agent, returning the child `Scriptable` object representing that account
3. `GET Password` — calls `script(ScriptVerb::Get, "Password", &[], None)` on the child

The capability system enforces access at every traversal step. Each `resolve_specifier` call checks the `required_capability` of the property being resolved, not just the final property. This means:

- `ChannelAccess` to `"identity"` → required to address the agent at all
- `PropertyAccess(Account)` → required to enumerate or address accounts
- `PropertyAccess(Account.Password)` → required to read the `Password` property

Derived capabilities are always a subset of the parent — the runtime evaluates the conjunction of all capabilities along the path. An attenuated token that grants `PropertyAccess(Account)` but not `PropertyAccess(Account.Password)` will fail at the final step with `CapabilityDenied`, not at the first step. This gives callers the ability to check what they can reach before attempting to read sensitive properties.

```rust
/// The runtime capability check performed at each traversal step.
fn check_traversal_capability(
    caller_caps: &CapabilitySet,
    prop_info: &PropertyInfo,
    verb: ScriptVerb,
) -> Result<(), AppError> {
    if let Some(required) = prop_info.required_capability {
        if !caller_caps.has(required) {
            return Err(AppError::CapabilityDenied {
                required,
                verb,
                property: prop_info.name,
            });
        }
    }
    // Additionally verify the verb is declared as supported for this property.
    if !prop_info.supported_verbs.contains(&verb) {
        return Err(AppError::VerbNotSupported { verb, property: prop_info.name });
    }
    Ok(())
}
```

---

## 6. AIRS Integration

The ADR recommendation is clear: AIRS uses Scriptable for **discovery and composition**, not as the primary execution path. The distinction matters:

**Discovery** — AIRS calls `DESCRIBE` on agents to populate the Tool Manager's registry. This happens at agent startup and on `SuitesChanged` notifications. No capability beyond `ChannelAccess` is required.

**Composition** — When AIRS constructs a multi-step workflow, it assembles a sequence of Scriptable verbs across agents. For example, "find all PDFs in the research Space and extract their key claims" becomes: `COUNT Entry of Agent "file-manager"` → `GET Entry N of Agent "file-manager"` (repeated) → `EXECUTE Extract of Agent "pdf-parser"` (per file). AIRS knows how to chain these because `DESCRIBE` told it the types: `GET Entry` returns `ObjectRef(ContentType::Pdf)` and `EXECUTE Extract` accepts `ObjectRef(ContentType::Pdf)`.

**Execution** — For complex operations, AIRS routes through the Tool Manager's 7-stage pipeline (schema validation → capability check → intent verification → sandbox → execution → output validation → audit). The Tool Manager wraps Scriptable with safety levels, timeout policies, and the behavioral monitor. Scriptable is the plumbing; Tool Manager is the safe API.

**Exception: simple reads** — `GET` and `COUNT` on non-sensitive properties bypass the Tool Manager. AIRS issues them directly when building context (e.g., reading agent state before deciding how to route a user request). Direct reads are logged to the audit ring but do not go through the 7-stage pipeline.

```text
User request → AIRS
    │
    ├── Discovery: DESCRIBE all agents → AgentScriptableMap cache
    │
    ├── Composition: build verb chain from type-matched capabilities
    │       │
    │       ├── Simple reads (GET/COUNT, non-sensitive) → direct Scriptable call
    │       │
    │       └── Actions (EXECUTE, SET, CREATE, DELETE) → Tool Manager pipeline
    │
    └── Result → Conversation Manager
```

---

## 7. Default Implementation

The `#[derive(Scriptable)]` macro generates an implementation of the `Scriptable` trait that exposes the mandatory `"aios:app:lifecycle"` suite. Every agent gets this for free. The suite contains four read-only properties:

| Property | Type | Verbs | Description |
|---|---|---|---|
| `Name` | `String` | GET | Human-readable agent name from manifest |
| `State` | `AppState` | GET, SUBSCRIBE | Current lifecycle state (Running, Suspended, Stopping) |
| `Version` | `String` | GET | Semver version string from manifest |
| `Capabilities` | `CapabilityList` | GET | Active capabilities (names only, not tokens) |

`State` supports `SUBSCRIBE` — callers can watch for lifecycle transitions. This is the mechanism AIRS uses to detect when an agent enters or leaves the `Running` state.

Agents extend their scriptable interface by declaring additional suites in their manifest and implementing the corresponding properties:

```rust
use aios_sdk::prelude::*;

#[agent(
    name = "Text Editor",
    version = "1.2.0",
    capabilities = [ReadSpace("user/home"), WriteSpace("user/home")],
)]
#[derive(Scriptable)]  // generates aios:app:lifecycle suite automatically
struct TextEditor {
    documents: Vec<Document>,
}

// Extend with domain-specific scripting.
impl Scriptable for TextEditor {
    fn suites(&self) -> &[Suite] {
        &[
            // Provided by #[derive(Scriptable)] — aios:app:lifecycle
            lifecycle_suite(),
            // Application-defined
            &Suite {
                id: "com.example:editor:document",
                name: "Document",
                properties: &[
                    PropertyInfo {
                        name: "Title",
                        value_type: ValueType::String,
                        supported_verbs: &[ScriptVerb::Get, ScriptVerb::Set],
                        required_capability: None,
                        description: "The document title shown in the title bar.",
                    },
                    PropertyInfo {
                        name: "Selection",
                        value_type: ValueType::TextRange,
                        supported_verbs: &[ScriptVerb::Get, ScriptVerb::Set],
                        required_capability: None,
                        description: "The current text selection.",
                    },
                    PropertyInfo {
                        name: "WordCount",
                        value_type: ValueType::Int,
                        supported_verbs: &[ScriptVerb::Get],
                        required_capability: None,
                        description: "Total word count of the active document.",
                    },
                ],
            },
        ]
    }

    fn script(
        &mut self,
        verb: ScriptVerb,
        property: &str,
        specifiers: &[Specifier],
        value: Option<PropertyValue>,
    ) -> Result<PropertyValue, AppError> {
        match (verb, property) {
            (ScriptVerb::Get, "Title") => {
                let doc = self.resolve_document(specifiers)?;
                Ok(PropertyValue::String(doc.title.clone()))
            }
            (ScriptVerb::Set, "Title") => {
                let title = value
                    .and_then(|v| v.as_string())
                    .ok_or(AppError::TypeMismatch)?;
                let doc = self.resolve_document_mut(specifiers)?;
                doc.title = title;
                Ok(PropertyValue::Unit)
            }
            (ScriptVerb::Get, "WordCount") => {
                let doc = self.resolve_document(specifiers)?;
                Ok(PropertyValue::Int(doc.word_count() as i64))
            }
            _ => Err(AppError::PropertyNotFound),
        }
    }
}
```

---

## 8. Design Principles

1. **Universality** — Scriptable is mandatory, not opt-in. Every agent in the system is addressable through the same protocol. AIRS can rely on `DESCRIBE` working on any agent, any time, without per-agent knowledge. This is the BeOS invariant that made `hey` powerful — applied to AI composition.

2. **Capability-scoped** — Property access checks capabilities at each specifier traversal step. The capability system's attenuation model applies: capabilities granted to navigate to an object do not automatically grant access to sensitive properties within it. The path `Account "admin" of Agent "identity"` and the path `Password of Account "admin" of Agent "identity"` may require different capabilities.

3. **Introspectable** — `DESCRIBE` is always available to any caller with a channel. Knowing what an agent can do is not the same as being allowed to do it. Capability checks apply on execution, not discovery. This lets AIRS build accurate capability maps without requiring elevated access.

4. **Composable** — AIRS chains verbs across agents based on declared types. A property returning `ObjectRef(ContentType::Pdf)` can feed any action accepting the same type, regardless of which agent implements it. Type compatibility is declared in `PropertyInfo`, making composition mechanical rather than requiring LLM reasoning about APIs.

5. **Human-readable** — Verbs and property names are descriptive text, not numeric opcodes. The scripting log visible in the Inspector reads like: `EXECUTE SaveAll of Agent "com.example.editor"` — comprehensible to both users and AIRS explanations. This is a deliberate continuity with BeOS `hey` syntax.

-----

## Cross-Reference Index

| Topic | Document | Relevant Sections |
|---|---|---|
| Scriptable trait definition | [App Kit](../kits/application/app.md) | §2.4 — Scriptable trait, Suite, PropertyInfo |
| Agent SDK integration | [agents/sdk.md](../applications/agents/sdk.md) | §8.1 AgentContext.scriptable(), §9 Scriptable Protocol |
| Tool Manager builds on Scriptable | [tool-manager.md](../intelligence/tool-manager.md) | §3 ToolRegistry, §5 Execution pipeline |
| AIRS discovery and composition | [airs.md](../intelligence/airs.md) | §5.7 Tool Manager, §5.3 Context Engine |
| Capability attenuation model | [security/model/capabilities.md](../security/model/capabilities.md) | §3.4 Attenuation, §3.5 Delegation |
| Conversation Manager surfaces DESCRIBE | [conversation-manager.md](../intelligence/conversation-manager.md) | §7 Tool discovery |
| BeOS design rationale | [ADR: BeOS Lessons](../knowledge/decisions/2026-03-23-jl-beos-haiku-redox-lessons.md) | Lesson 1: Scriptable Agent Protocol |
| Inspector shows scripting log | [inspector.md](../applications/inspector.md) | §5 Views |
