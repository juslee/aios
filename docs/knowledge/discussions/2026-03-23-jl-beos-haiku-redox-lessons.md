---
author: Justin Lee
date: 2026-03-23
tags: [kits, platform, intelligence, storage, compositor, security]
status: draft
---

# Discussion: Additional Lessons from BeOS, Haiku OS, and Redox OS

## Context

AIOS already adopts several foundational BeOS design patterns: the Kit architecture (30 Kits, 4 layers), App Kit messaging (BLooper/BHandler), Translation Kit (format conversion), Interface Kit naming, Storage Kit→Spaces evolution, and Flow Kit (unifying clipboard/drag-drop/share). These were formalized in the [Custom Core discussion](2026-03-16-jl-platform-vision-custom-core.md) and 10 ADRs extracted on 2026-03-22.

This discussion explores **7 additional design lessons** from three sources:

- **BeOS** (Be Inc., 1996–2001) — the original inspiration
- **Haiku OS** (open-source, 2001–present) — BeOS's successor, with 25 years of evolution
- **Redox OS** (Rust microkernel) — shares AIOS's language and microkernel DNA

The goal is to capture lessons that AIOS hasn't yet formalized, explore how they interact, and identify which are strong enough to graduate into ADRs.

---

## Lesson 1: Scriptable Agent Protocol

### Source: BeOS Hey Command + Scripting Suites

BeOS made every GUI object programmatically controllable via a standardized protocol. Every `BHandler` (the base message-handling class) could publish **suites** — self-describing schemas declaring what properties it exposes and what verbs it supports:

| Verb | Meaning | Example |
|---|---|---|
| `GET` | Read a property | `hey Tracker get Name of Entry 0 of Poses of Window /boot/home` |
| `SET` | Write a property | `hey StyledEdit set Value of Font of View 0 of Window 0 to "Courier"` |
| `CREATE` | Add a new entity | `hey NetPositive create Window` |
| `DELETE` | Remove an entity | `hey Tracker delete Entry "file.txt" of Poses of Window /boot/home` |
| `COUNT` | Count entities | `hey Tracker count Entry of Poses of Window /boot/home` |
| `EXECUTE` | Invoke an action | `hey StyledEdit execute Quit` |

The `hey` CLI tool could address any running application, traverse its object hierarchy via specifiers (index, name, reverse index, range), and compose operations. Every `BPropertyInfo` struct declared which verbs were valid for which properties, making the protocol introspectable.

Haiku extended this with the Scripting Explorer app, which lets you browse any running application's entire scriptable interface graphically.

### AIOS Mapping

**Every agent should implement a `Scriptable` trait** with standard verbs and property discovery. This is the single most impactful lesson for an AI-first OS because it enables AIRS to:

1. **Discover** what any agent can do (introspect suites)
2. **Compose** multi-agent workflows without custom integration
3. **Control** agents via a uniform protocol (no per-agent API learning)
4. **Explain** what it's doing to the user (verbs are human-readable)

Affected subsystems: App Kit, AIRS (Tool Manager), Conversation Manager, Service Manager.

### Design Considerations

- **Relationship to Tool Manager:** The existing Tool Manager already has `ToolRegistry` with `RegisteredTool` and schema-based tool definitions aligned with MCP. The Scriptable trait could be a *lower-level primitive* that Tool Manager builds on — tools are scriptable objects with additional metadata (descriptions, safety levels, timeout policies).
- **Relationship to MCP:** MCP tools have `inputSchema`/`outputSchema`. BeOS suites have property declarations with supported verbs. These are complementary: MCP for external tool integration, Scriptable for native agent introspection.
- **Verb set:** BeOS's 6 verbs (GET/SET/CREATE/DELETE/COUNT/EXECUTE) map cleanly to CRUD + introspection. Consider adding `SUBSCRIBE` (for reactive queries, see Lesson 2) and `DESCRIBE` (return suite schema).
- **Addressing:** BeOS used hierarchical specifiers (`Entry 0 of Poses of Window /boot/home`). AIOS should use capability-scoped paths — you can only address objects you have capabilities for.

### Open Questions → Recommendations

1. **Should the Scriptable trait be mandatory for all agents, or opt-in?**
   **Recommendation: Mandatory**, with a default implementation. The whole point of BeOS scripting was universality — `hey` could talk to *any* app. If Scriptable is opt-in, AIRS can't rely on it for discovery/composition, which kills the value proposition. Provide a derive macro or default impl that exposes basic lifecycle properties (Name, State, Version, Capabilities) automatically. Agents extend with domain-specific suites. This mirrors how every `BHandler` in BeOS got basic scripting for free.

2. **How does capability attenuation work with hierarchical property access?**
   **Recommendation: Each specifier step attenuates capabilities.** Traversing `Account "admin" → Password` checks capabilities at each level. The `PropertyInfo` struct already has `capability: Option<Capability>` — the runtime evaluates the *conjunction* of all capabilities along the path. Example: `GET Password of Account "admin" of Agent "identity"` requires (1) `ChannelAccess` to the identity agent, (2) `PropertyAccess(Account)` to enumerate accounts, (3) `PropertyAccess(Account.Password)` to read the field. This naturally maps to AIOS's existing capability attenuation model — derived capabilities are always a subset of the parent.

3. **Does AIRS use the scriptable protocol for ALL agent interaction, or only for discovery/composition?**
   **Recommendation: Discovery + composition, not execution.** AIRS uses Scriptable for (a) `DESCRIBE` verb to introspect what agents can do, (b) building multi-agent workflows by chaining verbs. For actual execution of complex operations, AIRS goes through Tool Manager (which wraps Scriptable with safety levels, timeouts, and the 7-stage pipeline). Scriptable is the *plumbing*; Tool Manager is the *safe API*. Exception: simple property reads (`GET Name`, `GET State`) go directly through Scriptable — they're introspection, not execution.

---

## Lesson 2: Reactive Queries on Spaces

### Source: BFS Live Queries + Haiku Node Monitoring

BFS (the Be File System) supported **live queries** — queries that remain open and push notifications when results change. Under the hood:

1. Application creates a `BQuery` with a predicate (e.g., `name == "*.pdf" && last_modified > 1 day ago`)
2. Query executes immediately, returning matching entries
3. Kernel registers the query predicate internally
4. On every file attribute modification, kernel checks all registered predicates
5. Matching changes are sent as `B_QUERY_UPDATE` messages (entry added/removed)

Separately, Haiku's **Node Monitoring** (`watch_node()`) provides lower-level notifications: `B_ENTRY_CREATED`, `B_ENTRY_REMOVED`, `B_ENTRY_MOVED`, `B_ATTR_CHANGED`, `B_STAT_CHANGED`. Applications can watch individual nodes or entire directories.

### AIOS Mapping

The Space Indexer currently provides one-shot search (full-text BM25 + semantic embeddings). Adding **reactive queries** would transform Spaces from a passive store into a live data fabric:

- **Subscribe to a query predicate** → receive push notifications as Space objects change
- **AIRS context feeds** → "notify me when any document in workspace/ mentions 'deadline'" — no polling
- **Attention Manager integration** → reactive queries feed the attention priority system
- **Flow Kit integration** → new content matching a subscription auto-appears in Flow

Affected subsystems: Space Indexer, Context Engine, AIRS, Attention Manager, Flow Kit.

### Design Considerations

- **Predicate storage:** Store active predicates in the Space Indexer. On every `object_create`/`object_update`/`object_delete`, evaluate registered predicates against the changed object.
- **Performance:** Naive approach (check all predicates on every mutation) is O(predicates × mutations). Optimize with predicate indexing — group predicates by content type, Space, or attribute name.
- **Notification coalescing:** Batch notifications to avoid flooding subscribers. Configurable: immediate (for attention-critical), debounced (for background feeds), or batched (for periodic digests).
- **Capability scoping:** A reactive query only receives notifications for objects the subscriber has capability to access. Predicate evaluation must check capabilities.
- **Lifetime:** Queries die with the subscribing agent (like BeOS). Persistent queries could be stored as Space metadata for system-level feeds.

### Open Questions → Recommendations

1. **Should reactive queries operate at the Space Indexer level (semantic) or Block Engine level (structural)?**
   **Recommendation: Space Indexer level.** The Block Engine is a storage primitive — it shouldn't know about query predicates. The Block Engine emits low-level mutation events (object created/updated/deleted) that the Space Indexer consumes. The Space Indexer evaluates registered predicates against these events. This preserves layer separation: Block Engine → mutation stream → Space Indexer → predicate evaluation → subscriber notifications.

