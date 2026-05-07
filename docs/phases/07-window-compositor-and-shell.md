# Phase 7: Window Compositor & Shell

**Tier:** 2 — Core System Services
**Duration:** 7 weeks
**Deliverable:** Window compositor with IPC-based surface lifecycle, software composition with damage tracking, floating window management with input routing, desktop shell (Status Strip, Taskbar, Workspace), Input Kit Tier 1
**Status:** In Progress (M23, M24, M25 complete; M26–M27 pending)
**Prerequisites:** Phase 6 (GPU & Display)
**Unlocks:** Phase 8 (Input & Terminal), Phase 16 (Flow & Translation), Phase 25 (Performance & Optimization)

-----

## Objective

Phase 7 brings interactive graphical display to AIOS by building the window compositor and desktop shell — the Layer 1 (Classic Desktop) foundation. This is the first phase where the user can see and interact with multiple windows, move them around, and switch focus with keyboard shortcuts.

The compositor is a **system service** (not a Kit) following the [ADR: Compositor as System Service](../knowledge/decisions/2026-03-22-jl-compositor-system-service.md). Apps interact with display through Compute Kit Tier 1 (`GpuSurface`), input through Input Kit, and data transfer through Flow Kit — never compositor APIs directly. The compositor consumes these Kit primitives internally to compose surfaces, route input, and manage layout.

Phase 7 implements Layer 1 only ([ADR: Three Interaction Layers](../knowledge/decisions/2026-03-16-jl-three-interaction-layers.md)): traditional windows, taskbar, manual floating layout, no intelligence. If AIRS fails, crashes, or hasn't loaded yet, the user has a fully functional desktop. Layer 2 (Smart Desktop, context-aware layout) and Layer 3 (Intelligence Surface, generative UI) are future phases.

By the end of this phase: (1) VirtIO-input devices (keyboard + tablet) probed and delivering events; (2) compositor service running with IPC-based surface lifecycle; (3) software composition with flat z-order alpha blending and damage tracking; (4) floating window layout with title bars, move/resize, and Alt+Tab switching; (5) desktop shell with Status Strip, Taskbar, and static Workspace; (6) Input Kit Tier 1 traits extracted; (7) Gate 2 benchmarks pass.

-----

## Architecture References

These existing documents define the technical design. This phase doc focuses on implementation order and acceptance criteria — not duplicating the architecture.

| Topic | Document | Relevant Sections |
|---|---|---|
| Compositor overview | [compositor.md](../platform/compositor.md) | §1 Core Insight, §2 Architecture, §15 Design Principles, §16 Implementation Order |
| Compositor protocol | [compositor/protocol.md](../platform/compositor/protocol.md) | §3.1 Surface Lifecycle, §3.2 Shared Buffer Protocol, §3.3 Buffer Synchronization, §3.4 Damage Reporting |
| Semantic hints | [compositor/protocol.md](../platform/compositor/protocol.md) | §4.1 Content Types, §4.2 Interaction State (stored but not acted on in Phase 7) |
| Render pipeline | [compositor/rendering.md](../platform/compositor/rendering.md) | §5.1 Scene Graph (simplified to flat z-order), §5.2 Frame Composition, §5.4 Frame Scheduling, §5.5 Animation System (stubs) |
| Layout engine | [compositor/rendering.md](../platform/compositor/rendering.md) | §6.1 Layout Modes (floating only in Phase 7) |
| Input routing | [compositor/input.md](../platform/compositor/input.md) | §7.1 Input Pipeline, §7.2 Focus Management, §7.3 Global Hotkeys |
| Compositor security | [compositor/security.md](../platform/compositor/security.md) | §10.1 Capability-Gated Surfaces |
| Experience layer | [experience.md](../experience/experience.md) | §1 Core Insight, §1.1 Three Interaction Layers, §2 Five Surfaces, §3 Workspace, §6 Status Strip, §21 Implementation Order |
| Input devices | [input/devices.md](../platform/input/devices.md) | §3.4 VirtIO-Input Driver (probe, config space, event format) |
| Input events | [input/events.md](../platform/input/events.md) | §4.1 InputEvent Hierarchy (RawInputEvent + typed events) |
| Input Kit | [kits/platform/input.md](../kits/platform/input.md) | §2 Core Traits (InputDevice, KeyEvent, MotionEvent) |
| GPU Service pattern | kernel/src/gpu/service.rs | Existing IPC service pattern to follow |
| VirtIO driver pattern | kernel/src/drivers/virtio_gpu.rs | Existing MMIO transport pattern to follow |
| Custom Core principle | [ADR: Custom Core](../knowledge/decisions/2026-03-16-jl-custom-core-principle.md) | Build AIOS-native compositor, wgpu bridge later |
| Three Interaction Layers | [ADR: Three Layers](../knowledge/decisions/2026-03-16-jl-three-interaction-layers.md) | Phase 7 = Layer 1 only |
| Kit architecture | [ADR: Kit Architecture](../knowledge/decisions/2026-03-22-jl-kit-architecture.md) | Compositor is system service, not a Kit |
| Deadlock prevention | [deadlock-prevention.md](../kernel/deadlock-prevention.md) | §3 Lock Ordering (must extend for compositor globals) |

-----

## Milestones

Milestones are numbered continuously across all phases. Phase 6 used M19–M22; Phase 7 continues with M23–M27.

| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| **M23 — VirtIO-Input Driver** | 1–7 | End of week 2 | VirtIO-input keyboard + tablet probed and initialized on QEMU; input events logged to UART; `just run-input` works |
| **M24 — Compositor Core** | 8–16 | End of week 4 | Compositor service running; IPC-based surface lifecycle; software composition with flat z-order and damage tracking; multi-surface display on QEMU; display handoff from GPU Service |
| **M25 — Window Manager & Input Routing** | 17–23 | End of week 5 | Floating window layout with decorations; pointer hit-testing with software cursor; keyboard/pointer focus; input routing pipeline; move/resize; Alt+Tab; shared crate tests |
| **M26 — Desktop Shell** | 24–30 | End of week 6 | Status Strip (time, CPU%, memory); Taskbar (surface list, focus indicator); Workspace (static home view); test application validating full IPC stack; shell rendering optimization |
| **M27 — Input Kit, Integration & Gate** | 31–36 | End of week 7 | Input Kit Tier 1 traits extracted; animation stubs; Gate 2 benchmarks pass; `just run-compositor` target; documentation updated; all quality gates pass |

-----

## Milestone 23 — VirtIO-Input Driver (End of Week 2)

*Goal: Probe and drive VirtIO-input devices on QEMU (keyboard + tablet). Produce typed InputEvent values from raw evdev events. Establish the input subsystem module with a polled event pipeline. This must come before the compositor because interactive testing requires keyboard/mouse.*

### Step 1: VirtIO-input shared types and constants

**What:** Define VirtIO-input wire-format types and evdev constants in the shared crate. These are the on-wire types from VirtIO spec §5.8 and Linux evdev — distinct from the higher-level typed InputEvent used by the compositor.

