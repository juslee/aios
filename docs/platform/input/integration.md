# AIOS Input System Integration

Part of: [input.md](../input.md) — Input Subsystem
**Related:** [events.md](./events.md) — Event model, [devices.md](./devices.md) — Hardware drivers, [gestures.md](./gestures.md) — Gesture recognition, [ai.md](./ai.md) — AI-native features

-----

## 6. System Integration

The input subsystem integrates with four major system concerns: the capability system (access control), the POSIX bridge (legacy compatibility), power management (device lifecycle), and observability (audit without surveillance). Two additional integration points — the compositor and the UI toolkit — define how input reaches applications.

These six integration surfaces are the contract between the input subsystem and the rest of AIOS. The input-specific design decisions are covered in sections 3–5. This section covers how those decisions connect outward.

-----

### 6.1 Capability System

Input access is controlled through a dedicated `InputCapability` enum that extends the system capability model. Every stage of the input pipeline is a capability boundary.

#### 6.1.1 Capability Hierarchy

```rust
pub enum InputCapability {
    /// Raw device access — drivers only
    DeviceAccess { device_class: DeviceClass },
    /// Receive input events for own surfaces (all surface-owning agents)
    Receive { seat_id: SeatId, viewport: Option<Rect> },
    /// Inject synthetic input events (system agents, accessibility)
    Inject { scope: InjectScope },
    /// Observe input globally (screen readers — audited, user-authorized)
    Observe { filter: EventFilter },
    /// Secure input session (password entry — excludes observers)
    SecureSession,
    /// Configure input parameters (acceleration, layout, accessibility)
    Configure { device_id: DeviceId },
    /// Receive gesture events (subset of Receive — restricted to gestures only)
    GestureRecognize { seat_id: SeatId },
    /// Define custom gestures
    GestureDefine,
    /// Force feedback / haptics control
    ForceFeedback { device_id: DeviceId },
}
```

The hierarchy from most to least privileged:

```text
DeviceAccess          (kernel drivers only — not grantable to userspace agents)
    │
    ▼
Observe               (screen readers — explicit user authorization required)
    │
    ▼
Inject                (accessibility services, test frameworks)
    │
    ▼
Receive               (all surface-owning agents — default grant)
    │
    ▼
Configure             (per-device configuration — user-level)
GestureDefine         (register custom gestures — user-level)
ForceFeedback         (haptics control — per-device)
SecureSession         (password mode — no hierarchy, orthogonal)
```

`InputCapability` is an extension of `shared::cap::Capability`. The kernel capability table stores `InputCapability` variants as capability tokens, subject to the same lifecycle (grant, attenuation, revocation, expiry) defined in [model/capabilities.md](../../security/model/capabilities.md) §3.

#### 6.1.2 Attenuation

Capabilities attenuate as they are delegated. A parent agent cannot grant a child more than it holds.

The primary attenuation dimension for input is **scope restriction**. Consider a game embedded inside a browser:

```text
Browser agent
  holds: Receive { seat_id: Seat(0) }
  (receives input for its entire window)
      │
      │ grants attenuated capability to embedded game
      ▼
Game agent
  holds: Receive { seat_id: Seat(0), viewport: Rect(400, 300, 800, 600) }
  (receives input only within its viewport rectangle)
```

The game cannot receive input that occurred outside its viewport, even though both agents share Seat 0. The compositor enforces the viewport clip during hit-testing.

Similarly, `Inject` scope is always bounded:

- **Per-window inject:** the injected events target one specific surface
- **Per-application inject:** the injected events target all surfaces of one agent
- **Per-seat inject:** the injected events enter the full event pipeline for one seat

Global injection — events that bypass focus and routing — is not a grantable scope. There is no capability that allows an agent to inject input into arbitrary surfaces it does not own.

#### 6.1.3 Observer Capability

`Observe` is the most sensitive input capability. It allows an agent to receive input events that are not addressed to its surfaces. Screen readers require this to monitor what the user types and clicks anywhere on screen.

