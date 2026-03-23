# Power Kit

**Layer:** Platform | **Crate:** `aios_power` | **Architecture:** [`docs/platform/power-management.md`](../../platform/power-management.md)

## 1. Overview

Power Kit manages system power states, wake lock arbitration, battery monitoring, and power
profile selection. It provides a policy layer that every other Kit consults before entering
low-power modes, ensuring that no single subsystem can silently prevent sleep or drain the
battery without accountability. Power Kit makes power consumption a first-class observable
property of the system -- every wake lock is attributed to an agent, visible in Inspector,
and subject to capability enforcement.

Application developers interact with Power Kit when their agent needs to keep the system
awake during a long-running operation (file sync, media playback, background computation)
or when they want to adapt behavior based on battery state. A music player holds an audio
wake lock so playback continues when the screen dims. A sync agent checks the battery
level before starting a large upload. A game queries the active power profile to decide
between high-fidelity and battery-saver rendering. If your agent has no background
processing needs and does not need battery awareness, you do not need Power Kit directly --
the system manages power states transparently.

Power Kit coordinates closely with [Thermal Kit](./thermal.md) to balance performance
against thermal and power constraints. When the battery is low, Power Kit may request
Thermal Kit to lower thermal trip points, reducing heat output and extending battery life.
The active `PowerProfile` (Performance, Balanced, Efficiency) influences scheduling
decisions, display brightness, radio duty cycles, and compute budgets across the entire
system.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use core::time::Duration;

/// System power state machine managing transitions between power levels.
///
/// Power state transitions are coordinated system-wide: all subsystems
/// receive advance notice and can veto transitions they are not ready for.
pub trait PowerState {
    /// Return the current power state.
    fn current(&self) -> PowerLevel;

    /// Request a transition to a new power state.
    /// Returns an error if a wake lock or subsystem prevents the transition.
    fn request_transition(
        &mut self,
        target: PowerLevel,
        cap: &CapabilityHandle,
    ) -> Result<(), PowerError>;

    /// Subscribe to power state transition events.
    fn on_transition(&self, handler: Box<dyn Fn(PowerTransition) + Send>) -> SubscriptionId;

    /// Return the time spent in each power state since boot.
    fn state_durations(&self) -> PowerStateDurations;
}

/// Power level hierarchy from highest to lowest power consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PowerLevel {
    /// Full performance, all subsystems active.
    Active,
    /// Screen dimmed, reduced CPU frequency, radios in low-power mode.
    Idle,
    /// CPU halted, RAM in self-refresh, wake on interrupt only.
    Suspend,
    /// State saved to disk, power fully removed from RAM.
    Hibernate,
    /// Orderly shutdown in progress.
    ShuttingDown,
}

/// A capability-gated lock that prevents the system from entering low-power states.
///
/// Wake locks are reference-counted and attributed to the requesting agent.
/// They appear in Inspector and are subject to timeout enforcement.
pub trait WakeLock {
    /// The agent that holds this lock.
    fn holder(&self) -> &AgentId;

    /// The reason declared when the lock was acquired.
    fn reason(&self) -> &str;

    /// The type of wake lock (full, partial, proximity).
    fn lock_type(&self) -> WakeLockType;

    /// How long this lock has been held.
    fn held_duration(&self) -> Duration;

    /// Release the wake lock, allowing the system to sleep.
    fn release(self) -> Result<(), PowerError>;
}

/// Wake lock types controlling which subsystems stay awake.
#[derive(Debug, Clone, Copy)]
pub enum WakeLockType {
    /// Keep CPU and screen active.
    Full,
    /// Keep CPU active, allow screen to dim/off.
    Partial,
    /// Keep CPU active only while the proximity sensor detects the user.
    Proximity,
}

/// Battery monitoring for charge level, health, and power source tracking.
pub trait BatteryMonitor {
    /// Current battery charge percentage (0-100).
    fn charge_percent(&self) -> u8;

    /// Whether the device is currently charging.
    fn is_charging(&self) -> bool;

    /// The current power source.
    fn power_source(&self) -> PowerSource;

    /// Estimated time to full charge (if charging) or to empty (if discharging).
    fn time_remaining(&self) -> Option<Duration>;

    /// Battery health as a percentage of design capacity.
    fn health_percent(&self) -> u8;

    /// Current charge/discharge rate in milliwatts (negative = discharging).
    fn power_draw_mw(&self) -> i32;

    /// Subscribe to battery level crossing a threshold.
    fn on_threshold(
        &self,
        threshold: BatteryThreshold,
        handler: Box<dyn Fn(BatteryStatus) + Send>,
    ) -> SubscriptionId;
}

/// Named power policy profiles with per-subsystem knobs.
///
/// The active profile is set by the user or automatically by Context Kit
/// based on usage patterns and battery level.
pub trait PowerProfile {
    /// The profile's identifier.
    fn id(&self) -> PowerProfileId;

