# Input Kit

**Layer:** Platform | **Crate:** `aios_input` | **Architecture:** [`docs/platform/input.md`](../../platform/input.md)

## 1. Overview

Input Kit provides a unified, hardware-agnostic event pipeline that transforms raw signals
from keyboards, mice, touchscreens, gamepads, and accessibility devices into semantic events
that agents can consume without caring about the underlying hardware. Raw USB HID reports,
VirtIO-input events, and Bluetooth HID packets all enter the pipeline and emerge as
strongly-typed Rust enums: `KeyEvent`, `MotionEvent`, `TouchEvent`, `GestureEvent`,
`TextEvent`, and `CommandEvent`. The agent receives exactly the level of abstraction it
needs -- a text editor gets composed text with IME support, a game gets raw axis deltas,
and a terminal gets individual key presses.

The pipeline is layered into three stages. First, device drivers produce evdev-compatible
raw events. Second, the input subsystem service translates these through keyboard layout
mapping, dead key composition, pointer acceleration, palm rejection, and gesture recognition.
Third, the focus manager routes processed events to the correct surface based on spatial
focus (pointer position), keyboard focus (active window), and capability scope (an agent can
only receive events for its own surfaces). Secure input mode isolates credential entry from
all observers, including accessibility services and input method editors.

Use Input Kit when your agent needs to receive keyboard input, pointer motion, touch events,
gamepad input, or system hotkeys. Do not use it for voice input (use
[Conversation Kit](../intelligence/conversation.md) speech-to-text, which routes through
[Audio Kit](./audio.md)) or for screen reading (use the accessibility tree in the
compositor, not raw input events). Input Kit feeds directly into the compositor's focus
system and the [Interface Kit](../application/interface.md) widget event handlers.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use core::time::Duration;

/// A hardware input device registered with the input subsystem.
///
/// Drivers for USB HID, VirtIO-input, Bluetooth HID, and accessibility
/// devices all implement this trait. Agents typically do not interact
/// with InputDevice directly -- they receive processed InputEvents.
pub trait InputDevice {
    /// The device's unique identifier.
    fn id(&self) -> DeviceId;

    /// Human-readable device name.
    fn name(&self) -> &str;

    /// The device class (keyboard, pointer, touchscreen, gamepad, etc.).
    fn device_class(&self) -> DeviceClass;

    /// Supported event types (EV_KEY, EV_REL, EV_ABS, etc.).
    fn capabilities(&self) -> &DeviceCapabilities;

    /// Whether the device is currently connected and active.
    fn is_connected(&self) -> bool;

    /// Set an LED state on the device (e.g., Caps Lock, Num Lock).
    fn set_led(&mut self, led: Led, state: bool) -> Result<(), InputError>;

    /// Send a force-feedback effect to the device (gamepads, steering wheels).
    fn send_haptic(&mut self, effect: &HapticEffect) -> Result<(), InputError>;
}

/// Typed input event delivered to agents via the focus manager.
///
/// This is the primary interface agents use to receive input. Events are
/// delivered through the agent's event loop or callback registration.
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// Keyboard or button press/release/repeat.
    Key(KeyEvent),
    /// Mouse, trackpad, or trackball motion with pointer coordinates.
    Motion(MotionEvent),
    /// Touchscreen contact (multi-touch capable).
    Touch(TouchEvent),
    /// Gamepad axes and buttons.
    Gamepad(GamepadEvent),
    /// Recognized gesture (pinch, swipe, rotate, long-press).
    Gesture(GestureEvent),
    /// Composed text from the input method editor (IME).
    Text(TextEvent),
    /// Hardware switch event (lid open/close, headphone insert).
    Switch(SwitchEvent),
    /// Gaze tracking input (eye-tracking accessibility devices).
    Gaze(GazeEvent),
    /// Semantic command from hotkey, voice, or AIRS shortcut.
    Command(CommandEvent),
}

/// Keyboard / button event with post-layout keysym.
#[derive(Debug, Clone)]
pub struct KeyEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    /// XKB keysym (post-layout-transform).
    pub key: KeyCode,
    /// Raw hardware scancode (pre-layout).
    pub scancode: u16,
    /// Key state: Pressed, Released, or Repeat.
    pub state: KeyState,
    /// Current modifier state (Shift, Ctrl, Alt, Super).
    pub modifiers: Modifiers,
    pub flags: EventFlags,
}

