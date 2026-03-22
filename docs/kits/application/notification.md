# Notification Kit

**Layer:** Application | **Architecture:** partially in `docs/intelligence/attention.md`

## Purpose

Notification Kit delivers notifications to users through the Attention Kit's filtering and prioritization layer. It manages notification channels, grouping, summarization, and presentation policy so that notifications reach users at the right time, in the right form, without overwhelming attention.

## Key APIs

| Trait / API | Description |
|---|---|
| `Notification` | A single notification with content, urgency, and routing metadata |
| `NotificationChannel` | Named channel (e.g. "messages", "reminders") with user-configurable delivery policy |
| `NotificationGroup` | Collapses related notifications into a single summary presentation |
| `DeliveryPolicy` | Rules governing when, where, and how notifications are presented |

## Orchestrates

- **Attention Kit** — filters, prioritizes, and schedules notification delivery
- **Interface Kit** — renders notification banners, badges, and the notification center
- **Audio Kit** — plays notification sounds within audio session policy

## Implementation Phase

Phase 14+
