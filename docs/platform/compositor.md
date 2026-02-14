# AIOS Compositor and Display Architecture

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [subsystem-framework.md](./subsystem-framework.md) — Display subsystem, [ipc.md](../kernel/ipc.md) — IPC protocol

-----

## 1. Core Insight

Traditional window managers are dumb frame compositors. They know nothing about window content — they paste rectangular pixel buffers together and present them to the display. Window management decisions (tiling, stacking, focus) are based on user commands, not content understanding.

AIOS's compositor is **semantically aware**. It receives hints from agents about what their windows contain, the user's context from the Context Engine, and attention state from the Attention Manager. It uses this to make intelligent layout, focus, and animation decisions — while still letting the user override everything manually.

The compositor is also the **display subsystem** in the subsystem framework. It implements the same capability gate, session model, audit logging, power management, and POSIX bridge as every other subsystem.

-----

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Experience Layer                          │
│  Workspace │ Browser │ Media Player │ Settings │ Inspector   │
│  (each an agent with compositor capabilities)                │
└────────────────────────┬────────────────────────────────────┘
                         │ IPC: surface creation, damage, hints
                         ▼
┌─────────────────────────────────────────────────────────────┐
│                      Compositor Service                       │
│                                                              │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────────┐  │
│  │   Window     │  │   Layout     │  │    Semantic       │  │
│  │   Manager    │  │   Engine     │  │    Hints          │  │
│  │             │  │              │  │                   │  │
│  │  z-order    │  │  tiling      │  │  content type    │  │
│  │  focus      │  │  floating    │  │  urgency         │  │
│  │  minimize   │  │  fullscreen  │  │  interaction     │  │
│  │  close      │  │  split       │  │  state           │  │
│  └─────────────┘  └──────────────┘  └───────────────────┘  │
│                                                              │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────────┐  │
│  │   Render    │  │   Animation  │  │    Input          │  │
│  │   Pipeline  │  │   System     │  │    Router         │  │
│  │             │  │              │  │                   │  │
│  │  damage     │  │  transitions │  │  focus → agent   │  │
│  │  composite  │  │  easing      │  │  global hotkeys  │  │
│  │  present    │  │  60fps       │  │  gesture recog   │  │
│  └─────────────┘  └──────────────┘  └───────────────────┘  │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │                  Accessibility Layer                    │ │
│  │  Accessibility tree │ Screen reader │ Keyboard nav     │ │
│  └────────────────────────────────────────────────────────┘ │
└────────────────────────┬────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│                      GPU Abstraction                          │
│                                                              │
│  wgpu (Rust GPU abstraction)                                 │
│  ├── Vulkan backend (primary, Pi 4/5)                        │
│  ├── VirtIO-GPU backend (QEMU development)                   │
│  └── Software renderer (fallback)                            │
│                                                              │
│  Render operations:                                          │
│  ├── Surface composition (alpha blend, z-order)              │
│  ├── Damage tracking (only redraw changed regions)           │
│  ├── VSync (tear-free presentation)                          │
│  └── Multi-monitor (independent render per output)           │
└────────────────────────┬────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│                    Display Drivers                            │
│  VirtIO-GPU │ VC4/V3D (Pi 4) │ HDMI │ DSI │ Framebuffer    │
└─────────────────────────────────────────────────────────────┘
```

-----

## 3. Compositor Protocol

Agents communicate with the compositor via IPC. The protocol defines how surfaces are created, updated, and managed:

```rust
/// Messages from agent to compositor
pub enum CompositorRequest {
    /// Create a new surface (window)
    CreateSurface {
        width: u32,
        height: u32,
        title: String,
        hints: SurfaceHints,
    },

    /// Attach a buffer to a surface (new frame)
    AttachBuffer {
        surface: SurfaceId,
        buffer: SharedBufferId,         // shared memory region
        damage: Vec<Rect>,             // regions that changed
    },

    /// Update surface hints (content changed)
    UpdateHints {
        surface: SurfaceId,
        hints: SurfaceHints,
    },

    /// Request resize
    Resize {
        surface: SurfaceId,
        width: u32,
        height: u32,
    },

    /// Destroy surface
    DestroySurface {
        surface: SurfaceId,
    },
}

