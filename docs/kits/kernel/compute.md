# Compute Kit

**Layer:** Kernel | **Architecture:** `docs/kernel/compute.md` + 5 sub-docs, `docs/platform/gpu.md` + 5 sub-docs

## Purpose

Unified access to all accelerated compute hardware: GPU, NPU, and CPU SIMD. Three tiers serve different consumers. A single Resource Manager schedules workloads across all hardware.

Replaces the original single `GpuDevice` trait design. See ADR: `docs/knowledge/decisions/2026-03-22-jl-compute-kit.md`.

## Three Tiers

| Tier | Purpose | Routes To | Consumers |
|---|---|---|---|
| **Tier 1: Display Surface** | Buffer alloc, composition, scanout, semantic hints | GPU (display controller) | Compositor, Interface Kit |
| **Tier 2: Render Pipeline** | 3D graphics, shaders, WebGPU | GPU | Games, creative apps, Browser Kit |
| **Tier 3: Inference Pipeline** | LLM inference, embeddings, vision models | NPU first, GPU fallback, CPU NEON fallback | AIRS Kit, Search Kit |

## Key APIs

| Trait / API | Description |
|---|---|
| `GpuSurface` | Tier 1 — allocate buffer, submit damage, set semantic hint, request direct scanout |
| `GpuRender` | Tier 2 — create pipeline, create texture, submit commands, wait fence |
| `InferencePipeline` | Tier 3 — load model, create session, run/stream inference |
| `ComputeResourceManager` | Routes workloads to best hardware, enforces capabilities, manages thermal/power budget, fault isolation |

## Bridge Stack (Linux Compatibility)

```
Linux Apps (X11/Wayland)
  → Wayland Bridge (Smithay)
    → wgpu (WebGPU)
      → Vulkan (Mesa)
        → Compute Kit (Tier 1 + 2)
          → GPU Driver (VirtIO/V3D/AGX)
```

## AIRS Scheduling

- Default: yield to interactive rendering (Option A)
- Upgrade: context-aware scheduling when Context Engine is online (Option C)
- On NPU-equipped hardware: no conflict — AIRS runs on NPU, games run on GPU

## Hardware Drivers

| Driver | Hardware | Tier Support |
|---|---|---|
| VirtIO-GPU | QEMU | Tier 1, Tier 2 |
| V3D | Raspberry Pi 4 (VideoCore VI) | Tier 1, Tier 2 |
| AGX | Apple Silicon | Tier 1, Tier 2, Tier 3 (Metal) |
| ANE | Apple Neural Engine | Tier 3 only |
| Ethos-U | ARM NPU | Tier 3 only |
| CPU NEON | All aarch64 | Tier 3 fallback |

## Dependencies

Memory Kit, Capability Kit

## Implementation Phase

Phase 6+ (compositor, GPU drivers). Tier 3 in Phase 9+ (AIRS inference).
