---
author: claude
date: 2026-03-25
tags: [gpu, ipc, capability, architecture]
status: final
---

# GPU Service Architecture Decisions (Phase 6 M20)

## GPU Service as separate module (not in drivers/)

**Decision**: Place the GPU Service in `kernel/src/gpu/service.rs`, not alongside the VirtIO driver in `drivers/`.

**Why**: The GPU Service is a higher-level abstraction — it manages buffers, double-buffering, and IPC protocol on top of the raw VirtIO-GPU driver. The `kernel/src/gpu/` module will later hold `text.rs` (M21 text rendering) and Kit trait implementations (M22). Keeping it separate from `drivers/` maintains the layering: `drivers/` = hardware transport, `gpu/` = subsystem logic.

**Trade-off**: More cross-module calls (GPU Service → virtio_gpu public wrappers), but cleaner separation of concerns.

## Flat repr(C) structs for IPC protocol (not Rust enums)

**Decision**: `GpuRequest` and `GpuResponse` are flat `repr(C)` structs with a `command: u32` discriminant, not Rust enums with variants.

**Why**: Flat structs copy directly into `RawMessage.data[256]` with `core::ptr::copy_nonoverlapping`. No serialization framework needed. The 256-byte IPC message limit makes Rust enum discriminants + variant data awkward to pack. Flat structs with a manual command field are predictable in size and layout.

**Trade-off**: Less type safety at compile time (command field is u32, not an enum variant). Mitigated by the `GpuCommand` enum for dispatch and `from_status()`/`to_status()` for error round-tripping.

## Capability checks are structural in Phase 6

**Decision**: The GPU Service checks `GpuBufferCreate` capability before allocating buffers, even though all callers are kernel threads that always have the capability.

**Why**: When the GPU Service is extracted to EL0 (future phase), the capability check becomes load-bearing. Adding it now means the code path is tested and the capability variants exist in the enum. Removing a check later is harder than adding one.

## All 6 GpuCommand variants defined upfront

**Decision**: Define all 6 command variants (GetDisplayInfo, AllocateBuffer, ReleaseBuffer, Present, GetBufferInfo, SwapBuffers) in Step 8, even though SwapBuffers is only wired in Step 10.

**Why**: Modifying an enum twice (add 5 variants, then add 1 more) causes unnecessary churn in tests and match arms. Defining the complete enum once is cleaner and avoids intermediate compile errors where a match arm is missing.

## FenceTracker in shared crate

**Decision**: `FenceTracker` lives in `shared/src/gpu.rs` despite being used only by the kernel's GPU Service.

**Why**: FenceTracker is a pure data structure with no hardware dependencies — it tracks monotonic IDs and completion status. Placing it in the shared crate enables comprehensive host-side unit tests (fence allocation, completion, wraparound) that run on the host without QEMU.
