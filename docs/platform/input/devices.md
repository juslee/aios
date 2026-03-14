# AIOS Input Devices & Hardware Abstraction

Part of: [input.md](../input.md) — Input Subsystem
**Related:** [events.md](./events.md) — Event model and dispatch, [integration.md](./integration.md) — Capability system and POSIX bridge, [ai.md](./ai.md) — BadUSB defense and anomaly detection

-----

## 3. Hardware Drivers & Device Abstraction

The input subsystem supports seven device classes across four transport layers. Every device — from a USB keyboard to an eye tracker — is represented through the same `InputDevice` trait, producing evdev-compatible events that the transform pipeline processes uniformly.

### 3.1 Device Class Taxonomy

```text
Input Devices
├── Keyboard
│   ├── Standard (104/105-key, ANSI/ISO)
│   ├── Compact (60%, 65%, 75%, TKL)
│   ├── Ergonomic (split, ortholinear)
│   └── Virtual (on-screen, IME-generated)
│
├── Pointing Device
│   ├── Mouse (relative axes, buttons, scroll wheel)
│   ├── Trackpad (multi-touch surface, gestures)
│   ├── Trackball (relative axes, buttons)
│   └── Trackpoint (relative axes, pressure-sensitive)
│
├── Touchscreen
│   ├── Capacitive (multi-touch, 10+ points)
│   ├── Resistive (single-touch, legacy)
│   └── Stylus/Pen (pressure, tilt, eraser, buttons)
│
├── Gamepad
│   ├── Standard (2 sticks, D-pad, face buttons, triggers)
│   ├── Flight stick / HOTAS (multiple axes, HAT switches)
│   ├── Racing wheel (wheel axis, pedals, force feedback)
│   └── Arcade stick (digital stick, buttons)
│
├── Accessibility — Braille Display
│   ├── USB HID (usage page 0x41)
│   ├── Serial protocol (Baum, EuroBraille, HIMS)
│   └── Bluetooth Braille
│
├── Accessibility — Switch Device
│   ├── USB HID switch (button usage page)
│   ├── Keyboard-as-switch (configurable key mapping)
│   └── Sip-and-puff (pneumatic switch)
│
└── Accessibility — Eye Tracker
    ├── USB (Tobii, EyeTech)
    ├── Bluetooth (mobile trackers)
    └── Camera-based (webcam gaze estimation)
```

Each device class has a corresponding `InputCapabilities` bitset that describes what events the device can produce:

```rust
bitflags! {
    pub struct InputCapabilities: u64 {
        // Event types
        const KEY           = 1 << 0;   // EV_KEY: keyboard keys, buttons
        const REL_AXIS      = 1 << 1;   // EV_REL: relative axes (mouse)
        const ABS_AXIS      = 1 << 2;   // EV_ABS: absolute axes (touch, tablet)
        const MISC          = 1 << 3;   // EV_MSC: miscellaneous events
        const SWITCH        = 1 << 4;   // EV_SW: switch events (lid, headphone)
        const LED           = 1 << 5;   // EV_LED: LED control (caps lock, num lock)
        const FORCE_FEEDBACK = 1 << 6;  // EV_FF: force feedback / haptics

        // Device class shortcuts
        const KEYBOARD      = Self::KEY.bits() | Self::LED.bits();
        const MOUSE         = Self::KEY.bits() | Self::REL_AXIS.bits();
        const TOUCHPAD      = Self::KEY.bits() | Self::ABS_AXIS.bits();
        const TOUCHSCREEN   = Self::ABS_AXIS.bits();
        const GAMEPAD       = Self::KEY.bits() | Self::ABS_AXIS.bits()
                            | Self::FORCE_FEEDBACK.bits();
        const BRAILLE       = Self::KEY.bits();  // routing keys + nav keys
        const SWITCH_DEVICE = Self::KEY.bits();   // single/dual switch
        const EYE_TRACKER   = Self::ABS_AXIS.bits();  // gaze X, Y, confidence
    }
}
```

### 3.2 USB HID Protocol Layer

USB HID is the universal protocol for input devices. Every USB keyboard, mouse, gamepad, Braille display, and switch device speaks HID.

#### 3.2.1 Report Descriptor Parsing

HID devices are self-describing via **report descriptors** — binary bytecode that defines the layout and semantics of every field in the device's input/output reports.

