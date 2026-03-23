# Browser Kit

**Layer:** Application | **Architecture:** `docs/applications/browser.md` + 6 sub-docs

## 1. Overview

Browser Kit is the AIOS-native platform SDK for web browsers. It defines Rust traits that any browser engine plugs into — gaining capability-enforced security, Space-backed storage, and subsystem-mediated hardware access without modifying core rendering or JavaScript engines. AIOS doesn't build a browser engine; it builds the platform that makes any engine a first-class citizen.

**ADR:** [Browser Kit Replaces Progressive Browser](../../knowledge/decisions/2026-03-22-jl-browser-kit.md)

The central design insight is that each browser tab maps to a separate AIOS agent. A tab's
agent receives its own capability set, address space, and resource budget. This means that
one tab cannot access another tab's network connections, storage, or sensor grants unless
the user explicitly delegates that capability. The browser chrome (address bar, bookmarks,
tab strip) runs as a privileged orchestrator agent that spawns and manages per-tab agents,
mediating capability delegation on the user's behalf.

| Trait / API | Description |
|---|---|
| `BrowserSurface` | Compositor surface contract for browser content rendering |
| `WebContentProcess` | Isolated renderer sandbox with capability-gated IPC |
| `NetworkBridge` | Translates browser network requests to Network Kit with CORB filtering |
| `MediaBridge` | Routes HTML5 audio/video through Media Kit codecs and sessions |
| `InputBridge` | Translates Input Kit events to DOM event format |
| `StorageBridge` | Maps web storage APIs (cookies, localStorage, IndexedDB) to Space sub-spaces |
| `CapabilityMapper` | Derives capability sets from web origins, enforces same-origin policy |

## Integration Tiers

| Tier | Description | Browser Availability |
|---|---|---|
| **Tier 1: Linux Compat** | Browsers run unmodified via Linux binary compatibility | Phase 35-36 |
| **Tier 2: Kit SDK** | Engines call Browser Kit traits directly for deep integration | Phase 30+ |
| **Tier 3: Reference** | html5ever + QuickJS native browser proves Kit APIs | Phase 30 |

## 2. Core Traits

- **Compute Kit (Tier 1)** — display surfaces and canvas presentation; compositor service — surface lifecycle, shared buffers, fences, damage reporting
- **Compute Kit (Tier 2)** — GPU command submission for WebGPU and canvas rendering
- **Network Kit** — all browser network I/O runs through capability-gated AIOS network stack
- **Media Kit** — codec selection, DRM, and session management for web media
- **Input Kit** — keyboard, pointer, touch, and gamepad event delivery
- **Storage Kit** — origin-partitioned persistent storage mapped to Spaces
- **Capability Kit** — origin-to-capability mapping, same-origin policy enforcement

## Dependencies

Compute Kit, Network Kit, Storage Kit, Media Kit, Input Kit, Capability Kit, compositor service

## Consumers

Web browsers (Firefox, Chrome, WebKit/WPE, Servo), reference browser, PWAs