    /// Human-readable profile name.
    fn name(&self) -> &str;

    /// CPU frequency scaling policy for this profile.
    fn cpu_governor(&self) -> CpuGovernor;

    /// Display brightness multiplier (0.0 to 1.0).
    fn brightness_factor(&self) -> f32;

    /// Whether background agent activity is restricted.
    fn background_restricted(&self) -> bool;

    /// Maximum GPU compute budget percentage (0-100).
    fn gpu_budget_percent(&self) -> u8;

    /// Radio power-save mode configuration.
    fn radio_power_save(&self) -> RadioPowerSave;
}

/// Well-known power profile identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfileId {
    Performance,
    Balanced,
    Efficiency,
    Custom(u32),
}
```

## 3. Usage Patterns

**Hold a wake lock during a background sync:**

```rust
use aios_power::{PowerKit, WakeLockType};
use aios_capability::CapabilityKit;

let cap = CapabilityKit::request("WakeLockAcquire")?;

// Acquire a partial wake lock (CPU stays on, screen can turn off)
let lock = PowerKit::acquire_wake_lock(
    WakeLockType::Partial,
    "Syncing 2.3 GB to cloud storage",
    Duration::from_secs(30 * 60), // max 30-minute timeout
    &cap,
)?;

// Perform the sync...
sync_to_cloud().await?;

// Release the lock when done (also released on drop)
lock.release()?;
```

**Adapt behavior based on battery state:**

```rust
use aios_power::{PowerKit, BatteryThreshold};

let battery = PowerKit::battery_monitor();

// Check battery before starting expensive work
if battery.charge_percent() < 20 && !battery.is_charging() {
    println!("Battery low -- deferring background indexing");
    return Ok(());
}

// Register for low-battery notification
battery.on_threshold(
    BatteryThreshold::Below(15),
    Box::new(|status| {
        println!(
            "Battery critical: {}% (~{} remaining)",
            status.charge_percent,
            format_duration(status.time_remaining),
        );
    }),
);
```

**Query and respond to the active power profile:**

```rust
use aios_power::{PowerKit, PowerProfileId};

let profile = PowerKit::active_profile();
match profile.id() {
    PowerProfileId::Efficiency => {
        // Reduce animation frame rate, defer non-essential work
        set_target_fps(30);
        defer_background_tasks();
    }
    PowerProfileId::Performance => {
        set_target_fps(60);
        enable_all_visual_effects();
    }
    _ => {
        set_target_fps(60);
    }
}

// Subscribe to profile changes
PowerKit::on_profile_change(Box::new(|new_profile| {
    println!("Power profile changed to: {}", new_profile.name());
}));
```

> **Common Mistakes**
>
> - **Forgetting to release wake locks.** Always use RAII patterns or explicit `release()`.
>   Wake locks held beyond their declared timeout are forcibly released and the agent is
>   flagged by the behavioral monitor.
> - **Acquiring `Full` wake locks when `Partial` suffices.** Full locks prevent the screen
>   from turning off. Use Partial for background work that does not need the display.
> - **Not setting a timeout.** Wake locks without timeouts can drain the battery if the
>   agent crashes. Always provide a reasonable maximum duration.
> - **Ignoring power profiles.** Users in Efficiency mode expect reduced power consumption.
>   Agents that ignore the active profile and run at full performance will be noticed.

## 4. Integration Examples

**Power Kit + Thermal Kit -- coordinated power-thermal management:**

```rust
use aios_power::{PowerKit, PowerProfileId};
use aios_thermal::{ThermalKit, ThermalState};

// When switching to Efficiency profile, also lower thermal thresholds
PowerKit::on_profile_change(Box::new(|profile| {
    if profile.id() == PowerProfileId::Efficiency {
        // Thermal Kit lowers trip points to reduce heat and power
        ThermalKit::request_thermal_headroom(ThermalHeadroom::Conservative);
    }
}));

// When thermal pressure is high, Power Kit can suggest profile changes
ThermalKit::on_state_change(Box::new(|state| {
    if state == ThermalState::Critical {
        PowerKit::suggest_profile(PowerProfileId::Efficiency);
    }
}));
```

**Power Kit + Audio Kit -- audio wake lock for media playback:**

```rust
use aios_power::{PowerKit, WakeLockType};
use aios_audio::{AudioKit, AudioSession, SessionIntent};

// Audio Kit acquires wake locks automatically for active playback sessions.
// Apps rarely need to manage this directly. Shown here for clarity.

let session = AudioKit::create_session(SessionIntent::Playback)?;
let cap = aios_capability::CapabilityKit::request("WakeLockAcquire")?;

// Lock is held for the duration of playback
let lock = PowerKit::acquire_wake_lock(
    WakeLockType::Partial,
    "Audio playback active",
    Duration::from_secs(4 * 3600), // 4-hour max for podcasts
    &cap,
)?;

