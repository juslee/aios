---
author: Justin Lee
date: 2026-03-16
updated: 2026-03-22
tags: [compositor, gpu, intelligence, platform, kits, browser, compute]
status: graduated
---

# Discussion: Platform Vision — Custom Core, Open-Source Bridges

## Context

This discussion emerged from exploring the browser engine choice for AIOS and expanded into a comprehensive review of the entire platform stack — GPU abstraction, compositor, UI toolkit, browser, AIRS inference, and the interaction model. The central question: should AIOS build on top of existing open-source projects (wgpu, Wayland, iced, Servo, GGML), or build its own core and add compatibility layers?

The answer became a design principle: **Custom Core, Open-Source Bridges.** Build AIOS's own implementations for all core features. Add open-source compatibility layers on top — never as the foundation. This applies to every layer of the stack.

On 2026-03-22, the discussion expanded into a comprehensive **Kit architecture** — a BeOS-inspired SDK model where every AIOS subsystem exposes a Kit (SDK) with Rust traits as the source of truth.

---

## Key Ideas

### 1. Custom Core, Open-Source Bridges (Design Principle)

Every core AIOS feature should be built from scratch with deep OS integration. Open-source projects are valuable but designed for general-purpose use on existing operating systems — they can't express AIOS-specific concepts like capabilities, attention state, semantic hints, or Flow.

**Pattern:** Build the AIOS-native implementation first (tight kernel integration, capability-aware, context-adaptive). Then add a compatibility bridge that translates external protocols/APIs to the native implementation. The bridge is optional and never on the critical path.

**Why this matters:** wgpu assumes it owns the GPU. Wayland assumes a POSIX environment. iced assumes a desktop windowing model. Servo assumes a full POSIX runtime. None of these can express what makes AIOS different. Starting with them means fighting their assumptions forever.

### 2. Kit Architecture (BeOS Heritage)

Every major AIOS subsystem exposes a **Kit** — a well-defined SDK with Rust traits as the API surface. The naming is an intentional nod to BeOS (1996), which pioneered coherent, per-domain SDK naming (Application Kit, Interface Kit, Media Kit, Storage Kit, etc.). Apple later adopted the pattern extensively (UIKit, AVKit, CloudKit, MetalKit, etc.).

**Key properties:**
- **Rust traits as source of truth** — C bindings auto-generated via cbindgen for ported apps
- **No backwards compatibility until 1.0** — break freely during development. Post-1.0, Apple-style deprecation (announce → warn → remove over releases)
- **Kit extraction is organic** — define each Kit's interface as that subsystem is implemented, not upfront
- **Layered hierarchy** — Kernel Kits → Platform Kits → Intelligence Kits → Application Kits. Lower layers never depend on higher ones
- **Application Kits are compositions** — they orchestrate lower Kits for specific use cases (e.g., Browser Kit composes Compute, Network, Storage, etc.)

**BeOS comparison:**

| BeOS Kit | AIOS Equivalent | Evolution |
|---|---|---|
| Kernel Kit | IPC Kit + Memory Kit + Capability Kit | Split into 3; capabilities are first-class |
| Support Kit | *(Rust stdlib/shared crate)* | Language covers this |
| Application Kit | App Kit | High-level app lifecycle |
| Interface Kit | Interface Kit | Same name; adds capabilities, attention, Flow |
| Storage Kit | Storage Kit | BFS attributes → Spaces + Query Engine |
| Media Kit | Media Kit + Audio Kit | Split for finer granularity |
| Network Kit | Network Kit | Adds capability-gated isolation |
| Device Kit | USB Kit + Input Kit | Richer — hotplug, wireless, cameras |
| Game Kit | Compute Kit Tier 2 | Direct scanout via Compute Kit |
| Translation Kit | Translation Kit | Format conversion, used by Flow Kit |

### 3. Compute Kit (Replaces GPU-Only Design)

The original plan had a single `GpuDevice` trait. The Kit architecture replaces this with a **Compute Kit** that encompasses all accelerated hardware: GPU, NPU, and CPU SIMD. The industry is converging on CPU+GPU+NPU as the standard hardware profile (Apple Silicon, Qualcomm Snapdragon, Intel Meteor Lake+).

**Three tiers:**

