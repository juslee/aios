# Interface Kit

**Layer:** Application | **Crate:** `aios_interface` | **Architecture:** `docs/applications/ui-toolkit.md`

## 1. Overview

Interface Kit is the AIOS-native UI toolkit. It provides the canonical way to build user interfaces
for AIOS agents and applications: views, controls, layout, theming, accessibility, and platform
abstraction. Interface Kit defines Rust traits that compose into a complete widget system following
the Elm Architecture (Model-View-Update), which maps naturally to the AIOS agent model where each
agent is an isolated process with its own state, message loop, and view function. Cross-platform
toolkits (Flutter, Qt, GTK) sit above Interface Kit as bridges, translating their widget and
rendering models into Kit primitives.

The Elm Architecture is not merely a UI pattern in AIOS -- it is the natural expression of the
capability-isolated agent model at the GUI level. Each agent owns its state (backed by its own
TTBR0 address space). State changes only happen through messages dispatched to the `update`
function. Side effects are explicit `InterfaceCommand` values returned from `update`, not
imperative calls buried in event handlers. The declarative `view` function returns a description
of what the UI should look like; the toolkit handles diffing, damage tracking, and rendering. This
design enforces the same isolation guarantees at the UI layer that the kernel enforces at the
process layer.

Interface Kit integrates deeply with other Kits in ways that no external toolkit can replicate.
Widgets declare the capabilities they require (see [Capability Kit](../kernel/capability.md)),
respond to attention and focus state from the [Attention Kit](../intelligence/attention.md),
accept Flow-native drag-and-drop payloads carrying full type information and provenance (see
[Flow Kit](../intelligence/flow.md)), and can bind their state reactively to Space queries from
the [Storage Kit](../platform/storage.md). When AIRS is available, layout can adapt intelligently
to user context; when it is not, every feature degrades gracefully to a fully functional baseline.
On non-AIOS platforms (Linux, macOS, Web), the same application code compiles and runs with
AIOS-specific features replaced by standard equivalents (clipboard for Flow, filesystem for
Spaces, all-granted for capabilities).

---

## 2. Core Traits

### 2.1 View -- Base Visual Element

Every visual element in Interface Kit implements `View`. A view owns a region of a compositor
surface and participates in layout, rendering, and the accessibility tree.

```rust
/// Base trait for all visual elements in Interface Kit.
///
/// A View represents a rectangular region that can be laid out, drawn,
/// and described to assistive technologies. Views are the leaves and
/// branches of the widget tree returned by `Widget::view()`.
pub trait View {
    /// Return the accessibility description of this view.
    /// Every view must be representable in the accessibility tree.
    fn accessibility(&self) -> AccessibilityNode;

    /// Return the set of capabilities this view requires to function.
    /// The runtime uses this to gate features and show appropriate
    /// fallback UI when capabilities are not granted.
    fn required_capabilities(&self) -> CapabilitySet {
        CapabilitySet::none()
    }

    /// Unique identifier for this view within its parent.
    /// Used for stable diffing across frames.
    fn id(&self) -> Option<ViewId> {
        None
    }
}
```

### 2.2 Control -- Interactive Elements

Controls are views that handle user interaction. Buttons, sliders, text inputs, and toggles
all implement `Control`.

```rust
/// Trait for interactive UI elements that respond to user input.
///
/// Controls extend View with event handling, focus participation,
/// and enabled/disabled state. Every Control is also a View.
pub trait Control: View {
    /// The message type this control emits when interacted with.
    type Message;

    /// Handle an input event, optionally producing a message.
    /// Returns `Handled` if the event was consumed, `Ignored` otherwise.
    fn on_event(
        &mut self,
        event: &InputEvent,
        layout: LayoutRect,
        cursor: CursorPosition,
    ) -> EventResult<Self::Message>;

    /// Whether this control currently accepts input.
    fn is_enabled(&self) -> bool {
        true
    }

    /// Whether this control participates in tab-order focus.
    fn is_focusable(&self) -> bool {
        self.is_enabled()
    }

    /// Called when the control gains or loses keyboard focus.
    fn on_focus_change(&mut self, focused: bool) {
        let _ = focused;
    }
}
```

### 2.3 Widget\<M\> -- Elm Architecture Core

The `Widget` trait is the heart of Interface Kit. It encodes the Elm Architecture:
application state (the struct), a message type `M`, an `update` function that advances
state, and a `view` function that produces the UI description.

```rust
/// The Elm Architecture widget trait.
///
/// Every AIOS application implements `Widget<M>` on its root state struct.
/// The runtime drives the update/view cycle:
///
/// 1. An event occurs (user input, IPC message, timer, subscription).
/// 2. The event is translated into a message of type `M`.
/// 3. `update(&mut self, msg)` is called, returning a command for side effects.
/// 4. `view(&self)` is called, returning the new widget tree.
/// 5. The toolkit diffs the tree, computes damage, and re-renders.
///
/// State changes ONLY happen in `update`. The view is a pure function of state.
pub trait Widget<M: Clone> {
    /// Initialize the widget, returning initial state and a startup command.
    fn new(flags: LaunchFlags) -> (Self, InterfaceCommand<M>)
    where
        Self: Sized;

    /// The window title (may change dynamically).
    fn title(&self) -> &str;

    /// Process a message, update state, and return side-effect commands.
    fn update(&mut self, message: M) -> InterfaceCommand<M>;

    /// Build the declarative view tree from current state.
    /// This must be a pure function of `&self` -- no mutation, no side effects.
    fn view(&self) -> Element<M>;

    /// Return subscriptions to external event sources (timers, Space watches,
    /// IPC channels, attention state changes).
    fn subscription(&self) -> Subscription<M> {
        Subscription::none()
    }

    /// Return the current theme. Called once per frame.
    fn theme(&self) -> Theme {
        Theme::system_default()
    }
}
```

**Why Elm Architecture fits AIOS:**