session.play(audio_source).await?;
lock.release()?;
```

**Power Kit + Context Kit -- automatic profile selection:**

```rust
use aios_power::PowerKit;
use aios_context::{ContextKit, UserActivity};

// Context Kit signals when user activity changes; Power Kit adjusts profile
ContextKit::on_activity_change(Box::new(|activity| {
    match activity {
        UserActivity::Gaming | UserActivity::VideoEditing => {
            PowerKit::set_profile(PowerProfileId::Performance);
        }
        UserActivity::Reading | UserActivity::Idle => {
            PowerKit::set_profile(PowerProfileId::Efficiency);
        }
        _ => {
            PowerKit::set_profile(PowerProfileId::Balanced);
        }
    }
}));
```

## 5. Capability Requirements

| Method | Required Capability | Default Grant |
| --- | --- | --- |
| `PowerKit::acquire_wake_lock` | `WakeLockAcquire` | Prompt user |
| `PowerState::request_transition` | `PowerStateControl` | System agents only |
| `PowerKit::set_profile` | `PowerProfileChange` | Prompt user |
| `BatteryMonitor::charge_percent` | None | Always available |
| `BatteryMonitor::is_charging` | None | Always available |
| `BatteryMonitor::on_threshold` | None | Always available |
| `PowerKit::active_profile` | None | Always available |
| `PowerKit::on_profile_change` | None | Always available |
| `PowerKit::list_wake_locks` | `PowerInspect` | System agents and Inspector |

**Agent manifest example:**

```toml
[capabilities.required]
WakeLockAcquire = "Keep device awake during background sync"

[capabilities.optional]
PowerProfileChange = "Suggest power profile based on workload"
```

## 6. Error Handling

```rust
/// Errors returned by Power Kit operations.
#[derive(Debug)]
pub enum PowerError {
    /// The required capability was not granted.
    CapabilityDenied(String),

    /// The wake lock could not be acquired (too many active locks).
    WakeLockLimitExceeded { current: u32, max: u32 },

    /// The wake lock was forcibly released due to timeout.
    WakeLockTimedOut { reason: String, held_for: Duration },

    /// The requested power state transition was vetoed by a subsystem.
    TransitionVetoed { target: PowerLevel, veto_reason: String },

    /// Battery information is unavailable (desktop, no battery hardware).
    NoBattery,

    /// The requested power profile does not exist.
    ProfileNotFound(PowerProfileId),

    /// The wake lock has already been released.
    AlreadyReleased,

    /// A power state transition is already in progress.
    TransitionInProgress,
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `WakeLockLimitExceeded` | Release existing locks before acquiring new ones |
| `WakeLockTimedOut` | Re-acquire if work is still in progress; consider longer timeout |
| `TransitionVetoed` | Check which subsystem vetoed; wait for it to finish |
| `NoBattery` | Treat as always-plugged-in; skip battery-dependent logic |
| `TransitionInProgress` | Wait for the current transition to complete |

## 7. Platform & AI Availability

**Platform support:**

| Platform | Battery | Suspend | Hibernate | DVFS | Notes |
| --- | --- | --- | --- | --- | --- |
| QEMU virt | Emulated | Limited | No | No | Basic power state simulation |
| Raspberry Pi 4 | External UPS only | Yes (PSCI) | No | Yes (arm-cpufreq) | No native battery |
| Raspberry Pi 5 | External UPS only | Yes (PSCI) | No | Yes (arm-cpufreq) | No native battery |
| Apple Silicon | Yes | Yes | Yes | Yes (P/E cores) | Full power management |

**AIRS-enhanced features:**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Battery prediction | ML-based discharge curve modeling with usage-pattern awareness | Linear extrapolation |
| Wake lock auditing | Identifies agents holding excessive wake locks | Manual Inspector review |
| Profile selection | Learns optimal profile transitions from user patterns | Manual or Context Kit rules |
| Sleep timing | Predicts optimal idle-to-suspend delay | Fixed 2-minute timeout |
| Charge scheduling | Suggests charging windows to preserve battery health | No charge management |

**Feature detection:**

```rust
use aios_power::PowerKit;

if PowerKit::battery_available() {
    let battery = PowerKit::battery_monitor();
    println!("Battery: {}%", battery.charge_percent());
} else {
    println!("No battery (AC-powered device)");
}

println!("Active profile: {}", PowerKit::active_profile().name());
println!("Power state: {:?}", PowerKit::power_state().current());
```

**Implementation phase:** Phase 27+. Power Kit depends on [Memory Kit](../kernel/memory.md)
and [Capability Kit](../kernel/capability.md). It is consumed by virtually all other Kits
for power-aware behavior.

---

*See also: [Thermal Kit](./thermal.md) | [Context Kit](../intelligence/context.md) | [Audio Kit](./audio.md) | [Compute Kit](../kernel/compute.md)*
