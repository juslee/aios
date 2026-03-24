# AIOS Interface Kit — Widget Library

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [layout.md](./layout.md) — Layout engine, [theme.md](./theme.md) — Theme system, [accessibility.md](./accessibility.md) — Accessibility tree

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
