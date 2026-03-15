# AIOS Apple Neural Engine Driver

Part of: [accelerators.md](../accelerators.md) — Platform Accelerator Drivers
**Related:** [drivers.md](./drivers.md) — AcceleratorDriver trait, [memory.md](./memory.md) — Accelerator memory management

-----

## 6. Apple Neural Engine Architecture

The Apple Neural Engine (ANE) is a fixed-function inference accelerator present in all Apple Silicon SoCs (M1, M2, M3, M4 and their Pro/Max/Ultra variants). Unlike GPUs, the ANE is not programmable with arbitrary shaders — it executes pre-compiled neural network graphs in a proprietary format. This architectural distinction fundamentally shapes the driver model.

### 6.1 Hardware Overview

```text
Apple Neural Engine (M-series)
├── 16 Neural Engine cores (M1/M2/M3/M4)
│   ├── Each core: matrix multiply + activation + pooling units
│   ├── INT8 throughput: ~11 TOPS per core (M1) to ~18 TOPS (M4)
│   ├── FP16 throughput: ~5.5 TFLOPS per core (M1) to ~9 TFLOPS (M4)
│   └── BF16 support: M3+ only
├── On-chip SRAM (scratchpad)
│   ├── M1: 32 MB shared across cores
│   ├── M2: 32 MB
│   ├── M3: 48 MB
│   └── M4: 64 MB
├── DMA engine (4 channels)
│   ├── System RAM ↔ scratchpad transfers
│   ├── Tiled transfer support (automatic tiling for large tensors)
│   └── Double-buffered: transfer next tile while processing current
├── Command queue interface
│   ├── MMIO registers for queue management
│   ├── Hardware supports 8 independent queues (priority levels)
│   └── Completion signaled via MSI interrupt
└── Unified memory (shared with CPU/GPU)
    └── Cache-coherent on M1+ (ARM ACE protocol)
```

### 6.2 ANE vs GPU for Inference

The ANE is purpose-built for neural network inference. It excels at the specific operations that dominate modern ML models but cannot perform arbitrary computation:

```text
Operation          ANE Performance    GPU Performance    Winner
────────────       ───────────────    ───────────────    ──────
Dense matmul       16 TOPS INT8       1.5 TFLOPS FP32   ANE (10x)
Depthwise conv     Dedicated unit     Shader emulation   ANE (5x)
Batch norm         Fused pipeline     Separate pass      ANE (3x)
Softmax            Hardware unit      Shader             ANE (2x)
Custom activation  NOT SUPPORTED      Shader             GPU
Scatter/gather     NOT SUPPORTED      Shader             GPU
Dynamic shapes     Limited support    Full support       GPU
Small batches      High efficiency    Launch overhead     ANE

Power efficiency:
  ANE: ~3 TOPS/W (M1) to ~6 TOPS/W (M4)
  GPU: ~0.5 TFLOPS/W
  CPU NEON: ~0.05 TFLOPS/W
```

**Key constraint:** The ANE runs compiled graphs to completion. It cannot execute arbitrary code. If a model contains an operation the ANE doesn't support, the entire model (or the unsupported subgraph) must fall back to GPU or CPU. AIRS handles this partitioning decision ([airs/inference.md](../../intelligence/airs/inference.md) §3.2).

### 6.3 Compiled Model Format

ANE models are compiled offline through Apple's CoreML tools or an equivalent AIOS compilation pipeline. The compiled format is a directed acyclic graph (DAG) of operations, with all tensor shapes, data types, and memory layouts resolved at compile time:

