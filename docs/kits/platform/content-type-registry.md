---
tags: [platform, agents, storage]
type: architecture
---

# AIOS Content Type Registry

**Parent document:** [README.md](../README.md) — Kit Architecture
**ADR:** [BeOS Lessons — Lesson 4](../../knowledge/decisions/2026-03-23-jl-beos-haiku-redox-lessons.md)
**Related:** [agents/lifecycle.md](../../applications/agents/lifecycle.md) — Agent manifest declares handled types, [space-indexer.md](../../intelligence/space-indexer.md) — Content type sniffing during indexing, [preferences.md](../../intelligence/preferences.md) — User handler overrides, [flow/transforms.md](../../storage/flow/transforms.md) — Translation Kit converts between types

-----

## 1. Overview

The Content Type Registry is a system-wide service that maps content types to the agents that handle them. When a user or AIRS opens a Space object, the registry resolves the preferred handler agent — the AIOS equivalent of BeOS's `registrar` server and MIME database. Like BeOS, it supports preferred handlers, supporting handlers, supertype wildcards, and a clear priority order when multiple agents compete for the same type.

The registry lives in the Service Manager, not in any Kit. This placement reflects its nature: it is a routing table consulted by multiple independent subsystems — the Space Indexer (type sniffing during indexing), AIRS (contextual handler selection), Flow Kit (content negotiation), and the compositor (launch-on-open). App Kit's role is to *declare* handled types in the agent manifest; Service Manager's role is to *store and resolve* the registry.

Dynamic registration is supported. Agents can add and remove handled types at runtime, subject to capability checks. The static manifest declaration is the starting point; runtime updates allow agents to adjust their coverage as features are enabled or disabled at runtime.

-----

## 2. Registry Data Model

Each registered content type is described by a `ContentTypeEntry`. The registry holds one entry per canonical MIME type string. Supporting handlers are stored in priority order, with the preferred handler tracked separately for O(1) resolution.

```rust
/// A registered content type and its associated handler agents.
pub struct ContentTypeEntry {
    /// Canonical MIME type string, e.g. "text/markdown" or "image/png".
    /// Supertype wildcards ("text/*") are stored with their own entries.
    pub mime_type: String,

    /// The single preferred handler for this type.
    /// None if no agent has claimed preferred status.
    pub preferred_handler: Option<AgentId>,

    /// Agents that support this type but are not the preferred handler.
    /// Ordered: first entry is offered first in "open with" menus.
    pub supporting_handlers: Vec<AgentId>,

    /// Sniffing rules used by the Space Indexer to detect this type
    /// from raw bytes (magic numbers, byte patterns, extension hints).
    pub sniffing_rules: Vec<SniffingRule>,

    /// Supertype wildcard handler (e.g. "audio/*" for any audio type).
    /// Consulted only when no exact-match handler is found.
    pub supertype_handler: Option<AgentId>,
}

/// A single sniffing rule used to identify a content type from raw bytes.
pub struct SniffingRule {
    /// Byte offset at which the pattern must appear.
    pub offset: u32,
    /// Byte pattern to match (exact bytes, not a regex).
    pub pattern: Vec<u8>,
    /// Relative priority when multiple rules match the same bytes.
    /// Higher value wins. Used to resolve ambiguous magic numbers.
    pub priority: u8,
}

/// A registration request submitted by an agent at activation or runtime.
pub struct TypeRegistration {
    /// The agent submitting the registration.
    pub agent_id: AgentId,
    /// MIME type being registered (exact or supertype wildcard).
    pub mime_type: String,
    /// Whether this agent claims to be the preferred handler.
    /// Requires no existing preferred handler, or a user override.
    pub preferred: bool,
    /// Sniffing rules contributed by this agent for this type.
    pub sniffing_rules: Vec<SniffingRule>,
}
```

-----

## 3. Registration

Agents declare their handled content types in the `content_types` section of their package manifest. The Agent Runtime submits these declarations to the Service Manager during activation. Agents may also register or deregister types at runtime via the `ContentTypeRegistry` IPC interface.

```toml
# manifest.toml — relevant section
[content_types]
handled = [
    { mime = "text/markdown",          preferred = true  },
    { mime = "text/plain",             preferred = false },
    { mime = "application/json",       preferred = false },
]
```

The Service Manager validates each registration against the agent's capability set before accepting it. An agent that lacks `StorageRead` capability for a given Space cannot register as a handler for objects stored in that Space, because it would be unable to fulfil the open request it volunteered to handle.