| Property | Elm Architecture | AIOS Agent Model |
| --- | --- | --- |
| Isolated state | Each app owns its state | Each agent owns its memory (TTBR0) |
| Message passing | Events produce messages for `update` | IPC messages are capability-checked |
| No shared mutation | State only changes in `update` | No shared memory between agents |
| Declarative view | `view()` returns a description | Compositor renders declarative surfaces |
| Explicit side effects | `InterfaceCommand` for async work | IPC for all external interaction |
| Subscriptions | Listen for external events | IPC channels, Space watches |

### 2.4 Layout -- Constraint-Based Positioning

The layout engine uses a constraint propagation model. Parents pass size constraints
down; children return their computed size up.

```rust
/// Constraint-based layout trait.
///
/// Layout is a single top-down, bottom-up pass:
/// 1. Root receives screen constraints (0,0 -> surface_width, surface_height).
/// 2. Root passes constraints to children (minus padding, spacing).
/// 3. Each child computes its intrinsic size within constraints.
/// 4. Parent arranges children (row: horizontal, column: vertical).
/// 5. Final positions are absolute surface coordinates.
pub trait Layout {
    /// Calculate intrinsic size given parent constraints.
    fn layout(&self, limits: &Limits) -> LayoutNode;

    /// Return child layout elements for recursive layout.
    fn children(&self) -> &[Element<()>] {
        &[]
    }
}

/// Size constraints passed from parent to child.
pub struct Limits {
    pub min: Size,
    pub max: Size,
}

/// A positioned layout result.
pub struct LayoutNode {
    pub size: Size,
    pub children: Vec<LayoutNode>,
}

/// Sizing strategy for a dimension.
pub enum Length {
    /// Fill all available space.
    Fill,
    /// Fill proportional to sibling FillPortion values.
    FillPortion(u16),
    /// Shrink to content size.
    Shrink,
    /// Fixed pixel size.
    Fixed(f32),
}
```

### 2.5 Theme -- Design Tokens

The theme system uses a token-based approach. Every visual property references a
semantic token, not a hardcoded value. On AIOS, themes adapt to the Context Engine's
inferred state (work, leisure, focus, gaming).

```rust
/// Design token system for Interface Kit.
///
/// Themes define the visual language: colors, typography, spacing, corner
/// radii, and animation curves. The system theme is the default; agents
/// may override specific tokens within accessibility bounds (e.g., accent
/// color must maintain WCAG AA 4.5:1 contrast ratio against background).
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

**Context-aware themes (AIOS only):**

```rust
/// Adapt the theme to the Context Engine's current state.
/// On non-AIOS platforms this function is never called.
pub fn theme_for_context(base: &Theme, context: &ContextState) -> Theme {
    let mut theme = base.clone();
    match context.mode {
        ContextMode::Work => {
            theme.spacing = Spacing::compact();
            theme.palette.background = Color::from_rgb(0.97, 0.97, 0.98);
        }
        ContextMode::Focus => {
            theme.spacing = Spacing::minimal();
            theme.palette.text_secondary = theme.palette.text_disabled;
        }
        ContextMode::Leisure => {
            theme.spacing = Spacing::relaxed();
            theme.palette.background = Color::from_rgb(0.98, 0.97, 0.95);
        }
        ContextMode::Gaming => {
            theme.palette = Palette::dark();
        }
    }
    // Night-shift warmth after 20:00
    if context.time_of_day.hour() >= 20 || context.time_of_day.hour() < 6 {
        theme.palette = theme.palette.warm_shift(0.05);
    }
    theme
}
```

### 2.6 AccessibilityNode

Every view in the widget tree generates an accessibility node. The accessibility tree
is structural -- generated from the widget hierarchy, not annotated after the fact.
On AIOS, it is exposed via IPC to the screen reader agent. See
[Accessibility architecture](../../experience/accessibility.md).

```rust
/// Semantic description of a view for assistive technologies.
pub struct AccessibilityNode {
    pub role: AccessRole,
    pub label: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
    pub bounds: Rectangle,
    pub focusable: bool,
    pub focused: bool,
    pub disabled: bool,
    pub children: Vec<AccessibilityNode>,
    pub actions: Vec<AccessAction>,
}

/// Semantic role of a UI element in the accessibility tree.
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
    ProgressBar,
    Generic,
}

/// Actions an assistive technology can invoke on a node.
pub enum AccessAction {
    Click,
    Focus,
    Scroll(ScrollDirection),
    SetValue(String),
    Expand,
    Collapse,
    Dismiss,
}
```

### 2.7 InterfaceBackend -- Platform Abstraction

The `InterfaceBackend` trait abstracts the platform-specific rendering, event polling,
and clipboard access. AIOS implements this via compositor IPC and GPU buffer sharing.
Non-AIOS platforms implement it via windowing libraries and GPU APIs.

```rust
/// Platform abstraction layer for Interface Kit.
///
/// Exactly one backend is active per application instance. The backend is
/// selected at startup based on the target platform. Application code never
/// calls backend methods directly -- the Widget runtime calls them.
pub trait InterfaceBackend {
    /// Allocate a compositor surface (AIOS) or OS window (other platforms).
    fn create_surface(&mut self, hints: SurfaceHints) -> SurfaceId;

    /// Destroy a surface and release its resources.
    fn destroy_surface(&mut self, surface: SurfaceId);

    /// Submit a display list for rendering to the given surface.
    fn submit(&mut self, surface: SurfaceId, display_list: &DisplayList);

    /// Poll for platform events (input, resize, focus, close).
    fn poll_events(&mut self) -> Vec<PlatformEvent>;

    /// Write content to the system clipboard (or Flow tray on AIOS).
    fn set_clipboard(&mut self, content: ClipboardContent);

    /// Read content from the system clipboard (or Flow tray on AIOS).
    fn get_clipboard(&self) -> Option<ClipboardContent>;

    /// Query what platform features are available.
    fn capabilities(&self) -> InterfaceCapabilities;

    /// Request a new animation frame for the given surface.
    fn request_frame(&mut self, surface: SurfaceId);

    /// Get the current display scale factor.
    fn scale_factor(&self) -> f32;
}
```

### 2.8 InterfaceCapabilities -- Feature Detection

Runtime feature detection replaces conditional compilation. Application code checks
capabilities to adapt behavior, never `#[cfg(target_os = "aios")]`.

