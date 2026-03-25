---
author: jl + claude
date: 2026-03-25
tags: [networking, identity, anm]
status: final
---

# ADR: Identity-Based Routing (DeviceId Over IP Addressing)

## Context

Traditional networking uses IP addresses for device identification and routing. IP addresses are location-dependent (they change when you switch networks), NAT breaks direct addressing, and identity is a separate concept from address. AIOS already has a strong identity model based on Ed25519 key pairs. The question is whether to use IP addressing as the primary routing mechanism or build on the existing identity system.

## Options Considered

### Option A: IP addressing (standard)

- Pros: Works with all existing network infrastructure (routers, switches, DNS), well-understood routing algorithms, no custom infrastructure needed
- Cons: Address changes when device moves between networks, NAT breaks direct peer-to-peer addressing, identity and address are decoupled (must be reconciled separately), requires DNS or similar service to map names to addresses

### Option B: DeviceId addressing (sha256(pubkey) truncated to 256 bits)

- Pros: Address never changes regardless of network, NAT is irrelevant at the mesh layer, identity IS the address (no reconciliation needed), works offline (LAN discovery by DeviceId), survives WiFi-to-cellular transitions seamlessly
- Cons: Requires mesh routing infrastructure to translate DeviceId to physical location, IP still needed for WAN tunneling through the Bridge Layer, bootstrap/discovery nodes needed for initial WAN peer discovery

## Decision

DeviceId as the network address for all mesh communication. IP addresses are used only in the Bridge/Tunnel Layer as transport-level detail, never exposed to application code. Agents address peers by DeviceId; the mesh routing layer resolves DeviceId to physical reachability (direct link, LAN, or WAN tunnel).

## Consequences

- Location-independent addressing: device identity persists across all network transitions
- Seamless roaming: WiFi-to-cellular handoff is invisible to agents
- Mesh routing infrastructure must be built: peer table, reachability probing, route selection
- Bootstrap nodes needed for WAN discovery (how do two devices find each other across the internet?)
- IP addresses become an implementation detail of the Bridge Layer, never leaked to the application layer