```text
ANE Compiled Model Format (.anemodel):

Header (64 bytes):
  magic: "ANEM" (0x414E454D)
  version: u32
  graph_count: u32
  total_weight_bytes: u64
  total_scratch_bytes: u64
  input_tensor_count: u16
  output_tensor_count: u16
  checksum: SHA-256

Graph Section:
  For each subgraph:
    node_count: u32
    nodes: [AneGraphNode; node_count]
    edges: adjacency list (source → destination)

Node Definition:
  op_type: AneOp (Matmul, Conv2d, DepthwiseConv, BatchNorm,
                   Relu, Gelu, Softmax, Pooling, Reshape, ...)
  input_shapes: [(batch, channels, height, width); N]
  output_shapes: [(batch, channels, height, width); N]
  weight_offset: u64  (offset into weight section)
  weight_bytes: u64
  quantization: QuantInfo (None, INT8_Symmetric, INT8_Asymmetric)
  tiling_strategy: TilingHint (Auto, Explicit(tile_h, tile_w))

Weight Section:
  Packed weight data, aligned to 64-byte boundaries.
  Weights are pre-quantized and pre-transposed for ANE layout.

Scratch Section:
  Activation buffer layout descriptors — tells the driver
  how to allocate scratchpad tiles for intermediate results.
```

### 6.4 Supported Operations

```text
ANE Hardware Operations (M1-M4):

Category        Operations                              Notes
────────        ──────────                              ─────
Linear          MatMul, FullyConnected, Einsum           Up to rank-5 tensors
Convolution     Conv2D, DepthwiseConv, TransposeConv     3×3, 5×5, 7×7 kernels
Normalization   BatchNorm, InstanceNorm, LayerNorm       Fused with activation
Activation      ReLU, GELU, SiLU/Swish, Sigmoid, Tanh   Hardware pipeline stage
Pooling         MaxPool, AvgPool, GlobalAvgPool          2D only
Elementwise     Add, Mul, Sub, Div, Max, Min             Broadcasting supported
Reshape         Reshape, Transpose, Concat, Split        Zero-copy when possible
Reduction       ReduceSum, ReduceMean, ReduceMax         Along specified axes
Quantize        Quantize, Dequantize                     INT8 ↔ FP16 conversion
Attention       ScaledDotProduct (M3+)                   Fused multi-head attention

NOT Supported (must fall back to CPU/GPU):
  - Dynamic control flow (if/else, while loops)
  - Custom operations
  - Scatter/gather indexing
  - Sparse operations
  - Dynamic tensor shapes (dimensions must be compile-time constants)
```

-----

## 7. ANE Driver Model

The ANE driver implements `AcceleratorDriver` with a programming model fundamentally different from GPU drivers. Instead of compiling and dispatching shaders, it loads pre-compiled model graphs and runs them to completion.

### 7.1 Driver Structure

```rust
/// Apple Neural Engine driver.
///
/// Implements Driver + ComputeDevice + AcceleratorDriver.
/// The ANE is a fixed-function inference accelerator — it runs
/// compiled neural network graphs, not arbitrary compute programs.
pub struct AneDriver {
    /// Device identity from the device model.
    device_id: DeviceId,
    /// Generation counter for the compute registry.
    generation: u32,

    /// MMIO base address (from DTB: "apple,ane").
    mmio_base: VirtAddr,
    /// Interrupt line for completion notification.
    irq: IrqLine,

    /// Hardware command queues (8 priority levels).
    command_queues: [AneCommandQueue; 8],

    /// Loaded models currently resident in ANE scratchpad
    /// or system memory. Keyed by model hash.
    loaded_models: BTreeMap<ContentHash, AneLoadedModel>,

    /// DMA engine for system RAM ↔ scratchpad transfers.
    dma: AneDmaEngine,

    /// Scratchpad allocator for on-chip SRAM.
    scratchpad: ScratchpadAllocator,

    /// Performance counters.
    counters: AneCounters,

    /// Thermal zone registration.
    thermal_zone: ComputeThermalZone,
}

/// A model loaded into the ANE.
pub struct AneLoadedModel {
    /// Model identifier (SHA-256 of compiled model).
    pub model_hash: ContentHash,
    /// Weight buffer — may be in system RAM (mapped read-only)
    /// or partially cached in scratchpad.
    pub weight_buffer: ComputeBuffer,
    /// Scratchpad allocation for activation buffers.
    pub scratch_allocation: Option<ScratchpadAllocation>,
    /// The compiled graph ready for execution.
    pub graph: AneCompiledGraph,
    /// Reference count — number of active sessions using this model.
    pub ref_count: u32,
    /// Last used timestamp for LRU eviction.
    pub last_used: Timestamp,
}
```

