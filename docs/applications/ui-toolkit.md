# AIOS Portable UI Toolkit

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [compositor.md](../platform/compositor.md) — Compositor protocol, [experience.md](../experience/experience.md) — Experience layer surfaces, [agents.md](./agents.md) — Agent SDK, [flow.md](../storage/flow.md) — Flow drag/drop integration, [context-engine.md](../intelligence/context-engine.md) — Context-aware adaptation

-----

## 1. Overview

A new operating system with no applications is dead on arrival. Developers won't invest in building for a platform with zero users, and users won't adopt a platform with no software. This is the cold-start problem that has killed every desktop OS challenger since Windows and macOS locked in their positions.

AIOS breaks this deadlock with a portable UI toolkit. The same application code runs on Linux, macOS, Web, and AIOS. Developers build and test on their current platform — macOS with Xcode, Linux with their favorite editor — and deploy to AIOS without modification. The toolkit abstracts the platform away. When running on AIOS, applications gain capabilities that don't exist elsewhere (semantic window hints, Flow integration, space-backed persistence, capability-aware UI). On other platforms, these features gracefully degrade to standard behavior.

**Why this matters:**

1. **Developer adoption.** Build and test on a familiar platform. No AIOS boot required for development. The edit-compile-test loop stays fast.
2. **Ecosystem bootstrapping.** Developers invest knowing their work isn't trapped on a zero-user platform. An AIOS agent is also a Linux application and a macOS application.
3. **Proving abstractions.** Multi-platform support proves the toolkit design isn't accidentally coupled to kernel internals. If it works on Linux and macOS, the abstractions are clean.
4. **Fast iteration.** Edit on Mac, test in QEMU, deploy to hardware. No context switching between toolchains.

**Toolkit choice: iced.** Elm-inspired, pure Rust, MIT-licensed, GPU-rendered via wgpu. Already works on Linux/macOS/Windows/Web. The architecture naturally separates platform from toolkit. Adding an AIOS backend is a defined engineering task, not research. The Elm Architecture (Model-View-Update) maps cleanly to the agent model: each agent is an iced `Application` with its own state, message loop, and view function.

-----

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Agent Application Code                       │
│            (identical across all platforms)                       │
│                                                                  │
│   struct App { state }                                           │
│   fn update(&mut self, msg) → Command                            │
│   fn view(&self) → Element                                       │
│   fn subscription(&self) → Subscription                          │
└──────────────────────────┬──────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                   UI Toolkit — Portable Core                     │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────┐ │
│  │ Widget Library│  │ Layout Engine │  │ Theme System          │ │
│  │ button, text, │  │ flexbox-like  │  │ colors, fonts,        │ │
│  │ input, list,  │  │ constraints   │  │ spacing, context-     │ │
│  │ scroll, image │  │ propagation   │  │ aware adaptation      │ │
│  └──────────────┘  └──────────────┘  └───────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────┐ │
│  │ Event Model   │  │ Render Tree   │  │ Text Layout           │ │
│  │ click, hover, │  │ diff, damage  │  │ shaping, line break,  │ │
│  │ focus, keybd  │  │ display list  │  │ bidi, font fallback   │ │
│  └──────────────┘  └──────────────┘  └───────────────────────┘ │
└──────────────────────────┬──────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                   Platform Backend (one per target)               │
│                                                                  │
│  ┌─────────────────┐  ┌──────────────────┐  ┌────────────────┐ │
│  │ AIOS             │  │ Linux             │  │ macOS          │ │
│  │ Compositor proto  │  │ wgpu + winit      │  │ wgpu + winit   │ │
│  │ + GPU direct      │  │ (Wayland/X11)     │  │ (Metal)        │ │
│  │ + semantic hints  │  │                   │  │                │ │
│  │ + Flow integration│  │                   │  │                │ │
│  └─────────────────┘  └──────────────────┘  └────────────────┘ │
│                                                                  │
│  ┌─────────────────┐                                            │
│  │ Web              │                                            │
│  │ Canvas + DOM     │                                            │
│  │ (WASM target)    │                                            │
│  └─────────────────┘                                            │
└─────────────────────────────────────────────────────────────────┘
```

The architecture enforces a strict separation between portable logic and platform-specific code. The portable core contains:

- **Widget library** — all UI elements, their state, their event handling
- **Layout engine** — constraint-based positioning, sizing, alignment
- **Theme system** — color, typography, spacing tokens with context-aware adaptation
- **Event model** — input event routing, focus management, gesture recognition
- **Render tree** — diffing, damage tracking, display list generation
- **Text layout** — shaping, line breaking, bidirectional text, font fallback

The platform backend implements a trait that the portable core calls:

```rust
pub trait PlatformBackend {
    /// Create a window/surface
    fn create_surface(&mut self, hints: SurfaceHints) -> SurfaceId;

    /// Submit a display list for rendering
    fn submit(&mut self, surface: SurfaceId, display_list: &DisplayList);

    /// Poll for platform events
    fn poll_events(&mut self) -> Vec<PlatformEvent>;

    /// Set clipboard content
    fn set_clipboard(&mut self, content: ClipboardContent);

    /// Get clipboard content
    fn get_clipboard(&self) -> Option<ClipboardContent>;

    /// Query platform capabilities
    fn capabilities(&self) -> PlatformCapabilities;

    /// Request animation frame
    fn request_frame(&mut self, surface: SurfaceId);
}

pub struct PlatformCapabilities {
    pub semantic_hints: bool,       // AIOS only
    pub flow_integration: bool,     // AIOS only
    pub space_backed_data: bool,    // AIOS only
    pub capability_aware_ui: bool,  // AIOS only
    pub gpu_rendering: bool,        // all except some web
    pub high_dpi: bool,
    pub touch_input: bool,
}
```

-----

## 3. The Elm Architecture in AIOS

Iced follows the Elm Architecture: **Model → View → Update**. This maps naturally to AIOS agents because each agent is an isolated process with its own state. There is no shared mutable state between agents — exactly what Elm prescribes.

### 3.1 The Pattern

```rust
use aios_toolkit::prelude::*;

