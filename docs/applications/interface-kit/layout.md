# AIOS Interface Kit — Layout Engine

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [widgets.md](./widgets.md) — Widget library, [rendering.md](./rendering.md) — Render pipeline, [performance.md](./performance.md) — Frame budget

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

```text
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

### 5.4 Incremental Layout

Rebuilding the entire layout tree every frame is wasteful. Interface Kit uses dirty flag propagation and constraint caching, inspired by Chrome's LayoutNG architecture:

**Dirty flags:**

```rust
bitflags! {
    pub struct LayoutDirty: u8 {
        /// This node's own layout is invalid.
        const SELF_DIRTY   = 0b001;
        /// At least one descendant's layout is invalid.
        const CHILD_DIRTY  = 0b010;
        /// This node needs a repaint but not a relayout.
        const PAINT_DIRTY  = 0b100;
    }
}
```

When a widget's content changes (text edit, list item added), it sets `SELF_DIRTY` on itself and `CHILD_DIRTY` on all ancestors up to the root. The layout pass skips any subtree where no dirty flags are set.

**Constraint caching:**

Layout is a pure function: `layout(widget, constraints) -> LayoutNode`. If the constraints and widget content are unchanged since the last frame, the cached `LayoutNode` is reused without re-entering the widget's `layout()` method.

```rust
pub struct LayoutCache {
    /// Last constraints this widget was laid out with.
    last_constraints: Option<Limits>,
    /// Cached layout result.
    cached_result: Option<LayoutNode>,
    /// Content hash (changes when widget content changes).
    content_hash: u64,
}

impl LayoutCache {
    pub fn get(&self, constraints: &Limits, content_hash: u64) -> Option<&LayoutNode> {
        if self.last_constraints.as_ref() == Some(constraints)
            && self.content_hash == content_hash
        {
            self.cached_result.as_ref()
        } else {
            None
        }
    }
}
```

**Relayout boundaries:**

Fixed-size containers act as relayout boundaries — changes inside them never propagate dirty flags upward. This isolates layout recalculation to subtrees:

```rust
pub trait Widget<M, R: Renderer> {
    // ... existing methods ...

    /// Whether this widget acts as a relayout boundary.
    /// Fixed-size widgets return true; their children's layout changes
    /// never affect siblings or ancestors.
    fn is_relayout_boundary(&self) -> bool { false }
}
```

-----

### 5.5 Grid Layout

In addition to `row![]` and `column![]` (flex layout), Interface Kit provides a grid layout for two-dimensional arrangements:

```rust
fn view(&self) -> Element<Message> {
    grid![
        // row 0
        [text("Name"), text_input("", &self.name).on_input(Message::NameChanged)],
        // row 1
        [text("Email"), text_input("", &self.email).on_input(Message::EmailChanged)],
        // row 2
        [space(), button("Submit").on_press(Message::Submit)],
    ]
    .column_widths([Length::FillPortion(1), Length::FillPortion(3)])
    .row_spacing(8)
    .column_spacing(16)
    .into()
}
```

Grid supports column/row spans, alignment per cell, and auto-sizing.
