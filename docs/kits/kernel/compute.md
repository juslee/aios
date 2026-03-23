# Compute Kit

**Layer:** Kernel | **Crate:** `aios_compute` | **Architecture:** [`docs/kernel/compute.md`](../../kernel/compute.md) + 6 sub-docs, [`docs/platform/gpu.md`](../../platform/gpu.md) + 5 sub-docs

## 1. Overview

Compute Kit provides unified access to all accelerated compute hardware: GPU, NPU, and
CPU SIMD. Modern SoCs bundle multiple compute engines onto a single die, each with
different strengths — GPU for parallel throughput, NPU for low-precision neural inference,
CPU NEON for fallback. Traditional OSes treat each as a separate driver concern. Compute
Kit introduces a kernel-mediated abstraction that routes workloads to the best available
hardware, enforces capability-gated access, and manages per-agent compute budgets.

The Kit organizes into **three tiers** serving different consumers:

- **Tier 1 — Display Surface**: buffer allocation, composition, and scanout for the
  compositor and UI toolkit. Most application developers never touch this directly.
- **Tier 2 — Render Pipeline**: 3D graphics, shaders, and WebGPU for games, creative
  apps, and browser rendering. Wraps wgpu/Vulkan with capability enforcement.
- **Tier 3 — Inference Pipeline**: LLM inference, embeddings, and vision models for
  AIRS Kit, Search Kit, and AI-powered agents. Routes to NPU first, GPU fallback,
  CPU NEON fallback.

The kernel classifies devices and enforces budgets; AIRS decides placement. A compromised
agent cannot monopolize an accelerator, bypass thermal limits, or access another agent's
compute buffers — the kernel mediates every operation.

See [ADR: Compute Kit](../../knowledge/decisions/2026-03-22-jl-compute-kit.md) for the
design rationale behind the three-tier split.

## 2. Core Traits

### Tier 1 — Display Surface

```rust
use aios_compute::surface::{GpuSurface, SurfaceBuffer, DamageRect, SemanticHint};
use aios_capability::CapabilityHandle;

/// Display surface trait for compositor and UI toolkit integration.
///
/// Allocates GPU-backed buffers, submits damage regions, and requests
/// direct scanout (bypassing composition when only one surface is visible).
pub trait GpuSurface {
    /// Allocate a new surface buffer with the given dimensions and format.
    fn allocate_buffer(
        &self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<SurfaceBuffer, ComputeError>;

    /// Submit damage regions to the compositor. Only damaged pixels are
    /// re-composited, reducing GPU work.
    fn submit_damage(&self, buffer: &SurfaceBuffer, damage: &[DamageRect]) -> Result<(), ComputeError>;

    /// Set a semantic hint for compositor optimization. The compositor
    /// uses hints to choose rendering strategies (e.g., video surfaces
    /// get direct scanout, text surfaces get subpixel rendering).
    fn set_semantic_hint(&self, hint: SemanticHint) -> Result<(), ComputeError>;

    /// Request direct scanout (bypass composition). Succeeds only when
    /// this is the only visible surface on the display.
    fn request_direct_scanout(&self) -> Result<bool, ComputeError>;
}

/// Semantic hints that inform compositor optimization.
pub enum SemanticHint {
    /// UI text — subpixel rendering, high priority for clarity.
    UiText,
    /// Video playback — direct scanout candidate, lower composition priority.
    VideoPlayback,
    /// 3D rendering — vsync-aligned, no subpixel.
    Rendering3D,
    /// Scrolling content — predictive composition.
    ScrollingContent,
    /// Static content — cache aggressively.
    StaticContent,
}
```

### Tier 2 — Render Pipeline