/// Messages from compositor to agent
pub enum CompositorEvent {
    /// Surface configuration changed (resize, scale)
    Configure {
        surface: SurfaceId,
        width: u32,
        height: u32,
        scale: f32,
    },

    /// Surface gained/lost focus
    FocusChanged {
        surface: SurfaceId,
        focused: bool,
    },

    /// Surface should close (user clicked X)
    CloseRequested {
        surface: SurfaceId,
    },

    /// Input events routed to this surface
    Input(InputEvent),
}
```

### 3.1 Shared Buffer Protocol

Agents render into shared memory buffers. The compositor reads from these buffers without copying:

```rust
pub struct SharedBuffer {
    id: SharedBufferId,
    /// Shared memory region (kernel-managed)
    memory: SharedMemoryRegion,
    width: u32,
    height: u32,
    stride: u32,                        // bytes per row
    format: PixelFormat,
}

pub enum PixelFormat {
    Argb8888,                           // 32-bit with alpha
    Xrgb8888,                           // 32-bit without alpha
    Rgb565,                             // 16-bit (low memory)
}
```

**Double buffering:** Agents allocate two buffers and alternate between them. While the compositor reads from one, the agent renders to the other. Buffer swap is atomic (pointer swap via IPC).

-----

## 4. Semantic Hints

The key differentiator. Agents can tell the compositor what their content is, enabling intelligent behavior:

```rust
pub struct SurfaceHints {
    /// What kind of content this surface shows
    content_type: SurfaceContentType,

    /// How interactive this surface is right now
    interaction_state: InteractionState,

    /// Urgency of this surface's content
    urgency: SurfaceUrgency,

    /// Whether this surface should be treated as a panel/overlay
    layer: SurfaceLayer,

    /// Accessibility: what this surface represents
    semantic_role: SemanticRole,

    /// Preferred layout behavior
    layout_preference: LayoutPreference,
}

pub enum SurfaceContentType {
    Document,                           // text-heavy, benefits from width
    Terminal,                           // monospace text, fixed aspect ratio
    Media,                              // video/images, prefer aspect ratio preservation
    Conversation,                       // chat interface, benefits from height
    Browser,                            // web content, flexible
    Game,                               // fullscreen preferred, low latency
    Inspector,                          // diagnostic, sidebar-friendly
    Settings,                           // form-like, moderate size
    Notification,                       // small, temporary, overlay
}

pub enum InteractionState {
    Active,                             // user is interacting
    Passive,                            // showing content, no interaction
    Background,                         // not visible but running
    Urgent,                             // needs attention
}

pub enum SurfaceUrgency {
    None,
    Low,
    Medium,
    High,                               // visual indicator (subtle glow, badge)
}

pub enum SurfaceLayer {
    Background,                         // wallpaper, desktop
    Normal,                             // regular windows
    TopLevel,                           // always-on-top (rare)
    Overlay,                            // notifications, tooltips
    Panel,                              // taskbar, status bar
}

pub enum LayoutPreference {
    Flexible,                           // compositor decides
    PreferWidth(u32),                   // minimum width for readable content
    PreferHeight(u32),                  // minimum height
    FixedAspect(f32),                   // maintain aspect ratio
    Fullscreen,                         // prefers fullscreen
}
```

**How the compositor uses hints:**
- A `Game` surface with `Active` interaction → suppress notifications, disable idle timeout
- A `Document` surface → auto-tile with generous width
- A `Conversation` surface → sidebar layout, narrow width OK
- A `Notification` with `High` urgency → overlay on top with animation
- `Passive` surfaces → candidates for background, lower rendering priority

**Graceful degradation:** On Linux/macOS (portable toolkit), hints are ignored. The app works normally without semantic layout. On AIOS, hints enhance the experience.

-----

## 5. Layout Engine

### 5.1 Layout Modes

```rust
pub enum LayoutMode {
    /// User manually positions and sizes windows
    Floating,
    /// Windows auto-tile in the available space
    Tiling {
        split: SplitDirection,
        ratio: f32,
    },
    /// One window fullscreen, others hidden
    Fullscreen,
    /// Stacked tabs (like browser tabs but for windows)
    Stacked,
    /// Columns (like macOS full-screen split)
    Columns(Vec<f32>),                  // width ratios
}

