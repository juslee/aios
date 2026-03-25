---
author: claude
date: 2026-03-25
tags: [gpu, ipc, capability, display, drivers]
status: in-progress
phase: 6
milestone: M20
---

# Plan: Phase 6 M20 — Custom GPU Service

## Context

Phase 6 M19 delivered a working VirtIO-GPU 2D driver (`kernel/src/drivers/virtio_gpu.rs`) that probes, initializes, and renders a solid-color test frame to QEMU display. M20 wraps this driver in a **capability-gated IPC service** following the echo service pattern from Phase 3, adds double-buffered page-flip, and implements the GOP→VirtIO-GPU transition.

**Current state:**
- VirtIO-GPU driver: fully functional with all 2D commands (create, attach, set_scanout, transfer, flush, present)
- `VIRTIO_GPU: Mutex<Option<VirtioGpu>>` global in `kernel/src/drivers/virtio_gpu.rs`
- Service manager: `service_register()`, `service_lookup()`, echo service pattern in `kernel/src/service/mod.rs`
- Capability system: 6 variants in `shared/src/cap.rs` (ChannelCreate, ChannelAccess, SharedMemoryCreate, SharedMemoryAccess, SpawnAgent, DebugPrint)
- IPC: `MAX_MESSAGE_SIZE = 256`, `RawMessage` with 256-byte inline data
- Boot phases: 19 variants (0-18), `GpuReady = 17`, `Complete = 18` is last (updated during M20)
- No `kernel/src/gpu/` module exists yet
- No `GpuCommand`/`GpuRequest`/`GpuResponse`/`FenceTracker` types exist yet in shared crate

**Key gaps:**
- GPU capability variants not yet in `Capability` enum
- No IPC protocol types for GPU Service
- No GPU Service kernel thread or IPC loop
- No double-buffering infrastructure
- No `GpuReady` boot phase
- No GOP→VirtIO-GPU transition logic

## Progress

### Step 7: GPU capability types
- [ ] 7a: Add 4 new variants to `Capability` enum in `shared/src/cap.rs`:
  - `GpuMmioAccess` — access GPU MMIO region
  - `GpuBufferCreate` — allocate GPU buffers
  - `GpuBufferAccess(u32)` — access specific buffer by resource ID
  - `DisplayControl` — configure display scanout
- [ ] 7b: Add `permits()` match arms for the 4 new variants (exact match for simple, ID match for `GpuBufferAccess`)
- [ ] 7c: Add `can_attenuate_to()` rules: `GpuBufferCreate` → `GpuBufferAccess(id)`
- [ ] 7d: Write host-side tests: permits, denies cross-variant, attenuation rules (6-8 tests)
- [ ] 7e: Verify: `just check` + `just test`

### Step 8: GPU Service IPC protocol types
- [ ] 8a: Add `GpuCommand` enum to `shared/src/gpu.rs`: `GetDisplayInfo = 1`, `AllocateBuffer = 2`, `ReleaseBuffer = 3`, `Present = 4`, `GetBufferInfo = 5`, `SwapBuffers = 6`
  - Note: Include `SwapBuffers` now (Step 10 needs it) to avoid modifying the enum later
- [ ] 8b: Define `GpuRequest` struct (repr(C), fits in 256 bytes):
  - `command: u32` (discriminant)
  - `resource_id: u32` (for ReleaseBuffer/Present/GetBufferInfo)
  - `width: u32`, `height: u32`, `format: u32` (for AllocateBuffer)
  - `damage_x: u32`, `damage_y: u32`, `damage_w: u32`, `damage_h: u32` (for Present)
  - Total: ~40 bytes << 256 limit
- [ ] 8c: Define `GpuResponse` struct (repr(C), fits in 256 bytes):
  - `status: i32` (0 = success, negative = GpuError variant)
  - `resource_id: u32` (for AllocateBuffer response)
  - `width: u32`, `height: u32`, `stride: u32`, `format: u32` (for GetBufferInfo/GetDisplayInfo)
  - `fb_virt: u64` (for GetBufferInfo — virtual address of framebuffer)
  - `scanout_id: u32`
  - Total: ~48 bytes << 256 limit