```rust
/// The IPC trait exposed by Service Manager for content type management.
/// All methods are capability-gated at the IPC call site.
pub trait ContentTypeRegistry {
    /// Register a content type handler.
    /// Returns Err(RegistryError::CapabilityDenied) if the agent lacks
    /// the capability required to handle this type.
    fn register(
        &self,
        registration: TypeRegistration,
    ) -> Result<(), RegistryError>;

    /// Remove a previously registered handler entry for the calling agent.
    fn deregister(
        &self,
        agent_id: AgentId,
        mime_type: &str,
    ) -> Result<(), RegistryError>;

    /// Resolve the preferred handler for a content type.
    /// Applies the full priority chain; see §4.
    fn resolve(
        &self,
        mime_type: &str,
        context: &ResolutionContext,
    ) -> Result<Option<AgentId>, RegistryError>;

    /// List all agents that support a given content type,
    /// in resolution priority order.
    fn supporting_handlers(
        &self,
        mime_type: &str,
    ) -> Result<Vec<AgentId>, RegistryError>;
}
```

-----

## 4. Resolution Algorithm

When the system needs to open a Space object, it calls `ContentTypeRegistry::resolve` with the object's content type and a `ResolutionContext` carrying the active user preferences. The registry applies a fixed priority chain:

```text
1. User override         — explicit preference stored via Preference service
2. Preferred handler     — agent that claimed preferred status at registration
3. Supporting handler    — first entry in supporting_handlers list
4. Supertype handler     — wildcard entry matching "type/*"
5. AIRS suggestion       — contextual recommendation (advisory only)
6. None                  — no handler found; caller decides how to proceed
```

```rust
/// Context supplied to the resolution algorithm.
pub struct ResolutionContext {
    /// Explicit user overrides from the Preference service.
    /// Key: mime_type string. Value: AgentId of preferred handler.
    pub user_overrides: HashMap<String, AgentId>,

    /// Whether to allow AIRS to contribute a suggestion at step 5.
    /// Callers set this to false when a deterministic answer is required
    /// (e.g. automated workflows).
    pub allow_airs_suggestion: bool,
}

fn resolve_handler(
    entries: &BTreeMap<String, ContentTypeEntry>,
    mime_type: &str,
    ctx: &ResolutionContext,
) -> Option<AgentId> {
    // Step 1: explicit user override always wins.
    if let Some(agent_id) = ctx.user_overrides.get(mime_type) {
        return Some(*agent_id);
    }

    let entry = entries.get(mime_type)?;

    // Step 2: registered preferred handler.
    if let Some(handler) = entry.preferred_handler {
        return Some(handler);
    }

    // Step 3: first supporting handler.
    if let Some(handler) = entry.supporting_handlers.first() {
        return Some(*handler);
    }

    // Step 4: supertype wildcard ("text/*", "audio/*", etc.).
    let supertype = mime_supertype(mime_type); // e.g. "text/markdown" → "text/*"
    if let Some(entry) = registry.get_entry(supertype) {
        if let Some(handler) = entry.supertype_handler {
            return Some(handler);
        }
    }

    // Step 5: AIRS contextual suggestion (advisory).
    if ctx.allow_airs_suggestion {
        return airs_suggest_handler(mime_type);
    }

    None
}
```

Supertype wildcards use the BeOS convention: exact type beats wildcard. `text/markdown` preferred handler wins over a `text/*` supertype handler. Only `type/*` patterns are supported — arbitrary globs are not, as they create ambiguity in resolution order.

-----

## 5. Content Type Sniffing

The Space Indexer identifies content types during indexing. It applies sniffing rules from the registry to raw block content before storing `SemanticMetadata`. Three mechanisms are tried in order:

1. **Byte pattern matching** — magic numbers and file signatures (e.g. `%PDF-` at offset 0 for `application/pdf`, `\x89PNG` for `image/png`)
2. **Extension mapping** — filename extension extracted from the object name attribute, cross-referenced against a built-in extension table
3. **Structural heuristics** — UTF-8 validity, JSON brace balance, XML prologue detection for `text/*` types

The `ContentType` enum in `shared/src/storage.rs` represents the types the kernel and shared crates work with natively. MIME strings in the registry are the authoritative external representation; `ContentType` is the efficient internal form used in hot paths.

