---
author: claude
date: 2026-03-25
tags: [gpu, drivers, virtio, display, dma]
status: in-progress
phase: 6
milestone: M19
---

# Plan: Phase 6 M19 — VirtIO-GPU 2D Driver

## Approach

Phase 6 M19 brings the first GPU driver to AIOS. Prior milestones built the VirtIO MMIO transport (Phase 4 VirtIO-blk), DMA memory pools (Phase 2 M7), and IPC channels (Phase 3). M19 reuses the same legacy MMIO transport pattern to probe and initialize a VirtIO-GPU device on QEMU, then drives a 2D display pipeline: resource creation → DMA-backed framebuffer → scanout configuration → transfer-to-host → flush.

The VirtIO-GPU driver follows the same structural pattern as `kernel/src/drivers/virtio_blk.rs`: probe via DTB+brute-force MMIO scan, negotiate features, setup controlq virtqueue, allocate DMA pages, expose a `Mutex<Option<VirtioGpu>>` global.

**Key gaps found during exploration:**

1. `VIRTIO_DEVICE_ID_GPU = 16` not yet defined in `shared/src/storage.rs` — must add
2. `Subsystem::Gpu` variant missing from observability enum — must add (becomes variant 13, COUNT→14)
3. No `shared/src/gpu.rs` module exists — must create for wire-format types
4. GPU uses 2-descriptor chains (cmd readable + resp writable) vs block's 3-descriptor chains — `submit_command()` pattern differs from `submit_request()`
5. GPU needs a `submit_command_with_extra()` for `RESOURCE_ATTACH_BACKING` which sends cmd header + extra mem_entry array as a single device-readable descriptor (or 2 chained readable descriptors)

**Shared crate plan:** All VirtIO-GPU wire-format types (`VirtioGpuCtrlHdr`, `VirtioGpuRect`, command/response structs, `GpuError`, `DisplayInfo`, `GpuPixelFormat`, `GpuBufferHandle`) go in `shared/src/gpu.rs` with host-side size assertions. Only driver state (`VirtioGpu` struct with MMIO base, virtqueue pointers) stays in kernel.

**VirtIO common infrastructure:** `VirtqDesc`, `VIRTQ_DESC_F_*` constants, and virtqueue layout helpers (`avail_offset`, `used_offset`, `virtqueue_size`, `order_for_pages`) are currently local to `virtio_blk.rs`. For M19, we extract these to a new `kernel/src/drivers/virtio_common.rs` module shared by both block and GPU drivers. This avoids code duplication and ensures consistent virtqueue management.

**Framebuffer DMA strategy:** A 1024x768×4B framebuffer = 3MB. Using individual 4K pages would require 768 `VirtioGpuMemEntry` structs (12KB), exceeding the cmd buffer DMA page. Instead, allocate the framebuffer as a single contiguous DMA region using `alloc_dma_pages(order)` with order high enough to cover 3MB (order 10 = 4MB). This yields exactly 1 `VirtioGpuMemEntry`, keeping ATTACH_BACKING simple.

## Progress

- [ ] Step 1: VirtIO-GPU shared types and constants
  - [ ] 1a: Create `shared/src/gpu.rs` with all `repr(C)` wire-format structs from VirtIO spec
  - [ ] 1b: Add `pub mod gpu;` to `shared/src/lib.rs`
  - [ ] 1c: Add `VIRTIO_DEVICE_ID_GPU: u32 = 16` to `shared/src/storage.rs` (alongside existing `VIRTIO_DEVICE_ID_BLK`)
  - [ ] 1d: Define command type constants: `GET_DISPLAY_INFO` (0x0100), `RESOURCE_CREATE_2D` (0x0101), `RESOURCE_UNREF` (0x0102), `SET_SCANOUT` (0x0103), `RESOURCE_FLUSH` (0x0104), `TRANSFER_TO_HOST_2D` (0x0105), `RESOURCE_ATTACH_BACKING` (0x0106), `RESOURCE_DETACH_BACKING` (0x0107)
  - [ ] 1e: Define response constants: `RESP_OK_NODATA` (0x1100), `RESP_OK_DISPLAY_INFO` (0x1101), error codes (0x1200–0x1205)
  - [ ] 1f: Define `repr(C)` structs: `VirtioGpuCtrlHdr` (24B: type_ u32, flags u32, fence_id u64, ctx_id u32, ring_idx u8, padding [u8;3]), `VirtioGpuRect` (16B), `VirtioGpuDisplayOne` (24B), `VirtioGpuRespDisplayInfo` (24 + 16*24 = 408B), `VirtioGpuResourceCreate2d`, `VirtioGpuSetScanout`, `VirtioGpuResourceFlush`, `VirtioGpuTransferToHost2d`, `VirtioGpuResourceAttachBacking`, `VirtioGpuMemEntry` (16B), `VirtioGpuResourceUnref`, `VirtioGpuResourceDetachBacking`
  - [ ] 1g: Define `VirtioGpuFormat` enum: `B8G8R8A8Unorm = 1`, `R8G8B8A8Unorm = 67`
  - [ ] 1h: Define `VIRTIO_GPU_FLAG_FENCE: u32 = 1`
  - [ ] 1i: Define AIOS-native types: `GpuPixelFormat` enum (`B8G8R8A8`, `R8G8B8A8`), `DisplayInfo` struct, `GpuError` enum, `GpuBufferHandle` struct
  - [ ] 1j: Add `Gpu = 13` to `Subsystem` enum in `shared/src/observability.rs`, update `COUNT` to 14, add `name()` match arm `"Gpu  "`, fix unit tests
  - [ ] 1k: Write host-side tests: struct size assertions (`size_of::<VirtioGpuCtrlHdr>() == 24`, etc.), `GpuError` derives, `DisplayInfo` field access
  - [ ] 1l: Verify: `just check` + `just test`

