# AIOS Compute Device Classification

Part of: [compute.md](../compute.md) — Kernel Compute Abstraction
**Related:** [registry.md](./registry.md) — ComputeRegistry, [security.md](./security.md) — Capability-gated compute access

-----

## 3. ComputeDevice Trait

Every compute-capable device in AIOS — whether CPU, GPU, NPU, DSP, or custom ASIC — implements the `ComputeDevice` trait. This trait provides a uniform interface for the kernel to query capabilities, track utilization, and route compute commands. It is implemented **alongside** the `Driver` trait ([device-model/discovery.md](../device-model/discovery.md) §6) — a GPU driver implements both `Driver` (for device lifecycle) and `ComputeDevice` (for compute operations).

### 3.1 Trait Definition

```rust
/// A compute-capable device that can execute workloads beyond CPU threads.
///
/// Implemented by accelerator drivers alongside the base Driver trait.
/// The kernel uses this interface to populate the ComputeRegistry (§5),
/// enforce compute budgets (§7), and gate access via capabilities (§11).
pub trait ComputeDevice: Send + Sync {
    /// Unique compute device identifier, linked to the device model's DeviceId.
    fn compute_id(&self) -> ComputeDeviceId;

    /// Classification of this compute device.
    fn compute_class(&self) -> ComputeClass;

    /// Detailed capability descriptor — what this device can do.
    fn capabilities(&self) -> &ComputeCapabilityDescriptor;

    /// Current utilization as a fraction (0.0 = idle, 1.0 = fully loaded).
    /// Updated by the driver after each workload completion or on a polling interval.
    fn utilization(&self) -> f32;

    /// Current thermal state of this compute device.
    /// Used by the thermal coupling logic (§13) and budget enforcer (§7).
    fn thermal_state(&self) -> ThermalState;

    /// Current power draw in milliwatts. Zero if the device cannot report power.
    fn power_draw_mw(&self) -> u32;

    /// Submit a compute command buffer for execution.
    ///
    /// The caller must hold a valid `ComputeGrant` (§12) for this device.
    /// The kernel validates the grant before forwarding to the driver.
    /// Returns a completion token for polling or async notification.
    fn submit(&mut self, commands: &ComputeCommandBuffer, grant: &ComputeGrant)
        -> Result<CompletionToken, ComputeError>;

    /// Poll completion status of a previously submitted command buffer.
    fn poll_completion(&self, token: CompletionToken) -> CompletionStatus;

    /// Allocate device-local memory (GPU VRAM, NPU scratchpad).
    /// Returns None for unified memory devices — use system allocator instead.
    fn alloc_device_memory(&mut self, size: usize, flags: MemoryFlags)
        -> Result<Option<DeviceMemoryHandle>, ComputeError>;

    /// Free device-local memory. No-op for unified memory devices.
    fn free_device_memory(&mut self, handle: DeviceMemoryHandle)
        -> Result<(), ComputeError>;

    /// Whether this device supports preemption of running workloads.
    /// GPUs with hardware preemption return true; most NPUs return false.
    fn supports_preemption(&self) -> bool;

    /// Request the device to preempt the current workload.
    /// Only valid if `supports_preemption()` returns true.
    fn preempt_current(&mut self) -> Result<(), ComputeError>;
}
```

### 3.2 ComputeClass

```rust
/// Classification of a compute device by its primary compute paradigm.
///
/// A physical device may support multiple compute paradigms (e.g., a GPU
/// that handles both rendering and general-purpose compute). In that case,
/// the device registers once with ComputeClass::Gpu and advertises its
/// compute capabilities via ComputeCapabilityDescriptor (§4).
///
/// The kernel uses ComputeClass for coarse-grained routing and capability
/// attenuation. AIRS uses ComputeCapabilityDescriptor for fine-grained
/// workload-to-device matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComputeClass {
    /// General-purpose CPU cores. Always present. The baseline compute device.
    /// Capabilities include NEON SIMD (128-bit), optional SVE/SVE2.
    /// The CPU ComputeDevice is registered at boot by the kernel itself,
    /// not by a driver probe.
    Cpu,

    /// Graphics Processing Unit with compute shader support.
    /// Massively parallel, optimized for data-parallel workloads.
    /// Examples: VirtIO-GPU 3D (QEMU), VideoCore VII (Pi 5), Apple AGX.
    Gpu,

    /// Neural Processing Unit — fixed-function or semi-programmable
    /// accelerator optimized for neural network inference.
    /// Typically excels at low-precision (INT8/INT4) matrix operations.
    /// Examples: Apple Neural Engine, Rockchip RKNN, Qualcomm Hexagon NPU.
    Npu,

    /// Digital Signal Processor — optimized for real-time signal processing
    /// with deterministic latency. Used for audio processing, sensor fusion,
    /// and communication protocols.
    /// Examples: Qualcomm Hexagon DSP, TI C66x.
    Dsp,

    /// Tensor Processing Unit or similar matrix-multiply accelerator.
    /// Distinct from NPU in being more programmable and typically higher
    /// throughput. Rare on edge devices today but included for completeness.
    Tpu,

    /// Application-Specific Integrated Circuit — fixed-function hardware
    /// for a narrow task. Not general-purpose compute, but registered in
    /// the compute registry for budget tracking and capability gating.
    /// Examples: hardware video encoder/decoder, cryptographic accelerator.
    Asic,
}
```