/// The application state (Model)
struct NotesApp {
    notes: Vec<Note>,
    search_query: String,
    selected: Option<usize>,
    theme: Theme,
}

/// Messages that can change state (Actions)
#[derive(Debug, Clone)]
enum Message {
    SearchChanged(String),
    NoteSelected(usize),
    NoteCreated,
    NoteDeleted(usize),
    ContentEdited(String),
    ThemeChanged(Theme),
    SpaceSynced(Vec<Note>),    // from space subscription
}

impl Application for NotesApp {
    type Message = Message;
    type Theme = Theme;
    type Executor = executor::Default;
    type Flags = AppFlags;

    fn new(flags: AppFlags) -> (Self, Command<Message>) {
        let app = NotesApp {
            notes: Vec::new(),
            search_query: String::new(),
            selected: None,
            theme: flags.system_theme,
        };
        // Load notes from space on startup
        let cmd = Command::perform(
            space::query("notes/*", QueryOrder::Modified),
            |result| Message::SpaceSynced(result.unwrap_or_default()),
        );
        (app, cmd)
    }

    fn title(&self) -> String {
        "Notes".into()
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::SearchChanged(query) => {
                self.search_query = query;
                Command::none()
            }
            Message::NoteSelected(idx) => {
                self.selected = Some(idx);
                Command::none()
            }
            Message::NoteCreated => {
                let note = Note::new();
                self.notes.push(note.clone());
                self.selected = Some(self.notes.len() - 1);
                // Persist to space
                Command::perform(
                    space::write(&format!("notes/{}", note.id), &note),
                    |_| Message::SearchChanged(String::new()),
                )
            }
            Message::NoteDeleted(idx) => {
                let note = self.notes.remove(idx);
                self.selected = None;
                Command::perform(
                    space::delete(&format!("notes/{}", note.id)),
                    |_| Message::SearchChanged(String::new()),
                )
            }
            Message::ContentEdited(content) => {
                if let Some(idx) = self.selected {
                    self.notes[idx].content = content;
                }
                Command::none()
            }
            Message::ThemeChanged(theme) => {
                self.theme = theme;
                Command::none()
            }
            Message::SpaceSynced(notes) => {
                self.notes = notes;
                Command::none()
            }
        }
    }

    fn view(&self) -> Element<Message> {
        let sidebar = column![
            text_input("Search...", &self.search_query)
                .on_input(Message::SearchChanged),
            button("New Note").on_press(Message::NoteCreated),
            self.note_list(),
        ]
        .width(Length::FillPortion(1))
        .spacing(8);

        let editor = if let Some(idx) = self.selected {
            text_editor(&self.notes[idx].content)
                .on_edit(Message::ContentEdited)
                .into()
        } else {
            text("Select a note").size(16).into()
        };

        let content = row![
            sidebar,
            vertical_rule(1),
            container(editor).width(Length::FillPortion(3)).padding(16),
        ];

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        // Watch the space for external changes (other agents, sync)
        space::watch("notes/*").map(|_| {
            Message::SpaceSynced(Vec::new()) // trigger reload
        })
    }

    fn theme(&self) -> Theme {
        self.theme.clone()
    }
}
```

### 3.2 Why Elm Architecture Fits AIOS

|Property|Elm Architecture|AIOS Agent Model|
|--------|---------------|----------------|
|Isolated state|Each app owns its state|Each agent owns its memory (TTBR0)|
|Message passing|Events → messages → update|IPC messages → capability checked|
|No shared mutation|State only changes in `update`|No shared memory between agents|
|Declarative view|`view()` returns a description|Compositor renders declarative surface|
|Side effects are explicit|`Command` for async work|IPC for all external interaction|
|Subscriptions|Listen for external events|IPC channels, space watches|

The Elm Architecture is not just a UI pattern in AIOS — it's the natural expression of the capability-isolated agent model at the GUI level.

### 3.3 Command System

Commands represent side effects — things the application wants to happen outside its own state:

```rust
pub enum Command<M> {
    None,
    /// Execute an async operation and map the result to a message
    Perform(Box<dyn Future<Output = M>>),
    /// Batch multiple commands
    Batch(Vec<Command<M>>),
    /// Platform-specific command (AIOS features)
    Platform(PlatformCommand<M>),
}

pub enum PlatformCommand<M> {
    /// Post an attention item (AIOS only, no-op elsewhere)
    PostAttention(AttentionRequest),
    /// Push data to Flow tray (AIOS only, clipboard elsewhere)
    FlowPush(FlowData),
    /// Set semantic window hints (AIOS only, ignored elsewhere)
    SetWindowHints(WindowHints),
    /// Query agent capabilities (AIOS only, returns all-granted elsewhere)
    QueryCapabilities(Box<dyn Fn(CapabilitySet) -> M>),
}
```

-----

## 4. Widget Library

### 4.1 Core Widgets

The toolkit ships a complete widget set. Every widget is a Rust struct implementing the `Widget` trait:

```rust
pub trait Widget<M, R: Renderer> {
    /// Calculate the intrinsic size given constraints
    fn layout(&self, renderer: &R, limits: &Limits) -> Node;

    /// Render the widget to the renderer
    fn draw(
        &self,
        renderer: &mut R,
        theme: &Theme,
        style: &Style,
        layout: Layout<'_>,
        cursor: Cursor,
    );

    /// Handle an event, optionally producing a message
    fn on_event(
        &mut self,
        event: Event,
        layout: Layout<'_>,
        cursor: Cursor,
        shell: &mut Shell<'_, M>,
    ) -> Status;