Granting `Observe` requires:

1. Explicit user authorization through the security dashboard ([inspector.md](../../applications/inspector.md))
2. Capability token with expiry (default: session lifetime, re-authorized on next login)
3. Every use logged to `system/audit/input/` with agent ID and filter

The `EventFilter` parameter on `Observe` restricts which events are visible. A screen reader that only needs keyboard events for speech synthesis does not receive pointer events. An accessibility agent monitoring gaze does not receive keyboard events.

```rust
pub struct EventFilter {
    /// Which event types to pass through (EV_KEY, EV_REL, EV_ABS, etc.)
    pub event_types: EventTypeMask,
    /// Which device classes to include
    pub device_classes: DeviceClassMask,
    /// Only observe events from these specific surfaces (empty = all)
    pub surfaces: FixedVec<SurfaceId, 16>,
}
```

#### 6.1.4 Secure Input Sessions

When an agent opens a secure input session (for password entry, PIN input, or biometric verification), the compositor enters a mode where `Observe` capability holders are excluded from the event stream for that surface.

```text
Normal mode:
  Keyboard event → compositor → focused surface
                             → Observe agents (screen readers etc.)

Secure session mode (password field active):
  Keyboard event → compositor → focused surface (SecureSession holder only)
                             ✗  Observe agents (excluded for this session)
```

The `SecureSession` capability is not hierarchically superior to `Observe` — it is orthogonal. `SecureSession` modifies the delivery rules for the holder's own surface. The compositor is responsible for enforcing the exclusion of observers during secure sessions.

Secure session events carry the `EventFlags::SECURE` flag in the raw event stream, so the audit logger can record that a secure input session was active without recording the content.

-----

### 6.2 POSIX Bridge

The POSIX bridge exposes input devices as files in `/dev/input/`, following the Linux evdev interface. This allows existing Linux applications, input libraries (libinput, SDL, SFML), and tools to run without modification.

The bridge follows the general device node pattern defined in [posix.md](../posix.md) §9 Devices. This section covers the input-specific details.

#### 6.2.1 Device Node Layout

Each input device registered with the input subsystem gets a corresponding device node:

```text
/dev/input/
├── event0          ← first keyboard
├── event1          ← first mouse / trackpad
├── event2          ← first touchscreen
├── event3          ← first gamepad
├── event4          ← additional devices (dynamically assigned)
├── mice            ← merged mouse interface (PS/2 protocol, legacy compat)
└── js0             ← first joystick / gamepad (legacy joystick API)
```

Device node numbers are assigned at enumeration time and held stable across hotplug events for the same physical device (matching by USB VID/PID and port path). A USB keyboard that is unplugged and replugged on the same port gets the same `event<N>` number.

#### 6.2.2 evdev ioctl Emulation

The standard Linux evdev ioctl interface is emulated:

| ioctl | Purpose | Notes |
|---|---|---|
| `EVIOCGVERSION` | Protocol version | Returns `0x010001` (evdev 1.0.1) |
| `EVIOCGID` | Device ID (bus, vendor, product, version) | From USB descriptors or VirtIO |
| `EVIOCGNAME` | Device name string | Human-readable, from descriptor |
| `EVIOCGPHYS` | Physical location string | USB port path, or "virtio" |
| `EVIOCGUNIQ` | Unique device ID | Serial number if available |
| `EVIOCGBIT` | Supported event types / codes bitmap | From `InputCapabilities` |
| `EVIOCGABS` | Absolute axis info (min, max, fuzz, flat) | From device calibration data |
| `EVIOCGKEY` | Current key state bitmap | 96 bytes for KEY_MAX bits |
| `EVIOCGLED` | Current LED state bitmap | 8 bytes for LED_MAX bits |
| `EVIOCGSW` | Current switch state bitmap | 4 bytes for SW_MAX bits |
| `EVIOCGRAB` | Exclusive device grab | Requires `InputCapability::Receive` |
| `EVIOCSFF` | Upload force feedback effect | Requires `InputCapability::ForceFeedback` |
| `EVIOCRMFF` | Remove force feedback effect | Requires `InputCapability::ForceFeedback` |
| `EVIOCGFFINFO` | Force feedback effect info | — |