### 7.2 Command Queue Interface

The ANE provides hardware command queues, each with an independent priority level. The driver maps AIOS compute priorities to ANE queue priorities:

```rust
/// ANE hardware command queue.
///
/// Each queue is a ring buffer of command descriptors in system
/// memory, with MMIO registers for head/tail pointer management.
pub struct AneCommandQueue {
    /// Queue priority (0 = highest, 7 = lowest).
    pub priority: u8,
    /// Ring buffer of command descriptors.
    pub ring: AneCommandRing,
    /// Write pointer (driver advances).
    pub write_ptr: u32,
    /// Read pointer (hardware advances).
    pub read_ptr: u32,
    /// Pending submissions awaiting completion.
    pub pending: Vec<AneSubmission>,
}

/// A single ANE command descriptor.
/// Placed in the command ring; hardware reads this to start execution.
#[repr(C)]
pub struct AneCommandDescriptor {
    /// Compiled graph physical address.
    pub graph_addr: u64,
    /// Input tensor physical addresses (up to 8 inputs).
    pub input_addrs: [u64; 8],
    /// Output tensor physical addresses (up to 8 outputs).
    pub output_addrs: [u64; 8],
    /// Weight buffer physical address.
    pub weight_addr: u64,
    /// Scratch buffer physical address.
    pub scratch_addr: u64,
    /// Scratch buffer size in bytes.
    pub scratch_size: u32,
    /// Flags (completion interrupt, DMA mode, tiling hints).
    pub flags: u32,
    /// Fence value written to completion register on finish.
    pub fence_value: u64,
}
```

### 7.3 Model Loading and Caching

The ANE driver maintains a model cache to avoid reloading weights for frequently used models. Weight buffers are the largest memory consumers (2-8 GB for modern LLMs) and should be loaded once, shared read-only across sessions:

```text
Model Loading Flow:

1. AIRS requests inference on model M
2. Driver checks loaded_models for M's content hash
3. If cached:
   a. Increment ref_count
   b. Allocate scratchpad for activation buffers
   c. Skip to step 7
4. If not cached:
   a. Load .anemodel from storage (via Space read)
   b. Validate header (magic, version, checksum)
   c. Allocate weight buffer from DMA pool
   d. DMA weights to buffer (or map directly — unified memory)
   e. If scratchpad has room, pre-load hot weights to SRAM
   f. Register in loaded_models
5. If memory pressure: LRU-evict least recently used model
   a. Wait for ref_count == 0 (no active sessions)
   b. Free weight buffer
   c. Free scratchpad allocation
6. Compile graph node addresses to physical addresses
7. Ready for inference submission
```

### 7.4 AcceleratorDriver Implementation

