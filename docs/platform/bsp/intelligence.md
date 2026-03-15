# AIOS AI-Native BSP Intelligence

Part of: [bsp.md](../bsp.md) — Board Support Package Architecture
**Related:** [platforms.md](./platforms.md) — Per-platform hardware, [../../intelligence/airs.md](../../intelligence/airs.md) — AI Runtime Service, [../thermal/intelligence.md](../thermal/intelligence.md) — Thermal intelligence

---

## §13 AI-Native BSP Intelligence

AIOS's AI-first design extends to hardware management. The BSP layer exposes hardware telemetry that AIRS and kernel-internal ML consume to optimize platform behavior at runtime. This ranges from lightweight frozen decision trees that operate without any AIRS dependency, to AIRS-driven predictions that require semantic understanding of user behavior patterns accumulated over multiple boot sessions.

The BSP intelligence layer sits below the subsystem-level intelligence described in thermal/intelligence.md and boot/intelligence.md. It deals specifically with hardware health, driver lifecycle, and platform-specific tuning — the concerns that the `Platform` trait owns. Subsystems consume the BSP's hardware telemetry feeds but do not implement them.

AI features in this document are classified by dependency tier:

| Category | AIRS required? | Runs without AIRS? | Notes |
| --- | --- | --- | --- |
| Kernel-internal ML | No | Yes | Frozen models shipped in kernel binary |
| AIRS-dependent | Yes | No | Requires Phase 9+ AIRS runtime |
| Hybrid | Partially | Degraded mode | Falls back to kernel-internal ML when AIRS is unavailable |

---

### §13.1 Predictive Driver Loading

**Category: AIRS-dependent** (requires semantic understanding of user patterns)

AIRS learns which peripheral drivers are needed based on historical usage patterns. The goal is to reduce perceived latency when a device is first used: the driver is already initialized before the hardware arrives.

- AIRS observes which USB device classes are connected, and at what point in the boot session. A USB audio interface connected at 08:00 every workday teaches AIRS to pre-initialize the USB audio class driver during early boot on weekdays.
- Boot access traces (boot/intelligence.md §16.3) extend to driver loading order — AIRS ranks drivers by their probability of being needed in the first 60 seconds after login and adjusts the service manager's initialization queue accordingly.
- Per-platform optimization: on Pi 5, AIRS pre-initializes the RP1 PCIe link during early boot if USB 3.0 devices are historically expected in the first minute. The RP1 firmware activation quirk (model.md §2.4) has a non-trivial startup latency (~80 ms); pre-triggering it eliminates that wait from the user's critical path.
- On Apple Silicon, AIRS pre-configures DART mappings for DMA-capable devices that appear in every session. DART programming requires a round-trip to the PMGR; batching multiple device mappings during pre-initialization amortizes the cost.

```rust
pub struct DriverLoadPrediction {
    /// Compatible strings of drivers to pre-load before devices are present.
    predicted_drivers: Vec<&'static str>,
    /// Confidence score in [0.0, 1.0] based on historical usage frequency.
    confidence: f32,
    /// Context that triggered the prediction (time of day, connected peripherals, etc.).
    context: BootContext,
}

pub struct BootContext {
    /// Seconds since midnight (local time via RTC or NTP).
    time_of_day_secs: u32,
    /// Previously-connected USB device classes in this session position.
    usb_class_history: u8,
    /// Bluetooth device MACs seen in the last N sessions at this boot stage.
    bt_device_history_count: u8,
}
```

Predictions are consumed by the service manager (boot/services.md §4) as hints. The service manager applies the following policy:

| Confidence | Action |
| --- | --- |
| < 0.6 | Ignored |
| 0.6 – 0.8 | Driver queued for lazy initialization (loads on first use, not proactively) |
| > 0.8 | Driver elevated to pre-initialization, runs before login shell |