```rust
/// Platform capability flags for runtime feature detection.
///
/// On AIOS, all fields are `true`. On other platforms, AIOS-specific
/// features are `false` and the application uses standard fallbacks.
pub struct InterfaceCapabilities {
    /// Compositor understands semantic window hints (content type, context behavior).
    pub semantic_hints: bool,
    /// Drag-and-drop routes through the Flow system with full type info.
    pub flow_integration: bool,
    /// Widget state can be persisted to and queried from Spaces.
    pub space_backed_data: bool,
    /// Widgets can query the agent's granted capability set.
    pub capability_aware_ui: bool,
    /// Attention Kit integration for focus/DND awareness.
    pub attention_aware: bool,
    /// Hardware-accelerated GPU rendering is available.
    pub gpu_rendering: bool,
    /// High-DPI / Retina display support.
    pub high_dpi: bool,
    /// Touch input is available.
    pub touch_input: bool,
    /// Intent verification for sensitive actions.
    pub intent_verification: bool,
}
```

### 2.9 InterfaceCommand\<M\> -- Side Effects

Commands represent side effects that the application wants to happen outside its own
state. They are returned from `update` and executed by the runtime. This keeps `update`
pure from the application's perspective.

```rust
/// Side-effect commands returned from Widget::update().
///
/// Commands are the ONLY way to cause effects outside the widget's own state.
/// The runtime executes them asynchronously and delivers results as messages.
pub enum InterfaceCommand<M> {
    /// No side effect.
    None,

    /// Execute an async operation and map the result to a message.
    Perform(Box<dyn Future<Output = M> + Send>),

    /// Batch multiple commands for parallel execution.
    Batch(Vec<InterfaceCommand<M>>),

    /// Write to the clipboard (or Flow tray on AIOS).
    Clipboard(ClipboardAction),

    /// AIOS-specific platform commands. No-op on other platforms.
    Platform(PlatformInterfaceCommand<M>),
}

/// AIOS-specific side-effect commands.
///
/// These compile on all platforms but only take effect on AIOS.
/// On non-AIOS platforms they are silently dropped or mapped to
/// standard equivalents (e.g., FlowPush becomes clipboard write).
pub enum PlatformInterfaceCommand<M> {
    /// Post an attention item (notification, badge, etc.).
    /// See [Attention Kit](../intelligence/attention.md).
    PostAttention(AttentionRequest),

    /// Push data to the Flow tray with full type and provenance info.
    /// Falls back to clipboard write on non-AIOS platforms.
    FlowPush(FlowData),

    /// Set semantic window hints for the compositor.
    /// Ignored on non-AIOS platforms.
    SetWindowHints(WindowHints),

    /// Query the agent's granted capability set.
    /// Returns all-granted on non-AIOS platforms.
    QueryCapabilities(Box<dyn Fn(CapabilitySet) -> M + Send>),

    /// Request intent verification for a sensitive action.
    /// See [Security Kit](security.md).
    VerifyIntent(IntentRequest, Box<dyn Fn(IntentResult) -> M + Send>),

    /// Subscribe to Space query results (reactive data binding).
    /// Falls back to filesystem watch on non-AIOS platforms.
    SpaceQuery(SpaceQueryRequest, Box<dyn Fn(QueryResult) -> M + Send>),
}
```

---

## 3. Usage Patterns

### 3.1 Minimal: Counter App

The simplest possible Interface Kit application -- a counter with increment and
decrement buttons. Five lines of view code.

```rust
use aios_interface::prelude::*;

struct Counter {
    value: i64,
}

#[derive(Debug, Clone)]
enum Msg {
    Increment,
    Decrement,
}

impl Widget<Msg> for Counter {
    fn new(_flags: LaunchFlags) -> (Self, InterfaceCommand<Msg>) {
        (Counter { value: 0 }, InterfaceCommand::None)
    }

    fn title(&self) -> &str {
        "Counter"
    }

    fn update(&mut self, message: Msg) -> InterfaceCommand<Msg> {
        match message {
            Msg::Increment => self.value += 1,
            Msg::Decrement => self.value -= 1,
        }
        InterfaceCommand::None
    }

    fn view(&self) -> Element<Msg> {
        column![
            button("+").on_press(Msg::Increment),
            text(self.value.to_string()).size(32.0),
            button("-").on_press(Msg::Decrement),
        ]
        .spacing(8.0)
        .align_items(Alignment::Center)
        .into()
    }
}
```

### 3.2 Realistic: Notes App with Space-Backed State

A notes application that persists data to a Space. On AIOS, notes are stored in the
agent's Space and sync across devices. On other platforms, notes are saved to the local
filesystem. The application code is identical.