`EVIOCGRAB` — exclusive device access — is supported for game input. Grabbing a device routes all its events exclusively to the grabbing process. The grab requires `InputCapability::Receive` for the device's seat, and the grab is released automatically when the file descriptor is closed or when the agent loses its capability.

#### 6.2.3 Event Format

Events read from `/dev/input/event<N>` use the standard `struct input_event` layout:

```text
struct input_event {
    struct timeval time;  // 8 or 16 bytes depending on 32/64-bit timeval
    __u16 type;           // EV_KEY, EV_REL, EV_ABS, EV_SYN, etc.
    __u16 code;           // keycode, axis code, etc.
    __s32 value;          // key state, delta, or absolute position
};
// Total: 24 bytes (64-bit) or 16 bytes (32-bit)
```

The timestamp field is populated from the hardware event timestamp (CNTPCT_EL0 ticks, converted to `struct timeval` with microsecond precision). Timestamps propagate unchanged through the entire pipeline — the value written to the POSIX fd is the same timestamp that was captured at the IRQ handler.

#### 6.2.4 Merged Mouse Interface

`/dev/input/mice` provides a merged view of all mice as a single PS/2-style protocol stream. This is for legacy applications that read the merged mouse device rather than individual event nodes. The protocol is:

```text
Byte 0: buttons (bit 0=left, bit 1=right, bit 2=middle) | 0x08
Byte 1: delta X (signed byte, relative)
Byte 2: delta Y (signed byte, relative)
```

Mouse events from all physical mice and the VirtIO tablet device are merged into this stream. The merged interface does not carry timestamps, touch, or scroll data — applications requiring these must use the per-device `/dev/input/event<N>` nodes directly.

#### 6.2.5 Legacy Joystick API

`/dev/input/js0` (and `js1`, `js2`, ...) provides the Linux joystick API for legacy game applications:

```text
struct js_event {
    __u32 time;     // timestamp in milliseconds
    __s16 value;    // axis value (-32767..32767) or button (0/1)
    __u8 type;      // JS_EVENT_BUTTON or JS_EVENT_AXIS
    __u8 number;    // button or axis number
};
// Total: 8 bytes
```

Gamepad axes are mapped from their -1.0..1.0 float representation to -32767..32767 for the legacy API. The mapping between AIOS gamepad button/axis layout and the legacy JS numbering follows the Linux gamepad specification.

#### 6.2.6 Capability Enforcement at the POSIX Boundary

Opening `/dev/input/event<N>` is not a free operation. The POSIX bridge checks capabilities at `open()` time:

- **No capability:** `open()` returns `EACCES` (permission denied)
- **`Receive` for the device's seat:** read-only access to events for focused surfaces
- **`Observe` (user-authorized):** read-only access to all events from this device
- **`DeviceAccess` (driver-level):** full access including configuration ioctls

This enforcement is structural. The POSIX bridge delegates the check to the kernel capability gate — the same gate that protects the IPC-based input API. A process that cannot receive input via the native API also cannot receive it via the POSIX bridge.

-----

### 6.3 Power Management

The input subsystem participates in the system power lifecycle through three mechanisms: user activity reporting (idle detection), device power state management, and UI responsiveness boosting.

The system-wide power management architecture is defined in [power-management.md](../power-management.md). This section covers how the input subsystem connects to that architecture.

#### 6.3.1 User Activity Reporting

The input subsystem is the primary source of user activity signals for the power policy engine. Every input event that reaches the compositor updates the system's idle timer.

The power policy engine subscribes to input events via the subsystem framework and maintains a `UserActivityState`:

```rust
pub struct UserActivityState {
    /// Timestamp of the last user input event of any kind.
    pub last_input: Timestamp,
    /// Timestamp of the last "significant" input event.
    /// Mouse jitter and scroll are excluded; key presses and clicks count.
    pub last_significant_input: Timestamp,
    /// Whether the system is currently receiving high-frequency input
    /// (e.g., typing, active gaming, scrolling).
    pub active_input: bool,
}
```

The distinction between `last_input` and `last_significant_input` prevents mouse jitter and accidental scroll from resetting an intentional idle state. The policy engine applies different idle escalation thresholds for each:

| Signal | Default idle timeout | Effect |
|---|---|---|
| Any input (`last_input`) | 5 minutes | Resets display dimming timer |
| Significant input (`last_significant_input`) | 10 minutes | Resets sleep timer |
| `active_input` = true | While true | Suppresses all idle escalation |

These defaults are user-configurable. Different activity profiles (presentation mode, gaming mode) adjust the thresholds.

#### 6.3.2 Wake on Input

Input events are the primary mechanism for waking the system from low-power states:

| System state | Wake source | Latency |
|---|---|---|
| S0ix / display off | Any key press, mouse move, touch | <100ms |
| S3 (suspend to RAM) | Any key press (configurable), power button | <2s |
| S4 (suspend to disk) | Power button only | ~15s |

Wake-on-key from S3 is configurable per-device. By default, any keyboard key wakes the system. This can be restricted to specific keys (power key only) for shared-use or kiosk devices.

Bluetooth HID devices register a `WakeEvent::InputDevice(device_id)` with the power management subsystem. This instructs the Bluetooth controller to maintain enough power for HID packet reception during S3.

USB HID devices use the USB remote wakeup mechanism. The input subsystem driver sets `bRemoteWakeup` on HID devices during suspend, allowing any HID report to generate a USB wakeup signal.

#### 6.3.3 Input-Triggered CPU Boost

When the system receives user input after a period of low activity, it briefly boosts CPU and GPU resources for faster UI response. This follows the Android input boost pattern.

```text
Input event arrives after >100ms idle
    │
    ▼
Input subsystem notifies scheduler: boost_input()
    │
    ▼
Scheduler temporarily elevates Interactive-class priority ceiling
Duration: 100ms (keyboard/mouse) or 150ms (touch begin)
    │
    ▼
UI rendering thread gets elevated scheduling priority
GPU frequency governor switches to performance profile
    │
    ▼
After boost window: scheduler returns to normal priority assignment
```

The boost is advisory — the scheduler applies it only if thermal headroom permits. The input subsystem does not directly modify scheduling parameters; it sends a `SchedulerHint::InputBoost { duration_ms }` notification to the scheduler via IPC. Reference: [scheduler.md](../../kernel/scheduler.md) for the scheduler's handling of external hints.

#### 6.3.4 Device-Level Power Management

Individual input devices follow the subsystem framework's `PowerState` lifecycle:

| State | Condition | Behavior |
|---|---|---|
| `Active` | Device receiving events recently | Full scan rate |
| `Idle` | No events for 2 seconds | Reduced scan rate (see below) |
| `Suspended` | No events for device's idle timeout | USB suspend / BT sniff mode |
| `Off` | Device explicitly powered down | Not applicable for most input devices |

Scan rate reduction during `Idle`:

| Device type | Active rate | Idle (AC) | Idle (battery) |
|---|---|---|---|
| Touchscreen | 120 Hz | 60 Hz | 30 Hz |
| USB mouse | 1000 Hz | 125 Hz | 125 Hz (AC) or 62.5 Hz (battery) |
| USB keyboard | Event-driven | Event-driven | Event-driven (no polling) |
| Bluetooth HID | 100 Hz | Sniff mode (~10 Hz) | Sniff mode |

Rate reduction is transparent to applications. The subsystem translates polling rate changes into updated device capabilities but does not expose the internal scan rate.