```rust
use aios_compute::render::{GpuRender, RenderPipeline, GpuTexture, CommandBuffer, Fence};

/// 3D rendering trait for games, creative apps, and browser rendering.
///
/// Wraps wgpu/Vulkan with capability enforcement and budget tracking.
/// Agents submit command buffers; the kernel validates and dispatches
/// them to the GPU driver.
pub trait GpuRender {
    /// Create a render pipeline (shaders, vertex layout, blend state).
    fn create_pipeline(&self, desc: &PipelineDescriptor) -> Result<RenderPipeline, ComputeError>;

    /// Allocate a GPU texture.
    fn create_texture(&self, desc: &TextureDescriptor) -> Result<GpuTexture, ComputeError>;

    /// Submit a command buffer for execution on the GPU.
    /// Returns a fence that signals when execution completes.
    fn submit_commands(&self, commands: &CommandBuffer) -> Result<Fence, ComputeError>;

    /// Wait for a fence to signal (GPU execution complete).
    fn wait_fence(&self, fence: &Fence, timeout: Duration) -> Result<(), ComputeError>;
}
```

### Tier 3 — Inference Pipeline

```rust
use aios_compute::inference::{InferencePipeline, ModelHandle, InferenceSession, InferenceOutput};

/// Inference pipeline for LLM, embedding, and vision models.
///
/// Routes to the best available hardware: NPU → GPU → CPU NEON.
/// The ComputeResourceManager handles routing transparently — agents
/// just submit inference requests and get results.
pub trait InferencePipeline {
    /// Load a model from the model registry. The kernel selects the
    /// best hardware based on model requirements and available budget.
    fn load_model(&self, model_id: &str) -> Result<ModelHandle, ComputeError>;

    /// Create an inference session with a token budget.
    fn create_session(
        &self,
        model: &ModelHandle,
        budget: InferenceBudget,
    ) -> Result<InferenceSession, ComputeError>;

    /// Run inference (blocking). Returns the complete output.
    fn run(&self, session: &InferenceSession, input: &[Token]) -> Result<InferenceOutput, ComputeError>;

    /// Stream inference output token by token.
    fn stream(
        &self,
        session: &InferenceSession,
        input: &[Token],
    ) -> Result<impl Iterator<Item = Result<Token, ComputeError>>, ComputeError>;
}

/// Per-session inference budget.
pub struct InferenceBudget {
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Maximum wall-clock time.
    pub max_duration: Duration,
    /// Maximum compute cost (in device-specific units).
    pub max_compute_units: u64,
}
```

### Resource Manager

```rust
use aios_compute::manager::ComputeResourceManager;

/// Central resource manager that routes workloads across all compute hardware.
///
/// The Resource Manager enforces per-agent budgets, thermal limits, and
/// capability-gated access. AIRS queries it for placement decisions;
/// the kernel enforces constraints.
pub trait ComputeResourceManager {
    /// Query available compute devices and their capabilities.
    fn query_devices(&self) -> Vec<ComputeDeviceInfo>;

    /// Check the agent's remaining compute budget.
    fn remaining_budget(&self, agent: AgentId) -> ComputeBudget;

    /// Request a specific device class for a workload.
    /// The manager may return a different device if the requested one
    /// is unavailable or over-budget.
    fn request_device(
        &self,
        class: ComputeClass,
        requirements: &ComputeRequirements,
    ) -> Result<ComputeDeviceId, ComputeError>;
}

/// Device classification.
pub enum ComputeClass {
    Cpu,
    Gpu,
    Npu,
    Dsp,
}
```

## 3. Usage Patterns

### Running inference (most common use case)

```rust
use aios_compute::inference::{InferencePipeline, InferenceBudget};
use aios_airs::ModelRegistry;

fn summarize_document(ctx: &AgentContext, text: &str) -> Result<String, AppError> {
    let pipeline = ctx.inference_pipeline()?;
    let model = pipeline.load_model("airs/summarizer-3b-q4")?;

    let session = pipeline.create_session(&model, InferenceBudget {
        max_tokens: 512,
        max_duration: Duration::secs(30),
        max_compute_units: 1_000_000,
    })?;

    let tokens = tokenize(text);
    let output = pipeline.run(&session, &tokens)?;
    Ok(detokenize(&output.tokens))
}
```

### Streaming inference for chat

