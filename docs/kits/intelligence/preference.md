# Preference Kit

**Layer:** Intelligence | **Architecture:** `docs/intelligence/preferences.md` + 8 sub-docs

## Purpose

The Preference Kit learns and resolves user preferences through a 7-tier source precedence
model — from enterprise policy down to inferred behavioral defaults. It exposes a natural
language settings interface backed by an NLU pipeline, observes behavioral signals to infer
unstated preferences, and propagates changes across devices. Temporal rules allow preferences
to activate contextually (by time of day, location, or detected activity).

## Key APIs

| Trait / API | Description |
|---|---|
| `PreferenceStore` | Typed key-value store for preference values with source metadata and schema registry |
| `PreferenceResolver` | Evaluates the 7-tier precedence stack to return the effective value for a preference key |
| `BehavioralObserver` | Watches user actions and emits inferred preference updates with confidence scores |
| `SettingsUI` | NLU-driven settings surface; accepts natural language and maps to preference keys |

## Dependencies

- **Context Kit** — context-driven temporal preference rules (time, location, activity)
- **AIRS Kit** — NLU parsing for natural language settings, contextual bandit preference inference
- **Storage Kit** — preference persistence, cross-device sync via Space Sync
- **Capability Kit** — enterprise policy tier enforcement, capability-gated preference access

## Consumers

- All Kits (query effective preference values to drive adaptive behavior)
- Settings UI application (user-facing preference editing surface)
- Applications (per-app preference namespaces and defaults)

## Implementation Phase

Phase 14+
