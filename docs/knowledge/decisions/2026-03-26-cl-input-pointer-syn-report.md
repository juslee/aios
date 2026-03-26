---
author: claude
date: 2026-03-26
tags: [input, compositor]
status: final
---

# ADR: Pointer Events Use SYN_REPORT Accumulation

## Context

The evdev model groups pointer-related events atomically using `EV_SYN/SYN_REPORT`. Individual events such as `ABS_X`, `ABS_Y`, and button state transitions arrive separately but represent a single logical pointer state update that the compositor expects as a coherent snapshot.

## Options Considered

1. Accumulate pointer events in a pending state and flush as a single `InputEvent::Pointer` on `EV_SYN/SYN_REPORT`.
2. Push each `ABS_X`/`ABS_Y` as a separate Pointer event immediately.

Option 2 was rejected because it produces partial pointer states (new X with stale Y), causing cursor jitter in the compositor.

## Decision

Accumulate `ABS_X`, `ABS_Y`, and button state in `PENDING_POINTER`. Flush as a single `InputEvent::Pointer` on `SYN_REPORT`. Keyboard events are pushed immediately (self-contained, no grouping needed).

## Consequences

- Slightly more complex code (PENDING_POINTER state + flush logic) but correct, jitter-free behavior.
- `PENDING_POINTER` and `INPUT_QUEUE` are both leaf locks, acquired separately. `INPUT_DEVICES` is released before processing to avoid 3-level lock nesting.
