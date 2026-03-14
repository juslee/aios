# AIOS Terminal Input Handling

Part of: [terminal.md](../terminal.md) — Terminal Emulator Architecture
**Related:** [emulation.md](./emulation.md) — VT modes affect input interpretation, [sessions.md](./sessions.md) — Input delivered to PTY channel, [rendering.md](./rendering.md) — Selection and cursor rendering

-----

## 6. Input Handling

The terminal's input pipeline transforms compositor input events (keycodes, mouse events, touch gestures) into byte sequences that shells and programs expect. This translation is the bridge between AIOS's modern input model and the decades-old VT escape sequence convention that all Unix tools understand.

### 6.1 Keyboard Event Flow

Keyboard events flow through a multi-stage pipeline before reaching the shell:

```text
Hardware keyboard
  ↓
Compositor input pipeline
  ├── System hotkeys consumed (Alt+Tab, Super+L)
  ├── Agent hotkeys consumed (Ctrl+Alt+T for new terminal)
  └── Remaining events routed to focused surface
        ↓
Terminal agent receives InputEvent::Keyboard
  ├── Signal intercept (Ctrl+C, Ctrl+Z, Ctrl+\, Ctrl+D)
  ├── Terminal-local actions (Ctrl+Shift+C = copy, Ctrl+Shift+V = paste)
  ├── Modifier translation (Ctrl+A → 0x01, Ctrl+B → 0x02, etc.)
  └── VT escape sequence generation
        ↓
Bytes written to PTY input channel
  ↓
Shell reads input
```

#### 6.1.1 Keyboard Event Structure

The terminal receives events from the compositor's input pipeline (see [compositor/input.md](../../platform/compositor/input.md) §7.1):

```rust
/// Keyboard event received from compositor.
pub struct KeyboardEvent {
    /// Physical key code (USB HID usage code).
    pub keycode: u32,
    /// Key state.
    pub state: KeyState,
    /// Active modifier keys.
    pub modifiers: Modifiers,
    /// UTF-8 text produced by the key (after layout mapping).
    /// None for non-printable keys (arrows, F-keys, etc.).
    pub text: Option<char>,
    /// Whether this is a key repeat event.
    pub is_repeat: bool,
}

pub enum KeyState {
    Pressed,
    Released,
}

bitflags::bitflags! {
    pub struct Modifiers: u8 {
        const SHIFT = 0b0001;
        const CTRL  = 0b0010;
        const ALT   = 0b0100;
        const SUPER = 0b1000;
    }
}
```

The terminal only processes `Pressed` events (and repeats). `Released` events are ignored for terminal input (but tracked for modifier state).

### 6.2 Keycode to VT Escape Sequence Translation

The input translator converts keycodes and modifiers into the byte sequences that terminal applications expect. The translation depends on the current terminal mode state (§3.3).

#### 6.2.1 Printable Characters

For keys that produce text (`event.text.is_some()`):

```text
No modifiers:    Send UTF-8 bytes of the character
Shift:           Already handled by keyboard layout (e.g., Shift+a = 'A')
Ctrl+letter:     Send control code (Ctrl+A = 0x01, ..., Ctrl+Z = 0x1A)
Alt+letter:      Send ESC + letter (Alt+a = ESC a = 0x1B 0x61)
Ctrl+Alt+letter: Send ESC + control code (Ctrl+Alt+a = ESC 0x01)
```

#### 6.2.2 Special Keys

| Key | Normal Mode | Application Mode (DECCKM) |
|---|---|---|
| Up | `ESC [ A` | `ESC O A` |
| Down | `ESC [ B` | `ESC O B` |
| Right | `ESC [ C` | `ESC O C` |
| Left | `ESC [ D` | `ESC O D` |
| Home | `ESC [ H` | `ESC O H` |
| End | `ESC [ F` | `ESC O F` |
| Insert | `ESC [ 2 ~` | `ESC [ 2 ~` |
| Delete | `ESC [ 3 ~` | `ESC [ 3 ~` |
| Page Up | `ESC [ 5 ~` | `ESC [ 5 ~` |
| Page Down | `ESC [ 6 ~` | `ESC [ 6 ~` |
| F1 | `ESC O P` | `ESC O P` |
| F2 | `ESC O Q` | `ESC O Q` |
| F3 | `ESC O R` | `ESC O R` |
| F4 | `ESC O S` | `ESC O S` |
| F5 | `ESC [ 1 5 ~` | `ESC [ 1 5 ~` |
| F6 | `ESC [ 1 7 ~` | `ESC [ 1 7 ~` |
| F7 | `ESC [ 1 8 ~` | `ESC [ 1 8 ~` |
| F8 | `ESC [ 1 9 ~` | `ESC [ 1 9 ~` |
| F9 | `ESC [ 2 0 ~` | `ESC [ 2 0 ~` |
| F10 | `ESC [ 2 1 ~` | `ESC [ 2 1 ~` |
| F11 | `ESC [ 2 3 ~` | `ESC [ 2 3 ~` |
| F12 | `ESC [ 2 4 ~` | `ESC [ 2 4 ~` |
| Enter | `0x0D` (CR) or `0x0D 0x0A` (CR+LF, if LNM mode) | Same |
| Tab | `0x09` (HT) | Same |
| Backspace | `0x7F` (DEL) | Same |
| Escape | `0x1B` (ESC) | Same |