- [ ] 8d: Add `GpuError::to_status()` and `GpuError::from_status()` for i32 round-trip conversion
- [ ] 8e: Write host-side tests: struct size ≤ 256, GpuCommand discriminants, status round-trip
- [ ] 8f: Verify: `just check` + `just test`

### Step 9: GPU Service kernel thread
- [ ] 9a: Add public wrapper functions to `kernel/src/drivers/virtio_gpu.rs` (see "VirtIO-GPU driver methods are private" gap above): `gpu_allocate_framebuffer`, `gpu_set_scanout`, `gpu_transfer_to_host`, `gpu_resource_flush`, `gpu_present_frame`, `gpu_resource_detach_backing`, `gpu_resource_unref`, `gpu_release_test_frame`
- [ ] 9b: Add `pub unsafe fn free_dma_pages(phys_addr: usize, order: usize)` to `kernel/src/mm/frame.rs` (see gap above)
- [ ] 9c: Create `kernel/src/gpu/mod.rs` with `pub mod service;`
- [ ] 9d: Add `pub mod gpu;` to `kernel/src/main.rs`
- [ ] 9e: Create `kernel/src/gpu/service.rs` with:
  - `GpuServiceState` struct: holds allocated buffer handles (fixed array), front/back buffer indices, semantic hint, fence tracker
  - `gpu_service_entry()` — thread entry function (called from thread spawn)
  - `gpu_service_loop()` — creates IPC channel, registers as "gpu-service", loops on `ipc_recv()` → decode `GpuCommand` → dispatch → `ipc_reply()`
- [ ] 9f: Implement handlers (these call the public wrappers from 9a):
  - `handle_get_display_info()` → calls `virtio_gpu::display_info()`
  - `handle_allocate_buffer()` → calls `virtio_gpu::gpu_allocate_framebuffer()`, enforces `GpuBufferCreate` capability (Phase 6: kernel-only, so always permitted; capability check is structural for future EL0 extraction)
  - `handle_release_buffer()` → calls `gpu_resource_detach_backing()` + `gpu_resource_unref()` + `free_dma_pages()`
  - `handle_present()` → calls `gpu_transfer_to_host()` + `gpu_resource_flush()` with damage rect
  - `handle_get_buffer_info()` → returns DMA virt addr, dimensions, stride from tracked buffer handles
- [ ] 9g: Add GPU Service process + thread spawn in `kernel_main` after `virtio_gpu::init()` succeeds:
  - Create `ProcessId(9)` ("gpu-svc") in PROCESS_TABLE
  - Grant capabilities: `ChannelCreate`, `ChannelAccess(gpu_ch)`, `GpuMmioAccess`, `GpuBufferCreate`, `DisplayControl`
  - Grant `ChannelAccess(gpu_ch)` to `ProcessId(0)` (kernel process, for direct calls)
  - Create `ThreadId(0x900)` thread with `SchedulerClass::Interactive`, entry=`gpu_service_entry`
  - Pattern: same as echo service thread creation in `service::init()`
- [ ] 9h: Verify: `just check` + QEMU logs "Service 'gpu-service' registered (pid=9, ch=N)"

### Step 10: Double-buffered page-flip
- [ ] 10a: In GPU Service init (inside `gpu_service_loop()` before the IPC loop), allocate two framebuffers:
  - `front_buffer = allocate_framebuffer(width, height)`
  - `back_buffer = allocate_framebuffer(width, height)`
  - Set scanout to front_buffer initially
- [ ] 10b: Implement `FenceTracker` struct in `shared/src/gpu.rs`:
  - `next_id: u64`, `last_completed: u64`
  - `allocate() -> u64`, `complete(fence_id: u64)`, `is_complete(fence_id: u64) -> bool`
  - Host-side tests: allocate increments, complete updates last_completed, is_complete checks
