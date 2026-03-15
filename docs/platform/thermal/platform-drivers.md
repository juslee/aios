# AIOS Thermal Platform Drivers

Part of: [thermal.md](../thermal.md) — Thermal Management
**Related:** [zones.md](./zones.md) — Thermal zones & sensors, [cooling.md](./cooling.md) — Cooling devices & governors

---

## §8 Per-Platform Thermal Drivers

Each platform implements the `PlatformThermal` trait (hal.md §17), providing thermal zone
discovery, sensor reading, and cooling device registration. Platform-specific code lives in
`kernel/src/platform/` with one file per supported board or SoC family. This separation keeps
the thermal core (`kernel/src/thermal/`) fully platform-agnostic: it operates on the abstract
`ThermalZone` and `CoolingDevice` types and never reaches into MMIO registers
or firmware mailboxes directly.

All temperatures exchanged through the `PlatformThermal` interface are in millidegrees Celsius
(`i32`), matching the convention used throughout zones.md §2 and cooling.md §4.

---

### §8.1 QEMU Virtual Thermal

The QEMU platform driver provides a fully software-emulated thermal environment. Its purpose
is to enable testing and development of the thermal framework — governors, trip-point logic,
and cooling integration — without access to real hardware that heats up.

**Emulation Model:**

Temperature is derived from CPU utilization measured by the scheduler. The formula gives
a linear ramp from ambient at idle to near-critical at full load:

```text
temp_mdegc = base_temp + (cpu_util_percent * thermal_gain)

Where:
  base_temp    = 35_000  (35°C ambient)
  thermal_gain =    500  (500 mdegC per 1% utilization)

Examples:
  0% util  →  35_000 mdegC (35°C)
  50% util →  60_000 mdegC (60°C)
  100%util →  85_000 mdegC (85°C)
```

The CPU utilization counter is updated by the scheduler tick handler (sched/scheduler.rs) and
stored as a shared atomic so the thermal driver can read it without holding a lock.

**Trip Points:**

| Name     | Temperature   | Action                                  |
|----------|---------------|-----------------------------------------|
| Passive  | 80,000 mdegC  | Reduce virtual DVFS frequency           |
| Critical | 95,000 mdegC  | Trigger orderly kernel shutdown         |

**Cooling Device:**

A virtual DVFS device exposes four frequency steps mapped to standard cooling states:

| Cooling State | Frequency  |
|---------------|------------|
| 0 (none)      | 2400 MHz   |
| 1 (mild)      | 1800 MHz   |
| 2 (moderate)  | 1200 MHz   |
| 3 (maximum)   | 600 MHz    |

The step-wise governor is used for the virtual sensor because there is no energy model to
justify the more expensive power-optimal governor.

**Implementation:**

```rust
pub struct QemuThermalDriver {
    base_temp_mdegc: i32,       // 35_000
    thermal_gain: i32,          // 500 per percent CPU utilization
    cpu_utilization: AtomicU32, // 0–100, updated by scheduler
}

impl PlatformThermal for QemuPlatform {
    fn thermal_zones(&self) -> Vec<ThermalZone> {
        vec![ThermalZone {
            name: "cpu-thermal",
            sensor_address: 0, // virtual; no real MMIO
            zone_type: ThermalZoneType::Cpu,
            polling_interval: Duration::from_secs(2),
            trip_points: &QEMU_CPU_TRIPS,
            coupling: None,
        }]
    }

    fn read_temperature(&self, zone: &ThermalZone) -> Result<i32> {
        let util = self.thermal.cpu_utilization.load(Ordering::Relaxed);
        Ok(self.thermal.base_temp_mdegc
            + (util as i32 * self.thermal.thermal_gain))
    }

    fn cooling_devices(&self) -> Vec<CoolingDevice> {
        vec![CoolingDevice::virtual_dvfs(&QEMU_FREQ_STEPS)]
    }
}
```

**Temperature Injection for Testing:**

