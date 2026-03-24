# AIOS Compute Intelligence

Part of: [compute.md](../compute.md) — Kernel Compute Abstraction
**Related:** [budget.md](./budget.md) — Thermal-aware budget enforcement, [registry.md](./registry.md) — Utilization tracking

-----

## 13. Cross-Device Thermal Coupling

On modern SoCs, CPU, GPU, and NPU share a die (or package) and a thermal budget. GPU compute at high utilization heats the die, reducing CPU thermal headroom. NPU inference contributes heat that affects GPU clock frequency. The kernel must reason about these thermal interactions holistically.

### 13.1 Thermal Zone Registration

Every compute device registers a `ThermalZone` with the thermal framework ([thermal/zones.md](../../platform/thermal/zones.md) §2):

```rust
/// Thermal zone registration for a compute device.
///
/// The thermal framework tracks per-zone temperatures and applies
/// cooling policies. The compute abstraction adds compute-specific
/// cooling actions: reducing compute quotas and rejecting low-priority
/// workloads before hardware throttling engages.
pub struct ComputeThermalZone {
    /// The compute device this zone monitors.
    pub device_id: ComputeDeviceId,

    /// Current temperature reading in millidegrees Celsius.
    pub temperature_mc: i32,

    /// Temperature thresholds for policy transitions.
    pub trip_points: ComputeTripPoints,

    /// Coupling coefficients to other compute devices on the same die.
    /// Entry (other_device, coefficient): when this device's utilization
    /// increases by 10%, the other device's effective temperature increases
    /// by coefficient * 10 millidegrees.
    pub coupling: Vec<(ComputeDeviceId, f32)>,
}

pub struct ComputeTripPoints {
    /// Temperature at which compute quota begins reducing (proactive).
    pub warm: i32,      // e.g., 65000 mc (65°C)
    /// Temperature at which quota is severely limited.
    pub hot: i32,       // e.g., 80000 mc (80°C)
    /// Temperature at which all non-system compute is rejected.
    pub critical: i32,  // e.g., 95000 mc (95°C)
}
```

### 13.2 Coupling Coefficients

Platform-specific coupling coefficients are measured or derived from thermal simulations:

```text
Platform          Device Pair           Coefficient    Meaning
──────────        ───────────           ───────────    ──────────────────────────
Pi 5              CPU ↔ GPU             0.7            GPU at 100% adds ~7°C to CPU zone
Pi 5              CPU ↔ CPU (cluster)   1.0            Same thermal zone
Apple M-series    CPU ↔ GPU             0.4            Better thermal design
Apple M-series    CPU ↔ ANE             0.2            ANE runs cool (2W TDP)
Apple M-series    GPU ↔ ANE             0.3            Moderate coupling
QEMU              All pairs             0.0            Virtual — no thermal coupling
```

The coupling coefficients feed into the thermal framework's policy engine. When the GPU thermal zone reaches `warm`, the policy engine checks coupled zones and may proactively reduce CPU compute budget or defer background NPU inference to prevent the entire SoC from reaching `hot`.

### 13.3 Thermal-Aware Compute Routing

AIRS uses thermal state from the compute registry to make routing decisions:

```text
Scenario: Agent requests FP16 inference.
  GPU thermal: Hot (80°C)
  CPU thermal: Nominal (55°C)
  NPU thermal: Warm (68°C)

Decision: Route to CPU despite lower throughput.
  Rationale: GPU is thermally constrained. NPU is warming.
  CPU has thermal headroom. FP16 inference on CPU NEON is
  viable (slower but won't cause throttling).
```

This routing logic lives in AIRS, not the kernel. The kernel provides thermal state and enforces quotas; AIRS makes the intelligent routing decision based on the holistic thermal picture.

-----

## 14. Kernel-Internal ML for Compute

These techniques require no LLM inference and no AIRS runtime. They are compiled into the kernel as fixed-size statistical models with O(1) per-observation cost. Total state overhead: under 8 KB for all compute-related ML.

### 14.1 Utilization Prediction

An EWMA (exponentially weighted moving average) predictor estimates future compute device utilization based on recent history. The kernel uses this to:

- **Pre-warm devices**: Wake an idle NPU 100ms before predicted inference activity (saves ~50ms cold-start latency).
- **Pre-allocate buffers**: Allocate activation buffers before AIRS requests them.
- **Adjust quotas**: If sustained high utilization is predicted, proactively tighten quotas before thermal limits are reached.

```rust
pub struct UtilizationPredictor {
    /// EWMA of utilization (α = 0.1, updated every 100ms).
    pub ewma: f32,
    /// Variance estimate for confidence bounds.
    pub variance: f32,
    /// Timestamp of last update.
    pub last_update: Timestamp,
}

impl UtilizationPredictor {
    pub fn predict_next(&self) -> (f32, f32) {
        // (predicted utilization, 95% confidence bound)
        let stddev = self.variance.sqrt();
        (self.ewma, self.ewma + 2.0 * stddev)
    }
}
```