The service manager never blocks on a prediction. If the predicted device never appears, the pre-loaded driver is torn down silently after a configurable idle timeout (default: 60 seconds). This ensures that a mis-prediction adds at most one wasted initialization/teardown cycle and has no visible effect on the user.

---

### §13.2 Hardware Anomaly Detection

**Category: Kernel-internal ML** (frozen decision tree, no AIRS dependency)

Lightweight runtime monitoring of hardware health indicators. These models are trained offline, shipped as static constants in the kernel binary, and run continuously on a 60-second poll cycle. They work from the first moment hardware is initialized, before AIRS loads.

**Clock drift detection.** Compare `CNTPCT_EL0` progression against expected wall-clock time (sourced from the RTC or NTP where available). Drift exceeding ±100 ppm indicates crystal oscillator degradation or thermal stress on the clock synthesizer. On Pi 4/5, the BCM2711/BCM2712 crystal oscillator is temperature-sensitive; drift above threshold correlates with die temperatures above 75°C sustained for more than 10 minutes.

**Bus error rate monitoring.** Track PCIe correctable error counts (AER registers, available on Pi 5 RP1 PCIe and Apple Silicon Thunderbolt PCIe). A rising trend of correctable errors predicts uncorrectable failures — the detector issues an advisory notification to the audit ring (security/model/operations.md §7) before data loss occurs.

**Voltage regulator health.** On platforms with PMU telemetry (Apple PMGR, Pi RP1 ADC), monitor supply rail voltage variance. Exceeding ±5% from nominal on the CPU or memory supply rails indicates PSU degradation or excessive transient loading. The kernel logs a warning; AIRS escalates to the user if the condition persists across three consecutive sessions.

**Memory error tracking.** If ECC memory is available (some Apple server configurations), track correctable error rate per DIMM bank. A rising rate triggers preemptive data migration away from the suspect bank. The kernel marks the physical memory region degraded in the buddy allocator, lowering its allocation priority.

```rust
pub struct HardwareHealthMonitor {
    /// Clock drift in parts per million (positive = fast, negative = slow).
    clock_drift_ppm: i32,
    /// Cumulative PCIe correctable error count since last boot.
    pcie_correctable_errors: u32,
    /// Voltage variance in millivolts per rail (indexed by VoltageRail enum).
    voltage_variance_mv: [i32; 4],
    /// ECC correctable error count per memory bank.
    memory_ce_count: u64,
}

pub enum HardwareAnomaly {
    /// Clock is running faster or slower than expected by more than threshold.
    ClockDrift {
        measured_ppm: i32,
        threshold_ppm: i32,
    },
    /// PCIe correctable error rate exceeds safe threshold.
    PcieErrorRate {
        errors_per_hour: u32,
        safe_threshold: u32,
    },
    /// A voltage rail is outside its nominal ±5% band.
    VoltageOutOfSpec {
        rail: &'static str,
        measured_mv: i32,
        nominal_mv: i32,
    },
    /// ECC correctable error rate suggests a degrading memory bank.
    MemoryErrors {
        bank: u8,
        rate_per_day: u32,
    },
}
```

Anomaly detection feeds a small frozen decision tree (similar in structure to thermal/intelligence.md §12.1) that classifies each anomaly by severity and determines the appropriate response:

| Severity | Condition examples | Kernel action | AIRS action |
| --- | --- | --- | --- |
| `Advisory` | Clock drift 100–500 ppm; voltage variance 3–5% | Write to audit ring | Log for trend analysis |
| `Warning` | Clock drift > 500 ppm; PCIe error rate > 10/hour | Write to audit ring + klog warn | Notify user agent |
| `Critical` | Voltage out of spec; memory errors > 100/day | Write to audit ring + throttle affected subsystem | Escalate to user immediately |

The decision tree has depth 4 — worst case is 4 comparisons per anomaly evaluation, adding negligible overhead to the 60-second poll cycle.

---

### §13.3 Adaptive Platform Tuning