```rust
use aios_interface::prelude::*;

struct NotesApp {
    notes: Vec<Note>,
    search_query: String,
    selected: Option<usize>,
    capabilities: InterfaceCapabilities,
}

#[derive(Debug, Clone)]
enum Msg {
    SearchChanged(String),
    NoteSelected(usize),
    NoteCreated,
    NoteDeleted(usize),
    ContentEdited(String),
    DataLoaded(Vec<Note>),
    DataSaved,
}

impl Widget<Msg> for NotesApp {
    fn new(flags: LaunchFlags) -> (Self, InterfaceCommand<Msg>) {
        let caps = flags.capabilities();
        let app = NotesApp {
            notes: Vec::new(),
            search_query: String::new(),
            selected: None,
            capabilities: caps,
        };

        // Load from Space on AIOS, filesystem elsewhere
        let cmd = if caps.space_backed_data {
            InterfaceCommand::Platform(PlatformInterfaceCommand::SpaceQuery(
                SpaceQueryRequest::new("notes/*").order_by(QueryOrder::Modified),
                Box::new(|result| Msg::DataLoaded(result.into_items())),
            ))
        } else {
            InterfaceCommand::Perform(Box::pin(async {
                let notes = load_notes_from_filesystem().await;
                Msg::DataLoaded(notes)
            }))
        };
        (app, cmd)
    }

    fn title(&self) -> &str {
        "Notes"
    }

    fn update(&mut self, message: Msg) -> InterfaceCommand<Msg> {
        match message {
            Msg::SearchChanged(query) => {
                self.search_query = query;
                InterfaceCommand::None
            }
            Msg::NoteSelected(idx) => {
                self.selected = Some(idx);
                InterfaceCommand::None
            }
            Msg::NoteCreated => {
                let note = Note::new();
                self.notes.push(note.clone());
                self.selected = Some(self.notes.len() - 1);
                InterfaceCommand::Platform(PlatformInterfaceCommand::SpaceQuery(
                    SpaceQueryRequest::write(&format!("notes/{}", note.id), &note),
                    Box::new(|_| Msg::DataSaved),
                ))
            }
            Msg::NoteDeleted(idx) => {
                let note = self.notes.remove(idx);
                self.selected = None;
                InterfaceCommand::Platform(PlatformInterfaceCommand::SpaceQuery(
                    SpaceQueryRequest::delete(&format!("notes/{}", note.id)),
                    Box::new(|_| Msg::DataSaved),
                ))
            }
            Msg::ContentEdited(content) => {
                if let Some(idx) = self.selected {
                    self.notes[idx].content = content;
                }
                InterfaceCommand::None
            }
            Msg::DataLoaded(notes) => {
                self.notes = notes;
                InterfaceCommand::None
            }
            Msg::DataSaved => InterfaceCommand::None,
        }
    }

    fn view(&self) -> Element<Msg> {
        let sidebar = column![
            text_input("Search...", &self.search_query)
                .on_input(Msg::SearchChanged),
            button("New Note").on_press(Msg::NoteCreated),
            scrollable(self.note_list()),
        ]
        .width(Length::FillPortion(1))
        .spacing(8.0);

        let editor = match self.selected {
            Some(idx) => text_editor(&self.notes[idx].content)
                .on_edit(Msg::ContentEdited)
                .into(),
            None => text("Select a note").size(16.0).into(),
        };

        row![
            sidebar,
            vertical_rule(1.0),
            container(editor).width(Length::FillPortion(3)).padding(16.0),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn subscription(&self) -> Subscription<Msg> {
        // On AIOS: watch Space for external changes (other agents, sync).
        // On other platforms: watch filesystem for changes.
        Subscription::space_watch("notes/*")
            .map(|_| Msg::DataLoaded(Vec::new()))
    }
}
```

### 3.3 Advanced: Multi-Pane App with Flow Drag-and-Drop

A file manager with multi-pane layout and Flow-native drag-and-drop. Dragging a file
between panes (or from another agent) carries full type information and provenance
through the Flow system.

```rust
use aios_interface::prelude::*;

struct FileManager {
    left_pane: PaneState,
    right_pane: PaneState,
    drag_state: Option<DragState>,
    capabilities: InterfaceCapabilities,
}

#[derive(Debug, Clone)]
enum Msg {
    LeftPaneNav(PathBuf),
    RightPaneNav(PathBuf),
    DragStarted(FlowEntry, PaneId),
    DragOver(PaneId, Point),
    DragDropped(PaneId, Point),
    DragCancelled,
    FilesMoved(Result<(), StorageError>),
    FlowReceived(FlowEntry),
}

impl Widget<Msg> for FileManager {
    fn new(flags: LaunchFlags) -> (Self, InterfaceCommand<Msg>) {
        let app = FileManager {
            left_pane: PaneState::new("/user/home/"),
            right_pane: PaneState::new("/user/home/documents/"),
            drag_state: None,
            capabilities: flags.capabilities(),
        };
        (app, InterfaceCommand::None)
    }

    fn title(&self) -> &str {
        "Files"
    }

    fn update(&mut self, message: Msg) -> InterfaceCommand<Msg> {
        match message {
            Msg::DragStarted(entry, source_pane) => {
                self.drag_state = Some(DragState { entry, source_pane });
                // On AIOS: register with Flow for cross-agent drag.
                // On other platforms: use OS drag-and-drop.
                InterfaceCommand::Platform(PlatformInterfaceCommand::FlowPush(
                    FlowData {
                        content: TypedContent::from(&self.drag_state.as_ref().unwrap().entry),
                        content_type: ContentType::File,
                        provenance: Provenance::current_agent(),
                        transforms: vec![
                            FlowTransform::WithProvenance,
                        ],
                    },
                ))
            }
            Msg::DragDropped(target_pane, _position) => {
                if let Some(drag) = self.drag_state.take() {
                    let target_path = match target_pane {
                        PaneId::Left => &self.left_pane.path,
                        PaneId::Right => &self.right_pane.path,
                    };
                    InterfaceCommand::Perform(Box::pin(async move {
                        let result = move_file(&drag.entry, target_path).await;
                        Msg::FilesMoved(result)
                    }))
                } else {
                    InterfaceCommand::None
                }
            }
            Msg::FlowReceived(entry) => {
                // Received a FlowEntry from another agent via drag-and-drop.
                // The entry carries provenance and type information.
                self.right_pane.add_incoming(entry);
                InterfaceCommand::None
            }
            _ => InterfaceCommand::None,
        }
    }

    fn view(&self) -> Element<Msg> {
        let left = pane(
            &self.left_pane,
            PaneId::Left,
            |entry| Msg::DragStarted(entry, PaneId::Left),
            |pos| Msg::DragOver(PaneId::Left, pos),
            |pos| Msg::DragDropped(PaneId::Left, pos),
        );

        let right = pane(
            &self.right_pane,
            PaneId::Right,
            |entry| Msg::DragStarted(entry, PaneId::Right),
            |pos| Msg::DragOver(PaneId::Right, pos),
            |pos| Msg::DragDropped(PaneId::Right, pos),
        );

        // Flow-aware drop zone wraps each pane
        row![
            flow_drop_zone(left).on_drop(Msg::FlowReceived),
            vertical_rule(1.0),
            flow_drop_zone(right).on_drop(Msg::FlowReceived),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}
```

### 3.4 Common Mistakes