#### 6.2.3 Modified Special Keys

When Shift, Ctrl, or Alt modify a special key, the xterm modifier encoding is used:

```text
Modifier encoding: modifier_code = 1 + (Shift?1:0) + (Alt?2:0) + (Ctrl?4:0) + (Super?8:0)

Examples:
  Shift+Up:        ESC [ 1 ; 2 A    (modifier_code = 2)
  Ctrl+Up:         ESC [ 1 ; 5 A    (modifier_code = 5)
  Ctrl+Shift+Up:   ESC [ 1 ; 6 A    (modifier_code = 6)
  Alt+Up:          ESC [ 1 ; 3 A    (modifier_code = 3)
  Ctrl+Alt+Up:     ESC [ 1 ; 7 A    (modifier_code = 7)

  Shift+F5:        ESC [ 1 5 ; 2 ~  (modifier_code = 2)
  Ctrl+F5:         ESC [ 1 5 ; 5 ~  (modifier_code = 5)
```

#### 6.2.4 Kitty Keyboard Protocol

The terminal supports the kitty keyboard protocol as an extension for applications that need more precise key information. When enabled (via `CSI > flags u`), all key events are reported in a structured format:

```text
CSI key_code ; modifier_code u

Where:
  key_code    = Unicode codepoint of the key
  modifier_code = 1 + modifier_bitmask (same as xterm encoding)
```

This protocol eliminates ambiguities in the legacy VT encoding (e.g., `ESC` key vs. escape sequence start, `Ctrl+H` vs. Backspace) and supports key release events, which legacy VT encoding cannot represent.

### 6.3 Mouse Reporting

When mouse reporting is enabled (DEC private modes 1000-1006), the terminal translates mouse events into escape sequences:

#### 6.3.1 Mouse Event Flow

```text
Compositor routes MouseEvent to terminal surface
  ↓
Terminal converts pixel position to cell coordinates:
  cell_col = (mouse_x - pad_left) / cell_width
  cell_row = (mouse_y - pad_top) / cell_height
  ↓
Terminal encodes mouse event based on active mode:
  Mode 1000 (X10): button press only
  Mode 1002 (button motion): press, release, motion while pressed
  Mode 1003 (any motion): all mouse motion events
  ↓
Terminal encodes coordinates based on format:
  Default: single byte per coordinate (max 223 columns/rows)
  SGR (mode 1006): decimal coordinates (unlimited)
```

#### 6.3.2 SGR Mouse Encoding (Preferred)

SGR mouse mode (1006) is the preferred encoding — it supports unlimited terminal dimensions and provides unambiguous button identification:

```text
Press:    CSI < button ; col ; row M
Release:  CSI < button ; col ; row m

Button encoding:
  0 = left button
  1 = middle button
  2 = right button
  32 = motion (added to button code during drag)
  64 = scroll up (wheel)
  65 = scroll down (wheel)
  128 = scroll left (horizontal wheel)
  129 = scroll right (horizontal wheel)

Modifier bits (added to button code):
  4 = Shift held
  8 = Alt held
  16 = Ctrl held
```

#### 6.3.3 Mouse Reporting Security

Mouse coordinates reveal where the user's cursor is pointing, which could leak information about screen layout. The terminal respects the following rules:

- Mouse reporting only sends events to the foreground PTY session
- Detached sessions never receive mouse events
- Secure input mode (§6.5) disables all mouse reporting
- Mouse events are not logged to the audit ring (position data is privacy-sensitive)

### 6.4 Selection and Clipboard (Flow Integration)

Text selection in the terminal uses the Flow subsystem for clipboard operations, providing secure, capability-gated clipboard access.

#### 6.4.1 Selection Model

```text
Selection modes:
  Single click:   Position cursor (no selection)
  Click + drag:   Character-level selection (rectangular or stream)
  Double click:   Word selection (word boundary detection)
  Triple click:   Line selection (full logical line)
  Shift + click:  Extend selection to click position

Selection types:
  Stream:      Continuous text across lines (default)
  Rectangular: Column-aligned block selection (Alt + drag)
```

The terminal tracks selection state independently of the cell grid:

```rust
/// Active text selection state.
pub struct Selection {
    /// Selection anchor (where the user started).
    pub anchor: SelectionPoint,
    /// Selection cursor (where the user ended).
    pub cursor: SelectionPoint,
    /// Selection type.
    pub kind: SelectionKind,
    /// Whether the selection is currently active (mouse button held).
    pub active: bool,
}

pub struct SelectionPoint {
    pub col: u16,
    pub row: i32,  // negative values index into scrollback
}

pub enum SelectionKind {
    Stream,
    Rectangular,
    Word,
    Line,
}
```

#### 6.4.2 Clipboard Operations

**Copy:** Extract selected text from the cell grid and send to Flow:

```text
1. User selects text and presses Ctrl+Shift+C (or right-click → Copy)
2. Terminal extracts selected cells, joining with newlines between lines
3. For stream selection: respect soft-wrap (join wrapped lines without newline)
4. For rectangular selection: extract column-aligned block, pad short lines with spaces
5. Terminal sends text to Flow clipboard channel
6. Flow system makes text available to other agents
```

**Paste:** Receive text from Flow and inject into PTY input:

```text
1. User presses Ctrl+Shift+V (or right-click → Paste)
2. Terminal requests text from Flow clipboard channel
3. If bracketed paste mode (2004) is active:
   a. Send ESC[200~ (paste start marker)
   b. Send paste text bytes
   c. Send ESC[201~ (paste end marker)
   → Shell receives pasted text as a quoted block (not executed line-by-line)
4. If bracketed paste mode is inactive:
   a. Send paste text bytes directly
   → CAUTION: pasted commands execute immediately on newlines
```

**OSC 52 clipboard access:** Programs can request clipboard read/write via OSC 52 escape sequences. This is capability-gated:

- Clipboard write: allowed if shell has `ClipboardWrite` capability
- Clipboard read: requires explicit `ClipboardRead` capability and user confirmation dialog

### 6.5 Secure Input Mode

When the shell reads a password or other sensitive input, the terminal activates secure input mode to prevent information leakage:

#### 6.5.1 Activation

Secure input mode activates when:

1. The shell sends a secure input hint (custom OSC sequence)
2. The terminal detects a password prompt pattern (heuristic: prompt ending with "password:", "passphrase:", etc.)
3. A capability-gated service explicitly requests secure input for the session

#### 6.5.2 Behavior

During secure input mode:

| Feature | Normal Mode | Secure Input Mode |
|---|---|---|
| Keyboard logging | Audit ring records key events | No key event logging |
| Screenshot | Compositor allows screen capture | Compositor denies capture for this surface |
| Clipboard read | Allowed (if capable) | Denied (no clipboard sniffing) |
| System hotkeys | All active | Only essential (Alt+Tab, Super+L) |
| Mouse reporting | If enabled by shell | Disabled (no position leakage) |
| Input display | Echo per terminal settings | Dots or nothing (password masking) |
| Surface hint | `InteractionState::Active` | `InteractionState::SecureInput` |

The compositor uses the `SecureInput` interaction state to apply visual indicators (e.g., a lock icon in the window decoration) so the user knows input is being handled securely.

#### 6.5.3 Deactivation

Secure input mode deactivates when:

1. The shell sends a secure input end hint
2. The Enter key is pressed (password entry completed)
3. The secure input timeout expires (configurable, default 60 seconds)
4. The session is detached or destroyed

### 6.6 IME / Compose Key Support

For languages that require input method editors (CJK, Arabic, Devanagari), the terminal integrates with the compositor's IME framework:

#### 6.6.1 IME Flow

```text
1. User presses a key that activates the IME (e.g., pinyin input)
2. Compositor's IME framework begins composition
3. Terminal receives IME events:
   a. CompositionStart: clear any pending input
   b. CompositionUpdate { preedit_text, cursor }: render preedit text at cursor
   c. CompositionEnd { committed_text }: send committed text to PTY
4. Terminal renders preedit text as a floating overlay at cursor position
5. On commit: send UTF-8 bytes of committed text to PTY input channel
```

#### 6.6.2 Preedit Rendering

The preedit text (uncommitted IME composition) is rendered as an overlay on the terminal surface, not as part of the cell grid:

```text
Terminal grid:
  $ echo 你好     ← committed text in cells
          ni hao  ← preedit overlay (floating, highlighted background)
          ^^^^^^
          IME composition in progress
```

The preedit overlay is rendered with a distinct background color (configurable) and a cursor indicating the current composition position. It disappears when the IME commits or cancels the composition.

#### 6.6.3 Compose Key Sequences

For Latin-script compose sequences (e.g., Compose + a + ' → á), the terminal handles these via the compositor's dead key / compose key mechanism. The compositor resolves the compose sequence and delivers the final Unicode character to the terminal as a normal `KeyboardEvent` with `text: Some('á')`.

The terminal does not need to implement its own compose key handling — the compositor does this universally for all surfaces.