```rust
impl AcceleratorDriver for AneDriver {
    fn init_compute_engine(&mut self) -> Result<(), ComputeError> {
        // Read ANE version register
        let version = self.read_mmio(ANE_REG_VERSION);
        if version < ANE_MIN_VERSION {
            return Err(ComputeError::DeviceUnavailable);
        }

        // Initialize all 8 command queues
        for (i, queue) in self.command_queues.iter_mut().enumerate() {
            queue.priority = i as u8;
            queue.ring = AneCommandRing::new(64)?; // 64 entries
            // Write ring base address to MMIO
            self.write_mmio(
                ANE_REG_QUEUE_BASE + i * 0x100,
                queue.ring.phys_addr(),
            );
        }

        // Initialize DMA engine
        self.dma.init()?;

        // Initialize scratchpad allocator
        let scratch_size = self.read_mmio(ANE_REG_SCRATCH_SIZE);
        self.scratchpad = ScratchpadAllocator::new(scratch_size as usize);

        // Register thermal zone
        self.thermal_zone = ComputeThermalZone {
            device_id: self.compute_id(),
            temperature_mc: self.read_temperature(),
            trip_points: ComputeTripPoints {
                warm: 60000,    // 60°C — ANE runs cool
                hot: 75000,     // 75°C
                critical: 90000, // 90°C
            },
            coupling: vec![
                (self.cpu_device_id(), 0.2),  // Low coupling
                (self.gpu_device_id(), 0.3),  // Moderate coupling
            ],
        };

        Ok(())
    }

    fn compile_program(
        &self,
        source: &ComputeProgram,
    ) -> Result<ProgramHandle, ComputeError> {
        match source {
            ComputeProgram::PreCompiled { format, data } => {
                // ANE only accepts pre-compiled models
                match format {
                    ModelFormat::AneMlModel => {
                        self.validate_ane_model(data)?;
                        Ok(ProgramHandle::from_model_hash(
                            sha256(data),
                        ))
                    }
                    _ => Err(ComputeError::InvalidCommands),
                }
            }
            _ => Err(ComputeError::InvalidCommands),
        }
    }

    fn map_compute_buffer(
        &self,
        buffer: &ComputeBuffer,
        access: BufferAccess,
    ) -> Result<DeviceAddress, ComputeError> {
        // Apple Silicon is hardware-coherent (ARM ACE protocol).
        // No explicit cache ops needed for unified memory buffers.
        // Just configure SMMU mapping for the ANE's stream ID.
        let device_addr = self.smmu_map(
            buffer.phys_addr.ok_or(ComputeError::InvalidCommands)?,
            buffer.size,
            access,
        )?;
        Ok(device_addr)
    }

    fn set_compute_power_state(
        &self,
        state: ComputePowerState,
    ) -> Result<(), ComputeError> {
        match state {
            ComputePowerState::Active => {
                self.write_mmio(ANE_REG_POWER, ANE_POWER_ACTIVE);
            }
            ComputePowerState::LowPower => {
                // Reduce clock, keep scratchpad powered
                self.write_mmio(ANE_REG_POWER, ANE_POWER_LOW);
            }
            ComputePowerState::Standby => {
                // Clock gate, scratchpad retention
                self.write_mmio(ANE_REG_POWER, ANE_POWER_STANDBY);
            }
            ComputePowerState::Off => {
                // Full power down — scratchpad contents lost
                self.evict_all_models();
                self.write_mmio(ANE_REG_POWER, ANE_POWER_OFF);
            }
        }
        Ok(())
    }

    fn performance_counters(&self) -> AcceleratorCounters {
        AcceleratorCounters {
            compute_utilization: self.read_utilization(),
            memory_bandwidth_utilization: self.read_bandwidth(),
            operations_completed: self.read_mmio(ANE_REG_OPS_COMPLETED),
            errors: self.read_mmio(ANE_REG_ERRORS),
            temperature_mc: self.read_temperature(),
            power_draw_mw: self.read_power(),
        }
    }

    fn clock_frequencies(&self) -> ClockInfo {
        ClockInfo {
            core_mhz: self.read_mmio(ANE_REG_CLOCK) as u32,
            memory_mhz: 0, // Unified memory — no separate clock
            dvfs_active: true,
        }
    }
}
```

### 7.5 ComputeDevice Implementation