The QEMU driver exposes a test-only interface that overrides the utilization-based formula
with an injected temperature value. This allows unit tests to drive the thermal framework
through exact trip-point thresholds without needing to spin CPUs to specific load levels.
The injection API is compiled out when the `thermal-tests` feature flag is absent.

```rust
#[cfg(feature = "thermal-tests")]
impl QemuThermalDriver {
    /// Override computed temperature with a fixed value for test purposes.
    /// Pass `None` to restore utilization-based computation.
    pub fn inject_temperature(&self, temp_mdegc: Option<i32>) {
        // stores Some(value) in injected_temp atomic pair
    }
}
```

---

### §8.2 Raspberry Pi 4 (BCM2711)

The BCM2711 SoC contains a single on-die temperature sensor (TSENS) located in the thermal
management block. There is no fan connector on the Raspberry Pi 4 board; cooling is passive
only, relying on heatsinks and airflow across the SoC package.

**Sensor Hardware:**

- TSENS base address: `0x7d5d_2200` (mapped via device tree or platform constant)
- The raw ADC reading is converted to millidegrees using BCM2711 TRM formula:

```text
temp_mdegc = 410_040 - (adc_val * 487)

Calibration:
  Accuracy:  ±3°C (factory trimmed per-chip)
  Range:     -40°C to 125°C ADC valid output
```

- The calibration offset from OTP fuses is read once at driver init and stored in
  `Pi4ThermalDriver::calibration_offset_mdegc`. Per-chip variation is within the ±3°C
  accuracy band and does not require additional correction in software.

**Polling Strategy:**

- Normal interval: 2 s (low overhead when cool)
- Elevated interval: 500 ms when temperature exceeds the Passive trip point
- The thermal core handles interval switching; the platform driver only supplies the
  two interval constants via `PlatformThermal::polling_config()`.

**Trip Points:**

| Trip Name | Temperature   | Action                                         |
|-----------|---------------|------------------------------------------------|
| Passive   | 80,000 mdegC  | VideoCore firmware throttles CPU frequency     |
| Critical  | 85,000 mdegC  | Kernel initiates orderly system shutdown       |

The 5°C window between Passive and Critical is intentionally narrow. The VideoCore firmware
acts aggressively on the Passive event; if the kernel still reaches Critical, a shutdown is
the only safe recourse to prevent silicon damage.

**Cooling — VideoCore Firmware Coordination:**

The Pi 4 does not expose direct CPU frequency control to the kernel. Frequency scaling is
managed by the VideoCore firmware. The kernel driver communicates cooling requests through the
VideoCore mailbox property interface:

- Mailbox base: `0x3F00_B880` (Bcm2711 peripheral base `0xFE00_0000` + `0x00B880`)
- Relevant mailbox tags:

| Tag ID     | Name             | Direction  | Description                      |
|------------|------------------|------------|----------------------------------|
| 0x00030002 | GET_CLOCK_RATE   | Kernel ← FW | Read current ARM clock rate     |
| 0x00038002 | SET_CLOCK_RATE   | Kernel → FW | Request ARM clock rate change   |
| 0x00030047 | GET_TEMPERATURE  | Kernel ← FW | Alternative temperature read    |

The firmware retains authority over the final clock rate. If the chip is already above the
Passive threshold when the kernel sends `SET_CLOCK_RATE`, the firmware applies the more
restrictive of the two values. The kernel reads back the applied rate via `GET_CLOCK_RATE`
after any SET operation and updates its cooling state accordingly.

**DVFS Frequency Steps:**

| Cooling State | Frequency  |
|---------------|------------|
| 0 (none)      | 1500 MHz   |
| 1 (mild)      | 1000 MHz   |
| 2 (moderate)  | 750 MHz    |
| 3 (maximum)   | 600 MHz    |

**Implementation:**

