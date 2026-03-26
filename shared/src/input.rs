//! VirtIO-input wire-format types, evdev constants, and typed input events.
//!
//! Wire-format types (`VirtioInputEvent`, `VirtioInputAbsInfo`) match the VirtIO
//! spec §5.8 exactly. Typed events (`InputEvent`, `KeyCode`, etc.) are AIOS-native
//! abstractions consumed by the compositor and input subsystem.

// ---------------------------------------------------------------------------
// VirtIO-input wire-format types (VirtIO spec §5.8)
// ---------------------------------------------------------------------------

/// VirtIO-input event — 8-byte wire format from the device eventq.
///
/// The device fills these structs into pre-allocated DMA buffers. The driver
/// reads them from the used ring after the device signals completion.
///
/// Layout matches Linux `struct input_event` (without timestamp fields):
/// - `event_type`: EV_KEY, EV_ABS, EV_SYN, etc.
/// - `code`: keycode, axis code, or sync subtype
/// - `value`: key state (0/1/2), axis position, or sync data
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtioInputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: u32,
}

const _: () = assert!(core::mem::size_of::<VirtioInputEvent>() == 8);

/// VirtIO-input absolute axis info — returned by config select CFG_ABS_INFO.
///
/// Describes the range and resolution of an absolute axis (e.g., tablet X/Y).
/// For QEMU's virtio-tablet: min=0, max=32767, fuzz=0, flat=0, res=0.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtioInputAbsInfo {
    pub min: u32,
    pub max: u32,
    pub fuzz: u32,
    pub flat: u32,
    pub res: u32,
}

const _: () = assert!(core::mem::size_of::<VirtioInputAbsInfo>() == 20);

/// Identifies a specific input device (index into the driver's device array).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputDeviceId(pub u8);

// ---------------------------------------------------------------------------
// Evdev event type constants
// ---------------------------------------------------------------------------

/// Synchronization event (group boundary).
pub const EV_SYN: u16 = 0x00;
/// Key/button state change.
pub const EV_KEY: u16 = 0x01;
/// Relative axis (mouse delta).
pub const EV_REL: u16 = 0x02;
/// Absolute axis (tablet/touch position).
pub const EV_ABS: u16 = 0x03;

/// End of atomic event group.
pub const SYN_REPORT: u16 = 0;

// ---------------------------------------------------------------------------
// Evdev key code constants (Linux input-event-codes.h)
// ---------------------------------------------------------------------------

pub const KEY_ESC: u16 = 1;
pub const KEY_1: u16 = 2;
pub const KEY_2: u16 = 3;
pub const KEY_3: u16 = 4;
pub const KEY_4: u16 = 5;
pub const KEY_5: u16 = 6;
pub const KEY_6: u16 = 7;
pub const KEY_7: u16 = 8;
pub const KEY_8: u16 = 9;
pub const KEY_9: u16 = 10;
pub const KEY_0: u16 = 11;
pub const KEY_MINUS: u16 = 12;
pub const KEY_EQUAL: u16 = 13;
pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_TAB: u16 = 15;
pub const KEY_Q: u16 = 16;
pub const KEY_W: u16 = 17;
pub const KEY_E: u16 = 18;
pub const KEY_R: u16 = 19;
pub const KEY_T: u16 = 20;
pub const KEY_Y: u16 = 21;
pub const KEY_U: u16 = 22;
pub const KEY_I: u16 = 23;
pub const KEY_O: u16 = 24;
pub const KEY_P: u16 = 25;
pub const KEY_LEFTBRACE: u16 = 26;
pub const KEY_RIGHTBRACE: u16 = 27;
pub const KEY_ENTER: u16 = 28;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_A: u16 = 30;
pub const KEY_S: u16 = 31;
pub const KEY_D: u16 = 32;
pub const KEY_F: u16 = 33;
pub const KEY_G: u16 = 34;
pub const KEY_H: u16 = 35;
pub const KEY_J: u16 = 36;
pub const KEY_K: u16 = 37;
pub const KEY_L: u16 = 38;
pub const KEY_SEMICOLON: u16 = 39;
pub const KEY_APOSTROPHE: u16 = 40;
pub const KEY_GRAVE: u16 = 41;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_BACKSLASH: u16 = 43;
pub const KEY_Z: u16 = 44;
pub const KEY_X: u16 = 45;
pub const KEY_C: u16 = 46;
pub const KEY_V: u16 = 47;
pub const KEY_B: u16 = 48;
pub const KEY_N: u16 = 49;
pub const KEY_M: u16 = 50;
pub const KEY_COMMA: u16 = 51;
pub const KEY_DOT: u16 = 52;
pub const KEY_SLASH: u16 = 53;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_SPACE: u16 = 57;
pub const KEY_CAPSLOCK: u16 = 58;
pub const KEY_F1: u16 = 59;
pub const KEY_F2: u16 = 60;
pub const KEY_F3: u16 = 61;
pub const KEY_F4: u16 = 62;
pub const KEY_F5: u16 = 63;
pub const KEY_F6: u16 = 64;
pub const KEY_F7: u16 = 65;
pub const KEY_F8: u16 = 66;
pub const KEY_F9: u16 = 67;
pub const KEY_F10: u16 = 68;
pub const KEY_F11: u16 = 87;
pub const KEY_F12: u16 = 88;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_HOME: u16 = 102;
pub const KEY_UP: u16 = 103;
pub const KEY_PAGEUP: u16 = 104;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const KEY_END: u16 = 107;
pub const KEY_DOWN: u16 = 108;
pub const KEY_PAGEDOWN: u16 = 109;
pub const KEY_DELETE: u16 = 111;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;