| Tier | Purpose | Routes To | Consumers |
|---|---|---|---|
| **Tier 1: Display Surface** | Buffer alloc, composition, scanout, semantic hints | GPU (display controller) | Compositor, Interface Kit |
| **Tier 2: Render Pipeline** | 3D graphics, shaders, WebGPU | GPU | Games, creative apps, Browser Kit (WebGPU) |
| **Tier 3: Inference Pipeline** | LLM inference, embeddings, vision models | NPU first → GPU fallback → CPU NEON fallback | AIRS Kit, Search Kit |

**Resource Manager** (single authority):
- Routes workloads to best available hardware
- GPU memory allocation from `Pool::Dma`
- Capability enforcement per consumer
- Thermal/power budget across all compute units
- Fault isolation — one consumer's bad shader can't crash another

**AIRS GPU scheduling:** Option A (yield to interactive rendering) as default, with Option C (context-aware upgrade) when Context Engine is online. On NPU-equipped hardware, there's no conflict — AIRS runs on NPU, games run on GPU.

**Bridge stack for Linux compatibility:**

```
Linux Apps (X11/Wayland clients)
    ↓
Wayland Bridge (Smithay)
    ↓
wgpu (WebGPU API)
    ↓
Vulkan (Mesa drivers)
    ↓
Compute Kit (Tier 1 + Tier 2)
    ↓
GPU Driver (VirtIO/V3D/AGX)
```

### 4. Browser Kit (Replaces Progressive Browser)

Instead of building a custom browser engine (html5ever + QuickJS), AIOS builds a **Browser Kit** — an SDK that exposes AIOS subsystems to any browser engine. Firefox, Chrome, Safari, or a custom AIOS browser can all build on top.

**What Browser Kit provides:**

| AIOS Subsystem | Browser Kit API | What Browsers Get |
|---|---|---|
| Compute Kit | Native surface allocation, WebGPU bridge | Zero-copy rendering, compositor-aware |
| Network Kit | Capability-gated sockets, credential vault | Per-tab network isolation |
| Storage Kit | Space-backed profiles, cookies as Space objects | Versioned browsing data |
| Media Kit | Native codec access, DRM/CDM | Hardware decode |
| Audio Kit | Audio session integration | WebAudio → OS sessions |
| Input Kit | Event dispatch translation | DOM events from AIOS input |
| Camera Kit | Capture session bridge | getUserMedia → AIOS camera |
| Flow Kit | Typed clipboard, drag-drop | Rich inter-app data exchange |
| Identity Kit | OS-level auth, credential delegation | WebAuthn, passwordless |
| AIRS Kit | On-device AI features | Smart features without cloud |

**Browser-specific glue** (on top of subsystem Kits):
1. Tab ↔ Capability mapping — each tab gets an isolated capability set
2. URL ↔ Space mapping — browsing data backed by Spaces with versioning
3. DOM event ↔ Input Kit translation
4. Compositor semantic hints — browser tells AIOS "this tab is video", "this tab is document"

**Why this is better than building a browser engine:**
- Building a browser engine is a losing game against Google/Mozilla
- Building the *platform* that makes their engines better is where an OS adds value
- Multiple browsers = user choice. AIOS makes all of them better
- Any browser can be ported by adapting its platform abstraction layer to call Browser Kit APIs

### 5. Interface Kit (Renamed from UI Toolkit)

AIOS builds its own UI toolkit called **Interface Kit** (BeOS heritage name). It has first-class concepts that no existing toolkit can express:

- **Capability-visible UI**: widgets show what permissions they use; user can revoke per-widget
- **Attention-aware widgets**: dim/simplify when user attention shifts away
- **Flow-native drag/drop**: typed data exchange with OS-mediated content transformation
- **Context-adaptive layout**: density, color temperature, information architecture respond to Context Engine
- **Space-backed state persistence**: OS versions all widget state; time-travel undo across sessions
- **Intent-based interaction**: widgets declare intents ("Share", "Save", "Discuss"); OS composes UX

**Cross-platform strategy:** Interface Kit is AIOS-native only. Cross-platform UI toolkits (Flutter, Qt, GTK, Electron) are **bridges on top** of Interface Kit — they translate their widget models to Interface Kit primitives. Apps built with those toolkits work on AIOS but don't get the full AIOS-native experience unless they adopt Interface Kit directly.

```
Cross-Platform Toolkits (Flutter, Qt, GTK, Electron) — bridges
    ↓
Interface Kit — AIOS-native, capability-aware
    ↓
Compute Kit (Tier 1), Input Kit, Flow Kit, Context Kit, etc.
```

### 6. Compositor as System Service

