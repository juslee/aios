# AIOS Thermal Security

Part of: [thermal.md](../thermal.md) — Thermal Management
**Related:** [zones.md](./zones.md) — Thermal zones & sensors, [integration.md](./integration.md) — External interfaces

---

## §11 Thermal Security

Thermal management is safety-critical infrastructure — incorrect thermal handling can cause permanent
hardware damage, data loss, or fire risk. Unlike most OS subsystems where a security failure leads to
data compromise, a thermal security failure can physically destroy the device.

AIOS addresses this through three complementary mechanisms: capability-based access control that
restricts who can observe and modify thermal state, non-overridable safety invariants enforced at the
kernel level that no agent or governor can bypass, and comprehensive audit logging that records every
thermally significant event for post-incident analysis and anomaly detection.

---

### §11.1 Capability-Gated Thermal Access

All thermal operations require explicit capability tokens. The capability system follows the standard
AIOS model described in `security/model/capabilities.md §3`.

```rust
pub enum ThermalCapability {
    /// Read thermal zone temperatures and cooling device states.
    ThermalRead,
    /// Modify trip points, change governors, adjust cooling parameters.
    ThermalConfigure,
    /// Register new cooling devices with the thermal framework.
    ThermalCoolingRegister,
    /// Override governor decisions (dangerous — requires trust level 3+).
    ThermalGovernorOverride,
    /// Access raw sensor registers (diagnostic — requires trust level 4).
    ThermalSensorRaw,
}
```

**Trust Level Mapping:**

| Capability | Min Trust Level | Typical Holder |
|---|---|---|
| `ThermalRead` | 1 (User) | Any agent |
| `ThermalConfigure` | 3 (System) | Policy Engine, system services |
| `ThermalCoolingRegister` | 3 (System) | Device drivers |
| `ThermalGovernorOverride` | 3 (System) | Policy Engine only |
| `ThermalSensorRaw` | 4 (Kernel) | Diagnostic tools, factory test |

The kernel thermal enforcement layer checks capability tokens at every thermal API entry point.
A missing or insufficiently trusted token results in `ThermalError::CapabilityDenied` and an
audit entry (see §11.2).

**Capability Attenuation:**

Capability tokens can be attenuated before delegation, narrowing the scope of access while
preserving the least-privilege principle:

- `ThermalRead` can be attenuated to a specific zone:
  `ThermalRead { zone: "cpu-thermal" }` — grants read access to that zone only.
- `ThermalConfigure` can be attenuated to a subset of parameters:
  `ThermalConfigure { scope: TripPointsOnly }` — prevents governor or cooling changes.
- Attenuation is monotone: an attenuated token cannot be widened by the holder.
- Delegation follows the standard attenuation rules in `security/model/capabilities.md §3.4`.

**Example enforcement:**

```rust
pub fn read_zone_temperature(zone: &str, cap: &CapabilityToken) -> Result<i32, ThermalError> {
    // Verify capability type and zone scope.
    check_thermal_capability(cap, ThermalCapability::ThermalRead, Some(zone))?;

    // Proceed with sensor read — all temperatures in millidegrees Celsius.
    let temp_mc = THERMAL_REGISTRY.read().zone(zone)?.current_temp_mc();
    audit_thermal(ThermalAuditEvent::ThermalCapabilityUse {
        capability: ThermalCapability::ThermalRead,
        zone,
        result: Ok(()),
    });
    Ok(temp_mc)
}
```

---

### §11.2 Thermal Audit Trail

Every thermally significant event is written to the kernel audit ring. This provides both a
real-time security monitoring stream and a post-incident forensic record. AIRS can analyze the
audit trail for anomaly detection as described in [intelligence.md](./intelligence.md) §13.5.

**Audited Events:**

| Event Type | Data Logged | Severity |
|---|---|---|
| `TripCrossing` | zone, trip_type, temp_mc, direction (rising/falling) | Warning |
| `GovernorDecision` | zone, governor, old_state, new_state, reason | Info |
| `CoolingStateChange` | device, old_state, new_state | Info |
| `ThermalCapabilityUse` | agent_id, capability, zone, result | Info |
| `ThermalCapabilityDenied` | agent_id, capability, zone | Warning |
| `SensorFailure` | zone, failure_type, last_good_temp_mc | Error |
| `CriticalTrip` | zone, temp_mc, shutdown_initiated | Critical |
| `GovernorOverride` | agent_id, zone, old_governor, new_governor | Warning |

**Audit entry structure:**

```rust
pub struct ThermalAuditEntry {
    /// Monotonic kernel timestamp in nanoseconds.
    pub timestamp: u64,
    /// Event classification.
    pub event: ThermalAuditEvent,
    /// Zone identifier (UTF-8 string, e.g., "cpu-thermal").
    pub zone: &'static str,
    /// PID of the agent that triggered the event (0 = kernel).
    pub pid: u32,
    /// Event-specific payload; interpretation depends on `event` variant.
    pub details: [u8; 32],
}
```

**Audit Storage:**

