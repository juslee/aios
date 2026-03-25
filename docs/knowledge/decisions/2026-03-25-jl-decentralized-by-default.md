---
author: jl + claude
date: 2026-03-25
tags: [networking, developer-experience, decentralisation, anm]
status: final
---

# ADR: Decentralized by Default (Mesh-First Developer Experience)

## Context

AIOS needs a developer experience for networking. The current web model defaults to centralized: developers write HTTP requests to servers, and peer-to-peer is an advanced topic. AIOS's philosophy is sovereignty and decentralization. The question is whether the default developer path should follow the familiar server-first model or lead with mesh-first peer-to-peer.

## Options Considered

### Option A: Server-first (like current web)

- Pros: Familiar to all web developers, extensive tutorials and patterns exist, lower learning curve
- Cons: Creates implicit server dependency in every application, contradicts AIOS sovereignty philosophy, developers build centralized apps by default and decentralize later (which rarely happens)

### Option B: Mesh-first (decentralized by default)

- Pros: No server needed to develop or test basic applications, aligns with AIOS sovereignty model, applications work offline and on LAN without internet, developers think peer-to-peer from the start
- Cons: Some use cases (heavy AI inference, global discovery, large-scale sync) still need servers, unfamiliar to most developers, requires new mental models

## Decision

Decentralized by default. `space::read()` resolves via mesh first, Bridge last. Bridge access (connecting to external HTTP servers) requires explicit opt-in in the agent manifest. Tutorials and documentation lead with mesh examples. The "Hello World" networking example is two devices syncing a space, not an HTTP request.

## Consequences

- Developers default to peer-to-peer communication; server dependency is a conscious, auditable choice
- Agent manifests must declare `bridge: true` to access external servers (capability-gated)
- QEMU multi-instance testing harness needed so developers can test mesh networking locally
- Developer tooling must support mesh natively (inspector shows mesh peers, not just HTTP connections)
- Documentation must teach mesh concepts before introducing Bridge Layer
- Some developers will find this unfamiliar; excellent documentation and examples are critical