**Tasks:**
- [x] Create `shared/src/input.rs` with `pub mod input;` in `shared/src/lib.rs`
- [x] Define `VirtioInputEvent` — 8-byte `repr(C)` wire format: `event_type: u16`, `code: u16`, `value: u32`
- [x] Define evdev event type constants: `EV_SYN` (0x00), `EV_KEY` (0x01), `EV_REL` (0x02), `EV_ABS` (0x03), `SYN_REPORT` (0)
- [x] Define evdev key code constants: `KEY_A` (30) through `KEY_Z`, `KEY_ENTER` (28), `KEY_ESC` (1), `KEY_BACKSPACE` (14), `KEY_TAB` (15), `KEY_SPACE` (57), `KEY_LEFTSHIFT` (42), `KEY_LEFTCTRL` (29), `KEY_LEFTALT` (56), `KEY_LEFTMETA` (125), `KEY_F1`–`KEY_F12`
- [x] Define evdev button constants: `BTN_LEFT` (0x110), `BTN_RIGHT` (0x111), `BTN_MIDDLE` (0x112)
- [x] Define evdev absolute axis constants: `ABS_X` (0x00), `ABS_Y` (0x01)
- [x] Define VirtIO-input config select constants: `VIRTIO_INPUT_CFG_UNSET` (0x00), `VIRTIO_INPUT_CFG_ID_NAME` (0x01), `VIRTIO_INPUT_CFG_ID_SERIAL` (0x02), `VIRTIO_INPUT_CFG_ID_DEVIDS` (0x03), `VIRTIO_INPUT_CFG_PROP_BITS` (0x10), `VIRTIO_INPUT_CFG_EV_BITS` (0x11), `VIRTIO_INPUT_CFG_ABS_INFO` (0x12)
- [x] Define `VirtioInputAbsInfo` — `repr(C)`: `min: u32`, `max: u32`, `fuzz: u32`, `flat: u32`, `res: u32`
- [x] Define `InputDeviceId(u8)` for identifying input devices
- [x] Add `Input = 14` variant to `Subsystem` enum in `shared/src/observability.rs`, update `COUNT` to 15, update `name()` match arm and all unit tests
- [x] Write host-side tests: `VirtioInputEvent` is exactly 8 bytes `repr(C)`, key code constants compile, Subsystem tests pass

**Key reference:** [input/devices.md](../platform/input/devices.md) §3.4, VirtIO spec §5.8

**Acceptance:** `just check` zero warnings. `just test` passes with new input type tests. Existing 442+ tests still pass.

-----

### Step 2: VirtIO-input MMIO driver — probe and init

**What:** Create `virtio_input.rs` following the `virtio_gpu.rs` pattern. Probe VirtIO MMIO slots for `device_id=18` (input). Support finding MULTIPLE devices (keyboard + tablet are separate VirtIO devices on different MMIO slots). Initialize each device: negotiate features, set up `eventq` (pre-fill with `VirtioInputEvent` buffers in available ring), set DRIVER_OK. Read device config via the select/subsel config space protocol (unique to VirtIO-input — not simple register reads like blk/gpu).

**Note:** VirtIO-input config space uses a select/subsel/read pattern (VirtIO spec §5.8.2). Write `select` and `subsel` fields to config space, then read `size` and `u.string[]` or `u.bitmap[]`. This is needed to read device name and absolute axis info (min/max range for tablet coordinates).

**Tasks:**
- [x] Create `kernel/src/drivers/virtio_input.rs`, add `pub mod virtio_input;` to `kernel/src/drivers/mod.rs`
- [x] Define `VirtioInputDevice` struct: `base: usize` (MMIO virt addr), `eventq` state (vq_phys, desc/avail/used offsets, queue_size), `device_id: InputDeviceId`, `name: [u8; 64]`
- [x] Define `MAX_INPUT_DEVICES: usize = 4` and `static INPUT_DEVICES: Mutex<[Option<VirtioInputDevice>; MAX_INPUT_DEVICES]>`
- [x] Implement `probe_all()` — scan DTB bases first, then brute-force MMIO slots; find ALL devices with `device_id=18`, not just the first
- [x] Implement `init_device(base)` — reset, acknowledge, negotiate features, read name via config select, set up eventq (queue size from QUEUE_NUM_MAX, pre-fill with empty event buffers), allocate statusq but don't use it (Phase 8 LED control), set DRIVER_OK
- [x] Implement `read_config_name(base)` — write `select=VIRTIO_INPUT_CFG_ID_NAME, subsel=0`, read `size` and `u.string[0..size]`
- [x] Implement `read_abs_info(base, axis)` — write `select=VIRTIO_INPUT_CFG_ABS_INFO, subsel=axis`, read `VirtioInputAbsInfo` for tablet coordinate range
- [x] Log device names and capabilities to UART

**Key reference:** [input/devices.md](../platform/input/devices.md) §3.4 (driver implementation steps 1–7), VirtIO spec §5.8.2

**Acceptance:** With `-device virtio-keyboard-device -device virtio-tablet-device`, both devices probed; UART shows `[Input] VirtIO-input: "QEMU Virtio Keyboard" at 0x...` and `"QEMU Virtio Tablet"` with abs info `min=0 max=32767`

-----

### Step 3: VirtIO-input event polling

**What:** Implement polled I/O event reading from the eventq virtqueue. Check used ring for completed buffers containing `VirtioInputEvent` structs. Recycle consumed buffers back to the available ring. Handle `EV_SYN/SYN_REPORT` grouping (batch events between SYN_REPORT into atomic groups). All existing VirtIO drivers use polled I/O — this matches the established pattern.

**Tasks:**
- [x] Implement `poll_events(device_id) -> Option<VirtioInputEvent>` — check used ring, extract event, recycle buffer
- [x] Implement `poll_all_devices()` — iterate all initialized devices, collect events
- [x] Handle `SYN_REPORT` as event group boundary
- [x] Add virtqueue notify after recycling buffers to available ring

**Key reference:** [input/devices.md](../platform/input/devices.md) §3.4 (steps 8–10), VirtIO spec §2.7 (used ring processing)

**Acceptance:** Keyboard presses produce `VirtioInputEvent` with `event_type=EV_KEY` logged to UART; tablet movement produces `event_type=EV_ABS` events

-----

### Step 4: Input event translation and keymap

**What:** Create the input subsystem module. Translate raw `VirtioInputEvent` (evdev format) into typed `InputEvent` enum. Track modifier state (shift, ctrl, alt, super). Convert absolute tablet coordinates (0–32767) to display coordinates (0–width, 0–height). Include a basic US-QWERTY keymap for scancode-to-character mapping.

**Tasks:**
- [x] Create `kernel/src/input/mod.rs` with input subsystem
- [x] Define typed `InputEvent` enum in `shared/src/input.rs`: `Keyboard { key: KeyCode, state: KeyState, modifiers: Modifiers }`, `Pointer { x: u32, y: u32, button: Option<MouseButton>, state: Option<ButtonState> }`
- [x] Define `KeyCode` enum (A–Z, 0–9, Enter, Esc, Tab, Space, Backspace, F1–F12, arrows, modifiers)
- [x] Define `KeyState` enum: `Pressed`, `Released`, `Repeat`
- [x] Define `Modifiers` bitflags: `SHIFT`, `CTRL`, `ALT`, `SUPER`
- [x] Define `MouseButton` enum: `Left`, `Right`, `Middle`
- [x] Implement evdev keycode → `KeyCode` translation table
- [x] Implement US-QWERTY keymap: `const KEYMAP_US: [Option<(char, char)>; 128]` mapping keycode → (unshifted, shifted) ASCII characters
- [x] Implement modifier tracking: set/clear bits on modifier key press/release
- [x] Implement absolute → display coordinate conversion: `x_display = abs_x * display_width / 32768`
- [x] Define global input event queue: `static INPUT_QUEUE: Mutex<FixedQueue<InputEvent, 256>>`
- [x] Implement `process_raw_event(device_id, raw_event)` → push typed `InputEvent` to queue

**Key reference:** [compositor/input.md](../platform/compositor/input.md) §7.1 (InputEvent enum), [input/events.md](../platform/input/events.md) §4.1

**Acceptance:** Pressing 'a' produces `InputEvent::Keyboard { key: KeyA, state: Pressed, modifiers: empty }`; Shift+A produces `modifiers: SHIFT`; moving tablet produces `InputEvent::Pointer { x, y }` in display coordinates

