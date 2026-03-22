# Browser Kit

**Layer:** Application | **Architecture:** `docs/applications/browser.md` (needs rewrite as Browser Kit)

## Purpose

Browser Kit exposes AIOS subsystems as SDK hooks that existing browsers (Firefox, Chrome, Safari) can build on directly. Rather than building a custom browser, AIOS becomes the best platform for browsers that already exist — providing native GPU acceleration, network, media, and input integration points they can consume.

## Key APIs

| Trait / API | Description |
|---|---|
| `BrowserSurface` | Compositor surface contract for browser content rendering via WebGPU |
| `WebContentProcess` | Isolated process sandbox for web content with capability-gated IPC |
| `NetworkBridge` | Connects browser network stack to AIOS Network Kit (TLS, QUIC, proxy) |
| `MediaBridge` | Routes HTML5 audio/video through AIOS Media Kit codecs and sessions |
| `InputBridge` | Delivers AIOS input events in the format browsers expect |

## Orchestrates

- **Compute Kit (Tier 2 — WebGPU)** — GPU command submission for WebGPU and canvas rendering
- **Network Kit** — all browser network I/O runs through the AIOS network stack
- **Media Kit** — codec selection, DRM, and session management for web media
- **Input Kit** — keyboard, pointer, touch, and gamepad event delivery
- **Storage Kit** — origin-partitioned persistent storage and cache

## Implementation Phase

Phase 30+
