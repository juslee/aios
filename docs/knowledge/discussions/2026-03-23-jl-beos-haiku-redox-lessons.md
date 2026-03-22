---
author: Justin Lee
date: 2026-03-23
tags: [kits, platform, intelligence, storage, compositor, security, posix]
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

### Open Questions

1. Should the Scriptable trait be mandatory for all agents, or opt-in?
2. How does capability attenuation work with hierarchical property access?
3. Does AIRS use the scriptable protocol for ALL agent interaction, or only for discovery/composition?

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

### Open Questions

1. Should reactive queries operate at the Space Indexer level (semantic) or Block Engine level (structural)?
2. How do reactive queries interact with the Version Store? (notify on version creation?)
3. Maximum concurrent reactive queries per agent? System-wide?

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

### Open Questions

1. Does this replace or complement the POSIX compatibility layer's approach to package management?
2. How do agent data files (user-generated content within an agent's Space) interact with package rollback?
3. Should the package format be a new Block Engine object type?

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

### Open Questions

1. Should content type registration be part of the App Kit (agent lifecycle) or a separate registry service?
2. How does this interact with the Scriptable Protocol (Lesson 1)? Does `GET SupportedTypes of Agent "code-editor"` work?
3. Should the registry support wildcard patterns (e.g., `text/*`, `application/vnd.aios.*`)?

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

### Open Questions

1. How many visual layers does AIOS need? BeOS/Haiku had 2 (Decorator + ControlLook). Should AIOS add a third for animation/transition behavior?
2. Should ControlLook be per-agent (agents can customize widget appearance) or system-wide only?
3. How does this interact with the three interaction layers (Classic Desktop → Smart Desktop → Intelligence Surface)?

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

### Open Questions

1. Should the Media Kit node graph be a general-purpose dataflow framework (usable by other subsystems) or media-specific?
2. How does format negotiation interact with the Translation Kit? (Auto-insert converter nodes when formats don't match?)
3. Should inference latency budgets use the same time source as media playback?

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

### Open Questions

1. Should schemes be a POSIX compatibility feature only, or a first-class AIOS concept?
2. How do schemes interact with Space paths? Is `space:` a scheme, or do Spaces have their own path namespace?
3. Should scheme providers support `select()`/`poll()` for async I/O?

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

## Graduation Candidates

When ready to extract ADRs from this discussion:

1. **Scriptable Agent Protocol** — strong candidate, affects core AIRS design
2. **Reactive Queries** — strong candidate, clear Space Indexer evolution
3. **Package-as-Filesystem** — needs more design work (interaction with A/B updates)
4. **Content Type Registry** — could merge with Service Manager ADR
5. **Decorator/ControlLook** — deferred until Compositor/Interface Kit phases
6. **Media Node Graph** — deferred until Media Kit phase
7. **URL Schemes** — deferred until POSIX compat phase

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