> **Mutation in `view()`** -- The `view` function receives `&self`, not `&mut self`.
> Do not attempt to cache layout results or mutate state inside `view`. If you need
> expensive computation, do it in `update` and store the result in your state struct.

---

> **Blocking in `update()`** -- Never perform I/O, sleep, or long computation inside
> `update`. Return an `InterfaceCommand::Perform` with an async future instead. The
> `update` function must complete in under 1ms to maintain 60fps.

---

> **Conditional compilation for platform features** -- Do not use
> `#[cfg(target_os = "aios")]`. Use runtime feature detection via
> `InterfaceCapabilities` so the same binary works everywhere.

---

> **Unbounded lists** -- Do not render thousands of items in a `column![]`. Use
> `virtual_list()` for scrollable content over 100 items to avoid layout blowup.

---

> **Ignoring accessibility** -- Every custom widget must return a meaningful
> `AccessibilityNode` from its `View::accessibility()` implementation. "Generic" role
> with no label is not acceptable for interactive controls.

---

## 4. Integration Examples

### 4.1 Interface Kit + Storage Kit (Reactive Queries)

Inspired by BeOS's live queries, Interface Kit can bind widget state to Space queries.
When objects in a Space change, the UI updates automatically without polling.

```rust
use aios_interface::prelude::*;

struct PhotoGallery {
    photos: Vec<Photo>,
}

#[derive(Debug, Clone)]
enum Msg {
    PhotosUpdated(Vec<Photo>),
    PhotoSelected(ObjectId),
}

impl Widget<Msg> for PhotoGallery {
    fn new(_flags: LaunchFlags) -> (Self, InterfaceCommand<Msg>) {
        (PhotoGallery { photos: Vec::new() }, InterfaceCommand::None)
    }

    fn title(&self) -> &str {
        "Photos"
    }

    fn update(&mut self, message: Msg) -> InterfaceCommand<Msg> {
        match message {
            Msg::PhotosUpdated(photos) => {
                self.photos = photos;
                InterfaceCommand::None
            }
            Msg::PhotoSelected(_id) => InterfaceCommand::None,
        }
    }

    fn view(&self) -> Element<Msg> {
        let grid = self.photos.iter().map(|photo| {
            image(&photo.thumbnail)
                .width(Length::Fixed(120.0))
                .on_click(Msg::PhotoSelected(photo.id))
        });
        wrap_grid(grid).spacing(8.0).into()
    }

    fn subscription(&self) -> Subscription<Msg> {
        // Reactive query: the UI updates whenever photos change in the Space.
        // This mirrors BeOS live queries -- the file manager updates in real time
        // when files are added, renamed, or deleted.
        Subscription::space_query(
            SpaceQueryRequest::new("photos/*")
                .content_type(ContentType::Image)
                .order_by(QueryOrder::Created),
        )
        .map(|results| Msg::PhotosUpdated(results.into_items()))
    }
}
```

See [Storage Kit](../platform/storage.md) for `SpaceQueryRequest` details.

### 4.2 Interface Kit + Attention Kit (Focus/DND Aware)

Widgets can respond to the system's attention state. During Do Not Disturb mode,
non-essential UI elements dim. During Focus mode, only the relevant application
receives full visual prominence.

```rust
use aios_interface::prelude::*;

struct MailClient {
    inbox: Vec<Email>,
    attention_state: AttentionState,
}

#[derive(Debug, Clone)]
enum Msg {
    InboxUpdated(Vec<Email>),
    AttentionChanged(AttentionState),
    EmailSelected(usize),
}

impl Widget<Msg> for MailClient {
    // ... new, title, update omitted for brevity ...

    fn view(&self) -> Element<Msg> {
        let opacity = match self.attention_state.focus_level {
            FocusLevel::Foreground => 1.0,
            FocusLevel::Background => 0.6,
            FocusLevel::DoNotDisturb => 0.3,
        };

        let badge = if self.attention_state.is_dnd() {
            // During DND, show count but suppress details
            text(format!("{} unread", self.unread_count()))
                .color(self.theme().palette.text_secondary)
        } else {
            // Normal mode: show full preview
            text(self.latest_subject())
                .color(self.theme().palette.text)
        };

        container(
            column![
                badge,
                self.email_list().opacity(opacity),
            ]
        )
        .into()
    }

    fn subscription(&self) -> Subscription<Msg> {
        Subscription::batch(vec![
            Subscription::space_watch("mail/inbox/*").map(|_| Msg::InboxUpdated(Vec::new())),
            // Subscribe to Attention Kit state changes
            Subscription::attention_state().map(Msg::AttentionChanged),
        ])
    }
}
```

See [Attention Kit](../intelligence/attention.md) for `AttentionState` and
`FocusLevel` details.

### 4.3 Interface Kit + Flow Kit (Drag/Drop Between Views)

Flow-native drag-and-drop preserves type information and provenance when data
moves between agents or views. Unlike clipboard-based drag-and-drop, the
destination knows the data's original type, source agent, and can apply
context-appropriate transformations.

```rust
use aios_interface::prelude::*;

/// A widget that accepts FlowEntry drops and displays them.
fn document_canvas<'a, M: Clone + 'a>(
    items: &'a [CanvasItem],
    on_flow_drop: impl Fn(FlowEntry, Point) -> M + 'a,
) -> Element<'a, M> {
    flow_drop_zone(
        canvas(items)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .accepted_types(&[
        ContentType::Image,
        ContentType::Text,
        ContentType::Document,
    ])
    .on_drop(move |entry, position| on_flow_drop(entry, position))
    .drop_indicator(DropIndicator::InsertionPoint)
    .into()
}
```

The `FlowEntry` carries:

- `TypedContent` -- the actual data with MIME type
- `Provenance` -- which agent produced it and when
- `FlowTransform` suggestions -- how to adapt the data for the destination

See [Flow Kit](../intelligence/flow.md) for the full `FlowEntry` data model.

### 4.4 Interface Kit + Conversation Kit (Chat UI with Streaming)

Building a chat interface that streams tokens from AIRS. The streaming subscription
delivers tokens incrementally, and the view updates as each token arrives.

