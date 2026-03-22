# Network Kit

**Layer:** Platform | **Architecture:** `docs/platform/networking.md` + 6 sub-docs

## Purpose

Capability-gated networking with Space-aware name resolution, connection lifecycle management, and traffic isolation per agent. The Network Transport Manager (NTM) enforces that every outbound connection requires an explicit capability and is routed through the Shadow Engine for privacy.

## Key APIs

| Trait / API | Description |
|---|---|
| `SpaceResolver` | Translates Space names and URIs to network addresses; capability-gated DNS |
| `ConnectionManager` | Lifecycle management for TCP/UDP/QUIC connections with per-agent isolation |
| `ShadowEngine` | Optional traffic proxying and identity shielding for privacy-sensitive connections |
| `ResilienceEngine` | Retry logic, circuit breakers, and fallback routing |
| `BandwidthScheduler` | Per-agent bandwidth quotas and QoS enforcement |
| `CapabilityGate` | Enforces `NetworkAccess` capabilities before any packet leaves the agent sandbox |

## Dependencies

IPC Kit, Capability Kit, Memory Kit

## Consumers

Browser Kit, agents, POSIX compatibility layer, Flow Kit (sync)

## Implementation Phase

Phase 7+