pub enum SplitDirection {
    Horizontal,
    Vertical,
    Auto,                               // compositor decides based on content hints
}
```

### 5.2 Context-Aware Layout

The compositor adjusts layout based on Context Engine state:

```
Work context (high engagement):
  - Tiling layout preferred
  - Active document gets 60% width
  - Terminal gets 40% width
  - Conversation bar is prominent

Leisure context (low engagement):
  - Floating layout preferred
  - Media player centered, large
  - Browser floating over media
  - Conversation bar is subtle

Gaming context:
  - Active game fullscreen
  - No other windows visible
  - Notifications suppressed
  - Compositor minimizes overhead (direct scanout if possible)
```

-----

## 6. Render Pipeline

### 6.1 Frame Composition

```
Per-frame (target: 16.67ms for 60fps):

1. Collect damage regions from all surfaces
   - Agent A: damaged rect (100, 200, 400, 300)
   - Agent B: no damage (skip)
   - Agent C: full repaint

2. Calculate visible regions (occlusion culling)
   - Surface B fully occluded by A → skip entirely
   - Surface C partially visible → clip to visible region

3. Composite visible surfaces in z-order
   - Background layer first
   - Normal windows in z-order
   - Overlay/panel layers last
   - Alpha blending for transparent regions

4. Apply effects (if any)
   - Window shadows (pre-computed, cached)
   - Rounded corners (shader)
   - Blur for overlay backgrounds (if GPU supports)

5. Present to display
   - VSync swap
   - Direct scanout for single fullscreen surface (zero-copy)
```

### 6.2 Damage Tracking

Only changed regions are redrawn. The compositor maintains a damage list:

```rust
pub struct DamageTracker {
    /// Regions damaged since last frame
    current_frame: Vec<DamageRect>,
    /// Previous frame's damage (for double-buffer coordination)
    previous_frame: Vec<DamageRect>,
}

pub struct DamageRect {
    surface: SurfaceId,
    x: i32, y: i32,
    width: u32, height: u32,
}
```

A surface with no damage (common for static content like a document not being edited) contributes zero GPU work per frame.

### 6.3 Direct Scanout

When a single surface covers the entire display (fullscreen game, video), the compositor can bypass composition entirely. The surface's buffer is scanned out directly to the display hardware. This eliminates one GPU copy and reduces latency.

-----

## 7. Input Routing

The compositor owns input routing. Keyboard, mouse, touch, and gamepad events flow through it:

```rust
pub struct InputRouter {
    /// Currently focused surface
    focus: Option<SurfaceId>,
    /// Global hotkey bindings
    hotkeys: Vec<HotkeyBinding>,
    /// Gesture recognizer
    gestures: GestureRecognizer,
}

pub struct HotkeyBinding {
    keys: KeyCombo,
    action: HotkeyAction,
}

pub enum HotkeyAction {
    SwitchWindow,                       // Alt+Tab
    ToggleConversationBar,              // system gesture
    Screenshot,
    LockScreen,
    ToggleInspector,
    Custom(AgentId, String),            // agent-registered hotkey
}
```

**Routing rules:**
1. Global hotkeys checked first (Alt+Tab, screenshot, etc.)
2. System gestures checked (conversation bar toggle)
3. Remaining input routed to focused surface's agent
4. Mouse events routed based on pointer position (surface under cursor)

-----

## 8. Multi-Monitor Support

```rust
pub struct DisplayManager {
    outputs: Vec<Output>,
    layout: MonitorLayout,
}

pub struct Output {
    id: OutputId,
    name: String,                       // "HDMI-1", "DSI-1"
    resolution: (u32, u32),
    refresh_rate: f32,
    scale: f32,                         // HiDPI scaling (1.0, 1.5, 2.0)
    position: (i32, i32),               // position in virtual desktop
    transform: Transform,              // rotation
}

pub enum MonitorLayout {
    Mirror,                             // same content on all outputs
    Extended,                           // continuous desktop across outputs
    Independent,                        // separate workspaces per output
}
```

Each output has its own render pipeline. Surfaces can span outputs (the compositor handles splitting the render). HiDPI scaling is per-output — a laptop screen at 2x next to an external monitor at 1x works correctly.

-----

## 9. Accessibility

### 9.1 Accessibility Tree

The compositor maintains an accessibility tree built from surface hints and agent-provided accessibility information:

```rust
pub struct AccessibilityTree {
    root: AccessNode,
}