- [ ] 10c: Implement `swap_buffers()` in GPU Service state:
  - Swap front/back handles
  - `set_scanout()` to bind new front
  - `transfer_to_host_2d()` + `resource_flush()` for new front
  - Log swap event
- [ ] 10d: Add `SwapBuffers` command handling in the IPC dispatch loop (enum variant already added in Step 8)
- [ ] 10e: Wire fenced commands: set `VIRTIO_GPU_FLAG_FENCE` on `resource_flush` commands, track fence ID in FenceTracker
- [ ] 10f: Verify: `just check` + QEMU logs "VirtIO-GPU: double buffering enabled"

### Step 11: GOP → VirtIO-GPU transition
- [ ] 11a: Add `GpuReady = 17` variant to `EarlyBootPhase` in `shared/src/boot.rs`:
  - Renumber `Complete` from 17 → 18
  - Update `EARLY_BOOT_PHASE_COUNT` from 18 → 19
  - Update `current_boot_phase()` transmute range in `kernel/src/boot_phase.rs` (val <= 17 → val <= 18)
  - Update all tests that assert count=18 or Complete=17
- [ ] 11b: Modify `kernel_main` GPU init section:
  - After `virtio_gpu::init()` succeeds: create GPU Service process/channel/thread (from Step 9g), then use **direct driver calls** (NOT IPC — scheduler hasn't started yet) to: release M19 test frame, allocate double buffers, fill back buffer with AIOS blue (#5B8CFF), set scanout, present
  - Advance boot phase to `GpuReady`
  - GPU Service IPC loop activates once scheduler starts (same as echo service pattern)
  - When GPU not present: skip GPU service, log "GOP framebuffer fallback"
- [ ] 11c: `framebuffer.rs` remains untouched — GOP fallback path preserved
- [ ] 11d: Verify: `just check` + `just run-gpu` shows AIOS blue via VirtIO-GPU + `just run` still boots normally

### Step 12: Shared crate refactoring and docs update for M20
- [ ] 12a: Verify all GPU types with no hardware deps are in `shared/src/gpu.rs` (GpuCommand, GpuRequest, GpuResponse, FenceTracker)
- [ ] 12b: Write additional host-side tests for FenceTracker, GpuCommand dispatch, GpuRequest/GpuResponse size assertions
- [ ] 12c: Dead code cleanup: remove `#[allow(dead_code)]` from `resource_detach_backing()` and `resource_unref()` in `virtio_gpu.rs` (now used by GPU Service)
- [ ] 12d: Update `CLAUDE.md`: Workspace Layout (new `kernel/src/gpu/` module), Key Technical Facts (GpuCommand enum, FenceTracker, GPU Service channel, EarlyBootPhase count 19)
- [ ] 12e: Update `README.md`: Project Structure
- [ ] 12f: Update phase doc: check off M20 steps, update Status
- [ ] 12g: Run `/audit-loop` — triple audit until 0 issues
- [ ] 12h: Verify: `just check` + `just test` + `just run` + `just run-gpu`

## Code Structure Decisions

- **GPU Service location**: `kernel/src/gpu/service.rs` — higher-level than the raw VirtIO driver in `drivers/`, distinct subsystem module. The `kernel/src/gpu/mod.rs` will later house `text.rs` (M21) and Kit implementation (M22).

- **IPC protocol as flat repr(C) structs**: `GpuRequest` and `GpuResponse` are flat C structs with a `command: u32` discriminant, not Rust enums. This ensures they fit cleanly in `RawMessage.data[256]` with `core::ptr::copy_nonoverlapping` serialization. No serde needed.

- **FenceTracker in shared crate**: Pure data structure with no hardware deps — belongs in `shared/src/gpu.rs` for host-side testing. The kernel's GPU Service holds a `FenceTracker` instance.

- **Capability check is structural**: In Phase 6, the GPU Service runs in kernel space (EL1) and all callers are kernel threads. The `GpuBufferCreate` capability check exists in the code path but is effectively always-permitted. This is intentional — it's structural scaffolding for when the GPU Service is extracted to EL0 in a future phase.

- **SwapBuffers included in GpuCommand from Step 8**: Adding all 6 command variants upfront avoids modifying the enum twice. The handler is wired in Step 10.

- **EarlyBootPhase::GpuReady insertion**: Insert at value 17, bump `Complete` to 18. This affects: `EARLY_BOOT_PHASE_COUNT`, `current_boot_phase()` transmute range, and 6+ tests in `shared/src/boot.rs`. The tests enumerate all variants — must add `GpuReady` to the array.

- **GPU Service thread creation pattern**: Follow the echo service pattern from `service::init()` — allocate a Thread with a specific ThreadId, set entry point to `gpu_service_entry`, add to run queue. Use `SchedulerClass::Interactive` for responsive command processing.

- **Double-buffer init in service loop**: The GPU Service allocates both framebuffers at the start of `gpu_service_loop()`, before entering the IPC receive loop. This simplifies the lifecycle — buffers exist for the entire service lifetime.

## Dependencies & Risks

- **Depends on**: M19 complete (VirtIO-GPU driver functional), Phase 3 IPC infrastructure (channels, service manager), Phase 3 capability system

### Thread ID allocation (verified — important subtlety)

The `ThreadId` passed to `Thread::new_kernel()` is a **label** stored in `sched.thread_id` for debugging. The actual identity used by the scheduler and IPC is the **THREAD_TABLE index** returned by `allocate_thread()`. The echo service creates `Thread::new_kernel(ThreadId(0x700), ...)` but the thread is stored at whatever free index `allocate_thread` finds (e.g., index 12). The enqueue uses `ThreadId(idx as u32)`, and `CURRENT_THREAD[cpu]` stores that index. `process_of_thread(tid)` uses `tid.0 as usize` to index THREAD_TABLE — so the index IS the runtime ThreadId.

**GPU Service**: Use `ThreadId(0x900)` as the debug label in `new_kernel`, but the actual runtime ThreadId = the `allocate_thread` return value. The channel's `owner_a` is set to `ThreadId(0x900)` (debug label) but this doesn't matter for IPC — `ipc_recv` checks capabilities, not ownership.

Used debug labels: `0x100+i` (idle/test), `0x200-0x201` (IPC tests), `0x300-0x301` (PI tests), `0x400` (more IPC tests), `0x700-0x701` (echo service), `0x800` (space storage), `0xB00-0xB02` (benchmarks). GPU Service: `0x900`.

### Process ID allocation (verified)
Used ProcessIds: 0 (kernel), 1-6 (IPC test processes), 7 (echo-svc), 8 (space-storage). **GPU Service: use `ProcessId(9)` ("gpu-svc")** — follows the pattern.

### Lock ordering (verified)
Current documented order: `PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE > CHANNEL_TABLE > SELECT_WAITERS > BLOCK_ENGINE > {VIRTIO_BLK, VIRTIO_GPU}`
The GPU Service IPC loop calls `ipc_recv()` (takes CHANNEL_TABLE) and then accesses `VIRTIO_GPU` (takes VIRTIO_GPU lock). This is **safe** — CHANNEL_TABLE > VIRTIO_GPU is already the documented order. The GPU Service must **release** the CHANNEL_TABLE lock (which `ipc_recv` does internally) before taking VIRTIO_GPU. Since `ipc_recv` returns the message (not holding the lock), this is naturally correct.

### DMA memory budget (verified)
Two framebuffers at 1280×800×4 = 4,096,000 bytes each. Each needs order-10 (4MiB) allocation from `Pool::Dma` (64MB). Total 8MB out of 64MB = 12.5%. Well within budget. Plus the existing test frame from M19 (~4MB). Total: ~12MB of 64MB DMA pool.

### Boot phase renumbering (high impact)
`current_boot_phase()` in `kernel/src/boot_phase.rs` uses `unsafe { transmute::<u32, EarlyBootPhase>(val) }` with guard `val <= 17`. Must change to `val <= 18`. Tests in `shared/src/boot.rs` that need updating:
- `early_boot_phase_count`: 18 → 19
- `early_boot_phase_contiguous_values`: add GpuReady to array
- `EarlyBootPhase::Complete as u32`: 17 → 18
- All variant discriminant assertions

### Echo service lifecycle (gap found)
The echo client calls `process_exit(ProcessId(7), 0)` which **kills the echo service** mid-boot. After that, service_lookup("echo") returns None. The GPU Service must be spawned **after** echo service init (`service::init()`) but its IPC channel must not collide. Since we create a separate process (ProcessId 9) and separate channel, this is safe. But note: the GPU Service should **not** exit — it runs for the kernel's lifetime.

### GPU Service vs direct driver calls (design gap)
Step 11 says "render AIOS test pattern to VirtIO-GPU back buffer via the GPU Service". But at the point in kernel_main where we need to render, the scheduler hasn't started yet (threads don't run until `sched::start()` + `enter_scheduler()`). The GPU Service thread won't execute until the scheduler starts.