**Category: AIRS-dependent + Kernel-internal ML hybrid** — runtime parameter adjustment based on workload and environmental context. The kernel-internal ML layer handles fast, reactive adjustments (sub-second response time). AIRS handles slow, learned adjustments (policy changes that take effect across sessions). When AIRS is unavailable, the BSP falls back to kernel-internal defaults for all tuning decisions.

**DVFS coordination with thermal.** The BSP provides platform-specific P-state tables to the thermal subsystem (thermal/intelligence.md §13.2). AIRS adjusts the DVFS policy based on workload type: sustain higher frequencies for short bursty workloads (interactive UI rendering, brief compilation), throttle earlier for sustained compute workloads (long inference runs, batch data processing). The distinction matters because thermal headroom is a depletable resource — spending it on a 2-second compilation burst has different value than spending it on a 10-minute video encode.

**Memory bandwidth allocation.** On Apple Silicon's unified memory architecture, the BSP exposes bandwidth partitioning controls via the PMGR. AIRS allocates GPU, CPU, and NPU memory bandwidth based on active agent requirements. When no inference workload is active, AIRS shifts bandwidth toward the CPU and display controller. When an AIRS inference request arrives, bandwidth allocation rebalances within one scheduling tick (1 ms) before the first inference kernel is dispatched.

**Peripheral power gating.** On platforms with fine-grained power domains (Pi 5 via RP1 power islands, Apple via PMGR per-domain gating), the BSP exposes per-peripheral power control through the `PlatformPower` extension trait. AIRS gates power to unused peripherals based on usage predictions: if no USB devices are connected and none are predicted within the next 5 minutes, the USB controller is powered down. Re-enabling takes ~80 ms on RP1; AIRS pre-powers the domain 100 ms before the predicted connection event to keep the latency imperceptible.

**Fan curve learning.** On Pi 5 and Apple Silicon, AIRS learns the optimal fan curve for each user's environment over the first 10 hours of operation. The learned curve accounts for ambient temperature (sensed via the platform's thermal zones), case airflow characteristics (inferred from the relationship between fan speed and die temperature), and acoustic tolerance (inferred from whether the user issues any "quieter" preference signals). The learned curve replaces the default step-wise profile from thermal/cooling.md §5.1. If the learned model produces a thermal violation, the kernel immediately falls back to the conservative default and marks the learned curve invalid for the current thermal context.

```rust
/// AIRS-provided tuning parameters, applied by the BSP at each adaptation tick.
pub struct PlatformTuningParams {
    /// Target CPU P-state index for the current workload class.
    cpu_pstate_target: u8,
    /// GPU memory bandwidth allocation fraction in percent (0–100).
    gpu_bw_fraction_pct: u8,
    /// Peripheral power gate mask: bit N set means domain N is powered on.
    peripheral_power_mask: u32,
    /// Fan speed override in RPM. 0 = use default thermal governor curve.
    fan_speed_override_rpm: u16,
}

/// The BSP exposes this telemetry feed to AIRS for policy learning.
pub struct BspTelemetry {
    /// Current P-state index for each CPU cluster.
    cpu_pstate_actual: [u8; 4],
    /// Measured memory bandwidth utilization per agent class (percent).
    mem_bw_utilization_pct: [u8; 4],
    /// Active power domain mask.
    active_power_domains: u32,
    /// Fan RPM as measured by tachometer (0 if no tachometer present).
    fan_rpm_measured: u16,
    /// Health monitor snapshot.
    health: HardwareHealthMonitor,
}
```

The telemetry feed is sampled at 1 Hz by the AIRS runtime and stored in the preferences database (intelligence/preferences.md) as a rolling window covering the last 24 hours. This history is the training signal for the learned fan curve and the DVFS policy model.

---

### §13.4 Future ISA Directions

AIOS's BSP model is designed for aarch64, but the abstractions are deliberately ISA-agnostic. The `Platform` trait, `DeviceTree` parser, `BootInfo` struct, and all subsystems above the HAL are written against abstract device handles. Porting to a new ISA requires a new `kernel/src/arch/<isa>/` directory and updated `Platform` implementations — no changes to schedulers, IPC, storage, or the AI layer.