/// Mouse / trackpad motion event with both processed and raw deltas.
#[derive(Debug, Clone)]
pub struct MotionEvent {
    pub timestamp: u64,
    pub device_id: DeviceId,
    /// Pointer position in screen coordinates (post-acceleration).
    pub x: f32,
    pub y: f32,
    /// Raw delta (pre-acceleration, for games and drawing apps).
    pub dx: f32,
    pub dy: f32,
    /// Scroll wheel deltas (pixels).
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub buttons: MouseButtons,
    pub flags: EventFlags,
}

/// Three-layer gesture recognition engine.
///
/// Layer 1 (device): hardware-specific preprocessing (palm rejection, jitter filter).
/// Layer 2 (semantic): gesture state machines (pinch, swipe, rotate, long-press).
/// Layer 3 (application): agent-registered custom gesture recognizers.
pub trait GestureRecognizer {
    /// Register a custom gesture pattern.
    fn register_gesture(
        &mut self,
        pattern: GesturePattern,
        callback: GestureCallback,
    ) -> Result<GestureId, InputError>;

    /// Unregister a previously registered gesture.
    fn unregister_gesture(&mut self, id: &GestureId) -> Result<(), InputError>;

    /// Query the state of a multi-touch gesture in progress.
    fn active_gesture(&self) -> Option<&ActiveGesture>;

    /// Set the gesture recognition threshold (sensitivity adjustment).
    fn set_threshold(&mut self, gesture_type: GestureType, threshold: f32)
        -> Result<(), InputError>;
}

/// Focus routing and surface-to-input binding.
///
/// The focus manager determines which surface receives input events based
/// on pointer position, keyboard focus, and the agent's capability scope.
pub trait FocusManager {
    /// Get the currently focused surface for keyboard input.
    fn keyboard_focus(&self) -> Option<SurfaceId>;

    /// Get the surface under the pointer.
    fn pointer_focus(&self) -> Option<SurfaceId>;

    /// Request keyboard focus for a surface (agent must own the surface).
    fn request_focus(&mut self, surface: SurfaceId) -> Result<(), InputError>;

    /// Enter secure input mode (isolates input from all observers).
    fn enter_secure_input(&mut self) -> Result<SecureInputGuard, InputError>;

    /// Check if secure input mode is active.
    fn is_secure_input_active(&self) -> bool;
}

/// System-wide hotkey registration with priority levels.
///
/// Hotkeys are processed before surface-level event dispatch. System hotkeys
/// (volume, brightness, screenshot) take priority over agent-registered hotkeys.
pub trait HotkeyRegistry {
    /// Register a global hotkey binding.
    fn register(
        &mut self,
        binding: KeyBinding,
        action: HotkeyAction,
        priority: HotkeyPriority,
    ) -> Result<HotkeyId, InputError>;

    /// Unregister a hotkey.
    fn unregister(&mut self, id: &HotkeyId) -> Result<(), InputError>;

    /// List all registered hotkeys (visible to the agent's scope).
    fn list(&self) -> &[RegisteredHotkey];

    /// Check if a key binding is already claimed.
    fn is_bound(&self, binding: &KeyBinding) -> bool;
}

/// Key binding descriptor for hotkey registration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub key: KeyCode,
    pub modifiers: Modifiers,
}

/// Device class categories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceClass {
    Keyboard,
    Pointer,
    Touchscreen,
    Touchpad,
    Gamepad,
    Stylus,
    SwitchDevice,
    EyeTracker,
    BrailleDisplay,
}
```

## 3. Usage Patterns

**Minimal -- receive keyboard input in a text field:**

```rust
use aios_input::{InputKit, InputEvent, KeyState};

// Subscribe to input events for the agent's focused surface
let mut receiver = InputKit::event_receiver()?;

loop {
    match receiver.next_event().await? {
        InputEvent::Key(key) if key.state == KeyState::Pressed => {
            handle_key_press(key.key, key.modifiers);
        }
        InputEvent::Text(text) => {
            // Composed text from IME (handles CJK, dead keys, etc.)
            text_field.insert(&text.string);
        }
        _ => {} // Ignore other event types
    }
}
```

**Realistic -- pointer and touch input for a drawing canvas:**

```rust
use aios_input::{InputKit, InputEvent, MouseButtons};

let mut receiver = InputKit::event_receiver()?;
let mut drawing = false;