```rust
use aios_interface::prelude::*;

struct ChatView {
    messages: Vec<ChatMessage>,
    input_text: String,
    streaming_response: Option<String>,
}

#[derive(Debug, Clone)]
enum Msg {
    InputChanged(String),
    SendMessage,
    TokenReceived(String),
    StreamComplete,
    StreamError(String),
}

impl Widget<Msg> for ChatView {
    // ... new omitted ...

    fn title(&self) -> &str {
        "Chat"
    }

    fn update(&mut self, message: Msg) -> InterfaceCommand<Msg> {
        match message {
            Msg::InputChanged(text) => {
                self.input_text = text;
                InterfaceCommand::None
            }
            Msg::SendMessage => {
                let user_msg = ChatMessage::user(self.input_text.clone());
                self.messages.push(user_msg);
                self.input_text.clear();
                self.streaming_response = Some(String::new());
                // Start inference via Conversation Kit
                InterfaceCommand::Perform(Box::pin(async move {
                    // The subscription handles streaming; this just initiates
                    Msg::TokenReceived(String::new())
                }))
            }
            Msg::TokenReceived(token) => {
                if let Some(ref mut response) = self.streaming_response {
                    response.push_str(&token);
                }
                InterfaceCommand::None
            }
            Msg::StreamComplete => {
                if let Some(response) = self.streaming_response.take() {
                    self.messages.push(ChatMessage::assistant(response));
                }
                InterfaceCommand::None
            }
            Msg::StreamError(err) => {
                self.streaming_response = None;
                self.messages.push(ChatMessage::system(format!("Error: {}", err)));
                InterfaceCommand::None
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        let messages = scrollable(
            column(
                self.messages.iter()
                    .map(|msg| chat_bubble(msg).into())
                    .collect()
            )
            .spacing(8.0)
        )
        .height(Length::Fill);

        let streaming = match &self.streaming_response {
            Some(partial) => column![
                chat_bubble_partial(partial),
                spinner().size(16.0),
            ].into(),
            None => column![].into(),
        };

        let input_bar = row![
            text_input("Type a message...", &self.input_text)
                .on_input(Msg::InputChanged)
                .on_submit(Msg::SendMessage)
                .width(Length::Fill),
            button("Send")
                .on_press(Msg::SendMessage)
                .style(ButtonStyle::Primary),
        ]
        .spacing(8.0);

        column![messages, streaming, input_bar]
            .spacing(8.0)
            .padding(16.0)
            .into()
    }
}
```

See [Conversation Kit](conversation.md) for streaming token
delivery and session management.

---

## 5. Capability Requirements

Interface Kit methods interact with system resources that are governed by the
capability system. Each operation requires specific capabilities; the runtime checks
these before executing. See [Capability Kit](../kernel/capability.md) for the
enforcement model.

| Method / Operation | Required Capability | Default Grant | Notes |
| --- | --- | --- | --- |
| `InterfaceBackend::create_surface` | `Surface::Create` | Granted to all UI agents | Allocates compositor surface |
| `InterfaceBackend::destroy_surface` | `Surface::Create` | Granted to all UI agents | Release compositor surface |
| `InterfaceBackend::submit` | `Surface::Render` | Granted to all UI agents | Submit display list to GPU |
| `InterfaceBackend::set_clipboard` | `Clipboard::Write` | Granted to all UI agents | Write to clipboard / Flow tray |
| `InterfaceBackend::get_clipboard` | `Clipboard::Read` | Granted to all UI agents | Read from clipboard / Flow tray |
| `PlatformInterfaceCommand::FlowPush` | `Flow::Write` | Granted to all UI agents | Push to Flow tray |
| `PlatformInterfaceCommand::PostAttention` | `Attention::Post` | Granted to all UI agents | Post notification / badge |
| `PlatformInterfaceCommand::SetWindowHints` | `Surface::Hints` | Granted to all UI agents | Set semantic compositor hints |
| `PlatformInterfaceCommand::QueryCapabilities` | None | Always allowed | Read-only introspection |
| `PlatformInterfaceCommand::VerifyIntent` | `Intent::Request` | Granted to trusted agents | Triggers Security Kit verification |
| `PlatformInterfaceCommand::SpaceQuery` | `Space::Read` or `Space::Write` | Per-agent manifest | Depends on read vs. write query |
| `Subscription::space_watch` | `Space::Read` | Per-agent manifest | Watch for Space changes |
| `Subscription::attention_state` | `Attention::Observe` | Granted to all UI agents | Observe focus / DND state |
| `flow_drop_zone` (receive) | `Flow::Read` | Granted to all UI agents | Accept incoming Flow drops |
| Secure input field | `Input::Secure` | Granted to credential agents | Password fields, PIN entry |

**Capability-visible widgets:** Widgets can declare the capabilities they need via
`View::required_capabilities()`. If the running agent lacks a required capability, the
widget renders in a disabled state with an explanatory tooltip. The application does not
crash -- it gracefully shows what is unavailable and why.

```rust
fn view(&self) -> Element<Msg> {
    let mut controls = column![];

    controls = controls.push(button("Save").on_press(Msg::Save));

    // capability_gate renders the child normally if the capability is granted,
    // or renders a disabled version with tooltip if not.
    controls = controls.push(
        capability_gate(
            Capability::Network,
            button("Sync").on_press(Msg::Sync),
            button("Sync")
                .style(ButtonStyle::Disabled)
                .tooltip("Network access not granted for this agent"),
        )
    );

    controls.into()
}
```

---

## 6. Error Handling & Degradation

### 6.1 InterfaceError