The compositor is NOT a Kit. It's a **system service** that consumes Kits:
- Reads surfaces from Compute Kit Tier 1
- Dispatches input via Input Kit
- Manages focus via Attention Kit
- Exchanges data via Flow Kit

Apps never call "compositor API" directly. They allocate surfaces through Compute Kit and receive input through Input Kit. The compositor orchestrates behind the scenes.

```
System Services (not Kits — internal consumers):
  ├── Compositor Service  → consumes Compute, Input, Flow, Context, Attention
  ├── Service Manager     → consumes IPC, Capability
  └── Scheduler           → consumes Thermal, Power
```

### 7. candle for AIRS Inference

Replace GGML (C library) with candle (pure Rust ML inference runtime by Hugging Face):

- **Pure Rust**: no C FFI, no unsafe C code, integrates naturally with AIOS build system
- **GGUF support**: loads the same quantized model files (Q4_K_M, Q5_K_M, etc.)
- **ARM NEON SIMD**: hardware-accelerated matrix operations on aarch64
- **Metal/CUDA backends**: for Apple Silicon GPU and NVIDIA (future)
- **Aligns with custom core**: Rust-native inference rather than wrapping a C library

AIRS Kit calls Compute Kit Tier 3 (Inference Pipeline). candle is one possible implementation behind that trait. The architecture doesn't lock in candle — swapping for another runtime is an implementation detail.

### 8. Three Interaction Layers

Three coexisting interaction layers, each building on the previous:

**Layer 1 — Classic Desktop (Phase 6-7):**
Traditional windows, taskbar, manual tiling. ALL software works: Linux apps (Wayland bridge), web apps (browser), native agents. No AIRS intelligence required. Always available as fallback.

**Layer 2 — Smart Desktop (Phase 9-15):**
Traditional windows WITH AIOS intelligence applied:
- Information gravity: related windows cluster semantically
- Context-aware layout: AIRS suggests arrangement based on activity
- Flow between windows: drag content, OS transforms format
- Attention-based dimming/prioritization
- Both native agents AND Linux apps benefit (compositor reads titles, detects media)

**Layer 3 — Intelligence Surface (Phase 29-30+):**
No fixed windows. AIRS composes information based on context and intent:
- Generative UI: OS creates purpose-built interfaces on the fly
- Temporal screen: time-travel through versioned screen state
- Information gravity: content clusters by semantic relevance
- Context morphs everything: density, color, layout, information architecture
- Native AIOS agents only (deepest integration required)

**Transition strategy:** Layers coexist on the same screen. Users naturally drift 1→2→3 as native apps improve. No forced migration.

### 9. Developer Experience — AIRS as Ambient Coding Assistant

AIOS provides a unique developer experience because AIRS has compositor-level awareness:

- **Sees everything simultaneously**: editor content, terminal output, browser docs, git status — all visible to AIRS through compositor semantic hints
- **Context without copy-paste**: AIRS infers what you're working on from window arrangement, focus patterns, and content
- **Proactive assistance**: suggests fixes when it sees a compiler error in terminal while the relevant file is open in editor
- **Capability-gated trust**: developer grants AIRS specific capabilities (read editor buffer, suggest in terminal) — revocable, audited
- **Tool execution**: AIRS can run build commands, execute tests, open documentation — all through the Tool Manager with explicit capability grants

### 10. Intelligence Surface Vision

Layer 3 reimagines the screen as an information surface rather than a window manager:

**Work scenario:** Developer opens AIOS. AIRS knows it's Monday morning (Context Engine). The screen assembles: PR reviews from last week, today's calendar, relevant Slack threads, the code editor with the branch you were working on — all composed into a purpose-built layout. No app launching. No window arranging.

**Play scenario:** Gaming session. The screen becomes the game. Notifications are suppressed except urgent ones. When a friend messages about the game, it appears as a subtle overlay. Voice chat controls float at screen edge.

**Leisure scenario:** Browsing, reading, watching. Content fills the screen. Related content clusters nearby. Bookmarks and reading list emerge based on what you're consuming. Time-of-day affects color temperature and information density.

**Creative scenario:** Working on music/art/writing. Tools arrange around the canvas/timeline/document. Reference material floats nearby. Version history is visually accessible. Inspiration sources from Spaces cluster by relevance.

---

## Complete Kit Inventory (29 Kits)

### Kernel Kits (4)

