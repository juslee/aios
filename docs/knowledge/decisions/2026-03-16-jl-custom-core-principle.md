---
author: Justin Lee
date: 2026-03-16
tags: [platform, architecture]
status: final
---

# ADR: Custom Core, Open-Source Bridges

## Context

AIOS needs GPU abstraction, a compositor, a UI toolkit, a browser, and an inference runtime. Should we build on existing open-source projects (wgpu, Wayland, iced, Servo, GGML) or build our own core and add compatibility layers?

Open-source projects are designed for general-purpose use on existing operating systems. They can't express AIOS-specific concepts like capabilities, attention state, semantic hints, or Flow.

## Options Considered

### Option A: Build on open-source foundations

- Pros: Faster initial progress, mature codebases, large communities
- Cons: Fighting assumptions forever (wgpu assumes it owns the GPU, Wayland assumes POSIX, iced assumes desktop windowing, Servo assumes full POSIX runtime), can't express AIOS-native concepts without invasive forks

### Option B: Custom core, open-source bridges

- Pros: Full control over AIOS-native concepts, tight kernel integration, bridges are optional and never on the critical path
- Cons: More upfront work, smaller initial feature set, must build core competence in each domain

## Decision

Custom Core, Open-Source Bridges (Option B). This is a system-wide design principle.

**Pattern:** Build the AIOS-native implementation first (tight kernel integration, capability-aware, context-adaptive). Then add a compatibility bridge that translates external protocols/APIs to the native implementation. The bridge is optional and never on the critical path.

Applied to:
- GPU: Custom Compute Kit at the bottom; Vulkan/wgpu as bridges on top
- Compositor: Custom AIOS protocol; Wayland bridge via Smithay (deferred)
- UI Toolkit: Custom Interface Kit; Flutter/Qt/GTK as bridges on top
- Browser: Browser Kit exposing AIOS subsystems; Firefox/Chrome/Safari build on top
- Inference: candle (pure Rust); Compute Kit Tier 3 abstracts the runtime

## Consequences

- Every subsystem requires building a native implementation before bridges
- Open-source projects still used, but as bridges not foundations
- AIOS-native apps get full platform integration; bridged apps work but don't get all features
- This principle guides all future architectural decisions
- Discussion: `docs/knowledge/discussions/2026-03-16-jl-platform-vision-custom-core.md`