loop {
    match receiver.next_event().await? {
        InputEvent::Motion(motion) => {
            if motion.buttons.contains(MouseButtons::LEFT) {
                // Draw with mouse -- use raw deltas for precision
                canvas.stroke_to(motion.x, motion.y, pressure: 1.0);
                drawing = true;
            } else if drawing {
                canvas.end_stroke();
                drawing = false;
            }
        }
        InputEvent::Touch(touch) => {
            match touch.phase {
                TouchPhase::Started => canvas.begin_stroke(touch.x, touch.y, touch.pressure),
                TouchPhase::Moved => canvas.stroke_to(touch.x, touch.y, touch.pressure),
                TouchPhase::Ended => canvas.end_stroke(),
                TouchPhase::Cancelled => canvas.cancel_stroke(),
            }
        }
        InputEvent::Gesture(gesture) => {
            match gesture.kind {
                GestureType::Pinch { scale, .. } => canvas.zoom(scale),
                GestureType::Rotate { angle, .. } => canvas.rotate(angle),
                GestureType::Pan { dx, dy, .. } => canvas.scroll(dx, dy),
                _ => {}
            }
        }
        _ => {}
    }
}
```

**Advanced -- register system hotkeys for a music player:**

```rust
use aios_input::{InputKit, KeyBinding, KeyCode, Modifiers, HotkeyPriority};

let mut hotkeys = InputKit::hotkey_registry()?;

// Register media control hotkeys (agent-level priority)
let play_pause = hotkeys.register(
    KeyBinding { key: KeyCode::MediaPlayPause, modifiers: Modifiers::empty() },
    HotkeyAction::Custom("toggle-playback"),
    HotkeyPriority::Agent,
)?;

let next_track = hotkeys.register(
    KeyBinding { key: KeyCode::MediaNextTrack, modifiers: Modifiers::empty() },
    HotkeyAction::Custom("next-track"),
    HotkeyPriority::Agent,
)?;

// Custom shortcut: Ctrl+Shift+M to toggle mute
let mute = hotkeys.register(
    KeyBinding { key: KeyCode::KeyM, modifiers: Modifiers::CTRL | Modifiers::SHIFT },
    HotkeyAction::Custom("toggle-mute"),
    HotkeyPriority::Agent,
)?;

// Handle hotkey events
let mut receiver = InputKit::event_receiver()?;
loop {
    if let InputEvent::Command(cmd) = receiver.next_event().await? {
        match cmd.action.as_str() {
            "toggle-playback" => player.toggle_play_pause(),
            "next-track" => player.next_track(),
            "toggle-mute" => player.toggle_mute(),
            _ => {}
        }
    }
}
```

> **Common Mistakes**
>
> - **Using KeyEvent for text input.** `KeyEvent` gives you raw keycodes, not composed
>   text. For text fields, use `TextEvent` which handles IME composition, dead keys, and
>   Unicode input correctly.
> - **Ignoring the `flags` field.** Events with `EventFlags::INJECTED` or
>   `EventFlags::FROM_ACCESSIBILITY` may need different handling (e.g., do not replay
>   injected events to avoid loops).
> - **Not releasing hotkeys.** Hotkey registrations persist until explicitly unregistered
>   or the agent exits. Leaked hotkeys block other agents from using those key combinations.
> - **Tight polling on `next_event()`.** Use `await` or callback-based delivery, not a
>   spin loop. Input events arrive at human timescales (milliseconds), not microseconds.
> - **Requesting raw device access.** Most agents should use `InputKit::event_receiver()`
>   for processed events, not `InputKit::raw_device_access()` which requires elevated
>   capabilities and bypasses gesture recognition.

## 4. Integration Examples

**Input Kit + Interface Kit -- widget event handling:**

```rust
use aios_input::{InputEvent, KeyCode, Modifiers};
use aios_interface::{Widget, Button, TextField};

// Interface Kit widgets receive Input Kit events through the focus manager.
// The widget framework translates InputEvents into widget-level callbacks.
let mut text_field = TextField::new("Search...");

text_field.on_text_input(|text_event| {
    // TextEvent from Input Kit, delivered through Interface Kit's event loop
    update_search_results(&text_event.string);
});

text_field.on_key(|key_event| {
    if key_event.key == KeyCode::Escape {
        clear_search();
    }
});
```

**Input Kit + Compositor -- secure credential entry:**

```rust
use aios_input::InputKit;

// Enter secure input mode for password entry.
// In this mode, no other agent (including accessibility services and IME)
// can observe keystrokes. The compositor shows a visual indicator.
let guard = InputKit::focus_manager()?.enter_secure_input()?;

// All KeyEvents during this guard's lifetime are isolated
let password = collect_password_keystrokes(&guard).await?;

// Secure mode ends when the guard is dropped
drop(guard);
```

**Input Kit + Accessibility -- switch scanning:**

```rust
use aios_input::{InputKit, InputEvent, DeviceClass};

// Accessibility switch devices generate simple press/release events
// that the accessibility service maps to UI scanning actions.
let mut receiver = InputKit::event_receiver()?;

