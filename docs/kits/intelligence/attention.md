# Attention Kit

**Layer:** Intelligence | **Architecture:** `docs/intelligence/attention.md`

## Purpose

The Attention Kit manages the flow of notifications and interruptions to the user. It filters,
prioritizes, and batches incoming notifications based on current context, active focus sessions,
and per-agent attention budgets. Its primary goal is to protect user focus while ensuring
high-urgency signals break through when necessary.

## Key APIs

| Trait / API | Description |
|---|---|
| `AttentionManager` | Central coordinator: routes notifications through filter and priority pipeline |
| `NotificationFilter` | Per-agent and system-wide rules that suppress or delay notifications |
| `FocusSession` | User-initiated or context-triggered period of heightened interruption suppression |
| `AttentionBudget` | Per-agent quota limiting notification rate and escalation frequency |

## Dependencies

- **Context Kit** — current activity and focus state drive filter policy
- **Capability Kit** — agents must hold attention capabilities to send notifications

## Consumers

- Notification Kit (delivery routing and suppression)
- Compositor (visual notification placement and urgency indicators)
- Applications (query current focus state before generating interruptions)

## Implementation Phase

Phase 14+