```rust
pub struct Pi4ThermalDriver {
    tsens_base: *mut u32,           // 0x7d5d_2200 (mapped MMIO)
    mailbox_base: *mut u32,         // 0x3F00_B880 (mapped MMIO)
    calibration_offset_mdegc: i32,  // per-chip factory offset from OTP
}

impl Pi4ThermalDriver {
    fn read_adc(&self) -> u32 {
        // SAFETY: tsens_base is a valid MMIO mapping of the BCM2711 TSENS block.
        // The register is read-only. Concurrent reads are safe; no locking needed.
        unsafe { core::ptr::read_volatile(self.tsens_base) & 0x3FF }
    }
}

impl PlatformThermal for Pi4Platform {
    fn read_temperature(&self, _zone: &ThermalZone) -> Result<i32> {
        let adc = self.thermal.read_adc();
        let raw = 410_040 - (adc as i32 * 487);
        Ok(raw + self.thermal.calibration_offset_mdegc)
    }

    fn set_cooling_state(
        &self,
        _device: &CoolingDevice,
        state: u32,
    ) -> Result<()> {
        let freq_hz = PI4_FREQ_STEPS[state as usize];
        self.thermal.mailbox_set_clock_rate(freq_hz)
    }
}
```

Reference: power-management.md §9.1 for the full Pi 4 platform power model.

---

### §8.3 Raspberry Pi 5 (BCM2712)

The Raspberry Pi 5 introduces a second thermal zone for the RP1 south-bridge, a dedicated
fan connector with PWM control, and a higher-capacity CPU that can sustain heavier workloads.
The thermal model is correspondingly more complex than Pi 4.

**Thermal Zones:**

| Zone         | Sensor              | Address             | Passive   | Critical  |
|--------------|---------------------|---------------------|-----------|-----------|
| cpu-thermal  | BCM2712 TSENS       | `0x1001_7000`       | 80,000    | 85,000    |
| gpu-thermal  | RP1 SMC mailbox     | SMC mailbox         | 75,000    | 85,000    |

The GPU zone uses a slightly lower Passive threshold (75,000 mdegC) because the RP1
south-bridge runs cooler than the main SoC die and benefits from earlier intervention to
stay within its lower rated junction temperature.

**Fan Hardware:**

The official Raspberry Pi Active Cooler connects to the board's 4-pin fan header:

- GPIO 45: PWM1 channel (fan speed control)
- GPIO 46: TACH input (fan RPM feedback, optional)
- PWM frequency: 25 kHz (inaudible to most humans)
- Duty cycle 0 % = fan off; 100 % = maximum speed

**Fan Speed Curve:**

```rust
const PI5_FAN_CURVE: [(i32, u32); 5] = [
    (50_000,   0),   // below 50°C:  fan off
    (55_000,  30),   // at 55°C:     30% duty cycle
    (60_000,  50),   // at 60°C:     50% duty cycle
    (70_000,  75),   // at 70°C:     75% duty cycle
    (75_000, 100),   // at 75°C:     100% duty cycle
];
```

Linear interpolation is used between table entries. The bang-bang governor governs the fan
device. A separate PID governor governs the DVFS device; the two governors operate
independently on the same thermal zone.

**Ramp Timing — Noise Reduction:**

Rapid fan speed oscillation ("hunting") is audible and annoying. The driver enforces:

- Ramp-up delay: 2 s minimum between fan speed increases
- Ramp-down delay: 10 s minimum between fan speed decreases

This asymmetry errs on the side of keeping the fan running slightly longer after a thermal
event rather than cycling on and off repeatedly.

```rust
pub struct Pi5ThermalDriver {
    cpu_tsens_base: *mut u32,   // 0x1001_7000 (mapped MMIO)
    rp1_smc_base: *mut u32,     // RP1 SMC mailbox (mapped MMIO)
    fan_pwm_base: *mut u32,     // PWM1 controller (mapped MMIO)
    fan_state: FanState,
}

struct FanState {
    current_duty: u32,
    target_duty: u32,
    last_change_tick: u64,   // scheduler tick at last duty change
    ramp_up_ticks: u64,      // 2_000 ticks (2 s at 1 kHz)
    ramp_down_ticks: u64,    // 10_000 ticks (10 s at 1 kHz)
}
```