Thermal audit entries flow through two storage paths based on severity:

- **Ring buffer**: 1024 entries per boot cycle, held in the kernel audit ring. The most recent
  entries are retained on overflow. Accessible via `sys_audit_read()` with `AuditFilter::Thermal`.
- **Persistent storage**: Events at severity Error and above (sensor failures, critical trips) are
  additionally written to `system/audit/thermal/` in Spaces storage. These entries survive reboot
  and are available for post-incident analysis.

Critical trip events include a full snapshot of all zone temperatures at the moment of trip
crossing. This provides the complete thermal picture for root cause analysis.

---

### §11.3 Thermal Safety Invariants

The following invariants are enforced by the kernel thermal core. They are non-overridable: no
capability token, governor decision, AIRS suggestion, or agent request can bypass them. They are
not configuration — they are code-level guarantees.

**Invariant 1: Critical Trip Point is Immutable**

No API call can modify a Critical trip point — it cannot be raised, lowered, or removed. The
`set_trip_point()` function rejects any modification attempt targeting a Critical-type trip,
regardless of the caller's capability level:

```rust
pub fn set_trip_point(
    zone: &str,
    trip_idx: usize,
    new_temp_mc: i32,
    cap: &CapabilityToken,
) -> Result<(), ThermalError> {
    check_thermal_capability(cap, ThermalCapability::ThermalConfigure, Some(zone))?;

    let trip = get_trip(zone, trip_idx)?;

    // Invariant 1: Critical trips are immutable. No capability overrides this.
    if trip.trip_type == ThermalTripType::Critical {
        return Err(ThermalError::CriticalTripImmutable);
    }

    // Non-critical trips must still respect the minimum floor (§11.5).
    if new_temp_mc < TRIP_MINIMUM_FLOOR_MC {
        return Err(ThermalError::BelowMinimumFloor);
    }

    // Proceed with modification and audit.
    apply_trip_point(zone, trip_idx, new_temp_mc)?;
    audit_thermal(ThermalAuditEvent::TripPointModified { zone, trip_idx, new_temp_mc });
    Ok(())
}
```

The kernel thermal polling loop checks every zone's Critical trip independently of any governor.
Even if a governor is hung, unresponsive, or actively malicious, the kernel initiates orderly
shutdown when `current_temp_mc >= critical_trip_mc` for any zone.

**Invariant 2: Sensor Failure Defaults to Worst Case**

Hardware sensors can fail in multiple ways: stuck-at-zero, stuck-at-max, intermittent dropouts,
or complete hardware fault. A "fail-open" thermal policy (treating an unknown temperature as
safe) risks hardware destruction. AIOS uses "fail-closed" — unknown temperature is treated as
maximally dangerous:

- Sensor failure detected → thermal state for that zone immediately transitions to `Hot`.
- Failure sustained beyond 30 seconds → thermal state escalates to `Critical` → orderly shutdown.
- "Failure" includes: reading returns error, value outside `[−40 000, 150 000]` millidegrees
  Celsius, or value unchanged for longer than the sensor's rated update interval × 10.

**Invariant 3: Minimum Cooling State Floor**

Every registered cooling device has a hardware-defined minimum effective state below which
cooling is non-functional. The thermal core enforces that no governor decision, including those
made via `ThermalGovernorOverride`, can set a cooling device below its minimum effective state:

```rust
pub fn set_cooling_state(
    device: &str,
    state: u32,
    cap: &CapabilityToken,
) -> Result<(), ThermalError> {
    let dev = cooling_device(device)?;
    let effective_state = state.max(dev.min_effective_state);
    // Apply effective_state, not the raw requested state.
    dev.apply_state(effective_state)
}
```

This means a fan registered with `min_effective_state = 1` can never be commanded to state 0
(fully off) through the thermal API, regardless of who is asking.

**Invariant 4: Monotonic Escalation Under Sensor Failure**

During a sensor failure event, the inferred thermal state can only escalate:

```text
Normal → Warm → Hot → Critical
```

The thermal state cannot de-escalate until the sensor is confirmed healthy — meaning it returns
a valid, changing reading — for 10 consecutive polling intervals. This prevents a brief transient
recovery from a failing sensor from prematurely lowering thermal protection and missing the
subsequent re-failure.

---

### §11.4 Formal Verification of Thermal State Machine

The thermal state machine is a finite, bounded system with clear safety properties, making it
amenable to model checking. Given its safety-critical role, formal verification is part of the
target quality bar rather than an optional enhancement.

**Properties to Verify:**

1. **Safety (shutdown reachability)**: If `temp_mc >= critical_trip_mc` for any zone, the kernel
   initiates shutdown within one polling interval. Formally: `AG(critical → AF(shutdown))`.
2. **Liveness (recovery)**: If temperature falls below all trip points and sensors are healthy,
   the system eventually returns to `Normal` state. Formally: `AG(all_clear → AF(normal))`.
3. **Monotonicity under failure**: Sensor failure never causes thermal state to decrease.
   Formally: `AG(sensor_failed → AX(state >= prev_state))`.
