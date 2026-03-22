---
author: Justin Lee
date: 2026-03-22
tags: [browser, platform, architecture]
status: final
---

# ADR: Browser Kit Replaces Progressive Browser

## Context

The original plan (Path C) was to build a progressive browser: html5ever + QuickJS for static rendering, expanding over time toward full web compat. This requires building a browser engine — one of the hardest software projects — competing with engines backed by thousands of engineers.

An alternative: instead of building a browser, build the OS services that make ANY browser engine an AIOS-native citizen.

## Options Considered

### Option A: Progressive browser (html5ever + QuickJS)

- Pros: Self-contained, no external dependencies, available before Linux compat
- Cons: Even 80% web compat is brutally hard, competing with Google/Mozilla on their turf, massive engineering effort for diminishing returns

### Option B: Browser Kit — expose AIOS subsystems to any browser engine

- Pros: Firefox/Chrome/Safari can all build on top, multiple browsers = user choice, AIOS adds value as a platform not a competitor, existing engines handle web compat
- Cons: Depends on Linux compat for ported browsers (Phase 35-36), no built-in browser in early phases

## Decision

Browser Kit (Option B). AIOS doesn't build a browser engine. It builds an SDK that exposes AIOS subsystems (Compute, Network, Storage, Media, Audio, Input, Camera, Flow, Identity) to any browser engine.

Browser Kit adds browser-specific glue on top of subsystem Kits:
1. Tab <-> Capability mapping (each tab gets isolated capabilities)
2. URL <-> Space mapping (browsing data backed by versioned Spaces)
3. DOM event <-> Input Kit translation
4. Compositor semantic hints (browser tells AIOS "this tab is video")

Building the platform that makes browsers better is where an OS adds value. Building a browser engine is a losing game.

## Consequences

- No custom browser engine — eliminates a massive engineering risk
- Browser availability depends on Linux compat (Phase 35-36) for Firefox/Chrome
- A lightweight reference browser (html5ever + QuickJS) could still exist to prove the Kit works, but it's not the primary browser strategy
- `docs/applications/browser.md` needs full rewrite (Servo plan -> Browser Kit)
- Browser Kit is an Application Kit — composes all lower Kits