**Resolution**: For Step 11's kernel_main transition, use **direct driver calls** (not IPC) to fill the back buffer and present. The GPU Service IPC path is available once the scheduler is running (Phase 7+ compositor will use it). The GPU Service thread still gets spawned and registered during boot, but its IPC loop only processes commands after the scheduler starts. This matches the echo service pattern — echo_server_entry runs `ipc_recv()` which blocks until the scheduler delivers a message.

### IPC channel creation pattern (verified — critical detail)

The IPC model uses capability-based access, not peer-based. `ipc_call()` checks `ChannelAccess(ch)` capability on the caller's process, NOT peer ownership. `ipc_recv()` similarly checks `ChannelAccess(ch)` on the receiver's process. `owner_b` (set via `channel_set_peer`) is used for cleanup/death notification only, not for access control.

**GPU Service channel setup** (differs from echo service):
1. `channel_create_unchecked(gpu_server_tid)` — creates channel, GPU server owns endpoint A
2. **Skip** `channel_set_peer()` — GPU Service accepts calls from any capable process, not a fixed peer
3. `service_register(b"gpu-service", ProcessId(9), ch)` — register in service manager
4. `grant_to_process(ProcessId(9), Capability::ChannelAccess(ch))` — GPU Service needs access
5. `grant_to_process(ProcessId(0), Capability::ChannelAccess(ch))` — **kernel process (pid=0) needs access** to call GPU Service from kernel_main and other kernel threads

