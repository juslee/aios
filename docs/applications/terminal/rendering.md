# AIOS Terminal Rendering Pipeline

Part of: [terminal.md](../terminal.md) — Terminal Emulator Architecture
**Related:** [emulation.md](./emulation.md) — Cell grid data source, [sessions.md](./sessions.md) — PTY byte stream, [input.md](./input.md) — Cursor and selection rendering

-----

## 4. Text Rendering Pipeline

The rendering pipeline transforms the cell grid (§3.5) into pixels on a compositor surface. Unlike standalone terminal emulators that manage their own OpenGL/Metal/Vulkan contexts, the AIOS terminal renders into a shared memory buffer that the compositor composites with other surfaces.

This architecture eliminates the most complex part of traditional terminal renderers — GPU context management, window system integration, and frame timing — and replaces it with the compositor's existing infrastructure.

### 4.1 Font Engine Integration

The terminal uses the OS font service, shared with all text-rendering agents (browser, UI toolkit, document viewer). The font service provides:

- **Font discovery:** Enumerate available monospace fonts by family name
- **Font loading:** Memory-mapped font file access via space objects
- **Glyph shaping:** HarfBuzz-based shaping for complex scripts (Arabic, Devanagari, CJK vertical)
- **Fallback chains:** Automatic fallback to system fonts for missing glyphs

The terminal requests a monospace font at initialization:

```rust
/// Terminal font configuration (from terminal profile space object).
pub struct TerminalFont {
    /// Primary font family name (e.g., "JetBrains Mono", "Fira Code").
    pub family: String,
    /// Font size in points.
    pub size_pt: f32,
    /// Whether to use font ligatures (fi, fl, ->, =>, etc.).
    pub ligatures: bool,
    /// Fallback font families (tried in order for missing glyphs).
    pub fallbacks: Vec<String>,
    /// Cell dimensions derived from font metrics.
    pub cell_width: u16,
    pub cell_height: u16,
    /// Baseline offset from top of cell.
    pub baseline: u16,
    /// Underline position and thickness.
    pub underline_position: u16,
    pub underline_thickness: u16,
}
```

Cell dimensions are derived from the font's `advance_width` (for monospace, all ASCII glyphs have the same advance) and `ascent + descent + leading` for height. All cells in the grid use the same dimensions — this is the fundamental monospace invariant.

### 4.2 Glyph Atlas and GPU Text Rendering

The compositor maintains a shared glyph atlas — a GPU texture containing pre-rasterized glyph bitmaps. The terminal submits glyphs to the atlas on first use and references them by atlas coordinates thereafter.

#### 4.2.1 Atlas Architecture

```text
┌────────────────────────────────────────────────────────┐
│                    Glyph Atlas (GPU Texture)             │
│                                                          │
│  ┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐        │
│  │A ││B ││C ││D ││ ...                      ││        │
│  └──┘└──┘└──┘└──┘└──┘└──┘└──┘└──┘└──┘└──┘└──┘        │
│  ┌──┐┌──┐┌──┐┌──┐                                      │
│  │a ││b ││c ││ 你 │  ← wide glyph occupies 2× width    │
│  └──┘└──┘└──┘└────┘                                     │
│  ┌──┐┌──┐                                               │
│  │🔥││📁│  ← emoji (color, may be larger)               │
│  └──┘└──┘                                               │
│                                                          │
│  [free space for new glyphs]                            │
└────────────────────────────────────────────────────────┘
```

Each glyph entry in the atlas is:

```rust
/// A glyph's location in the atlas texture.
pub struct GlyphAtlasEntry {
    /// Top-left corner in the atlas texture (pixels).
    pub atlas_x: u16,
    pub atlas_y: u16,
    /// Glyph dimensions in the atlas (pixels).
    pub width: u16,
    pub height: u16,
    /// Offset from cell origin to glyph origin (for baseline alignment).
    pub bearing_x: i16,
    pub bearing_y: i16,
}
```

The terminal maintains a local cache mapping `(char, CellAttributes)` → `GlyphAtlasEntry`. On a cache miss, the terminal requests rasterization from the font engine and uploads the bitmap to the atlas.

