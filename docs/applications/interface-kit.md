# AIOS Interface Kit

**Parent document:** [architecture.md](../project/architecture.md)
**Kit overview:** [Interface Kit](../kits/application/interface.md) — AIOS-native UI foundation (Rust traits for widgets, layout, events, theming). See [ADR: Kit Architecture](../knowledge/decisions/2026-03-22-jl-kit-architecture.md).
**Related:** [compositor.md](../platform/compositor.md) — Compositor protocol (system service, not a Kit), [experience.md](../experience/experience.md) — Experience layer surfaces, [agents.md](./agents.md) — Agent SDK, [flow.md](../storage/flow.md) — Flow drag/drop integration, [context-engine.md](../intelligence/context-engine.md) — Context-aware adaptation

-----

## 1. Overview

A new operating system with no applications is dead on arrival. Developers won't invest in building for a platform with zero users, and users won't adopt a platform with no software. This is the cold-start problem that has killed every desktop OS challenger since Windows and macOS locked in their positions.

AIOS breaks this deadlock with **Interface Kit** — the AIOS-native UI foundation. Interface Kit defines Rust traits for widgets, layout, events, and theming. These traits are the source of truth for AIOS's UI system. The same application code runs on Linux, macOS, Web, and AIOS. Developers build and test on their current platform — macOS with Xcode, Linux with their favorite editor — and deploy to AIOS without modification. When running on AIOS, applications gain capabilities that don't exist elsewhere (semantic window hints, Flow integration, space-backed persistence, capability-aware UI). On other platforms, these features gracefully degrade to standard behavior.

**Why this matters:**

1. **Developer adoption.** Build and test on a familiar platform. No AIOS boot required for development. The edit-compile-test loop stays fast.
2. **Ecosystem bootstrapping.** Developers invest knowing their work isn't trapped on a zero-user platform. An AIOS agent is also a Linux application and a macOS application.
3. **Proving abstractions.** Multi-platform support proves the Kit design isn't accidentally coupled to kernel internals. If it works on Linux and macOS, the abstractions are clean.
4. **Fast iteration.** Edit on Mac, test in QEMU, deploy to hardware. No context switching between toolchains.

**Bridge architecture:** Open-source UI toolkits sit **above** Interface Kit as bridges, translating their widget/rendering model to Kit primitives. The **default bridge is iced** — Elm-inspired, pure Rust, MIT-licensed, GPU-rendered via wgpu. Already works on Linux/macOS/Windows/Web. Other bridges (Flutter, Qt, GTK, Electron) can be added to give ported apps access to AIOS features through familiar interfaces. AIOS-native apps use Interface Kit traits directly. The Elm Architecture (Model-View-Update) maps cleanly to the agent model: each agent is an `Application` with its own state, message loop, and view function.

### 1.1 Comparison with Other Toolkits

| Property | Interface Kit | SwiftUI | Jetpack Compose | Flutter |
|---|---|---|---|---|
| Language | Rust | Swift | Kotlin | Dart |
| Architecture | Elm (Model-View-Update) | Declarative (`@State`) | Recomposition (`@Composable`) | Widget/Element/RenderObject |
| Rendering | wgpu + Vello (future) | Metal/Core Animation | Skia/Impeller (planned) | Impeller/Skia |
| Cross-platform | AIOS/Linux/macOS/Web | Apple platforms only | Android + desktop (experimental) | All platforms |
| `no_std` kernel path | Yes (Kit traits) | No | No | No |
| Accessibility | AccessKit-inspired tree | NSAccessibility | Android Semantics | Semantics widget |
| i18n | ICU4X (zero-copy, `no_std`) | Foundation.framework | ICU4C via Android | ICU4C via Dart |
| State isolation | Per-agent (TTBR0 boundary) | Per-view hierarchy | Per-composition | Per-widget tree |
| OS integration | Capability-aware, Flow, Spaces | iCloud, Shortcuts | Intents, Content Providers | Platform channels |
| Bridge ecosystem | iced (default) + Flutter/Qt/GTK | — | — | — |

-----

## 2. Architecture

