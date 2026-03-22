# Power Kit

**Layer:** Platform | **Architecture:** `docs/platform/power-management.md`

## Purpose

Power state management, wake lock arbitration, battery monitoring, and charging control. Provides a policy layer that all other Kits consult before entering low-power modes, ensuring no subsystem can silently prevent sleep or drain the battery.

## Key APIs

| Trait / API | Description |
|---|---|
| `PowerState` | System power state machine: active, idle, suspend, hibernate, shutdown |
| `WakeLock` | Capability-gated lock preventing entry into low-power states; reference-counted |
| `BatteryMonitor` | Battery level, charge rate, health, and capacity reporting |
| `PowerProfile` | Named power policy (performance, balanced, efficiency) with per-subsystem knobs |

## Dependencies

Memory Kit, Capability Kit

## Consumers

Scheduler, Thermal Kit, all Kits (power-aware behavior)

## Implementation Phase

Phase 27+
