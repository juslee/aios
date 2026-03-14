# AIOS Input Event Model & Dispatch

Part of: [input.md](../input.md) — Input Subsystem
**Related:** [devices.md](./devices.md) — Hardware drivers producing events, [gestures.md](./gestures.md) — Gesture recognition consuming events, [integration.md](./integration.md) — Focus routing and capability enforcement

-----

## 4. Event Model & Dispatch

The input subsystem uses a **layered event model**: evdev-compatible raw events at the driver boundary, translated to strongly-typed Rust events at the subsystem service boundary. This provides hardware compatibility at the bottom and type-safe ergonomics at the top.

### 4.1 InputEvent Type Hierarchy

#### 4.1.1 Raw Events (Driver Boundary)

At the driver layer, all input devices produce evdev-compatible events. This format is shared with VirtIO-input and USB HID, providing a universal hardware interface.

```rust
/// Raw input event — evdev-compatible format used at the driver boundary.
/// Matches Linux struct input_event and VirtIO-input event format.
#[repr(C)]
pub struct RawInputEvent {
    /// Hardware timestamp: CNTPCT_EL0 ticks at IRQ time
    pub timestamp: u64,
    /// Source device identifier
    pub device_id: DeviceId,
    /// Event type (EV_KEY, EV_REL, EV_ABS, EV_SYN, etc.)
    pub event_type: u16,
    /// Event code (keycode, axis code, etc.)
    pub code: u16,
    /// Value (key state 0/1/2, axis delta, abs position)
    pub value: i32,
    /// Flags: INJECTED, SYNTHETIC, FROM_ACCESSIBILITY
    pub flags: EventFlags,
}

/// Event type constants — evdev-compatible
pub mod event_type {
    pub const EV_SYN: u16  = 0x00;  // Synchronization / group boundary
    pub const EV_KEY: u16  = 0x01;  // Key/button state change
    pub const EV_REL: u16  = 0x02;  // Relative axis (mouse delta)
    pub const EV_ABS: u16  = 0x03;  // Absolute axis (touch position)
    pub const EV_MSC: u16  = 0x04;  // Miscellaneous (scancode, timestamp)
    pub const EV_SW: u16   = 0x05;  // Switch (lid open/close, headphone)
    pub const EV_LED: u16  = 0x11;  // LED state change
    pub const EV_FF: u16   = 0x15;  // Force feedback
}

/// Synchronization event codes
pub mod syn_code {
    pub const SYN_REPORT: u16    = 0;  // End of atomic event group
    pub const SYN_DROPPED: u16   = 3;  // Buffer overrun — client must resync
}

bitflags! {
    pub struct EventFlags: u16 {
        /// Event was injected by software, not hardware
        const INJECTED          = 1 << 0;
        /// Event was synthesized (e.g., gesture recognition output)
        const SYNTHETIC         = 1 << 1;
        /// Event came from an accessibility service
        const FROM_ACCESSIBILITY = 1 << 2;
        /// Event is part of a secure input session (password entry)
        const SECURE            = 1 << 3;
    }
}
```

#### 4.1.2 Typed Events (Application Boundary)

The input subsystem service translates raw events into strongly-typed Rust enums before delivery to the compositor and applications:

