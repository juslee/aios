---
author: Justin Lee
date: 2026-03-22
tags: [platform, architecture]
status: final
---

# ADR: Kit Architecture — BeOS-Inspired SDK Model

## Context

AIOS has 30+ subsystems. How should they expose their APIs to applications and to each other? Need a coherent, discoverable, layered SDK model.

BeOS (1996) pioneered per-domain "Kit" naming (Application Kit, Interface Kit, Media Kit, etc.). Apple scaled the pattern to 50+ Kits (UIKit, AVKit, CloudKit, MetalKit, etc.). Both demonstrate that Kit-based SDK organization scales well and is immediately understood by developers.

## Options Considered

### Option A: Flat trait collection

- Pros: Simple, no hierarchy to maintain
- Cons: No discoverability, no dependency rules, no layering discipline, grows into chaos at 30+ subsystems

### Option B: Kit architecture with 4-layer hierarchy

- Pros: BeOS heritage (proven), clear dependency rules (lower never depends on higher), Application Kits are compositions not new primitives, organic extraction (define as each subsystem ships)
- Cons: 29 Kits is a lot to name and document, some boundary decisions are judgment calls

## Decision

Kit architecture with 4-layer hierarchy (Option B). 29 Kits across 4 layers:

- **Kernel Kits (4):** Memory, IPC, Capability, Compute
- **Platform Kits (11):** Network, Storage, Audio, Media, Input, USB, Camera, Wireless, Power, Thermal, Translation
- **Intelligence Kits (7):** AIRS, Context, Attention, Search, Flow, Intent, Preference
- **Application Kits (7):** App, Interface, Browser, Conversation, Identity, Notification, Security

Key properties:
- **Rust traits as source of truth** — C bindings auto-generated via cbindgen for ported apps
- **No backwards compatibility until 1.0** — break freely during development; post-1.0 Apple-style deprecation
- **Kit extraction is organic** — define each Kit's interface as that subsystem is implemented
- **Lower layers never depend on higher ones** — enforced by build system
- **Application Kits are compositions** — they orchestrate lower Kits for specific use cases

## Consequences

- Every subsystem gets a Kit with well-defined Rust trait APIs
- C bindings deferred until Linux compat phase (no C consumers before then)
- Kit dependency graph formalized in `kits.toml` when 5+ Kits implemented
- Kernel/Platform Kits: static linking; Intelligence/Application Kits: service registration
- Discussion: `docs/knowledge/discussions/2026-03-16-jl-platform-vision-custom-core.md`