// ---------------------------------------------------------------------------
// Evdev button constants
// ---------------------------------------------------------------------------

pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

// ---------------------------------------------------------------------------
// Evdev absolute axis constants
// ---------------------------------------------------------------------------

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;

// ---------------------------------------------------------------------------
// VirtIO-input config select constants (VirtIO spec §5.8.2)
// ---------------------------------------------------------------------------

pub const VIRTIO_INPUT_CFG_UNSET: u8 = 0x00;
pub const VIRTIO_INPUT_CFG_ID_NAME: u8 = 0x01;
pub const VIRTIO_INPUT_CFG_ID_SERIAL: u8 = 0x02;
pub const VIRTIO_INPUT_CFG_ID_DEVIDS: u8 = 0x03;
pub const VIRTIO_INPUT_CFG_PROP_BITS: u8 = 0x10;
pub const VIRTIO_INPUT_CFG_EV_BITS: u8 = 0x11;
pub const VIRTIO_INPUT_CFG_ABS_INFO: u8 = 0x12;

// ---------------------------------------------------------------------------
// Typed input events (AIOS-native abstractions)
// ---------------------------------------------------------------------------

/// High-level input event produced by the input subsystem.
///
/// Keyboard events are emitted immediately on each EV_KEY. Pointer events
/// accumulate ABS_X/ABS_Y/button state and emit on EV_SYN/SYN_REPORT
/// (correct evdev atomic grouping model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Key press, release, or repeat.
    Keyboard {
        key: KeyCode,
        state: KeyState,
        modifiers: Modifiers,
    },
    /// Pointer position and/or button state (emitted on SYN_REPORT).
    Pointer {
        x: u32,
        y: u32,
        button: Option<MouseButton>,
        state: Option<ButtonState>,
    },
}

/// Logical key identifier, translated from evdev keycodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    Enter,
    Esc,
    Backspace,
    Tab,
    Space,
    Minus,
    Equal,
    LeftBracket,
    RightBracket,
    Backslash,
    Semicolon,
    Apostrophe,
    Grave,
    Comma,
    Period,
    Slash,
    CapsLock,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Up,
    Down,
    Left,
    Right,
    LeftShift,
    RightShift,
    LeftCtrl,
    RightCtrl,
    LeftAlt,
    RightAlt,
    LeftSuper,
    RightSuper,
    /// Unmapped evdev keycode.
    Unknown(u16),
}