```rust
/// Typed input event — delivered to applications via IPC.
/// Provides type-safe, exhaustive matching over all input event types.
pub enum InputEvent {
    Key(KeyEvent),
    Motion(MotionEvent),
    Touch(TouchEvent),
    Gamepad(GamepadEvent),
    Gesture(GestureEvent),
    Text(TextEvent),
    Switch(SwitchInputEvent),
    Gaze(GazeEvent),
}

/// Keyboard / button event
pub struct KeyEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    pub key: KeyCode,         // XKB keysym (post-layout-transform)
    pub scancode: u16,        // raw hardware scancode (pre-layout)
    pub state: KeyState,      // Pressed, Released, Repeat
    pub modifiers: Modifiers, // Shift, Ctrl, Alt, Super (current state)
    pub flags: EventFlags,
}

pub enum KeyState {
    Pressed,
    Released,
    Repeat,
}

bitflags! {
    pub struct Modifiers: u16 {
        const SHIFT     = 1 << 0;
        const CTRL      = 1 << 1;
        const ALT       = 1 << 2;
        const SUPER     = 1 << 3;
        const CAPS_LOCK = 1 << 4;
        const NUM_LOCK  = 1 << 5;
    }
}

/// Mouse / trackpad / trackball motion event
pub struct MotionEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    pub x: f32,              // pointer X (screen coordinates, post-acceleration)
    pub y: f32,              // pointer Y (screen coordinates, post-acceleration)
    pub dx: f32,             // delta X (raw, pre-acceleration, for games)
    pub dy: f32,             // delta Y (raw, pre-acceleration, for games)
    pub scroll_x: f32,       // horizontal scroll (pixels)
    pub scroll_y: f32,       // vertical scroll (pixels)
    pub buttons: MouseButtons,
    pub flags: EventFlags,
}

bitflags! {
    pub struct MouseButtons: u8 {
        const LEFT   = 1 << 0;
        const RIGHT  = 1 << 1;
        const MIDDLE = 1 << 2;
        const BACK   = 1 << 3;
        const FORWARD = 1 << 4;
    }
}

/// Multi-touch event (one per touch point)
pub struct TouchEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    pub touch_id: u16,       // tracking ID for this finger/contact
    pub phase: TouchPhase,
    pub x: f32,              // contact X (screen coordinates)
    pub y: f32,              // contact Y (screen coordinates)
    pub pressure: f32,       // 0.0–1.0 (if supported)
    pub contact_width: f32,  // contact ellipse width (for palm rejection)
    pub contact_height: f32, // contact ellipse height
    pub flags: EventFlags,
}

pub enum TouchPhase {
    Begin,     // new contact
    Move,      // existing contact moved
    End,       // contact lifted
    Cancel,    // contact cancelled (palm rejection, gesture override)
}

/// Gamepad event
pub struct GamepadEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    pub element: GamepadElement,
    pub flags: EventFlags,
}

pub enum GamepadElement {
    /// Analog stick axis: value in -1.0..1.0 (post-deadzone)
    Axis { axis: GamepadAxis, value: f32 },
    /// Analog trigger: value in 0.0..1.0
    Trigger { trigger: GamepadTrigger, value: f32 },
    /// Digital button press/release
    Button { button: GamepadButton, pressed: bool },
    /// D-pad direction
    DPad { direction: DPadDirection },
}

/// Recognized gesture event (from gesture recognition pipeline)
pub struct GestureEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    pub gesture: GestureKind,
    pub phase: GesturePhase,
    pub flags: EventFlags,
}

pub enum GesturePhase {
    Begin,
    Update,
    End,
    Cancel,
}

/// Composed text event (from IME or compose engine)
pub struct TextEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    pub kind: TextEventKind,
    pub flags: EventFlags,
}

pub enum TextEventKind {
    /// Pre-edit string (IME composition in progress)
    PreEdit {
        text: FixedString<256>,
        cursor: u16,
        segments: FixedVec<PreEditSegment, 8>,
    },
    /// Committed text (IME or compose sequence complete)
    Commit { text: FixedString<256> },
    /// Pre-edit cancelled
    PreEditClear,
}

/// Eye gaze event (from calibrated eye tracker)
pub struct GazeEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    pub x: f32,               // gaze X (screen coordinates)
    pub y: f32,               // gaze Y (screen coordinates)
    pub confidence: f32,      // 0.0–1.0 tracking confidence
    pub fixation_ms: u32,     // duration of current fixation
    pub pupil_diameter: f32,  // mm (if available)
    pub flags: EventFlags,
}

/// Switch device event (for switch scanning)
pub struct SwitchInputEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    pub switch: SwitchId,     // Primary or Secondary
    pub state: SwitchState,   // Activated or Released
    pub flags: EventFlags,
}
```