**Thermal Coupling — CPU / GPU:**

Heat generated in the BCM2712 CPU die and the RP1 south-bridge is conducted through the
shared PCB substrate and the Active Cooler heatsink. This creates measurable cross-zone
coupling. The platform driver registers the following coupling coefficients (see zones.md §3.4
for the coupling model definition):

| Source Zone  | Target Zone  | Coupling Coefficient |
|--------------|--------------|----------------------|
| cpu-thermal  | gpu-thermal  | 0.15                 |
| gpu-thermal  | cpu-thermal  | 0.20                 |

The asymmetry reflects that the RP1 runs cooler and dissipates less power; GPU heat has a
larger relative impact on the smaller thermal mass of the CPU-adjacent die area.

**DVFS — CPU and GPU:**

CPU frequency is controlled via the BCM2712 DVFS interface:

| Cooling State | CPU Frequency |
|---------------|---------------|
| 0 (none)      | 2400 MHz      |
| 1 (mild)      | 1800 MHz      |
| 2 (moderate)  | 1500 MHz      |
| 3 (maximum)   | 1000 MHz      |

GPU (RP1) frequency is managed independently through the RP1 SMC mailbox:

| Cooling State | GPU Frequency |
|---------------|---------------|
| 0 (none)      | 1000 MHz      |
| 1 (mild)      | 750 MHz       |
| 2 (maximum)   | 500 MHz       |

```rust
impl PlatformThermal for Pi5Platform {
    fn thermal_zones(&self) -> Vec<ThermalZone> {
        vec![
            ThermalZone {
                name: "cpu-thermal",
                zone_type: ThermalZoneType::Cpu,
                polling_interval: Duration::from_secs(2),
                trip_points: &PI5_CPU_TRIPS,
                coupling: Some(ThermalCoupling {
                    target: "gpu-thermal",
                    coefficient: 150, // 0.15 in milliunit
                }),
                ..Default::default()
            },
            ThermalZone {
                name: "gpu-thermal",
                zone_type: ThermalZoneType::Gpu,
                polling_interval: Duration::from_secs(2),
                trip_points: &PI5_GPU_TRIPS,
                coupling: Some(&[ThermalCoupling {
                    source_zone: "gpu-thermal",
                    coefficient: 0.20,
                }]),
                ..Default::default()
            },
        ]
    }
}
```

Reference: power-management.md §9.2 for the full Pi 5 platform power model.

---

### §8.4 Apple Silicon

Apple Silicon SoCs (M1, M2, M3, M4 series) expose thermal sensors exclusively through the
Apple System Management Controller (SMC). The SMC coprocessor handles all thermal sensing,
fan control, and power rail management internally; the application processor interacts with
it through a dedicated mailbox channel.

**Thermal Zones:**

| Zone        | SMC Keys            | Passive    | Hot        | Critical   |
|-------------|---------------------|------------|------------|------------|
| cpu-thermal | TC0P (P-core agg.)  | 95,000     | 100,000    | 105,000    |
|             | TC0E (E-core agg.)  |            |            |            |
| gpu-thermal | TGXP                | 90,000     | 95,000     | 105,000    |
| npu-thermal | TANP                | 90,000     | 95,000     | 105,000    |

Apple Silicon has substantially higher thermal limits than ARM SBCs. The 95°C Passive
threshold for CPU reflects the wide thermal margin engineered into the 3 nm and 4 nm process
nodes used in M-series chips. A Hot trip at 100°C triggers an intermediate response before
the Critical shutdown at 105°C.

**SMC Interface:**

The SMC communicates via a memory-mapped mailbox. SMC keys are 4-byte ASCII identifiers:

```rust
pub struct AppleThermalDriver {
    smc_base: *mut u32,    // SMC mailbox base (platform-specific, from device tree)
    pmgr_base: *mut u32,   // Power Manager base for DVFS requests
    core_count: CoreCount,
}

struct CoreCount {
    p_cores: u32,   // High-performance cores
    e_cores: u32,   // Efficiency cores
}
```

Key SMC operations:

```rust
impl AppleThermalDriver {
    /// Read a temperature sensor by 4-character SMC key.
    /// Returns temperature in millidegrees Celsius.
    fn smc_read_temperature(&self, key: [u8; 4]) -> Result<i32> {
        // Write key to SMC command register, read response from data register.
        // SMC encodes temperature as a signed 16.16 fixed-point Celsius value;
        // multiply fractional part by 1000 to produce mdegC.
        todo!()
    }

    /// Send an advisory fan speed hint to the SMC.
    /// The SMC may disregard the hint if its own thermal logic requires higher speed.
    fn smc_set_fan_hint(&self, fan_id: u8, target_rpm: u16) -> Result<()> {
        todo!()
    }
}
```

Common SMC thermal keys:

| Key  | Sensor                          | Unit       |
|------|---------------------------------|------------|
| TC0P | CPU die (P-core aggregate)      | Celsius SP78 |
| TC0E | CPU die (E-core aggregate)      | Celsius SP78 |
| TGXP | GPU die                         | Celsius SP78 |
| TANP | Neural Engine (NPU)             | Celsius SP78 |
| TaLP | Left fan intake air             | Celsius SP78 |
| TaRP | Right fan intake air            | Celsius SP78 |

**Per-Core Thermal Awareness:**

P-cores operate at higher frequencies and voltages than E-cores and run hotter under
equivalent workloads. When aggregate CPU thermal pressure exceeds the Warm level (defined
in zones.md §3.2), the thermal core signals the scheduler to:

- Prefer E-cores for new thread placement
- Migrate runnable Normal-class threads from P-cores to E-cores
- Reserve P-cores for threads with RT or Interactive scheduling class

This thermal-aware placement is advisory, not mandatory. The scheduler makes final placement
decisions accounting for load balance, cache affinity, and power policy jointly.

**DVFS via Apple PMGR:**

Apple Silicon does not expose raw PLL or voltage regulator registers to kernel software.
Frequency and voltage scaling is performed by the Power Manager (PMGR) coprocessor in
response to performance state requests:

- The kernel writes a desired performance level (0 = minimum, N = maximum) to a PMGR
  register for each cluster
- PMGR selects the voltage-frequency operating point from a table burned into the SoC
  and performs the transition atomically from the application processor's perspective
- Performance state transitions typically complete within 100 µs

The number of available P-states varies by SoC variant and is discovered at boot by
reading the PMGR performance table header.

**Fan Management Policy:**

The SMC manages fans autonomously. The kernel participates in an advisory capacity only:

- MacBook Air (fanless): no fan device registered; thermal headroom relies on DVFS alone
- MacBook Pro: dual fans, SMC-managed; kernel sends `smc_set_fan_hint` during sustained load
- Mac mini / Mac Studio: single or dual fans, same advisory model
- Mac Pro: multiple independently-controlled fans; SMC arbitrates between kernel hints and
  its own internal measurements

The SMC retains veto power. It will always spin fans faster than any kernel hint if its own
internal temperature readings warrant it. Kernel hints are guaranteed to be respected only
as a floor (i.e., the SMC will not spin fans slower than hinted while the kernel hint is active).

```rust
impl PlatformThermal for ApplePlatform {
    fn read_temperature(&self, zone: &ThermalZone) -> Result<i32> {
        let key = thermal_zone_to_smc_key(zone);
        self.thermal.smc_read_temperature(key)
    }

    fn set_cooling_state(
        &self,
        device: &CoolingDevice,
        state: u32,
    ) -> Result<()> {
        match device.device_type {
            CoolingType::Dvfs => {
                // Request performance state via PMGR.
                // state 0 = max cooling = lowest perf level.
                let perf_level = device.max_state - state;
                self.thermal.pmgr_set_perf_level(device.cluster_id, perf_level)
            }
            CoolingType::Fan => {
                // Advisory hint only; SMC may override.
                let rpm = fan_state_to_rpm(state);
                self.thermal.smc_set_fan_hint(device.fan_id, rpm)
            }
            _ => Ok(()),
        }
    }
}
```