impl KeyCode {
    /// Translate an evdev keycode to a `KeyCode`.
    pub fn from_evdev(code: u16) -> Self {
        match code {
            KEY_A => KeyCode::A,
            KEY_B => KeyCode::B,
            KEY_C => KeyCode::C,
            KEY_D => KeyCode::D,
            KEY_E => KeyCode::E,
            KEY_F => KeyCode::F,
            KEY_G => KeyCode::G,
            KEY_H => KeyCode::H,
            KEY_I => KeyCode::I,
            KEY_J => KeyCode::J,
            KEY_K => KeyCode::K,
            KEY_L => KeyCode::L,
            KEY_M => KeyCode::M,
            KEY_N => KeyCode::N,
            KEY_O => KeyCode::O,
            KEY_P => KeyCode::P,
            KEY_Q => KeyCode::Q,
            KEY_R => KeyCode::R,
            KEY_S => KeyCode::S,
            KEY_T => KeyCode::T,
            KEY_U => KeyCode::U,
            KEY_V => KeyCode::V,
            KEY_W => KeyCode::W,
            KEY_X => KeyCode::X,
            KEY_Y => KeyCode::Y,
            KEY_Z => KeyCode::Z,
            KEY_0 => KeyCode::Num0,
            KEY_1 => KeyCode::Num1,
            KEY_2 => KeyCode::Num2,
            KEY_3 => KeyCode::Num3,
            KEY_4 => KeyCode::Num4,
            KEY_5 => KeyCode::Num5,
            KEY_6 => KeyCode::Num6,
            KEY_7 => KeyCode::Num7,
            KEY_8 => KeyCode::Num8,
            KEY_9 => KeyCode::Num9,
            KEY_ENTER => KeyCode::Enter,
            KEY_ESC => KeyCode::Esc,
            KEY_BACKSPACE => KeyCode::Backspace,
            KEY_TAB => KeyCode::Tab,
            KEY_SPACE => KeyCode::Space,
            KEY_MINUS => KeyCode::Minus,
            KEY_EQUAL => KeyCode::Equal,
            KEY_LEFTBRACE => KeyCode::LeftBracket,
            KEY_RIGHTBRACE => KeyCode::RightBracket,
            KEY_BACKSLASH => KeyCode::Backslash,
            KEY_SEMICOLON => KeyCode::Semicolon,
            KEY_APOSTROPHE => KeyCode::Apostrophe,
            KEY_GRAVE => KeyCode::Grave,
            KEY_COMMA => KeyCode::Comma,
            KEY_DOT => KeyCode::Period,
            KEY_SLASH => KeyCode::Slash,
            KEY_CAPSLOCK => KeyCode::CapsLock,
            KEY_DELETE => KeyCode::Delete,
            KEY_HOME => KeyCode::Home,
            KEY_END => KeyCode::End,
            KEY_PAGEUP => KeyCode::PageUp,
            KEY_PAGEDOWN => KeyCode::PageDown,
            KEY_F1 => KeyCode::F1,
            KEY_F2 => KeyCode::F2,
            KEY_F3 => KeyCode::F3,
            KEY_F4 => KeyCode::F4,
            KEY_F5 => KeyCode::F5,
            KEY_F6 => KeyCode::F6,
            KEY_F7 => KeyCode::F7,
            KEY_F8 => KeyCode::F8,
            KEY_F9 => KeyCode::F9,
            KEY_F10 => KeyCode::F10,
            KEY_F11 => KeyCode::F11,
            KEY_F12 => KeyCode::F12,
            KEY_UP => KeyCode::Up,
            KEY_DOWN => KeyCode::Down,
            KEY_LEFT => KeyCode::Left,
            KEY_RIGHT => KeyCode::Right,
            KEY_LEFTSHIFT => KeyCode::LeftShift,
            KEY_RIGHTSHIFT => KeyCode::RightShift,
            KEY_LEFTCTRL => KeyCode::LeftCtrl,
            KEY_RIGHTCTRL => KeyCode::RightCtrl,
            KEY_LEFTALT => KeyCode::LeftAlt,
            KEY_RIGHTALT => KeyCode::RightAlt,
            KEY_LEFTMETA => KeyCode::LeftSuper,
            KEY_RIGHTMETA => KeyCode::RightSuper,
            other => KeyCode::Unknown(other),
        }
    }
}

/// Key state from evdev value field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Released,
    Pressed,
    Repeat,
}

impl KeyState {
    /// Convert evdev value (0=released, 1=pressed, 2=repeat) to KeyState.
    pub fn from_value(value: u32) -> Option<Self> {
        match value {
            0 => Some(KeyState::Released),
            1 => Some(KeyState::Pressed),
            2 => Some(KeyState::Repeat),
            _ => None,
        }
    }
}

/// Modifier key bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const NONE: u8 = 0;
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
    pub const SUPER: u8 = 8;

    /// Check if a modifier flag is set.
    pub const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }
}