loop {
    if let InputEvent::Key(key) = receiver.next_event().await? {
        if key.device_class == Some(DeviceClass::SwitchDevice) {
            match key.state {
                KeyState::Pressed => accessibility_scanner.advance(),
                KeyState::Released => accessibility_scanner.select(),
                _ => {}
            }
        }
    }
}
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `InputKit::event_receiver` | `InputReceive` | Scoped to agent's own surfaces only |
| `InputKit::raw_device_access` | `InputRawAccess` | Bypasses processing; rarely needed |
| `HotkeyRegistry::register(System)` | `InputSystemHotkey` | Reserved for system services |
| `HotkeyRegistry::register(Agent)` | `InputReceive` | Standard agent hotkeys |
| `FocusManager::enter_secure_input` | `InputSecure` | Isolates input for credentials |
| `FocusManager::request_focus` | `InputReceive` | Agent must own the target surface |
| `InputDevice::send_haptic` | `InputHaptic` | Force feedback / vibration |

```toml
# Agent manifest example
[capabilities.required]
InputReceive = { reason = "Receive keyboard and pointer input for UI" }

[capabilities.optional]
InputHaptic = { reason = "Provide tactile feedback on gamepad" }
InputSecure = { reason = "Secure password entry in login form" }
```

## 6. Error Handling

```rust
/// Errors returned by Input Kit operations.
#[derive(Debug)]
pub enum InputError {
    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// No input device of the requested class is connected.
    NoDevice(DeviceClass),

    /// The specified device was not found.
    DeviceNotFound(DeviceId),

    /// The hotkey binding is already claimed by another agent or the system.
    HotkeyConflict(KeyBinding),

    /// The maximum number of hotkey registrations has been reached.
    TooManyHotkeys { max: u32 },

    /// Secure input mode is not available (e.g., already active in another surface).
    SecureInputUnavailable,

    /// The gesture pattern is invalid or unsupported.
    InvalidGesturePattern,

    /// The event receiver was closed (agent shutting down).
    ReceiverClosed,

    /// The device does not support the requested operation (e.g., haptic on keyboard).
    UnsupportedOperation,

    /// A hardware driver error occurred.
    DeviceError(String),
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| No keyboard connected | Touchscreen virtual keyboard offered; switch/gaze input still works |
| No pointer device | Keyboard-only navigation (Tab/arrow keys); touch input still works |
| Gesture recognition fails | Raw touch events delivered; agent handles touch directly |
| AIRS unavailable | Predictive text and adaptive acceleration disabled; static defaults used |
| Hotkey conflict | `HotkeyConflict` error; agent can choose an alternative binding |
| Bluetooth HID disconnects | Device reconnection handled automatically; events resume when reconnected |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Predictive text | Transformer-based word/sentence prediction | No prediction |
| Adaptive acceleration | Learns user's preferred pointer speed and curves | Static acceleration curve |
| Gesture learning | Custom gesture recognition from user demonstrations | Predefined gestures only |
| Anomaly detection | Detects unusual input patterns (BadUSB, injection) | Static heuristic checks |
| Context-aware shortcuts | Suggests shortcuts based on usage patterns | Manual shortcut registration |
| Accessibility adaptation | ML-powered tremor filtering, adaptive debounce timing | Fixed accessibility thresholds |

**Kernel-internal ML (no AIRS dependency, ships with kernel):**

| Model | Size | Purpose |
| --- | --- | --- |
| BadUSB classifier | ~200 KB | Detects keystroke injection attacks from USB devices |
| Tremor filter | ~150 KB | Filters involuntary movement for motor accessibility |
| Palm rejection | ~300 KB | Rejects accidental palm contact on touchscreens |

**Platform availability:**

| Platform | USB HID | VirtIO-input | Bluetooth HID | Touchscreen | Gamepad | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| QEMU virt | Emulated | Yes | No | No | No | Keyboard + mouse via VirtIO |
| Raspberry Pi 4 | Yes | N/A | Yes | Yes (USB) | Yes (USB/BT) | Full input stack |
| Raspberry Pi 5 | Yes | N/A | Yes | Yes (USB) | Yes (USB/BT) | Full input stack |
| Apple Silicon | Yes | N/A | Yes | Yes (built-in) | Yes (USB/BT) | Full experience |

**Implementation phase:** Phase 6+ (event model, keyboard/pointer drivers, focus routing,
gesture recognition). Bluetooth HID arrives with Phase 10+ (wireless). AI features
require Phase 14+ (AIRS integration).

---

*See also: [Interface Kit](../application/interface.md) | [Audio Kit](./audio.md) | [Capability Kit](../kernel/capability.md) | [Camera Kit](./camera.md) | [Wireless Kit](./wireless.md)*