```rust
use aios_compute::inference::InferencePipeline;

fn stream_chat_response(
    ctx: &AgentContext,
    prompt: &[Token],
    on_token: impl FnMut(Token),
) -> Result<(), AppError> {
    let pipeline = ctx.inference_pipeline()?;
    let model = pipeline.load_model("airs/chat-8b-q4k")?;
    let session = pipeline.create_session(&model, InferenceBudget {
        max_tokens: 2048,
        max_duration: Duration::secs(120),
        max_compute_units: 10_000_000,
    })?;

    for token_result in pipeline.stream(&session, prompt)? {
        let token = token_result?;
        on_token(token);
    }
    Ok(())
}
```

### GPU rendering (Tier 2)

```rust
use aios_compute::render::GpuRender;

fn render_frame(renderer: &dyn GpuRender, scene: &Scene) -> Result<(), AppError> {
    let pipeline = renderer.create_pipeline(&scene.pipeline_desc)?;
    let commands = scene.build_command_buffer(&pipeline)?;
    let fence = renderer.submit_commands(&commands)?;
    renderer.wait_fence(&fence, Duration::millis(16))?; // 60 FPS target
    Ok(())
}
```

## 4. Integration Examples

### With AIRS Kit — inference routing

```rust
use aios_airs::{AirsSession, ModelSelector};
use aios_compute::inference::InferencePipeline;

/// AIRS Kit uses Compute Kit's Tier 3 to run inference. The Resource
/// Manager routes to the best hardware automatically.
fn airs_inference(ctx: &AgentContext, prompt: &str) -> Result<String, AppError> {
    let session = ctx.airs_session()?;
    // AIRS selects the model; Compute Kit handles device routing
    let response = session.generate(prompt)?;
    Ok(response.text)
}
```

### With Interface Kit — GPU-accelerated UI

```rust
use aios_interface::{View, Canvas};
use aios_compute::surface::GpuSurface;

/// Interface Kit uses Compute Kit's Tier 1 for GPU-accelerated rendering.
/// The compositor receives semantic hints to optimize composition.
fn render_widget(surface: &dyn GpuSurface, widget: &Widget) -> Result<(), AppError> {
    let buffer = surface.allocate_buffer(widget.width, widget.height, PixelFormat::Bgra8)?;
    widget.draw_into(&buffer)?;
    surface.submit_damage(&buffer, &[widget.dirty_rect()])?;
    surface.set_semantic_hint(SemanticHint::UiText)?;
    Ok(())
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
| --- | --- | --- |
| `ComputeAccess { tier: Tier1 }` | Display surface operations | Compositor only |
| `ComputeAccess { tier: Tier2 }` | GPU render pipeline | Prompt user on first use |
| `ComputeAccess { tier: Tier3 }` | Inference pipeline | Granted based on trust level |
| `ComputeBudget` | Per-agent compute time limits | Enforced by kernel, set by trust level |

### Agent manifest example

```toml
[agent]
name = "com.example.ai-assistant"
version = "1.0.0"

[capabilities.required]
compute_inference = true   # Tier 3 — inference pipeline access

[capabilities.optional]
compute_render = true      # Tier 2 — GPU rendering for visualization
```

## 6. Error Handling

```rust
/// Errors returned by Compute Kit operations.
pub enum ComputeError {
    /// No device of the requested class is available.
    /// Recovery: fall back to a lower class (GPU → CPU NEON).
    DeviceNotAvailable { class: ComputeClass },

    /// The agent's compute budget is exhausted.
    /// Recovery: wait for budget replenishment or reduce workload.
    BudgetExhausted { remaining: ComputeBudget },

    /// The model could not be loaded (not found, wrong format, too large).
    /// Recovery: try a smaller quantization or different model.
    ModelLoadFailed { model_id: String, reason: String },

    /// Inference was interrupted by thermal throttling.
    /// Recovery: wait for device to cool, retry with lower priority.
    ThermalThrottled,

    /// The GPU command buffer was rejected (invalid commands).
    /// Recovery: fix the command buffer — this is a programming error.
    InvalidCommandBuffer { reason: String },

    /// The fence timed out (GPU didn't complete in time).
    /// Recovery: the GPU may be overloaded — reduce workload or wait.
    FenceTimeout,