2. **How do reactive queries interact with the Version Store?**
   **Recommendation: Notify on version creation as a separate subscription type.** Version creation is a Space Indexer event like any other. This proposes an **extension to the existing `SpaceQuery` filtering semantics** — a reactive query like `subscribe(object_id, VersionEventFilter::Created, channel, SubscriptionMode::Debounced(500ms))` would notify subscribers when new versions appear for a specific object. Use case: AIRS subscribes to version events on a document → detects rapid version creation → infers "user is actively editing" → adjusts attention priority. Version events should be opt-in per subscription (`include_version_events: bool` in `SubscriptionMode`).

3. **Maximum concurrent reactive queries per agent? System-wide?**
   **Recommendation: Per-agent 32, system-wide 1024.** Each reactive query costs predicate storage + evaluation on every mutation. 32 per agent prevents flooding. 1024 system-wide is generous but bounded. Both are capability-gated — an agent needs `QuerySubscription` capability, and the system can reject under pressure. For comparison: Haiku's `BQuery` had no documented limit, but practical use rarely exceeded a dozen per app. AIOS agents are more autonomous, so higher limits make sense.

---

## Lesson 3: Package-as-Filesystem with State Rollback

### Source: Haiku hpkg + packagefs

Haiku's package management is architecturally unique among operating systems:

1. **Packages are never extracted.** A `.hpkg` file is a compressed filesystem image.
2. **`packagefs`** is a kernel module that mounts a virtual union filesystem from all activated packages. The filesystem view is the overlay of all package contents.
3. **Installation = activate.** Add `.hpkg` to `/system/packages/` → packagefs immediately shows its contents.
4. **Uninstallation = deactivate.** Remove `.hpkg` → contents disappear. Zero residue.
5. **State snapshots.** Every package activation/deactivation creates a state snapshot. The boot loader can select any previous state — instant rollback.
6. **Writable overlay.** User modifications go to a writable layer on top of the package union. Packages themselves are immutable.

### AIOS Mapping

For agent lifecycle management, this pattern provides:

- **Tamper resistance:** Agent code is sealed in a capability-constrained container, mounted read-only
- **Instant rollback:** If an agent update causes problems, revert to previous state by mounting the old package set
- **Clean uninstall:** Deactivate package → agent's entire filesystem footprint vanishes
- **Atomic updates:** Replace the package file → packagefs serves new contents

This pairs naturally with AIOS's existing:
- **A/B update scheme** (Secure Boot architecture) — the "state" is which package set is active on the A or B partition
- **Version Store** — package version history maps to Merkle DAG versioning
- **Capability system** — mounted agent filesystem is constrained by capabilities granted at activation

Affected subsystems: Secure Boot, Agent lifecycle, Storage (Spaces), Service Manager.

### Design Considerations

- **Agent packages vs. system packages:** Haiku uses hpkg for everything (OS + apps). AIOS should separate system updates (A/B partitions) from agent packages (individually activatable).
- **Package format:** A custom format based on AIOS's Block Engine (content-addressed, encrypted, signed) rather than adopting hpkg directly.
- **Union mount implementation:** Kernel-level union mount (like Haiku) or userspace FUSE-like approach? Kernel-level is faster but more complex.
- **State snapshot storage:** How many snapshots to retain? Configurable retention policy (last N states, or time-based).
- **Development mode:** Developers need writable agent filesystems. A dev-mode flag that mounts from a directory instead of a sealed package.

### Open Questions → Recommendations

1. **Does this replace or complement the POSIX compatibility layer's approach to package management?**
   **Recommendation: Complement.** POSIX compat handles the *bridge* — making Linux/BSD packages installable (`apt install firefox` → unpacks into POSIX-mapped paths). Package-as-FS handles *native* AIOS agent packaging (sealed, content-addressed, capability-constrained). The POSIX bridge translates POSIX package paths into Space Storage queries. Native packages bypass POSIX entirely. Both coexist — different audiences, same underlying storage.

