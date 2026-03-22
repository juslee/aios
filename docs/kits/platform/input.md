# Input Kit

**Layer:** Platform | **Architecture:** `docs/platform/input.md` + 6 sub-docs

## Purpose

Hardware-agnostic input device abstraction, event pipeline, gesture recognition, and focus management. Raw events from USB HID, VirtIO-input, and Bluetooth HID are normalized into a unified event hierarchy before dispatch to the focused surface.

## Key APIs

| Trait / API | Description |
|---|---|
| `InputDevice` | Driver trait for keyboard, pointer, touchscreen, gamepad, and accessibility devices |
| `InputEvent` | Normalized event hierarchy covering key, pointer, touch, gesture, and accessibility events |
| `GestureRecognizer` | Three-layer gesture architecture: device, semantic, and application-level recognition |
| `FocusManager` | Surface focus routing, multi-seat support, and secure input path for credentials |
| `HotkeyRegistry` | System-wide hotkey registration with capability-gated priority |

## Dependencies

Memory Kit, Capability Kit

## Consumers

Compositor (system service), Interface Kit, applications

## Implementation Phase

Phase 6+
