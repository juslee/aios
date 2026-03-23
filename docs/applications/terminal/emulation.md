# AIOS Terminal VT Emulation Engine

Part of: [terminal.md](../terminal.md) — Terminal Emulator Architecture
**Related:** [input.md](./input.md) — Input handling and VT escape translation, [rendering.md](./rendering.md) — Cell grid to pixel rendering, [sessions.md](./sessions.md) — PTY byte stream source

-----

## 3. VT Emulation Engine

The VT emulation engine is the terminal's core: it consumes a byte stream from the PTY channel and produces a 2D grid of styled character cells. Every byte from the shell passes through this engine before anything appears on screen.

The engine implements the xterm-compatible terminal emulation standard, covering VT100, VT220, and xterm extensions. It uses a state machine parser based on Paul Williams' canonical VT parser model, extended with modern features (truecolor, mouse reporting, bracketed paste, synchronized output).

### 3.1 VT Parser State Machine

The parser processes one byte at a time, transitioning between states and executing actions. This design is borrowed from Paul Williams' widely-adopted VT parser state machine (used by Alacritty, WezTerm, vte crate), adapted for AIOS's Rust/no_std environment.

#### 3.1.1 Parser States

```text
┌───────────────────────────────────────────────────────────┐
│                     Parser States                          │
├───────────────┬───────────────────────────────────────────┤
│ State         │ Description                                │
├───────────────┼───────────────────────────────────────────┤
│ Ground        │ Default state. Printable chars → grid.     │
│ Escape        │ After ESC (0x1B). Deciding sequence type.  │
│ EscapeInterm  │ ESC + intermediate byte (0x20-0x2F).       │
│ CsiEntry      │ After ESC [ or CSI (0x9B). Start of CSI.   │
│ CsiParam      │ Collecting CSI parameters (0x30-0x3B).     │
│ CsiInterm     │ CSI intermediate bytes (0x20-0x2F).        │
│ CsiIgnore     │ Malformed CSI — discard until final byte.  │
│ DcsEntry      │ After ESC P or DCS (0x90). Start of DCS.   │
│ DcsParam      │ Collecting DCS parameters.                 │
│ DcsInterm     │ DCS intermediate bytes.                    │
│ DcsPassthru   │ Passing DCS payload to handler.            │
│ DcsIgnore     │ Malformed DCS — discard until ST.          │
│ OscString     │ After ESC ] or OSC (0x9D). Collecting      │
│               │ OS command string until ST or BEL.         │
│ SosPmApc      │ SOS/PM/APC string — collect until ST.      │
│ Utf8          │ Collecting multi-byte UTF-8 sequence.      │
└───────────────┴───────────────────────────────────────────┘
```

#### 3.1.2 Parser Actions

Actions execute during state transitions:

```text
┌──────────────────┬────────────────────────────────────────────────┐
│ Action           │ Effect                                          │
├──────────────────┼────────────────────────────────────────────────┤
│ Print            │ Write character to cell grid at cursor position │
│ Execute          │ Handle C0 control code (BEL, BS, HT, LF, CR)   │
│ Clear            │ Reset parameter/intermediate collection buffers │
│ Collect          │ Append byte to intermediate buffer              │
│ Param            │ Append digit to current parameter or advance    │
│ EscDispatch      │ Dispatch ESC sequence to handler                │
│ CsiDispatch      │ Dispatch CSI sequence with params to handler    │
│ Hook             │ Begin DCS passthrough (allocate handler)        │
│ Put              │ Pass byte to active DCS handler                 │
│ Unhook           │ End DCS passthrough (finalize handler)          │
│ OscStart         │ Begin OSC string collection                     │
│ OscPut           │ Append byte to OSC string buffer                │
│ OscEnd           │ Dispatch completed OSC string to handler        │
│ Ignore           │ Discard byte (malformed sequence)               │
└──────────────────┴────────────────────────────────────────────────┘
```

#### 3.1.3 State Transition Table

The full transition table. Each cell is `action / next_state`. Empty cells mean the byte is handled by the "anywhere" rules (see §3.1.4).