#### 4.2.2 Rendering Pipeline

The render pass converts dirty grid cells into a surface buffer update:

```text
1. Identify dirty rows (from grid dirty flags)
2. For each dirty row:
   a. For each cell in the row:
      - Look up glyph in atlas cache
      - If cache miss: rasterize glyph, upload to atlas
      - Compute cell screen position: (col * cell_w, row * cell_h)
   b. Batch all cells into a draw list:
      Draw list entry = { atlas_rect, screen_rect, fg_color, bg_color, attr_flags }
3. Execute draw list:
   a. Fill background rectangles (batched by color)
   b. Blit glyph textures from atlas (instanced draw)
   c. Draw decorations (underline, strikethrough, overline)
   d. Draw cursor (block/underline/bar overlay)
4. Report damage region to compositor
```

#### 4.2.3 Instanced Rendering

For GPU-accelerated rendering, all glyph draws are batched into a single instanced draw call. Each instance specifies:

```rust
/// Per-instance data for GPU glyph rendering.
#[repr(C)]
pub struct GlyphInstance {
    /// Screen position (top-left of cell, in pixels).
    pub screen_x: f32,
    pub screen_y: f32,
    /// Atlas texture coordinates (normalized 0.0-1.0).
    pub atlas_u: f32,
    pub atlas_v: f32,
    pub atlas_w: f32,
    pub atlas_h: f32,
    /// Foreground color (RGBA, premultiplied alpha).
    pub fg_color: [f32; 4],
    /// Background color (RGBA).
    pub bg_color: [f32; 4],
}
```

A single instanced draw call renders hundreds of glyphs simultaneously. This is the key performance advantage of GPU-accelerated terminal rendering: the GPU processes all visible cells in parallel, whereas CPU rendering iterates sequentially.

### 4.3 Cell-to-Pixel Mapping

The monospace grid maps cells to screen pixels with exact precision:

```text
Cell (col, row) → Pixel (col * cell_w + pad_left, row * cell_h + pad_top)

Surface layout:
┌─────────────────────────────────────────┐
│ pad_top                                  │
│ ┌─────────────────────────────────────┐ │
│ │ Cell(0,0)  Cell(1,0)  Cell(2,0) ... │ │ ← row 0
│ │ Cell(0,1)  Cell(1,1)  Cell(2,1) ... │ │ ← row 1
│ │ ...                                  │ │
│ │ Cell(0,N)  Cell(1,N)  Cell(2,N) ... │ │ ← row N
│ └─────────────────────────────────────┘ │
│ pad_bottom                               │
└─────────────────────────────────────────┘
```

Cell padding (the space between the cell grid and the surface edge) prevents text from touching window borders. The padding is configurable in the terminal profile.

### 4.4 Damage Tracking

The terminal reports damage to the compositor using the compositor's damage protocol (see [compositor/protocol.md](../../platform/compositor/protocol.md) §3.4):

#### 4.4.1 Damage Strategies

| Scenario | Dirty Rows | Damage Report | Rationale |
|---|---|---|---|
| Single line edit | 1 | `Rect { y: row*h, h: cell_h }` | Minimal compositor work |
| Cursor-only move | 0–2 | Two `Rect` (old + new cursor pos) | Cursor doesn't dirty cell content |
| Partial scroll | 3–10 | Union `Rect` of dirty rows | Scroll region update |
| Full scroll (compilation output) | >50% | `FullSurface` | Full redraw cheaper than many rects |
| Synchronized output (mode 2026) | Varies | Deferred until `BSU` end | Batch all updates into single damage |
| No change | 0 | `Empty` | Compositor skips this surface |

#### 4.4.2 Synchronized Output

When the shell enables synchronized output (mode 2026), the terminal buffers all grid changes until the shell sends the end marker. This prevents partial rendering of multi-line updates (e.g., status bars, progress indicators, TUI applications).