    /// Return child widgets for the accessibility tree
    fn children(&self) -> Vec<Tree>;
}
```

**Layout widgets:**

|Widget|Description|
|------|-----------|
|`row![]`|Horizontal flex container|
|`column![]`|Vertical flex container|
|`container()`|Single-child wrapper with padding, alignment, sizing|
|`scrollable()`|Scrollable viewport (vertical, horizontal, or both)|
|`stack![]`|Z-axis layering (overlapping children)|
|`responsive()`|Layout that adapts to available size|
|`space()`|Flexible spacer for distributing space|

**Content widgets:**

|Widget|Description|
|------|-----------|
|`text()`|Static text display with font, size, color|
|`image()`|Image display (PNG, JPEG, SVG, WebP)|
|`canvas()`|Freeform 2D drawing surface|
|`markdown()`|Markdown renderer with inline formatting|
|`rich_text()`|Styled text with multiple spans|

**Input widgets:**

|Widget|Description|
|------|-----------|
|`button()`|Clickable button with label or child widget|
|`text_input()`|Single-line text input with placeholder|
|`text_editor()`|Multi-line text editor with syntax highlighting|
|`checkbox()`|Boolean toggle with label|
|`radio()`|Single-select from a group|
|`toggler()`|On/off switch|
|`slider()`|Numeric range slider|
|`pick_list()`|Dropdown selection|
|`combo_box()`|Searchable dropdown|

**Feedback widgets:**

|Widget|Description|
|------|-----------|
|`tooltip()`|Hover tooltip on any widget|
|`progress_bar()`|Determinate progress indicator|
|`spinner()`|Indeterminate loading indicator|
|`notification()`|Toast-style transient message|

**Overlay widgets:**

|Widget|Description|
|------|-----------|
|`modal()`|Modal dialog overlay|
|`menu()`|Context menu / dropdown menu|
|`pane_grid()`|Resizable multi-pane layout|

### 4.2 Custom Widgets

Agents extend the widget set by implementing the `Widget` trait. Custom widgets compose from primitives (rectangles, text, images, paths) and can contain child widgets:

```rust
pub struct AgentStateBadge {
    agent_name: String,
    status: AgentState,
    resource_usage: ResourceUsage,
}

impl<M, R: Renderer> Widget<M, R> for AgentStateBadge {
    fn layout(&self, _renderer: &R, limits: &Limits) -> Node {
        Node::new(Size::new(
            limits.max().width.min(200.0),
            32.0,
        ))
    }

    fn draw(
        &self,
        renderer: &mut R,
        theme: &Theme,
        _style: &Style,
        layout: Layout<'_>,
        _cursor: Cursor,
    ) {
        let bounds = layout.bounds();
        let colors = theme.agent_status_colors();

        // Status indicator dot
        let dot_color = match self.status {
            AgentState::Active => colors.active,
            AgentState::Paused => colors.paused,
            AgentState::Terminated => colors.stopped,
        };
        renderer.fill_quad(
            Quad::circle(bounds.position() + Vector::new(8.0, 16.0), 4.0),
            dot_color,
        );

        // Agent name
        renderer.fill_text(Text {
            content: &self.agent_name,
            position: bounds.position() + Vector::new(20.0, 8.0),
            size: 14.0,
            color: colors.text,
            font: Font::DEFAULT,
        });
    }

    fn on_event(&mut self, _event: Event, _layout: Layout<'_>,
                _cursor: Cursor, _shell: &mut Shell<'_, M>) -> Status {
        Status::Ignored
    }

    fn children(&self) -> Vec<Tree> {
        Vec::new()
    }
}
```

-----

## 5. Layout Engine

### 5.1 Constraint-Based Layout

The layout engine uses a constraint propagation model similar to CSS Flexbox. Parent widgets pass size constraints down; child widgets return their computed size up:

```rust
pub struct Limits {
    min: Size,
    max: Size,
}

pub struct Node {
    size: Size,
    children: Vec<Node>,
}

impl Limits {
    /// Constrain width to fill available space
    pub fn width(self, length: Length) -> Self { /* ... */ }

    /// Constrain height to fill available space
    pub fn height(self, length: Length) -> Self { /* ... */ }

    /// Resolve final size within constraints
    pub fn resolve(self, intrinsic: Size) -> Size { /* ... */ }
}