- [ ] Step 2: Extract VirtIO common helpers to shared module
  - [ ] 2a: Create `kernel/src/drivers/virtio_common.rs` with the functions/constants that are currently duplicated locally in `virtio_blk.rs`: `mmio_read32()`/`mmio_write32()` (MMIO access helpers), `avail_offset()`, `used_offset()`, `virtqueue_size()` (virtqueue layout math), plus constants `QUEUE_SIZE: u16 = 128`, `POLL_TIMEOUT: u32 = 10_000_000`, `VIRT_PAGE_SIZE: usize = 4096`
  - [ ] 2b: Note: `VirtqDesc`, `VIRTQ_DESC_F_*`, all MMIO register offsets, status constants are already in `shared/src/storage.rs`; `order_for_pages()` is already in `shared/src/memory.rs` (re-exported as `shared::order_for_pages`). Do NOT duplicate any of these. The `virtio_common.rs` module only holds kernel-side virtqueue layout helpers and MMIO access functions.
  - [ ] 2c: Refactor `virtio_blk.rs` to `use super::virtio_common::*` instead of local definitions
  - [ ] 2d: Add `pub mod virtio_common;` to `kernel/src/drivers/mod.rs`
  - [ ] 2e: Verify: `just check` + `just test` + `just run` (existing block driver still works)