**RISC-V (riscv64gc).**

A RISC-V port would add `kernel/src/arch/riscv64/` with: `boot.S` (machine-mode startup, privilege-level transitions to supervisor mode), `uart.rs` (8250/16550 UART, the standard on RISC-V development boards), `plic.rs` (Platform-Level Interrupt Controller — analogous to GIC, different register layout), and `timer.rs` (`mtime`/`mtimecmp` instead of ARM Generic Timer).

Key differences from aarch64:

- RISC-V uses SBI (Supervisor Binary Interface) for cross-core communication and timer, replacing PSCI. `sbi_hart_start()` corresponds to PSCI `CPU_ON`. The SBI call ABI uses `ecall` from supervisor mode with function IDs in `a7`.
- The RISC-V PLIC has a two-level claim/complete cycle per interrupt, compared to GIC's acknowledge/end-of-interrupt. The `InterruptController` abstraction hides this difference from the rest of the kernel.
- RISC-V has no hardware equivalent of ARM's `WFE`/`SEV` for parking secondary cores. Secondary cores spin on an SBI-provided mechanism (`sbi_hart_suspend`) until `sbi_hart_start()` is called.
- Target boards: SiFive HiFive Unmatched, StarFive VisionFive 2, QEMU `riscv64 virt`. All provide a standard DTB — the existing `dtb.rs` parser works unmodified.

The new platform file would be:

```rust
// kernel/src/platform/visionfive2.rs
pub struct VisionFive2Platform;

impl Platform for VisionFive2Platform {
    fn name(&self) -> &'static str { "StarFive VisionFive 2 (JH7110)" }

    fn init_uart(&self, dt: &DeviceTree) -> Result<Uart> {
        // 8250/16550 UART — new driver in kernel/src/arch/riscv64/uart.rs
        let base = dt.uart_base.unwrap_or(JH7110_UART0_BASE);
        Uart::init_16550(base)
    }

    fn init_interrupts(&self, dt: &DeviceTree) -> Result<InterruptController> {
        // RISC-V PLIC — new driver in kernel/src/arch/riscv64/plic.rs
        let plic_base = dt.plic_base.expect("PLIC base required");
        InterruptController::init_plic(plic_base)
    }

    fn init_timer(&self, dt: &DeviceTree, ic: &InterruptController) -> Result<Timer> {
        // mtime/mtimecmp via SBI — new driver in kernel/src/arch/riscv64/timer.rs
        Timer::init_riscv(dt.timer_ppi, ic)
    }
    // ... remaining methods
}
```

**x86_64.** An x86 port would add `kernel/src/arch/x86_64/` with: `boot.asm` (UEFI entry with `%rcx`/`%rdx` ABI instead of `x0`/`x1`), `uart.rs` (8250/16550 I/O-port UART, accessed via `in`/`out` instructions), `apic.rs` (Local APIC + I/O APIC, xAPIC and x2APIC modes), and `hpet.rs` (HPET for calibration; LAPIC timer for the per-core scheduler tick after calibration).

Key differences from aarch64:

- x86 uses ACPI (not DTB) for device discovery. A minimal ACPI parser is needed for MADT (interrupt routing and APIC topology), FADT (power management ports), and MCFG (PCIe MMIO base address). The `DeviceTree` abstraction would gain a parallel `AcpiTable` path.
- The interrupt controller is APIC (Local APIC + I/O APIC), not GIC. The `InterruptController` abstraction handles this behind the same interface.
- x86 page table format (CR3 → PML4 → PDPT → PD → PT) uses the same four-level depth as ARM but with different bit positions for NX, Accessed, Dirty, and caching attributes. `mm/pgtable.rs` would need a new backend parameterized on ISA.
- The UEFI boot path is identical — same stub code structure, same `BootInfo` struct, different target triple (`x86_64-unknown-uefi`) and entry point calling convention.