Reference: power-management.md §9.4 for the full Apple Silicon platform power model.

---

### §8.5 ARM SCMI Integration

The ARM System Control and Management Interface (SCMI, ARM specification DEN0056) provides
a standardized protocol for an OS agent to communicate with a platform management firmware
(System Control Processor, SCP). SCMI abstracts sensor access, DVFS, and power domain control
behind a uniform message-based API.

**Why SCMI Matters:**

Without SCMI, each SoC family requires a bespoke kernel driver to reach its temperature
sensors and frequency controllers. SCMI-enabled platforms (increasingly common on server-class
and recent embedded ARM SoCs) allow a single `ScmiThermalDriver` to replace multiple
platform-specific drivers, reducing code, reducing attack surface, and ensuring consistent
behavior.

**Transport Layer:**

SCMI messages are exchanged through a shared memory channel (typically a small SRAM region)
with a doorbell interrupt to signal the SCP. The transport details are hidden behind the
`ScmiTransport` abstraction:

```rust
pub struct ScmiSensorDriver {
    transport: ScmiTransport,
    sensor_count: u32,
    sensor_descriptors: Vec<ScmiSensorDescriptor>,
}

pub struct ScmiSensorDescriptor {
    pub id: u32,
    pub name: [u8; 16],          // null-terminated ASCII
    pub unit: ScmiSensorUnit,    // Celsius, milliCelsius, etc.
    pub multiplier: i32,         // base-10 exponent for fixed-point result
}
```

**Sensor Protocol (Protocol ID 0x15):**

Key messages exchanged during initialization and polling:

| Message ID | Name                      | Direction       | Description                               |
|------------|---------------------------|-----------------|-------------------------------------------|
| 0x00       | PROTOCOL_VERSION          | Agent → Platform | Negotiate protocol version               |
| 0x03       | SENSOR_DESCRIPTION_GET    | Agent → Platform | Enumerate available sensors              |
| 0x04       | SENSOR_TRIP_POINT_CONFIG  | Agent → Platform | Register trip points with firmware       |
| 0x06       | SENSOR_READING_GET        | Agent → Platform | Read current sensor value                |
| 0x07       | SENSOR_CONFIG_SET         | Agent → Platform | Enable or disable a sensor               |
| 0x05       | SENSOR_TRIP_POINT_EVENT   | Platform → Agent | Async notification when trip fires       |

Trip point events from SCMI firmware eliminate the need for software polling when the
platform supports async notifications. The thermal core registers a trip-point event handler
and switches the zone to interrupt-driven mode (polling_interval = 0) when the SCMI firmware
advertises notification support.

**Sensor Reading Conversion:**

SCMI sensor values arrive as 64-bit signed integers with a unit and multiplier:

```rust
impl ScmiSensorDriver {
    /// Convert a raw SCMI sensor reading to millidegrees Celsius.
    fn scmi_to_mdegc(
        &self,
        descriptor: &ScmiSensorDescriptor,
        raw: i64,
    ) -> i32 {
        // Apply base-10 multiplier to arrive at degrees, then scale to mdegC.
        // e.g., multiplier = -3 (i.e., 10^-3 = millidegrees) → raw value is already mdegC.
        // e.g., multiplier =  0 (degrees) → multiply raw by 1000.
        let scale = 10_i64.pow(descriptor.multiplier.unsigned_abs());
        let mdegc = if descriptor.multiplier >= 0 {
            raw * scale * 1000
        } else {
            raw * 1000 / scale
        };
        mdegc as i32
    }
}
```

**DVFS Protocol (Protocol ID 0x13):**