**State overhead:** 12 bytes per compute device. Updated every 100ms.

### 14.2 Power-Performance Curve Modeling

Each compute device has a non-linear relationship between utilization and power draw. The kernel maintains a piecewise-linear approximation:

```rust
pub struct PowerCurve {
    /// (utilization, power_mw) pairs defining the curve.
    /// Populated at boot from platform tables, refined at runtime
    /// from actual measurements.
    pub points: [(f32, u32); 8],
}

impl PowerCurve {
    /// Estimate power draw at a given utilization.
    pub fn estimate_power(&self, utilization: f32) -> u32 {
        // Piecewise linear interpolation between known points
        for window in self.points.windows(2) {
            if utilization <= window[1].0 {
                let t = (utilization - window[0].0)
                    / (window[1].0 - window[0].0);
                return (window[0].1 as f32
                    + t * (window[1].1 as f32 - window[0].1 as f32))
                    as u32;
            }
        }
        self.points.last().map(|p| p.1).unwrap_or(0)
    }
}
```

**State overhead:** 64 bytes per compute device. Updated on each workload completion.

### 14.3 Device Health Scoring

A simple z-score anomaly detector tracks compute device health:

- **Error rate trending**: If a device's error rate exceeds 2σ from its historical mean, the kernel logs a warning and reduces its reliability score in the compute registry. AIRS deprioritizes unhealthy devices.
- **Latency drift**: If submission-to-completion latency drifts beyond 2σ, this may indicate firmware bugs, thermal throttling, or hardware degradation.
- **Timeout rate**: If a device times out on workloads more than 1% of the time, the kernel marks it as degraded.

```rust
pub struct DeviceHealthScorer {
    /// Error count in current window.
    pub errors: u32,
    /// Total submissions in current window.
    pub submissions: u32,
    /// EWMA of error rate.
    pub error_rate_ewma: f32,
    /// Variance of error rate for z-score calculation.
    pub error_rate_variance: f32,
    /// Current health score (0.0 = dead, 1.0 = perfect).
    pub health_score: f32,
}
```

**State overhead:** 24 bytes per compute device. Updated on each workload completion.

-----

## 17. Future Directions

### 17.1 Disaggregated Compute

Future AIOS versions may support network-attached accelerators — GPUs or NPUs accessible over a local network (e.g., Thunderbolt-attached eGPU, NVMe-over-Fabrics compute). The ComputeTopology (§6) already supports asymmetric latency between devices, making disaggregated compute a topology extension rather than an architectural change.

### 17.2 AIRS-Dependent Compute Intelligence

When AIRS is available (Phase 30+), compute scheduling gains advanced capabilities:

- **Workload classification**: Given a compute workload's characteristics (memory access pattern, arithmetic intensity, data types), predict which device class is most efficient using a lightweight gradient-boosted tree trained on historical workload-device-performance triples.
- **Model-to-device routing**: Match quantized model formats to hardware capabilities. An INT4 model routes to NPU if available, then GPU, then CPU NEON — but with learned cost models that account for the specific model architecture, not just data types.
- **Predictive power management**: Wake accelerators before expected workload arrives based on user activity patterns (e.g., the user opens the conversation bar at 9am daily — pre-load the model and warm the NPU at 8:55am).
- **Cross-device model partitioning**: For large models that don't fit in a single accelerator's memory, AIRS partitions the model across multiple devices (e.g., embedding layers on NPU, attention layers on GPU, output head on CPU). The compute topology (§6) provides the interconnect latency data needed to optimize partition boundaries.

### 17.3 Formal Verification

The ComputeGrant lifecycle (§12.2) and buffer ownership protocol (§10.3) have small, well-defined state spaces suitable for formal verification:

- **TLA+ model**: Verify that buffer ownership transitions never produce a state where both CPU and device believe they own a buffer simultaneously.
- **Verus proofs**: Verify that ComputeGrant TTL enforcement never allows a grant to be used after expiry.
- **Capability attenuation**: Verify that attenuated ComputeAccess tokens never grant more access than the parent token.

### 17.4 Hardware Security Modules

Future compute devices may include hardware security features:

- **Encrypted compute**: NPUs that operate on encrypted model weights (never in plaintext, even during inference). The kernel would extend ComputeGrant with encryption key handles.
- **Attestation**: Accelerators that attest their firmware integrity to the kernel before receiving workloads. The kernel would verify attestation reports before issuing ComputeGrants.
- **Memory encryption**: Hardware memory encryption for compute buffers (ARM MTE for tagging, SMMU nested translation for address-space isolation).
