# Intent Kit

**Layer:** Intelligence | **Architecture:** `docs/intelligence/intent-verifier.md` + 6 sub-docs

## Purpose

The Intent Kit enforces that agent actions match their declared intent. It implements
decentralized information flow control (DIFC) through taint labels on data, a data flow graph
that tracks propagation across IPC boundaries, and a behavioral verification layer that checks
runtime actions against a temporal logic specification. Exfiltration attempts and intent
violations trigger escalating enforcement responses.

## Key APIs

| Trait / API | Description |
|---|---|
| `DeclaredIntent` | Agent manifest entry declaring allowed data flows and action categories |
| `IntentVerifier` | Seven-stage pipeline that checks each action against declared intent and DIFC labels |
| `TaintLabel` | Immutable label attached to data describing its sensitivity and allowed sinks |
| `DataFlowGraph` | Directed graph of IPC transfers; queried by the verifier for exfiltration paths |

## Dependencies

- **Capability Kit** — intent violations result in capability revocation and enforcement
- **AIRS Kit** — ML-based behavioral anomaly detection as a verification layer

## Consumers

- Security Kit (exfiltration detection, audit trail generation)
- Service manager (intent-gated service registration and invocation)
- Capability enforcement path (inline verification on IPC call and data access)

## Implementation Phase

Phase 15+