/// Mouse button identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Button press/release state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Pressed,
    Released,
}

// ---------------------------------------------------------------------------
// US-QWERTY keymap
// ---------------------------------------------------------------------------

/// US-QWERTY scancode-to-character mapping.
///
/// Index by evdev keycode (0–127). Value is `Some((unshifted, shifted))` for
/// printable keys, `None` for non-printable (modifiers, function keys, etc.).
pub const KEYMAP_US: [Option<(char, char)>; 128] = {
    let mut map: [Option<(char, char)>; 128] = [None; 128];
    map[KEY_1 as usize] = Some(('1', '!'));
    map[KEY_2 as usize] = Some(('2', '@'));
    map[KEY_3 as usize] = Some(('3', '#'));
    map[KEY_4 as usize] = Some(('4', '$'));
    map[KEY_5 as usize] = Some(('5', '%'));
    map[KEY_6 as usize] = Some(('6', '^'));
    map[KEY_7 as usize] = Some(('7', '&'));
    map[KEY_8 as usize] = Some(('8', '*'));
    map[KEY_9 as usize] = Some(('9', '('));
    map[KEY_0 as usize] = Some(('0', ')'));
    map[KEY_MINUS as usize] = Some(('-', '_'));
    map[KEY_EQUAL as usize] = Some(('=', '+'));
    map[KEY_TAB as usize] = Some(('\t', '\t'));
    map[KEY_Q as usize] = Some(('q', 'Q'));
    map[KEY_W as usize] = Some(('w', 'W'));
    map[KEY_E as usize] = Some(('e', 'E'));
    map[KEY_R as usize] = Some(('r', 'R'));
    map[KEY_T as usize] = Some(('t', 'T'));
    map[KEY_Y as usize] = Some(('y', 'Y'));
    map[KEY_U as usize] = Some(('u', 'U'));
    map[KEY_I as usize] = Some(('i', 'I'));
    map[KEY_O as usize] = Some(('o', 'O'));
    map[KEY_P as usize] = Some(('p', 'P'));
    map[KEY_LEFTBRACE as usize] = Some(('[', '{'));
    map[KEY_RIGHTBRACE as usize] = Some((']', '}'));
    map[KEY_ENTER as usize] = Some(('\n', '\n'));
    map[KEY_A as usize] = Some(('a', 'A'));
    map[KEY_S as usize] = Some(('s', 'S'));
    map[KEY_D as usize] = Some(('d', 'D'));
    map[KEY_F as usize] = Some(('f', 'F'));
    map[KEY_G as usize] = Some(('g', 'G'));
    map[KEY_H as usize] = Some(('h', 'H'));
    map[KEY_J as usize] = Some(('j', 'J'));
    map[KEY_K as usize] = Some(('k', 'K'));
    map[KEY_L as usize] = Some(('l', 'L'));
    map[KEY_SEMICOLON as usize] = Some((';', ':'));
    map[KEY_APOSTROPHE as usize] = Some(('\'', '"'));
    map[KEY_GRAVE as usize] = Some(('`', '~'));
    map[KEY_BACKSLASH as usize] = Some(('\\', '|'));
    map[KEY_Z as usize] = Some(('z', 'Z'));
    map[KEY_X as usize] = Some(('x', 'X'));
    map[KEY_C as usize] = Some(('c', 'C'));
    map[KEY_V as usize] = Some(('v', 'V'));
    map[KEY_B as usize] = Some(('b', 'B'));
    map[KEY_N as usize] = Some(('n', 'N'));
    map[KEY_M as usize] = Some(('m', 'M'));
    map[KEY_COMMA as usize] = Some((',', '<'));
    map[KEY_DOT as usize] = Some(('.', '>'));
    map[KEY_SLASH as usize] = Some(('/', '?'));
    map[KEY_SPACE as usize] = Some((' ', ' '));
    map
};

// ---------------------------------------------------------------------------
// Coordinate conversion
// ---------------------------------------------------------------------------