pub enum Length {
    /// Fill available space
    Fill,
    /// Fill proportional to other FillPortion siblings
    FillPortion(u16),
    /// Shrink to content
    Shrink,
    /// Fixed pixel size
    Fixed(f32),
}
```

### 5.2 Layout Pass

Layout is a single top-down, bottom-up pass:

```
1. Root receives screen constraints (0,0 → screen_width, screen_height)
2. Root passes constraints to children (minus padding, spacing)
3. Each child computes its intrinsic size within constraints
4. Parent arranges children (row: horizontal, column: vertical)
5. Final positions are absolute screen coordinates
```

```rust
/// Layout computation for a Column widget
fn layout_column(
    children: &[Element],
    renderer: &Renderer,
    limits: &Limits,
    spacing: f32,
    padding: Padding,
) -> Node {
    let available = limits.max() - padding.total();
    let total_spacing = spacing * (children.len().saturating_sub(1)) as f32;
    let mut remaining_height = available.height - total_spacing;

    // First pass: measure Shrink and Fixed children
    let mut child_nodes: Vec<Option<Node>> = vec![None; children.len()];
    let mut fill_count = 0u16;
    let mut fill_total = 0u16;

    for (i, child) in children.iter().enumerate() {
        match child.height() {
            Length::Shrink | Length::Fixed(_) => {
                let child_limits = Limits::new(
                    Size::ZERO,
                    Size::new(available.width, remaining_height),
                );
                let node = child.layout(renderer, &child_limits);
                remaining_height -= node.size.height;
                child_nodes[i] = Some(node);
            }
            Length::Fill => { fill_count += 1; fill_total += 1; }
            Length::FillPortion(p) => { fill_count += 1; fill_total += p; }
        }
    }

    // Second pass: distribute remaining space to Fill children
    let fill_height = remaining_height / fill_total as f32;
    for (i, child) in children.iter().enumerate() {
        if child_nodes[i].is_none() {
            let portion = match child.height() {
                Length::FillPortion(p) => p,
                _ => 1,
            };
            let child_limits = Limits::new(
                Size::ZERO,
                Size::new(available.width, fill_height * portion as f32),
            );
            child_nodes[i] = Some(child.layout(renderer, &child_limits));
        }
    }

    // Position children vertically
    let mut y = padding.top;
    let mut nodes = Vec::new();
    for node in child_nodes.into_iter().flatten() {
        let positioned = node.move_to(Point::new(padding.left, y));
        y += positioned.size.height + spacing;
        nodes.push(positioned);
    }

    Node::with_children(
        Size::new(available.width + padding.horizontal(), y - spacing + padding.bottom),
        nodes,
    )
}
```

### 5.3 Responsive Layouts

The `responsive` widget adapts layout based on available size:

```rust
fn view(&self) -> Element<Message> {
    responsive(|size| {
        if size.width > 800.0 {
            // Wide layout: sidebar + content
            row![
                self.sidebar().width(Length::FillPortion(1)),
                self.content().width(Length::FillPortion(3)),
            ].into()
        } else {
            // Narrow layout: stacked
            column![
                self.sidebar().height(Length::Shrink),
                self.content().height(Length::Fill),
            ].into()
        }
    })
}
```

-----

## 6. Theme System

### 6.1 Theme Tokens

The theme system uses a token-based approach. Every visual property references a token, not a hardcoded value:

```rust
pub struct Theme {
    pub palette: Palette,
    pub typography: Typography,
    pub spacing: Spacing,
    pub radius: Radius,
    pub animation: AnimationConfig,
}

pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_disabled: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub border: Color,
    pub shadow: Color,
}

pub struct Typography {
    pub heading_large: TextStyle,   // 28px, bold
    pub heading_medium: TextStyle,  // 22px, semibold
    pub heading_small: TextStyle,   // 18px, semibold
    pub body: TextStyle,            // 15px, regular
    pub body_small: TextStyle,      // 13px, regular
    pub caption: TextStyle,         // 11px, regular
    pub monospace: TextStyle,       // 14px, monospace
}

pub struct Spacing {
    pub xs: f32,    // 4
    pub sm: f32,    // 8
    pub md: f32,    // 16
    pub lg: f32,    // 24
    pub xl: f32,    // 32
    pub xxl: f32,   // 48
}
```

### 6.2 Context-Aware Themes

On AIOS, the theme adapts to the Context Engine's inferred state:

```rust
pub fn theme_for_context(base: &Theme, context: &ContextState) -> Theme {
    let mut theme = base.clone();

    match context.mode {
        ContextMode::Work => {
            // Higher information density
            theme.spacing = Spacing::compact();
            // Neutral, focused palette
            theme.palette.background = Color::from_rgb(0.97, 0.97, 0.98);
        }
        ContextMode::Leisure => {
            // Lower density, more breathing room
            theme.spacing = Spacing::relaxed();
            // Warmer tones
            theme.palette.background = Color::from_rgb(0.98, 0.97, 0.95);
        }
        ContextMode::Focus => {
            // Minimal chrome, maximum content area
            theme.spacing = Spacing::minimal();
            // Reduced contrast for non-focused elements
            theme.palette.text_secondary = theme.palette.text_disabled;
        }
        ContextMode::Gaming => {
            // Dark theme, high contrast
            theme.palette = Palette::dark();
        }
    }

    // Time-of-day adjustment
    if context.time_of_day.hour() >= 20 || context.time_of_day.hour() < 6 {
        theme.palette = theme.palette.warm_shift(0.05);
    }

    theme
}
```

On non-AIOS platforms, `theme_for_context` is never called. The application uses the base theme directly, or the system light/dark mode preference.

### 6.3 Agent Theming

Agents can customize their theme within bounds set by the system:

```rust
pub struct AgentThemeOverride {
    /// Accent color (agent branding)
    pub accent: Option<Color>,
    /// Whether to use a custom icon set
    pub icon_set: Option<IconSet>,
    /// Font override (must be available on system)
    pub font: Option<Font>,
}

impl AgentThemeOverride {
    /// Apply agent overrides to system theme, clamping to accessibility bounds
    pub fn apply(&self, system_theme: &Theme) -> Theme {
        let mut theme = system_theme.clone();
        if let Some(accent) = self.accent {
            // Ensure sufficient contrast ratio (WCAG AA: 4.5:1)
            if contrast_ratio(accent, theme.palette.background) >= 4.5 {
                theme.palette.accent = accent;
            }
        }
        theme
    }
}
```

-----

## 7. Text Rendering

Text is the hardest part of any UI toolkit. AIOS is a text-forward OS — summaries, search results, conversation, code — so text rendering must be excellent.

### 7.1 Text Pipeline

```
Input string (Unicode)
  │
  ▼
Itemization (split into runs by script, direction, font)
  │
  ▼
Font fallback (select font for each run from fallback chain)
  │
  ▼
Shaping (Unicode codepoints → positioned glyphs)
  │    Uses: swash (pure Rust) or harfbuzz (C, via harfbuzz-rs)
  │    Handles: ligatures, kerning, contextual alternates
  ▼
