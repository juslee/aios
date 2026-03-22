---
author: Justin Lee
date: 2026-03-22
tags: [compositor, platform, architecture]
status: final
---

# ADR: Compositor as System Service, Not a Kit

## Context

The compositor orchestrates surfaces, input dispatch, focus management, and frame composition. Should it be a Kit that apps call into, or a system service that consumes other Kits?

## Options Considered

### Option A: Compositor Kit (apps call compositor APIs)

- Pros: Explicit API surface, familiar pattern (Wayland is a protocol apps speak)
- Cons: Apps don't actually need compositor APIs — they need surfaces (Compute Kit) and input (Input Kit). A "Compositor Kit" would be a confused abstraction mixing resource allocation with internal orchestration.

### Option B: Compositor as system service

- Pros: Clean separation — apps use Compute Kit Tier 1 for surfaces, Input Kit for events, Flow Kit for data exchange. Compositor is internal infrastructure that reads surfaces and orchestrates them. No app-facing API to maintain.
- Cons: Less visible in the Kit hierarchy

## Decision

Compositor as system service (Option B). Apps never call "compositor API" directly.

Apps interact with the compositor indirectly through:
- **Compute Kit Tier 1** — allocate surfaces, submit damage, set semantic hints
- **Input Kit** — receive input events, focus notifications
- **Flow Kit** — typed data exchange, drag-and-drop

The compositor consumes: Compute Kit, Input Kit, Flow Kit, Context Kit, Attention Kit. It's like the scheduler — critical infrastructure but not an app-facing SDK.

System Services (not Kits):
- Compositor Service
- Service Manager
- Scheduler

## Consequences

- No "Compositor Kit" in the Kit inventory
- Compositor architecture doc (`docs/platform/compositor.md`) stays as internal design
- Apps have a simpler mental model — "I make surfaces and receive input"
- Compositor implementation details can change without breaking app APIs