-----

### Step 5: QEMU run target and input demo

**What:** Add `just run-input` recipe that includes VirtIO-input devices. Create a simple input demo thread that reads from the input queue and logs events to UART. Also update `run-gpu` to include input devices for future use.

**Tasks:**
- [x] Add `run-input` recipe to justfile: same as `run-gpu` plus `-device virtio-keyboard-device -device virtio-tablet-device`
- [x] Update `run-gpu` recipe to also include `-device virtio-keyboard-device -device virtio-tablet-device`
- [x] Add `input::init()` call to `kernel/src/main.rs` after GPU init
- [x] Implement input polling in the timer tick or a dedicated polling loop (poll all devices every ~16ms)
- [x] Create input demo: log key name and pointer coordinates to UART on each event

**Note:** QEMU GUI interaction: click into the QEMU window to capture keyboard/mouse input. `Ctrl+Alt+G` releases the mouse grab. UART output continues to the terminal via `-serial stdio` (the GPU/input recipes use `-serial stdio` with a graphical window, unlike `just run` which uses `-nographic`).

**Key reference:** Existing justfile patterns

**Acceptance:** `just run-input` boots; keyboard presses show `[Input] Key: A Pressed` in UART; tablet movement shows `[Input] Pointer: x=640 y=400`

-----

### Step 6: EarlyBootPhase update

**What:** Add input-related boot phase tracking. Extend `EarlyBootPhase` enum with new variants for input and compositor readiness.

**Tasks:**
- [x] Add `InputReady = 18` and `CompositorReady = 19` variants to `EarlyBootPhase` in `shared/src/boot.rs`
- [x] Update `Complete` variant to `= 20`
- [x] Ensure `EarlyBootPhase` logging works as expected (it derives `Debug`; there is no `name()` method to update)
- [x] Update `EARLY_BOOT_PHASE_COUNT` to 21, the "Total: N variants" comment in `shared/src/boot.rs`, and all unit tests that assert count or discriminant values
- [x] Call `advance_boot_phase(InputReady)` after input init in `kernel/src/main.rs`

**Note (post-implementation):** The pre-M23 enum had 18 variants (EntryPoint=0 through 17), so the new variants slotted in as 18/19/20 with COUNT=21, not 19/20/21 with COUNT=22 as initially drafted. Numbering above reflects what shipped.

**Key reference:** `kernel/src/boot_phase.rs`, `shared/src/boot.rs`

**Acceptance:** `just check` + `just test` pass; UART boot log shows `[Boot] InputReady` phase transition

-----

### Step 7: Shared crate input types and unit tests

**What:** Comprehensive host-side tests for all input types. Ensure all types are no_std compatible.

**Tasks:**
- [x] Add `#[cfg(test)] mod tests` to `shared/src/input.rs`
- [x] Test: `VirtioInputEvent` is exactly 8 bytes repr(C)
- [x] Test: `KeyCode` round-trips through evdev keycode conversion
- [x] Test: `Modifiers` bitflags combine correctly
- [x] Test: US-QWERTY keymap maps KEY_A(30) → ('a', 'A')
- [x] Test: absolute→display coordinate conversion at boundaries (0, 16383, 32767)
- [x] Test: `InputEvent` enum variants construct correctly
- [x] Test: `KeyState` discrimination (Pressed/Released/Repeat)
- [x] Target: 20+ new tests in input module

**Acceptance:** `just check` + `just test` pass with 20+ new input tests

-----

## Milestone 24 — Compositor Core (End of Week 4)

*Goal: Implement the compositor as a system service with IPC-based surface lifecycle, shared-buffer software composition, and multi-surface display. The compositor reads surface buffers from shared memory, alpha-blends them into a DMA-backed composition buffer, and pushes the result to the VirtIO-GPU display. Includes clean display handoff from GPU Service.*

### Step 8: Compositor shared types

**What:** Define compositor protocol types in the shared crate. All message types must fit within MAX_MESSAGE_SIZE (256 bytes) — verified with compile-time assertions.

**Tasks:**
- [x] Create `shared/src/compositor.rs` with `pub mod compositor;` in `shared/src/lib.rs`
- [x] Define `SurfaceId(u64)` — unique surface identifier
- [x] Define `SurfaceState` enum: `Created`, `Configured`, `Active`, `Suspended`, `Destroyed`
- [x] Define `SurfaceLayer` enum: `Background = 0`, `Normal = 1`, `TopLevel = 2`, `Overlay = 3`, `Panel = 4`
- [x] Define `SurfaceTitle` struct: `{ bytes: [u8; 64], len: u8 }` — UTF-8 encoded, similar to `ServiceName`
- [x] Define `SurfaceContentType` enum (simplified): `Document`, `Terminal`, `Browser`, `Game`, `Settings`, `SystemUI`, `Generic`
- [x] Define `DamageRegion` enum: `Rect { x: u32, y: u32, width: u32, height: u32 }`, `FullSurface`, `Empty`
- [x] Define `CompositorRequest` repr(C): `CreateSurface { width, height, title, content_type, layer }`, `AttachBuffer { surface_id, shmem_id, damage }`, `DestroySurface { surface_id }`, `Resize { surface_id, width, height }`, `SetLayer { surface_id, layer }`
- [x] Define `CompositorEvent` repr(C): `Configure { surface_id, width, height, scale_x100 }`, `FocusChanged { surface_id, focused }`, `CloseRequested { surface_id }`, `BufferReleased { shmem_id }`, `FramePresented { surface_id, timestamp_ticks }`, `Input { surface_id, event: InputEvent }`
- [x] Add `Compositor = 15` variant to `Subsystem` enum, update `COUNT` to 16, update `name()` and tests
- [x] Add compile-time size assertions: `CompositorRequest` ≤ 256 bytes, `CompositorEvent` ≤ 256 bytes
- [x] Write host-side tests: size assertions, SurfaceState ordering, SurfaceLayer ordering, SurfaceTitle construction and truncation

**Key reference:** [compositor/protocol.md](../platform/compositor/protocol.md) §3.1, §3.4

**Acceptance:** `just check` zero warnings. `just test` passes with new compositor type tests. All message types ≤ 256 bytes.

-----

### Step 9: Compositor capability types

**What:** Add compositor-related capability variants to the existing flat `Capability` enum. Phase 7 uses the flat enum pattern (consistent with existing GPU capabilities). The rich `DisplayCapability` struct from security.md §10.1 is deferred to Phase 18.

**Tasks:**
- [x] Add to `Capability` enum in `shared/src/cap.rs`: `CompositorCreateSurface`, `CompositorFullscreen`, `CompositorOverlay`, `CompositorInputAccess`
- [x] Update `Capability::permits()` match arms for new variants
- [x] Update capability unit tests
- [x] Document: Phase 7 uses flat capabilities; rich `DisplayCapability` struct deferred to Phase 18

**Key reference:** [compositor/security.md](../platform/compositor/security.md) §10.1, `shared/src/cap.rs`

**Acceptance:** `just check` + `just test` pass; new capability variants compile and match correctly

-----

### Step 10: Compositor service process

**What:** Create the compositor service following the GPU Service pattern (`ProcessId(10)`, `SchedulerClass::Interactive`). Register IPC channel as "compositor". The compositor uses direct VirtIO-GPU driver access (not IPC to GPU Service) for frame submission — same trust level, no round-trip overhead.

