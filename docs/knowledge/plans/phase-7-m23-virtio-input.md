---
author: claude
date: 2026-03-26
tags: [input, drivers, virtio, compositor, gpu]
status: in-progress
phase: 7
milestone: M23
---

# Plan: Phase 7 M23 — VirtIO-Input Driver

## Approach

M23 adds VirtIO-input support (keyboard + tablet) on QEMU as the foundation for interactive display in later milestones. This is the first new VirtIO driver since Phase 6 (GPU), following the established pattern from `virtio_gpu.rs` and `virtio_blk.rs`.

**Current state:** Phase 6 complete (M19–M22). GPU Service running with IPC-based double-buffered display, Compute Kit Tier 1 extracted. No input subsystem exists — `kernel/src/input/` and `shared/src/input.rs` are both new.

**Key gaps:**
- No input types or constants in shared crate
- No VirtIO-input driver (device_id=18)
- No input event translation pipeline
- No QEMU run target with input devices
- `EarlyBootPhase` lacks `InputReady` and `CompositorReady` variants

**Novel challenges vs existing VirtIO drivers:**
1. **Multi-device**: Keyboard and tablet are separate MMIO slots — must find ALL device_id=18, not just first
2. **Device-to-driver eventq**: Available ring pre-filled with empty VirtioInputEvent buffers (reverse of blk/gpu pattern)
3. **Config select/subsel protocol**: Unique to VirtIO-input — write select+subsel, read size+data union

## Progress

- [ ] Step 1: VirtIO-input shared types and constants
  - [ ] 1a: Create `shared/src/input.rs`, add `pub mod input;` to `shared/src/lib.rs` (between `gpu` and `ipc` alphabetically). No root re-exports needed — types accessed via `shared::input::*` (same pattern as `shared::gpu::*` which has no root re-exports)
  - [ ] 1b: Define `VirtioInputEvent` — 8-byte repr(C): event_type(u16), code(u16), value(u32). Compile-time size assert.
  - [ ] 1c: Define evdev event type constants: `EV_SYN=0x00`, `EV_KEY=0x01`, `EV_REL=0x02`, `EV_ABS=0x03`, `SYN_REPORT=0`
  - [ ] 1d: Define evdev key code constants: `KEY_ESC(1)`, `KEY_1(2)`..`KEY_0(11)`, `KEY_BACKSPACE(14)`, `KEY_TAB(15)`, `KEY_Q(16)`..`KEY_P(25)`, `KEY_ENTER(28)`, `KEY_LEFTCTRL(29)`, `KEY_A(30)`..`KEY_Z(44)`, `KEY_LEFTSHIFT(42)`, `KEY_LEFTALT(56)`, `KEY_SPACE(57)`, `KEY_F1(59)`..`KEY_F12(88)`, `KEY_LEFTMETA(125)`, `KEY_UP(103)`, `KEY_DOWN(108)`, `KEY_LEFT(105)`, `KEY_RIGHT(106)`
  - [ ] 1e: Define evdev button constants: `BTN_LEFT(0x110)`, `BTN_RIGHT(0x111)`, `BTN_MIDDLE(0x112)`
  - [ ] 1f: Define abs axis constants: `ABS_X(0x00)`, `ABS_Y(0x01)`
  - [ ] 1g: Define VirtIO-input config select constants: `VIRTIO_INPUT_CFG_UNSET(0x00)`, `CFG_ID_NAME(0x01)`, `CFG_ID_SERIAL(0x02)`, `CFG_ID_DEVIDS(0x03)`, `CFG_PROP_BITS(0x10)`, `CFG_EV_BITS(0x11)`, `CFG_ABS_INFO(0x12)`
  - [ ] 1h: Define `VirtioInputAbsInfo` — repr(C): min(u32), max(u32), fuzz(u32), flat(u32), res(u32). Compile-time size assert (20B).
  - [ ] 1i: Define `InputDeviceId(pub u8)` — derives Debug, Clone, Copy, PartialEq, Eq
  - [ ] 1j: Add `Input = 14` to `Subsystem` enum in `shared/src/observability.rs`:
    - Update `COUNT` from 14 to 15
    - Add `Subsystem::Input => "Input"` to `name()` (exactly 5 chars)
    - Update `subsystem_count` test: assert COUNT==15, Input as u8 == 14
    - Update `subsystem_repr_values` test: add `Subsystem::Input as u8 == 14`
    - Update `subsystem_names_are_5_chars` test: add Input to array
    - Update `subsystem_name_content` test: add Input assertion
  - [ ] 1k: Add `VIRTIO_DEVICE_ID_INPUT: u32 = 18` to `shared/src/storage.rs` (next to existing VIRTIO_DEVICE_ID_BLK=2 and VIRTIO_DEVICE_ID_GPU=16)
  - [ ] 1l: Write host-side tests in `shared/src/input.rs`: VirtioInputEvent size=8B, VirtioInputAbsInfo size=20B, key code constants compile, config select constants
  - [ ] 1m: Verify: `just check` zero warnings, `just test` passes (existing 442+ tests + new input/observability tests)

