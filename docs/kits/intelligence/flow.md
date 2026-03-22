# Flow Kit

**Layer:** Intelligence | **Architecture:** `docs/storage/flow.md` + 7 sub-docs

## Purpose

The Flow Kit is AIOS's unified data exchange layer — it replaces the traditional clipboard,
drag-and-drop, and inter-agent file transfer with a single typed, history-preserving channel.
Every transfer passes through a content transform pipeline that converts between formats on
demand. Flow entries are persisted with provenance so users can revisit and replay past transfers
across sessions and devices.

## Key APIs

| Trait / API | Description |
|---|---|
| `FlowEntry` | A single transfer unit: typed content payload, source agent, timestamp, provenance |
| `FlowChannel` | Named channel over which agents publish and subscribe to flow entries |
| `TransformPipeline` | Conversion graph that transforms content between registered TypedContent formats |
| `FlowHistory` | Persistent, searchable log of past flow entries with replay and retention policy |

## Dependencies

- **Storage Kit** — Flow entry persistence, version-tracked history, Space integration
- **Translation Kit** — format conversion within the transform pipeline
- **Capability Kit** — agents require Flow capabilities to publish or subscribe to channels

## Consumers

- Compositor (system clipboard surface, drag-and-drop routing)
- Interface Kit (copy/paste UI, share sheet)
- Browser Kit (web content ingestion into Spaces via Flow)
- Applications (inter-agent data exchange, handoff)

## Implementation Phase

Phase 10+