**Tasks:**
- [x] Create `kernel/src/compositor/mod.rs` and `kernel/src/compositor/service.rs`
- [x] Define `COMPOSITOR_CHANNEL: Mutex<Option<ChannelId>>`
- [x] Implement `init_compositor()` — create process (`ProcessId(10)`, name="compositor"), grant capabilities (`CompositorCreateSurface`, `GpuMmioAccess`, `ChannelCreate`, `DebugPrint`), create IPC channel, register service as "compositor", spawn compositor thread
- [x] Implement `compositor_entry()` — unmask IRQs, enter main loop
- [x] Add `compositor::init_compositor()` call to `kernel/src/main.rs` after GPU Service init
- [x] Document lock ordering: `... > BLOCK_ENGINE > SURFACE_TABLE > INPUT_EVENT_QUEUE > {VIRTIO_BLK, VIRTIO_GPU, VIRTIO_INPUT}`

**Note:** Lock ordering for compositor globals must be documented at each declaration site with `// Lock ordering:` comments, following the pattern in ipc/shmem.rs and ipc/notify.rs.

**Key reference:** `kernel/src/gpu/service.rs` (pattern), [deadlock-prevention.md](../kernel/deadlock-prevention.md) §3

**Acceptance:** Compositor process starts; UART shows `[Compositor] started, channel=N`; `just run-gpu` boots successfully

-----

### Step 11: Display handoff from GPU Service

**What:** The compositor takes ownership of display resources from the GPU Service. Allocate DMA-backed composition buffers (double-buffered), create VirtIO-GPU resources, and swap in as the new scanout. Release GPU Service's old display buffers. After handoff, only the compositor drives the display.

**Note:** DMA pool budget: GPU Service uses ~8MB (2×4MB). Compositor allocates ~8MB more. Total ~16MB out of 64MB DMA pool. Client surface buffers use `Pool::Kernel` (not DMA) — only the compositor's final composition buffer needs DMA for VirtIO-GPU transfer.

**Tasks:**
- [x] Implement `display_handoff()` in compositor service: get display info from `virtio_gpu::display_info()`
- [x] Allocate 2 DMA-backed composition buffers via `alloc_dma_pages()` (order-10 for 1280×800×4)
- [x] Create VirtIO-GPU resources for both buffers (`gpu_allocate_framebuffer()`)
- [x] Render initial frame (solid background color) to back buffer
- [x] Set compositor's buffer as scanout via `gpu_set_scanout()`
- [x] Signal GPU Service to stop its display loop (set a global `COMPOSITOR_ACTIVE: AtomicBool`)
- [x] Log handoff completion to UART

**Note (post-implementation):** Surfaced and fixed a pre-existing bounds-check
gap in `cap/mod.rs` — `check_channel_create`, `check_channel_access`,
`check_shared_memory_create`, and `check_shared_memory_access` indexed
`PROCESS_TABLE` with `pid.0 as usize` without first verifying the index was in
range, causing rare `index out of bounds` panics on garbage `pid` values
returned from torn `Thread.owner_pid` reads. Each function now returns `Eperm`
when `pid.0 >= MAX_PROCESSES`, matching the `None`-slot fallthrough.

**Key reference:** `kernel/src/gpu/service.rs` (existing buffer allocation), [compositor.md](../platform/compositor.md) §2

**Acceptance:** Compositor takes over display; QEMU shows compositor's solid background; no black frame or flickering during handoff

-----

### Step 12: Surface lifecycle management

**What:** Implement the surface table and state machine. Handle `CreateSurface` (allocate `SurfaceId`, send `Configure` event), `AttachBuffer` (update front buffer reference, transition to `Active`), `DestroySurface` (cleanup resources). Use fixed-size array (not Vec) to avoid OOM panic risk.

**Tasks:**
- [x] Create `kernel/src/compositor/surface.rs`
- [x] Define `Surface` struct: `id: SurfaceId`, `state: SurfaceState`, `layer: SurfaceLayer`, `title: SurfaceTitle`, `content_type: SurfaceContentType`, `x: i32`, `y: i32`, `width: u32`, `height: u32`, `shmem_id: Option<SharedMemoryId>`, `owner_pid: ProcessId`, `channel: ChannelId`, `damaged: bool`
- [x] Define `MAX_SURFACES: usize = 32` and `static SURFACE_TABLE: Mutex<[Option<Surface>; MAX_SURFACES]>` with lock ordering comment
- [x] Implement `surface_create()` — validate capability, allocate SurfaceId (monotonic counter), insert into table, send `Configure` event via IPC
- [x] Implement `surface_attach_buffer()` — validate surface exists and owned by caller, update shmem_id, set `damaged=true`, transition state to `Active` if first buffer
- [x] Implement `surface_destroy()` — mark as `Destroyed`, release resources, remove from table
- [x] Implement `surface_resize()` — update dimensions, send new `Configure` event
- [x] Validate state machine transitions per protocol.md §3.1 state diagram

**Key reference:** [compositor/protocol.md](../platform/compositor/protocol.md) §3.1

**Acceptance:** `CreateSurface` IPC call returns `SurfaceId` and `Configure` event; `AttachBuffer` transitions to Active; `DestroySurface` cleans up

-----

### Step 13: Software compositor — flat z-order blitting

**What:** Implement the core composition function. Iterate surfaces in z-order (layer first, then insertion order within layer), for each visible `Active` surface read pixels from its shared memory buffer and write into the DMA composition buffer. Use `Xrgb8888` opaque blitting for Phase 7 (premultiplied alpha blending infrastructure defined but shell surfaces are opaque). Implement full-surface and rect damage tracking.

**Note:** Premultiplied alpha formula (for future transparent surfaces): `out = src + dst * (1 - src_alpha)`. Phase 7 shell surfaces use `Xrgb8888` (opaque, no blending needed).

**Tasks:**
- [x] Create `kernel/src/compositor/render.rs`
- [x] Implement `compose_frame(surfaces, comp_buffer)` — iterate surfaces in z-order, blit visible regions to composition buffer
- [x] Implement `blit_opaque(src_ptr, dst_ptr, src_rect, dst_x, dst_y, stride)` — per-pixel copy for `Xrgb8888`
- [x] Implement `blit_alpha_premultiplied(src_ptr, dst_ptr, ...)` — premultiplied alpha blend for `Argb8888` (used by window decorations)
- [x] Implement `DamageTracker` — per-surface dirty flags, screen-space damage accumulation, union of all damage regions
- [x] Implement damage-driven composition: only redraw regions that changed since last frame
- [x] Clear background to AIOS blue (`#5B8CFF` = 0xFF5B8CFF in B8G8R8A8) in undamaged areas
- [x] Map pixel format: compositor internal `Xrgb8888` maps to VirtIO-GPU `B8G8R8A8Unorm`

**Key reference:** [compositor/rendering.md](../platform/compositor/rendering.md) §5.2 Frame Composition, §5.1 (simplified)

**Acceptance:** Two test surfaces at different positions composited correctly; overlapping shows correct layering; background visible around surfaces

-----

### Step 14: Composition loop and frame pacing

**What:** The compositor main loop: poll input, receive IPC requests, update surface state, compose if damaged, transfer+flush to VirtIO-GPU. Target 60fps (16.67ms budget). Skip frames when no damage (idle power saving).

**Tasks:**
- [x] Implement compositor main loop in `service.rs`: `loop { poll_input(); process_ipc(); if any_damage() { compose_frame(); present(); } yield_or_sleep(); }`
- [x] Use `TICK_COUNT` for frame pacing: compose at most once per 16ms
- [x] Call `gpu_transfer_to_host()` + `gpu_resource_flush()` to present composed frame
- [x] Implement double-buffer swap: render to back buffer, then swap (rebind scanout)
- [x] Skip composition when no surface has damage (static desktop → zero GPU work)
- [x] Log frame timing statistics to UART every 60 frames (once per second)
- [x] Add watchdog: log warning if any frame takes >100ms

