# Thermal Kit

**Layer:** Platform | **Architecture:** `docs/platform/thermal.md` + 5 sub-docs

## Purpose

Thermal zone monitoring, trip-point-driven cooling control, and thermal-aware scheduling. Couples hardware temperature sensors to cooling devices via configurable governors; AIRS provides predictive control beyond what reactive policies can achieve.

## Key APIs

| Trait / API | Description |
|---|---|
| `ThermalZone` | Named thermal monitoring region with sensor polling and exponential smoothing |
| `TripPoint` | Threshold-triggered escalation step with hysteresis and cross-zone coupling |
| `CoolingDevice` | Driver trait for DVFS, fan control, and compute gating |
| `ThermalGovernor` | Policy engine: step-wise, PID, and bang-bang governor implementations |
| `ThermalState` | Per-zone state exported to Scheduler for WCET budgeting and load balancing |

## Dependencies

Memory Kit, Capability Kit

## Consumers

Scheduler, Compute Kit, all subsystems

## Implementation Phase

Phase 27+
