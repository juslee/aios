# AIOS Interface Kit — Accessibility

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [accessibility.md](../../experience/accessibility.md) — System accessibility, [development.md](./development.md) — Testing strategy

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

### 12.4 AccessKit Integration

AIOS's accessibility tree model is inspired by AccessKit — a cross-platform Rust accessibility library used by iced, egui, and Masonry. The key design decisions:

- **Incremental updates.** The tree is not rebuilt every frame. Widgets produce `TreeUpdate` deltas containing only changed nodes. This matches the damage-tracking approach used in the render pipeline.
- **~60 semantic roles.** AccessKit defines roles matching WAI-ARIA (Button, CheckBox, Heading, Link, List, Menu, Slider, Table, TextInput, etc.). AIOS adopts this taxonomy for its `AccessRole` enum.
- **Platform adapters.** On non-AIOS platforms, AccessKit translates the tree to native APIs: AT-SPI2 (Linux), NSAccessibility (macOS), UIA (Windows). On AIOS, the tree feeds directly to the built-in screen reader (eSpeak-NG) via IPC — no AccessKit adapter needed.

```rust
/// Incremental accessibility tree update.
/// Produced by the widget runtime after each view() cycle.
pub struct AccessTreeUpdate {
    /// Changed or new nodes (keyed by NodeId).
    pub nodes: Vec<(NodeId, AccessibilityNode)>,
    /// Removed node IDs.
    pub removed: Vec<NodeId>,
    /// Currently focused node.
    pub focus: Option<NodeId>,
}
```

-----

### 12.5 Reduced Motion

Agents must respect the system's reduced-motion preference:

- The `AnimationConfig.reduce_motion` flag in the theme (see [theme.md §6.4](./theme.md)) is set by the accessibility subsystem.
- When true, all transitions collapse to instant state changes. Spring animations resolve immediately to their target value.
- Widgets never check this flag directly — the animation runtime handles it transparently.
- On non-AIOS platforms, `reduce_motion` reads `prefers-reduced-motion` (Web), `UIAccessibility.isReduceMotionEnabled` (macOS), or `gtk-enable-animations` (Linux).

-----

### 12.6 High Contrast and Magnification

- The theme system provides high-contrast palette variants with minimum 7:1 contrast ratios (WCAG AAA).
- Agents that use theme tokens automatically gain high-contrast support. Agents that hardcode colors will fail accessibility validation.
- Magnification is a compositor responsibility (see [compositor.md §11](../../platform/compositor/security.md)) — agents do not implement zoom. The compositor scales surfaces and adjusts input coordinates.