**Process 0 ("kernel")** is created by `ipc::tests::init()` in `kernel/src/ipc/tests.rs` and already has `ChannelCreate`, `DebugPrint`, `SpawnAgent`, `SharedMemoryCreate` capabilities. We just need to add `ChannelAccess(gpu_ch)` for the GPU Service channel.

### No `free_dma_pages` public API (gap — affects handle_release_buffer)

`frame.rs` has `pub alloc_dma_pages(order)` but the matching `free_pages` is `pub unsafe fn free_page(phys_addr)` for single pages and `pub unsafe fn free_pages(&mut self, phys_addr, order)` on `FrameAllocator` (behind `FRAME_ALLOC` lock). There's no public `free_dma_pages(phys_addr, order)` wrapper.

**Resolution**: Add `pub unsafe fn free_dma_pages(phys_addr: usize, order: usize)` to `frame.rs` as a convenience wrapper (matches `alloc_dma_pages`). The GPU Service's `handle_release_buffer` needs this to free the DMA pages when a buffer is released.

### M19 test frame retained in `VirtioGpu.test_frame` (gap — affects double-buffer init)

The M19 `display_test_frame()` allocates a framebuffer and stores the handle in `gpu.test_frame = Some(handle)` to prevent DMA leak. This uses resource_id=1 and ~4MB DMA.

**Resolution for M20**: When the GPU Service initializes double-buffering, it should:
1. Detach and unref the M19 test frame (resource_id=1) to free its DMA pages
2. Allocate two new framebuffers for double-buffering (resource_ids=2,3)
3. Set `gpu.test_frame = None`

