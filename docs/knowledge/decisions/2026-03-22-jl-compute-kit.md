---
author: Justin Lee
date: 2026-03-22
tags: [gpu, compute, platform, intelligence]
status: final
---

# ADR: Compute Kit — GPU + NPU + CPU SIMD

## Context

The original design had a single `GpuDevice` trait (~500 LoC) for hardware GPU abstraction. But different consumers need radically different things (compositor needs surfaces, games need 3D pipelines, AIRS needs matrix ops), and the industry is converging on CPU+GPU+NPU as the standard hardware profile (Apple Silicon, Qualcomm, Intel Meteor Lake+).

## Options Considered

### Option A: Single GpuDevice trait (original design)

- Pros: Simple, one trait to implement per hardware
- Cons: Conflates display, rendering, and compute; no NPU story; no resource scheduling between consumers; no fault isolation

### Option B: Compute Kit with 3 tiers + Resource Manager

- Pros: Purpose-built APIs per use case, NPU as first-class inference target, single Resource Manager for scheduling/isolation/thermal, aligns with actual hardware partitions
- Cons: More complex, 3 traits instead of 1, Resource Manager is a new component

## Decision

Compute Kit with 3 tiers (Option B):

| Tier | Purpose | Routes To |
|---|---|---|
| **Tier 1: Display Surface** | Buffer alloc, composition, scanout, semantic hints | GPU (display controller) |
| **Tier 2: Render Pipeline** | 3D graphics, shaders, WebGPU | GPU |
| **Tier 3: Inference Pipeline** | LLM inference, embeddings, vision | NPU first -> GPU fallback -> CPU NEON fallback |

**Resource Manager** (single authority): routes workloads to best hardware, allocates GPU memory from Pool::Dma, enforces capabilities, manages thermal/power budget, provides fault isolation.

**AIRS scheduling:** Option A (yield to interactive rendering) as default. Option C (context-aware) when Context Engine is online. On NPU-equipped hardware, no conflict.

**Bridge stack for Linux compat:** GPU Driver -> Compute Kit -> Vulkan (Mesa) -> wgpu -> Wayland Bridge -> Linux apps.

## Consequences

- Replaces the single GpuDevice trait from the architecture docs
- Software-only fallback removed — all target hardware has GPU (even QEMU via VirtIO-GPU)
- AIRS Kit never knows which hardware runs inference — calls Inference Pipeline, Compute Kit routes
- candle becomes one implementation behind the Inference Pipeline trait
- Gaming + AIRS conflict only exists on hardware without NPU; resolved by yield-to-interactive
- `docs/kernel/compute.md` and `docs/platform/gpu.md` need updating