```text
Byte ranges → states:

From Ground:
  0x00-0x17, 0x19, 0x1C-0x1F  →  Execute / Ground
  0x20-0x7E                     →  Print / Ground
  0x7F                          →  Ignore / Ground
  0x80-0xBF (UTF-8 cont.)      →  Utf8 (see §3.4)
  0xC0-0xFD (UTF-8 lead)       →  Utf8 (see §3.4)

From Escape:
  0x20-0x2F                     →  Collect / EscapeInterm
  0x30-0x4F, 0x51-0x57, 0x59,
  0x5A, 0x5C, 0x60-0x7E        →  EscDispatch / Ground
  0x5B ([)                      →  Clear / CsiEntry
  0x5D (])                      →  OscStart / OscString
  0x50 (P)                      →  Clear / DcsEntry
  0x58 (X), 0x5E (^), 0x5F (_) →  / SosPmApc

From CsiEntry:
  0x20-0x2F                     →  Collect / CsiInterm
  0x30-0x39 (0-9)              →  Param / CsiParam
  0x3A (:)                      →  / CsiIgnore
  0x3B (;)                      →  Param / CsiParam
  0x3C-0x3F (< = > ?)          →  Collect / CsiParam
  0x40-0x7E (@-~)              →  CsiDispatch / Ground

From CsiParam:
  0x20-0x2F                     →  Collect / CsiInterm
  0x30-0x39 (0-9)              →  Param / CsiParam
  0x3A (:)                      →  / CsiIgnore
  0x3B (;)                      →  Param / CsiParam
  0x40-0x7E (@-~)              →  CsiDispatch / Ground

From CsiInterm:
  0x20-0x2F                     →  Collect / CsiInterm
  0x30-0x3F                     →  / CsiIgnore
  0x40-0x7E (@-~)              →  CsiDispatch / Ground

From OscString:
  0x07 (BEL)                    →  OscEnd / Ground
  0x20-0xFF (except 0x9C)       →  OscPut / OscString

From DcsEntry:
  0x20-0x2F                     →  Collect / DcsInterm
  0x30-0x39 (0-9), 0x3B (;)   →  Param / DcsParam
  0x3C-0x3F (< = > ?)          →  Collect / DcsParam
  0x40-0x7E (@-~)              →  Hook / DcsPassthru

From DcsPassthru:
  0x00-0x17, 0x19, 0x1C-0x1F  →  Put / DcsPassthru
  0x20-0x7E                     →  Put / DcsPassthru
  0x7F                          →  Ignore / DcsPassthru
```

#### 3.1.4 Anywhere Rules

These transitions apply from any state and take priority over state-specific rules:

```text
0x18 (CAN)     →  Execute / Ground     (cancel current sequence)
0x1A (SUB)     →  Execute / Ground     (substitute — cancel + print ?)
0x1B (ESC)     →  Clear / Escape       (start new escape sequence)
0x80-0x8F      →  Execute / Ground     (C1 controls: IND, NEL, HTS, etc.)
0x90 (DCS)     →  Clear / DcsEntry     (8-bit DCS introducer)
0x91-0x97      →  Execute / Ground     (C1 controls)
0x98 (SOS)     →  / SosPmApc
0x99 (SGCI)    →  Execute / Ground
0x9A (DECID)   →  Execute / Ground
0x9B (CSI)     →  Clear / CsiEntry     (8-bit CSI introducer)
0x9C (ST)      →  / Ground             (string terminator)
0x9D (OSC)     →  OscStart / OscString (8-bit OSC introducer)
0x9E (PM)      →  / SosPmApc
0x9F (APC)     →  / SosPmApc
```

#### 3.1.5 Rust Data Structures

```rust
/// Parser state machine.
pub struct VtParser {
    state: ParserState,
    params: ParamBuffer,
    intermediates: IntermediateBuffer,
    osc_buffer: OscBuffer,
    utf8_parser: Utf8Parser,
}

/// Fixed-size parameter buffer (no heap allocation).
pub struct ParamBuffer {
    params: [u16; 32],     // max 32 parameters per sequence
    sub_params: [u16; 32], // colon-separated sub-parameters
    count: u8,
    sub_count: u8,
}

/// Intermediate byte buffer.
pub struct IntermediateBuffer {
    bytes: [u8; 4],  // max 4 intermediate bytes
    count: u8,
}

/// OSC string buffer.
pub struct OscBuffer {
    data: [u8; 4096],  // max OSC string length
    len: usize,
}

/// Parser state enum.
#[derive(Clone, Copy, PartialEq)]
pub enum ParserState {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsPassthrough,
    DcsIgnore,
    OscString,
    SosPmApc,
    Utf8,
}
```

The parser is designed for `no_std` environments: all buffers are fixed-size, no heap allocation occurs during parsing, and the state machine processes one byte at a time with O(1) transitions via a lookup table.

#### 3.1.6 Parser Error Recovery

The VT parser processes untrusted byte streams from arbitrary programs running in the shell. Robust error recovery ensures that no malformed sequence can corrupt parser state, hang the terminal, or escape the emulation boundary.

**Anywhere rules as universal recovery.** The "anywhere" transitions (§3.1.4) are the parser's primary recovery mechanism. From *any* state:

- **CAN (0x18)** and **SUB (0x1A)** immediately return the parser to Ground state, discarding any partially-collected sequence. SUB additionally prints the substitution character (U+FFFD).
- **ESC (0x1B)** transitions to Escape state, effectively starting a new sequence and abandoning any in-progress sequence.

These rules guarantee that any malformed or truncated sequence is bounded: the parser recovers within at most one additional control byte.

**CsiIgnore and DcsIgnore states.** When the parser detects structurally invalid parameters during CSI or DCS collection (e.g., a colon in CsiEntry, or a parameter byte after an intermediate byte in CsiInterm), it transitions to CsiIgnore or DcsIgnore. These states silently discard all subsequent bytes until a valid final byte (0x40-0x7E for CSI) or string terminator (ST for DCS) returns the parser to Ground. No handler is invoked — the malformed sequence is simply dropped.

**Buffer overflow protection.** All parser buffers are fixed-size with strict bounds:

| Buffer | Capacity | Overflow Behavior |
|---|---|---|
| ParamBuffer | 32 parameters | Extra parameters silently discarded |
| IntermediateBuffer | 4 bytes | Extra intermediates trigger CsiIgnore/DcsIgnore |
| OscBuffer | 4096 bytes | Excess bytes discarded, OSC truncated at 4096 |
| Utf8Parser | 4 bytes | Invalid sequences emit U+FFFD, parser resets |

**UTF-8 error handling.** Invalid UTF-8 byte sequences are handled per the Unicode specification's "best practice for maximum interoperability":

- An unexpected continuation byte (0x80-0xBF) in Ground state emits U+FFFD and stays in Ground.
- A lead byte followed by invalid continuation bytes emits U+FFFD for each invalid byte and restarts decoding.
- Overlong encodings are rejected (emit U+FFFD).
- Codepoints above U+10FFFF are rejected (emit U+FFFD).

The parser never panics on any byte value, including null bytes (0x00), which are silently ignored in Ground state.

**Security properties.** The parser's error recovery prevents several classes of attacks:

- **Recursive ESC in strings:** OSC and DCS strings are only terminated by BEL or ST (either the 8-bit ST 0x9C or the 7-bit ST sequence `ESC \`). ESC bytes that appear inside these string states are interpreted as potential starts of the 7-bit ST sequence and do not introduce nested escape sequences. This prevents sequences designed to nest indefinitely.
- **Unbounded strings:** OSC and DCS strings without terminators are bounded by their respective buffer limits (4096 bytes for OSC). The parser does not allocate additional memory.
- **State machine convergence:** For any input byte sequence of length N, the parser returns to Ground state within at most N + max_sequence_length bytes. There are no cycles that avoid Ground indefinitely.

**Performance.** Error recovery adds no overhead to the normal parsing path. The state machine processes every byte — valid or invalid — in O(1) time with a single table lookup. No backtracking or lookahead occurs. The parser's total memory footprint is fixed at 4.2 KB regardless of input.

-----

### 3.2 Escape Sequence Handlers

After the parser identifies a complete sequence, it dispatches to a handler. Handlers modify the cell grid, cursor state, or terminal modes.

#### 3.2.1 CSI (Control Sequence Introducer)

CSI sequences are the most common escape sequences. Format: `ESC [ <params> <intermediates> <final_byte>`.

The handler receives the final byte, parameter list, and intermediate bytes, then dispatches to the appropriate grid/cursor operation. See §3.7 for the full CSI reference table.

#### 3.2.2 OSC (Operating System Command)

OSC sequences set terminal metadata. Format: `ESC ] <number> ; <string> ST`.

| OSC Number | Purpose |
|---|---|
| 0 | Set window title and icon name |
| 1 | Set icon name |
| 2 | Set window title |
| 4 | Set/query color palette entry |
| 7 | Set current working directory (shell integration) |
| 8 | Hyperlink (URL) |
| 10 | Set/query foreground color |
| 11 | Set/query background color |
| 12 | Set/query cursor color |
| 52 | Clipboard access (read/write via base64) |
| 104 | Reset color palette entry |
| 110-112 | Reset foreground/background/cursor color |
| 133 | Shell integration (prompt/command/output markers) |
| 1337 | Image display (iTerm2 protocol) |

**Security note:** OSC 52 (clipboard access) is capability-gated. The terminal only honors clipboard writes if the shell agent has `ClipboardWrite` capability. Clipboard reads require `ClipboardRead` and are never granted by default — the user must explicitly approve via Flow integration.

#### 3.2.3 DCS (Device Control String)

DCS sequences are used for device-specific extensions. Format: `ESC P <params> <intermediates> <final_byte> <payload> ST`.

| DCS Sequence | Purpose |
|---|---|
| `+q` | Request termcap/terminfo data (XTGETTCAP) |
| `$q` | Request mode value (DECRQM) |
| `>|` | Report terminal name and version |
| Sixel data | Sixel graphics rendering |
| TMUX DCS | tmux control mode passthrough |

#### 3.2.4 Simple ESC Sequences

| Sequence | Name | Effect |
|---|---|---|
| ESC 7 | DECSC | Save cursor position and attributes |
| ESC 8 | DECRC | Restore cursor position and attributes |
| ESC D | IND | Index (move cursor down, scroll if at bottom) |
| ESC E | NEL | Next Line (CR + LF) |
| ESC H | HTS | Set horizontal tab stop at cursor column |
| ESC M | RI | Reverse Index (move cursor up, scroll if at top) |
| ESC c | RIS | Full reset (clear screen, reset all modes) |
| ESC = | DECKPAM | Application keypad mode |
| ESC > | DECKPNM | Normal keypad mode |
| ESC ( B | SCS G0 | Designate G0 character set (ASCII) |
| ESC ( 0 | SCS G0 | Designate G0 character set (DEC Special Graphics) |

#### 3.2.5 Kitty Graphics Protocol

The kitty graphics protocol enables high-performance image display in the terminal. Unlike iTerm2's OSC 1337 (which base64-encodes image data inside an OSC string) or Sixel (which uses DCS with a bitmap encoding), kitty uses APC (Application Program Command) sequences with key-value control data and supports shared memory image transfer — a natural fit for AIOS's IPC architecture.

**Protocol format:**

```text
ESC _ G <key>=<value>,<key>=<value>,...; <payload> ESC \
 │       │                                │          │
 APC     Control data (key-value pairs)   Base64     ST (7-bit: ESC \)
         a=T (transmit), a=p (place),     image
         a=d (delete), a=q (query)        data
```

The APC sequence is terminated by ST — either the 7-bit form (`ESC \`, two bytes: 0x1B 0x5C) or the 8-bit form (0x9C). The VT state machine's `SosPmApc` state handles both termination forms. This is the same ST handling used for OSC and DCS strings throughout the parser.

**Transmission modes:**

| Mode | Key | Description | AIOS Adaptation |
|---|---|---|---|
| Direct | `t=d` | Base64 image data inline in APC payload | Parsed in SosPmApc state, decoded to buffer |
| File | `t=f` | Path to image file on local filesystem | Resolved via space storage path |
| Temp file | `t=t` | Path to temp file (deleted after use) | Mapped to ephemeral space object |
| Shared memory | `t=s` | Shared memory region ID | Maps directly to AIOS `SharedMemoryId` |

The shared memory mode (`t=s`) is the highest-performance path. The sending program writes image data to a shared memory region (allocated via IPC), then sends only the region ID in the APC sequence. The terminal reads the image directly from shared memory with zero copying. This aligns with AIOS's existing shared memory IPC infrastructure (§5.1 `bulk_buffer` in [sessions.md](./sessions.md)).

**Image lifecycle:**

```text
1. Transmit:  Program sends image data (any mode above)
              Terminal assigns an image ID, stores image in memory/space
2. Place:     Program sends placement command (a=p)
              Terminal creates a virtual placement on the cell grid
              Placement specifies: position, size (cells or pixels), z-index
3. Display:   Rendering pipeline draws image at placement coordinates
              Image composited with text (z-index determines layering)
4. Delete:    Program sends delete command (a=d)
              Terminal removes image and/or placements
```

**Image storage.** Transmitted images are stored in the terminal's space as objects, keyed by image ID. This provides:

- **Persistence:** Images survive terminal detach/reattach (the multiplexer preserves them)
- **Deduplication:** Identical images (by content hash) share storage
- **Memory management:** Images evicted under memory pressure, re-fetched from space on demand

**Placement model.** Each image can have multiple placements on the cell grid:

```rust
/// An image placement on the terminal grid.
pub struct ImagePlacement {
    /// Image ID (references stored image data).
    pub image_id: u32,
    /// Placement ID (unique within this image).
    pub placement_id: u32,
    /// Grid position (column, row) of top-left corner.
    pub col: u16,
    pub row: u16,
    /// Display size in cells (0 = auto from image dimensions).
    pub cols: u16,
    pub rows: u16,
    /// Z-index for layering (-1 = behind text, 0 = default, 1+ = above text).
    pub z_index: i32,
    /// Source rectangle within the image (for cropping/tiling).
    pub src_rect: Option<ImageRect>,
}
```

**Unicode placeholder mode.** For compatibility with text selection and accessibility, kitty supports a Unicode placeholder character (U+10EEEE from the private use area) that marks cells occupied by image placements. Screen readers announce these as "image" rather than reading invisible characters. Text selection skips placeholder cells.

**Comparison with other image protocols:**

| Feature | Kitty (APC) | iTerm2 (OSC 1337) | Sixel (DCS) |
|---|---|---|---|
| Transport | APC + ST | OSC + ST | DCS + ST |
| Encoding | Base64 / SHM / file | Base64 only | Sixel bitmap |
| Max payload | 4 KB per chunk (chunked transfer, bounded APC buffer) | 4096 bytes (OSC limit) | Unbounded |
| Shared memory | Yes (`t=s`) | No | No |
| Multiple placements | Yes (virtual) | No (inline only) | No (inline only) |
| Z-ordering | Yes (-1, 0, 1+) | No | No |
| Animation | Yes (frame composition) | No | No |
| Cell-level control | Yes (rows × cols) | Yes (width × height) | Pixel-level |
| AIOS fit | Best (SHM + space) | Good (simple) | Legacy (compat) |

**Capability gate.** Image display is passive output — it requires no special capability beyond the shell's standard PTY output channel. However, shared memory transmission requires that the shell process has been granted access to the shared memory region, which is controlled by the existing IPC capability system (§8.2 in [integration.md](./integration.md)).

-----

### 3.3 Terminal Modes

Terminal modes control how the emulation engine interprets input and renders output. Modes are set via CSI sequences (SM/RM for standard modes, DECSET/DECRST for DEC private modes).

#### 3.3.1 DEC Private Modes (DECSET/DECRST)

Set with `CSI ? <mode> h`, reset with `CSI ? <mode> l`.

| Mode | Name | Set Behavior | Reset Behavior |
|---|---|---|---|
| 1 | DECCKM | Application cursor keys (ESC O A) | Normal cursor keys (ESC [ A) |
| 3 | DECCOLM | 132-column mode | 80-column mode |
| 5 | DECSCNM | Reverse video (swap fg/bg) | Normal video |
| 6 | DECOM | Origin mode (cursor relative to scroll region) | Absolute cursor addressing |
| 7 | DECAWM | Auto-wrap at right margin | No auto-wrap |
| 12 | Cursor blink | Cursor blinks | Cursor steady |
| 25 | DECTCEM | Show cursor | Hide cursor |
| 47 | Alternate screen | Switch to alternate screen buffer | Switch to primary screen buffer |
| 1000 | Mouse reporting | Send mouse button press/release (X10) | Disable mouse reporting |
| 1002 | Mouse motion | Send motion while button pressed | Disable motion reporting |
| 1003 | All motion | Send all mouse motion events | Disable all motion |
| 1004 | Focus events | Send focus in/out events | Disable focus events |
| 1006 | SGR mouse | SGR-format mouse coordinates | Default mouse format |
| 1049 | Alt screen + save cursor | Save cursor, switch to alt, clear | Restore cursor, switch to primary |
| 2004 | Bracketed paste | Wrap pastes in ESC[200~/ESC[201~ | Raw paste |
| 2026 | Synchronized output | Buffer output until BSU end | Immediate output |

#### 3.3.2 Standard Modes (SM/RM)

Set with `CSI <mode> h`, reset with `CSI <mode> l`.

| Mode | Name | Set Behavior | Reset Behavior |
|---|---|---|---|
| 2 | KAM | Keyboard locked | Keyboard unlocked |
| 4 | IRM | Insert mode (shift chars right) | Replace mode (overwrite) |
| 12 | SRM | Local echo off (send/receive) | Local echo on |
| 20 | LNM | New line mode (CR+LF on Enter) | Line feed mode (LF only on Enter) |

#### 3.3.3 Mode State Storage

```rust
/// Terminal mode state — all modes tracked as bitfields for efficiency.
pub struct TerminalModes {
    /// DEC private modes (packed as u64 bitfield, indexed by mode number).
    dec_modes: [u64; 32],  // supports mode numbers up to 2047

    /// Standard ANSI modes.
    ansi_modes: u32,

    /// Saved cursor state for DECSC/DECRC.
    saved_cursor: Option<SavedCursor>,

    /// Saved cursor state for alternate screen entry.
    alt_saved_cursor: Option<SavedCursor>,
}

pub struct SavedCursor {
    row: u16,
    col: u16,
    attrs: CellAttributes,
    charset: CharsetState,
    origin_mode: bool,
    auto_wrap: bool,
}
```

-----

### 3.4 Character Set Handling

#### 3.4.1 UTF-8 Decoding

The parser handles UTF-8 inline with the state machine. When a byte >= 0x80 arrives in Ground state, the parser enters the Utf8 state and collects continuation bytes until the codepoint is complete.

```rust
pub struct Utf8Parser {
    buffer: [u8; 4],      // max 4 bytes per UTF-8 codepoint
    len: u8,              // bytes collected so far
    expected: u8,         // total bytes expected (2, 3, or 4)
}

impl Utf8Parser {
    /// Feed a byte. Returns Some(char) when a complete codepoint is assembled.
    pub fn feed(&mut self, byte: u8) -> Option<char> {
        // ...
    }
}
```

Invalid UTF-8 sequences are replaced with U+FFFD (REPLACEMENT CHARACTER) per the Unicode specification. The parser never panics on malformed input.

#### 3.4.2 Wide Characters

Characters with East Asian Width property "Wide" or "Fullwidth" (CJK ideographs, fullwidth ASCII, etc.) occupy two cell columns. The grid tracks this per-cell:

```rust
pub struct Cell {
    /// The character in this cell.
    pub c: char,
    /// Cell attributes (colors, bold, italic, etc.).
    pub attrs: CellAttributes,
    /// Cell width flags.
    pub flags: CellFlags,
}

bitflags::bitflags! {
    pub struct CellFlags: u8 {
        /// This cell is the leading (left) half of a wide character.
        const WIDE_CHAR       = 0b0000_0001;
        /// This cell is the trailing (right) half — display nothing.
        const WIDE_CHAR_TAIL  = 0b0000_0010;
        /// This cell contains a grapheme cluster that wraps from the previous line.
        const WRAPPED         = 0b0000_0100;
        /// This cell was explicitly set (not default/cleared).
        const DIRTY           = 0b0000_1000;
    }
}
```

When a wide character is printed at column `c`, the cell at `c` gets the character with `WIDE_CHAR` flag, and the cell at `c+1` gets a space with `WIDE_CHAR_TAIL` flag. If the wide character would overflow the line (cursor at last column), the current line wraps and the character prints at column 0 of the next line.

#### 3.4.3 Grapheme Clusters and Combining Marks

Unicode combining characters (accents, diacritical marks, emoji modifiers) attach to the preceding base character. The cell grid stores the full grapheme cluster as a single `char` for the base character plus a small overflow buffer for combining marks:

```rust
/// Extended cell content for grapheme clusters with combining marks.
pub struct GraphemeCell {
    /// Base character.
    pub base: char,
    /// Combining characters (up to 4 for common cases).
    pub combining: [char; 4],
    /// Number of combining characters present.
    pub combining_count: u8,
}
```

Emoji sequences (ZWJ sequences like family emoji, flag sequences) are stored as the full sequence in the grapheme cell. The rendering pipeline (§4) handles shaping these into a single glyph.

-----

### 3.5 Cell Grid Data Structure

The cell grid is the terminal's display model: a 2D array of character cells organized into rows, with a cursor tracking the write position.

#### 3.5.1 Grid Architecture

```rust
/// The terminal cell grid.
pub struct Grid {
    /// Active display rows (visible area).
    rows: Vec<Row>,
    /// Number of visible columns.
    cols: u16,
    /// Number of visible rows.
    visible_rows: u16,
    /// Cursor position and state.
    cursor: Cursor,
    /// Scroll region (top and bottom row, inclusive).
    scroll_region: ScrollRegion,
    /// Alternate screen buffer (for fullscreen apps like vim).
    alt_screen: Option<Box<Grid>>,
    /// Tab stops (column positions where HT advances to).
    tab_stops: TabStops,
}

/// A single row in the grid.
pub struct Row {
    /// Cell data for this row.
    cells: Vec<Cell>,
    /// Whether this row has been modified since last render.
    dirty: bool,
    /// Whether this row wraps to the next (soft wrap, not hard newline).
    wrapped: bool,
}

/// Cursor state.
pub struct Cursor {
    pub row: u16,
    pub col: u16,
    pub attrs: CellAttributes,       // current SGR attributes applied to new chars
    pub visible: bool,               // DECTCEM mode
    pub shape: CursorShape,          // block, underline, or bar
    pub blink: bool,                 // cursor blink mode
}

#[derive(Clone, Copy)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

/// Scroll region (VT100 DECSTBM).
pub struct ScrollRegion {
    pub top: u16,    // first scrolling row (inclusive)
    pub bottom: u16, // last scrolling row (inclusive)
}
```

#### 3.5.2 Dirty Tracking

The grid tracks which rows have been modified since the last render frame. This enables the rendering pipeline (§4) to perform incremental updates rather than full redraws.

```text
Frame N:     Grid state after processing 100 bytes from PTY
             Rows 5, 6, 7 modified (cursor moved, text printed)
             → dirty flags: [_, _, _, _, _, D, D, D, _, _, ...]

Render pass: Only rows 5-7 are re-rendered to the surface buffer
             → Damage region: Rect { x: 0, y: 5*cell_h, w: surface_w, h: 3*cell_h }

Frame N+1:   Clear dirty flags, process next batch of PTY bytes
```

For rapidly scrolling output (e.g., compilation logs), the engine detects that more than 50% of visible rows are dirty and switches to full-surface redraw mode, which is faster than per-row updates due to GPU batch efficiency.

#### 3.5.3 Resize Behavior

When the compositor sends a configure event with new dimensions, the grid resizes:

1. Calculate new `(cols, rows)` from pixel dimensions and cell size
2. If columns decreased: reflow lines (re-wrap long lines to new width)
3. If columns increased: unwrap previously soft-wrapped lines
4. If rows decreased: move excess rows to scrollback
5. If rows increased: pull rows from scrollback (if available)
6. Notify the PTY channel of the new size (shell reads via `TIOCGWINSZ` or IPC query)
7. Mark all rows dirty (full redraw after resize)

Line reflow preserves semantic line boundaries: a line that was hard-wrapped (ended with newline) stays as separate lines. A line that was soft-wrapped (auto-wrapped at column limit) is reflowed as a single logical line.

-----

### 3.6 Color Model

The terminal supports three color depths, with automatic negotiation based on the `TERM` environment variable:

#### 3.6.1 Color Representation

```rust
/// Terminal color, supporting all three color depths.
#[derive(Clone, Copy, PartialEq)]
pub enum Color {
    /// Named color from the 16-color palette (0-7 normal, 8-15 bright).
    Named(NamedColor),
    /// Indexed color from the 256-color palette (0-255).
    Indexed(u8),
    /// 24-bit truecolor (RGB).
    Rgb(u8, u8, u8),
}

/// The 16 named terminal colors.
#[derive(Clone, Copy, PartialEq)]
pub enum NamedColor {
    Black = 0,
    Red = 1,
    Green = 2,
    Yellow = 3,
    Blue = 4,
    Magenta = 5,
    Cyan = 6,
    White = 7,
    BrightBlack = 8,
    BrightRed = 9,
    BrightGreen = 10,
    BrightYellow = 11,
    BrightBlue = 12,
    BrightMagenta = 13,
    BrightCyan = 14,
    BrightWhite = 15,
}
```

#### 3.6.2 SGR (Select Graphic Rendition) Attributes

SGR sequences (`CSI <params> m`) set cell display attributes:

```rust
/// Cell display attributes set by SGR sequences.
#[derive(Clone, Copy, Default)]
pub struct CellAttributes {
    pub fg: Option<Color>,      // None = default foreground
    pub bg: Option<Color>,      // None = default background
    pub underline_color: Option<Color>,
    pub flags: AttrFlags,
}

bitflags::bitflags! {
    pub struct AttrFlags: u16 {
        const BOLD          = 0b0000_0000_0001;
        const DIM           = 0b0000_0000_0010;
        const ITALIC        = 0b0000_0000_0100;
        const UNDERLINE     = 0b0000_0000_1000;
        const BLINK         = 0b0000_0001_0000;
        const INVERSE       = 0b0000_0010_0000;
        const HIDDEN        = 0b0000_0100_0000;
        const STRIKETHROUGH = 0b0000_1000_0000;
        const DOUBLE_UNDER  = 0b0001_0000_0000;  // double underline
        const CURLY_UNDER   = 0b0010_0000_0000;  // curly/wavy underline
        const DOTTED_UNDER  = 0b0100_0000_0000;  // dotted underline
        const DASHED_UNDER  = 0b1000_0000_0000;  // dashed underline
        const OVERLINE      = 0b0001_0000_0000_0000;
        const HYPERLINK     = 0b0010_0000_0000_0000;  // OSC 8 hyperlink active
    }
}
```

#### 3.6.3 Color Palette

The 256-color palette consists of:

- **Colors 0-7:** Standard colors (configurable via terminal profile)
- **Colors 8-15:** Bright colors (configurable via terminal profile)
- **Colors 16-231:** 6×6×6 color cube: `index = 16 + 36*r + 6*g + b` where r,g,b ∈ [0,5]
- **Colors 232-255:** 24-step grayscale ramp from dark to light

The palette is stored in the terminal profile (space object) and can be customized per-session or globally. OSC 4 sequences allow programs to modify individual palette entries at runtime.

-----

### 3.7 Supported Escape Sequence Reference

#### 3.7.1 CSI Sequences

| Sequence | Name | Parameters | Effect |
|---|---|---|---|
| CSI n A | CUU | n=count (default 1) | Cursor up n rows |
| CSI n B | CUD | n=count (default 1) | Cursor down n rows |
| CSI n C | CUF | n=count (default 1) | Cursor forward n columns |
| CSI n D | CUB | n=count (default 1) | Cursor back n columns |
| CSI n E | CNL | n=count (default 1) | Cursor to beginning of line n lines down |
| CSI n F | CPL | n=count (default 1) | Cursor to beginning of line n lines up |
| CSI n G | CHA | n=column (default 1) | Cursor to column n |
| CSI n;m H | CUP | n=row, m=col (default 1;1) | Cursor to row n, column m |
| CSI n J | ED | n=mode (default 0) | Erase in display (0=below, 1=above, 2=all, 3=all+scrollback) |
| CSI n K | EL | n=mode (default 0) | Erase in line (0=right, 1=left, 2=all) |
| CSI n L | IL | n=count (default 1) | Insert n blank lines at cursor |
| CSI n M | DL | n=count (default 1) | Delete n lines at cursor |
| CSI n P | DCH | n=count (default 1) | Delete n characters at cursor |
| CSI n S | SU | n=count (default 1) | Scroll up n lines |
| CSI n T | SD | n=count (default 1) | Scroll down n lines |
| CSI n X | ECH | n=count (default 1) | Erase n characters at cursor (no cursor move) |
| CSI n @ | ICH | n=count (default 1) | Insert n blank characters at cursor |
| CSI n;m r | DECSTBM | n=top, m=bottom | Set scroll region |
| CSI n d | VPA | n=row (default 1) | Cursor to row n (absolute) |
| CSI s | SCP | — | Save cursor position |
| CSI u | RCP | — | Restore cursor position |
| CSI n b | REP | n=count | Repeat last printed character n times |
| CSI ... m | SGR | (see §3.6.2) | Set graphic rendition (colors, attributes) |
| CSI n c | DA | n=0 | Device attributes (primary) — report terminal identity |
| CSI > n c | DA2 | n=0 | Secondary device attributes — report version |
| CSI n t | XTWINOPS | n=op | Window operations (resize, report size, etc.) |
| CSI n;m;o t | XTWINOPS | — | Extended window ops with parameters |
| CSI ? n $ p | DECRQM | n=mode | Request DEC private mode value |

#### 3.7.2 SGR Parameter Reference

| Parameter | Effect |
|---|---|
| 0 | Reset all attributes to default |
| 1 | Bold (or increased intensity) |
| 2 | Dim (decreased intensity) |
| 3 | Italic |
| 4 | Underline |
| 4:0 | No underline |
| 4:1 | Single underline |
| 4:2 | Double underline |
| 4:3 | Curly/wavy underline |
| 4:4 | Dotted underline |
| 4:5 | Dashed underline |
| 5 | Slow blink |
| 7 | Inverse/reverse video |
| 8 | Hidden (invisible text) |
| 9 | Strikethrough |
| 21 | Double underline (alternative) |
| 22 | Normal intensity (neither bold nor dim) |
| 23 | Not italic |
| 24 | Not underlined |
| 25 | Not blinking |
| 27 | Not inverse |
| 28 | Not hidden |
| 29 | Not strikethrough |
| 30-37 | Set foreground color (standard 8) |
| 38;5;n | Set foreground color (256-palette index n) |
| 38;2;r;g;b | Set foreground color (truecolor RGB) |
| 39 | Default foreground color |
| 40-47 | Set background color (standard 8) |
| 48;5;n | Set background color (256-palette index n) |
| 48;2;r;g;b | Set background color (truecolor RGB) |
| 49 | Default background color |
| 53 | Overline |
| 55 | Not overlined |
| 58;5;n | Set underline color (256-palette index n) |
| 58;2;r;g;b | Set underline color (truecolor RGB) |
| 59 | Default underline color |
| 90-97 | Set foreground color (bright 8) |
| 100-107 | Set background color (bright 8) |

#### 3.7.3 OSC Sequences Summary

See §3.2.2 for the full OSC reference table.

#### 3.7.4 Terminal Identity

When queried via `CSI c` (Device Attributes), the AIOS terminal reports:

```text
Primary DA response:   CSI ? 6 4 ; 2 2 c
  → VT420 with ANSI color (indicates xterm-256color compatibility)

Secondary DA response: CSI > 1 ; <version> ; 0 c
  → Terminal type 1 (VT100 family), version number, no hardware options
```

The `TERM` environment variable is set to `xterm-256color` by default, with `COLORTERM=truecolor` to indicate 24-bit color support.