```text
Report Descriptor Parser (no_std Rust)

    Bytecode stream
         │
         ▼
    ┌──────────────┐
    │  Item Parser  │    Parse Main/Global/Local items
    │  (stack-based │    Push/Pop global state
    │   state FSM)  │    Track Usage Page, Logical Min/Max,
    │               │    Report Size, Report Count
    └──────┬───────┘
           │
           ▼
    ┌──────────────┐
    │  Collection   │    Build tree of HID collections
    │  Builder      │    (Application, Physical, Logical)
    └──────┬───────┘
           │
           ▼
    ┌──────────────┐
    │  Report       │    Map each field to:
    │  Field Map    │    - Usage Page + Usage
    │               │    - Bit offset + size
    │               │    - Logical min/max
    │               │    - Flags (variable/array/relative/absolute)
    └──────┬───────┘
           │
           ▼
    InputDescriptor   →   Self-describing device capabilities
```

Key data structures:

```rust
/// Parsed HID report descriptor — device self-description
pub struct InputDescriptor {
    pub name: [u8; 64],
    pub vendor_id: u16,
    pub product_id: u16,
    pub collections: FixedVec<HidCollection, 16>,
    pub input_reports: FixedVec<ReportLayout, 8>,
    pub output_reports: FixedVec<ReportLayout, 4>,
}

/// Layout of a single HID report
pub struct ReportLayout {
    pub report_id: u8,
    pub fields: FixedVec<ReportField, 64>,
    pub total_bits: u16,
}

/// A single field within a HID report
pub struct ReportField {
    pub usage_page: u16,
    pub usage_min: u16,
    pub usage_max: u16,
    pub logical_min: i32,
    pub logical_max: i32,
    pub bit_offset: u16,
    pub bit_size: u8,
    pub report_count: u8,
    pub flags: FieldFlags,  // Variable/Array, Relative/Absolute, etc.
}
```

#### 3.2.2 Usage Pages

HID usage pages define semantic meaning. The input subsystem maps these to AIOS device classes:

| Usage Page | ID | Device Class | Examples |
|---|---|---|---|
| Generic Desktop | 0x01 | Mouse, Keyboard, Joystick, Gamepad | X/Y axes, buttons, hat switch |
| Keyboard/Keypad | 0x07 | Keyboard | All keycodes (a-z, modifiers, function keys) |
| Button | 0x09 | All devices with buttons | Button 1-32 |
| Consumer | 0x0C | Media keys | Volume, play/pause, mute, brightness |
| Digitizer | 0x0D | Touchscreen, Stylus | Touch X/Y, pressure, tilt, contact ID |
| Braille Display | 0x41 | Braille | Routing keys, nav keys, dots |
| Vendor Defined | 0xFF00+ | Device-specific | Custom features |

#### 3.2.3 Boot Protocol

For early boot and BIOS compatibility, USB HID defines a **boot protocol** with fixed report formats:

- **Boot Keyboard:** 8 bytes — modifier byte + reserved + 6 keycode bytes
- **Boot Mouse:** 3 bytes — buttons + X delta + Y delta

The input subsystem uses boot protocol during early boot (before full HID descriptor parsing is available) and switches to report protocol for full functionality.

### 3.3 Platform-Specific Drivers

| Platform | Keyboard | Pointing | Touch | Gamepad |
|---|---|---|---|---|
| **QEMU** | virtio-keyboard | virtio-tablet (abs) | N/A | N/A |
| **Pi 4/5** | USB HID | USB HID, GPIO buttons | I2C/SPI touch controller | USB HID |
| **Apple Silicon** | Apple HID (SPI) | Apple HID trackpad (SPI) | N/A (external USB) | USB/BT HID |

#### QEMU (Development Target)

QEMU provides paravirtualized input devices via VirtIO-input (see §3.4). For GUI integration, QEMU also provides:

- `virtio-keyboard-device` — keyboard with evdev keycodes
- `virtio-tablet-device` — absolute pointing device (0-32767 range per axis) for seamless host-guest cursor integration

QEMU flags:

```text
-device virtio-keyboard-device
-device virtio-tablet-device
```

#### Raspberry Pi 4/5

Physical input on Pi boards comes through USB ports and GPIO:

- **USB HID devices:** Standard keyboard, mouse, gamepad via USB ports
- **GPIO buttons:** Hat-switch style buttons for embedded/kiosk configurations (optional)
- **I2C/SPI touchscreens:** Official Raspberry Pi Touch Display uses DSI + I2C for touch events
- **Bluetooth HID:** Built-in Bluetooth supports wireless keyboards, mice, gamepads

#### Apple Silicon

Apple Silicon Macs use SPI-connected internal keyboard and trackpad with Apple's proprietary HID extensions:

- **Internal keyboard:** SPI bus, Apple HID descriptor with function key mappings and Touch Bar (if present)
- **Internal trackpad:** SPI bus, Force Touch pressure sensing, multi-touch up to 5 fingers
- **External devices:** Standard USB/Bluetooth HID via Thunderbolt/USB-C ports

