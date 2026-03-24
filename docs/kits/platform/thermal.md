# Thermal Kit

**Layer:** Platform | **Crate:** `aios_thermal` | **Architecture:** [`docs/platform/thermal.md`](../../platform/thermal.md)

## 1. Overview

Thermal Kit monitors hardware temperature sensors, manages trip-point-driven cooling
responses, and exports thermal state to the scheduler and other subsystems for
thermal-aware decision making. It couples temperature sensors to cooling devices
(CPU frequency scaling, fan control, compute gating) through configurable governor
policies, ensuring the system stays within safe thermal limits while maximizing
performance within those constraints.

Most application developers do not interact with Thermal Kit directly. The scheduler
automatically reduces CPU budgets when thermal pressure rises, and Compute Kit throttles
GPU workloads in response to thermal state changes. However, agents that run sustained
compute-intensive workloads -- video encoding, ML training, large compilations -- can
benefit from querying thermal headroom to make intelligent decisions about work scheduling.
A video encoder might reduce output resolution when thermal headroom is low rather than
waiting for the system to forcibly throttle it. A background indexer might defer work
entirely when thermal state reaches the warning threshold.

Thermal Kit integrates with [Power Kit](./power.md) for coordinated power-thermal
management. The active power profile influences thermal trip point thresholds: the
Efficiency profile lowers trip points to reduce heat output, while the Performance profile
allows higher temperatures before throttling begins. AIRS provides predictive thermal
control that anticipates temperature rises from workload patterns and begins proactive
cooling before trip points are reached, resulting in smoother performance than reactive
throttling alone.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use core::time::Duration;

/// A named thermal monitoring region with sensor polling and filtering.
///
/// Each zone tracks one or more temperature sensors and reports a filtered
/// temperature reading. Zones are defined by the BSP (board support package)
/// and correspond to physical heat sources (CPU, GPU, SoC, ambient).
pub trait ThermalZone {
    /// The zone's unique identifier.
    fn id(&self) -> &ThermalZoneId;

    /// Human-readable zone name (e.g., "cpu-thermal", "gpu-thermal").
    fn name(&self) -> &str;

    /// Current filtered temperature in millidegrees Celsius.
    fn temperature_mc(&self) -> i32;

    /// Current temperature as floating-point degrees Celsius.
    fn temperature_c(&self) -> f32 {
        self.temperature_mc() as f32 / 1000.0
    }

    /// The trip points configured for this zone, ordered by temperature.
    fn trip_points(&self) -> &[TripPoint];

    /// The current thermal state (derived from which trip points are active).
    fn state(&self) -> ThermalState;

    /// Subscribe to thermal state changes in this zone.
    fn on_state_change(&self, handler: Box<dyn Fn(ThermalState) + Send>) -> SubscriptionId;

    /// The cooling devices bound to this zone.
    fn cooling_devices(&self) -> &[Box<dyn CoolingDevice>];
}

/// A temperature threshold that triggers a cooling response.
///
/// Trip points form an escalation ladder: as temperature rises through
/// successive trip points, increasingly aggressive cooling is applied.
pub struct TripPoint {
    /// Temperature threshold in millidegrees Celsius.
    pub temperature_mc: i32,

    /// Hysteresis in millidegrees (prevents oscillation at the threshold).
    pub hysteresis_mc: i32,

    /// The severity level of this trip point.
    pub severity: TripSeverity,

    /// The cooling action taken when this trip point is crossed.
    pub action: CoolingAction,
}

/// Trip point severity levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TripSeverity {
    /// Advisory only; no automatic throttling.
    Passive,
    /// Active cooling engages (fan speed increase, mild DVFS).
    Active,
    /// Aggressive throttling to prevent thermal damage.
    Hot,
    /// Emergency shutdown to protect hardware.
    Critical,
}

/// Thermal state exported to scheduler and other subsystems.
///
/// Consumers use this to make thermal-aware decisions without needing
/// to understand the details of zone configuration or cooling policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThermalState {
    /// All temperatures nominal. Full performance available.
    Normal,
    /// Approaching trip points. Consider reducing workload.
    Warm,
    /// Passive trip point crossed. Scheduler reducing CPU budget.
    Throttled,
    /// Hot trip point crossed. Aggressive throttling active.
    Critical,
    /// Emergency threshold reached. Shutdown imminent.
    Emergency,
}

/// A cooling device that can reduce temperature in a thermal zone.
///
/// Cooling devices are controlled by thermal governors. Application code
/// does not control them directly but can observe their state.
pub trait CoolingDevice {
    /// The device's unique identifier.
    fn id(&self) -> &CoolingDeviceId;

    /// Human-readable name (e.g., "cpu-freq", "fan0", "gpu-gating").
    fn name(&self) -> &str;

    /// The type of cooling this device provides.
    fn cooling_type(&self) -> CoolingType;

    /// Current cooling level (0 = off, max_level = maximum cooling).
    fn current_level(&self) -> u32;

    /// Maximum cooling level supported.
    fn max_level(&self) -> u32;
}