- [ ] Step 2: VirtIO-input MMIO driver — probe and init
  - [ ] 2a: Create `kernel/src/drivers/virtio_input.rs`, add `pub mod virtio_input;` to `kernel/src/drivers/mod.rs`
  - [ ] 2b: Define `VirtioInputDevice` struct:
    ```
    base: usize (MMIO virtual addr = MMIO_BASE + phys)
    desc_virt: usize, avail_virt: usize, used_virt: usize (virtqueue layout)
    event_buf_phys: usize, event_buf_virt: usize (DMA page for event buffers)
    last_used_idx: u16
    queue_size: u16
    device_id: InputDeviceId
    name: [u8; 64], name_len: u8
    has_abs: bool, abs_max_x: u32, abs_max_y: u32
    ```
  - [ ] 2c: Define `MAX_INPUT_DEVICES: usize = 4` and `static INPUT_DEVICES: Mutex<[Option<VirtioInputDevice>; MAX_INPUT_DEVICES]>` — lock ordering: after BLOCK_ENGINE, before nothing (leaf lock)
  - [ ] 2d: Implement `init_all(dt: &DeviceTree) -> usize`:
    - Scan DTB `virtio_mmio_bases` first, then brute-force MMIO slots (0x0A00_0000, stride 0x200, 32 slots)
    - For each slot: check magic=0x74726976, version=1, device_id=18
    - Convert phys to virt: `MMIO_BASE + phys` (same pattern as virtio_gpu.rs)
    - Call `init_device(virt_base)` for each match; on `Err`, log `kerror!` and skip device (don't fail the whole scan)
    - Store in next free `INPUT_DEVICES` slot; if all 4 slots full, log `kwarn!` and stop scanning
    - Return count of successfully initialized devices
  - [ ] 2e: Implement `init_device(base: usize) -> Result<VirtioInputDevice, InputError>`:
    - Reset: write 0 to STATUS
    - ACKNOWLEDGE: status |= 1
    - DRIVER: status |= 2
    - Features: read DEVICE_FEATURES (offset 0x010), write 0 to DRIVER_FEATURES (no features needed)
    - GUEST_PAGE_SIZE: write 4096 to offset 0x028
    - Select eventq (queue 0): write 0 to QUEUE_SEL (0x030), dsb sy, read QUEUE_NUM_MAX (0x034)
    - Set QUEUE_NUM = min(QUEUE_NUM_MAX, 128), write QUEUE_ALIGN = 4096
    - Allocate DMA for virtqueue: `virtqueue_size(queue_size)` bytes → `alloc_dma_pages(order)`
    - Zero allocation, compute desc/avail/used offsets
    - Set QUEUE_PFN = phys / 4096
    - **Pre-fill eventq available ring** (see 2f)
    - Allocate statusq (queue 1) similarly but leave empty (Phase 8 LED control)
    - DRIVER_OK: status |= 4
    - Read device name and abs info (see 2g, 2h)
  - [ ] 2f: Eventq pre-fill strategy:
    - Allocate 1 DMA page (4KiB) for event buffers: 128 × 8B = 1024B fits in 1 page
    - For each descriptor i (0..queue_size):
      - desc[i].addr = event_buf_phys + i*8 (physical address of 8-byte VirtioInputEvent slot)
      - desc[i].len = 8
      - desc[i].flags = VIRTQ_DESC_F_WRITE (device writes into buffer)
      - desc[i].next = 0 (no chaining — each descriptor is standalone)
    - For each i (0..queue_size): avail_ring[i] = i (add all descriptors)
    - Set avail_idx = queue_size (all buffers available to device)
    - dsb sy + notify (QUEUE_NOTIFY = 0) to kick device
  - [ ] 2g: Implement `read_config_name(base: usize, name: &mut [u8; 64]) -> u8`:
    - Config space starts at `base + VIRTIO_MMIO_CONFIG_SPACE` (= `base + 0x100`)
    - VirtIO-input config layout (VirtIO spec §5.8.2):
      - offset +0x00: select (u8) — config query type
      - offset +0x01: subsel (u8) — config query subtype
      - offset +0x02: size (u8) — number of valid bytes in data union
      - offset +0x03..+0x07: reserved (5 bytes)
      - offset +0x08: data union start (string[128] / bitmap[128] / abs_info)
    - Write packed u32 to `base + 0x100`: `val = (0u8 << 8) | VIRTIO_INPUT_CFG_ID_NAME` (select=1, subsel=0)
    - dsb sy (let device process config change)
    - Read size: `mmio_read32(base + 0x100 + 0x00) >> 16 & 0xFF` — OR read byte at `base + 0x102` via u32 read
      - **Alternative (simpler)**: read u32 at `base + 0x100`, size is byte at offset 2 = `(val >> 16) & 0xFF`
      - NOTE: After writing select/subsel, re-reading offset 0x100 may return the same select/subsel in low bytes + size in byte 2
    - Read string bytes from `base + 0x108`: read u32 words, extract bytes, up to min(size, 64)
    - Return size
  - [ ] 2h: Implement `read_abs_info(base: usize, axis: u8) -> VirtioInputAbsInfo`:
    - Write packed u32: `(axis << 8) | VIRTIO_INPUT_CFG_ABS_INFO` to `base + 0x100`
    - dsb sy
    - Read size (should be 20 for abs_info): byte at `base + 0x102`; if size == 0, device doesn't have this axis — return default (min=0, max=0)
    - Read 5 u32 values from `base + 0x108`: min, max, fuzz, flat, res
    - Check if device supports ABS at all: query CFG_EV_BITS(subsel=EV_ABS); if size==0, set has_abs=false
  - [ ] 2i: Log to UART: `kinfo!(Input, "VirtIO-input: \"{}\" at {:#x}", name_str, phys_addr)` and abs info for tablet
  - [ ] 2j: Define `InputError` enum (or reuse StorageError pattern): `ProbeError`, `InitError`, `QueueError`, `OutOfMemory`
  - [ ] 2k: Verify: `just check`; with QEMU flags `-device virtio-keyboard-device -device virtio-tablet-device`, UART shows both devices probed with names and tablet abs info `min=0 max=32767`

- [ ] Step 3: VirtIO-input event polling
  - [ ] 3a: Implement `poll_events(dev: &mut VirtioInputDevice) -> Option<VirtioInputEvent>`:
    - **IMPORTANT: DMA memory vs MMIO distinction** — virtqueue descriptors, avail ring, and used ring are in DMA memory (accessed via `core::ptr::read_volatile`/`write_volatile` at DIRECT_MAP_BASE + phys). Only device registers (status, queue_sel, queue_notify, config) use `mmio_read32`/`mmio_write32` at MMIO_BASE + phys.
    - Read used ring idx: `core::ptr::read_volatile((used_virt + 2) as *const u16)` (DMA, not MMIO)
    - If `last_used_idx == used_idx`: return None (no new events)
    - Read used ring element at `used_virt + 4 + (last_used_idx % queue_size) * 8`:
      - id: `core::ptr::read_volatile((elem_addr) as *const u32)` — descriptor index
      - len: `core::ptr::read_volatile((elem_addr + 4) as *const u32)` — bytes written (should be 8)
    - Read VirtioInputEvent from `event_buf_virt + id * 8`:
      - Copy 8 bytes to local: `let mut event = VirtioInputEvent { event_type: 0, code: 0, value: 0 }; core::ptr::copy_nonoverlapping(buf_addr as *const u8, &mut event as *mut _ as *mut u8, 8);`
      - Or equivalently: `core::ptr::read_volatile(buf_addr as *const VirtioInputEvent)` — safe because event buffers are 8-byte aligned within the DMA page (i * 8)
      - Follows GPU driver pattern of reading repr(C) structs from DMA buffers
    - Increment `last_used_idx`
    - Recycle: add descriptor id back to available ring (see 3c)
    - Return Some(event)
  - [ ] 3b: Implement `poll_all_devices() -> Option<(InputDeviceId, VirtioInputEvent)>`:
    - Lock INPUT_DEVICES, iterate all Some entries
    - For each device, call poll_events() repeatedly until None
    - Return first event found (or cycle through all devices round-robin)
    - Note: SYN_REPORT grouping — collect events between SYN_REPORT boundaries into atomic groups
  - [ ] 3c: Buffer recycling detail (all ring accesses use `ptr::read_volatile`/`write_volatile` — DMA memory, not MMIO):
    - After reading used element with desc_id:
    - Read current avail_idx: `core::ptr::read_volatile((avail_virt + 2) as *const u16)`
    - Write desc_id to avail ring: `core::ptr::write_volatile((avail_virt + 4 + (avail_idx % queue_size) * 2) as *mut u16, desc_id as u16)`
    - dsb sy (ensure ring entry visible before idx update)
    - Increment avail_idx: `core::ptr::write_volatile((avail_virt + 2) as *mut u16, avail_idx.wrapping_add(1))`
    - dsb sy (ensure idx visible before device notify)
    - Notify device (MMIO register): `mmio_write32(base + VIRTIO_MMIO_QUEUE_NOTIFY, 0)`
    - dsb sy
  - [ ] 3d: Handle SYN_REPORT in raw polling layer:
    - The VirtIO driver layer (Step 3) returns raw VirtioInputEvent values including EV_SYN
    - SYN_REPORT grouping is handled in the translation layer (Step 4j), not in the driver
    - The driver's `poll_events()` returns ALL events including EV_SYN — the caller decides grouping
  - [ ] 3e: Verify: with QEMU, keyboard presses produce VirtioInputEvent with event_type=1 (EV_KEY) logged; tablet produces event_type=3 (EV_ABS)

- [ ] Step 4: Input event translation and keymap
  - [ ] 4a: Create `kernel/src/input/mod.rs`, add `pub mod input;` to `kernel/src/main.rs`
  - [ ] 4b: Define typed `InputEvent` enum in `shared/src/input.rs` (matches phase doc exactly):
    ```rust
    pub enum InputEvent {
        Keyboard { key: KeyCode, state: KeyState, modifiers: Modifiers },
        Pointer { x: u32, y: u32, button: Option<MouseButton>, state: Option<ButtonState> },
    }
    ```
  - [ ] 4c: Define `KeyCode` enum: A–Z, Num0–Num9, Enter, Esc, Backspace, Tab, Space, F1–F12, Up/Down/Left/Right, LeftShift/RightShift, LeftCtrl/RightCtrl, LeftAlt/RightAlt, LeftSuper/RightSuper, Minus, Equal, LeftBracket, RightBracket, Backslash, Semicolon, Apostrophe, Grave, Comma, Period, Slash, CapsLock, Delete, Home, End, PageUp, PageDown, Unknown(u16)
  - [ ] 4d: Define supporting types:
    - `KeyState`: Pressed, Released, Repeat (from evdev value: 0=Released, 1=Pressed, 2=Repeat)
    - `Modifiers(pub u8)` with consts: SHIFT=1, CTRL=2, ALT=4, SUPER=8, NONE=0
    - `MouseButton`: Left, Right, Middle
    - `ButtonState`: Pressed, Released
  - [ ] 4e: Implement `KeyCode::from_evdev(code: u16) -> KeyCode` — match statement mapping evdev keycodes to KeyCode variants, Unknown(code) for unmapped
  - [ ] 4f: Implement `const KEYMAP_US: [Option<(char, char)>; 128]`:
    - Index by evdev keycode, value = Some((unshifted, shifted)) or None
    - Examples: [30] = Some(('a','A')), [2] = Some(('1','!')), [57] = Some((' ',' '))
    - Provides ASCII character output for keyboard events
  - [ ] 4g: Implement modifier tracking in `kernel/src/input/mod.rs`:
    - `static MODIFIER_STATE: AtomicU8 = AtomicU8::new(0)` — no Mutex needed
    - On EV_KEY for modifier keys: set/clear appropriate bit based on value (1=set, 0=clear)
    - Read current state when constructing InputEvent::Keyboard
  - [ ] 4h: Implement abs→display coordinate conversion:
    - `fn abs_to_display(abs_val: u32, abs_max: u32, display_dim: u32) -> u32`
    - Formula: `abs_val * display_dim / (abs_max + 1)` (avoids off-by-one)
    - Display dimensions: call `drivers::virtio_gpu::display_info() -> Option<DisplayInfo>` (returns DisplayInfo{width, height, format}), fallback to 1280×800 if None
  - [ ] 4i: Define `static INPUT_QUEUE: Mutex<FixedQueue<InputEvent, 256>>` in `kernel/src/input/mod.rs`. On push failure (queue full), log `kwarn!(Input, "input queue full, dropping event")` and discard. For M23 demo, 256 entries is ample since events are drained every 16ms.
  - [ ] 4j: Implement `process_raw_event(device_id: InputDeviceId, raw: VirtioInputEvent)`:
    - Uses pending pointer state: `static PENDING_POINTER: Mutex<PendingPointer>` with fields `x: u32, y: u32, button: Option<MouseButton>, button_state: Option<ButtonState>, dirty: bool`
    - EV_KEY + code < BTN_LEFT → keyboard event: KeyCode::from_evdev, KeyState from value (0=Released, 1=Pressed, 2=Repeat), read MODIFIER_STATE, push InputEvent::Keyboard to INPUT_QUEUE immediately
    - EV_KEY + code >= BTN_LEFT → button event: set PENDING_POINTER.button/button_state, mark dirty
    - EV_ABS(ABS_X) → set PENDING_POINTER.x (converted to display coords), mark dirty
    - EV_ABS(ABS_Y) → set PENDING_POINTER.y (converted to display coords), mark dirty
    - EV_SYN(SYN_REPORT) → if PENDING_POINTER.dirty: push InputEvent::Pointer{x,y,button,state} to INPUT_QUEUE, clear dirty + button/state (position persists across reports)
    - Other event types → skip (log at Trace level)
    - **This is the correct evdev model**: accumulate partial events, emit combined Pointer on SYN_REPORT. Without this, ABS_X and ABS_Y would produce two separate incomplete Pointer events.
  - [ ] 4k: Verify: 'a' press → Keyboard{KeyA, Pressed, Modifiers(0)}; Shift+A → Modifiers(SHIFT); tablet → Pointer{x, y, None, None}

- [ ] Step 5: QEMU run target and input demo
  - [ ] 5a: Add `run-input` recipe to justfile — same as `run-gpu` plus:
    ```
    -device virtio-keyboard-device \
    -device virtio-tablet-device
    ```
    Must use `-serial stdio` (not `-nographic`) since QEMU window needed for input capture
  - [ ] 5b: Update `run-gpu` recipe: add `-device virtio-keyboard-device -device virtio-tablet-device` after `-device virtio-gpu-device`
  - [ ] 5c: Add `input::init(&dt)` call to `kernel/src/main.rs`:
    - Location: after GPU init block (after line ~318, before bench::init at line ~321)
    - Pattern: conditional like GPU init — `let input_count = drivers::virtio_input::init_all(&dt); if input_count > 0 { input::init(input_count); advance_boot_phase(InputReady); }` — graceful no-op if no input devices
    - **NOTE**: Step 6 (EarlyBootPhase) adds the `InputReady` variant to `shared/src/boot.rs`. Since `shared/` compiles first, the variant must exist before main.rs uses it. Implementation order: do Step 6 (boot.rs change) BEFORE Step 5c, or combine them.
  - [ ] 5d: Implement input polling — deferred flag approach:
    - In `kernel/src/input/mod.rs`: `static INPUT_POLL_DUE: AtomicBool = AtomicBool::new(false)`
    - In `timer_tick_handler` (timer.rs, CPU 0 section, after log drain): set `INPUT_POLL_DUE` when `tick % 16 == 0` — add `if tick.is_multiple_of(16) { crate::input::set_poll_due(); }` (lightweight, no lock, just an atomic store)
    - Actual polling: `input::poll_if_due()` checks flag, if set: lock INPUT_DEVICES, poll all devices, process raw events, push to INPUT_QUEUE, clear flag. Called from non-IRQ context only.
    - For M23 demo: call `input::poll_if_due()` from a dedicated input thread loop with `ipc::timeout::sleep_ticks(16)` (16ms) between iterations
  - [ ] 5e: Create input demo thread (follow `gpu/service.rs` pattern):
    - Allocate stack: `sched::alloc_kernel_stack()` → phys, convert to virt, compute stack_virt_top
    - Create thread: `Thread::new_kernel(ThreadId(0xA00), b"input-poll\0\0\0\0\0\0", input_poll_entry as *const () as usize, stack_phys)` — ThreadId(0xA00) is just a debug label, NOT the THREAD_TABLE index
    - Set `thread.sched.class = SchedulerClass::Interactive`, `affinity = CpuSet::all()`, `context.sp = stack_virt_top`
    - Allocate slot: `let idx = sched::allocate_thread(thread).expect("thread table full")` — returns table index (0-63)
    - Enqueue: `sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Interactive)` — uses the ACTUAL table index, not the debug label
    - **IMPORTANT**: THREAD_TABLE is indexed by `tid.0 as usize` (MAX=64). `allocate_thread()` finds the first free slot. ThreadId(0xA00) in new_kernel is just a label; the real tid used by the scheduler is the allocated index.
    - Thread entry function: `fn input_poll_entry() -> ! { loop { poll_if_due(); drain_and_log_events(); sleep_ticks(16); } }`
    - Log format: `kinfo!(Input, "Key: {:?} {:?}", key, state)` and `kinfo!(Input, "Pointer: x={} y={}", x, y)`
  - [ ] 5f: Verify: `just run-input` boots; click QEMU window to capture input; keyboard → `[Input] Key: A Pressed` in UART terminal; tablet → `[Input] Pointer: x=640 y=400`; Ctrl+Alt+G releases mouse grab

- [ ] Step 6: EarlyBootPhase update
  - [ ] 6a: Add variants to `EarlyBootPhase` in `shared/src/boot.rs`:
    - `InputReady = 18` (after GpuReady=17)
    - `CompositorReady = 19` (placeholder for M24)
    - `Complete = 20` (was 18, shifted +2)
  - [ ] 6b: Update `EARLY_BOOT_PHASE_COUNT` from 19 to 21
  - [ ] 6c: Update doc comment: "Total: 21 variants (EntryPoint=0 through Complete=20)"
  - [ ] 6d: Update unit tests in `shared/src/boot.rs`:
    - `early_boot_phase_count`: COUNT==21, InputReady as u32 == 18, CompositorReady as u32 == 19, Complete as u32 == 20
    - `early_boot_phase_ordering`: add `GpuReady < InputReady < CompositorReady < Complete`
    - `early_boot_phase_contiguous_values`: add InputReady and CompositorReady to phases array, update length assertion
  - [ ] 6e: Call `advance_boot_phase(EarlyBootPhase::InputReady)` in main.rs after input init succeeds
  - [ ] 6f: Note: `current_boot_phase()` in boot_phase.rs uses `transmute` bounded by `Complete as u32` — automatically works with new discriminant values since Complete is still the max variant
  - [ ] 6g: Verify: `just check` + `just test` pass; UART shows `[Boot] InputReady — Nms` phase transition

- [ ] Step 7: Shared crate input types and unit tests (20+ tests)
  - [ ] 7a: Add `#[cfg(test)] mod tests` to `shared/src/input.rs`
  - [ ] 7b: Wire-format size tests: `size_of::<VirtioInputEvent>() == 8`, `size_of::<VirtioInputAbsInfo>() == 20`
  - [ ] 7c: KeyCode round-trip: `KeyCode::from_evdev(30) == KeyCode::A`, `KeyCode::from_evdev(999) == KeyCode::Unknown(999)`, all defined keys round-trip
  - [ ] 7d: Modifiers bitflags: `SHIFT | CTRL` combines correctly, `contains()` checks, `Modifiers(0)` is empty
  - [ ] 7e: US-QWERTY keymap: `KEYMAP_US[30] == Some(('a','A'))`, `KEYMAP_US[2] == Some(('1','!'))`, `KEYMAP_US[57] == Some((' ',' '))`, `KEYMAP_US[0] == None` (reserved)
  - [ ] 7f: Coordinate conversion: `abs_to_display(0, 32767, 1280) == 0`, `abs_to_display(16384, 32767, 1280) == 640`, `abs_to_display(32767, 32767, 1280) == 1279`
  - [ ] 7g: InputEvent::Keyboard variant constructs and pattern-matches correctly
  - [ ] 7h: InputEvent::Pointer variant constructs with None button/state
  - [ ] 7i: KeyState from_value: 0→Released, 1→Pressed, 2→Repeat, other→None
  - [ ] 7j: MouseButton discrimination: Left, Right, Middle are distinct
  - [ ] 7k: ButtonState discrimination: Pressed ≠ Released
  - [ ] 7l: InputDeviceId: equality, Copy semantics, `InputDeviceId(0) != InputDeviceId(1)`
  - [ ] 7m: Config select constants: `CFG_ID_NAME == 0x01`, `CFG_ABS_INFO == 0x12`
  - [ ] 7n: VirtioInputEvent repr(C) field offsets: event_type at 0, code at 2, value at 4
  - [ ] 7o: Evdev constant values: `EV_KEY == 1`, `EV_ABS == 3`, `BTN_LEFT == 0x110`, `KEY_A == 30`
  - [ ] 7p: Target: 20+ tests. Current shared crate total: 442+, expected after: 462+
  - [ ] 7q: Verify: `just check` + `just test` with 20+ new input tests, all existing tests still pass

- [ ] Step 7+: Docs update and audit
  - [ ] Update CLAUDE.md Workspace Layout: add `kernel/src/input/` with mod.rs, add `shared/src/input.rs`
  - [ ] Update CLAUDE.md Key Technical Facts: add VirtIO-input device ID=18, MMIO slot scanning, event format, config space protocol, InputDeviceId, MAX_INPUT_DEVICES=4, INPUT_QUEUE capacity=256, EarlyBootPhase count=21
  - [ ] Update CLAUDE.md lock ordering: add INPUT_DEVICES position (leaf lock, after BLOCK_ENGINE)
  - [ ] Update README.md project structure and build commands (add `just run-input`)
  - [ ] Check off completed tasks in phase doc
  - [ ] Run `/audit-loop` — recursive triple audit until 0 issues

## Code Structure Decisions

- **`value: u32` in VirtioInputEvent**: VirtIO spec wire format is le32 (unsigned). Cast to i32 only during EV_REL translation for negative deltas. Tablet ABS values are always non-negative (0–32767).

- **Config space byte writes via packed u32**: Write `(subsel << 8) | select` as a single mmio_write32 to offset 0x100. Avoids adding byte-width MMIO helpers; works on little-endian (QEMU virt is always LE). Add dsb sy after write to ensure device sees the config change before we read the result.

- **Multi-device storage as fixed array**: `[Option<VirtioInputDevice>; 4]` behind Mutex. No alloc needed, predictable memory, matches no_std constraints. Lock ordering: INPUT_DEVICES is a leaf lock (after BLOCK_ENGINE in the hierarchy).

- **Two DMA allocations per device**: (1) virtqueue itself via `alloc_dma_pages(order)` — holds descriptor table + avail ring + used ring; (2) event buffers via `alloc_dma_page()` (singular, order 0, 1 page = 4KiB) — holds 128 × 8-byte VirtioInputEvent slots. Each descriptor points to its slot in the event buffer page with `VIRTQ_DESC_F_WRITE` flag. No descriptor chaining (standalone, unlike blk/gpu). Statusq gets a third allocation (virtqueue only, no event buffers — unused in M23).

- **Polling from non-IRQ context**: Timer tick sets `INPUT_POLL_DUE` AtomicBool every 16ms. Actual polling happens outside IRQ handler to avoid Mutex deadlock with INPUT_DEVICES. For M23 demo, call poll_if_due() from a dedicated kernel thread.

- **Modifier tracking via AtomicU8**: No Mutex needed — single byte, updated atomically on modifier key press/release. Thread-safe without contention.

- **Coordinate conversion formula**: `abs_val * display_dim / (abs_max + 1)`. When abs_max=32767, divides by 32768. Result: 32767×1280/32768 = 1279 (last pixel).

- **InputEvent matches phase doc exactly**: `Keyboard { key, state, modifiers }` — no extra `character` field. `Pointer { x, y, button: Option<MouseButton>, state: Option<ButtonState> }` — two separate Option fields per phase doc. Must derive `Debug, Clone, Copy, PartialEq` — `Copy` is required by `FixedQueue<T, N>`. All contained types (KeyCode, KeyState, Modifiers, MouseButton, ButtonState, Option wrappers) are trivially Copy.

- **SYN_REPORT handling**: Keyboard events are pushed immediately (no grouping needed — each key event is self-contained). Pointer events use accumulation: ABS_X/ABS_Y/button events update `PENDING_POINTER` state, and SYN_REPORT flushes the accumulated state as a single `InputEvent::Pointer`. This is the correct evdev model and avoids pushing partial pointer states.

- **statusq allocation**: Phase doc Step 2 says "allocate statusq but don't use it". Plan: select queue 1, read QUEUE_NUM_MAX, allocate DMA, set QUEUE_PFN, but don't pre-fill or notify. Placeholder for Phase 8 LED control.

- **InputError enum**: Kernel-only (in `kernel/src/drivers/virtio_input.rs`, not shared crate). Variants: ProbeError, InitError, QueueError, OutOfMemory. Matches GpuError pattern.

- **`abs_to_display` in shared crate**: Pure function (no hardware deps) → define in `shared/src/input.rs` so it can be unit-tested on host. Takes `(abs_val, abs_max, display_dim)`, returns display coordinate. Kernel calls it with display info from GPU driver.

- **Input demo thread**: ThreadId(0xA00) is a debug label only — the actual THREAD_TABLE index is allocated dynamically by `sched::allocate_thread()` which returns a small index (0-63). The thread is enqueued with `ThreadId(idx as u32)`. Created in `input::init()` before `sched::start()`. Uses `sleep_ticks(16)` for 16ms polling interval.

- **Lock nesting strategy for polling**: The poll_if_due() function must avoid holding INPUT_DEVICES while acquiring PENDING_POINTER or INPUT_QUEUE. Approach: (1) acquire INPUT_DEVICES, poll raw events into a local fixed buffer `[Option<(InputDeviceId, VirtioInputEvent)>; 64]`, release INPUT_DEVICES; (2) iterate buffer, call process_raw_event() for each (acquires PENDING_POINTER and INPUT_QUEUE separately). This avoids 3-level lock nesting. Lock ordering: INPUT_DEVICES is independent of PENDING_POINTER and INPUT_QUEUE.

- **Step execution order**: Phase doc lists Step 6 (EarlyBootPhase) after Step 5. But Step 5c uses `InputReady` variant from shared/src/boot.rs. In practice: implement Step 6's boot.rs changes first (or as part of Step 5), then use the variant in main.rs. This is a compile-order dependency, not a phase doc deviation.

- **Unsafe blocks**: Every `unsafe` block in the VirtIO-input driver and input subsystem requires a 3-part `// SAFETY:` comment per `.claude/rules/06-unsafe-documentation.md` (invariant, who maintains it, violation consequence). Expected unsafe blocks: MMIO reads/writes (config space), DMA volatile reads/writes (descriptors, avail/used rings, event buffers), dsb sy barriers, inline asm. Follow existing patterns in `virtio_gpu.rs`.

- **Device overflow**: If MMIO scan finds > MAX_INPUT_DEVICES(4) devices, log `kwarn!(Input, "max input devices reached, skipping")` and break the scan loop. Don't silently ignore.

- **`abs_to_display` overflow safety**: `u32 * u32` can overflow. `32767 * 100000 = 3.27B` fits in u32 (max 4.29B). For displays up to ~131K pixels wide, u32 is safe. No u64 cast needed for any realistic display size. Note in code comment.

## Phase Doc Reconciliation

- **EarlyBootPhase numbering**: Phase doc says InputReady=19, CompositorReady=20, Complete=21, COUNT=22. Sequential numbering from existing GpuReady=17 gives InputReady=18, CompositorReady=19, Complete=20, COUNT=21. Phase doc numbers would require a phantom variant at 18 which is non-standard for repr(u32) enums. **Plan: use sequential (18/19/20, COUNT=21) and update phase doc Step 6 during implementation.**

- **VIRTIO_DEVICE_ID_INPUT placement**: Existing pattern puts VirtIO device IDs in `shared/src/storage.rs` (VIRTIO_DEVICE_ID_BLK=2, VIRTIO_DEVICE_ID_GPU=16). **Plan: follow existing pattern, put in storage.rs.**

- **Subsystem name padding**: Existing pattern requires exactly 5 chars. "Input" is exactly 5 — no padding needed.

## Files Modified (Complete List)

| File | Action | Step |
|---|---|---|
| `shared/src/input.rs` | **Create** — wire-format types, evdev constants, InputEvent, KeyCode, keymap, unit tests | 1, 4, 7 |
| `shared/src/lib.rs` | **Edit** — add `pub mod input;` and re-exports | 1 |
| `shared/src/observability.rs` | **Edit** — add Input=14 variant, update COUNT, name(), tests | 1 |
| `shared/src/storage.rs` | **Edit** — add VIRTIO_DEVICE_ID_INPUT=18 | 1 |
| `shared/src/boot.rs` | **Edit** — add InputReady/CompositorReady, shift Complete, update tests | 6 |
| `kernel/src/drivers/virtio_input.rs` | **Create** — VirtIO-input MMIO driver | 2, 3 |
| `kernel/src/drivers/mod.rs` | **Edit** — add `pub mod virtio_input;` | 2 |
| `kernel/src/input/mod.rs` | **Create** — input subsystem, event translation, polling | 4, 5 |
| `kernel/src/main.rs` | **Edit** — add `mod input;`, input init call, boot phase advance | 4, 5, 6 |
| `justfile` | **Edit** — add run-input recipe, update run-gpu | 5 |

## Reusable Functions & Patterns

| Function / Constant | Location | Used For |
|---|---|---|
| `mmio_read32` / `mmio_write32` | `kernel/src/drivers/virtio_common.rs` | All MMIO register access |
| `avail_offset` / `used_offset` / `virtqueue_size` | `kernel/src/drivers/virtio_common.rs` | Virtqueue DMA layout |
| `QUEUE_SIZE` (128), `POLL_TIMEOUT`, `VIRT_PAGE_SIZE` | `kernel/src/drivers/virtio_common.rs` | Queue sizing |
| `alloc_dma_page` (singular, order 0) | `kernel/src/mm/frame.rs` | Event buffer page (1 per device) |
| `alloc_dma_pages` / `free_dma_pages` | `kernel/src/mm/frame.rs` | Virtqueue DMA allocation |
| `order_for_pages` | `shared/src/memory.rs` | Buddy order calculation |
| `DIRECT_MAP_BASE`, `MMIO_BASE` | `kernel/src/mm/kmap.rs` | Phys→virt address conversion |
| `VirtqDesc`, `VIRTQ_DESC_F_WRITE` | `shared/src/storage.rs` | Descriptor setup |
| All VIRTIO_MMIO_* register offsets | `shared/src/storage.rs` | MMIO register addresses |
| `FixedQueue<T, N>` | `shared/src/collections.rs` | INPUT_QUEUE bounded queue |
| `kinfo!` / `kwarn!` / `kerror!` | `kernel/src/observability/mod.rs` | Structured logging |
| `advance_boot_phase` | `kernel/src/boot_phase.rs` | Boot phase transitions |
| GPU init pattern (`init`, `probe`, `probe_slot`) | `kernel/src/drivers/virtio_gpu.rs` | Template for input driver |
| `Thread::new_kernel(id, name, entry, stack)` | `kernel/src/task/mod.rs` | Input demo thread creation (id is debug label) |
| `sched::allocate_thread(thread) -> Option<usize>` | `kernel/src/sched/mod.rs` | Allocate THREAD_TABLE slot (returns actual idx) |
| `sched::alloc_kernel_stack()` | `kernel/src/sched/mod.rs` | Stack allocation for thread |
| `sched::phys_to_virt(phys)` | `kernel/src/sched/mod.rs` | Convert stack phys → virt for SP |
| `sched::enqueue_on_cpu(cpu, tid, class)` | `kernel/src/sched/mod.rs` | Enqueue thread on scheduler |
| `ipc::timeout::sleep_ticks(n)` | `kernel/src/ipc/timeout.rs` | Sleep 16ms between poll cycles |
| `drivers::virtio_gpu::display_info()` | `kernel/src/drivers/virtio_gpu.rs` | Display dimensions for coordinate conversion |

## Dependencies & Risks

**Dependencies (all confirmed present):**

- Phase 6 complete (GPU Service, VirtIO-GPU driver, Compute Kit Tier 1)
- `alloc_dma_pages()` in `kernel/src/mm/frame.rs` — DMA page allocation from Pool::Dma
- `DIRECT_MAP_BASE` / `MMIO_BASE` in `kernel/src/mm/kmap.rs` — phys→virt mapping
- `FixedQueue<T, N>` in `shared/src/collections.rs` — bounded queue for INPUT_QUEUE
- `VirtqDesc`, `VIRTQ_DESC_F_WRITE`, MMIO register offsets in `shared/src/storage.rs`
- `mmio_read32`/`mmio_write32`, virtqueue layout helpers in `kernel/src/drivers/virtio_common.rs`
- `DeviceTree` with `virtio_mmio_bases` in `kernel/src/dtb.rs`

**Risks:**

- **IRQ deadlock**: If input polling runs inside timer IRQ handler while another context holds INPUT_DEVICES Mutex → deadlock. **Mitigation**: Never poll inside IRQ. Use deferred flag approach (AtomicBool set by timer, polled in non-IRQ context).
- **Config space read timing**: VirtIO-input config space requires device to process `select/subsel` write before `size` read is valid. **Mitigation**: dsb sy barrier between write and read.
- **Eventq buffer starvation**: If buffers aren't recycled to available ring, device stops sending events silently. **Mitigation**: Always recycle immediately after reading from used ring; pre-fill all queue_size slots.
- **MMIO slot collision**: Input devices occupy different MMIO slots than blk/gpu. Brute-force scan filters by device_id=18. Confirmed safe.
- **Queue size mismatch**: QEMU may report QUEUE_NUM_MAX < 128. **Mitigation**: Use `min(device_max, 128)`, allocate DMA proportionally.
- **VirtIO legacy v1 only**: Existing infrastructure only supports legacy MMIO transport. VirtIO-input on QEMU uses legacy, so this is fine.
- **Lock nesting deadlock**: If poll_if_due() holds INPUT_DEVICES while process_raw_event() acquires PENDING_POINTER and INPUT_QUEUE → 3-level nesting risk. **Mitigation**: Buffer raw events locally while holding INPUT_DEVICES, then release before processing (see Code Structure Decisions).
- **THREAD_TABLE overflow**: MAX_THREADS=64. Adding one more thread (input-poll) to existing threads (idle×4, gpu-service, bench×3, test threads). Well within limit. `allocate_thread()` returns None if full — handle with `expect("thread table full")`.

## Verification

End-to-end verification after all steps complete:

1. `just check` — zero warnings (fmt, clippy, build)
2. `just test` — all host-side tests pass (462+ expected: 442 existing + 20+ new input tests)
3. `just run-input` — QEMU boots with:
   - UART: `[Input] VirtIO-input: "QEMU Virtio Keyboard" at 0x...`
   - UART: `[Input] VirtIO-input: "QEMU Virtio Tablet" at 0x... abs: min=0 max=32767`
   - UART: `[Boot] InputReady — Nms`
   - Keyboard presses → `[Input] Key: A Pressed`
   - Tablet movement → `[Input] Pointer: x=640 y=400`
4. `just run` (headless) — still boots correctly without input devices (graceful no-op)

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
