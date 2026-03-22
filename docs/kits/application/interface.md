# Interface Kit

**Layer:** Application | **Architecture:** `docs/applications/ui-toolkit.md` (needs rewrite as Interface Kit)

## Purpose

Interface Kit is the AIOS-native UI toolkit — views, controls, layout, theming, and accessibility. It is the canonical way to build native AIOS user interfaces. Cross-platform toolkits (Flutter, Qt, GTK) are bridges on top of Interface Kit rather than replacements for it.

## Key APIs

| Trait / API | Description |
|---|---|
| `View` | Base trait for all visual elements; owns a compositor surface and damage region |
| `Control` | Interactive UI element (button, slider, text field) with event handling |
| `Layout` | Constraint-based layout engine for composing views into hierarchies |
| `Theme` | Design token system — colors, typography, spacing, motion curves |
| `AccessibilityNode` | Semantic tree node exposed to assistive technologies and AIRS |

## Orchestrates

- **Compute Kit (Tier 1 — surfaces)** — allocates and presents compositor surfaces
- **Input Kit** — routes keyboard, pointer, and touch events to focused views
- **Flow Kit** — drag-and-drop, clipboard, and content transfer between views
- **Attention Kit** — focus management and notification presentation within UI

## Implementation Phase

Phase 6+ (basic compositor surfaces). Full toolkit Phase 29+