```text
Shell sends: ESC [ ? 2 0 2 6 h   ← begin synchronized update
Shell sends: [many escape sequences updating the screen]
Shell sends: ESC [ ? 2 0 2 6 l   ← end synchronized update

Terminal behavior:
  1. On begin: set sync_mode = true, start buffering
  2. Process all sequences, update grid, track dirty rows
  3. On end: set sync_mode = false, render all dirty rows, report damage
  → Result: single atomic compositor frame with all changes
```

### 4.5 Scrollback Buffer

The scrollback buffer stores lines that have scrolled off the top of the visible grid. It serves as the terminal's history.

#### 4.5.1 Storage Architecture

```rust
/// Scrollback buffer backed by space storage.
pub struct ScrollbackBuffer {
    /// In-memory ring buffer for recent scrollback (fast access).
    recent: RingBuffer<Row>,
    /// Space-backed persistent storage for older scrollback.
    persistent: Option<ScrollbackSpace>,
    /// Maximum lines in memory before spilling to space.
    memory_limit: usize,
    /// Maximum total lines (memory + space) before oldest lines are evicted.
    total_limit: usize,
    /// Current scroll position (0 = bottom, positive = scrolled up).
    scroll_offset: usize,
}
```

The scrollback uses a two-tier architecture:

1. **Memory tier:** A ring buffer holds the most recent N lines (configurable, default 10,000) for instant access. This covers typical scroll-up operations.
2. **Space tier:** When the memory tier is full, oldest lines spill to a space object. Space-backed scrollback is persistent (survives terminal restart), searchable (AIRS can query it), and syncable (available on other devices via Space Mesh Protocol).

#### 4.5.2 Scroll Rendering

When the user scrolls, the visible area shifts into the scrollback:

```text
Scrollback:  [line -100] [line -99] ... [line -1]
Visible:     [line 0] [line 1] ... [line 23]   ← 24-row terminal
Cursor:      at line 23, col 0

User scrolls up 5 lines:
Visible now: [line -5] [line -4] ... [line 18]
             ^^^^^^^^^^^^^^^^^^^^^^^^^ from scrollback
                                        ^^^^^^^^^ from grid

All visible rows marked dirty → full surface redraw
```

During scroll, the terminal surface reports `FullSurface` damage. The compositor can optimize this using hardware scroll if the GPU supports it (translate the existing surface content and render only the new rows).

### 4.6 Compositor Surface Protocol

The terminal creates and manages a compositor surface following the standard surface lifecycle (see [compositor/protocol.md](../../platform/compositor/protocol.md) §3).

#### 4.6.1 Surface Configuration

```rust
/// Terminal surface hints sent to compositor.
pub fn terminal_surface_hints() -> SurfaceHints {
    SurfaceHints {
        content_type: ContentType::Terminal,
        layout_preference: LayoutPreference::PreferWidth(640),
        min_size: Size { w: 240, h: 180 },  // ~30×10 chars at minimum
        interaction_state: InteractionState::Active,
        semantic_role: SemanticRole::Primary,
        resize_increment: Some(Size {
            w: cell_width as u32,   // snap to cell grid
            h: cell_height as u32,
        }),
    }
}
```

The `resize_increment` hint tells the compositor to snap window dimensions to cell boundaries, preventing partial-cell rendering at the right and bottom edges.

#### 4.6.2 Shared Buffer Format

The terminal writes to a shared memory buffer allocated via the compositor:

```rust
/// Terminal surface buffer format.
pub struct TerminalSurfaceBuffer {
    /// Pixel data (BGRA8888, pre-multiplied alpha).
    pub pixels: &mut [u8],
    /// Buffer dimensions.
    pub width: u32,
    pub height: u32,
    /// Bytes per row (may include padding for alignment).
    pub stride: u32,
}
```

The buffer format matches the compositor's expected input (BGRA8888 with pre-multiplied alpha). The terminal writes glyph pixels directly into this buffer, and the compositor reads it for composition without any format conversion.

#### 4.6.3 Frame Scheduling

The terminal does not render on a fixed timer. Instead, it renders on demand:

1. PTY data arrives → process bytes through VT parser → update grid
2. If any rows are dirty and sync mode is off → render dirty rows → report damage
3. Compositor includes the terminal surface in its next frame