```text
┌─────────────────────────────────────────────────────────────────┐
│                     Agent Application Code                       │
│            (identical across all platforms)                       │
│                                                                  │
│   struct App { state }                                           │
│   fn update(&mut self, msg) → InterfaceCommand                   │
│   fn view(&self) → Element                                       │
│   fn subscription(&self) → Subscription                          │
└──────────────────────────┬──────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│               Interface Kit — AIOS-Native Foundation              │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────┐ │
│  │ Widget Library│  │ Layout Engine │  │ Theme System          │ │
│  │ button, text, │  │ flexbox-like  │  │ colors, fonts,        │ │
│  │ input, list,  │  │ constraints   │  │ spacing, context-     │ │
│  │ scroll, image │  │ propagation   │  │ aware adaptation      │ │
│  └──────────────┘  └──────────────┘  └───────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────┐ │
│  │ Event Model   │  │ Render Tree   │  │ Text Layout           │ │
│  │ click, hover, │  │ diff, damage  │  │ shaping, line break,  │ │
│  │ focus, keybd  │  │ display list  │  │ bidi, font fallback   │ │
│  └──────────────┘  └──────────────┘  └───────────────────────┘ │
└──────────────────────────┬──────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│           Platform Backend + Bridge Layer (one per target)        │
│                                                                  │
│  ┌─────────────────┐  ┌──────────────────┐  ┌────────────────┐ │
│  │ AIOS (native)    │  │ Linux (iced       │  │ macOS (iced    │ │
│  │ Compute Kit T1   │  │   bridge)         │  │   bridge)      │ │
│  │ + Compositor IPC  │  │ wgpu + winit      │  │ wgpu + winit   │ │
│  │ + semantic hints  │  │ (Wayland/X11)     │  │ (Metal)        │ │
│  │ + Flow Kit        │  │                   │  │                │ │
│  └─────────────────┘  └──────────────────┘  └────────────────┘ │
│                                                                  │
│  ┌─────────────────┐  ┌──────────────────────────────────────┐ │
│  │ Web (iced bridge)│  │ Other bridges: Flutter, Qt, GTK,     │ │
│  │ Canvas + DOM     │  │ Electron — translate their APIs to   │ │
│  │ (WASM target)    │  │ Interface Kit primitives              │ │
│  └─────────────────┘  └──────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

The architecture enforces a strict separation between portable logic and platform-specific code. The portable core contains:

- **Widget library** — all UI elements, their state, their event handling
- **Layout engine** — constraint-based positioning, sizing, alignment (with dirty flag propagation and constraint caching inspired by Chrome LayoutNG)
- **Theme system** — color, typography, spacing tokens with context-aware adaptation
- **Event model** — input event routing, focus management, gesture recognition
- **Render tree** — diffing, damage tracking, display list generation, animation scheduling
- **Text layout** — shaping (swash), line breaking, bidirectional text (ICU4X), font fallback (fontique/Parley)

The platform backend implements the `InterfaceBackend` trait (see [backends.md](./interface-kit/backends.md)) that the portable core calls.

**Rendering pipeline (layered):**

| GPU capability | Renderer | Targets |
|---|---|---|
| Compute shaders (Vulkan/Metal) | Vello (GPU vector rendering) | V3D (Pi 5), AGX (Apple), VirtIO-GPU |
| Fragment shaders only (GLES 3.1) | wgpu triangles + glyph atlas | VC4 (Pi 4) |
| No GPU | Software rasterizer + glyph atlas | Headless, fallback |

-----

## Document Map

| Document | Sections | Content |
|---|---|---|
| **This file** | §1, §2, §15, §16 | Overview, architecture, design principles, implementation order |
| [application-model.md](./interface-kit/application-model.md) | §3 | Elm Architecture, Widget trait, Command system |
| [widgets.md](./interface-kit/widgets.md) | §4 | Widget library (30+ widgets), custom widgets |
| [layout.md](./interface-kit/layout.md) | §5 | Constraint-based layout, responsive, incremental layout |
| [theme.md](./interface-kit/theme.md) | §6 | Design tokens, context-aware themes, agent theming |
| [text.md](./interface-kit/text.md) | §7 | Text pipeline, font fallback, glyph cache, i18n, variable fonts |
| [rendering.md](./interface-kit/rendering.md) | §8 | Render pipeline, display list, damage tracking, animation system |
| [backends.md](./interface-kit/backends.md) | §9 | Platform backends, bridge trait, AIOS/Linux/macOS/Web |
| [aios-features.md](./interface-kit/aios-features.md) | §10 | Semantic hints, Flow integration, Space persistence, capability-aware UI |
| [development.md](./interface-kit/development.md) | §11, §14 | SDK integration, manifest, dev workflow, CI/CD, testing strategy |
| [accessibility.md](./interface-kit/accessibility.md) | §12 | Accessibility tree (AccessKit model), screen reader, keyboard navigation |
| [performance.md](./interface-kit/performance.md) | §13 | Frame budget, texture atlas, performance guidelines |
| [intelligence.md](./interface-kit/intelligence.md) | §17, §18, §19 | AI-native UI intelligence, kernel-internal ML, future directions |

-----

## 15. Design Principles

1. **Portability is non-negotiable.** Every line of application code must compile and run on Linux, macOS, Web, and AIOS. Platform-specific features are additive, never required.

2. **AIOS features enhance, never gate.** An agent that uses Flow integration must still work (with clipboard fallback) on macOS. Capability-aware UI must still show all features on platforms without capabilities.

3. **Declarative, not imperative.** Applications describe what the UI should look like (`view()`), not how to mutate it. The toolkit handles diffing, damage tracking, and rendering.

4. **State is the truth.** The widget tree is a pure function of the application state. No hidden widget state. No out-of-band mutations. State changes only happen in `update()`.

5. **Performance is a feature, not an afterthought.** 60fps is the baseline. Damage tracking, lazy subtrees, virtual lists, constraint caching, and frame budgeting are built into the architecture, not bolted on.

6. **Accessibility is structural.** The accessibility tree is generated from the widget hierarchy — inspired by AccessKit's `Node`/`Role`/`TreeUpdate` model — not annotated after the fact. If a widget exists, it's accessible.

7. **Agents own their UI, the system owns the chrome.** Window decorations, context transitions, focus indicators, and system overlays are compositor responsibilities. Agents control content. No agent can fake system UI.

8. **Same toolkit, native feel.** Because both system experience surfaces and third-party agents use Interface Kit (via the iced bridge or directly), there's one visual language. Agent UIs don't feel like foreign widgets — they're native.

-----

## 16. Implementation Order

Interface Kit is Phase 34 in the development plan. Internal milestones:

```text
Phase 34a:  Interface Kit traits + iced bridge       → Kit compiles for AIOS target
Phase 34b:  AIOS platform backend (basic)            → surfaces rendered via Compute Kit Tier 1 + compositor IPC
Phase 34c:  Input routing                            → keyboard/mouse events reach widgets
Phase 34d:  Core widgets (text, button, input)       → basic interactive UI works
Phase 34e:  Theme system + context-aware themes      → context-adaptive theming on AIOS
Phase 34f:  Full widget set + text rendering pipeline → shaping, bidi, font fallback (Parley + ICU4X)
Phase 34g:  Animation system                         → spring animations, transitions, interruptible
Phase 34h:  Flow integration + Space persistence     → drag/drop through Flow, state saved to Spaces
Phase 34i:  Capability-aware UI + semantic hints     → widgets respond to capabilities, compositor understands content
Phase 34j:  Accessibility tree (AccessKit model)     → screen reader support, WCAG AA
Phase 34k:  Performance optimization                 → Vello renderer, constraint caching, virtual scrolling
Phase 34l:  i18n (ICU4X integration)                 → RTL, CJK, locale formatting
Phase 34m:  Web backend + Agent SDK packaging        → WASM target, aios-interface crate published
```

-----

## Cross-Reference Index

| Section | Sub-file | External references |
|---|---|---|
| §3 Application Model | [application-model.md](./interface-kit/application-model.md) | [agents.md §8](./agents.md) Agent SDK |
| §4 Widget Library | [widgets.md](./interface-kit/widgets.md) | — |
| §5 Layout Engine | [layout.md](./interface-kit/layout.md) | [compositor.md §5](../platform/compositor.md) Scene graph |
| §6 Theme System | [theme.md](./interface-kit/theme.md) | [context-engine.md §6](../intelligence/context-engine.md) Consumers |
| §7 Text Rendering | [text.md](./interface-kit/text.md) | [terminal.md §4](./terminal/rendering.md) Font engine |
| §8 Render Pipeline | [rendering.md](./interface-kit/rendering.md) | [gpu.md §9](../platform/gpu/rendering.md) wgpu integration |
| §9 Platform Backends | [backends.md](./interface-kit/backends.md) | [compositor.md §3](../platform/compositor/protocol.md) Surface protocol |
| §10 AIOS Features | [aios-features.md](./interface-kit/aios-features.md) | [flow.md §3](../storage/flow/data-model.md) FlowEntry |
| §11 Development | [development.md](./interface-kit/development.md) | [agents.md §12](./agents/distribution.md) Agent Store |
| §12 Accessibility | [accessibility.md](./interface-kit/accessibility.md) | [accessibility.md §9](../experience/accessibility/system-integration.md) Accessibility tree |
| §13 Performance | [performance.md](./interface-kit/performance.md) | [compositor.md §5.4](../platform/compositor/rendering.md) Frame scheduling |
| §14 Cross-Platform | [development.md](./interface-kit/development.md) | — |
| §17-19 AI Intelligence | [intelligence.md](./interface-kit/intelligence.md) | [airs.md §5](../intelligence/airs/intelligence-services.md) AIRS services |