```rust
/// Errors that Interface Kit operations can produce.
pub enum InterfaceError {
    /// The compositor is unavailable (headless system, compositor crash).
    CompositorUnavailable,

    /// Surface allocation failed (out of GPU memory, too many surfaces).
    SurfaceAllocationFailed { reason: String },

    /// GPU rendering is not available; software fallback will be used.
    GpuUnavailable,

    /// A required capability was not granted to the agent.
    CapabilityDenied { capability: Capability, operation: String },

    /// Space query failed (storage service unavailable).
    SpaceQueryFailed { path: String, reason: String },

    /// Flow operation failed (Flow service unavailable).
    FlowError { reason: String },

    /// Attention Kit is unavailable (no AIRS, degraded mode).
    AttentionUnavailable,

    /// Intent verification was denied by the user or security policy.
    IntentDenied { action: String },

    /// Font not found in the system font registry.
    FontNotFound { family: String },

    /// Display list exceeds the GPU command buffer limit.
    DisplayListTooLarge { primitives: usize, limit: usize },

    /// Timeout waiting for compositor frame acknowledgment.
    FrameTimeout { surface: SurfaceId },
}
```

### 6.2 Fallback Cascade

Interface Kit degrades gracefully when subsystems are unavailable. The application
does not need to handle these cases explicitly -- the runtime applies fallbacks
automatically and reports degradation through `InterfaceCapabilities`.

| Condition | Affected Feature | Fallback Behavior |
| --- | --- | --- |
| No GPU | Hardware rendering | Software rasterizer (CPU rendering, reduced frame rate) |
| No compositor | Surface allocation | Direct framebuffer write (single fullscreen surface) |
| No AIRS | Context-aware theme | Static system theme (no context adaptation) |
| No AIRS | Attention-aware UI | All widgets render at full opacity; DND ignored |
| No AIRS | Smart layout | Standard constraint-based layout (no AI adaptation) |
| No Flow service | Drag-and-drop | OS clipboard-based drag-and-drop |
| No Flow service | `FlowPush` command | Standard clipboard write |
| No Space service | `SpaceQuery` command | Local filesystem read/write |
| No Space service | `space_watch` subscription | Filesystem notification (inotify/kqueue) |
| No network | Sync-dependent views | Stale data with "offline" indicator |
| Capability denied | Gated widget | Widget renders disabled with tooltip |
| Font missing | Text rendering | System default font via fallback chain |
| High memory pressure | Texture atlas | Evict LRU glyphs/images, reduce atlas size |
| Thermal throttling | Frame budget | Reduce target to 30fps, skip non-essential animations |

### 6.3 Error Delivery

Errors are delivered as messages through the Elm Architecture. The runtime wraps
fallible operations in result types and maps failures to application messages.
Applications that ignore errors get reasonable default behavior; applications that
handle them can present specific UI.

```rust
#[derive(Debug, Clone)]
enum Msg {
    // Normal messages
    DataLoaded(Vec<Item>),
    // Error messages -- delivered when operations fail
    LoadFailed(InterfaceError),
}

fn update(&mut self, message: Msg) -> InterfaceCommand<Msg> {
    match message {
        Msg::LoadFailed(InterfaceError::SpaceQueryFailed { path, reason }) => {
            self.error_banner = Some(format!("Could not load {}: {}", path, reason));
            InterfaceCommand::None
        }
        Msg::LoadFailed(InterfaceError::CapabilityDenied { capability, .. }) => {
            self.error_banner = Some(format!(
                "This action requires the {:?} capability", capability
            ));
            InterfaceCommand::None
        }
        _ => InterfaceCommand::None,
    }
}
```

---

## 7. Platform & AI Availability

### 7.1 Feature Detection Patterns

Application code uses runtime feature detection to adapt behavior. This is the
recommended pattern for all AIOS-specific features.

```rust
fn setup_persistence(&self) -> InterfaceCommand<Msg> {
    let caps = self.capabilities;

    if caps.space_backed_data {
        // AIOS: load from Space with reactive updates
        InterfaceCommand::Platform(PlatformInterfaceCommand::SpaceQuery(
            SpaceQueryRequest::new("settings/*"),
            Box::new(|result| Msg::SettingsLoaded(result.into_items())),
        ))
    } else {
        // Other platforms: load from filesystem
        InterfaceCommand::Perform(Box::pin(async {
            let settings = load_settings_from_file().await;
            Msg::SettingsLoaded(settings)
        }))
    }
}

fn handle_share(&self, data: ShareData) -> InterfaceCommand<Msg> {
    if self.capabilities.flow_integration {
        // AIOS: push to Flow with provenance
        InterfaceCommand::Platform(PlatformInterfaceCommand::FlowPush(
            FlowData {
                content: TypedContent::from(&data),
                content_type: data.content_type(),
                provenance: Provenance::current_agent(),
                transforms: vec![FlowTransform::WithProvenance],
            },
        ))
    } else {
        // Other platforms: clipboard
        InterfaceCommand::Clipboard(ClipboardAction::Write(data.to_string()))
    }
}
```

### 7.2 Headless Systems

On headless systems (servers, CI, embedded devices without displays), Interface Kit
operates in test mode. The backend is a null backend that records submitted display
lists without rendering them. This enables:

- **Unit testing** of widget logic without a GPU or display.
- **Snapshot testing** of view output for regression detection.
- **Server-side rendering** for generating UI previews.

```rust
/// Null backend for headless testing.
pub struct NullBackend {
    submitted: Vec<(SurfaceId, DisplayList)>,
}

impl InterfaceBackend for NullBackend {
    fn create_surface(&mut self, _hints: SurfaceHints) -> SurfaceId {
        SurfaceId(self.submitted.len() as u64)
    }

    fn submit(&mut self, surface: SurfaceId, display_list: &DisplayList) {
        self.submitted.push((surface, display_list.clone()));
    }

    fn capabilities(&self) -> InterfaceCapabilities {
        InterfaceCapabilities {
            semantic_hints: false,
            flow_integration: false,
            space_backed_data: false,
            capability_aware_ui: false,
            attention_aware: false,
            gpu_rendering: false,
            high_dpi: false,
            touch_input: false,
            intent_verification: false,
        }
    }

    // ... remaining methods return defaults ...
}
```

### 7.3 AIRS-Enhanced Features

When AIRS (AI Runtime Service) is available, Interface Kit gains intelligent
behaviors that are impossible with static rules. All AIRS features degrade to
deterministic baselines when AIRS is unavailable.