For high-throughput output (compilation, log streaming), the terminal batches multiple PTY reads into a single render pass. A configurable debounce delay (default 8ms, ~120fps equivalent) prevents excessive rendering during burst output.

For cursor blink, the terminal reports a small damage region at the cursor position on a timer (configurable, default 530ms on / 530ms off, matching xterm defaults).

### 4.7 Performance Model

The rendering pipeline operates under strict latency and throughput budgets. These targets ensure the terminal feels responsive for interactive use while maintaining efficiency during bulk output. For measurement methodology and verification procedures, see [testing.md](./testing.md) §14.

#### 4.7.1 Latency Targets

| Scenario | p50 Target | p99 Target | Notes |
|---|---|---|---|
| Keystroke echo (idle terminal) | <4ms | <8ms | Single compositor frame at 120fps |
| Interactive output (line-by-line) | <8ms | <16ms | One frame at 60fps |
| Bulk output (compilation) | <16ms | <33ms | Throughput-optimized, frame skipping allowed |
| Cursor blink toggle | <2ms | <4ms | Damage region is single cell |
| Window resize | <16ms | <50ms | Full grid rebuild + surface resize |

Latency is measured from input event timestamp (T₁) to the compositor frame that includes the rendered result (T₂). The keystroke echo target (<4ms p50) means a character appears on screen within the same compositor frame at 120fps refresh rates.

#### 4.7.2 Throughput Targets

```text
VT Parser:
  Sustained throughput:  ≥100 MB/s (Alacritty-class, 10ms for 1MB of output)
  Burst throughput:      ≥500 MB/s (short bursts with pre-allocated buffers)
  Pure ASCII:            ≥1 GB/s (no escape sequences, straight Print action)
  Heavy CSI:             ≥50 MB/s (dense escape sequences, e.g., colored ls -la)

Rendering (grid-to-surface):
  80×24 (standard):     <2ms full redraw, <0.5ms single row
  120×40 (medium):      <4ms full redraw, <0.5ms single row
  240×80 (large):       <8ms full redraw, <1ms single row

Frame rate under sustained output:
  Target: ≥60fps visible frame rate with frame skipping
  Method: debounce at 8ms, skip intermediate frames during burst
```

#### 4.7.3 Memory Budgets

| Resource | Default Budget | Maximum | Notes |
|---|---|---|---|
| Glyph atlas (GPU) | 4 MB | 16 MB | Grows on demand, LRU eviction |
| Scrollback (memory tier) | 2 MB (~10K lines) | 20 MB (~100K lines) | Oldest lines spill to space tier |
| Surface buffer (BGRA8888) | 1.9 MB (800×600) | 7.7 MB (1920×1080) | Proportional to surface dimensions |
| PTY shared memory | 64 KB | 256 KB | Grows under sustained output |
| Cell grid (in-memory) | 96 KB (80×24×50B/cell) | 960 KB (240×80×50B/cell) | Fixed per terminal size |
| VT parser state | 4.2 KB | 4.2 KB (fixed) | ParamBuffer + OscBuffer + state |
| Session state (per session) | ~8 KB | ~8 KB | Channels + notification + metadata |

Memory metrics are reported to the observability subsystem every 10 seconds. Alerts trigger if any resource exceeds 80% of its maximum budget.

#### 4.7.4 Adaptive Strategies

The terminal adapts its rendering behavior based on output rate:

- **Frame skipping:** When output rate exceeds the compositor frame rate, the terminal skips intermediate render passes. Only the most recent grid state is rendered, reducing GPU/CPU load without visible degradation.
- **Dirty coalescing:** Adjacent dirty rows are merged into a single damage rectangle. When more than 50% of rows are dirty, the terminal reports `FullSurface` damage instead of individual row rectangles.
- **Debounce:** A configurable delay (default 8ms) batches rapid PTY reads into single render passes. This prevents rendering every byte of a `cat large_file` individually.
- **Scrollback pressure:** When the memory-tier scrollback approaches its limit, the terminal increases the spill rate to the space tier. Under extreme pressure, it reduces the memory tier size temporarily and relies on space-backed retrieval for scroll-up operations.