Line breaking (Unicode UAX #14 line break algorithm)
  │    Handles: word wrap, hyphenation, CJK break rules
  ▼
Bidi reordering (Unicode UAX #9 bidirectional algorithm)
  │    Handles: mixed LTR/RTL text (English + Arabic)
  ▼
Layout (position lines, compute baselines, alignment)
  │
  ▼
Rasterization (glyphs → GPU texture atlas)
  │    Subpixel positioning for crisp rendering
  ▼
Rendering (textured quads to GPU)
```

### 7.2 Font Fallback

```rust
pub struct FontFallbackChain {
    fonts: Vec<FontDescriptor>,
}

impl FontFallbackChain {
    pub fn system_default() -> Self {
        FontFallbackChain {
            fonts: vec![
                FontDescriptor::new("Inter", Weight::NORMAL),       // Latin
                FontDescriptor::new("Noto Sans CJK", Weight::NORMAL), // CJK
                FontDescriptor::new("Noto Sans Arabic", Weight::NORMAL), // Arabic
                FontDescriptor::new("Noto Color Emoji", Weight::NORMAL), // Emoji
                FontDescriptor::new("Symbols Nerd Font", Weight::NORMAL), // Icons
            ],
        }
    }

    /// Find the first font that contains the given character
    pub fn font_for_char(&self, ch: char) -> &FontDescriptor {
        for font in &self.fonts {
            if font.contains_glyph(ch) {
                return font;
            }
        }
        &self.fonts[0] // fallback to primary
    }
}
```

### 7.3 Glyph Cache

Glyphs are rasterized once and cached in a GPU texture atlas. The cache is keyed by `(font_id, glyph_id, size, subpixel_offset)`:

```rust
pub struct GlyphCache {
    atlas: TextureAtlas,
    entries: HashMap<GlyphKey, AtlasEntry>,
    lru: LruIndex,
}

pub struct GlyphKey {
    font_id: FontId,
    glyph_id: u16,
    size: OrderedFloat<f32>,
    subpixel_x: u8,  // 0-3 for quarter-pixel positioning
    subpixel_y: u8,
}

impl GlyphCache {
    pub fn get_or_rasterize(
        &mut self,
        key: GlyphKey,
        rasterizer: &mut Rasterizer,
    ) -> &AtlasEntry {
        if !self.entries.contains_key(&key) {
            let image = rasterizer.rasterize(key.font_id, key.glyph_id, key.size);
            let entry = self.atlas.allocate(image.width, image.height);
            self.atlas.upload(entry.region, &image.data);
            self.entries.insert(key, entry);
        }
        self.lru.touch(&key);
        &self.entries[&key]
    }
}
```

-----

## 8. Render Pipeline

### 8.1 From Widgets to Pixels

```
Widget Tree (declarative, returned by view())
  │
  ▼
Layout Tree (positioned nodes with absolute coordinates)
  │
  ▼
Diff (compare with previous frame's tree)
  │    Only process changed subtrees
  ▼
Display List (flat list of draw commands)
  │    Primitives: quad, text, image, clip, transform
  ▼
Damage Tracking (regions that changed since last frame)
  │    Only re-render damaged rectangles
  ▼
GPU Submission (wgpu render pass)
  │    Batched draw calls, texture atlas
  ▼
Present (swap chain present / compositor submit)
```

### 8.2 Display List Primitives

```rust
pub enum Primitive {
    /// Filled rectangle with optional rounded corners
    Quad {
        bounds: Rectangle,
        background: Background,
        border: Border,
        shadow: Shadow,
    },
    /// Rendered text run
    Text {
        content: String,
        bounds: Rectangle,
        color: Color,
        size: f32,
        font: Font,
        alignment: Alignment,
    },
    /// Image from texture atlas or standalone
    Image {
        handle: ImageHandle,
        bounds: Rectangle,
        filter: ImageFilter,
    },
    /// Clip all children to a rectangle
    Clip {
        bounds: Rectangle,
        children: Vec<Primitive>,
    },
    /// Affine transform
    Transform {
        transform: Transform2D,
        children: Vec<Primitive>,
    },
    /// Custom shader (for canvas widget)
    Shader {
        bounds: Rectangle,
        program: ShaderProgram,
    },
}
```

### 8.3 Damage Tracking

Only redraw regions that changed. This is critical for battery life and GPU efficiency:

```rust
pub struct DamageTracker {
    previous_display_list: Vec<Primitive>,
    damaged_regions: Vec<Rectangle>,
}

impl DamageTracker {
    pub fn compute_damage(
        &mut self,
        current: &[Primitive],
    ) -> &[Rectangle] {
        self.damaged_regions.clear();

        for (prev, curr) in self.previous_display_list.iter().zip(current.iter()) {
            if prev != curr {
                // Mark both old and new bounds as damaged
                self.damaged_regions.push(prev.bounds());
                self.damaged_regions.push(curr.bounds());
            }
        }

        // Handle list length changes
        if current.len() > self.previous_display_list.len() {
            for prim in &current[self.previous_display_list.len()..] {
                self.damaged_regions.push(prim.bounds());
            }
        }

        self.previous_display_list = current.to_vec();
        self.merge_overlapping(&mut self.damaged_regions);
        &self.damaged_regions
    }
}
```

### 8.4 Frame Pacing

Target 60fps (16.6ms per frame). Budget breakdown:

|Phase|Budget|
|-----|------|
|Event handling + `update()`|1ms|
|`view()` — build widget tree|2ms|
|Layout|2ms|
|Diff + damage|1ms|
|Display list generation|2ms|
|GPU submission|2ms|
|GPU rendering|4ms|
|Present + vsync|2.6ms|
|**Total**|**16.6ms**|

If layout or rendering exceeds budget, the toolkit skips frames rather than dropping interactivity. Input events are always processed — the view may lag but never the response.

-----

## 9. Platform Backends

### 9.1 AIOS Backend

The AIOS backend communicates with the compositor via IPC instead of creating its own window:

```rust
pub struct AiosBackend {
    compositor_channel: IpcChannel,
    surface_buffers: HashMap<SurfaceId, GpuBuffer>,
    flow_channel: Option<IpcChannel>,
    capability_set: CapabilitySet,
    context_subscription: Option<IpcChannel>,
}

impl PlatformBackend for AiosBackend {
    fn create_surface(&mut self, hints: SurfaceHints) -> SurfaceId {
        // Send semantic hints to compositor (content type, resize behavior)
        let msg = CompositorMsg::CreateSurface {
            hints: hints.into(),
            buffer_format: BufferFormat::Bgra8,
        };
        self.compositor_channel.send(&msg);

        let response: CompositorResponse = self.compositor_channel.recv();
        let surface_id = response.surface_id;

        // Allocate GPU buffer for this surface
        let buffer = GpuBuffer::allocate(hints.initial_size);
        self.surface_buffers.insert(surface_id, buffer);

        surface_id
    }

    fn submit(&mut self, surface: SurfaceId, display_list: &DisplayList) {
        let buffer = &mut self.surface_buffers[&surface];

        // Render display list to GPU buffer
        let encoder = buffer.begin_render_pass();
        render_display_list(&display_list, &mut encoder);
        encoder.finish();

        // Share buffer handle with compositor (zero-copy)
        self.compositor_channel.send(&CompositorMsg::SubmitBuffer {
            surface,
            buffer_handle: buffer.share_handle(),
            damage: display_list.damage_regions(),
        });
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            semantic_hints: true,
            flow_integration: true,
            space_backed_data: true,
            capability_aware_ui: true,
            gpu_rendering: true,
            high_dpi: true,
            touch_input: true,
        }
    }
}
```

**Unique AIOS capabilities:**

- **GPU buffer sharing.** The agent renders to a GPU buffer and shares the handle with the compositor via IPC. No pixel copying — the compositor composites directly from the agent's buffer.
- **Semantic window hints.** The agent tells the compositor what kind of content it displays (text editor, media player, terminal). The compositor uses this for intelligent layout, animations, and context transitions.
- **Flow integration.** Drag events route through the Flow system, preserving data types and provenance. Dropping an image from a browser into a notes agent carries the source URL and content type.
- **Capability-aware rendering.** Widgets can query the agent's capability set and disable or hide elements the agent cannot use.

### 9.2 Linux Backend

Standard iced behavior — wgpu + winit on Wayland or X11:

```rust
pub struct LinuxBackend {
    window: winit::window::Window,
    surface: wgpu::Surface,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl PlatformBackend for LinuxBackend {
    fn create_surface(&mut self, hints: SurfaceHints) -> SurfaceId {
        // Ignore AIOS-specific hints, create standard window
        self.window.set_title(&hints.title);
        self.window.set_inner_size(hints.initial_size.into());
        SurfaceId(0) // single window
    }

    fn submit(&mut self, _surface: SurfaceId, display_list: &DisplayList) {
        let frame = self.surface.get_current_texture().unwrap();
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());

        render_display_list_wgpu(display_list, &view, &mut encoder);

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            semantic_hints: false,
            flow_integration: false,
            space_backed_data: false,
            capability_aware_ui: false,
            // LinuxBackend is only constructed after successful wgpu surface creation
            // and GPU initialization; headless / no-GPU paths use SoftwareBackend instead.
            gpu_rendering: true,
            high_dpi: true,
            touch_input: false,
        }
    }
}
```

### 9.3 macOS Backend

Identical to Linux backend but uses Metal via wgpu. winit handles Cocoa window management.

### 9.4 Web Backend

WASM target using Canvas API for rendering and DOM events for input:

```rust
pub struct WebBackend {
    canvas: web_sys::HtmlCanvasElement,
    context: web_sys::CanvasRenderingContext2d,
    event_queue: VecDeque<PlatformEvent>,
}

impl PlatformBackend for WebBackend {
    fn submit(&mut self, _surface: SurfaceId, display_list: &DisplayList) {
        self.context.clear_rect(
            0.0, 0.0,
            self.canvas.width() as f64,
            self.canvas.height() as f64,
        );

        for primitive in display_list.primitives() {
            match primitive {
                Primitive::Quad { bounds, background, border, .. } => {
                    self.context.set_fill_style(&background.to_css().into());
                    self.context.fill_rect(
                        bounds.x as f64, bounds.y as f64,
                        bounds.width as f64, bounds.height as f64,
                    );
                }
                Primitive::Text { content, bounds, color, size, .. } => {
                    self.context.set_fill_style(&color.to_css().into());
                    self.context.set_font(&format!("{}px Inter", size));
                    self.context.fill_text(
                        content, bounds.x as f64, bounds.y as f64,
                    ).ok();
                }
                _ => { /* ... */ }
            }
        }
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            semantic_hints: false,
            flow_integration: false,
            space_backed_data: false,
            capability_aware_ui: false,
            gpu_rendering: false, // Canvas 2D, not WebGPU
            high_dpi: true,
            touch_input: true,
        }
    }
}
```

-----

## 10. AIOS-Specific Features

These features are only available when running on AIOS. On other platforms, the API calls compile but return defaults or no-ops. Application code uses feature detection, not conditional compilation:

```rust
fn update(&mut self, message: Message) -> Command<Message> {
    match message {
        Message::ShareData(data) => {
            if platform::capabilities().flow_integration {
                // On AIOS: push to Flow tray with full type info
                Command::platform(PlatformCommand::FlowPush(FlowData {
                    content: data,
                    content_type: ContentType::Document,
                    provenance: Provenance::current(),
                }))
            } else {
                // Elsewhere: standard clipboard
                Command::clipboard(ClipboardAction::Write(data.to_string()))
            }
        }
        _ => Command::none(),
    }
}
```

### 10.1 Semantic Window Hints

```rust
pub struct WindowHints {
    /// What kind of content this window displays
    pub content_type: ContentType,
    /// How the window should behave during context transitions
    pub context_behavior: ContextBehavior,
    /// Resize constraints
    pub resize: ResizeHints,
    /// Whether the window should be included in overview mode
    pub show_in_overview: bool,
}