4. **No dead states**: Every reachable state has at least one outgoing transition.
5. **Critical non-overridability**: No finite sequence of `set_trip_point()` calls reachable
   from any capability level can raise the Critical trip temperature.

**Verification Approach:**

The thermal state machine is modelled as a timed automaton with the polling interval as the
clock. Properties are expressed in CTL and checked with a model checker (UPPAAL or equivalent).
The verified model then generates test harnesses for use in the host-side unit test suite.

```text
Thermal State Machine (timed automaton sketch):

  States:   { Normal, Warm, Passive, Hot, Critical, SensorFailed, ShuttingDown }
  Clock:    poll_timer (reset every polling_interval_ms)

  Transitions:
    Normal      → Warm         when temp_mc >= warm_trip_mc
    Warm        → Passive      when temp_mc >= passive_trip_mc
    Passive     → Hot          when temp_mc >= hot_trip_mc
    Hot         → Critical     when temp_mc >= critical_trip_mc
    *           → SensorFailed when sensor_error detected
    SensorFailed→ Hot          immediately (invariant 2)
    SensorFailed→ Critical     when failure_duration > 30s
    Critical    → ShuttingDown immediately (invariant 1)
    Warm/Passive→ Normal       when temp_mc < warm_trip_mc − hysteresis_mc

  Invariants:
    Critical → shutdown_initiated within poll_interval (enforced by §11.3 Invariant 1)
    SensorFailed ∧ duration > 30s → Critical (enforced by §11.3 Invariant 2)
    ¬(set_trip_point(Critical)) for any caller (enforced by §11.3 Invariant 1)
```

Reference: `docs/security/static-analysis.md` for the broader formal verification methodology
applied across AIOS subsystems.

---

### §11.5 Thermal DoS Prevention

The thermal subsystem is a shared kernel resource. Without rate limiting, a malicious or buggy
agent could exhaust sensor read bandwidth, flood the audit ring, or cause rapid trip point
oscillation that destabilizes governors.

**API Rate Limits (per agent):**

| API | Limit | Rationale |
|---|---|---|
| `sys_thermal_headroom()` | 10 calls/sec | Headroom polling does not need sub-100ms granularity |
| Sensor read (POSIX `/sys/thermal/`) | 5 reads/sec | Matches typical user-space polling needs |
| Trip point modification | 1 change/min | Prevents oscillation attacks on governors |
| Cooling device registration | 4 devices/agent | Prevents device table exhaustion |
| Zone monitoring subscriptions | 8 zones/agent | Bounds notification delivery overhead |

Rate limits are enforced per-agent using token bucket counters. Exceeding a rate limit returns
`ThermalError::RateLimitExceeded` and emits a `ThermalCapabilityDenied` audit entry. Repeated
violations are reported to the AIRS anomaly detector.

**Capability Revocation:**

Revoking a `ThermalRead` or `ThermalConfigure` token via the standard capability revocation
path (`security/model/capabilities.md §3.5`) immediately terminates all thermal access for the
holding agent. Any in-flight thermal operations complete normally; subsequent calls fail with
`CapabilityRevoked`. The revocation event is written to the audit trail.

**Threat Model:**

| Threat | Mitigation |
|---|---|
| Malicious agent disables cooling entirely | Minimum cooling state floor (§11.3 Invariant 3) |
| Agent floods sensor reads to starve polling | Rate limiting: 5 reads/sec per agent |
| Agent raises trip points to unsafe values | Critical trip immutability; non-critical trips have a 50 000 mc minimum floor |
| Compromised governor ignores high temperature | Kernel-level Critical trip check runs independently of all governors |
| Agent registers unlimited cooling devices | 4-device cap per agent; table size bounded |
| Timing side-channel via temperature reads | Thermal reads are not sub-millisecond precise; rate-limited to 5/sec |
| Audit ring exhaustion via synthetic events | Audit ring is 1024-entry circular; oldest entries overwritten; critical events mirrored to persistent storage |
| Sensor spoofing via raw register access | `ThermalSensorRaw` requires trust level 4; normal agents cannot obtain it |

Reference: `docs/security/model.md §1` for the full AIOS threat model and trust level
definitions.

---

### Summary

Thermal security in AIOS rests on four mutually reinforcing pillars:

- **Capability-gated access** ensures only appropriately trusted agents can observe or modify
  thermal state, with attenuation enabling fine-grained delegation.
- **Non-overridable invariants** at the kernel level guarantee that safety-critical properties
  (Critical trip enforcement, sensor failure escalation, minimum cooling) cannot be bypassed
  regardless of software behavior above the kernel thermal core.
- **Comprehensive audit logging** provides both real-time anomaly detection surface for AIRS
  and a persistent forensic record for post-incident analysis.
- **DoS prevention** via rate limiting and resource caps ensures thermal management remains
  available and stable under adversarial agent behavior.

Together these mechanisms ensure that thermal management, despite being accessible to user-space
agents, can never be leveraged to physically damage hardware or deny thermal protection to the
system.
