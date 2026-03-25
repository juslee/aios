---
author: jl + claude
date: 2026-03-25
tags: [networking, architecture, anm]
status: final
---

# ADR: ANM (AI Network Model) Over OSI

## Context

AIOS needs a networking model. The OSI model assumes heterogeneous, untrusted, location-addressed networks where security is bolted on after the fact. AIOS has fundamentally different properties: sovereign identity (Ed25519 key pairs), capability-gated access, content-addressed data, and a peer mesh topology. The question is whether to build on the OSI model or design a new one that reflects these properties natively.

## Options Considered

### Option A: Build on OSI (smoltcp TCP/IP as foundation, NTM on top)

- Pros: Proven model, well-understood by all network engineers, every internet service uses it, extensive tooling and debugging support
- Cons: TCP/IP assumptions (location-based addressing, connectionless security, socket abstraction) leak into the design at every layer, no native security model, agents see raw sockets instead of spaces

### Option B: ANM (AI Network Model) -- new 5-layer model, TCP/IP as bridge only

- Pros: Native security at every layer, spaces as the addressing primitive, mesh-first topology, developer API matches AIOS philosophy (space::read/write instead of socket::send/recv)
- Cons: Must maintain a Bridge Layer for legacy internet interop, novel model requires extensive documentation, no existing ecosystem tooling

## Decision

ANM. TCP/IP is relegated to the Bridge Layer -- a translation boundary, not a foundation. The Mesh Layer is the native protocol. The five ANM layers (Application/Space, Routing/Mesh, Security/Noise, Transport/Bridge, Physical/Link) replace OSI's seven layers with purpose-built abstractions.

## Consequences

- Bridge Layer complexity: must maintain two full stacks (mesh native + TCP/IP bridge) and translate between them at the boundary
- All ~170 networking architecture docs need updating to reflect ANM terminology and layer model
- Security and developer experience are fundamentally better: no "add TLS later" pattern, no socket API confusion
- Mesh-first means offline/LAN communication works without any internet infrastructure
- Bridge weaknesses are contained at the boundary and never compromise mesh guarantees
