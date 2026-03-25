---
author: jl + claude
date: 2026-03-25
tags: [networking, architecture, decentralisation, anm]
status: final
---

# ADR: Servers as Mesh Peers (No Special Server Protocol)

## Context

AIOS needs servers for certain functions: relay (NAT traversal), backup (off-device storage), discovery (finding peers on WAN), and compute (heavy AI inference). The question is whether servers should get a special protocol or be treated as regular mesh peers with specific role capabilities.

## Options Considered

### Option A: Separate server protocol (HTTP API for servers, mesh for peers)

- Pros: Simpler server implementation (standard HTTP/REST), server developers can use existing tooling, clear separation between "server world" and "mesh world"
- Cons: Two protocols to maintain, servers become special entities with different trust rules, harder to self-host (must implement HTTP API), different audit trail for server vs peer communication

### Option B: Servers as mesh peers with role capabilities

- Pros: Uniform trust model (all peers authenticated the same way), any peer can take any role, users can self-host everything (same protocol), auditable in Inspector (same audit trail as peer communication), servers are replaceable (switch providers without protocol change)
- Cons: Server implementors must implement full mesh protocol stack, slightly more complex than a simple HTTP API

## Decision

Servers ARE mesh peers. Role capabilities (Relay, Backup, Discovery, Compute) are standard capabilities granted to peers, not special server designations. The same Noise IK authentication, capability validation, and audit trail apply to servers as to personal devices.

## Consequences

- Any peer can take any role: a Raspberry Pi at home can be a relay, backup, or discovery node
- Users can self-host everything with no protocol change
- Server trust level is "Service" -- below own devices, below friends, above Bridge Layer entities
- Uniform audit trail: Inspector shows server interactions alongside peer interactions
- Server providers are commodity: switching from one relay provider to another requires only re-granting the Relay capability
- Server implementors must implement the full mesh stack (reference implementation provided)