Alternatively, skip `display_test_frame()` entirely when the GPU Service will be started — allocate directly into double buffers. Since `display_test_frame()` is called before the GPU Service spawn in `kernel_main`, the simplest approach is:
- **Option A**: Keep M19 test frame call, GPU Service cleans it up at init
- **Option B**: Conditionally skip test frame when GPU Service will follow

**Recommendation**: Option A — keep the test frame for "GPU works" confidence, then GPU Service replaces it. Simpler code path, and test_frame only wastes ~4MB temporarily.

### VirtIO-GPU driver methods are private (gap — critical for GPU Service)

All `VirtioGpu` struct methods (`allocate_framebuffer`, `set_scanout`, `transfer_to_host_2d`, `resource_flush`, `present_frame`, `resource_detach_backing`, `resource_unref`, `get_display_info`) are `fn` (private), not `pub fn`. Only the module-level functions `init()`, `display_info()`, and `display_test_frame()` are public.

**Resolution**: Add public module-level wrapper functions in `virtio_gpu.rs` that lock `VIRTIO_GPU` and delegate:

- `pub fn gpu_allocate_framebuffer(w, h) -> Result<GpuBufferHandle, GpuError>`
- `pub fn gpu_set_scanout(scanout_id, resource_id, rect) -> Result<(), GpuError>`
- `pub fn gpu_transfer_to_host(resource_id, rect, offset) -> Result<(), GpuError>`
- `pub fn gpu_resource_flush(resource_id, rect) -> Result<(), GpuError>`
- `pub fn gpu_present_frame(handle) -> Result<(), GpuError>`
- `pub fn gpu_resource_detach_backing(resource_id) -> Result<(), GpuError>`
- `pub fn gpu_resource_unref(resource_id) -> Result<(), GpuError>`
- `pub fn gpu_release_test_frame()` — detach + unref + free DMA for the M19 test frame

This follows the pattern of `display_test_frame()` which already locks `VIRTIO_GPU` and calls private methods. The struct methods stay private — module-level functions are the public API.

### Fence support requires `check_response` enhancement (gap)

`check_response()` only checks `type_` field (offset 0-3) of the response header. For fenced commands, Step 10 needs to also read `fence_id` (offset 8-15) from the response and call `FenceTracker::complete()`. The response header is the same `VirtioGpuCtrlHdr` struct (24 bytes).

**Resolution**: Either:
- Modify `check_response` to optionally return `fence_id` (breaking change to all callers)
- Add a `check_response_fenced` variant that returns `((), Option<u64>)` with the fence_id
- Have `submit_command` take a `FenceTracker` reference and auto-complete fences

**Recommendation**: Add `submit_command_fenced(&mut self, cmd, resp, fence_tracker)` that sets `VIRTIO_GPU_FLAG_FENCE` in the command, assigns `fence_id` from tracker, and completes the fence on response. Keeps unfenced path unchanged.

### `channel_create_unchecked` is in `ipc/tests.rs` (architectural note)

The `channel_create_unchecked` function lives in `kernel/src/ipc/tests.rs` and is re-exported as `pub(crate)`. This is used for init-time channel creation where threads don't exist yet (so `channel_create` would fail looking up `owner_pid`). The GPU Service follows the same pattern — channel created before its thread exists.

### `GpuBufferHandle.order` field exists (verified — needed for freeing)

`GpuBufferHandle` in `shared/src/gpu.rs` already has an `order: usize` field (the buddy allocator order used for allocation). This is needed by `free_dma_pages(phys_addr, order)` to return pages to the correct buddy list. No gap here — the field was added in M19.

### GPU Service needs a buffer tracking table (design detail)

The GPU Service must track allocated buffers for `handle_get_buffer_info` (lookup by resource_id) and `handle_release_buffer` (find + free). A fixed `[Option<GpuBufferHandle>; MAX_GPU_BUFFERS]` array (say `MAX_GPU_BUFFERS = 8`) indexed by a scan for matching `resource_id`. This is the `GpuServiceState` struct mentioned in Step 9e.

### Thread entry DAIFClr (pattern to follow)

