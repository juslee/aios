---
author: Justin Lee
date: 2026-03-16
tags: [compositor, gpu, intelligence, platform]
status: active
---

# Discussion: Platform Vision — Custom Core, Open-Source Bridges

## Context

This discussion emerged from exploring the browser engine choice for AIOS and expanded into a comprehensive review of the entire platform stack — GPU abstraction, compositor, UI toolkit, browser, AIRS inference, and the interaction model. The central question: should AIOS build on top of existing open-source projects (wgpu, Wayland, iced, Servo, GGML), or build its own core and add compatibility layers?

The answer became a design principle: **Custom Core, Open-Source Bridges.** Build AIOS's own implementations for all core features. Add open-source compatibility layers on top — never as the foundation. This applies to every layer of the stack.

## Key Ideas

### 1. Custom Core, Open-Source Bridges (Design Principle)

Every core AIOS feature should be built from scratch with deep OS integration. Open-source projects are valuable but designed for general-purpose use on existing operating systems — they can't express AIOS-specific concepts like capabilities, attention state, semantic hints, or Flow.

**Pattern:** Build the AIOS-native implementation first (tight kernel integration, capability-aware, context-adaptive). Then add a compatibility bridge that translates external protocols/APIs to the native implementation. The bridge is optional and never on the critical path.

**Why this matters:** wgpu assumes it owns the GPU. Wayland assumes a POSIX environment. iced assumes a desktop windowing model. Servo assumes a full POSIX runtime. None of these can express what makes AIOS different. Starting with them means fighting their assumptions forever.

### 2. Layered GPU Stack

The GPU stack has four layers, with AIOS owning the bottom and open-source providing compatibility at the top:

```
Layer 4: Applications (browser WebGPU, UI toolkit rendering)
Layer 3: wgpu (Rust WebGPU, speaks Vulkan) — bridge
Layer 2: Vulkan driver (Mesa ports: v3dv, lavapipe, Venus) — bridge
Layer 1: AIOS GpuDevice trait (register access, DMA, IRQ) — custom core
Layer 0: Hardware (VirtIO-GPU, V3D, AGX)
```

**GpuDevice trait** (~500 LoC): thin, OS-integrated hardware abstraction. Direct implementations for VirtIO-GPU 2D (QEMU), V3D (Pi 4), AGX (Apple Silicon), and a software fallback. Allocates from `Pool::Dma`, integrates with AIOS page tables and IOMMU.

**Key insight:** wgpu is designed for applications ON an operating system — it assumes someone else manages the GPU, display, and memory. For the OS itself, you need direct hardware control. The compositor uses Layer 1 directly for basic composition; Layer 3 (wgpu) only for effects.

### 3. Custom Compositor Protocol

AIOS compositor uses a custom IPC protocol, not Wayland. The protocol includes concepts that Wayland cannot express:

- **Semantic hints**: surfaces declare content type (document, video, terminal, conversation), enabling AIRS-driven layout
- **Capability-gated surfaces**: each surface tied to the creating agent's capabilities
- **Attention state**: compositor knows which surfaces the user is attending to
- **Flow integration**: typed drag-and-drop with OS-mediated content transformation

Scene graph inspired by Fuchsia's Flatland. Damage tracking, direct scanout promotion, vsync-aware frame scheduling.

**Wayland bridge** (deferred to Phase 35-36): `WaylandTranslator` service using Smithay for protocol parsing. Maps `wl_surface` → AIOS `SurfaceId`, translates events bidirectionally. XWayland for legacy X11 apps. Linux apps participate in Layer 2 intelligence automatically (compositor reads titles, detects media).

### 4. Custom UI Toolkit

AIOS builds its own UI toolkit instead of using iced. The toolkit has first-class concepts that no existing toolkit can express:

- **Capability-visible UI**: widgets show what permissions they use; user can revoke per-widget
- **Attention-aware widgets**: dim/simplify when user attention shifts away
- **Flow-native drag/drop**: typed data exchange with OS-mediated content transformation
- **Context-adaptive layout**: density, color temperature, information architecture respond to Context Engine
- **Space-backed state persistence**: OS versions all widget state; time-travel undo across sessions
- **Intent-based interaction**: widgets declare intents ("Share", "Save", "Discuss"); OS composes UX

**Why not iced:** iced is Elm-inspired and excellent for cross-platform Rust apps, but it assumes a traditional desktop model. It can't express capabilities, attention, Flow, or intent without fighting its architecture. Building custom means these concepts are in the widget trait itself.

**Rendering:** Direct to GpuDevice trait (Layer 1) or via Vello for complex path rendering.

### 5. Progressive Browser (Path C)

Instead of porting full Servo (which requires SpiderMonkey C++ JIT, POSIX shims, and massive integration effort), AIOS builds progressively:

**Phase 1 — Static renderer (Phase 30):**
- html5ever (HTML parser — standalone Servo crate)
- cssparser + selectors (CSS parsing — standalone Servo crates)
- Custom layout engine (flex, grid, block — target 80% web compat)
- Render to compositor surface via AIOS rendering pipeline
- QuickJS-ng for basic JS interactivity (already planned for agents)

**Phase 2 — Interactive (later):**
- Expand JS Web API surface (fetch → OS networking, localStorage → Spaces)
- CSS animations, transitions
- Improve layout edge cases

**Phase 3 — Full engine (much later, if needed):**
- SpiderMonkey JIT for JS-heavy web apps (requires POSIX shim from Phase 22+)
- Or continue improving QuickJS + custom layout

**Rationale:** Gets a working browser earlier. Avoids the SpiderMonkey C++ build system cliff. Linux compatibility (Phase 35-36) provides Chrome/Firefox as a bridge for full web compat.