pub enum ContentType {
    TextEditor,
    Terminal,
    MediaPlayer,
    Browser,
    Communication,
    DataVisualization,
    Settings,
    Custom(String),
}

pub enum ContextBehavior {
    /// Dim during focus mode unless this is the focused window
    DimWhenUnfocused,
    /// Hide during leisure (e.g., work tools)
    HideInLeisure,
    /// Always visible (e.g., media player in leisure)
    AlwaysVisible,
    /// Follow default compositor rules
    Default,
}
```

The compositor uses these hints to make intelligent decisions about window layout, animations, and context transitions — without the application having to manage these itself.

### 10.2 Flow Integration

```rust
pub struct FlowData {
    /// The actual content
    pub content: TypedContent,
    /// MIME type and semantic type
    pub content_type: ContentType,
    /// Where this data came from
    pub provenance: Provenance,
    /// Suggested transformations for different destinations
    pub transforms: Vec<FlowTransform>,
}

pub enum FlowTransform {
    /// Strip formatting when dropping into terminal
    PlainText,
    /// Add provenance metadata when dropping into space
    WithProvenance,
    /// Convert image format when dropping into editor
    ImageConvert(ImageFormat),
}
```

When a user drags data from one agent to another, Flow preserves the full type information, provenance, and applies context-appropriate transformations.

### 10.3 Space-Backed Persistence

Agent state can be persisted to spaces automatically:

```rust
pub trait SpacePersistence {
    /// Save widget state to the agent's space
    fn save_state<S: Serialize>(&self, key: &str, state: &S) -> Command<()>;