| Kit | Subsystem(s) | What It Exposes |
|---|---|---|
| **Memory Kit** | Page tables, address spaces, shared memory | Buffer allocation, zero-copy sharing, memory-mapped regions |
| **IPC Kit** | Channels, notifications, select | Message passing, capability-gated communication, direct switch |
| **Capability Kit** | Capability system | Grant, attenuate, delegate, revoke capabilities |
| **Compute Kit** | GPU, NPU, CPU SIMD | Display surfaces (T1), render pipeline (T2), inference pipeline (T3), resource scheduling |

### Platform Kits (11)

| Kit | Subsystem(s) | What It Exposes |
|---|---|---|
| **Network Kit** | TCP/IP, Connection Manager, TLS, Space Resolver | Sockets, HTTP/2, QUIC, WebSocket, credential vault, per-agent isolation |
| **Storage Kit** | Spaces, Objects, Versions, Block Engine, Query Engine | Create/read/version objects, full-text + semantic search, encryption zones |
| **Audio Kit** | Audio sessions, mixer, capture, DSP pipeline | Playback, recording, real-time processing, spatial audio |
| **Media Kit** | Codecs, playback pipeline, streaming, DRM | Decode/encode, adaptive streaming, media sessions, content protection |
| **Input Kit** | Keyboard, mouse, touch, gamepad, gesture recognition | Event streams, gesture callbacks, hotkeys, IME, accessibility input |
| **USB Kit** | Host controller, device classes, hotplug | Device enumeration, class drivers, plug/unplug events |
| **Camera Kit** | UVC, CSI/MIPI, ISP pipeline, privacy controls | Capture sessions, frame delivery, hardware privacy LED enforcement |
| **Wireless Kit** | WiFi (WPA3), Bluetooth (Classic + BLE + Mesh) | Connect, scan, pair, BLE GATT, audio routing, coexistence |
| **Power Kit** | DVFS, idle states, battery, charging | Power profiles, wake locks, battery status |
| **Thermal Kit** | Thermal zones, sensors, cooling, governors | Temperature monitoring, thermal budget, throttle notifications |
| **Translation Kit** | Format conversion (image, document, data) | Convert between formats, used by Flow Kit for content transforms |

### Intelligence Kits (7)

| Kit | Subsystem(s) | What It Exposes |
|---|---|---|
| **AIRS Kit** | Inference engine (candle), model registry | Run local models, stream tokens, manage model lifecycle |
| **Context Kit** | Context Engine, signals, inference, overrides | Current context, transitions, override API |
| **Attention Kit** | Attention Manager, focus tracking | What the user is attending to, priority signals |
| **Search Kit** | Space Indexer, embeddings (HNSW), full-text (BM25), relationship graph | Semantic search, keyword search, entity relationships |
| **Flow Kit** | Flow entries, transforms, history, clipboard | Typed data exchange, content transformation, drag-drop |
| **Intent Kit** | Intent Verifier, behavioral monitor | Declare intents, verify actions, detect anomalies |
| **Preference Kit** | Preference resolution, NLU, inference, history | Read/write preferences, context-driven rules, explainability |

### Application Kits (7)

| Kit | Composes | What It Exposes |
|---|---|---|
| **App Kit** | IPC, Capability, process lifecycle | App launch/quit, foreground/background, lifecycle hooks, scripting |
| **Interface Kit** | Compute (T1), Input, Flow, Context, Attention, Preference | Widgets, layout, capability-visible UI, attention-aware components |
| **Browser Kit** | Compute, Network, Storage, Media, Audio, Input, Camera, Flow, Identity | Tab↔capability, URL↔Space, DOM events, WebGPU/WebAudio bridges |
| **Conversation Kit** | AIRS, Search, Flow, Context, Attention | Session management, context assembly, streaming, tool orchestration |
| **Identity Kit** | Capability, Network (TLS), Storage (credential vault) | Authentication, credential delegation, OS-level identity, WebAuthn |
| **Notification Kit** | Attention, Context, Preference, Flow | Priority-aware delivery, context-sensitive suppression |
| **Security Kit** | Capability, Intent, behavioral monitor | Audit trail, capability management UI, trust levels, privacy controls |

---

## Resolved Questions