### 3.4 VirtIO-Input Driver

VirtIO-input is defined in VirtIO specification §5.8 and is the primary development target for QEMU.

#### Transport

VirtIO-input uses the same MMIO transport as AIOS's existing VirtIO-blk driver (`kernel/src/drivers/virtio_blk.rs`). The transport code (MMIO register access, virtqueue management) is shared.

#### Virtqueues

| Queue | Direction | Purpose |
|---|---|---|
| `eventq` | Device → Driver | Input events (pre-populated with empty buffers) |
| `statusq` | Driver → Device | LED status feedback |

#### Event Format

```rust
/// VirtIO input event — matches Linux evdev format
#[repr(C)]
pub struct VirtioInputEvent {
    pub event_type: u16,  // EV_KEY, EV_REL, EV_ABS, EV_SYN, etc.
    pub code: u16,        // keycode, axis code, etc.
    pub value: u32,       // key state (0/1/2), axis value
}
```

Events arrive in atomic groups bounded by `EV_SYN / SYN_REPORT`. A single mouse movement with a button click produces:

```text
EV_REL  REL_X     +5        ← X axis moved
EV_REL  REL_Y     -3        ← Y axis moved
EV_KEY  BTN_LEFT  1         ← left button pressed
EV_SYN  SYN_REPORT 0        ← end of atomic group
```

#### Device Configuration

The device's configuration space provides:

- **Name** (`VIRTIO_INPUT_CFG_ID_NAME`): human-readable device name
- **Serial** (`VIRTIO_INPUT_CFG_ID_SERIAL`): serial number
- **Device IDs** (`VIRTIO_INPUT_CFG_ID_DEVIDS`): USB vendor/product/version
- **Property bits** (`VIRTIO_INPUT_CFG_PROP_BITS`): device properties (e.g., `INPUT_PROP_POINTER`, `INPUT_PROP_DIRECT`)
- **Event bitmaps** (`VIRTIO_INPUT_CFG_EV_BITS`): supported event types and codes (mirrors Linux `EVIOCGBIT`)
- **Absolute axis info** (`VIRTIO_INPUT_CFG_ABS_INFO`): min/max/fuzz/flat/resolution per abs axis

#### Driver Implementation

```text
1. Probe VirtIO MMIO region (scan 0x0A00_0000–0x0A00_3E00)
2. Check device ID (device_id field in VirtIO header)
3. Read device configuration (name, capabilities, abs axis info)
4. Negotiate features
5. Set up eventq: allocate buffers, populate available ring
6. Set up statusq: for LED feedback
7. Enable device (DRIVER_OK)
8. Poll eventq (or IRQ) for input events
9. Translate VirtioInputEvent → RawInputEvent
10. Recycle used buffers back to available ring
```

### 3.5 Bluetooth HID

Bluetooth HID devices use the **HID over GATT** (HOGP) profile for BLE devices and the classic **HID Profile** for BR/EDR devices.

#### Architecture

```text
Bluetooth Stack (userspace)
    │
    ├── L2CAP channel (classic HID) or GATT service (BLE HOGP)
    │
    ▼
Bluetooth HID Translator
    │
    ├── Parse HID report descriptor (same parser as USB HID)
    ├── Translate HID reports to RawInputEvent
    │
    ▼
Input Subsystem Service (same pipeline as USB/VirtIO devices)
```

Key differences from USB HID:

- **Latency:** Bluetooth adds 5-20ms latency (connection interval dependent). BLE HOGP typically 7.5-30ms.
- **Power management:** Bluetooth HID devices enter sniff mode when idle. The input subsystem must handle reconnection delays.
- **Pairing:** Requires pairing and bonding. The security subsystem manages pairing authorization.
- **Connection management:** Devices may disconnect and reconnect. The input subsystem must handle seamless reconnection.

### 3.6 Accessibility Devices

#### 3.6.1 Braille Displays

Braille displays are bidirectional devices — they output Braille cells and accept input from routing keys, navigation keys, and sometimes chord input keyboards.

```rust
/// Braille display device interface
pub trait BrailleDevice: InputDevice {
    /// Number of Braille cells on the display
    fn cell_count(&self) -> u8;

    /// Write Braille dots to display
    fn write_cells(&self, cells: &[BrailleCell]);

    /// Input events:
    /// - Routing keys (one per cell): EV_KEY with KEY_BRAILLE_DOT_1..8
    /// - Navigation keys: EV_KEY with vendor-specific codes
    /// - Chord input: EV_KEY with simultaneous dot combinations
}
```

**Transport support:**