USB HID suspend resumes within ~10ms on first event. Bluetooth sniff mode resumes within ~20ms. These latencies are within the interactive input budget (<4ms total pipeline target) only for the first event after resume — subsequent events flow at full rate.

-----

### 6.4 Audit & Observability

The input subsystem's audit system enforces **privacy by architecture**: it records that input happened, not what was typed or where the pointer went. This is not a policy configuration — it is a structural constraint built into the audit record type.

The audit system follows the subsystem framework pattern defined in [subsystem-framework.md](../subsystem-framework.md) §7.

#### 6.4.1 What Is Logged

Every audit record implements `AuditRecord` (timestamp, agent ID, session ID, event type, summary). Input-specific audit events are written to `system/audit/input/`:

```rust
pub struct InputAuditEvent {
    // AuditRecord fields (timestamp, agent, session, event_type, summary)

    /// Device that generated this event
    pub device: DeviceId,
    /// Number of events processed (NOT which keys — only count)
    pub event_count: u64,
    /// Duration of the audited period
    pub duration: Duration,
}
```

Events that are always logged:

- **Device connected:** new input device appeared (device class, VID/PID, port)
- **Device disconnected:** input device removed (device ID, session count at disconnect)
- **Authorization decision:** device capability check result (APPROVED, DENIED, REVOKED)
- **Capability grant:** `InputCapability` token issued to an agent (capability type, scope, expiry)
- **Capability revocation:** capability token revoked (reason: manual, expiry, device disconnect)
- **Focus change:** which agent gained keyboard focus for a seat (agent ID, seat ID, timestamp)
- **Secure session start/end:** password mode entered or exited (no content logged)
- **BadUSB decision:** device blocked or revoked after anomaly detection (device descriptor hash, reason)
- **Input rate anomaly:** unusual event rate detected (device ID, measured rate, expected range)
- **Observer session:** `Observe` capability used (agent ID, event filter, event count — no content)

#### 6.4.2 What Is Never Logged

The following are **structurally excluded** from all audit records. The `InputAuditEvent` type does not have fields for them, and no path in the audit system produces them:

- Keystroke content — which keys were pressed
- Pointer positions — where the cursor was at any time
- Touch coordinates — where the screen was touched
- Gamepad axis values — stick positions or trigger pressures
- IME pre-edit or committed text — what was composed
- Gesture trajectories — the path of a swipe or stroke

The only numeric content in audit records is `event_count` and `duration`. These reveal usage patterns (the user typed for 20 minutes) but not content (what was typed).

#### 6.4.3 Audit Space Structure

```text
system/audit/input/
├── sessions/          ← one record per device session (open → close)
│   ├── <timestamp>-<device-id>-<agent-id>   ← active session
│   └── ...
├── devices/           ← device connect/disconnect history
│   ├── <timestamp>-<device-id>-connected
│   └── <timestamp>-<device-id>-disconnected
├── capabilities/      ← capability grant and revocation records
│   ├── <timestamp>-<agent-id>-granted
│   └── <timestamp>-<agent-id>-revoked
└── anomalies/         ← BadUSB decisions and rate anomalies
    └── <timestamp>-<device-id>-<reason>
```

Audit records are serialized as `InputAuditEvent` structs and written to the Block Engine under the `SecurityZone::System` classification. At the kernel level, high-frequency events (device connect/disconnect, capability changes) are first buffered in the per-core `LogRing` (64-byte `LogEntry` format) and flushed to persistent storage asynchronously. Access to `system/audit/input/` requires `Capability::AuditRead` with the `input` scope.

#### 6.4.4 Keystroke Dynamics as Biometric Data

Keystroke timing statistics (inter-key intervals and hold durations) are collected by the AI subsystem for behavioral anomaly detection and continuous authentication. These are classified as biometric data and handled separately from the audit system:

- Stored in the user's identity space with `BiometricTemplate` capability protection
- Encrypted at rest using the user's identity key
- Not accessible to audit queries — they are biometric data, not audit data
- Reference: [identity.md](../../experience/identity.md) for biometric template management