#### 4.1.3 Translation Layer

The translation from `RawInputEvent` to `InputEvent` happens in the input subsystem service, after the transform pipeline has processed raw events:

```text
RawInputEvent (evdev)                    InputEvent (typed)
─────────────────────                    ──────────────────
EV_KEY + keycode + state     ──────►     KeyEvent { key, state, modifiers }
                                         (after layout transform + modifier tracking)

EV_REL + REL_X/Y + delta    ──────►     MotionEvent { x, y, dx, dy }
                                         (after acceleration curve)

EV_ABS + ABS_MT_* + value   ──────►     TouchEvent { touch_id, phase, x, y }
                                         (after contact tracking + palm rejection)

EV_ABS + ABS_X/Y (gamepad)  ──────►     GamepadEvent { Axis, value }
                                         (after deadzone + response curve)

[gesture recognizer output]  ──────►     GestureEvent { kind, phase }

[IME / compose output]      ──────►     TextEvent { PreEdit | Commit }
```

### 4.2 Event Pipeline

The event pipeline processes every input event through a chain of transforms. Each transform is a capability-scoped stage that can consume, modify, or pass through events.

```text
                    Hardware IRQ
                         │
                         ▼
              ┌─────────────────────┐
              │  Driver (raw events) │
              │  evdev-compatible    │
              └──────────┬──────────┘
                         │ RawInputEvent
                         ▼
    ┌────────────────────────────────────────────────┐
    │              Transform Pipeline                 │
    │                                                 │
    │  Stage 1: Calibration / Deadzone               │
    │  ├── Analog axis calibration                    │
    │  ├── Gamepad deadzone application               │
    │  └── Touch coordinate mapping                   │
    │                                                 │
    │  Stage 2: Device-Specific Filtering             │
    │  ├── Palm rejection (CNN, touch only)           │
    │  ├── Device quirks (fixup malformed events)     │
    │  └── Contact tracking (multi-touch slot mgmt)   │
    │                                                 │
    │  Stage 3: Keyboard Processing                   │
    │  ├── Layout transform (XKB: scancode → keysym)  │
    │  ├── Compose / dead key engine                  │
    │  ├── Modifier state tracking                    │
    │  └── Key repeat generation                      │
    │                                                 │
    │  Stage 4: Pointer Processing                    │
    │  ├── Acceleration curve (adaptive sigmoid)      │
    │  ├── Scroll processing (discrete / smooth)      │
    │  └── Coordinate system transform                │
    │                                                 │
    │  Stage 5: Gesture Recognition                   │
    │  ├── $P+ geometric matcher                      │
    │  ├── Gesture state machine (multi-touch)        │
    │  └── TCN backbone (custom gestures)             │
    │                                                 │
    │  Stage 6: Accessibility Transforms              │
    │  ├── Tremor filter (Kalman)                     │
    │  ├── StickyKeys / FilterKeys / BounceKeys       │
    │  ├── Switch scanning engine                     │
    │  ├── Gaze-to-selection (dwell / smooth pursuit) │
    │  └── Adaptive debounce (HMM)                    │
    │                                                 │
    │  Stage 7: Input Method                          │
    │  ├── IME agent channel (CJK, predictive)        │
    │  └── Pre-edit / commit delivery                 │
    │                                                 │
    │  Stage 8: Intelligence                          │
    │  ├── AIRS prediction request (when available)   │
    │  ├── Anomaly detection (kernel-internal)        │
    │  └── Behavioral feature extraction              │
    │                                                 │
    └────────────────────┬───────────────────────────┘
                         │ InputEvent (typed)
                         ▼
              ┌─────────────────────┐
              │  Event Dispatcher    │
              │  → Compositor        │
              │  → POSIX bridge      │
              │  → Audit logger      │
              └─────────────────────┘
```

#### Pipeline Configuration

The pipeline is configured per-device-class. Not all stages apply to all devices:

| Stage | Keyboard | Mouse | Touch | Gamepad | Switch | Eye |
|---|---|---|---|---|---|---|
| Calibration | — | — | Coord map | Deadzone | — | Coord map |
| Device filtering | — | Quirks | Palm reject | Quirks | — | Confidence |
| Keyboard processing | Layout, compose, repeat | — | — | — | — | — |
| Pointer processing | — | Accel, scroll | — | — | — | — |
| Gesture recognition | — | Multi-finger | Tap/swipe/pinch | — | — | — |
| Accessibility | Sticky/Filter/Bounce | Tremor | — | — | Scan engine | Dwell/pursuit |
| Input method | IME | — | IME (on-screen) | — | — | — |
| Intelligence | Predict, anomaly | Anomaly | Predict | — | Predict | — |

### 4.3 Event Queuing & Priority

Input events are queued in per-device shared-memory ring buffers. The subsystem service processes events by priority tier:

| Priority | Tier | Latency Target | Use Case |
|---|---|---|---|
| 0 (highest) | Realtime | <1ms processing | Game input, musical instruments |
| 1 | Interactive | <4ms processing | Normal keyboard/mouse, UI interaction |
| 2 | Background | <16ms processing | Input recording, analytics, non-focused windows |

Priority is determined by the consuming agent's scheduling class:

- **RT-class agents** (games, instruments) receive events via Realtime tier
- **Interactive-class agents** (most applications) receive via Interactive tier
- **Normal-class agents** (background processes) receive via Background tier

#### Event Batching

High-frequency devices (1000Hz mice, multi-touch panels) can generate hundreds of events per frame. The subsystem batches events for non-Realtime consumers:

- **Realtime tier:** Individual event delivery, zero batching
- **Interactive tier:** Batch at frame boundary (16ms at 60Hz, 8ms at 120Hz)
- **Background tier:** Batch at 100ms intervals

Batching coalesces redundant motion events — only the final position in each batch is delivered, with the delta accumulated. This reduces IPC overhead without losing information.

#### SYN_DROPPED Recovery

If a consumer falls behind and the ring buffer overflows, the subsystem sends `SYN_DROPPED`. The consumer must then:

1. Discard any partially accumulated event group
2. Query the current device state (key state bitmap, pointer position)
3. Resume processing from the next `SYN_REPORT`

This follows Linux evdev semantics for compatibility.

### 4.4 Focus Routing

The compositor owns input focus. The input subsystem delivers events to the compositor, which routes them to the focused surface.

#### Focus Types

| Focus Type | Scope | Routing Rule |
|---|---|---|
| **Keyboard focus** | Per-seat, one surface | Keyboard events → focused surface's agent |
| **Pointer focus** | Per-seat, one surface | Pointer events → surface under cursor (hit-test) |
| **Touch focus** | Per-touch-point | Touch events → surface where touch began (sticky) |
| **Gamepad focus** | Per-device | Gamepad events → last-focused gaming surface |

#### Routing Flow

```text
InputEvent arrives at compositor
    │
    ├── Is this a global hotkey? ──► Yes: handle in compositor (§4.5)
    │
    ├── Is this a system gesture? ──► Yes: handle in compositor (workspace switch)
    │
    ├── Key event?
    │   └── Route to keyboard-focused surface for this seat
    │
    ├── Motion event?
    │   └── Hit-test: find surface under pointer
    │       ├── Surface found → route to that surface's agent
    │       └── No surface → route to compositor (desktop background)
    │
    ├── Touch event?
    │   ├── TouchPhase::Begin → hit-test, assign touch to surface (sticky)
    │   ├── TouchPhase::Move/End → route to assigned surface
    │   └── TouchPhase::Cancel → notify assigned surface
    │
    └── Gamepad event?
        └── Route to gamepad-focused surface
```

#### Focus Change

When focus changes (user clicks a different window, or Tab switches focus):

