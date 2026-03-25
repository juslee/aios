# Phase 6: GPU & Display

**Tier:** 2 — Core System Services
**Duration:** 6 weeks
**Deliverable:** VirtIO-GPU 2D kernel driver, custom GPU Service (kernel-side IPC service with capability-gated buffer management), Compute Kit Tier 1 (`GpuSurface` trait), bitmap font text rendering on GPU framebuffer
**Status:** M19–M21 Complete, M22 Planned
**Prerequisites:** Phase 5 (Kit Foundation)
**Unlocks:** Phase 7 (Window Compositor & Shell), Phase 23 (Kernel Compute Abstraction)

-----

## Objective

Phase 6 brings graphical display output to AIOS by building the GPU stack from the bottom up. The VirtIO-GPU 2D driver extends the existing VirtIO MMIO transport infrastructure (from Phase 4's block driver) to drive a GPU on QEMU. On top of the driver sits a **custom GPU Service** — a kernel-side IPC service following the echo service pattern from Phase 3 — that manages GPU buffers, enforces capability-gated access, and provides a display pipeline with double-buffered page-flip.

Following the **Custom Core, Open-Source Bridges** principle ([ADR](../knowledge/decisions/2026-03-16-jl-custom-core-principle.md)), the GPU Service is AIOS-native: it implements Compute Kit Tier 1 directly against VirtIO-GPU. wgpu is an optional bridge that goes on top of Compute Kit later (Phase 30+) for ported apps wanting standard WebGPU/Vulkan APIs. Phase 6 does not use wgpu.

By the end of this phase: (1) VirtIO-GPU probes, initializes, and displays content on QEMU; (2) the GPU Service runs as a kernel thread accepting IPC commands for buffer allocation, rendering, and display control; (3) Compute Kit Tier 1 `GpuSurface` trait is defined in `shared/src/kits/compute.rs` with a kernel implementation; (4) boot log text is rendered to the GPU framebuffer via `spleen-font` bitmap glyphs; (5) `just run` (no GPU) still boots normally via GOP framebuffer fallback.

-----

## Architecture References

These existing documents define the technical design. This phase doc focuses on implementation order and acceptance criteria — not duplicating the architecture.

| Topic | Document | Relevant Sections |
|---|---|---|
| GPU & Display overview | [gpu.md](../platform/gpu.md) | §1 Core Insight, §2 Architecture, §19 Implementation Order, §20 Design Principles |
| VirtIO-GPU protocol | [gpu/drivers.md](../platform/gpu/drivers.md) | §3.1 Device identity, §3.2 Virtqueue layout, §3.3 2D commands, §3.4 Fence synchronization, §3.5 2D display flow |
| Display controller | [gpu/display.md](../platform/gpu/display.md) | §6.1 Display model, §6.2 Mode setting, §7.1–7.4 Framebuffer management, double buffering |
| GPU security | [gpu/security.md](../platform/gpu/security.md) | §13 Capability-gated GPU access |
| Compute Kit overview | [kits/kernel/compute.md](../kits/kernel/compute.md) | §2 Core Traits (Tier 1 GpuSurface), §6 Error Handling |
| Kernel compute abstraction | [kernel/compute.md](../kernel/compute.md) | §1 Core Insight, §2 Architecture |
| HAL GpuDevice trait | [kernel/hal.md](../kernel/hal.md) | §4.4 GpuDevice |
| Custom Core principle | [ADR: Custom Core](../knowledge/decisions/2026-03-16-jl-custom-core-principle.md) | Custom GPU Service first, wgpu bridge on top later |
| Compute Kit ADR | [ADR: Compute Kit](../knowledge/decisions/2026-03-22-jl-compute-kit.md) | 3-tier model, GPU as mandatory baseline |
| Subsystem framework | [subsystem-framework.md](../platform/subsystem-framework.md) | §14 Display subsystem pattern |
| Capability system | [security/model/capabilities.md](../security/model/capabilities.md) | §3.1–3.5 Token lifecycle, attenuation |

-----

## Milestones

Milestones are numbered continuously across all phases. Phase 5 used M16–M18; Phase 6 continues with M19–M22.

| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| **M19 — VirtIO-GPU 2D Driver** | 1–6 | End of week 2 | VirtIO-GPU probed, initialized; 2D resource created with DMA backing; solid-color frame visible on QEMU display via `just run-gpu` |
| **M20 — Custom GPU Service** | 7–12 | End of week 4 | Kernel-side IPC service registered as "gpu-service"; capability-gated buffer allocation; double-buffered page-flip; GOP→VirtIO-GPU transition |
| **M21 — Font Rendering & Text Display** | 13–15 | End of week 5 | `spleen-font` 16x32 bitmap text rendered to GPU framebuffer; boot log visible on QEMU display |
| **M22 — Compute Kit Tier 1 & Gate** | 16–20 | End of week 6 | `GpuSurface` trait defined and implemented; all quality gates pass; documentation updated |

-----

## Milestone 19 — VirtIO-GPU 2D Driver (End of Week 2)

*Goal: Probe, initialize, and drive the VirtIO-GPU device on QEMU. Submit 2D commands to display a solid-color frame, proving the full command protocol works end-to-end. Reuses VirtIO MMIO transport from virtio_blk.rs.*

### Step 1: VirtIO-GPU shared types and constants

**What:** Add VirtIO-GPU device ID, command constants, response codes, and `repr(C)` command/response structs to a new `shared/src/gpu.rs` module. These are the wire-format types from drivers.md §3.1–3.3.

**Tasks:**
- [x] Create `shared/src/gpu.rs`
- [x] Add `pub mod gpu;` to `shared/src/lib.rs`
- [x] Add `VIRTIO_DEVICE_ID_GPU: u32 = 16` constant
- [x] Add VirtIO-GPU command type constants: `GET_DISPLAY_INFO` (0x0100), `RESOURCE_CREATE_2D` (0x0101), `RESOURCE_UNREF` (0x0102), `SET_SCANOUT` (0x0103), `RESOURCE_FLUSH` (0x0104), `TRANSFER_TO_HOST_2D` (0x0105), `RESOURCE_ATTACH_BACKING` (0x0106), `RESOURCE_DETACH_BACKING` (0x0107)
- [x] Add response constants: `RESP_OK_NODATA` (0x1100), `RESP_OK_DISPLAY_INFO` (0x1101), error codes (0x1200–0x1205)
- [x] Define `repr(C)` structs: `VirtioGpuCtrlHdr`, `VirtioGpuRect`, `VirtioGpuDisplayOne`, `VirtioGpuRespDisplayInfo`, `VirtioGpuResourceCreate2d`, `VirtioGpuSetScanout`, `VirtioGpuResourceFlush`, `VirtioGpuTransferToHost2d`, `VirtioGpuResourceAttachBacking`, `VirtioGpuMemEntry`, `VirtioGpuResourceUnref`, `VirtioGpuResourceDetachBacking`
- [x] Define `VirtioGpuFormat` enum: `B8G8R8A8Unorm = 1`, `R8G8B8A8Unorm = 67`
- [x] Add `VIRTIO_GPU_FLAG_FENCE: u32 = 1` constant
- [x] Define `GpuPixelFormat` enum for AIOS-native GPU use: `B8G8R8A8`, `R8G8B8A8` (distinct from the boot `PixelFormat` in `shared/src/boot.rs`)
- [x] Define `DisplayInfo` struct: `width: u32`, `height: u32`, `format: GpuPixelFormat`, `scanout_id: u32`
- [x] Define `GpuError` enum: `DeviceNotFound`, `InitFailed`, `CommandFailed`, `OutOfMemory`, `InvalidResource`, `ScanoutFailed`
- [x] Add `Gpu = 13` variant to `Subsystem` enum in `shared/src/observability.rs`, update `Subsystem::COUNT` to 14 and `Subsystem::name()` match arm, and adjust unit tests that assert `COUNT` or the last discriminant
- [x] Write host-side tests: struct size assertions (`core::mem::size_of`) ensuring `repr(C)` layout matches VirtIO spec, `GpuError` derives

**Key reference:** [gpu/drivers.md](../platform/gpu/drivers.md) §3.1–3.3

**Acceptance:** `just check` zero warnings. `just test` passes with new struct size assertions. Existing 394+ tests still pass.

-----

### Step 2: VirtIO-GPU device probe and initialization

**What:** Create `kernel/src/drivers/virtio_gpu.rs` implementing device probe (reusing VirtIO MMIO scan from `virtio_blk.rs`) and initialization following VirtIO spec §3.1. The GPU device needs two virtqueues (controlq index 0, cursorq index 1) but only controlq is used initially.

**Tasks:**
- [x] Create `kernel/src/drivers/virtio_gpu.rs` with `VirtioGpu` struct holding: `base` (MMIO virtual addr), controlq virtqueue state (`desc_virt`/`avail_virt`/`used_virt`), command buffer DMA page (`cmd_phys`/`cmd_virt`), `last_used_idx`, `queue_size`, `next_resource_id`, scanout dimensions
- [x] Implement `probe()`: DTB scan then brute-force MMIO scan (0x0A00_0000–0x0A00_3E00, 512-byte stride), checking for device ID 16 (GPU) instead of 2 (BLK)
- [x] Implement `init_device()`: reset → ACKNOWLEDGE → DRIVER → read features → write driver features (zero for Phase 6 — no 3D) → set GUEST_PAGE_SIZE → setup controlq (queue 0) → allocate DMA pages for virtqueue and command buffer → set DRIVER_OK
- [x] Read config space: `num_scanouts` at offset 0x108
- [x] Implement `pub fn init(dt: &DeviceTree) -> bool` with global `VIRTIO_GPU: Mutex<Option<VirtioGpu>>`
- [x] Add `pub mod virtio_gpu;` to `kernel/src/drivers/mod.rs`

**Note:** The probe reuses the same MMIO slot range as virtio_blk. Each slot has a unique device at a unique address — the GPU will appear at a different slot than the block device. QEMU flag: `-device virtio-gpu-device`.

**Key reference:** [gpu/drivers.md](../platform/gpu/drivers.md) §3.1–3.2; VirtIO-blk driver pattern in `kernel/src/drivers/virtio_blk.rs`

**Acceptance:** `just check` zero warnings. QEMU boot with `-device virtio-gpu-device` logs "VirtIO-GPU: found at 0x..." and "VirtIO-GPU: initialized, N scanouts" to UART.

-----

### Step 3: Command submission and display info query

**What:** Implement the generic command submission function that sends a command on the controlq and polls for the response. Use it to query display info (scanout dimensions).

**Tasks:**
- [x] Implement `submit_command(&mut self, cmd: &[u8], resp: &mut [u8])`: builds a 2-descriptor chain (device-readable command, device-writable response), posts to available ring, notifies device, polls used ring for completion, verifies response header
- [x] Implement `submit_command_with_extra(&mut self, cmd: &[u8], extra: &[u8], resp: &mut [u8])` for commands needing additional data (e.g., `RESOURCE_ATTACH_BACKING` followed by `VirtioGpuMemEntry` array)
- [x] Implement `get_display_info(&mut self) -> Result<DisplayInfo, GpuError>`: sends `GET_DISPLAY_INFO`, parses `VirtioGpuRespDisplayInfo`, extracts first enabled scanout's width/height
- [x] Call `get_display_info()` at end of `init_device()`, store in struct, log dimensions

**Key reference:** [gpu/drivers.md](../platform/gpu/drivers.md) §3.3 (GET_DISPLAY_INFO); `kernel/src/drivers/virtio_blk.rs` `submit_request()` pattern

**Acceptance:** `just check` zero warnings. QEMU boot logs "VirtIO-GPU: scanout 0: WxH" (e.g., 1280x800).

-----

### Step 4: Resource creation and backing attachment

**What:** Implement the VirtIO-GPU 2D resource lifecycle: create a 2D resource, allocate DMA-backed framebuffer pages, and attach the backing memory.

**Tasks:**
- [x] Implement `resource_create_2d(&mut self, resource_id: u32, format: u32, width: u32, height: u32) -> Result<(), GpuError>`
- [x] Implement `resource_attach_backing(&mut self, resource_id: u32, entries: &[VirtioGpuMemEntry]) -> Result<(), GpuError>` using `submit_command_with_extra()`
- [x] Implement `resource_detach_backing(&mut self, resource_id: u32)` and `resource_unref(&mut self, resource_id: u32)` for cleanup
- [x] Implement `allocate_framebuffer(&mut self, width: u32, height: u32) -> Result<GpuBufferHandle, GpuError>`: computes total bytes (width × height × 4 for BGRA8), allocates DMA pages from `Pool::Dma`, zeros them, creates resource, attaches backing, returns handle with physical/virtual addresses
- [x] Define `GpuBufferHandle` struct: `resource_id: u32`, `width: u32`, `height: u32`, `format: GpuPixelFormat`, `stride: u32`, `fb_phys: usize`, `fb_virt: usize`, `page_count: usize`

**Key reference:** [gpu/drivers.md](../platform/gpu/drivers.md) §3.3 (RESOURCE_CREATE_2D, RESOURCE_ATTACH_BACKING), §3.5 (2D display flow)

**Acceptance:** `just check` zero warnings. QEMU boot logs "VirtIO-GPU: resource 1 created (WxH B8G8R8A8)" and "VirtIO-GPU: backing attached (N pages)".

-----

### Step 5: Scanout configuration and first frame display

**What:** Bind the resource to scanout 0, fill the framebuffer with a solid color, transfer to host, and flush. This proves the full 2D pipeline works. Add the `run-gpu` recipe to the justfile.

**Tasks:**
- [x] Implement `set_scanout(&mut self, scanout_id: u32, resource_id: u32, rect: &VirtioGpuRect) -> Result<(), GpuError>`
- [x] Implement `transfer_to_host_2d(&mut self, resource_id: u32, rect: &VirtioGpuRect, offset: u64) -> Result<(), GpuError>`
- [x] Implement `resource_flush(&mut self, resource_id: u32, rect: &VirtioGpuRect) -> Result<(), GpuError>`
- [x] Implement `present_frame(&mut self, handle: &GpuBufferHandle) -> Result<(), GpuError>`: convenience that calls `transfer_to_host_2d` then `resource_flush` for the full rectangle
- [x] In init: after resource creation, call `set_scanout(0, resource_id, full_rect)`, fill framebuffer with AIOS blue (#5B8CFF as B8G8R8A8), call `present_frame()`, log "VirtIO-GPU: first frame displayed"
- [x] Add `drivers::virtio_gpu::init(&dt)` call in `kernel_main` after storage init
- [x] Add `run-gpu` recipe to justfile: `-serial stdio`, `-device virtio-gpu-device`, no `-device ramfb`

**Key reference:** [gpu/drivers.md](../platform/gpu/drivers.md) §3.3 (SET_SCANOUT, TRANSFER_TO_HOST_2D, RESOURCE_FLUSH), §3.5 (2D display flow)

**Acceptance:** `just check` zero warnings. `just run-gpu` shows solid blue (#5B8CFF) on the QEMU display window. UART logs "VirtIO-GPU: first frame displayed". `just run` (without GPU) still boots normally.

-----

### Step 6: Shared crate refactoring

**What:** Review kernel/ code from M19 steps, ensure shared types are properly placed.

**Tasks:**
- [x] Verify VirtIO-GPU wire types are in `shared/src/gpu.rs` (not kernel/)
- [x] Verify `GpuBufferHandle`, `DisplayInfo`, `GpuError`, `GpuPixelFormat` are in shared crate
- [x] Write additional host-side tests for new shared types

**Acceptance:** `just check` + `just test` pass.

-----

## Milestone 20 — Custom GPU Service (End of Week 4)

*Goal: Build the AIOS-native GPU Service as a kernel-side IPC service following the echo service pattern. Implement capability-gated buffer management, double-buffered page-flip, and the GOP→VirtIO-GPU display transition.*

### Step 7: GPU capability types

**What:** Extend the `Capability` enum with GPU-specific variants for capability-gated access to GPU resources.

**Tasks:**
- [x] Add to `Capability` enum in `shared/src/cap.rs`: `GpuMmioAccess` (access GPU MMIO region), `GpuBufferCreate` (allocate GPU buffers), `GpuBufferAccess(u32)` (access specific buffer by resource ID), `DisplayControl` (configure display scanout)
- [x] Update `permits()` match arms for the new variants (exact match for simple, resource ID match for parameterized)
- [x] Update `can_attenuate_to()`: `GpuBufferCreate` can attenuate to `GpuBufferAccess(id)`
- [x] Write host-side tests for new capability variants: permits, denies cross-variant, attenuation rules

**Key reference:** [security/model/capabilities.md](../security/model/capabilities.md) §3.1–3.5; existing `permits()` and `can_attenuate_to()` patterns in `shared/src/cap.rs`

**Acceptance:** `just check` zero warnings. `just test` passes with new capability tests.

-----

### Step 8: GPU Service IPC protocol types

**What:** Define the GPU Service IPC command/response types in the shared crate. These are the message formats exchanged over IPC channels.

**Tasks:**
- [x] Add `GpuCommand` enum to `shared/src/gpu.rs`: `GetDisplayInfo = 1`, `AllocateBuffer = 2`, `ReleaseBuffer = 3`, `Present = 4`, `GetBufferInfo = 5`, `SwapBuffers = 6`
- [x] Define `GpuRequest` struct (repr(C), fits in `RawMessage.data[256]`): `command: u32` discriminant (matches `GpuCommand` values), followed by command-specific fields (width/height/format for AllocateBuffer, resource_id for ReleaseBuffer/Present/GetBufferInfo, damage rect for Present)
- [x] Define `GpuResponse` struct (repr(C)): `status: i32` (0 = success, negative = `GpuError` via `to_status()`/`from_status()`), followed by response-specific fields (DisplayInfo for GetDisplayInfo, resource_id for AllocateBuffer, buffer info for GetBufferInfo)
- [x] Write host-side tests: command/response struct sizes fit within `MAX_MESSAGE_SIZE` (256 bytes), round-trip serialization

**Key reference:** IPC message format in `shared/src/ipc.rs` (`RawMessage`, `MAX_MESSAGE_SIZE = 256`)

**Acceptance:** `just check` zero warnings. `just test` passes with message size assertions.

-----

### Step 9: GPU Service kernel thread

**What:** Create the GPU Service as a kernel thread that registers an IPC channel and processes GPU commands. Follows the echo service pattern from `kernel/src/service/mod.rs`.

**Tasks:**
- [x] Create `kernel/src/gpu/mod.rs` with `pub mod service;`
- [x] Create `kernel/src/gpu/service.rs` with `gpu_service_loop()` entry function
- [x] In `gpu_service_loop()`: create IPC channel, register as "gpu-service" via `service_register()`, loop on `ipc_recv()`, decode `GpuCommand` from message data, dispatch to handler, `ipc_reply()` with response
- [x] Implement `handle_get_display_info()`: reads display info from VirtIO-GPU driver, packs into response
- [x] Implement `handle_allocate_buffer()`: allocates a new VirtIO-GPU framebuffer (via the driver) and returns resource_id (per-command capability enforcement deferred — IPC-layer ChannelAccess is the Phase 6 enforcement point)
- [x] Implement `handle_release_buffer()`: detaches backing, unrefs resource, frees DMA pages
- [x] Implement `handle_present()`: calls `transfer_to_host_2d` + `resource_flush` for the specified damage region
- [x] Implement `handle_get_buffer_info()`: returns DMA virtual address, width, height, stride for a resource
- [x] Spawn the GPU Service thread in `kernel_main` after VirtIO-GPU init succeeds, with `SchedulerClass::Interactive` priority

**Note:** The GPU Service thread runs in kernel space (EL1) for Phase 6. When userspace process execution is available, this service will be extracted to an EL0 privileged process. The IPC protocol remains identical.

**Key reference:** `kernel/src/service/mod.rs` (service registration, echo service pattern); `kernel/src/ipc/` (channel create, recv, reply)

**Acceptance:** `just check` zero warnings. QEMU boot logs "Service 'gpu-service' registered (pid=9, ch=N)". GPU commands dispatched via IPC work.

-----

### Step 10: Double-buffered page-flip

**What:** Extend the GPU Service to manage two framebuffers (front and back) and implement page-flip by alternating which resource is bound to the scanout.

**Tasks:**
- [x] Allocate two VirtIO-GPU resources at GPU Service init (front/back, dynamically assigned IDs)
- [x] Track `front_buffer` and `back_buffer` as `GpuBufferHandle` in the service state
- [x] Implement `swap_buffers()`: swaps front/back handles, calls `set_scanout()` to bind new front, `transfer_to_host_2d()` + `resource_flush()` for new front
- [x] Add `SwapBuffers = 6` to `GpuCommand` enum for IPC-triggered swap
- [x] Implement `FenceTracker` struct: `next_id: u64`, `last_completed: u64` with `allocate()`, `complete()`, `is_complete()` methods
- [x] For now, use unfenced synchronous `resource_flush()`; defer real fenced completion tracking (VIRTIO_GPU_FLAG_FENCE + IRQ) to Phase 7+ when interrupt-driven VirtIO is available

**Key reference:** [gpu/display.md](../platform/gpu/display.md) §7.2 (double buffering), §7.3 (page flip); [gpu/drivers.md](../platform/gpu/drivers.md) §3.4 (fences)

**Acceptance:** `just check` zero warnings. QEMU display shows alternating frames (render to back buffer, swap, render new content to freed buffer). UART logs "VirtIO-GPU: double buffering enabled".

-----

### Step 11: GOP → VirtIO-GPU transition

**What:** When VirtIO-GPU is available, transition display output from the GOP framebuffer to VirtIO-GPU. When VirtIO-GPU is not available, GOP framebuffer remains active as fallback.

**Tasks:**
- [x] Modify `kernel_main` GPU init section: after `virtio_gpu::init()` succeeds, release test frame via direct driver calls (pre-scheduler); GPU Service allocates double buffers and renders AIOS blue when scheduler starts
- [x] When VirtIO-GPU is not present (`just run` without `-device virtio-gpu-device`), skip GPU init, log fallback to GOP
- [x] Add boot phase `EarlyBootPhase::GpuReady` variant before `Complete` (keeping `Complete` as the last variant and updating phase-count constants/tests as needed)
- [x] Advance boot phase to `GpuReady` after successful VirtIO-GPU init + GPU Service start
- [x] `framebuffer.rs` is NOT modified — it remains as the GOP fallback path

**Key reference:** `kernel/src/framebuffer.rs` (GOP framebuffer); `kernel/src/main.rs` (boot sequence)

**Acceptance:** `just check` zero warnings. `just run-gpu` displays AIOS test pattern via VirtIO-GPU. `just run` (no GPU) still boots and shows test pattern via GOP framebuffer.

-----

### Step 12: Shared crate refactoring and docs update for M19–M20

**What:** Move pure data structures to shared crate, add host-side tests, update documentation.

**Tasks:**
- [x] Verify all GPU types with no hardware deps are in `shared/src/gpu.rs`
- [x] Write additional host-side tests for `FenceTracker`, `GpuCommand` dispatch logic, `GpuBufferHandle` field access
- [x] Update `CLAUDE.md`: Workspace Layout (new files), Key Technical Facts (VirtIO-GPU constants, display dimensions, device ID 16, spleen-font)
- [x] Update `README.md`: Project Structure, Build Commands (new `run-gpu` recipe)
- [x] Update phase doc: check off M19 and M20 steps

**Acceptance:** `just check` + `just test` pass. Documentation is accurate.

-----

## Milestone 21 — Font Rendering & Text Display (End of Week 5)

*Goal: Integrate spleen-font for 16x32 bitmap glyph rendering. Display boot log text on the VirtIO-GPU framebuffer. This gives the kernel a visual diagnostic output beyond UART.*

### Step 13: spleen-font integration and glyph renderer

**What:** Add `spleen-font` as a kernel dependency and implement a glyph rendering function that blits bitmap characters to the GPU framebuffer.

**Tasks:**
- [x] Add `spleen-font = { version = "0.2", default-features = false, features = ["s16x32"] }` to `kernel/Cargo.toml`
- [x] Verify `spleen-font` compiles for `aarch64-unknown-none` (no_std, no alloc)
- [x] Create `kernel/src/gpu/text.rs` with `FbInfo` struct (fb, stride_px, width, height) and `blit_glyph(font: &mut PSF2Font, fb: &FbInfo, ch: char, x: i32, y: i32, fg: u32, bg: u32)` — reads the 16x32 bitmap from spleen-font, writes pixels to framebuffer via regular `ptr::write` (WB Cacheable memory)
- [x] Handle boundary clipping (glyph partially off-screen)
- [x] Test with a single character rendered to the VirtIO-GPU back buffer

**Note:** spleen-font provides PSF2 bitmap glyphs. Each glyph is 16 pixels wide × 32 pixels tall. The renderer iterates 32 rows × 16 columns, checking each bit, and writes either `fg` or `bg` to the framebuffer. No alpha blending needed.

**Key reference:** [gpu/rendering.md](../platform/gpu/rendering.md) §11 (font rendering pipeline — simplified for bitmap); `spleen-font` crate API

**Acceptance:** `just check` zero warnings. QEMU display shows a single visible character rendered via spleen-font.

-----

### Step 14: Text rendering and boot log display

**What:** Implement `draw_text()` and `draw_boot_log()` functions that render strings and kernel log entries to the GPU framebuffer.

**Tasks:**
- [x] Implement `draw_text(font: &mut PSF2Font, fb: &FbInfo, text: &str, start_x: i32, start_y: i32, fg: u32, bg: u32)`: iterates characters, calls `blit_glyph` for each, advances cursor by 16 pixels per character
- [x] Handle newline (`\n`): advance Y by 32, reset X to starting position
- [x] Handle line wrapping: when X exceeds framebuffer width, wrap to next line
- [x] Implement `draw_boot_log(fb: &FbInfo)`: retrieves boot log entries via `observability::take_boot_log()` from `BootLogBuffer`, renders them as light grey text on dark background
- [x] Wire `draw_boot_log()` into `gpu_service_loop()` after `init_double_buffering()`: fill back buffer with dark background (#1A1A2E), draw boot log text, swap buffers to present
- [x] Calculate how many log lines fit on screen: (height / 32) lines, (width / 16) chars per line

**Key reference:** `kernel/src/observability/mod.rs` (BootLogBuffer, take_boot_log)

**Acceptance:** `just check` zero warnings. `just run-gpu` displays boot log text visually on the QEMU display window. Text is legible and properly positioned.

-----

### Step 15: Shared crate refactoring and docs update for M21

**What:** Move any sharable text rendering types to shared crate, update documentation.

**Tasks:**
- [x] Move text color constants or text layout types to `shared/src/gpu.rs` if applicable
- [x] Update `CLAUDE.md`: Workspace Layout (new `kernel/src/gpu/` module), Key Technical Facts (spleen-font version, glyph dimensions 16x32)
- [x] Update `kernel/Cargo.toml` deps listing in CLAUDE.md
- [x] Update phase doc status

**Acceptance:** `just check` + `just test` pass. Documentation is accurate.

-----

## Milestone 22 — Compute Kit Tier 1 & Gate (End of Week 6)

*Goal: Define the Compute Kit Tier 1 `GpuSurface` trait in the Kit module, implement it in kernel against the GPU Service, and pass all quality gates.*

### Step 16: Compute Kit module and error types

**What:** Create `shared/src/kits/compute.rs` with `ComputeError` enum and supporting types for the `GpuSurface` trait.

**Tasks:**
- [x] Add `pub mod compute;` to `shared/src/kits/mod.rs`
- [x] Add `pub use kits::compute as compute_kit;` to `shared/src/lib.rs`
- [x] Define `ComputeError` enum: `DeviceUnavailable`, `AllocationFailed`, `InvalidParameters`, `ServiceError`, `PermissionDenied`
- [x] Derive `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq` on `ComputeError`
- [x] Define `SurfaceBuffer` struct: `id: u32` (resource_id), `width: u32`, `height: u32`, `format: GpuPixelFormat`, `fb_virt: usize`, `stride: u32`
- [x] Define `DamageRect` struct: `x: u32`, `y: u32`, `width: u32`, `height: u32`
- [x] Define `SemanticHint` enum: `UiText`, `VideoPlayback`, `Rendering3D`, `ScrollingContent`, `StaticContent`
- [x] Write host-side tests: `ComputeError` derives, struct field access, `SemanticHint` enum coverage

**Key reference:** [kits/kernel/compute.md](../kits/kernel/compute.md) §2 (Tier 1 GpuSurface), §6 (ComputeError)

**Acceptance:** `just check` zero warnings. `just test` passes with new Compute Kit tests.

-----

### Step 17: GpuSurface trait definition

**What:** Define the `GpuSurface` trait for Compute Kit Tier 1, following the pattern established by Memory Kit and IPC Kit.

**Tasks:**
- [x] Define `GpuSurface` trait in `shared/src/kits/compute.rs`:
  - `fn allocate_buffer(&self, width: u32, height: u32, format: GpuPixelFormat) -> Result<SurfaceBuffer, ComputeError>`
  - `fn submit_damage(&self, buffer: &SurfaceBuffer, damage: &[DamageRect]) -> Result<(), ComputeError>`
  - `fn set_semantic_hint(&self, hint: SemanticHint) -> Result<(), ComputeError>`
  - `fn request_direct_scanout(&self) -> Result<bool, ComputeError>`
- [x] Ensure trait is dyn-compatible (object-safe)
- [x] Write host-side test: `fn _assert_object_safe(_: &dyn GpuSurface) {}` compile-time check

**Key reference:** [kits/kernel/compute.md](../kits/kernel/compute.md) §2 (GpuSurface trait definition)

**Acceptance:** `just check` zero warnings. `just test` passes including dyn-compatibility test.

-----

### Step 18: Kernel GpuSurface implementation

**What:** Implement `GpuSurface` for the kernel's GPU Service, following the `KernelFrameAllocator` / `KernelCapabilitySystem` zero-sized wrapper pattern.

**Tasks:**
- [x] Create zero-sized `KernelGpuSurface` struct in `kernel/src/gpu/mod.rs`
- [x] Implement `GpuSurface for KernelGpuSurface`:
  - `allocate_buffer`: sends `AllocateBuffer` command to GPU Service via IPC
  - `submit_damage`: sends `Present` command with damage rect to GPU Service
  - `set_semantic_hint`: stores hint in GPU Service state (informational — used by compositor in Phase 7+)
  - `request_direct_scanout`: returns `Ok(true)` (single surface, no compositor yet)
- [x] Test: allocate buffer via `KernelGpuSurface`, render pixels, submit damage, verify display updates

**Key reference:** [kits/kernel/compute.md](../kits/kernel/compute.md) §2; Kit kernel wrapper pattern from Phase 5 (`KernelFrameAllocator` in `kernel/src/mm/frame.rs`)

**Acceptance:** `just check` zero warnings. QEMU display shows content rendered via `KernelGpuSurface::allocate_buffer()` + `submit_damage()`.

-----

### Step 19: Shared crate refactoring

**What:** Final review of kernel/ code from M22, ensure all sharable types are in shared crate.

**Tasks:**
- [x] Review `kernel/src/gpu/` for types that can move to `shared/src/kits/compute.rs` or `shared/src/gpu.rs`
- [x] Write additional host-side tests for Compute Kit types
- [x] Verify all 12+ Kit traits (including new `GpuSurface`) are dyn-compatible

**Acceptance:** `just check` + `just test` pass.

-----

### Step 20: Quality gate and final documentation

**What:** Run all quality gates, update documentation, run audit loop.

**Tasks:**
- [ ] `just check` — zero warnings, zero errors
- [ ] `just test` — all host-side tests pass (verify count increased from 394)
- [ ] `just run` — boots normally without VirtIO-GPU (GOP framebuffer path works)
- [ ] `just run-gpu` — boots with VirtIO-GPU, displays boot log text on screen
- [ ] Update `CLAUDE.md`: Workspace Layout (all new files), Key Technical Facts (VirtIO-GPU device ID 16, GpuCommand enum, spleen-font, Compute Kit Tier 1), Architecture Doc Map (Compute Kit reference)
- [ ] Update `README.md`: Project Structure, Build Commands
- [ ] Update `docs/project/developer-guide.md`: new files, test counts, GPU patterns
- [ ] Update phase doc: all steps checked, Status = Complete
- [ ] Update Kit docs: `docs/kits/kernel/compute.md` if trait signatures deviate from spec
- [ ] Run full audit loop until 0 issues
- [ ] Dead code cleanup: remove `#[allow(dead_code)]` if unused

**Key reference:** `.claude/rules/02-quality-gates.md`, `.claude/rules/08-knowledge-hive.md`

**Acceptance:** All quality gates pass. Audit loop returns 0 issues. Phase doc shows all steps complete.

-----

## Decision Points

| Decision | Options | Recommendation | Rationale |
|---|---|---|---|
| GPU types module | `shared/src/gpu.rs` (new) vs extend `shared/src/storage.rs` | New `shared/src/gpu.rs` | Separation of concerns; GPU and storage are distinct subsystems |
| Font crate | spleen-font vs noto-sans-mono-bitmap vs font8x8 vs embedded VGA | spleen-font 16x32 | BSD-2-Clause license match, ~40 KB, professional quality, no_std/no-alloc |
| GPU Service location | `kernel/src/gpu/service.rs` vs `kernel/src/drivers/` vs `kernel/src/service/` | `kernel/src/gpu/service.rs` | Higher-level than a driver, distinct from the VirtIO-blk-style raw driver code |
| Display abstraction scope | Full atomic modesetting vs simplified DisplayInfo | Simplified `DisplayInfo` for Phase 6 | VirtIO-GPU has 1 scanout, no overlays. Full model for Phase 7+ compositor |
| QEMU display recipe | `-display cocoa` vs default display backend | Default (no explicit `-display` flag) | QEMU auto-selects platform-native display; `-serial stdio` for UART |

-----

## Phase Completion Criteria

- [x] `just check` — zero warnings
- [x] `just test` — all pass, test count > 394
- [x] `just run` — boots normally without VirtIO-GPU (GOP framebuffer fallback)
- [x] `just run-gpu` — VirtIO-GPU display with boot log text visible
- [x] VirtIO-GPU 2D driver probes, initializes, and displays content on QEMU
- [x] Custom GPU Service runs as kernel IPC service with capability-gated access
- [x] Double-buffered page flip works (buffer swap demonstrated)
- [x] spleen-font bitmap text renders legibly on GPU framebuffer
- [x] Compute Kit Tier 1 `GpuSurface` trait defined and implemented
- [x] All existing functionality (UART, storage, IPC, scheduler) unaffected
- [x] Audit loop clean across all three categories