The audit system records that the anomaly detector ran and what decision it made — not the timing data that informed the decision.

-----

### 6.5 Compositor Integration

The compositor is the input subsystem's primary consumer and the final routing authority. All input events flow through the compositor before reaching applications.

The full compositor architecture is defined in [compositor.md](../compositor.md). This section covers the input-specific integration surface.

#### 6.5.1 Focus Management

The compositor maintains per-seat focus state and is the sole authority for keyboard focus changes. The input subsystem does not make routing decisions — it delivers cooked events to the compositor and the compositor routes them.

```text
Focus state per seat:
├── keyboard_focus: Option<SurfaceId>    // which surface receives keyboard events
├── pointer_focus: Option<SurfaceId>     // which surface is under the pointer
├── touch_focus: HashMap<TouchId, SurfaceId>  // per-touch-point assignment
└── gamepad_focus: Option<SurfaceId>     // last-focused game surface
```

Focus changes are triggered by:

- User click on a surface (pointer/touch)
- Tab key (keyboard navigation within the compositor's focus order)
- Programmatic focus request from an agent (requires `FOCUS` seat permission)
- Compositor gesture (workspace switch, window management)
- Accessibility switch scan reaching a new target

Every focus change generates a `FocusLost` / `FocusGained` pair delivered to the losing and gaining surfaces. IME is notified immediately on focus change.

#### 6.5.2 Hit-Testing

The compositor maps pointer and touch coordinates to surfaces using a hit-test engine. Hit-testing occurs on every pointer motion event and every touch begin event.

Hit-test inputs:

- Pointer or touch position in screen coordinates (post-acceleration, post-calibration)
- Current surface list with positions, sizes, stacking order, and visibility

Hit-test output:

- The topmost surface at that position (or compositor desktop background if none)
- The position in the surface's local coordinate system (for delivery to the agent)
- The viewport clip for that surface (for capability enforcement of embedded agents)

Hit-testing is the mechanism that enforces viewport attenuation (§6.1.2). An embedded game agent's viewport rectangle is part of the surface record. The compositor's hit-test reports the embedded surface only when the pointer is within the embedded viewport, and the coordinates delivered to the game are in the game's local coordinate system — not the browser's.

#### 6.5.3 Cursor Rendering

The compositor renders the pointer cursor using a hardware overlay when available. The overlay bypasses vsync for cursor motion, giving sub-vsync cursor response:

```text
Hardware overlay path (preferred):
  MotionEvent → compositor → update overlay position register → display controller
  Latency: ~1ms (one register write, next scanline)

Software cursor path (fallback when no overlay available):
  MotionEvent → compositor → schedule cursor composite → next vsync → display
  Latency: 8–16ms (one full frame)
```

Cursor shapes are managed by the compositor. Applications request shape changes via the compositor's cursor API:

| Shape | Usage |
|---|---|
| Arrow (default) | General pointer |
| Text (I-beam) | Over text input areas |
| Pointer (hand) | Over clickable elements |
| Resize (directional) | Window resize handles |
| Wait (spinner) | Blocking operation in progress |
| None | Cursor hidden (games, media playback) |
| Custom | Application-provided bitmap (restricted size) |

Auto-hide behavior: the compositor hides the cursor after 2 seconds of touch-only input (no pointer motion). The cursor reappears on first pointer motion event. This is relevant for convertible devices (laptop/tablet) that switch between pointer and touch modes.

#### 6.5.4 Input Method Controller

The compositor manages the Input Method Editor (IME) lifecycle. The IME is a separate agent that intercepts keyboard events for a focused text input surface and produces composed text events.

IME lifecycle:

```text
1. Text input surface gains focus
   └── Compositor sends: ActivateInputMethod { surface_id, content_type, cursor_rect }

2. IME active
   ├── Compositor routes KeyEvents to IME (not to the application)
   ├── IME produces TextEvent::PreEdit (compositor forwards to application)
   └── IME produces TextEvent::Commit (compositor forwards to application)

3. Text input surface loses focus
   └── Compositor sends: DeactivateInputMethod { surface_id }
       └── IME clears pre-edit state, application receives PreEditClear
```

The `content_type` field (email, URL, password, number, search, etc.) informs the IME which keyboard layout, prediction behavior, and autocorrect aggressiveness to apply. Password fields disable IME entirely — the compositor reports `ContentType::Password` and the IME agent is not activated.

Pre-edit display positioning: the compositor provides `cursor_rect` (the text cursor position in screen coordinates) to the IME so the IME can position its candidate list near the insertion point. The IME renders its candidate list as its own surface, composited by the compositor at the appropriate position.

IME is switched immediately on focus change — there is no state leakage between input contexts.

#### 6.5.5 Compositor-Intercepted Gestures

Some gestures are consumed by the compositor and not delivered to applications. These are system-level actions that applications should not intercept:

| Gesture | Action | Interception level |
|---|---|---|
| 4-finger swipe left/right | Workspace switch | Compositor (consumed) |
| 4-finger pinch | Mission Control / overview | Compositor (consumed) |
| 3-finger swipe up | App exposé | Compositor (consumed) |
| Edge swipe from left | Navigation back | Compositor (forwarded to app after hold threshold) |
| Edge swipe from bottom | Home | Compositor (consumed) |

Applications that need full gesture control (games, drawing applications) can request a **gesture exclusive mode** via the compositor API. In this mode, the compositor passes all gestures to the application and handles only the Secure Attention Sequence. Gesture exclusive mode requires explicit user consent.

#### 6.5.6 Timestamp Propagation

Hardware timestamps are preserved through the entire pipeline. The compositor does not replace or adjust timestamps — it adds its own processing timestamp alongside the hardware timestamp for latency measurement:

```rust
pub struct CompositorTimestamp {
    /// Hardware timestamp (CNTPCT_EL0 at IRQ time)
    pub hardware: u64,
    /// Compositor receive timestamp (when compositor processed the event)
    pub compositor: u64,
    /// Application delivery timestamp (when event was placed in app ring)
    pub delivered: u64,
}
```

These three timestamps enable end-to-end latency measurement at any point in the system. The developer tools can display the full timestamp chain for any event.

-----

### 6.6 UI Toolkit Integration

The UI toolkit receives typed `InputEvent` values from the compositor and maps them to widget interactions. The full toolkit architecture is defined in [interface-kit.md](../../applications/interface-kit.md). This section covers the input handling contract between the toolkit and the input subsystem.

#### 6.6.1 Widget Focus Model

The toolkit maintains an internal focus order — the ordered list of focusable widgets — and tracks which widget holds keyboard focus within each surface.

Keyboard navigation rules:

- **Tab:** moves focus to the next focusable widget in document order
- **Shift+Tab:** moves focus to the previous focusable widget
- **Arrow keys:** navigate within list, grid, menu, and tree widgets
- **Enter / Space:** activates the focused widget (click equivalent)
- **Escape:** closes the nearest dismissible container (menu, dropdown, dialog)

Tab order follows document order by default. Widgets can declare an explicit `tab_index` to override ordering. Negative `tab_index` removes a widget from the tab sequence (still focusable by pointer).

Every toolkit widget is keyboard-navigable by default. There is no opt-in required. Widgets that cannot meaningfully receive keyboard focus (decorative images, non-interactive labels) are excluded from the tab sequence automatically.

#### 6.6.2 Event Bubbling

Input events propagate from the target widget through the widget hierarchy. Each widget in the path can consume the event (stop propagation) or let it continue.

```text
User presses Tab key
    │
    ▼
FocusManager intercepts Tab (moves focus, does not bubble)

User presses Ctrl+Z
    │
    ▼
TextInputWidget receives KeyEvent { key: Z, modifiers: CTRL }
  └── TextInputWidget handles undo → consumes event
  └── Event stops here

User presses Ctrl+Z in non-text context
    │
    ▼
ButtonWidget receives KeyEvent { key: Z, modifiers: CTRL }
  └── ButtonWidget does not handle Ctrl+Z → passes through
    │
    ▼
PanelWidget receives event
  └── PanelWidget does not handle → passes through
    │
    ▼
WindowWidget receives event
  └── WindowWidget handles undo at window level → consumes
```

Pointer events bubble from the hit-tested widget upward. Touch events are delivered to the widget where the touch began (sticky assignment, matching compositor touch focus semantics) and bubble from there.

#### 6.6.3 Focus Indicators

Focused widgets must display a visible focus indicator. This is an accessibility requirement (WCAG 2.4.7: Focus Visible) and is not optional.

The toolkit enforces focus indicator rendering at the framework level: the default widget rendering pipeline draws a focus ring around any widget that holds keyboard focus. Applications can customize the focus ring style but cannot suppress it entirely.

The focus ring must meet minimum contrast requirements against the widget background. The toolkit checks contrast at render time and falls back to a high-contrast default if the application-supplied ring fails the check.

#### 6.6.4 Content Type Hints

Text input widgets declare a `ContentType` that informs the input subsystem how to configure the input pipeline for that field:

```rust
pub enum ContentType {
    Text,             // general text — default autocorrect, prediction
    Email,            // email address — disable autocorrect, @ key prominent
    Url,              // URL — disable autocorrect, suggest history
    Password,         // secret — disable IME, disable prediction, disable logging
    Pin,              // numeric PIN — numeric keyboard, no prediction
    Number,           // general number — numeric keyboard, allow decimal
    Search,           // search query — aggressive prediction, no autocorrect
    PersonName,       // proper noun — capitalization, no autocorrect
    PhoneNumber,      // phone — numeric with +, dash, space
    Multiline,        // free-form text — softer autocorrect, Enter = newline
}
```

Content type propagates from the widget to the compositor via `ActivateInputMethod { content_type, ... }`, and from the compositor to the IME agent. The IME adjusts its behavior accordingly — prediction, autocorrect aggressiveness, layout, and candidate list content all respond to content type.

Password fields receive special treatment at every layer:

- The compositor does not activate the IME (`ContentType::Password` suppresses IME activation)
- The input pipeline does not record keystroke counts during password entry
- Events carry `EventFlags::SECURE`
- Observer agents are excluded (§6.1.4)
- The accessibility layer suppresses character announcement (screen readers hear "password character" not the character itself)

#### 6.6.5 Touch Targets

Touchscreen targets must meet minimum size requirements for reliable activation:

| Requirement | Minimum | Preferred |
|---|---|---|
| Touch target size | 44×44 pt (logical pixels) | 48×48 pt |
| Touch target spacing | 8 pt between adjacent targets | — |

These values follow WCAG 2.5.5 (AAA criterion). The toolkit enforces the minimum at layout time — if a developer places a button smaller than 44×44 pt, the toolkit expands the touch target area (invisible padding) to meet the minimum, without changing the visual size.

Touch target expansion is transparent: the visual widget is 24×24 pt (an icon button), but the touch-sensitive area is 44×44 pt centered on the widget. Pointer events still report position in visual coordinates — the expansion affects only the hit-test.

#### 6.6.6 Pointer and Touch Exclusivity

The toolkit respects the input subsystem's pointer/touch exclusivity model. When touch input is active on a widget, pointer events to that widget are suppressed. When pointer input is active, touch events from a different touch ID do not trigger the widget's click handler.

This prevents the "ghost click" problem common in dual-input applications: a touch that lifts and triggers a click through the same code path as a pointer click, resulting in double activation.

The toolkit implements this through its own pointer/touch state machine, aligned with the compositor's touch focus assignment. If the compositor assigns `TouchId(3)` to a surface, the toolkit within that surface handles `TouchId(3)` events as the active touch and ignores pointer hover events until the touch ends.
