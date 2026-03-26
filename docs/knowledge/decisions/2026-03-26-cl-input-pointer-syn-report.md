---
author: claude
date: 2026-03-26
tags: [input, compositor]
status: final
---

# Pointer Events Use SYN_REPORT Accumulation

Decided to accumulate pointer events (ABS_X, ABS_Y, button state) in a pending state and flush as a single InputEvent::Pointer on EV_SYN/SYN_REPORT. This is the correct evdev atomic grouping model.

**Alternative considered:** Push each ABS_X/ABS_Y as a separate Pointer event immediately. Rejected because this would produce partial pointer states (new X with stale Y, or vice versa), which would cause cursor jitter in the compositor.

**Trade-off:** Slightly more complex code (PENDING_POINTER state + flush logic) but correct behavior that the compositor can rely on. Keyboard events are still pushed immediately since each key event is self-contained (no grouping needed).

**Lock nesting:** PENDING_POINTER and INPUT_QUEUE are both leaf locks, acquired separately in process_raw_event(). INPUT_DEVICES is released before processing to avoid 3-level nesting.