| AIRS Feature | What It Does | Fallback Without AIRS |
| --- | --- | --- |
| Smart layout | Predicts optimal pane sizes based on content and user history | Fixed ratios from `responsive()` |
| Predictive scrolling | Pre-renders offscreen content the user is likely to scroll to | Standard on-demand rendering |
| Context-aware theme | Adapts colors, density, and typography to inferred activity | Static system theme |
| Intelligent text completion | Suggests completions in text input fields | No suggestions |
| Anomaly detection | Detects unusual UI interaction patterns (possible automation attack) | Standard input validation only |
| Attention routing | Prioritizes which notifications to show based on user context | FIFO notification queue |
| Adaptive animation | Adjusts animation duration and easing based on user motion sensitivity | Default 200ms ease-in-out |
| Smart grid layout | Learns preferred arrangements for grid/gallery views | Uniform grid |

**AIRS integration point:** Interface Kit does not call AIRS directly. Instead, it
subscribes to intelligence services (Context Engine, Attention Manager, Preference
Service) through the standard subscription mechanism. These services may use AIRS
internally, but Interface Kit treats them as opaque event sources.

```rust
fn subscription(&self) -> Subscription<Msg> {
    Subscription::batch(vec![
        // Context Engine delivers theme adaptation hints
        Subscription::context_state().map(Msg::ContextChanged),
        // Attention Manager delivers focus/DND state
        Subscription::attention_state().map(Msg::AttentionChanged),
        // Preference Service delivers user preference changes
        Subscription::preferences("ui.*").map(Msg::PreferenceChanged),
    ])
}
```

### 7.4 Widget Taxonomy

Interface Kit ships a complete widget set organized into five categories. Every widget
implements the `View` trait and optionally the `Control` trait for interactive elements.

**Layout widgets** -- structure and spatial arrangement:

| Widget | Description |
| --- | --- |
| `row![]` | Horizontal flex container |
| `column![]` | Vertical flex container |
| `container()` | Single-child wrapper with padding, alignment, sizing |
| `scrollable()` | Scrollable viewport (vertical, horizontal, or both) |
| `stack![]` | Z-axis layering (overlapping children) |
| `responsive()` | Layout that adapts to available size |
| `space()` | Flexible spacer for distributing space |
| `pane_grid()` | Resizable multi-pane layout |
| `wrap_grid()` | Wrapping grid layout for gallery views |

**Content widgets** -- display information:

| Widget | Description |
| --- | --- |
| `text()` | Static text display with font, size, color |
| `image()` | Image display (PNG, JPEG, SVG, WebP) |
| `canvas()` | Freeform 2D drawing surface |
| `markdown()` | Markdown renderer with inline formatting |
| `rich_text()` | Styled text with multiple spans |
| `virtual_list()` | Virtualized scrollable list for large data sets |

**Input widgets** -- accept user interaction:

| Widget | Description |
| --- | --- |
| `button()` | Clickable button with label or child widget |
| `text_input()` | Single-line text input with placeholder |
| `text_editor()` | Multi-line text editor with syntax highlighting |
| `checkbox()` | Boolean toggle with label |
| `radio()` | Single-select from a group |
| `toggler()` | On/off switch |
| `slider()` | Numeric range slider |
| `pick_list()` | Dropdown selection |
| `combo_box()` | Searchable dropdown |
| `secure_input()` | Password/PIN field (requires `Input::Secure` capability) |

**Feedback widgets** -- communicate status:

| Widget | Description |
| --- | --- |
| `tooltip()` | Hover tooltip on any widget |
| `progress_bar()` | Determinate progress indicator |
| `spinner()` | Indeterminate loading indicator |
| `notification()` | Toast-style transient message |
| `chat_bubble()` | Message bubble for conversation UI |

**Overlay widgets** -- float above other content:

| Widget | Description |
| --- | --- |
| `modal()` | Modal dialog overlay |
| `menu()` | Context menu / dropdown menu |
| `flow_drop_zone()` | Flow-aware drag-and-drop target |
| `capability_gate()` | Conditionally renders based on granted capabilities |

### 7.5 Keyboard Navigation

All widgets support keyboard navigation by default. Focus management follows the
W3C ARIA design patterns.

- **Tab** / **Shift+Tab** moves focus between focusable widgets in document order.
- **Enter** / **Space** activates the focused widget.
- **Arrow keys** navigate within composite widgets (lists, menus, radio groups).
- **Escape** dismisses overlays and modals.
- Focus indicators are always visible. They are never removed for aesthetic reasons.
- The compositor draws a system-level focus ring so agents cannot fake or suppress it.

---

## Cross-References

- **[App Kit](app.md)** -- Application lifecycle (launch, quit, suspend, resume) wrapping
  Interface Kit's widget runtime.
- **[Storage Kit](../platform/storage.md)** -- Space-backed persistence and reactive queries
  that bind to widget state.
- **[Flow Kit](../intelligence/flow.md)** -- Flow-native drag-and-drop, clipboard, and
  content transfer between agents.
- **[Attention Kit](../intelligence/attention.md)** -- Focus management, Do Not Disturb
  state, and notification routing that widgets respond to.
- **[Capability Kit](../kernel/capability.md)** -- Capability enforcement that gates
  what widgets can do and display.
- **[Conversation Kit](conversation.md)** -- Streaming token delivery
  for chat and conversational UIs.
- **[Security Kit](security.md)** -- Intent verification for sensitive actions
  triggered from UI controls.
- **[Accessibility architecture](../../experience/accessibility.md)** -- Screen reader
  integration, accessibility tree exposure, WCAG compliance.
- **[Compositor architecture](../../platform/compositor.md)** -- Surface protocol, GPU
  buffer sharing, semantic hints, damage tracking.
- **[Context Engine](../../intelligence/context-engine.md)** -- Context-aware theme
  adaptation signals.

## Implementation Phase

Phase 6+ (basic compositor surfaces and core widgets). Full widget set and theme system
Phase 12+. Flow integration and Space-backed persistence Phase 15+. Accessibility tree
Phase 22+. Agent SDK packaging Phase 29+.
