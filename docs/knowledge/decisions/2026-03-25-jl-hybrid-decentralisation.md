---
author: jl + claude
date: 2026-03-25
tags: [networking, decentralisation, philosophy, anm]
status: final
---

# ADR: Hybrid Decentralisation (Sovereignty, Not Isolation)

## Context

AIOS positions itself as sovereignty-first. The question is whether this means pure decentralization (no servers ever, all peer-to-peer) or hybrid decentralization (servers as optional infrastructure). Pure decentralization is ideologically appealing but has practical limitations.

## Options Considered

### Option A: Pure decentralization (no servers)

- Pros: No dependencies on any external infrastructure, cannot be censored or shut down, ideologically pure
- Cons: Cannot discover unknown entities (how do strangers find each other?), cannot do heavy AI compute on low-power devices, cannot interact with the existing web, cannot sync when all personal devices are offline, ideologically pure but practically useless for most users

### Option B: Hybrid -- sovereignty, not isolation

- Pros: Pragmatic: works with the existing internet, servers are optional mesh peers (can leave anytime without data/identity/relationship loss), user retains full sovereignty over identity, data, and relationships regardless of server usage
- Cons: Bridge Layer is the weakest security boundary (translating between trust models), some users may over-rely on servers and lose the sovereignty benefits

## Decision

Hybrid decentralization. The principle is: "I OWN my identity, data, and relationships. I CHOOSE to use your server. I can LEAVE anytime without losing anything." Servers are optional mesh peers with role capabilities. Users can operate fully offline, fully on mesh, or use servers -- their choice, revocable at any time.

## Consequences

- Bridge Layer exists and is honestly the weakest link in the security model
- Bridge weaknesses are contained: they never compromise mesh-layer guarantees (data labeled as crossing the bridge boundary is tagged and cannot be silently elevated to mesh-trust level)
- DATA labeling at the bridge boundary is the critical firewall between trust models
- Users who never use servers get full sovereignty with zero trust in external infrastructure
- Users who use servers get convenience with explicit, auditable, revocable trust delegation
- The system must work fully offline -- servers enhance but never enable core functionality
