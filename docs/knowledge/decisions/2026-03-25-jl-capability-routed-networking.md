---
author: jl + claude
date: 2026-03-25
tags: [networking, security, capabilities, anm]
status: final
---

# ADR: Capability-Routed Networking

## Context

How should network access be controlled in AIOS? Traditional systems use firewall rules and ACLs that are configured separately from the application security model. AIOS already has a capability-based security system where every resource access requires a capability token. The question is whether networking should use a separate access control mechanism or integrate with the existing capability system.

## Options Considered

### Option A: Traditional firewall + ACLs on top of mesh

- Pros: Well-understood model, extensive tooling, familiar to network administrators
- Cons: Separate from the capability system (two security models to reason about), static rules that must be manually maintained, can be misconfigured (open ports, overly permissive rules), does not prevent unauthorized discovery

### Option B: Capability tokens as routing credentials

- Pros: No token means not routable (zero trust is structural, not policy), eliminates unauthorized discovery (no port scanning possible), no firewall configuration needed (security is implicit), integrates with existing capability attenuation and delegation
- Cons: Cold-start discovery requires explicit pairing ceremony or bootstrap nodes, cannot fall back to "open network" mode, every network operation requires capability validation

## Decision

Capability-routed networking. A mesh message cannot be routed to a peer without a valid capability token that authorizes the connection. Zero trust is structural -- it cannot be disabled or misconfigured because routing itself requires capabilities.

## Consequences

- Port scanning is impossible: devices without capabilities cannot even discover endpoints
- No firewall configuration: the capability system IS the firewall
- Cannot be disabled: there is no "trusted network" concept in the mesh layer
- Cold-start requires a pairing ceremony (QR code, NFC tap, or bootstrap node introduction)
- Every network operation pays the cost of capability validation (mitigated by caching validated sessions)
- Capability revocation immediately severs network access (no stale firewall rules)
