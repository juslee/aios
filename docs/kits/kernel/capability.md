# Capability Kit

**Layer:** Kernel | **Architecture:** `docs/security/model.md`, `docs/security/model/capabilities.md`

## Purpose

Grant, attenuate, delegate, and revoke capabilities. The security foundation — every Kit uses capabilities to control access.

## Key APIs

| Trait / API | Description |
|---|---|
| `CapabilityToken` | Unforgeable token with type, permissions, and lineage |
| `CapabilityTable` | Per-process table (256 slots), O(1) handle lookup |
| `grant/attenuate` | Create capabilities with reduced permissions |
| `delegate` | Transfer capabilities between agents |
| `revoke` | Cascade revocation — revoking a parent revokes all children |

## Dependencies

None — Capability Kit is a foundation Kit.

## Consumers

Every other Kit uses capabilities internally. Agents use Capability Kit for permission management. Security Kit exposes it to users via Inspector UI.

## Implementation Phase

Phase 3 (capability system, enforcement, cascade revocation) — **implemented**.