2. **How do agent data files interact with package rollback?**
   **Recommendation: Agent data is NEVER rolled back.** Agent code (in the sealed package) rolls back on version revert. Agent data (in the user's Space, under `/spaces/user/agents/{agent}/data/`) persists across all agent versions. Agent config has two tiers: version-specific config in the package, user overrides in the data Space. This matches mobile OS behavior. AIOS defaults to preserving data, with explicit `data_migration` hooks in the agent manifest for schema changes between versions.

3. **Should the package format be a new Block Engine object type?**
   **Recommendation: No — use existing Space objects.** Agent packages are content-addressed objects stored in Spaces (the deep dive proposes `AgentPackage` with an `ObjectId`). A package is just a large object with `ContentType::AgentPackage` and a signed manifest. The Version Store tracks package versions via the existing Merkle DAG. Adding a new Block Engine type would break the "everything is a Space object" invariant.

---

## Lesson 4: Content Type Registry

### Source: BeOS Registrar + MIME Database

BeOS's `registrar` server maintained a system-wide database mapping content types to handlers:

- **MIME type → preferred handler** (the default app for that type)
- **MIME type → supporting handlers** (other apps that can open it)
- **Sniffing rules** for type identification (byte patterns, magic numbers)
- **File extension → MIME type** mappings
- **Supertype handlers** (e.g., an app that handles all `audio/*` types)
- **Handler chain** with fallback logic

Applications registered their supported types at install time. The `roster` (application registry) tracked running applications and their capabilities. `BRoster::Launch()` could open a file with the preferred handler.

### AIOS Mapping

AIOS needs a **content type → agent handler registry** integrated with the Space Indexer:

- **Agent registration:** When an agent activates (see Lesson 3), it registers which content types it handles, with preferred/supporting distinction
- **"Open with" semantics:** User or AIRS selects a Space object → system resolves the preferred handler agent → launches/focuses it
- **AI-driven handler selection:** AIRS can learn user preferences ("Justin always opens .py files with the code editor, not the text viewer") and override default handlers contextually
- **Content type sniffing:** Space Indexer already extracts content for indexing — extend with MIME-type identification

Affected subsystems: Service Manager, Space Indexer, App Kit, AIRS (Preference service).

### Design Considerations

- **Registration mechanism:** Agents declare handled types in their manifest (package metadata from Lesson 3). Service Manager maintains the live registry.
- **Capability gating:** An agent can only register as handler for types it has capability to access.
- **Priority resolution:** Preferred > supporting > supertype handler > AIRS suggestion. User override always wins.
- **Dynamic vs. static:** BeOS was static (register at install). AIOS should support dynamic registration (agents can add/remove handled types at runtime, with capability checks).
- **Translation Kit interaction:** Content Type Registry knows what types exist. Translation Kit knows how to convert between types. Together: "convert this .docx to .pdf" → Translation Kit provides the converter, Content Type Registry resolves the output handler.

### Open Questions → Recommendations

1. **Should content type registration be part of App Kit or a separate registry service?**
   **Recommendation: Service Manager extension.** The registry is a system-level concern — it persists across agent lifecycles and is queried by multiple subsystems (AIRS, Flow, compositor). App Kit manages individual agent lifecycle (launch/quit/suspend/resume). The registry belongs with Service Manager because it already tracks registered services, content type → handler mapping is a routing table like service lookup, and multiple consumers need it. App Kit's role is to *declare* handled types in the manifest; Service Manager's role is to *store and resolve* the registry.

2. **How does this interact with the Scriptable Protocol (Lesson 1)?**
   **Recommendation: Yes — the registry itself implements `Scriptable`.** `GET PreferredHandler of ContentType "application/pdf"` → returns agent ID. `GET SupportedTypes of Agent "code-editor"` → returns MIME type list. `SET PreferredHandler of ContentType "text/plain" to "my-editor"` → user override. `COUNT Handler of ContentType "image/*"` → how many agents can open images. This is one of the strongest Lesson 1 + Lesson 4 synergies — the registry becomes introspectable by AIRS through standard verbs.

3. **Should the registry support wildcard patterns?**
   **Recommendation: Yes — supertypes only (`text/*`, `audio/*`), not arbitrary globs.** BeOS supported supertype handlers and it was useful — a "text viewer" that handles all `text/*` types. But arbitrary patterns like `application/vnd.aios.*` create ambiguity in resolution order. Rule: exact type > wildcard supertype. `text/markdown` preferred handler wins over `text/*` supertype handler. This is the same resolution chain BeOS used.

---

## Lesson 5: Decorator + ControlLook Separation

### Source: Haiku's Three-Layer Visual Architecture

Haiku separated visual rendering into three independent, swappable layers:

1. **Decorators** — control window chrome: title bar, borders, resize handles, close/minimize/zoom buttons. The `Decorator` class is a loadable add-on. Different decorators change the entire window appearance without affecting content.

2. **BControlLook** — renders widget primitives: buttons, scrollbars, checkboxes, text fields, tabs, menus. Also a swappable add-on. Changing BControlLook changes the look of all widgets system-wide without recompiling apps.

3. **Widget behavior** — the actual `BButton`, `BScrollBar`, `BCheckBox` classes handle input, state, and semantic behavior. Completely independent of how they're drawn.

This separation means:
- System-wide themes change Decorator + ControlLook, apps don't notice
- Accessibility needs (high contrast, large targets) swap ControlLook
- Custom window chrome (media player, game) replaces just the Decorator
- No app recompilation needed for any visual change

### AIOS Mapping

The Compositor and Interface Kit should adopt this three-layer separation:

- **Compositor `WindowChrome` trait** (= Decorator) — pluggable window frame rendering. Attention-aware: chrome can dim/highlight based on attention state.
- **Interface Kit `WidgetRenderer` trait** (= ControlLook) — pluggable widget painting. Context-adaptive: renderer can adjust for accessibility, time-of-day, or user preference.
- **Interface Kit `Widget` trait** (= behavior) — input handling and state, unchanged by visual changes.

Affected subsystems: Compositor, Interface Kit, Accessibility, Preference service.

### Design Considerations

- **Attention integration:** The Decorator equivalent should respond to attention signals — unfocused windows get subdued chrome, priority notifications get highlighted chrome.
- **Context-adaptive ControlLook:** Different rendering based on Context Engine state — high-contrast mode at night, larger touch targets on tablet form factor, simplified controls during focus mode.
- **Accessibility-first:** Swappable ControlLook is a natural home for accessibility adaptations — screen reader hints, increased contrast, motion reduction, target size enlargement.
- **Performance:** Pluggable rendering must not add indirection cost. Trait-based dispatch in Rust (static dispatch via generics where possible, dynamic dispatch only at the theme-swap boundary).

### Open Questions → Recommendations

1. **How many visual layers does AIOS need?**
   **Recommendation: Three (Chrome + Rendering + Behavior), with animation as a renderer property, not a fourth layer.** Haiku's ControlLook controlled both static painting and transitions. AIOS's `WidgetRenderer` should include animation parameters (transition duration, easing, motion reduction flag) as part of its rendering contract. A separate animation layer would over-engineer this — the renderer already knows "draw button in pressed state" and should also know "animate from unpressed to pressed over 150ms" as part of the same concern.

2. **Should ControlLook be per-agent or system-wide only?**
   **Recommendation: System-wide default, per-agent override with restrictions.** System-wide ControlLook ensures visual consistency (critical for accessibility). But specific agents should be able to override for legitimate reasons: games → custom rendering, media players → custom chrome, accessibility tools → enhanced rendering. Override requires a `CustomRendering` capability, and the compositor enforces that overridden rendering still meets accessibility minimums (contrast ratio, target sizes). Trust level affects this: TL1 (fully trusted) gets full override, TL3 (sandboxed) gets system-only.

3. **How does this interact with the three interaction layers?**
   **Recommendation: Each layer gets a different WidgetRenderer/WindowChrome pair.** Classic Desktop → traditional window chrome, standard widget rendering. Smart Desktop → simplified chrome (fewer buttons, more gesture-driven), card-based widgets. Intelligence Surface → minimal/no chrome, conversational widgets, AI-driven layout. The Context Engine signals which interaction mode is active → compositor swaps the active Chrome + Renderer pair. This is exactly the kind of runtime swapping Haiku's Decorator/ControlLook system was designed for.

---

## Lesson 6: Media Kit Node Graph with Latency Propagation

### Source: BeOS Media Kit Architecture

BeOS's Media Kit provided a real-time media processing framework built on a **node graph** abstraction:

- **Nodes** are processing units: producers (camera, microphone), consumers (speaker, display), or filters (codec, mixer, effects)
- **Connections** link an output of one node to an input of another
- **Format negotiation** is mandatory at connection time:
  1. Producer proposes a format via `FormatProposal()`
  2. Consumer accepts, rejects, or counter-proposes via `FormatChanged()`
  3. Both agree before data flows
- **Latency propagation:** Every node reports its processing latency via `FindLatencyFor()`. The system sums latencies along the path and uses this to schedule buffer delivery (producers start filling buffers early enough to meet the deadline).
- **Late notices:** If a buffer arrives after its deadline, the consumer sends a `LateNoticeReceived()` callback. The producer can then adjust (drop quality, skip frames).
- **Time source synchronization:** All nodes share a `BTimeSource` for synchronized playback.

This architecture was ahead of its time. PipeWire (2020s) reinvented much of it but separated format negotiation from connection, which is less clean.

### AIOS Mapping

The Media Pipeline and Audio subsystem architectures already describe codec frameworks and mixing graphs, but should explicitly adopt the node-graph-with-negotiation pattern:

- **Explicit format negotiation protocol** at connection time — not implicit "hope the formats match"
- **Latency budget propagation** through the graph — essential for real-time audio and video
- **Late-notice handling** for graceful degradation under load
- **Shared time source** for A/V sync

Additionally, the **AIRS inference pipeline** could benefit from the same pattern — inference has latency budgets too (time-to-first-token, streaming deadlines).

Affected subsystems: Media Kit, Audio Kit, Compositor (frame scheduling), AIRS (inference latency).

### Design Considerations

- **Rust trait design:** `MediaNode` trait with `fn propose_format()`, `fn accept_format()`, `fn report_latency()`, `fn late_notice()`. Connections are capability-gated IPC channels carrying typed buffers.
- **Zero-copy buffers:** Media data should flow through shared memory regions (already in AIOS design), with format negotiation ensuring both sides agree on buffer layout.
- **AIRS inference integration:** The inference pipeline is conceptually a media node graph: prompt → tokenize → inference → detokenize → stream. Latency budgets matter here too. Could the Compute Kit expose inference as a media node?
- **Dynamic graph modification:** BeOS required stopping connections to change format. AIOS should support live renegotiation (e.g., video resolution change during a call).

### Open Questions → Recommendations

1. **Should the Media Kit node graph be a general-purpose dataflow framework or media-specific?**
   **Recommendation: Media-specific, with a shared latency primitive.** A fully general dataflow framework is a massive abstraction — Rust's type system makes generic node graphs complex (trait object lifetime challenges, buffer ownership). Keep `MediaElement` media-specific. But extract latency budget propagation into a shared `LatencyAware` trait that both Media Kit and Compute Kit (AIRS inference) implement: `reported_latency()`, `set_latency_budget()`, `late_notice()`. The scheduling logic is shared; the domain-specific processing is not.

2. **How does format negotiation interact with the Translation Kit?**
   **Recommendation: Yes — auto-insert converter nodes.** This is the killer synergy. When format negotiation fails between two connected nodes, the pipeline queries the Translation Kit's conversion graph for a path. If one exists, auto-insert converter nodes. This is exactly how BeOS worked — the Media Kit would insert `BMediaCodec` nodes when formats didn't match. Implementation: `Pipeline::connect(source, sink)` → try `source.propose_format()` → `sink.accept_format()` → if rejected, query `TranslationKit::find_path(source_format, sink_format)` → insert intermediate nodes.

3. **Should inference latency budgets use the same time source as media playback?**
   **Recommendation: Same time source, different latency classes.** Both use the system-wide monotonic clock (ARM `CNTPCT_EL0`). But their budgets differ by orders of magnitude: audio 5-20ms (real-time), video 16-33ms (frame deadline), inference 100ms-2s (time-to-first-token). Using the same `LatencyAware` trait with the same clock lets the scheduler make cross-domain tradeoffs: "inference is using GPU bandwidth that audio needs → inference gets `late_notice()` → drops to smaller model."

---

## Lesson 7: URL Scheme Resource Model

### Source: Redox OS Schemes

Redox OS (a Rust-based microkernel) models **every system resource as a URL scheme**:

```
file:/path/to/file       → filesystem driver
tcp:1.2.3.4:80           → network stack
display:1                → compositor
audio:default             → audio server
log:                      → kernel log
disk:0                    → block device
```

All I/O operations (`open`, `read`, `write`, `close`, `dup`, `fpath`) go through scheme handlers — userspace services that register a scheme name and handle requests via IPC. The kernel's only job is routing operations to the correct scheme provider and managing the file descriptor table.

This creates a **uniform I/O model**: everything is a URL, every URL is handled by a scheme provider, every scheme provider is a userspace service. POSIX file operations naturally map to scheme lookups.

### AIOS Mapping

AIOS's POSIX compatibility layer could use a scheme-like routing model:

- `space:workspace/documents/report.pdf` → Space Storage service
- `flow:clipboard/current` → Flow service
- `ipc:channel/42` → IPC channel
- `device:uart/0` → device driver
- `airs:conversation/current` → Conversation Manager

This provides a **natural bridge between POSIX expectations and AIOS services**. A POSIX `open()` call with a scheme-prefixed path routes through capability-checked IPC to the appropriate service. Non-scheme paths (e.g., `/home/user/file.txt`) route through the POSIX compatibility layer's path translation.

Affected subsystems: POSIX compatibility, IPC Kit, Service Manager, all services that expose file-like interfaces.

### Design Considerations

- **Complement, not replace:** This doesn't replace AIOS's syscall dispatch or IPC channels. It's a thin routing layer that maps POSIX `open()` paths to the correct IPC destination. The real work still happens through capability-gated IPC.
- **Capability integration:** Scheme providers check the caller's capabilities before handling requests. `open("space:private/secrets/key")` fails without the appropriate Space capability.
- **Registration:** Services register their schemes with the Service Manager at startup. Dynamic scheme registration mirrors the content type registry (Lesson 4).
- **Discoverability:** `ls scheme:` could list all registered schemes — useful for debugging and AIRS introspection.
- **Performance:** Scheme lookup adds one IPC hop. For hot paths (frequent file operations), cache scheme→provider mappings.

### Open Questions → Recommendations

1. **Should schemes be POSIX-only or a first-class AIOS concept?**
   **Recommendation: First-class AIOS concept.** Scheme URLs provide a universal addressing model for both native agents and POSIX apps. Native agents use `open_resource("space:workspace/report.pdf")` → IPC to Space service. POSIX apps use `/scheme/space/workspace/report.pdf` → bridge translates → same IPC. Making schemes first-class means every AIOS resource has a canonical URL. This enables: AIRS can reference any resource by URL, Flow can carry scheme URLs as content references, Scriptable properties can contain scheme URLs as values.

2. **How do schemes interact with Space paths? Is `space:` a scheme?**
   **Recommendation: Yes — `space:` is the primary scheme.** Spaces are the canonical storage abstraction. `space:system/agents/installed/myagent` and `space:user/home/documents/report.pdf` are natural. Other schemes (`flow:`, `device:`, `airs:`, `surface:`) route to other services but share the same resolution mechanism. Bare paths (no scheme prefix) should default to `space:` — like how browsers assume `http://`. So `open_resource("workspace/report.pdf")` implicitly means `space:workspace/report.pdf`.

3. **Should scheme providers support `select()`/`poll()` for async I/O?**
   **Recommendation: Yes, via the existing IPC select mechanism.** AIOS Phase 3 already implemented `IpcSelect` — multi-wait on channels + notifications. Scheme-backed file descriptors participate in select by mapping to the underlying IPC channel to the scheme provider. The scheme provider sends a notification when data is available → POSIX bridge converts to `POLLIN` on the fd → `select()`/`poll()`/`epoll()` returns. This reuses existing infrastructure without adding a new mechanism.

---

## Interaction Map

These lessons reinforce each other in powerful ways:

```
                    ┌─────────────────────┐
                    │  1. Scriptable       │
                    │     Protocol         │
                    └──────┬──────────────┘
                           │ agents expose suites
                    ┌──────▼──────────────┐
                    │  4. Content Type     │◄──── Translation Kit converts
                    │     Registry        │       between registered types
                    └──────┬──────────────┘
                           │ "when new PDF arrives..."
                    ┌──────▼──────────────┐
                    │  2. Reactive         │◄──── Context Engine subscribes
                    │     Queries          │       for attention feeds
                    └──────┬──────────────┘
                           │ agents mounted from packages
                    ┌──────▼──────────────┐
                    │  3. Package-as-FS    │◄──── Secure Boot snapshots
                    │     + Rollback       │       for A/B updates
                    └─────────────────────┘

    ┌─────────────────────┐  ┌─────────────────────┐
    │  5. Decorator +      │  │  6. Media Node       │
    │     ControlLook      │  │     Graph + Latency   │
    └──────────────────────┘  └───────────────────────┘
     Compositor + Interface    Media Kit + Audio Kit
     Kit visual separation     + AIRS inference

                    ┌─────────────────────┐
                    │  7. URL Schemes      │
                    │     (Redox)          │
                    └─────────────────────┘
                     POSIX compat routing
                     to AIOS services
```

Key synergies:
- **Scriptable + Content Type:** AIRS discovers agent capabilities AND what content types they handle → `GET SupportedTypes of Agent "pdf-viewer"`
- **Reactive Queries + Content Type:** "When a new PDF appears in workspace/, route it to the preferred PDF handler agent" — fully declarative automation
- **Package-as-FS + Scriptable:** Installing an agent activates its package AND registers its scriptable interface. Uninstalling removes both atomically.
- **URL Schemes + Spaces:** `space:workspace/documents/` is a natural POSIX path for Space objects, routed via scheme lookup to the Space Storage service
- **Media Node Graph + AIRS Inference:** Both need latency budget propagation. A shared time-source and latency model could unify real-time media and AI inference scheduling.

---

## What BeOS/Haiku Got Wrong

These confirm AIOS's existing design choices:

| BeOS/Haiku Limitation | AIOS Already Addresses |
|---|---|
| Single-user model, no security | 8-layer capability-based security model |
| Drivers run in kernel space (crash = kernel panic) | DriverGrant isolation + crash recovery |
| Thread-per-window creates excessive context switching | IPC channels + async, not per-window threads |
| Tight filesystem-UI coupling (Tracker IS the filesystem) | Spaces separated from compositor |
| Native app ecosystem collapsed under weight of POSIX ports | Balance POSIX compat with native incentives |
| No memory protection between application add-ons | Capability-constrained agent isolation |
| BFS limited to single-disk, no versioning | Spaces: content-addressed, versioned, multi-device sync |
| No AI/ML integration (1990s design) | AIRS is a first-class kernel subsystem |

---

## Deep Dive: Integration with AIOS Architecture

After grounding each lesson in the actual AIOS architecture docs, here are concrete integration designs.

### Deep Dive 1: Scriptable Agent Protocol → Tool Manager Integration

**Existing substrate (from `tool-manager/registry.md` and `tool-manager/interop.md`):**

The Tool Manager already provides most of the infrastructure:

- `ToolId(provider: AgentId, name: ToolName)` — unique tool identifier
- `RegisteredTool` — full record with `ToolSchema` (JSON Schema), `capability_required`, `version` (SemVer), `latency_class`, `idempotent` flag
- `ToolRegistry` — central store with secondary indexes: `by_name`, `by_tag`, `by_provider`
- 7-stage execution pipeline: Call Initiation → Registry Lookup → Capability Validation (3 levels) → Schema Validation → IPC Dispatch → Provider Execution → Result Delivery
- Multi-runtime bridging: Rust, Python (RustPython), TypeScript (QuickJS-ng), WASM (wasmtime)
- MCP alignment: `name + description + inputSchema` is identical to MCP tool definitions

**Proposed layering — Scriptable as the primitive under Tool Manager:**

```
┌─────────────────────────────────────────────┐
│  MCP Bridge (external tool ecosystem)       │  ← Inbound/outbound MCP
├─────────────────────────────────────────────┤
│  Tool Manager (AI-optimized tool layer)     │  ← Descriptions, safety, timeouts
├─────────────────────────────────────────────┤
│  Scriptable Protocol (universal verbs)      │  ← GET/SET/CREATE/DELETE/COUNT/EXECUTE
├─────────────────────────────────────────────┤
│  IPC Kit (capability-gated channels)        │  ← <5µs round-trip
└─────────────────────────────────────────────┘
```

**Proposed `Scriptable` trait:**

```rust
pub trait Scriptable {
    /// Return the suite of properties this object exposes
    fn describe(&self) -> Suite;

    /// Execute a standard verb on a property
    fn execute_verb(&mut self, verb: VerbRequest, specifier: &Specifier) -> Result<Value, ScriptError>;
}

/// Payload-free verb kind — used in PropertyInfo for capability/allow-listing
pub enum VerbKind {
    Get,          // Read property value
    Set,          // Write property value
    Create,       // Add new entity
    Delete,       // Remove entity
    Count,        // Count entities in collection
    Execute,      // Invoke action
    Subscribe,    // Reactive query (Lesson 2 synergy)
    Describe,     // Return suite schema
}

/// Payload-carrying verb request — used when actually executing a verb
pub enum VerbRequest {
    Get,
    Set(Value),
    Create(Value),
    Delete,
    Count,
    Execute,
    Subscribe(ChannelId),
    Describe,
}

pub struct Suite {
    pub name: &'static str,
    pub properties: &'static [PropertyInfo],
}

pub struct PropertyInfo {
    pub name: &'static str,
    pub verbs: &'static [VerbKind],    // Which verbs are allowed (payload-free)
    pub value_type: ValueType,          // Expected type
    pub capability: Option<Capability>, // Required capability
}
```

**Key architectural decision:** The Scriptable trait is the *lower-level primitive*. Tool Manager wraps it with AI-friendly metadata (descriptions for LLM consumption, safety levels, timeout policies). MCP bridge wraps Tool Manager for external ecosystem compatibility. This means:

- Every tool is scriptable (GET/SET/CREATE/DELETE on its properties)
- Not every scriptable object is a "tool" (tools have extra metadata for AI)
- External MCP tools are bridged in through both layers

**Capability integration:** Hierarchical property access respects capabilities. `GET Password of Account "admin"` requires both `ChannelAccess` to the agent AND the specific `PropertyAccess(Account.Password)` capability. Capabilities attenuate as you traverse deeper into the hierarchy.

**Default suite (auto-generated for every agent):**

Every agent implementing `Scriptable` gets a baseline suite via derive macro. This is the minimum contract — agents extend with domain-specific suites on top.

```rust
/// Auto-generated by #[derive(Scriptable)] on every Agent struct
const AGENT_BASE_SUITE: Suite = Suite {
    name: "agent",
    properties: &[
        PropertyInfo {
            name: "Name",
            verbs: &[VerbKind::Get],
            value_type: ValueType::String,
            capability: None,  // Public — any caller with ChannelAccess
        },
        PropertyInfo {
            name: "State",
            verbs: &[VerbKind::Get],
            value_type: ValueType::Enum(&["Running", "Suspended", "Starting", "Stopping"]),
            capability: None,
        },
        PropertyInfo {
            name: "Version",
            verbs: &[VerbKind::Get],
            value_type: ValueType::String,  // SemVer
            capability: None,
        },
        PropertyInfo {
            name: "Capabilities",
            verbs: &[VerbKind::Get, VerbKind::Count],
            value_type: ValueType::List(ValueType::String),
            capability: Some(Capability::AgentIntrospect),
        },
        PropertyInfo {
            name: "SupportedTypes",
            verbs: &[VerbKind::Get, VerbKind::Count],
            value_type: ValueType::List(ValueType::String),  // MIME types
            capability: None,  // Public — enables Content Type Registry (Lesson 4)
        },
        PropertyInfo {
            name: "Suites",
            verbs: &[VerbKind::Get, VerbKind::Describe],
            value_type: ValueType::List(ValueType::Suite),
            capability: None,  // Suite discovery is always public
        },
    ],
};
```

**ValueType enum (for type-safe property values):**

```rust
pub enum ValueType {
    String,
    Integer,
    Float,
    Bool,
    Enum(&'static [&'static str]),
    List(Box<ValueType>),
    Map(Box<ValueType>, Box<ValueType>),
    Suite,                         // Nested suite reference
    SchemeUrl,                     // Lesson 7 synergy — typed resource reference
    ObjectId,                      // Space object reference
}

pub enum Value {
    String(heapless::String<256>),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Enum(u16),                     // Index into ValueType::Enum variants
    List(heapless::Vec<Value, 64>),
    Map(heapless::Vec<(Value, Value), 32>),
    Suite(Suite),
    SchemeUrl(heapless::String<256>),
    ObjectId(ObjectId),
    Null,
}
```

**Specifier addressing (BeOS-style hierarchical traversal):**

```rust
pub enum Specifier {
    /// Direct property name: "Name"
    Direct(&'static str),
    /// Index into collection: "Entry 0"
    Index(&'static str, usize),
    /// Named lookup: "Account 'admin'"
    Named(&'static str, heapless::String<64>),
    /// Range: "Entry 0 to 5"
    Range(&'static str, usize, usize),
    /// Chained traversal: "Password of Account 'admin'"
    Chain(Box<Specifier>, Box<Specifier>),
}
```

**How the derive macro works (pseudocode — actual macro generates `'static` data via `const` items):**

```rust
// Agent author writes:
#[derive(Scriptable)]
pub struct MyAgent {
    name: String,
    state: AgentState,
    // ...
}

// The derive macro generates const suites with 'static lifetime.
// Custom suites are declared as const items, not built dynamically:
impl MyAgent {
    const EDITOR_PROPS: &'static [PropertyInfo] = &[
        PropertyInfo {
            name: "Document",
            verbs: &[VerbKind::Get, VerbKind::Set, VerbKind::Create, VerbKind::Delete, VerbKind::Count],
            value_type: ValueType::ObjectId,
            capability: Some(Capability::SpaceAccess),
        },
        PropertyInfo {
            name: "Cursor",
            verbs: &[VerbKind::Get, VerbKind::Set],
            value_type: ValueType::Integer,
            capability: None,
        },
    ];
    const EDITOR_SUITE: Suite = Suite { name: "editor", properties: Self::EDITOR_PROPS };

    fn custom_suites(&self) -> &'static [Suite] {
        &[Self::EDITOR_SUITE]
    }
}

// AIRS can now: GET Document 0 of Agent "my-editor"
// Or:           SET Cursor of Document "report.pdf" of Agent "my-editor" to 42
// Or:           COUNT Document of Agent "my-editor"
```

### Deep Dive 2: Reactive Queries → Space Indexer Extension

**Existing substrate (from `space-indexer/search-integration.md` and `context-engine/consumers.md`):**

The Space Indexer currently provides:

- `SpaceQuery` enum: `TextSearch`, `Semantic`, `Traverse`, `Filter` (composition is via intersecting result sets, not a separate variant)
- `SearchResponse` with results, source metadata, and latency
- Score fusion via RRF (Reciprocal Rank Fusion)
- Graceful degradation (full-text always works; semantic degrades if AIRS unavailable)

The Context Engine already implements pub/sub:

- `StatePublisher` holds subscriber channel list
- Publishes `ContextUpdate` to all subscribers on state change
- 500ms coalescing window prevents rapid re-publication

**Proposed `QuerySubscription` extension to `QueryEngine`:**

```rust
pub trait QueryEngine {
    // Existing
    fn execute_query(&self, query: &SpaceQuery) -> Result<SearchResponse>;

    // NEW: Reactive query subscription
    fn subscribe(
        &self,
        query: &SpaceQuery,
        subscriber: ChannelId,
        mode: SubscriptionMode,
    ) -> Result<SubscriptionId>;

    fn unsubscribe(&self, sub_id: SubscriptionId) -> Result<()>;
}

pub enum SubscriptionMode {
    Immediate,                 // Push on every matching mutation
    Debounced(Duration),       // Coalesce within window (like Context Engine's 500ms)
    Digest(Duration),          // Batch and send every N seconds
}

pub enum QueryUpdate {
    Delta {
        added: Vec<SearchResult>,
        removed: Vec<ObjectId>,
        changed: Vec<(ObjectId, SearchResult)>,
    },
    Full(Vec<SearchResult>),   // On major index rebuild
    Invalidated { reason: String },  // Embedding model updated, etc.
}
```

**Predicate indexing for performance:** Naive O(predicates × mutations) is unacceptable. Group subscriptions by:

1. **Space** — most mutations are space-scoped; skip predicates for other spaces
2. **Content type** — if predicate filters on `ContentType::Document`, skip non-document mutations
3. **Attribute name** — if predicate filters on `modified_after`, only trigger on timestamp changes

**Integration with Context Engine:** The Context Engine subscribes to Space Indexer events via reactive queries (not polling). Example: `subscribe(SpaceQuery::Filter { content_type: Some(ContentType::Conversation), modified_after: Some(now() - Duration::from_secs(300)), ..Default::default() }, context_channel, SubscriptionMode::Debounced(Duration::from_millis(500)))` — Context Engine learns "user is actively conversing" from object mutation patterns.

**Integration with Attention Manager:** Reactive queries feed the attention system: "notify when any object in workspace/ has urgency > High" → Attention Manager receives `QueryUpdate::Delta` → creates `AttentionItem`.

**Predicate restrictions by query type:**

Not all `SpaceQuery` variants are efficient as reactive predicates. The cost model per mutation:

| SpaceQuery variant | Reactive cost | Allowed modes | Rationale |
|---|---|---|---|
| `Filter` | O(1) attribute check | Immediate, Debounced, Digest | Cheap — attribute comparison against changed object |
| `TextSearch` | O(terms) BM25 re-score | Debounced, Digest | Moderate — re-run inverted index lookup on changed object |
| `Semantic` | O(embed_dim) re-embed + ANN | Digest only | Expensive — requires re-embedding changed object, then approximate nearest neighbor search |
| `Traverse` | O(depth) graph walk | Debounced, Digest | Moderate — walk relationship graph from changed node |
| `Composed` | max(constituent costs) | Most restrictive constituent | Composite inherits the most expensive constituent's restrictions |

**Enforcement rules:**

```rust
pub fn validate_subscription(
    query: &SpaceQuery,
    mode: &SubscriptionMode,
) -> Result<(), SubscriptionError> {
    match (query, mode) {
        // Filter allows all modes
        (SpaceQuery::Filter { .. }, _) => Ok(()),

        // TextSearch disallows Immediate (too frequent for BM25)
        (SpaceQuery::TextSearch { .. }, SubscriptionMode::Immediate) =>
            Err(SubscriptionError::ModeTooExpensive {
                query_type: "TextSearch",
                suggested: SubscriptionMode::Debounced(Duration::from_millis(500)),
            }),

        // Semantic only allows Digest (re-embedding is expensive)
        (SpaceQuery::Semantic { .. }, SubscriptionMode::Digest(_)) => Ok(()),
        (SpaceQuery::Semantic { .. }, _) =>
            Err(SubscriptionError::ModeTooExpensive {
                query_type: "Semantic",
                suggested: SubscriptionMode::Digest(Duration::from_secs(30)),
            }),

        // Traverse and Composed: check constituents
        _ => Ok(()),  // Detailed validation for Composed
    }
}
```

**Version event subscription (opt-in):**

```rust
pub struct SubscriptionOptions {
    pub mode: SubscriptionMode,
    pub include_version_events: bool,  // Default: false
    pub max_batch_size: u16,           // Default: 100 (for Digest mode)
}
```

When `include_version_events` is true, the subscriber receives `QueryUpdate::Delta` entries for `EventType::VersionCreated` in addition to object mutations. This enables AIRS to detect edit velocity ("5 versions in 30 seconds → user is actively editing").

### Deep Dive 3: Package-as-Filesystem → Storage + Secure Boot Integration

**Existing substrate (from `secure-boot/updates.md` and `spaces/versioning.md`):**

The A/B update scheme:
- ESP (FAT32) with `.prev` files for known-good rollback
- Atomic staging: write `.new` → validate signature → rename
- Anti-rollback via monotonic counters
- Three update channels: system (kernel/platform), agent (individual agents), model (AI models)

The Version Store:
- Merkle DAG with `Version` nodes: `id, parent, content_hash, object_id, timestamp, author, message`
- Rollback = revert to parent version (pointer update, no data copy)
- Content-addressed: identical content shares storage

**Proposed package model — packages as versioned Space objects:**

```rust
pub struct AgentPackage {
    pub object_id: ObjectId,           // Content-addressed in Space
    pub manifest: AgentManifest,       // Signed declaration
    pub content_hash: ContentHash,     // SHA-256 of package contents
    pub version: Version,              // Merkle DAG node
    pub activation_state: ActivationState,
}

pub enum ActivationState {
    Available,       // Downloaded, not mounted
    Active,          // Mounted read-only, agent running
    Suspended,       // Mounted but agent not running
    Deactivated,     // Unmounted, retained for rollback
}
```

**How it works with A/B updates:**
- **System packages** (kernel, platform services) use A/B partitions — atomic swap at boot
- **Agent packages** use individual activation — mount/unmount without reboot
- **Model packages** use the model update channel — AIRS manages model lifecycle independently
- Each activation state change creates a Version Store snapshot → rollback = activate previous version

**POSIX path mapping (from `posix.md §6`):**

The POSIX bridge already translates `/spaces/system/agents/installed/{bundle_id}/` into Space Service queries. Package-as-Filesystem adds:
- `/spaces/system/agents/installed/myagent@1.0.0/manifest.json` → read-only from sealed package
- `/spaces/user/agents/myagent/data/` → writable overlay for agent's user data
- Agent data is **NOT rolled back** when agent code is rolled back — user data persists across agent versions

**Mount mechanism — how does read-only access actually work?**

Three options, with different trade-offs:

| Option | Approach | Pro | Con | When |
|---|---|---|---|---|
| **A. Kernel union mount** | Like Haiku's packagefs — kernel module overlays sealed packages | Fastest (no IPC for file access) | Requires kernel filesystem support AIOS doesn't have yet | Post-Phase 22 |
| **B. Space service read-only view** | Space Storage presents package contents as read-only Space objects; agents access via normal IPC | Reuses existing infrastructure; no new kernel code | One IPC hop per file access | Phase 10+ (Service Manager) |
| **C. FUSE-like userspace mount** | Userspace daemon presents package as POSIX directory | POSIX apps get transparent access | Requires POSIX compat layer; extra process | Phase 22+ (POSIX compat) |

**Recommendation: Option B for native AIOS agents, Option C deferred for POSIX apps.**

Option B works within the existing architecture:

```rust
/// Space Storage service handles package content requests
pub enum PackageAccess {
    /// Read file from sealed package (immutable)
    ReadPackageFile {
        package_id: ObjectId,
        relative_path: heapless::String<256>,
    },
    /// List directory within package
    ListPackageDir {
        package_id: ObjectId,
        relative_path: heapless::String<256>,
    },
}

/// Agent manifest declares package layout
pub struct AgentManifest {
    pub agent_id: AgentId,
    pub version: SemVer,
    pub content_hash: ContentHash,
    pub signature: Signature,
    pub handled_content_types: Vec<(MimeType, HandlerRole)>,  // Lesson 4 synergy
    pub requested_capabilities: Vec<Capability>,
    pub scriptable_suites: Vec<SuiteName>,                    // Lesson 1 synergy
    pub data_migration: Option<MigrationScript>,              // Schema changes between versions
    pub package_layout: PackageLayout,
}

pub struct PackageLayout {
    pub binary: &'static str,          // "bin/myagent"
    pub resources: &'static str,       // "res/"
    pub config_defaults: &'static str, // "config/defaults.toml"
}
```

**Activation sequence:**

1. Space Storage receives `ActivatePackage(object_id)` via IPC
2. Validates `content_hash` against stored SHA-256
3. Verifies `signature` against agent publisher's key (from Identity subsystem)
4. Sets `activation_state = Active`
5. Registers Scriptable suites with Tool Manager (Lesson 1)
6. Registers content types with Content Type Registry (Lesson 4)
7. Creates Version Store snapshot for rollback
8. Service Manager starts agent process with capability-constrained IPC channels

**Deactivation is the reverse** — steps 6→5→4 in reverse order, atomically. Agent data in `/spaces/user/agents/{agent}/data/` is untouched.

### Deep Dive 4: Content Type Registry → Service Manager + Tool Manager

**Existing substrate (from `tool-manager/registry.md` and Flow's `TypedContent`):**

- Tools already declare `inputSchema` (JSON Schema) — but not content types they handle
- Flow has `TypedContent` abstraction mapping MIME types to transforms
- Space objects have implicit `content_type` in `CompactObject` metadata
- Agent manifests declare `requested_capabilities` — but not handled content types

**Proposed `ContentTypeRegistry` as Service Manager extension:**

```rust
pub struct ContentTypeRegistry {
    /// MIME type → ordered handler list
    handlers: BTreeMap<MimeType, Vec<HandlerEntry>>,
    /// Extension → MIME type mapping
    extensions: HashMap<String, MimeType>,
    /// Sniffing rules for type identification
    sniffers: Vec<SniffRule>,
}

pub struct HandlerEntry {
    pub agent_id: AgentId,
    pub role: HandlerRole,             // Preferred, Supporting, Supertype
    pub capability_required: Capability,
    pub registered_at: Timestamp,
}

pub enum HandlerRole {
    Preferred,    // Default handler (only one per type)
    Supporting,   // "Open with" alternative
    Supertype,    // Handles all audio/*, text/*, etc.
}
```

**Registration flow:**
1. Agent package activates (Lesson 3) → Service Manager reads manifest
2. Manifest declares `handled_content_types: Vec<(MimeType, HandlerRole)>`
3. Service Manager registers entries in `ContentTypeRegistry`
4. On deactivation → entries removed atomically

**Resolution chain:** User preference (Preference service) > Preferred handler > Supporting handler > Supertype handler > AIRS suggestion > Translation Kit auto-conversion.

**Scriptable integration (Lesson 1 synergy):** `GET SupportedTypes of Agent "code-editor"` returns the agent's registered content types. `GET PreferredHandler of ContentType "application/pdf"` returns the preferred agent. Both are standard verbs on the registry's Scriptable interface.

**ADR dependency chain and manifest extension:**

The Content Type Registry depends on three other lessons being formalized:

```
ADR: Scriptable Protocol (Lesson 1)
  ↓ registry implements Scriptable trait
ADR: Package-as-FS (Lesson 3)
  ↓ manifest declares handled_content_types
ADR: Content Type Registry (Lesson 4)
  ↓ resolves handlers for "open with"
  → consumed by: AIRS, Flow Kit, Experience layer
```

**Manifest extension** (extends `AgentManifest` from Lesson 3):

```rust
/// Added to AgentManifest for Content Type Registry integration
pub struct ContentTypeDeclaration {
    pub mime_type: MimeType,
    pub role: HandlerRole,
    /// Sniffing rule for ambiguous files (optional)
    pub sniff_rule: Option<SniffRule>,
    /// Verbs this agent supports for this content type
    pub supported_verbs: Vec<ContentVerb>,
}

pub enum ContentVerb {
    View,       // Read-only display
    Edit,       // Read-write editing
    Convert,    // Translation Kit integration — can convert FROM this type
    Preview,    // Thumbnail/summary generation
    Index,      // Can extract searchable metadata (Space Indexer integration)
}

pub struct SniffRule {
    pub offset: usize,          // Byte offset to check
    pub pattern: Vec<u8>,       // Magic bytes
    pub mask: Option<Vec<u8>>,  // Bit mask for partial match
    pub priority: u8,           // Higher = checked first (0-255)
}
```

**AI-driven handler learning (Preference service integration):**

The resolution chain includes AIRS suggestion as a fallback. Over time, AIRS observes handler choices:

1. User opens `report.pdf` → system offers preferred handler (PDF Viewer) + supporting (Code Editor)
2. User chooses Code Editor 5 times in a row for `.pdf` files
3. Preference service records: `(user=justin, type=application/pdf, preferred=code-editor, confidence=0.85)`
4. Next time: Code Editor becomes the *contextual* preferred handler (overriding the manifest default)
5. Context-dependent: same user might prefer PDF Viewer when in "reading" context but Code Editor in "reviewing code" context — Context Engine state affects resolution

This is strictly better than BeOS's static MIME database, which had no learning capability.

### Deep Dive 5: Decorator + ControlLook → Compositor Scene Graph

**Existing substrate (from `compositor/rendering.md` and `compositor/security.md`):**

The compositor already separates these concerns, though not as explicitly as Haiku:

- **Scene graph** uses `SceneNode` enum: `Surface`, `Group`, `Effect`, `Clip`
- `Effect` nodes (Shadow, RoundedCorners, Blur, ColorTransform) wrap surfaces — this IS the Decorator pattern
- `SurfaceContentType` (Document, Terminal, Conversation, Game, etc.) provides semantic hints
- `InteractionState` tracks urgency, focus, fullscreen — drives chrome behavior
- Trust levels affect chrome: TL3 agents get simplified chrome, TL1 gets full control

**Proposed formalization — three explicit trait layers:**

```rust
/// Layer 1: Window chrome (= Haiku Decorator)
pub trait WindowChrome: Send {
    fn render_frame(&self, surface: &Surface, state: &InteractionState) -> Vec<SceneNode>;
    fn hit_test(&self, point: Point) -> ChromeHitResult;  // Close, minimize, resize, etc.
    fn attention_response(&self, urgency: Urgency) -> ChromeAnimation;
}

/// Layer 2: Widget rendering (= Haiku BControlLook)
pub trait WidgetRenderer: Send {
    fn draw_button(&self, state: &ButtonState, bounds: Rect) -> DisplayList;
    fn draw_scrollbar(&self, state: &ScrollState, bounds: Rect) -> DisplayList;
    fn draw_text_field(&self, state: &TextFieldState, bounds: Rect) -> DisplayList;
    // ... one method per widget type
    fn accessibility_adjustments(&self) -> AccessibilityProfile;
}

/// Layer 3: Widget behavior (= Haiku BButton/BScrollBar)
pub trait Widget: Send {
    fn handle_input(&mut self, event: InputEvent) -> WidgetResponse;
    fn state(&self) -> &dyn WidgetState;
    fn accessibility_node(&self) -> AccessNode;
    // Behavior is INDEPENDENT of rendering
}
```

**Context-adaptive rendering:** The compositor selects `WidgetRenderer` based on Context Engine state:
- Night mode → high-contrast renderer with warmer colors
- Tablet form factor → larger touch targets, simplified controls
- Focus mode → minimal chrome, reduced animation
- Accessibility → screen-reader-optimized renderer with enhanced contrast

**Attention-aware chrome:** `WindowChrome::attention_response()` receives urgency level and returns an animation:
- `Urgency::Low` → subtle pulse on window border
- `Urgency::High` → chrome highlight + badge
- `Urgency::Critical` → full chrome animation + sound

### Deep Dive 6: Media Node Graph → Pipeline + Inference Latency

**Existing substrate (from `media-pipeline/playback.md` and `audio/subsystem.md`):**

The media pipeline already has:
- `MediaElement` trait with `process() → ProcessResult` (NeedInput, HaveOutput, Eos, Error)
- Element graph: Source → Demuxer → Decoder → Filter → Sink
- `MediaClock` with audio-master timing model
- Pull-based scheduling (sink-driven, natural backpressure)

**Missing:** No latency measurement, no latency propagation, no late-notice handling.

**Proposed extension — add latency awareness to `MediaElement`:**

```rust
pub trait MediaElement: Send {
    fn process(&mut self) -> Result<ProcessResult, MediaError>;

    // NEW: Latency reporting
    fn reported_latency(&self) -> Duration;
    fn set_latency_budget(&mut self, budget: Duration);
    fn late_notice(&mut self, how_late: Duration);

    // NEW: Format negotiation (BeOS-style)
    fn propose_format(&self, pad: PadId) -> Vec<MediaFormat>;
    fn accept_format(&mut self, pad: PadId, format: &MediaFormat) -> FormatResult;
}

pub struct ProcessResult {
    pub status: ProcessStatus,
    pub processing_time: Duration,     // NEW: measured time
    pub queue_depth: u16,              // NEW: input queue fullness
}

pub enum FormatResult {
    Accepted,
    CounterProposal(MediaFormat),
    Rejected { reason: String },
}
```

**Latency budget propagation through the graph:**
1. Sink knows its deadline (next frame PTS from `MediaClock`)
2. Sink asks upstream filter: "you have 8ms to process"
3. Filter subtracts its own latency (2ms) → asks upstream decoder: "you have 6ms"
4. Decoder subtracts its latency (4ms) → asks upstream demuxer: "you have 2ms"
5. If any element can't meet budget → `late_notice()` propagates downstream → quality adaptation

**AIRS inference as a media node graph:** The inference pipeline maps naturally:
```
PromptSource → Tokenizer → InferenceEngine → Detokenizer → StreamSink
                              ↑
                    (latency budget from StreamSink:
                     "time-to-first-token < 200ms")
```
The Compute Kit's inference pipeline could implement `MediaElement`, sharing the latency propagation model. AIRS sets the latency budget ("respond within 200ms for interactive, 2s for batch").

**Translation Kit auto-insertion (Lesson 4 synergy):** When format negotiation fails (producer offers H.264, consumer needs VP9), the pipeline auto-inserts a Translation Kit converter node: `H264Decoder → TranscodeFilter(H264→VP9) → VP9Encoder`. The Translation Kit's conversion graph finds the shortest path.

### Deep Dive 7: URL Schemes → POSIX Bridge + Service Manager

**Existing substrate (from `posix.md` and Space Storage):**

The POSIX bridge already implements path → Space routing:
- `FdKind` enum: `SpaceObject`, `Directory`, `Pipe`, `Socket`, `Device`, `Terminal`, `ProcSelf`
- Path resolution: `/spaces/{space_name}/{object_path}` → Space Service IPC
- Device access: `/dev/{subsystem}/{device}` → Subsystem IPC
- Capability-gated: every `open()` checks caller's capabilities

**Proposed scheme registry — extend Service Manager:**

```rust
pub struct SchemeRegistry {
    schemes: BTreeMap<String, SchemeProvider>,
}

pub struct SchemeProvider {
    pub name: String,                   // "space", "flow", "device", etc.
    pub service_channel: ChannelId,     // IPC endpoint
    pub capabilities_required: Vec<Capability>,
    pub description: String,
}

// Scheme URL format: {scheme}:{path}
// Examples:
//   space:workspace/documents/report.pdf
//   flow:clipboard/current
//   device:display/hdmi-1
//   airs:conversation/current
//   version:myobject/v3
```

**Resolution flow for `open("space:workspace/report.pdf")`:**
1. POSIX bridge parses scheme prefix `space:`
2. Looks up `space` in `SchemeRegistry` → finds Space Storage service channel
3. Checks caller has required capabilities for Space access
4. Forwards `open` request via IPC to Space Storage service
5. Space Storage returns shared memory handle → POSIX bridge creates `FdEntry::SpaceObject`

**First-class vs. POSIX-only:** Recommended: **first-class AIOS concept**, not just POSIX compat. Native AIOS agents use scheme URLs directly in IPC messages (not through POSIX `open()`). POSIX apps use `/scheme/{scheme_name}/{path}` paths which the bridge translates. Both routes converge on the same Service Manager lookup.

**Coherent namespace (all AIOS resources addressable):**
```
space:system/agents/installed/myagent@1.0.0     → agent package
space:user/home/documents/report.pdf            → user document
flow:clipboard/current                           → current clipboard
device:display/hdmi-1                            → display output
device:compute/gpu-0                             → GPU accelerator
airs:conversation/current                        → active conversation
version:report.pdf/v3                            → specific version
surface:12345                                    → compositor surface
input:keyboard/0                                 → input device
```

---

## Graduation Candidates

### Tier 1: Ready for ADR extraction

These lessons have all open questions resolved, concrete trait designs, and clear integration points.

**1. Scriptable Agent Protocol** — All gaps closed. Mandatory trait with derive macro, default suite (6 auto-properties: Name, State, Version, Capabilities, SupportedTypes, Suites), `ValueType`/`Value` enums, `Specifier` addressing, capability conjunction chain. Layering: IPC → Scriptable → Tool Manager → MCP. Affects: App Kit, Tool Manager, AIRS, Service Manager.

**2. Reactive Queries on Spaces** — All gaps closed. `QueryEngine` trait extension with `subscribe()`/`unsubscribe()`, three `SubscriptionMode` variants, `QueryUpdate` delta/full/invalidated. Predicate restrictions by query type (Filter=all modes, TextSearch=debounced+, Semantic=digest only). Version events opt-in. Limits: 32/agent, 1024/system. Affects: Space Indexer, Context Engine, Attention Manager.

### Tier 2: Nearly ready — one design decision resolved

These had one remaining gap that has now been addressed in the deep dives.

**3. Package-as-Filesystem** — Mount mechanism resolved: Option B (Space service read-only view) for native agents, Option C (FUSE-like) deferred to POSIX compat. Full activation sequence designed (8 steps), `AgentManifest` struct with cross-lesson synergies, data persistence rule ("agent data is never rolled back"). Affects: Secure Boot, Storage, Service Manager.

**4. Content Type Registry** — ADR dependency chain resolved: depends on Scriptable Protocol (Lesson 1) and Package-as-FS (Lesson 3). Own ADR (not merged into Service Manager) due to cross-cutting scope. `ContentTypeDeclaration` with `ContentVerb` enum, `SniffRule` struct, AI-driven handler learning via Preference service. Affects: Service Manager, Space Indexer, AIRS, Flow Kit.

### Tier 3: Deferred — principle captured, implementation later

These are well-understood but depend on subsystems not yet built. Capture the design principle now; refine trait signatures during implementation phases.

**5. Decorator/ControlLook** — Deferred to Compositor (Phase 6) + Interface Kit (Phase 29). **Capture now:** Three swappable layers (Chrome, Rendering, Behavior). Context Engine triggers renderer swap. Animation is a renderer property, not a fourth layer. System-wide default + capability-gated per-agent override.

**6. Media Node Graph + LatencyAware** — Deferred to Media Pipeline (Phase 20), **except** `LatencyAware` trait. **Extract now as standalone micro-ADR:** `LatencyAware` (3 methods: `reported_latency`, `set_latency_budget`, `late_notice`) affects Compute Kit (Phase 5) and AIRS inference (Phase 12) before the full Media Kit lands. Defer `MediaElement` format negotiation to Phase 20.

**7. URL Schemes** — Deferred to POSIX compat (Phase 22). **Capture now as design principle ADR:** Schemes are first-class (not POSIX-only). `space:` is the primary scheme. `SchemeRegistry` lives in Service Manager. Bare paths default to `space:`. Implementation deferred, but Service Manager (Phase 10) should reserve the registry pattern.

### ADR extraction order

```
1. ADR: Scriptable Agent Protocol          ← extract first (no dependencies)
2. ADR: Reactive Queries on Spaces         ← extract second (no dependencies)
3. ADR: LatencyAware Trait (micro-ADR)     ← small, affects Compute Kit design
4. ADR: Package-as-Filesystem              ← depends on nothing, but enables 5
5. ADR: Content Type Registry              ← depends on 1 + 4
6. ADR: URL Scheme Resource Model (principle) ← design principle only
7. ADR: Visual Layer Separation (principle)   ← design principle only
```

---

## Sources

- [Haiku Scripting with Hey](https://www.haiku-os.org/blog/humdinger/2017-11-05_scripting_the_gui_with_hey/)
- [Programming with Haiku: Application Scripting (Lessons 18–19)](https://www.haiku-os.org/development/programming_with_haiku)
- [Haiku Node Monitoring](https://www.haiku-os.org/documents/dev/node_monitoring)
- [Programming with Haiku: Queries (Lesson 13)](https://www.haiku-os.org/development/programming_with_haiku)
- [Haiku Package Management Infrastructure](https://www.haiku-os.org/docs/develop/packages/Infrastructure.html)
- [Haiku Package Management Blog](https://www.markround.com/blog/2023/02/13/haiku-package-management/)
- [Haiku Launch Daemon](https://www.haiku-os.org/blog/axeld/2015-07-17_introducing_launch_daemon/)
- [Haiku Replicants](https://www.haiku-os.org/documents/dev/replicants_more_application_than_an_application/)
- [BeOS Media Kit Overview](https://www.haiku-os.org/legacy-docs/bebook/TheMediaKit_Overview_Introduction.html)
- [BeOS Media Kit Plugin Review](http://plugin.org.uk/GMPI/beos-review.html)
- [Haiku Layout API](https://www.haiku-os.org/docs/api/layout_intro.html)
- [Haiku Decorator Documentation](https://www.haiku-os.org/docs/develop/servers/app_server/Decorator.html)
- [Haiku Registrar Protocols](https://www.haiku-os.org/docs/develop/servers/registrar/Protocols.html)
- [Redox OS Book: Scheme Operations](https://doc.redox-os.org/book/scheme-operation.html)
- [Redox OS: Why Rust](https://doc.redox-os.org/book/why-rust.html)
- [Making the Case for BeOS Pervasive Multithreading (OSNews)](https://www.osnews.com/story/180/making-the-case-for-beoss-pervasive-multithreading/)
- [Pervasive Multithreading (LWN)](https://lwn.net/Articles/495229/)