    /// Load widget state from the agent's space
    fn load_state<D: DeserializeOwned>(&self, key: &str) -> Command<Option<D>>;

    /// Watch for external changes to persisted state
    fn watch_state(&self, key: &str) -> Subscription<StateChanged>;
}
```

On non-AIOS platforms, `save_state` writes to `~/.config/<agent>/` and `watch_state` uses filesystem notifications.

### 10.4 Capability-Aware UI

```rust
fn view(&self) -> Element<Message> {
    let mut controls = column![];

    controls = controls.push(
        button("Save").on_press(Message::Save)
    );

    // Only show network button if agent has network capability
    if self.capabilities.has(Capability::Network) {
        controls = controls.push(
            button("Sync").on_press(Message::Sync)
        );
    } else {
        controls = controls.push(
            button("Sync").style(Style::Disabled)
                .tooltip("Network access not granted")
        );
    }

    controls.into()
}
```

On non-AIOS platforms, `self.capabilities.has()` always returns `true` — the capability system doesn't exist, so all features are available.

-----

## 11. Agent UI Development

### 11.1 SDK Integration

The AIOS SDK provides the toolkit as a crate:

```toml
# Cargo.toml for a third-party agent
[package]
name = "weather-agent"
version = "0.1.0"

[dependencies]
aios-toolkit = "0.1"
aios-sdk = "0.1"

[target.'cfg(not(target_os = "aios"))'.dependencies]
# Fallback backends for development
aios-toolkit-linux = "0.1"
```

### 11.2 Manifest UI Declaration

The agent manifest declares UI requirements:

```toml
[agent]
name = "weather-agent"
version = "0.1.0"

[ui]
# Agent needs a window
surface = true
# Minimum and preferred window size
min_size = { width = 300, height = 200 }
preferred_size = { width = 400, height = 600 }
# Content type hint
content_type = "DataVisualization"
# Theme customization
accent_color = "#4A90D9"
```

### 11.3 Development Workflow

```
Developer's Mac/Linux Machine          AIOS Device (or QEMU)
┌───────────────────────┐              ┌──────────────────────┐
│                        │              │                       │
│  1. Write agent code   │              │  4. Deploy & test     │
│  2. cargo run          │    deploy    │     (full AIOS        │
│     (runs on Mac with  │ ─────────── │      features work)   │
│      winit backend)    │              │                       │
│  3. Test basic UI      │              │  5. Debug via         │
│     locally            │              │     Inspector         │
│                        │              │                       │
└───────────────────────┘              └──────────────────────┘
```

**Hot reload during development:**

```rust
// In debug mode, watch for source changes and reload view
#[cfg(debug_assertions)]
fn subscription(&self) -> Subscription<Message> {
    Subscription::batch(vec![
        // Watch agent source for hot reload
        file_watcher::watch("src/").map(|_| Message::HotReload),
        // Normal subscriptions
        self.normal_subscriptions(),
    ])
}
```

-----

## 12. Accessibility

### 12.1 Accessibility Tree

The widget hierarchy generates an accessibility tree automatically. Each widget has a semantic role:

```rust
pub enum AccessRole {
    Button,
    TextInput,
    Label,
    Heading(u8),     // h1-h6
    List,
    ListItem,
    Image,
    Slider,
    Checkbox,
    RadioButton,
    Tab,
    TabPanel,
    Dialog,
    Alert,
    Toolbar,
    Menu,
    MenuItem,
    ScrollArea,
    Generic,
}

pub struct AccessNode {
    pub role: AccessRole,
    pub label: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
    pub bounds: Rectangle,
    pub focusable: bool,
    pub focused: bool,
    pub disabled: bool,
    pub children: Vec<AccessNode>,
    pub actions: Vec<AccessAction>,
}
```

### 12.2 Screen Reader Integration

On AIOS, the accessibility tree is exposed via IPC to the screen reader agent. On Linux/macOS, it maps to AT-SPI2 / NSAccessibility.

### 12.3 Keyboard Navigation

All widgets support keyboard navigation by default:

- **Tab** moves focus between focusable widgets
- **Enter/Space** activates the focused widget
- **Arrow keys** navigate within composite widgets (lists, menus)
- **Escape** dismisses overlays and modals
- Focus indicators are always visible (never removed for aesthetics)

-----

## 13. Performance

### 13.1 Frame Budget Enforcement

```rust
pub struct FrameProfiler {
    budget_ms: f64,
    phase_timings: HashMap<Phase, f64>,
}