All kernel service thread entries (`echo_server_entry`, `echo_client_entry`) start with `unsafe { core::arch::asm!("msr DAIFClr, #0x2") }` to unmask IRQs. The GPU Service thread entry `gpu_service_entry` must do the same — otherwise timer interrupts (needed for IPC timeout) won't fire on that thread's CPU.

### Channel ID allocation is dynamic (verified — no collision risk)

Channel IDs are auto-allocated by finding the first `None` slot in `CHANNEL_TABLE[128]`. Space-storage service took the next free slot at boot. GPU Service will similarly get the next free slot. No hardcoded channel IDs — no collision risk.

### `run-gpu` recipe has no `-device ramfb` (interaction with GOP path)

The `just run-gpu` recipe uses `-device virtio-gpu-device` but does **not** include `-device ramfb`. QEMU may or may not expose VirtIO-GPU as a UEFI GOP device. Two scenarios:

1. **QEMU provides GOP via VirtIO-GPU**: Both GOP framebuffer and VirtIO-GPU work. Step 8 in kernel_main renders test pattern to GOP, then Step 7b2 initializes VirtIO-GPU. Both display the same content to the same physical display. Step 11's transition replaces GOP content with VirtIO-GPU content.

2. **QEMU does not provide GOP without ramfb**: `Framebuffer::from_boot_info` returns `None` → "No framebuffer available" → GOP path skipped entirely. VirtIO-GPU is the only display path.

**Resolution**: Both scenarios are handled correctly by the existing code. In scenario 1, the VirtIO-GPU display eventually replaces the GOP content (VirtIO-GPU takes priority over GOP/ramfb in QEMU's display). In scenario 2, VirtIO-GPU is the sole display. The `just run` recipe (with ramfb, without VirtIO-GPU) continues to use GOP. **No gap here** — just documenting the behavior for implementation awareness.

### `ipc_reply` has no capability check (verified — intentional)

Per `ipc.md §9.1`, `ipc_reply` does not require capability enforcement. The comment in code says "No capability check required". This means the GPU Service can reply to any caller without needing the caller's process to have a specific capability. Only `ipc_call` and `ipc_recv` check `ChannelAccess`. This is correct for the GPU Service pattern.

### GPU Service serialization: `core::ptr::copy_nonoverlapping` for flat repr(C) structs

`GpuRequest` and `GpuResponse` are flat `repr(C)` structs. To serialize into `RawMessage.data[256]`:

```rust
// Serialize: copy struct bytes into message data
let req = GpuRequest { ... };
let bytes = core::slice::from_raw_parts(&req as *const _ as *const u8, core::mem::size_of::<GpuRequest>());
msg.data[..bytes.len()].copy_from_slice(bytes);
msg.len = bytes.len();

// Deserialize: copy message data into struct
let mut req = GpuRequest::zeroed();
core::ptr::copy_nonoverlapping(msg.data.as_ptr(), &mut req as *mut _ as *mut u8, core::mem::size_of::<GpuRequest>());
```

Both are `unsafe` operations that require: (a) struct is `repr(C)`, (b) no padding bytes with uninitialized values, (c) size fits in 256 bytes. Host-side tests will assert `size_of::<GpuRequest>() <= 256` and `size_of::<GpuResponse>() <= 256`.

**Gap**: Need `GpuRequest` and `GpuResponse` to have a `zeroed()` constructor (or use `core::mem::zeroed()`). Since they're `repr(C)` with only integer fields, `zeroed()` is safe.

## Phase Doc Reconciliation

- Step 9: Phase doc marks handlers (9d-9h) as `[x]` done, but no `kernel/src/gpu/` module exists in the codebase. These checkboxes appear to be design-complete markers, not implementation-complete. The plan treats ALL Step 9 sub-tasks as pending implementation.
- Step 10: Phase doc marks `swap_buffers()` and `FenceTracker` as `[x]`. Same situation — design is settled but code doesn't exist. Plan treats as pending.
- Step 8: Phase doc doesn't include `SwapBuffers` in `GpuCommand` — it's added in Step 10. Plan moves it to Step 8 to avoid double-modification.

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