/// Types of cooling mechanisms.
#[derive(Debug, Clone, Copy)]
pub enum CoolingType {
    /// CPU/GPU frequency and voltage scaling.
    Dvfs,
    /// Fan speed control.
    Fan,
    /// Disabling compute units entirely.
    ComputeGating,
    /// Reducing display brightness.
    DisplayDimming,
    /// Reducing radio transmit power.
    RadioThrottle,
}

/// Thermal governor policy engine that decides how to respond to temperature changes.
///
/// The system ships with three built-in governors. Custom governors can be
/// registered by system agents.
pub trait ThermalGovernor {
    /// The governor's identifier.
    fn id(&self) -> GovernorId;

    /// Evaluate current temperatures and return cooling decisions.
    fn evaluate(
        &self,
        zones: &[&dyn ThermalZone],
    ) -> Vec<CoolingDecision>;
}

/// Built-in governor types.
#[derive(Debug, Clone, Copy)]
pub enum GovernorId {
    /// Incremental steps: increase cooling one level per trip point.
    StepWise,
    /// PID controller: proportional-integral-derivative feedback loop.
    Pid,
    /// Binary: full cooling above trip, no cooling below.
    BangBang,
    /// Custom governor registered by a system agent.
    Custom(u32),
}
```

## 3. Usage Patterns

**Check thermal headroom before intensive work:**

```rust
use aios_thermal::{ThermalKit, ThermalState};

let cpu_zone = ThermalKit::zone("cpu-thermal")?;

match cpu_zone.state() {
    ThermalState::Normal => {
        println!("Thermal headroom OK -- starting video encode");
        start_encode(Quality::High).await?;
    }
    ThermalState::Warm => {
        println!("System warm -- encoding at reduced quality");
        start_encode(Quality::Medium).await?;
    }
    ThermalState::Throttled | ThermalState::Critical => {
        println!("Thermal pressure high -- deferring encode");
        schedule_for_later()?;
    }
    ThermalState::Emergency => {
        // System is shutting down imminently; do nothing
    }
}
```

**Subscribe to thermal state changes:**

```rust
use aios_thermal::{ThermalKit, ThermalState};

let cpu_zone = ThermalKit::zone("cpu-thermal")?;

cpu_zone.on_state_change(Box::new(|state| {
    match state {
        ThermalState::Throttled => {
            // Reduce animation frame rate to lower CPU load
            set_target_fps(30);
        }
        ThermalState::Normal => {
            // Restore full frame rate
            set_target_fps(60);
        }
        _ => {}
    }
}));
```

**Query all thermal zones and their temperatures:**

```rust
use aios_thermal::ThermalKit;

for zone in ThermalKit::all_zones()? {
    println!(
        "  {} : {:.1} C ({:?})",
        zone.name(),
        zone.temperature_c(),
        zone.state(),
    );
    for trip in zone.trip_points() {
        println!(
            "    Trip {:?} at {:.1} C (hysteresis {:.1} C)",
            trip.severity,
            trip.temperature_mc as f32 / 1000.0,
            trip.hysteresis_mc as f32 / 1000.0,
        );
    }
}
```

> **Common Mistakes**
>
> - **Polling temperature in a tight loop.** Thermal zones update at their configured
>   polling interval (typically 1-5 seconds). Polling more frequently returns the same
>   cached value and wastes CPU. Use `on_state_change()` instead.
> - **Ignoring thermal state during sustained workloads.** Agents that push the CPU hard
>   without checking thermal headroom will be forcibly throttled by the scheduler, resulting
>   in unpredictable performance drops. Proactive adaptation is smoother.
> - **Trying to control cooling devices directly.** Cooling devices are managed by thermal
>   governors, not by application code. Use `ThermalKit::request_thermal_headroom()` to
>   express your needs; the governor decides how to meet them.

## 4. Integration Examples

**Thermal Kit + Compute Kit -- GPU thermal awareness:**

```rust
use aios_thermal::{ThermalKit, ThermalState};
use aios_compute::{ComputeKit, ComputeBudget};

let gpu_zone = ThermalKit::zone("gpu-thermal")?;

gpu_zone.on_state_change(Box::new(|state| {
    match state {
        ThermalState::Throttled => {
            // Request reduced GPU compute budget
            ComputeKit::set_budget(ComputeBudget::percent(50));
        }
        ThermalState::Normal => {
            ComputeKit::set_budget(ComputeBudget::percent(100));
        }
        _ => {}
    }
}));
```

**Thermal Kit + Power Kit -- coordinated power-thermal management:**

```rust
use aios_thermal::{ThermalKit, ThermalState};
use aios_power::{PowerKit, PowerProfileId};

// When thermal state is critical, suggest switching to Efficiency profile
ThermalKit::on_any_zone_state(ThermalState::Critical, Box::new(|| {
    PowerKit::suggest_profile(PowerProfileId::Efficiency);
}));