| # | Question | Resolution | Date |
|---|---|---|---|
| 1 | Mixed contexts on Intelligence Surface | Weighted blending (e.g., coding:0.6, reference:0.25, chat:0.15), smooth transitions between zones | 2026-03-22 |
| 2 | Minimum viable Intelligence Surface demo | "Morning briefing" — single generated screen on login (calendar, recent work, messages, ambient context) | 2026-03-22 |
| 3 | Testing Layer 3 without infrastructure | Mock context replay + real rendering; synthetic scenarios fed to Interface Kit, capture screenshots for visual regression | 2026-03-22 |
| 4 | Cross-platform Interface Kit? | No — Interface Kit is AIOS-native. Cross-platform toolkits (Flutter, Qt, GTK) are bridges on top | 2026-03-22 |
| 5 | Compositor semantic hint granularity | 3 levels: coarse (content class, always on), medium (activity, opt-in), fine (content summary, explicit capability grant) | 2026-03-22 |
| 6 | candle vs GGML performance | Defer — benchmark at AIRS Kit implementation time. Compute Kit Tier 3 abstracts the runtime | 2026-03-22 |
| 7 | Progressive browser web compat | Replaced — Browser Kit approach means Firefox/Chrome do the heavy lifting | 2026-03-22 |
| 8 | C binding generation strategy | cbindgen auto-generation + hand-written stable ABI layer. Defer until Linux compat phase | 2026-03-22 |
| 9 | Kit dependency graph formalization | `kits.toml` at workspace root with layer rules. Formalize when 5+ Kits implemented | 2026-03-22 |
| 10 | Kit discovery/registration | Static linking (kernel/platform Kits) + service registration (intelligence/application Kits) | 2026-03-22 |

---

## References

- `docs/platform/compositor.md` — Compositor architecture (custom protocol already designed)
- `docs/platform/compositor/gpu.md` — GPU backend, wgpu integration, Wayland translation
- `docs/platform/compositor/rendering.md` — Scene graph, frame composition
- `docs/platform/gpu.md` — GPU & Display hub
- `docs/kernel/compute.md` — Heterogeneous compute architecture (aligns with Compute Kit)
- `docs/applications/interface-kit.md` — Current iced-based toolkit design (to be updated → Interface Kit)
- `docs/applications/browser.md` — Current Servo-based browser plan (to be updated → Browser Kit)
- `docs/intelligence/airs.md` — AIRS architecture overview
- `docs/intelligence/airs/inference.md` — Inference engine (currently GGML → candle)
- `docs/intelligence/context-engine.md` — Context Engine (drives Layer 2/3 adaptation)
- `docs/intelligence/attention.md` — Attention management
- `docs/project/development-plan.md` — Phase dependency graph, risk register
- `docs/experience/experience.md` — Experience layer vision

---

## Outcome

**Status: graduated (ADRs extracted 2026-03-22).**

Extracted ADRs:

- `docs/knowledge/decisions/2026-03-16-jl-custom-core-principle.md` — "Custom Core, Open-Source Bridges" philosophy
- `docs/knowledge/decisions/2026-03-22-jl-kit-architecture.md` — 29 Kits across 4 layers, BeOS heritage, Rust traits
- `docs/knowledge/decisions/2026-03-22-jl-compute-kit.md` — Compute Kit (GPU+NPU+CPU), 3 tiers, replaces GpuDevice
- `docs/knowledge/decisions/2026-03-22-jl-browser-kit.md` — Browser Kit replaces progressive browser
- `docs/knowledge/decisions/2026-03-22-jl-interface-kit.md` — Interface Kit (renamed from UI Toolkit), cross-platform bridges on top
- `docs/knowledge/decisions/2026-03-22-jl-compositor-system-service.md` — Compositor is a system service, not a Kit
- `docs/knowledge/decisions/2026-03-22-jl-app-kit.md` — App Kit for high-level app lifecycle
- `docs/knowledge/decisions/2026-03-22-jl-translation-kit.md` — Translation Kit for format conversion
- `docs/knowledge/decisions/2026-03-16-jl-candle-inference-runtime.md` — candle replaces GGML
- `docs/knowledge/decisions/2026-03-16-jl-three-interaction-layers.md` — Classic → Smart → Intelligence Surface

Architecture docs to update after graduation:
- `docs/project/architecture.md` — Add "Custom Core" design principle, Kit architecture overview
- `docs/platform/compositor.md` — Compositor as system service, not a Kit
- `docs/platform/compositor/gpu.md` — Compute Kit replaces GpuDevice
- `docs/kernel/compute.md` — Align with Compute Kit 3-tier model
- `docs/applications/interface-kit.md` — Rename to Interface Kit, add cross-platform bridge strategy
- `docs/applications/browser.md` — Browser Kit replaces progressive browser
- `docs/intelligence/airs/inference.md` — candle replaces GGML, Compute Kit Tier 3
- `docs/project/development-plan.md` — Update risk register and phase descriptions
