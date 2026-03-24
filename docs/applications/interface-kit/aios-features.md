# AIOS Interface Kit — AIOS-Specific Features

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [flow.md](../../storage/flow.md) — Flow data model, [compositor.md](../../platform/compositor.md) — Compositor protocol, [agents.md](../agents.md) — Agent capabilities

-----

## 10. AIOS-Specific Features

These features are only available when running on AIOS. On other platforms, the API calls compile but return defaults or no-ops. Application code uses feature detection, not conditional compilation:

```rust
fn update(&mut self, message: Message) -> InterfaceCommand<Message> {
    match message {
        Message::ShareData(data) => {
            if platform::capabilities().flow_integration {
                // On AIOS: push to Flow tray with full type info
                InterfaceCommand::platform(PlatformCommand::FlowPush(FlowData {
                    content: data,
                    content_type: ContentType::Document,
                    provenance: Provenance::current(),
                }))
            } else {
                // Elsewhere: standard clipboard
                InterfaceCommand::clipboard(ClipboardAction::Write(data.to_string()))
            }
        }
        _ => InterfaceCommand::none(),
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
    fn save_state<S: Serialize>(&self, key: &str, state: &S) -> InterfaceCommand<()>;

    /// Load widget state from the agent's space
    fn load_state<D: DeserializeOwned>(&self, key: &str) -> InterfaceCommand<Option<D>>;

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