1. Compositor sends `FocusLost` to the old surface
2. Compositor updates focus state for the seat
3. Compositor sends `FocusGained` to the new surface
4. Any pending keyboard events are flushed (no events "leak" across focus boundaries)
5. IME is notified of focus change (may switch input method, clear pre-edit)

### 4.5 Global Hotkeys & System Shortcuts

Some keyboard combinations are handled by the compositor before reaching applications:

#### Secure Attention Sequence

The **Secure Attention Sequence** (SAS) is handled at the lowest possible level — the input subsystem intercepts it before any transform or routing:

- **QEMU:** Ctrl+Alt+Del (configurable)
- **Hardware:** Power button hold (handled by firmware/power management)

The SAS cannot be intercepted, suppressed, or spoofed by any agent or transform. It triggers the secure attention handler in the security subsystem, which presents a trusted dialog for login, lock, or shutdown.

#### System Hotkeys

| Hotkey | Action | Intercepted By |
|---|---|---|
| SAS (Ctrl+Alt+Del) | Secure attention | Input subsystem (pre-pipeline) |
| Super key | App launcher / search | Compositor |
| Alt+Tab | Window/agent switcher | Compositor |
| Ctrl+Alt+Arrow | Workspace switch | Compositor |
| Super+L | Lock screen | Compositor → security subsystem |
| Volume keys (media) | Volume control | Compositor → audio subsystem |
| Brightness keys | Display brightness | Compositor → power management |
| Print Screen | Screenshot | Compositor → screenshot agent |

Applications can register custom hotkeys via the compositor's hotkey API, but system hotkeys always take priority.

#### Hotkey Priority

```text
1. Secure Attention (input subsystem — cannot be overridden)
2. System hotkeys (compositor — can be reconfigured by user)
3. Application hotkeys (registered per-surface — can be overridden by system)
4. Normal key events (delivered to focused surface)
```

### 4.6 Multi-Seat Support

A **seat** is a logical grouping of input and output devices assigned to one user session. AIOS supports multiple simultaneous seats, each with independent focus, cursor, and input context.

#### Seat Architecture

```text
Seat 0 (primary)              Seat 1 (secondary)
├── USB keyboard A             ├── USB keyboard B
├── USB mouse A                ├── USB mouse B
├── Monitor (HDMI-1)           ├── Monitor (HDMI-2)
├── Keyboard focus: Terminal   ├── Keyboard focus: Browser
├── Pointer position: (400,300)├── Pointer position: (200,500)
└── Cursor: default            └── Cursor: text
```

#### Capability Model

Each seat is a **capability domain**:

```rust
/// Seat capability — grants access to a seat's input devices
pub struct SeatCapability {
    pub seat_id: SeatId,
    pub permissions: SeatPermissions,
}

bitflags! {
    pub struct SeatPermissions: u16 {
        /// Receive input events from this seat
        const RECEIVE    = 1 << 0;
        /// Control focus for this seat
        const FOCUS      = 1 << 1;
        /// Assign devices to this seat
        const ASSIGN     = 1 << 2;
        /// Create new seats
        const CREATE     = 1 << 3;
    }
}
```

- Applications receive input only from their assigned seat
- An application on Seat 0 cannot observe Seat 1's input
- The compositor holds `FOCUS` permission for all seats
- Device assignment (`ASSIGN`) is a privileged operation (session manager only)
- Seat creation (`CREATE`) is reserved for the session manager

#### Virtual Seats

AIRS agents can be assigned their own **virtual seat** for synthetic input injection:

- Virtual seats have no physical devices
- Events are produced programmatically via `InputInject` capability
- Virtual seat events are flagged as `INJECTED` and `SYNTHETIC`
- Applications can query whether input came from a physical or virtual seat

#### Use Cases

| Scenario | Configuration |
|---|---|
| Single user, one display | One seat with all devices |
| Multi-user (library kiosk) | Multiple seats, one per station |
| Accessibility (switch + keyboard) | Single seat with multiple input paths |
| AI agent automation | Virtual seat with InputInject capability |
| Multi-player local gaming | Gamepad per seat, shared display |
