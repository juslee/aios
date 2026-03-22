# Context Kit

**Layer:** Intelligence | **Architecture:** `docs/intelligence/context-engine.md` + 5 sub-docs

## Purpose

The Context Kit collects signals from sensors, activity streams, and user behavior to infer
what the user is currently doing and what context they are operating in. It drives context-aware
adaptation across scheduling, compositing, and preference resolution. Inference results are
stabilized with hysteresis to prevent thrashing, and consumers receive push notifications on
context transitions.

## Key APIs

| Trait / API | Description |
|---|---|
| `ContextSignal` | Raw signal from a sensor or activity source with weight and freshness |
| `ContextInference` | Classifier output: current activity, confidence, and transition history |
| `ContextOverride` | Manual or rule-based override applied on top of inferred context |
| `ContextConsumer` | Subscription interface for receiving context transition events |

## Dependencies

- **AIRS Kit** — activity classification model, learned context transitions
- **Capability Kit** — capability-gated access to sensor signals and context state

## Consumers

- Scheduler (priority adjustments based on active context)
- Attention Kit (do-not-disturb and focus rules driven by context)
- Compositor (UI surface hints and layout adaptation)
- Preference Kit (context-driven preference rule evaluation)
- Compute Kit — AIRS inference scheduling hints

## Implementation Phase

Phase 10+