```rust
/// Sniff the content type of a byte slice.
/// Applies registered sniffing rules in priority order, then falls back
/// to extension mapping, then structural heuristics.
/// Returns the most specific matching MIME type, or "application/octet-stream"
/// if nothing matches.
pub fn sniff_content_type(
    bytes: &[u8],
    filename_hint: Option<&str>,
    entries: &BTreeMap<String, ContentTypeEntry>,
) -> String {
    // Phase 1: byte patterns from registered sniffing rules.
    let mut candidates: Vec<(u8, &str)> = entries
        .iter()
        .flat_map(|(mime, e)| e.sniffing_rules.iter().map(move |r| (mime.as_str(), r)))
        .filter_map(|(mime, rule)| {
            let end = rule.offset as usize + rule.pattern.len();
            if bytes.len() >= end
                && bytes[rule.offset as usize..end] == *rule.pattern
            {
                Some((rule.priority, mime))
            } else {
                None
            }
        })
        .collect();
    candidates.sort_by_key(|(priority, _)| core::cmp::Reverse(*priority));
    if let Some((_, mime)) = candidates.first() {
        return mime.to_string();
    }

    // Phase 2: file extension hint.
    if let Some(name) = filename_hint {
        if let Some(ext) = extension_of(name) {
            if let Some(mime) = EXTENSION_TABLE.get(ext) {
                return mime.to_string();
            }
        }
    }

    // Phase 3: structural heuristics for text types.
    sniff_structural(bytes).unwrap_or("application/octet-stream")
}
```

-----

## 6. AIRS Integration

AIRS participates in handler selection at two points: as the step-5 fallback in the resolution algorithm, and as a long-running preference learner that populates user overrides over time.

The **preference learner** observes which agent the user actually uses to open each content type. When a pattern is stable across several sessions — "Justin always opens `.py` files with the code editor, not the text viewer" — AIRS proposes a user override to the Preference service. The user sees a notification: "Set code-editor as your default for Python files?" Accepting writes the override to the user's preference Space, where it becomes a permanent step-1 entry in the resolution chain.

The **contextual suggestion** (step 5) is advisory: it is only consulted when no deterministic handler exists. AIRS may suggest a handler based on the current context (time of day, active project, recent activity). Automated callers set `allow_airs_suggestion: false` to skip this step and receive a deterministic `None` rather than a potentially inconsistent suggestion.

The registry itself exposes the `Scriptable` trait (from the Scriptable Protocol), making it introspectable by AIRS through standard verbs:

```text
GET PreferredHandler of ContentType "application/pdf"  → AgentId
GET SupportedTypes of Agent "code-editor"             → Vec<String>
SET PreferredHandler of ContentType "text/plain" to "my-editor"
COUNT Handler of ContentType "image/*"                → usize
```

-----

## 7. Translation Kit Interaction

The Content Type Registry and Translation Kit address complementary problems. The registry answers "which agent opens this type?" The Translation Kit answers "how do I convert this type to another type?" Together they resolve compound requests: "open this `.docx` as a PDF" = registry resolves the PDF viewer + Translation Kit provides the `.docx` → `application/pdf` converter.

The Translation Kit's `ConversionGraph` stores edges as `(source_mime, target_mime) → Transform`. When Flow Kit needs to deliver content to an agent that accepts a different type than the source, it queries the registry for the target type, then asks the Translation Kit for a conversion path. The Translation Kit is not aware of handlers; the registry is not aware of conversion paths. The caller (Flow Kit, AIRS, or the compositor) holds both references and composes the query.

```rust
/// Resolve a handler for `object` with optional type conversion.
/// Returns the target agent and the transform to apply (if any).
pub fn resolve_with_conversion(
    registry: &dyn ContentTypeRegistry,
    translation: &dyn ConversionGraph,
    object_type: &str,
    ctx: &ResolutionContext,
) -> Option<(AgentId, Option<Transform>)> {
    // Try direct resolution first (no conversion needed).
    if let Some(agent) = registry.resolve(object_type, ctx).ok().flatten() {
        return Some((agent, None));
    }

    // Find a reachable type that has a handler, via Translation Kit.
    for (target_type, transform) in translation.reachable_from(object_type) {
        if let Some(agent) = registry.resolve(&target_type, ctx).ok().flatten() {
            return Some((agent, Some(transform)));
        }
    }

    None
}
```

-----

## 8. Design Principles

1. **Capability-gated** — agents can only register as handlers for types they have capability to access; the registry enforces this at every `register` call.
2. **Agent manifest is source of truth** — registration is driven by manifest declarations at activation; runtime updates are additive adjustments, not replacements.
3. **User override always wins** — AIRS suggestions are advisory and never bypass an explicit user preference stored in the Preference service.
4. **Dynamic** — agents add and remove handled types at runtime; the registry is a live routing table, not a static database baked at install time.
5. **Exact type beats wildcard** — `text/markdown` preferred handler wins over a `text/*` supertype handler; arbitrary glob patterns are not supported.
6. **Service Manager, not App Kit** — the registry is a system-level routing concern consulted by multiple subsystems; App Kit's role is declaration, not storage.

-----