    /// The agent does not hold the required ComputeAccess capability.
    /// Recovery: request the capability or degrade gracefully.
    CapabilityDenied,

    /// Out of GPU/accelerator memory.
    /// Recovery: release unused textures/buffers, try a smaller allocation.
    OutOfMemory { requested: usize, available: usize },
}
```

## 7. Platform & AI Availability

### Hardware driver matrix

| Driver | Hardware | Tier 1 | Tier 2 | Tier 3 |
| --- | --- | --- | --- | --- |
| VirtIO-GPU | QEMU | Yes | Yes | No |
| V3D | Raspberry Pi 4 (VideoCore VI) | Yes | Yes | No |
| AGX | Apple Silicon | Yes | Yes | Yes (Metal) |
| ANE | Apple Neural Engine | No | No | Yes |
| Ethos-U | ARM NPU | No | No | Yes |
| CPU NEON | All aarch64 | No | No | Fallback |

### AIRS scheduling integration

When AIRS and interactive rendering compete for the GPU:

- **Default behavior** (no AIRS): inference yields to interactive rendering.
  Games and UI always get GPU priority; inference uses remaining capacity.
- **NPU-equipped hardware**: no conflict — AIRS runs on NPU, rendering on GPU.
- **Context-aware scheduling** (AIRS online): when AIRS detects the user is idle
  (no foreground GPU activity), inference gets full GPU access. When the user
  launches a game, inference migrates to NPU or CPU automatically.

### Bridge stack (Linux compatibility)

Linux apps (X11/Wayland) access Compute Kit through a bridge stack:

```text
Linux Apps → Wayland Bridge (Smithay) → wgpu (WebGPU)
  → Vulkan (Mesa) → Compute Kit (Tier 1 + 2) → GPU Driver
```

This gives Linux apps GPU acceleration without direct hardware access.

## For Kit Authors

### Registering a new compute device

```rust
use aios_compute::registry::{ComputeRegistry, ComputeDeviceInfo};

/// Hardware drivers register their compute devices with the kernel
/// during initialization. The registry makes devices discoverable
/// to the Resource Manager and AIRS.
fn register_my_accelerator(registry: &ComputeRegistry) -> Result<(), ComputeError> {
    registry.register(ComputeDeviceInfo {
        class: ComputeClass::Npu,
        capabilities: ComputeCapabilityDescriptor {
            int8_tops: 16.0,
            fp16_tflops: 8.0,
            memory_bytes: 512 * 1024 * 1024,
            max_batch_size: 4,
            supported_formats: vec![ModelFormat::Ggml, ModelFormat::CoreML],
        },
        thermal_zone: ThermalZoneId(2),
        power_domain: PowerDomainId(1),
    })?;
    Ok(())
}
```

### Implementing the ComputeDevice trait

```rust
use aios_compute::device::ComputeDevice;

/// Every accelerator driver implements this trait. The kernel calls
/// these methods after capability validation and budget checks.
impl ComputeDevice for MyAccelerator {
    fn submit_workload(&self, workload: &ComputeWorkload) -> Result<WorkloadId, ComputeError> {
        // Validate workload against device capabilities
        // Submit to hardware queue
        // Return tracking ID
    }

    fn poll_completion(&self, id: WorkloadId) -> Option<ComputeResult> {
        // Check hardware completion status
    }

    fn cancel_workload(&self, id: WorkloadId) -> Result<(), ComputeError> {
        // Cancel in-flight workload (best-effort)
    }

    fn device_utilization(&self) -> f32 {
        // Return current utilization (0.0 to 1.0) for scheduling decisions
    }
}
```

## Cross-References

- [Kernel Compute Abstraction](../../kernel/compute.md) — kernel implementation details
- [GPU & Display Architecture](../../platform/gpu.md) — hardware driver details
- [Accelerator Drivers](../../platform/accelerators.md) — NPU/ANE/Ethos-U drivers
- [AIRS Kit](../intelligence/airs.md) — inference session management
- [Interface Kit](../application/interface.md) — GPU-accelerated UI rendering
- [Thermal Kit](../platform/thermal.md) — thermal throttling coordination