- **USB HID (usage page 0x41):** Standard HID Braille, supported natively by HID parser
- **Serial protocols:** Many Braille displays use proprietary serial protocols. A userspace Braille driver agent handles protocol translation and produces standard InputEvents.
- **Bluetooth:** Bluetooth serial (RFCOMM) or Bluetooth HID

The accessibility service (see [accessibility.md](../../experience/accessibility.md)) consumes Braille input events and manages the display output. The input subsystem provides the device channel; the accessibility service provides the intelligence.

#### 3.6.2 Switch Devices

Switch devices produce simple binary events (press/release). They include:

- **USB HID switches:** Standard button usage page, one or two buttons
- **Keyboard-as-switch:** Any key can be configured as a switch (Space, Enter, or any key)
- **Sip-and-puff:** Pneumatic switches that detect inhale/exhale as two switch actions
- **Proximity sensors:** IR or capacitive sensors that detect approach without contact

The input subsystem maps all switch types to a unified `SwitchEvent`:

```rust
pub enum SwitchEvent {
    /// Primary switch activated (press, sip, approach)
    Primary(SwitchState),
    /// Secondary switch activated (release-triggered, puff, withdraw)
    Secondary(SwitchState),
}

pub enum SwitchState {
    Activated,
    Released,
}
```

The switch scanning engine (see [ai.md](./ai.md) §10.6) consumes these events and generates synthetic pointer/selection events.

#### 3.6.3 Eye Trackers

Eye tracking devices produce gaze coordinates and optional pupil data:

```rust
/// Eye tracker device interface
pub trait EyeTrackerDevice: InputDevice {
    /// Current calibration state
    fn calibration_state(&self) -> CalibrationState;

    /// Gaze events produced:
    /// - ABS_X, ABS_Y: calibrated gaze position (screen coordinates)
    /// - ABS_PRESSURE: gaze confidence (0-1000)
    /// - ABS_MISC: fixation duration (ms)
}

pub enum CalibrationState {
    Uncalibrated,
    Calibrating { points_remaining: u8 },
    Calibrated { accuracy_deg: f32 },
}
```

The gaze-to-selection engine (dwell click, smooth pursuit) runs in the input subsystem transform pipeline. The eye tracker driver provides raw calibrated coordinates; the transform converts them to pointer events or selection events.

### 3.7 Device Discovery & Hotplug

Device discovery follows the subsystem framework's hotplug pattern (see [subsystem-framework.md](../subsystem-framework.md) §11):

```text
Device Connected
    │
    ▼
┌────────────────────┐
│  USB Enumeration   │     Parse device descriptor, identify class
│  (kernel USB stack) │
└──────────┬─────────┘
           │
           ▼
┌────────────────────┐
│  Class Routing     │     UsbClass::HID → Input Subsystem
│  (subsystem fw)    │     UsbClass::Audio → Audio Subsystem
└──────────┬─────────┘     UsbClass::MassStorage → Storage
           │
           ▼
┌────────────────────┐
│  BadUSB Pre-Screen │     1. Parse HID descriptor
│  (input subsystem) │     2. Check class combination anomalies
│                    │     3. Analyze first 100 reports (timing entropy)
│                    │     4. Run traffic fingerprint CNN
└──────────┬─────────┘
           │
     ┌─────┴──────┐
     │             │
  PASS           FAIL
     │             │
     ▼             ▼
  Grant          Block device
  InputDevice    Alert user
  capability     Log to audit
     │
     ▼
┌────────────────────┐
│  Device Registry   │     Assign device ID
│  (input subsystem) │     Assign to seat
│                    │     Create event channel
│                    │     Notify compositor
└────────────────────┘
```

#### Device Authorization Policy

The input subsystem maintains a device authorization database:

| Policy | Behavior |
|---|---|
| **Known device** (previously authorized, matching VID/PID/serial) | Auto-grant InputDevice capability |
| **New device, standard class** (keyboard-only or mouse-only) | Grant after BadUSB pre-screen passes |
| **New device, unusual class combination** (keyboard + mass storage) | Block; prompt user for authorization |
| **Device fails behavioral analysis** (timing entropy anomaly) | Block; alert user; log to audit |
| **Device under rate-limit** (>1000 events/sec from "keyboard") | Revoke InputDevice capability; alert user |

#### Restartability

Input drivers run as userspace agents. If a driver crashes:

1. The service manager detects the crash (heartbeat timeout)
2. The service manager restarts the driver agent
3. The input subsystem re-initializes the device (re-read descriptors, re-populate virtqueues)
4. The event stream resumes — applications see a brief gap but no data corruption

Gesture recognizer state and adaptive model parameters are persisted to the user's input profile space, so they survive driver restarts. See [ai.md](./ai.md) §10.2 for adaptive state persistence.