**Note (post-implementation):** The compose-and-present pipeline is wired
end-to-end (`present_frame_if_due` → `present_frame` → `compose_frame` →
`gpu_transfer_to_host` + `gpu_resource_flush` → `swap_buffers_after_compose`)
and gated by a single `COMPOSITOR_PRESENT_ENABLED` flag. M24 ships the flag
**off** because the post-handoff IPC bench path surfaces three pre-existing
kernel-side races when the compositor adds frame-pacing pressure
(`cap/mod.rs:86` torn `pid` reads, fixed in Step 11; `virtio_input.rs:228`
modulo-by-zero on uninitialized `queue_size`, fixed in Step 14; an unrelated
data abort at low VAs that surfaces only with M24's added activity). Step 17
(M25) flips the flag once IPC dispatch wires real client surfaces and the
remaining race is root-caused.

**Key reference:** [compositor/rendering.md](../platform/compositor/rendering.md) §5.4 Frame Scheduling

**Acceptance:** Compositor composites at ~60fps when surfaces have damage; goes idle when static; frame timing logged; no >100ms warnings

-----

### Step 15: Multi-surface composition test

**What:** Create a test that spawns 3 surfaces at different layers and positions. Validates the entire compositor pipeline end-to-end: IPC surface creation, shared memory buffer writing, composition, and display.

**Tasks:**
- [x] Implement `compositor_test()` in `kernel/src/compositor/mod.rs`
- [x] Create 3 test surfaces: background (layer Background, full-screen, dark gray), window (layer Normal, 400×300 at position 100,100, blue), overlay (layer Overlay, 200×50, semi-transparent yellow)
- [x] Allocate shared memory for each surface, write solid color pixels
- [x] Attach buffers via IPC, verify Configure events received
- [x] Verify z-ordering: overlay on top of window on top of background
- [x] Destroy surfaces and verify cleanup

**Note (post-implementation):** Per the plan's Step 15d alternative, the
multi-surface composition test is implemented as a host-side unit test in
`shared/src/compositor.rs::tests::multi_surface_composition_z_order` rather
than a kernel-side runtime test. This exercises the same blit + z-order
logic the kernel uses (via `host_blit_opaque` mirroring
`render::blit_opaque`) plus damage-tracker accumulation, verifies the
overlay-on-top-of-window-on-top-of-background z-order, and checks that
clipping and off-screen blits behave correctly. Coverage is broader than
the QEMU-only test would have been because boundary cases are
deterministic in the host environment.

**Acceptance:** `just run-input` shows 3 colored rectangles composited at correct z-order; UART logs surface lifecycle events

-----

### Step 16: Shared crate compositor types and unit tests

**What:** Comprehensive host-side tests for all compositor shared types. Move any pure data structures from kernel to shared.

**Tasks:**
- [x] Add `#[cfg(test)] mod tests` to `shared/src/compositor.rs`
- [x] Test: `CompositorRequest` and `CompositorEvent` repr(C) size ≤ 256 bytes
- [x] Test: `SurfaceState` ordering and valid transitions
- [x] Test: `SurfaceLayer` ordering (Background < Normal < TopLevel < Overlay < Panel)
- [x] Test: `SurfaceTitle` construction, truncation at 64 bytes, UTF-8 preservation
- [x] Test: `DamageRegion::Rect` contains correct coordinates
- [x] Test: `SurfaceId` monotonic generation
- [x] Target: 15+ new tests

**Note (post-implementation):** 31 tests landed in
`shared/src/compositor.rs::tests` covering request/event size,
SurfaceState transitions (forward, suspend, destroy-from-anywhere, and
invalid), SurfaceLayer ordering, SurfaceTitle truncation (ASCII,
multi-byte UTF-8 boundary safety), DamageRegion + DamageTracker, command
round-trips, multi-surface composition + clip + off-screen, plus IPC
event encoders (Configure, Input keyboard, Input pointer).

**Acceptance:** `just check` + `just test` pass with 15+ new compositor tests

-----

## Milestone 25 — Window Manager & Input Routing (End of Week 5)

*Goal: Implement floating window management with decorations, pointer hit-testing with software cursor, keyboard/pointer focus management, the input routing pipeline, window move/resize, and Alt+Tab switching.*

### Step 17: Window manager — floating layout and decorations

**What:** Implement `WindowManager` that manages surface positions and sizes in floating layout. Compositor draws window decorations: title bar (24px height with surface title text via spleen-font), close button (X glyph). Decorations are rendered by the compositor, not the surface owner.

**Tasks:**
- [x] Create `kernel/src/compositor/window.rs`
- [x] Define `WindowDecoration` struct: `title_bar_height: u32 = 24`, `border_width: u32 = 1`, `close_button_width: u32 = 24`
- [x] Implement `render_title_bar(surface, buffer)` — draw title bar background, render title text using spleen-font 16×32 (scaled to 8×16 for title bar), draw close button "X" glyph
- [x] Implement `render_focus_indicator(surface, buffer)` — colored border (blue for focused, gray for unfocused)
- [x] Track z-order as `Vec<SurfaceId>` (most-recently-focused last = top)
- [x] Implement `raise_to_top(surface_id)` — move to end of z-order list
- [x] Place new surfaces at default position (centered, with slight offset for each new window)

**Note (post-implementation):** Title bar text is rendered with spleen
**8×16** (added the `s8x16` feature to `kernel/Cargo.toml`) instead of
the 16×32 scaled-down approach in the original draft — 8×16 fits the
24-pixel bar with 4 px of padding above and below and avoids the
nearest-neighbour downsample. The compositor-side text helper lives in
a new `kernel/src/compositor/text.rs` (slice-based, distinct from the
existing pointer-based `kernel/src/gpu/text.rs`). Z-order is a
`shared::compositor::ZOrder` array (not `Vec`) — fixed-size, no
allocations, matches `MAX_SURFACES`.

**Key reference:** [compositor/rendering.md](../platform/compositor/rendering.md) §6.1 Layout Modes

**Acceptance:** Surfaces display with title bars showing their name; close button visible; focused surface has colored border

-----

### Step 18: Pointer hit-testing and software cursor

**What:** Implement hit-testing to find the topmost surface under the pointer. Render a software cursor (16×16 pixel arrow sprite embedded as const data). The cursor composites on top of all surfaces.

**Tasks:**
- [x] Implement `hit_test(x, y) -> Option<(SurfaceId, HitZone)>` — walk z-order list top-to-bottom, check bounds including decorations
- [x] Define `HitZone` enum: `TitleBar`, `CloseButton`, `Content`, `ResizeBorderN/S/E/W/NE/NW/SE/SW`
- [x] Define cursor sprite as `const CURSOR_ARROW: [u32; 16 * 16]` — 16×16 RGBA pixels, arrow shape with black outline and white fill
- [x] Implement `render_cursor(comp_buffer, x, y)` — alpha-blend cursor sprite at pointer position (last operation before present)
- [x] Track cursor position from `InputEvent::Pointer` updates

**Note (post-implementation):** `HitZone::ResizeBorder` carries a single
`ResizeEdge` enum variant (`N/S/E/W/NE/NW/SE/SW`) instead of eight
flat zone variants — keeps the match arms in `handle_decoration_event`
flatter and lets the resize-delta math share a single computation
parameterized by edge. The geometric helper `hit_zone()` lives in
`shared/src/compositor.rs` so it gets host-side test coverage.

**Key reference:** [compositor/input.md](../platform/compositor/input.md) §7.2

**Acceptance:** Moving tablet shows arrow cursor tracking pointer position; cursor renders on top of all windows

-----

### Step 19: Focus management

**What:** Implement `FocusManager` per compositor/input.md §7.2: separate keyboard and pointer focus, focus history ring buffer.

**Tasks:**
- [x] Create `kernel/src/compositor/focus.rs`
- [x] Define `FocusManager`: `keyboard_focus: Option<SurfaceId>`, `pointer_focus: Option<SurfaceId>`, `focus_history: FixedQueue<SurfaceId, 16>`
- [x] Clicking a surface sets keyboard focus and raises it to top of z-order
- [x] Send `FocusChanged { focused: true/false }` events via IPC on focus change
- [x] Pointer focus follows cursor position (via hit-test, no click required) — used for routing pointer events
- [x] Focus steal prevention: only user-initiated actions (click, Alt+Tab) can change keyboard focus

**Note (post-implementation):** `focus_history` is a custom MRU
container (`shared::compositor::FocusHistory`) rather than the generic
`FixedQueue<SurfaceId, 16>` — a true MRU ring needs `touch(id)` to
move existing entries to the front, which `FixedQueue`'s FIFO API
does not provide. `FocusManager` operations return a `FocusChange`
struct so the caller can drop the manager mutex before issuing IPC
events (lock-ordering invariant).

**Key reference:** [compositor/input.md](../platform/compositor/input.md) §7.2

**Acceptance:** Clicking a surface gives it keyboard focus; `FocusChanged` sent via IPC; focused surface raised to top with visual indicator

-----

### Step 20: Input routing pipeline

**What:** Implement the input pipeline stages: Event Coalescing → Hotkey Filter → Focus Router → Agent Delivery. Simplified from the full 6-stage architecture (Device Driver stage is handled in Step 3–4; Gesture Recognizer deferred to Phase 8).

**Tasks:**
- [x] Create `kernel/src/compositor/input_route.rs`
- [x] Define `InputFilter` trait: `fn filter(&mut self, event: &InputEvent) -> FilterResult` with `FilterResult::Pass/Consume/Transform`
- [x] Implement event coalescing: merge multiple pointer events within a frame interval into one (latest position wins)
- [x] Implement hotkey filter: check keyboard events against system hotkey table before routing
- [x] Implement focus routing: keyboard events → keyboard_focus surface, pointer events → pointer_focus surface (from hit-test)
- [x] Implement agent delivery: serialize `InputEvent` into `CompositorEvent::Input` and send via the surface's IPC channel
- [x] Wire pipeline into compositor main loop: after `poll_input()`, run events through pipeline

**Note (post-implementation):** Step 20 also lands the IPC dispatch
that was implicitly required by M24 Step 12 — the compositor service
loop now decodes incoming `CompositorRequest` payloads and dispatches
to `surface_create/_attach_buffer/_destroy/_resize/_set_layer`
instead of replying with an empty ack. Without this, the input
router would have no surfaces to route events to. M25 keeps all
surfaces on the well-known compositor channel as a placeholder; M26
will introduce per-client channels alongside the test app.

The pure routing decision lives in `shared::compositor::route_event`
so it gets host-side test coverage; the kernel side wires it into
`drain_and_route` which drains `crate::input::INPUT_QUEUE` once per
compositor loop iteration.

**Key reference:** [compositor/input.md](../platform/compositor/input.md) §7.1

**Acceptance:** Key presses arrive at focused surface's IPC channel; pointer events arrive at surface under cursor; coalescing reduces redundant motion events

-----

### Step 21: Window move and resize

**What:** Title bar drag → window move. Edge/corner resize zones (8px border) → window resize. Minimum window size 200×100.

**Tasks:**
- [x] Implement move mode: when pointer pressed on `HitZone::TitleBar`, track delta from initial position, update surface x/y each frame until pointer released
- [x] Implement resize mode: when pointer pressed on `HitZone::ResizeBorder*`, resize surface dimensions, send `Configure` event to surface's IPC channel
- [x] Enforce minimum window size: 200×100 pixels
- [x] Implement close button: when pointer clicked on `HitZone::CloseButton`, send `CloseRequested` event to surface

**Note (post-implementation):** North/West resize edges also re-anchor
the origin so the opposite edge stays fixed when the clamp truncates
the size below `MIN_WINDOW_*`. The drag state machine
(`DragState::Idle/Moving/Resizing`) is a single global mutex
(`DRAG_STATE`) — at most one window drags at a time.

**Key reference:** [compositor/rendering.md](../platform/compositor/rendering.md) §6.1

**Acceptance:** Dragging title bar moves window smoothly; dragging edges resizes; close button sends `CloseRequested`; Configure events sent on resize

-----

### Step 22: Alt+Tab window switching and system hotkeys

**What:** Register system hotkeys that are consumed before any surface receives them. Alt+Tab cycles through focus history. Alt+F4 sends `CloseRequested`.

**Tasks:**
- [x] Create `kernel/src/compositor/hotkey.rs`
- [x] Define `HotkeyBinding`: `key_combo`, `action: HotkeyAction`
- [x] Define `HotkeyAction` enum: `SwitchWindow`, `CloseWindow`, `ShowWorkspace`
- [x] Register: Alt+Tab → SwitchWindow, Alt+F4 → CloseWindow, Super → ShowWorkspace
- [x] Alt+Tab: cycle through `focus_history`, set keyboard focus to next entry, raise to top
- [x] Alt+F4: send `CloseRequested` to currently focused surface
- [x] Super key: toggle Workspace surface visibility (Step 27)
- [x] Hotkeys consumed by the filter — never forwarded to surfaces

**Note (post-implementation):** Bare-Super → ShowWorkspace is wired in
the `apply()` dispatch but **not** registered in `SYSTEM_HOTKEYS` for
M25 — the bare-Super press carries `Modifiers::SUPER` set on the same
event, which makes the matching ambiguous without a release-edge
detector. Workspace toggle activation lands in M26 alongside the
Workspace surface itself; the dispatch path emits an
`info!("ShowWorkspace placeholder")` log when invoked.

**Key reference:** [compositor/input.md](../platform/compositor/input.md) §7.3

**Acceptance:** Alt+Tab switches focus between surfaces; Alt+F4 sends CloseRequested; hotkeys not forwarded to surfaces

-----

### Step 23: Shared crate window manager types and tests

**What:** Move any reusable types to shared crate. Comprehensive unit tests for hit-testing, focus history, z-order operations.

**Tasks:**
- [x] Add any shared types needed (e.g., `HitZone`, `WindowPosition`, `WindowSize`) to `shared/src/compositor.rs`
- [x] Test: hit-test with overlapping surfaces returns correct topmost surface
- [x] Test: focus history ring buffer wraps correctly at capacity 16
- [x] Test: z-order raise operation moves surface to top
- [x] Test: hotkey matching for Alt+Tab, Alt+F4, Super
- [x] Target: 15+ new tests

**Note (post-implementation):** 25 new tests landed in
`shared/src/compositor.rs::tests` covering: `SurfaceState::is_visible`;
`hit_zone` outside/corners/edges/close-button/title-bar/content;
`ZOrder` push, iter, iter_top_down, raise_to_top, remove,
full-capacity rejection, and NONE rejection; `FocusHistory` empty,
touch-promotes-existing, evicts-LRU at capacity, remove, and NONE
rejection; `route_event` keyboard-with-focus / no-focus / pointer-
content / pointer-decoration / pointer-no-hit; `clamp_window_size`
minimum and exact-minimum; and `WindowDecoration::DEFAULT` metrics.
Pure-logic types (`ZOrder`, `FocusHistory`, `RouteTarget`, `route_event`,
`clamp_window_size`) were moved from kernel to shared so the same
code is exercised by host-side `just test`. Total test count:
518 → 543.

**Acceptance:** `just check` + `just test` pass with 15+ new window manager tests

-----

## Milestone 26 — Desktop Shell (End of Week 6)

*Goal: Implement the three shell surfaces: Status Strip (top bar), Taskbar (bottom bar), and Workspace (home view). All are compositor-internal surfaces rendered using spleen-font bitmap text. Includes a test application validating the full IPC surface lifecycle.*

### Step 24: Status Strip surface

**What:** Panel-layer surface locked to the top edge (1280×32 pixels). Displays system time (HH:MM from TICK_COUNT), CPU load, memory usage, core count. Refreshes every 1 second. Compositor-internal (not a separate process).

**Tasks:**
- [ ] Create `kernel/src/compositor/shell/mod.rs` and `kernel/src/compositor/shell/status_strip.rs`
- [ ] Implement `StatusStrip` struct: tracks last-render tick, cached display values
- [ ] Render text using spleen-font 16×32 bitmap glyphs (reuse `gpu::text` renderer)
- [ ] Display: `AIOS  HH:MM  CPU: NN%  MEM: NN%  CORES: N`
- [ ] Calculate time from `TICK_COUNT` (ticks since boot / 1000 = seconds)
- [ ] Calculate memory from frame allocator stats
- [ ] Calculate CPU from scheduler metrics (if available, else "N/A")
- [ ] Only redraw when display values change (damage optimization)
- [ ] Register as Panel-layer surface (always on top of Normal windows)

**Key reference:** [experience.md](../experience/experience.md) §6 (Status Strip)

**Acceptance:** Top bar visible showing time, memory%, core count; updates every second; doesn't obscure window content below

-----

### Step 25: Taskbar surface

**What:** Panel-layer surface locked to the bottom edge (1280×40 pixels). Displays a horizontal list of active surface titles. Focused surface entry is highlighted. Workspace button on the left.

**Tasks:**
- [ ] Create `kernel/src/compositor/shell/taskbar.rs`
- [ ] Render horizontal list of active non-shell surface titles (truncated to fit)
- [ ] Highlight focused surface entry with a different background color
- [ ] Left side: `[W]` workspace button (toggles Workspace visibility)
- [ ] Right side: surface count display
- [ ] Only redraw when surface list or focus changes

**Key reference:** [experience.md](../experience/experience.md) §2 (Five Surfaces)

**Acceptance:** Bottom bar shows active surface titles; clicking entry focuses the surface; focused entry highlighted

-----

### Step 26: Workspace surface

**What:** Normal-layer surface shown via Super key or workspace button. Static layout (no context inference — Layer 1). Displays AIOS title, system spaces list, uptime.

**Tasks:**
- [ ] Create `kernel/src/compositor/shell/workspace.rs`
- [ ] Render: "AIOS" title centered, system spaces list (from `storage::space_list()`), uptime (from TICK_COUNT)
- [ ] Static layout — no context inference, no adaptive behavior
- [ ] Toggle visibility: Super key or taskbar [W] button
- [ ] When visible, positioned behind other Normal-layer windows but above Background

**Key reference:** [experience.md](../experience/experience.md) §3.1–3.2 (Workspace)

**Acceptance:** Super key toggles Workspace; shows spaces list and uptime; workspace button in taskbar works

-----

### Step 27: Shell input integration

**What:** Wire shell surfaces into the input routing pipeline. Taskbar and Status Strip are Panel-layer and receive pointer events for their interactive elements.

**Tasks:**
- [ ] Taskbar entries are clickable: clicking sets focus to the corresponding surface
- [ ] Workspace button in taskbar toggles Workspace visibility
- [ ] Close button in Status Strip not needed (always visible)
- [ ] Shell surfaces don't receive keyboard focus (keyboard events always go to application surfaces)

**Key reference:** [compositor/input.md](../platform/compositor/input.md) §7.2

**Acceptance:** Clicking taskbar entries switches focus; workspace toggle works from taskbar

-----

### Step 28: Test application surface

**What:** A kernel-side test process that creates a surface via IPC, allocates shared memory, renders content (colored rectangle with text "Hello from AIOS!"), and responds to keyboard input by appending typed characters. Validates the full IPC surface lifecycle end-to-end.

**Tasks:**
- [ ] Create `kernel/src/compositor/test_app.rs`
- [ ] Spawn test process (`ProcessId(11)`, name="test-app")
- [ ] Create surface via IPC: `CompositorRequest::CreateSurface`
- [ ] Allocate shared memory buffer, write solid color background + text using spleen-font
- [ ] Receive `Configure` event, attach buffer
- [ ] Handle `CompositorEvent::Input` — keyboard events append characters to display text, re-render buffer, re-attach
- [ ] Handle `CloseRequested` — destroy surface and exit

**Key reference:** Full stack validation

**Acceptance:** Test app window visible at 400×300; shows "Hello from AIOS!"; keyboard input appends characters; Alt+Tab switches between test app and workspace; Alt+F4 closes it

-----

### Step 29: Shell rendering optimization

**What:** Optimize shell rendering: minimize composition work when only shell content changed. Profile and log composition times.

**Tasks:**
- [ ] Status Strip: only redraw when time changes (1/sec) or metrics change
- [ ] Taskbar: only redraw when surface list or focus changes
- [ ] Workspace: only redraw on toggle
- [ ] Per-surface damage tracking: shell surfaces set `DamageRegion::Empty` when unchanged
- [ ] Log composition time per frame to UART (average over 60 frames)
- [ ] Profile full-frame vs damage-optimized composition: measure and report savings

**Acceptance:** Composition time <5ms for typical desktop (3 shell surfaces + 1 app); UART shows frame timing; static desktop = 0ms composition

-----

### Step 30: Shell shared types and unit tests

**What:** Tests for shell rendering logic. Ensure shell text layout calculations are correct.

**Tasks:**
- [ ] Test: Status Strip time formatting (ticks → HH:MM)
- [ ] Test: Taskbar entry layout (title truncation, highlighting)
- [ ] Test: Workspace spaces list rendering
- [ ] Test: damage tracking correctly identifies unchanged frames
- [ ] Target: 10+ new tests

**Acceptance:** `just check` + `just test` pass with 10+ new shell tests

-----

## Milestone 27 — Input Kit, Integration & Gate (End of Week 7)

*Goal: Extract Input Kit traits, add animation stubs, run Gate 2 benchmarks, update QEMU targets, update all documentation, and pass all quality gates.*

### Step 31: Input Kit Tier 1 trait extraction

**What:** Extract Input Kit traits to `shared/src/kits/input.rs` following the Kit pattern (Memory Kit, Capability Kit, etc.). Phase 7 extracts minimal traits for keyboard + pointer. Full trait set (GestureEvent, TextEvent, GamepadEvent, TouchEvent) deferred to Phase 8+.

**Tasks:**
- [ ] Create `shared/src/kits/input.rs`, add `pub mod input;` to `shared/src/kits/mod.rs`
- [ ] Define `InputKitError` enum: `DeviceNotFound`, `QueueFull`, `EventDropped`, `InvalidKeyCode`, `Timeout`
- [ ] Define `InputDevice` trait: `fn id(&self) -> InputDeviceId`, `fn name(&self) -> &[u8]`, `fn capabilities(&self) -> InputCapabilities`, `fn is_connected(&self) -> bool`
- [ ] Define `InputEventReceiver` trait: `fn poll_event(&self) -> Result<Option<InputEvent>, InputKitError>`, `fn has_events(&self) -> bool`
- [ ] Define `FocusOps` trait: `fn keyboard_focus(&self) -> Option<SurfaceId>`, `fn pointer_focus(&self) -> Option<SurfaceId>`, `fn request_focus(&self, surface: SurfaceId) -> Result<(), InputKitError>`
- [ ] Define `InputCapabilities` bitflags: `KEY`, `REL_AXIS`, `ABS_AXIS`, `KEYBOARD`, `MOUSE`, `TOUCHPAD`
- [ ] Implement kernel wrappers: `KernelInputDevice`, `KernelInputEventReceiver`, `KernelFocusManager` as zero-sized unit structs
- [ ] Ensure all traits are dyn-compatible
- [ ] Add `pub use kits::input as input_kit` to `shared/src/lib.rs`
- [ ] Write host-side tests: trait method signatures compile, error enum round-trips, capabilities combine correctly

**Key reference:** [kits/platform/input.md](../kits/platform/input.md) §2, existing Kit patterns in `shared/src/kits/`

**Acceptance:** `just check` + `just test` pass; all Input Kit traits are dyn-compatible; 10+ new tests

-----

### Step 32: Animation stubs

**What:** Placeholder animation infrastructure for future window open/close/resize transitions. Phase 7: all transitions are instant. Defines the types and API surface without actual animation.

**Tasks:**
- [ ] Create `kernel/src/compositor/animation.rs`
- [ ] Define `AnimationState` enum: `Idle`, `Running`, `Complete`
- [ ] Define `WindowTransition` enum: `Open`, `Close`, `Resize`, `Move`
- [ ] Define `Animation` struct: `surface_id`, `transition`, `state`, `progress: f32` (0.0–1.0)
- [ ] Implement stub `animate()` — immediately sets `progress=1.0`, `state=Complete`
- [ ] Compositor calls `animate()` on window open/close — transition is instant

**Key reference:** [compositor/rendering.md](../platform/compositor/rendering.md) §5.5

**Acceptance:** Window open/close works (instantly); animation infrastructure compiles; no visual delay

-----

### Step 33: Gate 2 benchmarks

**What:** Phase 7 performance benchmarks, following the Gate 1 pattern from Phase 3.

**Tasks:**
- [ ] Add Gate 2 benchmarks to `kernel/src/bench.rs`
- [ ] Benchmark 1: compositor frame composition time — compose 3 surfaces, measure time (target <5ms)
- [ ] Benchmark 2: input event latency — inject event, measure time until surface IPC delivery (target <2ms)
- [ ] Benchmark 3: surface creation IPC round-trip — CreateSurface → Configure (target <1ms)
- [ ] Benchmark 4: focus switch time — click → FocusChanged delivery (target <1ms)
- [ ] Log results to UART in same format as Gate 1

**Key reference:** `kernel/src/bench.rs` (Gate 1 pattern)

**Acceptance:** All benchmarks run; composition <5ms; input latency <2ms; surface creation <1ms; focus switch <1ms

-----

### Step 34: QEMU target updates

**What:** Update justfile recipes for Phase 7. The primary development target becomes `run-compositor`.

**Tasks:**
- [ ] Add `run-compositor` recipe: same as `run-gpu` + `-device virtio-keyboard-device -device virtio-tablet-device` (the new default for interactive development)
- [ ] Ensure `run-input` also has GPU device for visual feedback
- [ ] Ensure `just run` (text-only, no GPU) still boots normally
- [ ] Add comments to `run-compositor`: click into QEMU window for input, Ctrl+Alt+G to release grab

**Acceptance:** `just run-compositor` boots to graphical desktop with input; `just run` still works text-only

-----

### Step 35: Documentation updates

**What:** Update all project documentation to reflect Phase 7 changes.

**Tasks:**
- [ ] Update CLAUDE.md: Workspace Layout (new compositor/* and input/* modules, ~20 new files), Key Technical Facts (new constants: VirtIO-input device ID=18, MAX_SURFACES=32, MAX_INPUT_DEVICES=4, compositor ProcessId=10, etc.), Architecture Doc Map (new entries), lock ordering
- [ ] Update README.md: Project Structure, new QEMU targets, Phase 7 status
- [ ] Update `docs/project/developer-guide.md`: new modules, test counts, file sizes, patterns
- [ ] Check off Phase 7 tasks in phase doc
- [ ] Run audit loop

**Acceptance:** All docs accurate; audit loop returns 0 issues

-----

### Step 36: Final quality gates

**What:** Full quality gate suite.

**Tasks:**
- [ ] `just check` — zero warnings (fmt + clippy + build)
- [ ] `just test` — all pass (target: 500+ tests including ~65 new)
- [ ] `just run-compositor` — visual verification: Status Strip, Taskbar, movable test app window, input working
- [ ] `just run` — still boots normally (text-only fallback)
- [ ] CI passes on push
- [ ] Gate 2 benchmarks logged
- [ ] Audit loop final pass: 0 issues

**Acceptance:** All gates pass; boot log shows: VirtIO-input probed (2 devices), compositor started, display handoff complete, surfaces composited, input routing active, shell rendered, test app interactive

-----

## Decision Points

| Decision | Options | Recommendation | Rationale |
|---|---|---|---|
| Input polling vs IRQ | (a) Polled I/O at 60Hz frame tick (b) IRQ-driven via GICv3 SPI | Start with polling; IRQ optional upgrade | All existing VirtIO drivers use polling; IRQ needs DTB interrupt parsing + GICv3 SPI wiring (first in codebase); 60Hz polling adequate for Layer 1 |
| Compositor→GPU path | (a) IPC to GPU Service (b) Direct VirtIO-GPU driver access | Direct access | Compositor is same trust level; IPC adds ~4μs round-trip per frame; GPU Service stops display loop after handoff |
| Scene graph model | (a) Full SceneNode tree (b) Flat z-ordered surface list | Flat z-order list | Layer 1 has no effects (blur, shadow, rounded corners); flat list is simpler, adequate for 32 surfaces; full scene graph deferred to Phase 30+ |
| Shared memory limits | (a) Bump MAX_SHARED_REGIONS to 128 (b) Direct DMA for compositor buffers | Direct DMA for compositor; keep MAX_SHARED_REGIONS=64 | Compositor's composition buffers need DMA, not shared memory; client surface buffers use Pool::Kernel shared memory; avoids changing a kernel-wide constant |
| Capability model | (a) Rich DisplayCapability struct (b) Flat Capability enum variants | Flat enum variants | Consistent with existing pattern; sufficient for Layer 1; rich struct deferred to Phase 18 Security Hardening |
| Compositor crash recovery | (a) Restart mechanism (b) Robust error handling only | Robust error handling (Result<> throughout) | Full restart is Phase 18 territory; watchdog timer + UART warnings for now |
| Multi-monitor | (a) Full multi-output (b) Single-output with data structures | Single-output with infrastructure types | QEMU virt has one display; data structures support multiple outputs; exercised with one |
| Semantic hints | (a) Full SurfaceHints with layout behavior (b) Store-only, no behavior | Store-only | Layer 1 uses manual floating layout; hints stored for future Layer 2 consumption |
| Alpha blending | (a) Premultiplied (b) Straight | Premultiplied | Architecture spec (protocol.md §3.2); Phase 7 mostly opaque (Xrgb8888); formula correct for future transparent surfaces |

-----

## Phase Completion Criteria

- [ ] `just check` — zero warnings
- [ ] `just test` — all pass, >500 tests
- [ ] `just run-compositor` — graphical desktop: Status Strip (top), Taskbar (bottom), test app window (movable, resizable, keyboard-interactive), Alt+Tab switching, workspace toggle
- [ ] `just run` — text-only boot still works
- [ ] Gate 2: composition <5ms, input latency <2ms, surface creation <1ms, focus switch <1ms
- [ ] All milestones checked off above
- [ ] Lock ordering documented and verified
- [ ] Input Kit Tier 1 traits defined and dyn-compatible
