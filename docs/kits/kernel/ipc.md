# IPC Kit

**Layer:** Kernel | **Architecture:** `docs/kernel/ipc.md`

## Purpose

Capability-gated message passing, notifications, and multi-wait between agents. The communication backbone of AIOS.

## Key APIs

| Trait / API | Description |
|---|---|
| `Channel` | Bidirectional message channel with 16-slot ring buffer |
| `MessageRing` | Lock-free ring buffer for IPC messages (256-byte inline payload) |
| `Notification` | Atomic OR signaling with mask-based wake, up to 8 waiters |
| `IpcSelect` | Multi-wait on channels + notifications with timeout |
| `SharedMemoryRegion` | Large data exchange via Memory Kit shared regions |
| `ipc_call/recv/reply/send` | Core IPC operations with direct switch optimization |

## Dependencies

Memory Kit, Capability Kit

## Consumers

All inter-agent communication. App Kit wraps IPC Kit into a higher-level message dispatch loop.

## Implementation Phase

Phase 3 (IPC, notifications, select, shared memory) — **implemented**.