// When power profile changes, adjust thermal policy
PowerKit::on_profile_change(Box::new(|profile| {
    match profile.id() {
        PowerProfileId::Performance => {
            ThermalKit::request_thermal_headroom(ThermalHeadroom::Aggressive);
        }
        PowerProfileId::Efficiency => {
            ThermalKit::request_thermal_headroom(ThermalHeadroom::Conservative);
        }
        _ => {
            ThermalKit::request_thermal_headroom(ThermalHeadroom::Balanced);
        }
    }
}));
```

**Thermal Kit + Scheduler -- thermal-aware task placement:**

```rust
use aios_thermal::ThermalKit;

// The scheduler consumes thermal state automatically, but agents can
// query it to make informed decisions about task submission.

let zones = ThermalKit::all_zones()?;
let coolest_zone = zones.iter()
    .filter(|z| z.name().starts_with("cpu"))
    .min_by_key(|z| z.temperature_mc());

if let Some(zone) = coolest_zone {
    println!(
        "Coolest CPU zone: {} ({:.1} C) -- scheduling work there",
        zone.name(),
        zone.temperature_c(),
    );
}
```

## 5. Capability Requirements

| Method | Required Capability | Default Grant |
| --- | --- | --- |
| `ThermalKit::zone` | None | Always available (read-only) |
| `ThermalKit::all_zones` | None | Always available (read-only) |
| `ThermalZone::temperature_mc` | None | Always available (read-only) |
| `ThermalZone::state` | None | Always available (read-only) |
| `ThermalZone::on_state_change` | None | Always available |
| `ThermalKit::request_thermal_headroom` | `ThermalHint` | System agents and trusted apps |
| `ThermalKit::register_governor` | `ThermalControl` | System agents only |
| `ThermalKit::override_trip_point` | `ThermalControl` | System agents only |

**Agent manifest example:**

```toml
[capabilities.optional]
ThermalHint = "Request thermal headroom for sustained compute workloads"
```

Most agents do not need any thermal capabilities -- reading thermal state is always
available. Only agents that want to influence thermal policy (e.g., a game engine
requesting aggressive headroom) need the `ThermalHint` capability.

## 6. Error Handling

```rust
/// Errors returned by Thermal Kit operations.
#[derive(Debug)]
pub enum ThermalError {
    /// The requested thermal zone does not exist.
    ZoneNotFound(String),

    /// The temperature sensor returned an invalid reading.
    SensorReadFailed { zone: ThermalZoneId, reason: String },

    /// The required capability was not granted.
    CapabilityDenied(String),

    /// The cooling device failed to change state.
    CoolingDeviceError { device: CoolingDeviceId, reason: String },

    /// The requested governor is not registered.
    GovernorNotFound(GovernorId),

    /// Trip point override was rejected (safety constraint violation).
    SafetyConstraintViolation { reason: String },

    /// The thermal subsystem is not available on this platform.
    NotAvailable,
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `ZoneNotFound` | Use `ThermalKit::all_zones()` to discover available zones |
| `SensorReadFailed` | Transient; retry after polling interval. Zone reports last known value |
| `CoolingDeviceError` | Governor falls back to next-best cooling option automatically |
| `SafetyConstraintViolation` | Cannot override critical trip points; request rejected by design |
| `NotAvailable` | Platform has no thermal sensors (e.g., minimal QEMU); skip thermal logic |

## 7. Platform & AI Availability

**Platform support:**

| Platform | CPU Thermal | GPU Thermal | Fan Control | DVFS | Notes |
| --- | --- | --- | --- | --- | --- |
| QEMU virt | Simulated | No | No | No | Fixed temperature for testing |
| Raspberry Pi 4 | BCM2711 sensor | VideoCore VI | GPIO fan header | arm-cpufreq | Single thermal zone |
| Raspberry Pi 5 | BCM2712 sensor | VideoCore VII | Active cooler | arm-cpufreq | Built-in fan connector |
| Apple Silicon | Per-cluster sensors | Unified memory | System fan | P/E core switching | Multiple fine-grained zones |

**AIRS-enhanced features:**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Predictive cooling | Anticipates temperature rise from workload patterns; pre-cools | Reactive trip-point-only |
| Workload fingerprinting | Identifies thermal signature of workloads for proactive scheduling | No workload awareness |
| Cross-zone coupling | Models heat transfer between adjacent zones (CPU heating GPU) | Independent zone management |
| Governor selection | Selects optimal governor policy per workload type | Fixed governor per zone |
| Anomaly detection | Detects sensor drift or cooling device failure | Manual inspection only |

**Feature detection:**

```rust
use aios_thermal::ThermalKit;

if ThermalKit::is_available() {
    let zones = ThermalKit::all_zones()?;
    println!("{} thermal zones available", zones.len());
    for zone in &zones {
        println!("  {} : {:.1} C", zone.name(), zone.temperature_c());
    }
} else {
    println!("Thermal monitoring not available (QEMU or unsupported platform)");
}
```

**Implementation phase:** Phase 28+. Thermal Kit depends on [Memory Kit](../kernel/memory.md)
and [Capability Kit](../kernel/capability.md). It is consumed by the scheduler,
[Compute Kit](../kernel/compute.md), and all subsystems with thermal-sensitive behavior.

---

*See also: [Power Kit](./power.md) | [Compute Kit](../kernel/compute.md) | [Scheduler](../kernel/scheduler.md)*