- [ ] Step 3: VirtIO-GPU device probe and initialization
  - [ ] 3a: Create `kernel/src/drivers/virtio_gpu.rs` with `VirtioGpu` struct: `base: usize`, controlq virtqueue state (`desc_virt`/`avail_virt`/`used_virt`), command buffer DMA page (`cmd_phys`/`cmd_virt`), `last_used_idx: u16`, `queue_size: u16`, `next_resource_id: u32`, `display: DisplayInfo`
  - [ ] 3b: Implement `probe(dt: &DeviceTree) -> Option<usize>`: DTB scan then brute-force MMIO scan, checking device ID == 16. When a slot has device_id=16 but version != 1, log `kwarn!(Gpu, "VirtIO-GPU: found at {:#x} but version={}, expected 1", phys, version)` and continue scanning (don't silently skip)
  - [ ] 3c: Implement `init_device(base: usize) -> Result<VirtioGpu, GpuError>`: reset → ACKNOWLEDGE → DRIVER → read features → write driver_features (0 for 2D-only) → GUEST_PAGE_SIZE → setup controlq (queue 0) → allocate DMA pages for virtqueue + cmd buffer → DRIVER_OK → read config `num_scanouts` at offset 0x108
  - [ ] 3d: Implement `pub fn init(dt: &DeviceTree) -> bool` with global `VIRTIO_GPU: Mutex<Option<VirtioGpu>>`
  - [ ] 3e: Add `pub mod virtio_gpu;` to `kernel/src/drivers/mod.rs`
  - [ ] 3f: Verify: `just check`

- [ ] Step 4: Command submission and display info query
  - [ ] 4a: Implement `submit_command(&mut self, cmd: &[u8], resp: &mut [u8])`: copy cmd to DMA page, build 2-descriptor chain (desc[0]: device-readable cmd, desc[1]: device-writable resp), post to available ring, notify device (write queue index to QUEUE_NOTIFY), poll used ring, copy response out. Uses `VirtqDesc` and helpers from `virtio_common`.
  - [ ] 4b: Implement `submit_command_with_extra(&mut self, cmd: &[u8], extra: &[u8], resp: &mut [u8])`: builds 3-descriptor chain (cmd readable, extra readable, resp writable) for RESOURCE_ATTACH_BACKING. Extra data also copied to DMA page at offset after cmd.
  - [ ] 4c: Implement `get_display_info(&mut self) -> Result<DisplayInfo, GpuError>`: build `VirtioGpuCtrlHdr` with type=GET_DISPLAY_INFO, submit, parse `VirtioGpuRespDisplayInfo`, find first enabled scanout
  - [ ] 4d: Call `get_display_info()` at end of `init_device()`, store result in struct, log dimensions
  - [ ] 4e: Verify: `just check`. Note: QEMU verification deferred to Step 6 when `run-gpu` recipe exists

- [ ] Step 5: Resource creation and backing attachment
  - [ ] 5a: Implement `resource_create_2d(&mut self, resource_id: u32, format: u32, width: u32, height: u32) -> Result<(), GpuError>`
  - [ ] 5b: Implement `resource_attach_backing(&mut self, resource_id: u32, entries: &[VirtioGpuMemEntry]) -> Result<(), GpuError>` using `submit_command_with_extra()`
  - [ ] 5c: Implement `resource_detach_backing(&mut self, resource_id: u32)` and `resource_unref(&mut self, resource_id: u32)`
  - [ ] 5d: Implement `allocate_framebuffer(&mut self, width: u32, height: u32) -> Result<GpuBufferHandle, GpuError>`: compute bytes (w×h×4), check if order > MAX_ORDER (10) — if so, clamp dimensions and log warning. Alloc contiguous DMA region via `alloc_dma_pages(order)`, zero pages, create resource, attach backing with single `VirtioGpuMemEntry` (one contiguous region), return handle. Max framebuffer = 4MB = ~1024×1024 at 4bpp.
  - [ ] 5e: Verify: `just check`

- [ ] Step 6: Scanout configuration and first frame display
  - [ ] 6a: Implement `set_scanout(&mut self, scanout_id: u32, resource_id: u32, rect: &VirtioGpuRect) -> Result<(), GpuError>`
  - [ ] 6b: Implement `transfer_to_host_2d(&mut self, resource_id: u32, rect: &VirtioGpuRect, offset: u64) -> Result<(), GpuError>`
  - [ ] 6c: Implement `resource_flush(&mut self, resource_id: u32, rect: &VirtioGpuRect) -> Result<(), GpuError>`
  - [ ] 6d: Implement `present_frame(&mut self, handle: &GpuBufferHandle) -> Result<(), GpuError>`: transfer_to_host_2d + resource_flush for full rect
  - [ ] 6e: In init: after resource creation, `set_scanout(0, resource_id, full_rect)`, fill framebuffer with AIOS blue (#5B8CFF as B8G8R8A8 = 0xFF5B8CFF little-endian), call `present_frame()`, log "VirtIO-GPU: first frame displayed"
  - [ ] 6f: Add `drivers::virtio_gpu::init(&dt)` call in `kernel_main` after storage init
  - [ ] 6g: Add `run-gpu` recipe to justfile: same as `run-display` but replace `-device ramfb` with `-device virtio-gpu-device`. If QEMU probe fails (version mismatch), try adding `disable-modern=on` or `disable-legacy=off` flag and document in plan Issues section
  - [ ] 6h: Verify: `just check`. `just run-gpu` shows solid blue on QEMU display. `just run` (no GPU) still boots normally.

- [ ] Step 7: Shared crate refactoring
  - [ ] 7a: Verify all VirtIO-GPU wire types are in `shared/src/gpu.rs` (not kernel/)
  - [ ] 7b: Verify `GpuBufferHandle`, `DisplayInfo`, `GpuError`, `GpuPixelFormat` are in shared crate
  - [ ] 7c: Write additional host-side tests: `GpuBufferHandle` field access, `DisplayInfo` defaults, constant value assertions
  - [ ] 7d: Verify: `just check` + `just test`

## Code Structure Decisions

- **`kernel/src/drivers/virtio_common.rs` (new module)**: Extracts shared VirtIO infrastructure (`VirtqDesc`, descriptor flags, virtqueue layout helpers, MMIO access, status constants, poll timeout) from `virtio_blk.rs`. Both block and GPU drivers import from this module. This avoids code duplication and is the correct refactoring before adding a second VirtIO driver.

- **`shared/src/gpu.rs` (new module)**: All VirtIO-GPU wire-format structs and AIOS-native GPU types. Rationale: separation from storage subsystem; GPU is a distinct domain. The `pub mod gpu;` goes in `shared/src/lib.rs`.

- **`VIRTIO_DEVICE_ID_GPU` in `shared/src/storage.rs`**: The VirtIO MMIO constants (magic, region base, stride, register offsets) are already centralized in storage.rs. Adding GPU device ID=16 alongside BLK device ID=2 keeps VirtIO transport constants together. GPU-specific types go in their own module.

- **2-descriptor chain for commands**: GPU commands differ from block's 3-descriptor chain. GPU uses: desc[0] = command header+params (device-readable), desc[1] = response buffer (device-writable). For `RESOURCE_ATTACH_BACKING`, we need a 3-descriptor variant: desc[0] = header, desc[1] = mem_entry array (both readable), desc[2] = response (writable). This is handled by `submit_command_with_extra()`.

- **Command buffer layout in DMA page**: The single DMA cmd page (4096B) is split: cmd data at offset 0, response at offset 2048. For `submit_command_with_extra()`, the extra data (mem_entry array) is also placed in the cmd page after the header. With contiguous framebuffer allocation, the extra data is just one 16-byte `VirtioGpuMemEntry`, fitting easily.

- **Contiguous framebuffer DMA allocation**: Instead of allocating many individual 4K pages (which would require hundreds of `VirtioGpuMemEntry` structs), allocate one contiguous DMA region using `alloc_dma_pages(order)`. **MAX_ORDER=10** in the buddy allocator means the maximum contiguous allocation is 4MB (1024 pages). A 1024×768×4 = 3MB framebuffer fits in order-10. However, a 1920×1080×4 = ~8MB framebuffer DOES NOT FIT. If QEMU reports a resolution >1024×1024, the driver must either: (a) cap resolution to fit order-10 (max ~1024×1024 at 4bpp), or (b) allocate multiple smaller contiguous blocks and use multiple `VirtioGpuMemEntry` entries (scatter-gather). **Decision: cap at 1024×1024 for M19, allow larger in future phases with scatter-gather support.**

- **Polled I/O only**: Follow VirtIO-blk pattern — no interrupts for Phase 6. Spin on used ring index. GPU operations are infrequent enough (frame updates, not per-sector I/O) that polling is acceptable.

- **`GpuBufferHandle` in shared crate**: Contains `resource_id`, dimensions, format, stride, `fb_phys`/`fb_virt`/`page_count`, `order` (for deallocation). Lives in shared crate because the GPU Service (M20) and Compute Kit (M22) reference it.

- **AIOS blue color**: #5B8CFF = RGB(91, 140, 255). In B8G8R8A8 (BGRA) byte order: bytes are [B=0xFF, G=0x8C, R=0x5B, A=0xFF]. As a u32 written to memory (little-endian): 0xFF5B8CFF. Verify byte order matches QEMU's expected format.

- **`run-gpu` recipe**: Uses `-serial stdio` (not `-nographic`) + `-device virtio-gpu-device` (no `-device ramfb`). This gives both UART output in terminal AND a display window showing GPU output.

- **EarlyBootPhase::GpuReady**: Deferred to M20 Step 11 (GOP→GPU transition). M19 does not add a boot phase — it only probes and displays a test frame during init.

## Dependencies & Risks

- **Depends on**: Phase 5 (Kit Foundation) complete, VirtIO MMIO infrastructure from Phase 4, DMA pool (Pool::Dma) from Phase 2 M7, `DeviceTree` parsing from Phase 1
- **Risk: QEMU VirtIO-GPU default resolution**: GET_DISPLAY_INFO returns QEMU's default scanout size. If QEMU returns 0x0 for an unconfigured scanout, need fallback. Mitigation: check `enabled` field in `VirtioGpuDisplayOne`, use 1024x768 as fallback.
- **Risk: DMA page physical addresses**: `RESOURCE_ATTACH_BACKING` requires guest physical addresses. Must pass raw physical addresses from frame allocator, NOT virtual (direct-map) addresses. The VirtIO-blk driver already does this correctly — follow same pattern.
- **Risk: VirtIO-GPU and VirtIO-blk both scanning same MMIO range**: Each scans for their own device ID, so they won't conflict. Both devices exist at different MMIO slot addresses.
- **Risk: Command buffer size**: The DMA page (4096 bytes) must fit the largest command. `VirtioGpuRespDisplayInfo` is ~408 bytes — fits comfortably. With contiguous framebuffer allocation, ATTACH_BACKING only needs 1 entry (16 bytes), so no overflow.
- **Risk: No GPU device present (`just run` without `-device virtio-gpu-device`)**: `probe()` must gracefully return `None` when no VirtIO device with ID=16 exists. The `init()` function returns `false`, and `kernel_main` skips GPU init. This is the same pattern as VirtIO-blk.
- **Risk: GPU init placement in boot sequence**: Must be after storage init (Step 7b) since both use VirtIO MMIO scanning and DMA allocation. Insert as Step 7b2 between storage init and benchmark init. Must also be before `sched::start()` (Step 7d) since the GPU test frame rendering happens on the boot CPU before secondary cores are released.
- **Risk: `VirtioGpuRespDisplayInfo` size (408 bytes)**: Response buffer in DMA page (offset 2048, 2048 bytes available) fits this easily. But must ensure the response buffer region doesn't overlap with the command region. Split the 4K page: cmd at 0, response at 2048.
- **Risk: QEMU display resolution default vs MAX_ORDER**: QEMU's VirtIO-GPU returns a default resolution via GET_DISPLAY_INFO. The buddy allocator MAX_ORDER=10 caps contiguous DMA allocation at 4MB (1024 pages). A 1024×768×4 = 3MB framebuffer fits (order-10). But 1280×800×4 = 4MB is borderline, and 1920×1080×4 = ~8MB EXCEEDS order-10. Mitigation: if reported resolution requires >4MB, clamp to the largest resolution that fits in order-10 (e.g., 1024×768). Log a warning about the clamp. Future phases can add scatter-gather ATTACH_BACKING for larger framebuffers.
- **Risk: QEMU virtio-gpu-device transport version**: QEMU may default `virtio-gpu-device` to version=2 (modern MMIO transport). The probe function rejects version != 1 (legacy). If this happens, the GPU probe silently fails. Mitigation: (a) add a `kwarn!` log when version != 1 is detected at a slot with device_id=16, so the failure is visible in UART; (b) use `-device virtio-gpu-device` in the justfile `run-gpu` recipe — if QEMU defaults to v2, try adding `disable-modern=on` or `disable-legacy=off` flags; (c) test empirically on the first `just run-gpu` attempt and document the working flag combination.
- **Risk: `order_for_pages` local copy in `virtio_blk.rs`**: This function is duplicated — the canonical version is in `shared::memory::order_for_pages`. Step 2 should update `virtio_blk.rs` to use the shared version.
- **Risk: GPU driver imports**: GPU driver needs `use shared::storage::*` for VirtIO transport types (`VirtqDesc`, MMIO register offsets, status constants) AND `use shared::gpu::*` for GPU-specific types. No naming conflicts exist between these two modules. The GPU driver also needs `use shared::order_for_pages` for DMA allocation math.
- **Risk: VirtIO-GPU config space layout**: `num_scanouts` at MMIO offset 0x108 is confirmed correct per VirtIO GPU spec: `events_read` (0x100), `events_clear` (0x104), `num_scanouts` (0x108), `num_capsets` (0x10C). QEMU `virtio-gpu-device` populates this correctly.
- **Not a gap (M20 awareness)**: M19 does not touch the `Capability` enum — that's M20 Step 7. No accidental dependency created.

## Phase Doc Reconciliation

- **New Step 2 added (VirtIO common extraction)**: Phase doc has 6 steps for M19 (Steps 1–6). This plan adds a Step 2 ("Extract VirtIO common infrastructure") before the GPU probe step, making 7 plan steps total. The phase doc should be updated to reflect this — the M19 step range becomes 1–7 and the summary table adjusted.
- **Step count shift**: Phase doc Steps 2–6 become plan Steps 3–7. Phase doc M20 steps (7–12) remain as-is since they're in a different milestone.
- Phase doc Step 5 says "Add `run-gpu` recipe to justfile: `-serial stdio`, `-device virtio-gpu-device`, no `-device ramfb`" — correct, but the recipe also needs ESP disk, UEFI firmware, and data disk flags. Will model after `run-display` recipe.
- Phase doc Step 1 lists `GpuBufferHandle` — this should also be in shared crate (already specified in Step 7 verification, but worth noting the initial placement).
- Phase doc mentions "config space: `num_scanouts` at offset 0x108" — this is the VirtIO config space register for GPU. Need to verify against QEMU source if this is correct for legacy MMIO v1 (config space starts at MMIO offset 0x100, so num_scanouts = config_space[0x8] = MMIO offset 0x108).

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