### 3.3 ComputeDeviceId

```rust
/// Unique identifier for a compute device within the kernel.
///
/// Extends the device model's DeviceId (device-model/representation.md §3.2)
/// with a compute-specific generation counter. The generation prevents
/// stale references after device removal and re-addition (e.g., USB
/// accelerator hot-unplug).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComputeDeviceId {
    /// The underlying device model identifier.
    /// Links this compute device to its DeviceNode in the device graph.
    pub device_id: DeviceId,

    /// Compute-specific generation counter. Incremented when a device
    /// is removed and re-added (hot-swap). A ComputeGrant referencing
    /// a stale generation is rejected.
    pub generation: u32,

    /// The compute class for fast filtering without trait dispatch.
    pub class: ComputeClass,
}
```

-----

## 4. ComputeCapabilityDescriptor

The capability descriptor captures everything the kernel and AIRS need to make placement decisions. It is populated by the driver at probe time and may be updated at runtime if the device's capabilities change (e.g., thermal throttling reduces peak throughput).

### 4.1 Descriptor Structure

```rust
/// Detailed description of what a compute device can do.
///
/// The kernel stores this in the ComputeRegistry (§5). AIRS queries it
/// to match workloads to devices. The driver populates it at probe time
/// and may update it at runtime (e.g., thermal throttling reduces peak_ops).
pub struct ComputeCapabilityDescriptor {
    // --- Performance ---

    /// Peak throughput in operations per second.
    /// The unit depends on compute class:
    ///   CPU: FLOPS (FP32) or IPS (integer)
    ///   GPU: shader FLOPS (FP32)
    ///   NPU: INT8 operations per second (most relevant for inference)
    ///   DSP: MAC operations per second
    pub peak_ops: u64,

    /// Supported data types for compute operations.
    pub supported_types: ComputeDataTypes,

    /// Supported quantization formats for neural network inference.
    /// Empty for non-inference devices (DSP, ASIC).
    pub quant_formats: QuantFormatSet,

    /// Maximum concurrent command streams or compute queues.
    /// CPU: number of cores available for compute.
    /// GPU: number of hardware compute queues.
    /// NPU: typically 1 (runs compiled graphs sequentially).
    pub max_concurrent_streams: u32,

    // --- Memory ---

    /// Device-local memory in bytes (GPU VRAM, NPU on-chip buffers).
    /// Zero for unified memory architectures where the device shares
    /// system RAM with the CPU.
    pub device_memory_bytes: usize,

    /// Whether this device shares system RAM with the CPU.
    /// True for ARM SoCs (Apple Silicon, Pi 5, Qualcomm).
    /// False for discrete GPUs (hypothetical future target).
    pub unified_memory: bool,

    /// Memory bandwidth to/from this device in bytes per second.
    /// For unified memory: shared DRAM bandwidth.
    /// For discrete: PCIe/interconnect bandwidth.
    pub memory_bandwidth_bytes_per_sec: u64,

    // --- Latency ---

    /// Typical submission-to-first-result latency for a minimal workload,
    /// in microseconds. Includes command buffer submission, device wakeup
    /// (if idle), and first output. Used by AIRS to estimate time-to-first-token.
    pub min_latency_us: u32,

    /// Whether the device supports preemption of running workloads.
    /// True: interactive requests can interrupt background compute.
    /// False: workloads run to completion (most NPUs, some ASICs).
    pub preemptible: bool,

    // --- Power ---

    /// Thermal Design Power in milliwatts (TDP equivalent).
    /// The maximum sustained power this device can consume before
    /// thermal throttling. Used by the thermal coupling logic (§13)
    /// and budget enforcer (§7).
    pub tdp_mw: u32,

    /// Power draw at idle in milliwatts. Some devices can be powered
    /// down entirely (0 mW) when unused. Others maintain a base draw.
    pub idle_power_mw: u32,

    // --- Identity ---

    /// Human-readable device name for diagnostics and audit.
    /// Example: "Apple Neural Engine (16-core)", "VideoCore VII QPU".
    pub name: &'static str,

    /// Firmware or hardware version string, if available.
    pub version: Option<&'static str>,
}
```