/// Convert absolute axis value to display coordinate.
///
/// Formula: `abs_val * display_dim / (abs_max + 1)`.
/// Using `abs_max + 1` avoids off-by-one at the maximum value:
/// `32767 * 1280 / 32768 = 1279` (last pixel).
///
/// # Overflow safety
/// `u32 * u32` can overflow for very large display dimensions, but
/// `32767 * 131072 = 4,294,836,224` is near u32::MAX. For any realistic
/// display (< 100K pixels), this is safe.
pub fn abs_to_display(abs_val: u32, abs_max: u32, display_dim: u32) -> u32 {
    if abs_max == 0 || display_dim == 0 {
        return 0;
    }
    // Clamp abs_val to abs_max to handle buggy/malicious device data.
    let clamped = if abs_val > abs_max { abs_max } else { abs_val };
    // Use u64 intermediate to avoid overflow for large values.
    let result = (clamped as u64) * (display_dim as u64) / (abs_max as u64 + 1);
    result as u32
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Wire-format size tests ---

    #[test]
    fn virtio_input_event_size() {
        assert_eq!(core::mem::size_of::<VirtioInputEvent>(), 8);
    }

    #[test]
    fn virtio_input_abs_info_size() {
        assert_eq!(core::mem::size_of::<VirtioInputAbsInfo>(), 20);
    }

    #[test]
    fn virtio_input_event_repr_c_offsets() {
        // Verify field offsets match VirtIO spec wire format.
        let event = VirtioInputEvent {
            event_type: 0x0102,
            code: 0x0304,
            value: 0x05060708,
        };
        let ptr = &event as *const VirtioInputEvent as *const u8;
        unsafe {
            // event_type at offset 0 (2 bytes LE)
            assert_eq!(*ptr, 0x02);
            assert_eq!(*ptr.add(1), 0x01);
            // code at offset 2 (2 bytes LE)
            assert_eq!(*ptr.add(2), 0x04);
            assert_eq!(*ptr.add(3), 0x03);
            // value at offset 4 (4 bytes LE)
            assert_eq!(*ptr.add(4), 0x08);
            assert_eq!(*ptr.add(5), 0x07);
            assert_eq!(*ptr.add(6), 0x06);
            assert_eq!(*ptr.add(7), 0x05);
        }
    }

    // --- Evdev constant tests ---

    #[test]
    fn evdev_event_type_values() {
        assert_eq!(EV_SYN, 0x00);
        assert_eq!(EV_KEY, 0x01);
        assert_eq!(EV_REL, 0x02);
        assert_eq!(EV_ABS, 0x03);
        assert_eq!(SYN_REPORT, 0);
    }

    #[test]
    fn evdev_key_code_values() {
        assert_eq!(KEY_ESC, 1);
        assert_eq!(KEY_A, 30);
        assert_eq!(KEY_Z, 44);
        assert_eq!(KEY_ENTER, 28);
        assert_eq!(KEY_SPACE, 57);
        assert_eq!(KEY_F1, 59);
        assert_eq!(KEY_F12, 88);
        assert_eq!(KEY_LEFTSHIFT, 42);
        assert_eq!(KEY_LEFTCTRL, 29);
        assert_eq!(KEY_LEFTALT, 56);
        assert_eq!(KEY_LEFTMETA, 125);
    }

    #[test]
    fn evdev_button_values() {
        assert_eq!(BTN_LEFT, 0x110);
        assert_eq!(BTN_RIGHT, 0x111);
        assert_eq!(BTN_MIDDLE, 0x112);
    }

    #[test]
    fn evdev_abs_values() {
        assert_eq!(ABS_X, 0x00);
        assert_eq!(ABS_Y, 0x01);
    }

    // --- Config select constant tests ---

    #[test]
    fn config_select_values() {
        assert_eq!(VIRTIO_INPUT_CFG_UNSET, 0x00);
        assert_eq!(VIRTIO_INPUT_CFG_ID_NAME, 0x01);
        assert_eq!(VIRTIO_INPUT_CFG_ID_SERIAL, 0x02);
        assert_eq!(VIRTIO_INPUT_CFG_ID_DEVIDS, 0x03);
        assert_eq!(VIRTIO_INPUT_CFG_PROP_BITS, 0x10);
        assert_eq!(VIRTIO_INPUT_CFG_EV_BITS, 0x11);
        assert_eq!(VIRTIO_INPUT_CFG_ABS_INFO, 0x12);
    }

    // --- KeyCode tests ---

    #[test]
    fn keycode_from_evdev_letters() {
        assert_eq!(KeyCode::from_evdev(KEY_A), KeyCode::A);
        assert_eq!(KeyCode::from_evdev(KEY_Z), KeyCode::Z);
        assert_eq!(KeyCode::from_evdev(KEY_M), KeyCode::M);
    }

    #[test]
    fn keycode_from_evdev_numbers() {
        assert_eq!(KeyCode::from_evdev(KEY_0), KeyCode::Num0);
        assert_eq!(KeyCode::from_evdev(KEY_1), KeyCode::Num1);
        assert_eq!(KeyCode::from_evdev(KEY_9), KeyCode::Num9);
    }

    #[test]
    fn keycode_from_evdev_special() {
        assert_eq!(KeyCode::from_evdev(KEY_ENTER), KeyCode::Enter);
        assert_eq!(KeyCode::from_evdev(KEY_ESC), KeyCode::Esc);
        assert_eq!(KeyCode::from_evdev(KEY_SPACE), KeyCode::Space);
        assert_eq!(KeyCode::from_evdev(KEY_TAB), KeyCode::Tab);
        assert_eq!(KeyCode::from_evdev(KEY_BACKSPACE), KeyCode::Backspace);
    }

    #[test]
    fn keycode_from_evdev_function_keys() {
        assert_eq!(KeyCode::from_evdev(KEY_F1), KeyCode::F1);
        assert_eq!(KeyCode::from_evdev(KEY_F12), KeyCode::F12);
    }

    #[test]
    fn keycode_from_evdev_arrows() {
        assert_eq!(KeyCode::from_evdev(KEY_UP), KeyCode::Up);
        assert_eq!(KeyCode::from_evdev(KEY_DOWN), KeyCode::Down);
        assert_eq!(KeyCode::from_evdev(KEY_LEFT), KeyCode::Left);
        assert_eq!(KeyCode::from_evdev(KEY_RIGHT), KeyCode::Right);
    }

    #[test]
    fn keycode_from_evdev_modifiers() {
        assert_eq!(KeyCode::from_evdev(KEY_LEFTSHIFT), KeyCode::LeftShift);
        assert_eq!(KeyCode::from_evdev(KEY_RIGHTSHIFT), KeyCode::RightShift);
        assert_eq!(KeyCode::from_evdev(KEY_LEFTCTRL), KeyCode::LeftCtrl);
        assert_eq!(KeyCode::from_evdev(KEY_LEFTALT), KeyCode::LeftAlt);
        assert_eq!(KeyCode::from_evdev(KEY_LEFTMETA), KeyCode::LeftSuper);
    }

    #[test]
    fn keycode_from_evdev_unknown() {
        assert_eq!(KeyCode::from_evdev(999), KeyCode::Unknown(999));
        assert_eq!(KeyCode::from_evdev(0xFFFF), KeyCode::Unknown(0xFFFF));
    }

    // --- KeyState tests ---

    #[test]
    fn keystate_from_value() {
        assert_eq!(KeyState::from_value(0), Some(KeyState::Released));
        assert_eq!(KeyState::from_value(1), Some(KeyState::Pressed));
        assert_eq!(KeyState::from_value(2), Some(KeyState::Repeat));
        assert_eq!(KeyState::from_value(3), None);
        assert_eq!(KeyState::from_value(u32::MAX), None);
    }

    // --- Modifiers tests ---

    #[test]
    fn modifiers_empty() {
        let m = Modifiers(Modifiers::NONE);
        assert!(!m.contains(Modifiers::SHIFT));
        assert!(!m.contains(Modifiers::CTRL));
        assert!(!m.contains(Modifiers::ALT));
        assert!(!m.contains(Modifiers::SUPER));
    }

    #[test]
    fn modifiers_combine() {
        let m = Modifiers(Modifiers::SHIFT | Modifiers::CTRL);
        assert!(m.contains(Modifiers::SHIFT));
        assert!(m.contains(Modifiers::CTRL));
        assert!(!m.contains(Modifiers::ALT));
        assert!(!m.contains(Modifiers::SUPER));
    }

    #[test]
    fn modifiers_all() {
        let m = Modifiers(Modifiers::SHIFT | Modifiers::CTRL | Modifiers::ALT | Modifiers::SUPER);
        assert!(m.contains(Modifiers::SHIFT));
        assert!(m.contains(Modifiers::CTRL));
        assert!(m.contains(Modifiers::ALT));
        assert!(m.contains(Modifiers::SUPER));
    }

    // --- MouseButton / ButtonState tests ---

    #[test]
    fn mouse_button_discrimination() {
        assert_ne!(MouseButton::Left, MouseButton::Right);
        assert_ne!(MouseButton::Right, MouseButton::Middle);
        assert_ne!(MouseButton::Left, MouseButton::Middle);
    }

    #[test]
    fn button_state_discrimination() {
        assert_ne!(ButtonState::Pressed, ButtonState::Released);
    }

    // --- InputDeviceId tests ---

    #[test]
    fn input_device_id_equality() {
        assert_eq!(InputDeviceId(0), InputDeviceId(0));
        assert_ne!(InputDeviceId(0), InputDeviceId(1));
    }

    #[test]
    fn input_device_id_copy() {
        let id = InputDeviceId(5);
        let id2 = id;
        assert_eq!(id, id2);
    }

    // --- InputEvent tests ---

    #[test]
    fn input_event_keyboard_construct() {
        let ev = InputEvent::Keyboard {
            key: KeyCode::A,
            state: KeyState::Pressed,
            modifiers: Modifiers(Modifiers::NONE),
        };
        match ev {
            InputEvent::Keyboard {
                key,
                state,
                modifiers,
            } => {
                assert_eq!(key, KeyCode::A);
                assert_eq!(state, KeyState::Pressed);
                assert_eq!(modifiers, Modifiers(0));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn input_event_pointer_construct() {
        let ev = InputEvent::Pointer {
            x: 640,
            y: 400,
            button: None,
            state: None,
        };
        match ev {
            InputEvent::Pointer {
                x,
                y,
                button,
                state,
            } => {
                assert_eq!(x, 640);
                assert_eq!(y, 400);
                assert!(button.is_none());
                assert!(state.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn input_event_pointer_with_button() {
        let ev = InputEvent::Pointer {
            x: 100,
            y: 200,
            button: Some(MouseButton::Left),
            state: Some(ButtonState::Pressed),
        };
        match ev {
            InputEvent::Pointer { button, state, .. } => {
                assert_eq!(button, Some(MouseButton::Left));
                assert_eq!(state, Some(ButtonState::Pressed));
            }
            _ => panic!("wrong variant"),
        }
    }

    // --- Keymap tests ---

    #[test]
    fn keymap_letter_a() {
        assert_eq!(KEYMAP_US[KEY_A as usize], Some(('a', 'A')));
    }

    #[test]
    fn keymap_number_1() {
        assert_eq!(KEYMAP_US[KEY_1 as usize], Some(('1', '!')));
    }

    #[test]
    fn keymap_space() {
        assert_eq!(KEYMAP_US[KEY_SPACE as usize], Some((' ', ' ')));
    }

    #[test]
    fn keymap_reserved_zero() {
        // Index 0 (no evdev keycode 0 for printable) should be None.
        assert_eq!(KEYMAP_US[0], None);
    }

    #[test]
    fn keymap_punctuation() {
        assert_eq!(KEYMAP_US[KEY_SEMICOLON as usize], Some((';', ':')));
        assert_eq!(KEYMAP_US[KEY_COMMA as usize], Some((',', '<')));
        assert_eq!(KEYMAP_US[KEY_SLASH as usize], Some(('/', '?')));
    }

    // --- Coordinate conversion tests ---

    #[test]
    fn abs_to_display_zero() {
        assert_eq!(abs_to_display(0, 32767, 1280), 0);
    }

    #[test]
    fn abs_to_display_half() {
        // 16384 * 1280 / 32768 = 640
        assert_eq!(abs_to_display(16384, 32767, 1280), 640);
    }

    #[test]
    fn abs_to_display_max() {
        // 32767 * 1280 / 32768 = 1279 (last pixel)
        assert_eq!(abs_to_display(32767, 32767, 1280), 1279);
    }

    #[test]
    fn abs_to_display_zero_max() {
        assert_eq!(abs_to_display(0, 0, 1280), 0);
    }

    #[test]
    fn abs_to_display_zero_dim() {
        assert_eq!(abs_to_display(16384, 32767, 0), 0);
    }

    #[test]
    fn abs_to_display_height() {
        // 32767 * 800 / 32768 = 799 (last row)
        assert_eq!(abs_to_display(32767, 32767, 800), 799);
    }
}