**ISA independence summary** — components and their ISA dependency:

| Component | ISA-dependent? | Notes |
| --- | --- | --- |
| Platform trait | No | Same 7 init methods and extension traits |
| DeviceTree parser | No | DTB format is ISA-independent |
| BootInfo struct | No | Same fields; UEFI ABI differs (argument registers) |
| Memory manager (allocators) | No | Buddy, slab, and frame allocator are pure Rust |
| Page table implementation | Yes | Bit layout differs by ISA and privilege mode |
| Scheduler (run queues, policy) | No | Thread context register-file size differs |
| IPC | No | Syscall ABI (SVC vs ECALL vs SYSCALL) differs |
| Capability system | No | Pure data structures, no ISA dependency |
| Storage (Block Engine) | No | Entirely above the HAL |
| AI layer (AIRS, kernel-internal ML) | No | Pure Rust floating-point, no ISA intrinsics required |
| Boot assembly | Yes | New `boot.S` (or `boot.asm`) per ISA |
| Interrupt controller driver | Yes | GIC vs PLIC vs APIC |
| Timer driver | Yes | ARM Generic Timer vs mtime vs LAPIC |
| UART driver | Partially | PL011 on ARM; 8250/16550 on RISC-V/x86 |

---

### §13.5 Future Platforms

Near-term aarch64 platforms where BSP work would be minimal:

**ARM SBSA servers.** The ARM Server Base System Architecture mandates GICv3, a standard UART, PCIe, and ACPI (rather than DTB). An SBSA-compliant server requires only a new `Platform` implementation that reads device addresses from ACPI MADT and MCFG tables rather than the DTB. GIC, timer, and PL011 drivers are reused unchanged. This is the platform class closest to QEMU virt in terms of driver work required.

**Embedded microcontrollers (Cortex-M/R).** These are ARMv7-M/R, not aarch64 — a separate kernel build target would be required. AIOS's AI capabilities would not meaningfully fit within the memory constraints of Cortex-M class devices. Not planned.

**Automotive (Cortex-A with safety island).** Mixed-criticality systems pair a high-performance Cortex-A cluster (running AIOS) with a Cortex-R safety island (running a certified RTOS). The interface between the two domains is typically a shared memory region with a mailbox protocol. AIOS would run on the Cortex-A side with a BSP that includes a mailbox driver for safety-island communication. Relevant for AIRS-assisted ADAS workloads but requires ASIL-certified boot sequences — a research topic for a later phase.

**Custom AI accelerator boards.** Boards with dedicated NPUs (Google Coral TPU, Hailo-8, RockChip NPU) extend the `Platform` trait via a `PlatformNpu` extension trait. AIRS binds to the NPU for accelerated inference via the same extension trait discovery pattern used for USB, WiFi, and thermal (model.md §3.6.1).

```rust
/// Extension trait for platforms with dedicated NPU hardware.
pub trait PlatformNpu: Platform {
    /// Power on the NPU and load firmware. Called during AIRS initialization.
    fn init_npu(&self, dt: &DeviceTree) -> Result<NpuDevice>;

    /// NPU telemetry: utilization, temperature, and error counts.
    fn npu_telemetry(&self) -> NpuTelemetry;
}

pub struct NpuTelemetry {
    /// Compute utilization in percent (0–100).
    utilization_pct: u8,
    /// Die temperature in degrees Celsius.
    temperature_c: i8,
    /// Correctable error count since last reset.
    error_count: u32,
}
```

AIRS queries for `PlatformNpu` at startup and, if found, routes inference requests to the NPU rather than the CPU. The BSP intelligence telemetry feed (§13.3) gains a `npu_utilization` channel so AIRS can observe NPU health alongside CPU, GPU, and memory metrics. On platforms without a dedicated NPU, AIRS falls back to CPU-side inference using the ONNX Runtime target for aarch64.