SCMI performance domains map directly onto the `CoolingDevice` abstraction (cooling.md §4.1).
Each performance domain exposes a list of performance levels; the thermal governor selects a
cooling state and the SCMI driver translates it to a `PERF_LEVEL_SET` message:

| Message ID | Name               | Description                                |
|------------|--------------------|--------------------------------------------|
| 0x04       | PERF_DESCRIBE_LEVELS | Enumerate performance levels for a domain |
| 0x07       | PERF_LEVEL_SET     | Request a specific performance level       |
| 0x08       | PERF_LEVEL_GET     | Read the currently active performance level |

**Device Tree Binding:**

SCMI thermal sensors are described in the device tree as follows:

```text
scmi: scmi@2b1f0000 {
    compatible = "arm,scmi";
    mbox-names = "tx", "rx";
    mboxes = <&mailbox 0 0>, <&mailbox 0 1>;
    shmem = <&scmi_shmem_tx>, <&scmi_shmem_rx>;
    #address-cells = <1>;
    #size-cells = <0>;

    scmi_sensors: protocol@15 {
        reg = <0x15>;
        #thermal-sensor-cells = <1>;
    };

    scmi_perf: protocol@13 {
        reg = <0x13>;
    };
};

thermal-zones {
    cpu_thermal: cpu-thermal {
        polling-delay-passive = <250>;
        polling-delay = <2000>;
        thermal-sensors = <&scmi_sensors 0>;
        /* ... trip points ... */
    };
};
```

The thermal core reads the device tree via `dtb.rs` and instantiates an `ScmiSensorDriver`
for each SCMI thermal zone descriptor found. See device-model.md §5 for the bus abstraction
patterns used to register SCMI as a virtual bus type.

**Benefits over Platform-Specific Drivers:**

- Firmware handles ADC calibration, filtering, and smoothing — the kernel receives clean,
  pre-calibrated readings without raw ADC conversion code per platform.
- A single `ScmiThermalDriver` serves any SCMI-capable SoC, eliminating per-board driver
  proliferation.
- SCMI trip point registration offloads wake-up latency to firmware: the SCP can fire a
  doorbell interrupt the moment a threshold is crossed rather than waiting for the kernel
  polling timer.
- Power domain and performance level discovery through SCMI enables the thermal framework
  to adapt to the number and configuration of DVFS domains at runtime rather than requiring
  compile-time constants per platform.

---

### §8.6 Platform Driver Registration

Platform drivers register themselves with the thermal core during the platform init phase of
boot (boot/services.md §4). The registration sequence is:

```rust
// In platform init (e.g., qemu.rs, pi5.rs):
fn init_thermal(platform: &mut dyn PlatformThermal, thermal_core: &mut ThermalCore) {
    for zone in platform.thermal_zones() {
        thermal_core.register_zone(zone);
    }
    for device in platform.cooling_devices() {
        thermal_core.register_cooling_device(device);
    }
    // Bind zones to cooling devices per platform policy.
    platform.bind_cooling_devices(thermal_core);
}
```

The `bind_cooling_devices` hook allows each platform to express its own cooling topology:
which cooling devices service which zones, in what priority order, and with which governor.
The thermal core then drives the bound devices through the common governor interface defined
in cooling.md §5.

**Platform Driver Summary:**

| Platform           | Zones | Fan | DVFS | Protocol      | Governor         |
|--------------------|-------|-----|------|---------------|------------------|
| QEMU (virtual)     | 1     | No  | Virtual (4-step) | Direct MMIO  | Step-wise        |
| Raspberry Pi 4     | 1     | No  | VideoCore mailbox | Mailbox     | Step-wise        |
| Raspberry Pi 5     | 2     | PWM | BCM2712 + RP1 SMC | MMIO + SMC  | PID + bang-bang  |
| Apple Silicon      | 3     | SMC advisory | PMGR P-states | SMC mailbox | PID              |
| SCMI (generic)     | N     | Optional | SCMI perf domains | SCMI transport | Power-optimal  |