pub struct AccessNode {
    role: AccessRole,
    name: String,
    description: Option<String>,
    state: AccessState,
    children: Vec<AccessNode>,
    actions: Vec<AccessAction>,
    bounds: Rect,
}

pub enum AccessRole {
    Window, Button, TextInput, Label, List, ListItem, Menu, MenuItem,
    ScrollArea, Slider, Checkbox, RadioButton, Tab, TabPanel,
    Toolbar, StatusBar, Dialog, Alert, Image, Link, Table, Row, Cell,
}

pub enum AccessAction {
    Click, Focus, Expand, Collapse, ScrollTo, SetValue(String),
}
```

### 9.2 Screen Reader Integration

The compositor sends accessibility events to the screen reader service:

```rust
pub enum AccessibilityEvent {
    FocusChanged { node: AccessNodeId },
    ValueChanged { node: AccessNodeId, value: String },
    Alert { message: String },
    WindowCreated { surface: SurfaceId },
    WindowDestroyed { surface: SurfaceId },
}
```

The screen reader service converts these to speech output via the audio subsystem. All standard Web Accessibility Initiative (WAI-ARIA) roles are supported.

-----

## 10. Subsystem Framework Integration

The compositor is the Display subsystem:

```rust
impl Subsystem for DisplaySubsystem {
    const ID: SubsystemId = "display";
    type Capability = DisplayCapability;
    type Device = DisplayOutput;
    type Session = RenderSession;
    type AuditEvent = DisplayAuditEvent;

    fn open_session(
        &self,
        agent: AgentId,
        cap: &DisplayCapability,
        intent: &SessionIntent,
    ) -> Result<RenderSession> {
        gate_check(agent, Self::ID, cap, intent)?;

        let surface = self.compositor.create_surface(
            cap.max_width,
            cap.max_height,
            agent,
        )?;

        Ok(RenderSession {
            surface,
            agent,
            capability: cap.clone(),
            buffers: SharedBufferPool::new(cap.max_memory),
        })
    }
}

pub struct DisplayCapability {
    /// Maximum surface size
    max_width: u32,
    max_height: u32,
    /// Maximum GPU memory usage
    max_memory: usize,
    /// Can request fullscreen?
    fullscreen: bool,
    /// Can create overlay surfaces?
    overlay: bool,
    /// WebGL/WebGPU access? (for browser tabs)
    gpu_compute: bool,
}
```

**POSIX bridge:** The display subsystem exposes `/dev/fb0` (framebuffer) and DRM/KMS interfaces for legacy applications. The Wayland compatibility layer (Phase 25) will provide full Wayland protocol support for Linux GUI applications.

-----

## 11. Design Principles

1. **Semantic, not just spatial.** The compositor understands content types, not just rectangles.
2. **60 fps or drop features.** Frame rate is sacred. If effects can't maintain 60fps, they're disabled automatically.
3. **Zero-copy when possible.** Shared buffers, direct scanout, damage tracking — minimize GPU copies.
4. **Accessibility from day one.** The accessibility tree is built during Phase 6, not Phase 23. Screen reader support is an early design constraint.
5. **Input is mediated.** All input flows through the compositor. No agent can capture global input without capability.
6. **HiDPI is default.** Scaling is always active. 1x is just scale=1.0.

-----

## 12. Implementation Order

```
Phase 5a:  VirtIO-GPU driver + wgpu init           → GPU rendering works
Phase 5b:  Font rendering (fontdue or ab_glyph)     → text on screen
Phase 5c:  Basic surface composition                → multiple surfaces composited
Phase 6a:  Window manager (floating + tiling)        → windows are manageable
Phase 6b:  Compositor protocol (IPC-based)           → agents create/update surfaces
Phase 6c:  Input routing                             → keyboard/mouse to focused window
Phase 6d:  Damage tracking + VSync                   → 60fps, efficient rendering
Phase 6e:  Desktop shell (taskbar, launcher)          → boot to usable desktop
Phase 6f:  Semantic hints                            → content-aware layout
Phase 20:  Portable UI toolkit (iced) backend         → iced renders via compositor
Phase 23a: Accessibility tree + screen reader         → accessibility support
Phase 25:  Wayland compatibility layer                → Linux GUI apps
```