### 6. candle for AIRS Inference

Replace GGML (C library) with candle (pure Rust ML inference runtime by Hugging Face):

- **Pure Rust**: no C FFI, no unsafe C code, integrates naturally with AIOS build system
- **GGUF support**: loads the same quantized model files (Q4_K_M, Q5_K_M, etc.)
- **ARM NEON SIMD**: hardware-accelerated matrix operations on aarch64
- **Metal/CUDA backends**: for Apple Silicon GPU and NVIDIA (future)
- **Aligns with custom core**: Rust-native inference rather than wrapping a C library

AIRS architecture unchanged — still runs existing LLMs (Llama, Mistral, Phi) locally. candle is the inference runtime; Intelligence Services (Space Indexer, Context Engine, Attention Manager, etc.) remain custom Rust orchestration code. Kernel-internal ML uses tiny decision trees (not LLMs).

### 7. Three Interaction Layers

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

### 8. Developer Experience — AIRS as Ambient Coding Assistant

AIOS provides a unique developer experience because AIRS has compositor-level awareness:

- **Sees everything simultaneously**: editor content, terminal output, browser docs, git status — all visible to AIRS through compositor semantic hints
- **Context without copy-paste**: AIRS infers what you're working on from window arrangement, focus patterns, and content
- **Proactive assistance**: suggests fixes when it sees a compiler error in terminal while the relevant file is open in editor
- **Capability-gated trust**: developer grants AIRS specific capabilities (read editor buffer, suggest in terminal) — revocable, audited
- **Tool execution**: AIRS can run build commands, execute tests, open documentation — all through the Tool Manager with explicit capability grants

This is fundamentally different from AI coding assistants that only see what's pasted into a chat window. AIRS operates at the OS level — it's the difference between a colleague looking over your shoulder vs. one you have to describe everything to.

### 9. Intelligence Surface Vision

Layer 3 reimagines the screen as an information surface rather than a window manager:

**Work scenario:** Developer opens AIOS. AIRS knows it's Monday morning (Context Engine). The screen assembles: PR reviews from last week, today's calendar, relevant Slack threads, the code editor with the branch you were working on — all composed into a purpose-built layout. No app launching. No window arranging.

**Play scenario:** Gaming session. The screen becomes the game. Notifications are suppressed except urgent ones. When a friend messages about the game, it appears as a subtle overlay. Voice chat controls float at screen edge.

**Leisure scenario:** Browsing, reading, watching. Content fills the screen. Related content clusters nearby. Bookmarks and reading list emerge based on what you're consuming. Time-of-day affects color temperature and information density.

**Creative scenario:** Working on music/art/writing. Tools arrange around the canvas/timeline/document. Reference material floats nearby. Version history is visually accessible. Inspiration sources from Spaces cluster by relevance.

## Open Questions

- How does the Intelligence Surface handle mixed contexts (e.g., coding while referencing design specs while chatting)?
- What's the minimum viable Intelligence Surface demo that would be compelling?
- How do we test/validate Layer 3 experiences without having all the underlying infrastructure?
- Should the custom UI toolkit support a cross-platform target (build on macOS, deploy to AIOS)?
- What's the right granularity for compositor semantic hints? Too coarse = useless; too fine = privacy concern.
- How does candle's performance compare to GGML on ARM NEON for common model sizes (7B Q4)?
- Can the progressive browser reach sufficient web compat for developer tools (GitHub, docs sites) before Linux compat arrives?

## References

- `docs/platform/compositor.md` — Compositor architecture (custom protocol already designed)
- `docs/platform/compositor/gpu.md` — GPU backend, wgpu integration, Wayland translation
- `docs/platform/compositor/rendering.md` — Scene graph, frame composition
- `docs/platform/gpu.md` — GPU & Display hub
- `docs/applications/ui-toolkit.md` — Current iced-based toolkit design
- `docs/applications/browser.md` — Current Servo-based browser plan
- `docs/intelligence/airs.md` — AIRS architecture overview
- `docs/intelligence/airs/inference.md` — Inference engine (currently GGML)
- `docs/intelligence/context-engine.md` — Context Engine (drives Layer 2/3 adaptation)
- `docs/intelligence/attention.md` — Attention management
- `docs/project/development-plan.md` — Phase dependency graph, risk register
- `docs/experience/experience.md` — Experience layer vision

## Outcome

_Status: active — decisions not yet extracted._

When ready, extract to:
- `decisions/2026-03-16-jl-custom-core-principle.md` — "Custom Core, Open-Source Bridges" philosophy
- `decisions/2026-03-16-jl-layered-gpu-stack.md` — GpuDevice → Vulkan → wgpu
- `decisions/2026-03-16-jl-custom-compositor-protocol.md` — Custom protocol, Wayland bridge deferred
- `decisions/2026-03-16-jl-custom-ui-toolkit.md` — Custom toolkit, not iced
- `decisions/2026-03-16-jl-progressive-browser.md` — html5ever + QuickJS, not full Servo
- `decisions/2026-03-16-jl-candle-inference-runtime.md` — candle replaces GGML
- `decisions/2026-03-16-jl-three-interaction-layers.md` — Classic → Smart → Intelligence Surface

Architecture docs to update after graduation:
- `docs/project/architecture.md` — Add "Custom Core" design principle
- `docs/platform/compositor.md` — Reference custom protocol decision
- `docs/platform/compositor/gpu.md` — GpuDevice as primary, wgpu as bridge
- `docs/applications/ui-toolkit.md` — Custom toolkit replaces iced
- `docs/applications/browser.md` — Progressive strategy replaces full Servo
- `docs/intelligence/airs/inference.md` — candle replaces GGML
- `docs/project/development-plan.md` — Update risk register and phase descriptions