```rust
impl ComputeDevice for AneDriver {
    fn compute_id(&self) -> ComputeDeviceId {
        ComputeDeviceId {
            device_id: self.device_id,
            generation: self.generation,
            class: ComputeClass::Npu,
        }
    }

    fn compute_class(&self) -> ComputeClass {
        ComputeClass::Npu
    }

    fn capabilities(&self) -> &ComputeCapabilityDescriptor {
        // Returns reference to descriptor stored in self.caps
        // (populated at probe time with these values):
        //   peak_ops: 176_000_000_000_000 (16 cores x ~11 TOPS/core, M1)
        //   supported_types: FP16 | INT8 | INT4
        //   quant_formats: Q8_0 | Q4_0 | F16
        //   max_concurrent_streams: 8 (8 hardware queues)
        //   device_memory_bytes: 0 (unified memory)
        //   unified_memory: true
        //   memory_bandwidth_bytes_per_sec: 68_000_000_000 (68 GB/s, M1)
        //   min_latency_us: 10
        //   preemptible: false (graph runs to completion)
        //   tdp_mw: 2000, idle_power_mw: 50
        //   name: "Apple Neural Engine (16-core)"
        //   version: Some("M1")
        &self.caps
    }

    fn submit(
        &mut self,
        commands: &ComputeCommandBuffer,
        grant: &ComputeGrant,
    ) -> Result<CompletionToken, ComputeError> {
        // Parse inference request from command buffer
        let request = AneInferenceRequest::from_command_buffer(commands)?;

        // Look up loaded model
        let model = self.loaded_models.get(&request.model_hash)
            .ok_or(ComputeError::DeviceUnavailable)?;

        // Validate buffer references against grant
        for buffer_id in &request.input_buffers {
            if !grant.authorized_buffers.contains(buffer_id) {
                return Err(ComputeError::InvalidGrant);
            }
        }

        // Build command descriptor
        let desc = self.build_command_descriptor(
            model,
            &request,
            grant,
        )?;

        // Select queue based on priority
        let queue_idx = self.priority_to_queue(request.priority);
        let token = self.command_queues[queue_idx]
            .submit(desc)?;

        Ok(token)
    }

    fn utilization(&self) -> f32 {
        self.counters.compute_utilization
    }

    fn power_draw_mw(&self) -> u32 {
        self.counters.power_draw_mw
    }

    fn supports_preemption(&self) -> bool {
        false // ANE graphs run to completion
    }
}
```

### 7.6 ANE Memory Model

The ANE uses a hybrid memory model combining unified system RAM with on-chip scratchpad:

```text
ANE Memory Hierarchy:

Layer              Size              Access Time    Contents
─────              ────              ───────────    ────────
On-chip SRAM       32-64 MB          1 cycle        Hot weights, activations
System RAM         Unified (all)     ~100 cycles    Full weights, I/O tensors
                                                    (cache-coherent via ACE)

Data Flow for Single Inference:

1. Input tensor:   System RAM → ANE DMA → scratchpad (tiled)
2. Weights:        System RAM → ANE DMA → scratchpad (per-layer)
3. Computation:    Scratchpad → ANE cores → scratchpad
4. Between layers: Scratchpad (activations stay on-chip when possible)
5. Output tensor:  Scratchpad → ANE DMA → system RAM

Key Optimization: Double-buffered DMA
  While ANE processes layer N with weights from scratchpad buffer A,
  DMA transfers layer N+1 weights into scratchpad buffer B.
  Swaps A/B between layers — hides transfer latency.
```

The scratchpad allocator manages on-chip SRAM as a series of tiles:

```rust
/// Scratchpad allocator for ANE on-chip SRAM.
///
/// The scratchpad is divided into tiles. Each inference operation
/// gets a set of tiles for activations. Weight tiles are allocated
/// from a separate partition (double-buffered).
pub struct ScratchpadAllocator {
    /// Total scratchpad size in bytes.
    pub total_bytes: usize,
    /// Activation partition (front half of scratchpad).
    pub activation_region: ScratchRegion,
    /// Weight double-buffer partition (back half).
    pub weight_buffer_a: ScratchRegion,
    pub weight_buffer_b: ScratchRegion,
    /// Current weight buffer (A or B).
    pub active_weight_buffer: bool,
}

pub struct ScratchRegion {
    pub base_offset: usize,
    pub size: usize,
    pub allocated: usize,
}
```
