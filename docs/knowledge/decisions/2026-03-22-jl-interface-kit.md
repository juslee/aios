---
author: Justin Lee
date: 2026-03-22
tags: [ui, platform, compositor]
status: final
---

# ADR: Interface Kit — Custom UI Toolkit with Cross-Platform Bridges

## Context

AIOS needs a UI toolkit for native applications. The original plan used iced (Elm-inspired Rust toolkit). The toolkit needs to express AIOS-native concepts: capability-visible widgets, attention-aware components, Flow-native drag/drop, context-adaptive layout, Space-backed state persistence, intent-based interaction.

Additionally: should the toolkit be cross-platform (build on macOS, deploy to AIOS)?

## Options Considered

### Option A: iced as the foundation

- Pros: Mature, cross-platform, Rust-native, active community
- Cons: Assumes traditional desktop windowing, can't express capabilities/attention/Flow/intent without fighting its Elm architecture

### Option B: Custom toolkit, AIOS-only

- Pros: Full AIOS integration, capabilities and attention in the widget trait itself
- Cons: No apps until toolkit is built, no cross-platform developer story

### Option C: Custom toolkit (Interface Kit), cross-platform toolkits as bridges on top

- Pros: Full AIOS integration for native apps, Flutter/Qt/GTK/Electron apps work via bridges, developers choose their tradeoff, app ecosystem from day one via bridges
- Cons: Bridge quality depends on how well external toolkits map to Interface Kit primitives

## Decision

Interface Kit with cross-platform bridges (Option C). Named "Interface Kit" — BeOS heritage.

Interface Kit is AIOS-native only. It has first-class concepts no existing toolkit can express:
- Capability-visible UI, attention-aware widgets, Flow-native drag/drop
- Context-adaptive layout, Space-backed state, intent-based interaction

Cross-platform toolkits (Flutter, Qt, GTK, Electron) are bridges that translate their widget models to Interface Kit primitives. Apps built with those toolkits work on AIOS but don't get the full AIOS-native experience unless they adopt Interface Kit directly.

## Consequences

- iced is not used as a foundation (but could be a bridge)
- Interface Kit's design language can be published as a specification (like Apple's HIG)
- Developers can port existing Flutter/Qt apps to AIOS via bridges immediately
- Native AIOS apps using Interface Kit get the richest experience
- `docs/applications/ui-toolkit.md` needs rewrite (iced -> Interface Kit + bridge strategy)