impl FrameProfiler {
    pub fn begin_phase(&mut self, phase: Phase) {
        self.phase_timings.insert(phase, Instant::now().as_millis_f64());
    }

    pub fn end_phase(&mut self, phase: Phase) -> f64 {
        let start = self.phase_timings[&phase];
        let elapsed = Instant::now().as_millis_f64() - start;
        elapsed
    }

    pub fn should_skip_frame(&self) -> bool {
        let total: f64 = self.phase_timings.values().sum();
        total > self.budget_ms * 1.5 // allow 50% overshoot before skipping
    }
}
```

### 13.2 Texture Atlas

All images, glyphs, and icons share a single GPU texture atlas to minimize draw calls and texture binding switches:

```rust
pub struct TextureAtlas {
    texture: wgpu::Texture,
    allocator: AtlasAllocator,
    size: u32, // 4096x4096 default
}

impl TextureAtlas {
    pub fn allocate(&mut self, width: u32, height: u32) -> AtlasRegion {
        match self.allocator.allocate(width, height) {
            Some(region) => region,
            None => {
                // Atlas full — evict least recently used entries
                self.evict_lru();
                self.allocator.allocate(width, height)
                    .expect("atlas eviction failed to free space")
            }
        }
    }
}
```

### 13.3 Agent UI Performance Guidelines

- Keep `view()` under 2ms — avoid allocations, use `lazy()` for expensive subtrees
- Use `lazy()` to skip unchanged subtrees during diff
- Avoid unbounded lists — use `virtual_list()` for scrollable content over 100 items
- Profile with the AIOS Inspector's frame timing panel

-----

## 14. Cross-Platform Development Workflow

### 14.1 Feature Detection Pattern

```rust
/// Check AIOS features at runtime, not compile time
fn setup(&self) -> Command<Message> {
    let caps = platform::capabilities();

    if caps.space_backed_data {
        // Load from space
        Command::perform(space::query("data/*", QueryOrder::Modified), Message::DataLoaded)
    } else {
        // Load from filesystem
        Command::perform(fs::read_dir("~/.config/myagent/data/"), Message::DataLoaded)
    }
}
```

### 14.2 CI/CD Pipeline

```
cargo test                        # unit tests (no platform)
cargo run --target x86_64-linux   # visual test on Linux
cargo run --target aarch64-aios   # test on AIOS (QEMU)
cargo build --target wasm32       # web build
aios-package build                # create .agent bundle
aios-package sign                 # sign with developer key
aios-package submit               # submit to Agent Store
```

### 14.3 Conditional Features

```rust
// In view(), feature detection controls UI elements
fn view(&self) -> Element<Message> {
    let mut root = column![];

    root = root.push(self.main_content());

    // Flow tray button only appears on AIOS
    if platform::capabilities().flow_integration {
        root = root.push(
            button("Send to Flow").on_press(Message::FlowPush)
        );
    }

    root.into()
}
```

-----

## 15. Implementation Order

```
Phase 6a:   iced integration scaffolding       → toolkit compiles for AIOS target
Phase 6b:   AIOS platform backend (basic)      → surfaces rendered via compositor IPC
Phase 6c:   Input routing                      → keyboard/mouse events reach widgets
Phase 6d:   Core widgets (text, button, input) → basic interactive UI works

Phase 9a:   Theme system                       → context-aware theming on AIOS
Phase 9b:   Full widget set                    → all standard widgets available
Phase 9c:   Text rendering pipeline            → shaping, bidi, font fallback

Phase 11a:  Flow integration                   → drag/drop through Flow system
Phase 11b:  Space-backed persistence           → agent state saved to spaces
Phase 11c:  Capability-aware UI                → widgets respond to capability set
Phase 11d:  Semantic window hints              → compositor understands content

Phase 15:   Accessibility tree                 → screen reader support
Phase 16:   Performance optimization           → damage tracking, texture atlas, profiling
Phase 18:   Web backend                        → WASM target works
Phase 20:   Agent SDK packaging                → aios-toolkit crate published
Phase 23:   Accessibility polish               → WCAG AA compliance
```

-----

## 16. Design Principles

1. **Portability is non-negotiable.** Every line of application code must compile and run on Linux, macOS, Web, and AIOS. Platform-specific features are additive, never required.

2. **AIOS features enhance, never gate.** An agent that uses Flow integration must still work (with clipboard fallback) on macOS. Capability-aware UI must still show all features on platforms without capabilities.

3. **Declarative, not imperative.** Applications describe what the UI should look like (`view()`), not how to mutate it. The toolkit handles diffing, damage tracking, and rendering.

4. **State is the truth.** The widget tree is a pure function of the application state. No hidden widget state. No out-of-band mutations. State changes only happen in `update()`.

5. **Performance is a feature, not an afterthought.** 60fps is the baseline. Damage tracking, lazy subtrees, virtual lists, and frame budgeting are built into the architecture, not bolted on.

6. **Accessibility is structural.** The accessibility tree is generated from the widget hierarchy, not annotated after the fact. If a widget exists, it's accessible.

7. **Agents own their UI, the system owns the chrome.** Window decorations, context transitions, focus indicators, and system overlays are compositor responsibilities. Agents control content. No agent can fake system UI.

8. **Same toolkit, native feel.** Because both system experience surfaces and third-party agents use iced, there's one visual language. Agent UIs don't feel like foreign widgets — they're native.
