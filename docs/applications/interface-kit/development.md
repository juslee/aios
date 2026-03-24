# AIOS Interface Kit — Development

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [agents.md](../agents.md) — Agent SDK & distribution, [backends.md](./backends.md) — Platform backends

-----

## 11. Agent UI Development

### 11.1 SDK Integration

The AIOS SDK provides the Interface Kit as a crate:

```toml
# Cargo.toml for a third-party agent
[package]
name = "weather-agent"
version = "0.1.0"

[dependencies]
aios-interface = "0.1"
aios-sdk = "0.1"

[target.'cfg(not(target_os = "aios"))'.dependencies]
# Fallback backends for development
aios-interface-linux = "0.1"
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

```text
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

## 14. Cross-Platform Development Workflow

### 14.1 Feature Detection Pattern

```rust
/// Check AIOS features at runtime, not compile time
fn setup(&self) -> InterfaceCommand<Message> {
    let caps = platform::capabilities();

    if caps.space_backed_data {
        // Load from space
        InterfaceCommand::perform(
            space::query("data/*", QueryOrder::Modified),
            Message::DataLoaded,
        )
    } else {
        // Load from filesystem
        InterfaceCommand::perform(
            fs::read_dir("~/.config/myagent/data/"),
            Message::DataLoaded,
        )
    }
}
```

### 14.2 CI/CD Pipeline

```text
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

### 14.4 Testing Strategy

Interface Kit provides multiple testing layers:

**Widget unit tests** (no platform required):

```rust
#[test]
fn notes_app_creates_note() {
    let (mut app, _cmd) = NotesApp::new(AppFlags::default());
    let cmd = app.update(Message::NoteCreated);
    assert_eq!(app.notes.len(), 1);
    assert!(app.selected.is_some());
    assert!(matches!(cmd, InterfaceCommand::Perform(_)));
}
```

**Snapshot testing** (view tree comparison):

```rust
#[test]
fn notes_app_view_snapshot() {
    let app = NotesApp { notes: vec![sample_note()], ..Default::default() };
    let view = app.view();
    // Compare widget tree structure against saved snapshot
    assert_snapshot!(view, "notes_app_with_one_note");
}
```

**Accessibility validation:**

```rust
#[test]
fn notes_app_accessibility() {
    let app = NotesApp::default();
    let view = app.view();
    let tree = build_accessibility_tree(&view);

    // Every interactive element must be focusable
    for node in tree.interactive_nodes() {
        assert!(node.focusable, "Interactive node {:?} is not focusable", node.role);
    }

    // Every image must have alt text
    for node in tree.nodes_with_role(AccessRole::Image) {
        assert!(node.label.is_some() || node.description.is_some(),
            "Image node missing alt text");
    }
}
```

**Visual regression testing:**

Render the widget tree to an in-memory buffer and compare pixel-by-pixel against a reference image. Useful for catching unintended visual changes across refactors. Runs on the software renderer backend (no GPU required).