### 4.2 Data Type Flags

```rust
/// Bitflags for supported compute data types.
///
/// Used by AIRS to match model quantization formats to hardware.
/// A device that supports INT8 but not FP16 will be preferred for
/// INT8-quantized models but not for FP16 models.
bitflags! {
    pub struct ComputeDataTypes: u32 {
        /// 32-bit floating point. Supported by all CPUs and most GPUs.
        const FP32     = 0b0000_0001;
        /// 16-bit floating point. Common on GPUs and some NPUs.
        const FP16     = 0b0000_0010;
        /// 16-bit brain floating point. Used in ML training/inference.
        const BF16     = 0b0000_0100;
        /// 8-bit integer. Primary NPU data type for quantized inference.
        const INT8     = 0b0000_1000;
        /// 4-bit integer. Aggressive quantization for memory-constrained devices.
        const INT4     = 0b0001_0000;
        /// 64-bit floating point. CPU-only for scientific compute.
        const FP64     = 0b0010_0000;
        /// 8-bit floating point (FP8 E4M3/E5M2). Emerging ML data type.
        const FP8      = 0b0100_0000;
        /// NEON SIMD 128-bit operations (aarch64 CPU).
        const NEON     = 0b1000_0000;
        /// SVE/SVE2 scalable vector operations (future aarch64 CPUs).
        const SVE      = 0b0001_0000_0000;
    }
}

/// Set of supported quantization formats for neural network inference.
///
/// Maps directly to GGML quantization types used by the AIRS inference
/// engine (airs/inference.md §3.1). The kernel does not interpret these —
/// it stores and serves them for AIRS to match against model requirements.
bitflags! {
    pub struct QuantFormatSet: u32 {
        const Q4_0     = 0b0000_0001;
        const Q4_K_M   = 0b0000_0010;
        const Q5_K_M   = 0b0000_0100;
        const Q6_K     = 0b0000_1000;
        const Q8_0     = 0b0001_0000;
        const F16      = 0b0010_0000;
        const F32      = 0b0100_0000;
    }
}
```

### 4.3 Platform Capability Profiles

Pre-defined capability profiles for known AIOS target platforms:

```text
Device               Class    peak_ops (INT8)    Memory        Bandwidth    TDP     Preemptible
─────────────────    ─────    ───────────────    ──────────    ─────────    ────    ───────────
Cortex-A72 (QEMU)   CPU      ~4 GOPS            Unified       ~8 GB/s     5 W     Yes (threads)
Cortex-A76 (Pi 5)   CPU      ~8 GOPS            Unified       ~18 GB/s    5 W     Yes (threads)
VideoCore VII        GPU      ~50 GFLOPS (FP32)  Unified       ~18 GB/s    3 W     Partial
Apple M-series GPU   GPU      ~2.6 TFLOPS        Unified       ~100 GB/s   15 W    Yes
Apple ANE (16-core)  NPU      ~15.8 TOPS         Unified       ~100 GB/s   2 W     No
Rockchip RK3588      NPU      ~6 TOPS            Unified       ~12 GB/s    2 W     No
Qualcomm Hexagon     DSP      ~15 TOPS           Unified       ~34 GB/s    3 W     Partial
```

These profiles are used as reference implementations for the `ComputeCapabilityDescriptor`. Actual values are populated by the driver at probe time based on hardware register reads, DTB properties, or hardcoded platform tables.

### 4.4 Error Types

```rust
/// Errors returned by ComputeDevice trait methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeError {
    /// The ComputeGrant is invalid (expired, revoked, or wrong device).
    InvalidGrant,
    /// The device is not available (powered down, removed, or in error state).
    DeviceUnavailable,
    /// The device rejected the command buffer (malformed, unsupported op).
    InvalidCommands,
    /// Out of device-local memory.
    OutOfDeviceMemory,
    /// Compute budget exceeded for this agent.
    BudgetExceeded,
    /// The device does not support preemption.
    PreemptionNotSupported,
    /// The device is thermally throttled and refusing new work.
    ThermalThrottled,
    /// An internal driver error occurred.
    DriverError,
}
```
