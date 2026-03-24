# AIOS Interface Kit — Application Model

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [widgets.md](./widgets.md) — Widget library, [backends.md](./backends.md) — Platform backends, [agents.md](../agents.md) — Agent SDK

-----

## 3. Application Model

Interface Kit follows the **Model → View → Update** pattern (inspired by Elm Architecture): each agent is an isolated process with its own state — no shared mutable state between agents. The default iced bridge preserves this pattern naturally, but any bridge that implements Interface Kit's `Widget<M>` trait inherits the same guarantees.

### 3.1 The Pattern

```rust
use aios_interface::prelude::*;

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

// Note: simplified for illustration. Full trait signature is in
// docs/kits/application/interface.md §2.3 (Widget<M: Clone>).
impl Widget<Message> for NotesApp {
    type Flags = AppFlags;

    fn new(flags: AppFlags) -> (Self, InterfaceCommand<Message>) {
        let app = NotesApp {
            notes: Vec::new(),
            search_query: String::new(),
            selected: None,
            theme: flags.system_theme,
        };
        // Load notes from space on startup
        let cmd = InterfaceCommand::perform(
            space::query("notes/*", QueryOrder::Modified),
            |result| Message::SpaceSynced(result.unwrap_or_default()),
        );
        (app, cmd)
    }

    fn title(&self) -> &str {
        "Notes"
    }

    fn update(&mut self, message: Message) -> InterfaceCommand<Message> {
        match message {
            Message::SearchChanged(query) => {
                self.search_query = query;
                InterfaceCommand::none()
            }
            Message::NoteSelected(idx) => {
                self.selected = Some(idx);
                InterfaceCommand::none()
            }
            Message::NoteCreated => {
                let note = Note::new();
                self.notes.push(note.clone());
                self.selected = Some(self.notes.len() - 1);
                // Persist to space
                InterfaceCommand::perform(
                    space::write(&format!("notes/{}", note.id), &note),
                    |_| Message::SearchChanged(String::new()),
                )
            }
            Message::NoteDeleted(idx) => {
                let note = self.notes.remove(idx);
                self.selected = None;
                InterfaceCommand::perform(
                    space::delete(&format!("notes/{}", note.id)),
                    |_| Message::SearchChanged(String::new()),
                )
            }
            Message::ContentEdited(content) => {
                if let Some(idx) = self.selected {
                    self.notes[idx].content = content;
                }
                InterfaceCommand::none()
            }
            Message::ThemeChanged(theme) => {
                self.theme = theme;
                InterfaceCommand::none()
            }
            Message::SpaceSynced(notes) => {
                self.notes = notes;
                InterfaceCommand::none()
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

| Property | Elm Architecture | AIOS Agent Model |
|---|---|---|
| Isolated state | Each app owns its state | Each agent owns its memory (TTBR0) |
| Message passing | Events → messages → update | IPC messages → capability checked |
| No shared mutation | State only changes in `update` | No shared memory between agents |
| Declarative view | `view()` returns a description | Compositor renders declarative surface |
| Side effects are explicit | `InterfaceCommand` for async work | IPC for all external interaction |
| Subscriptions | Listen for external events | IPC channels, space watches |

The Elm Architecture is not just a UI pattern in AIOS — it's the natural expression of the capability-isolated agent model at the GUI level.

**How this compares to alternatives:**

- **SwiftUI** uses `@State`/`@Observable` with implicit diffing — powerful but couples state management to the framework. AIOS keeps state management entirely in user code.
- **Jetpack Compose** uses `@Composable` recomposition — smart but opaque. AIOS's explicit `update()` → `view()` cycle is fully debuggable.
- **Xilem** (Linebender) uses type-driven view diffing for zero-allocation rebuilds — a potential future optimization for AIOS, but the Elm model is simpler and sufficient for Phase 34.

### 3.3 Command System

Commands represent side effects — things the application wants to happen outside its own state:

```rust
pub enum InterfaceCommand<M> {
    /// No side effect.
    None,
    /// Execute an async operation and map the result to a message.
    Perform(Box<dyn Future<Output = M>>),
    /// Batch multiple commands.
    Batch(Vec<InterfaceCommand<M>>),
    /// Platform-specific command (AIOS features).
    Platform(PlatformCommand<M>),
}

pub enum PlatformCommand<M> {
    /// Post an attention item (AIOS only, no-op elsewhere).
    PostAttention(AttentionRequest),
    /// Push data to Flow tray (AIOS only, clipboard elsewhere).
    FlowPush(FlowData),
    /// Set semantic window hints (AIOS only, ignored elsewhere).
    SetWindowHints(WindowHints),
    /// Query agent capabilities (AIOS only, returns all-granted elsewhere).
    QueryCapabilities(Box<dyn Fn(CapabilitySet) -> M>),
    /// Subscribe to context changes (AIOS only, no-op elsewhere).
    SubscribeContext(Box<dyn Fn(ContextState) -> M>),
    /// Declare an intent for verification (AIOS only, no-op elsewhere).
    DeclareIntent(DeclaredIntent),
}
```

On non-AIOS platforms, `PlatformCommand` variants either no-op or fall back to standard behavior (e.g., `FlowPush` → clipboard write). Application code uses feature detection at runtime, not conditional compilation:

```rust
fn update(&mut self, message: Message) -> InterfaceCommand<Message> {
    match message {
        Message::ShareData(data) => {
            if platform::capabilities().flow_integration {
                InterfaceCommand::platform(PlatformCommand::FlowPush(FlowData {
                    content: data,
                    content_type: ContentType::Document,
                    provenance: Provenance::current(),
                }))
            } else {
                InterfaceCommand::clipboard(ClipboardAction::Write(data.to_string()))
            }
        }
        _ => InterfaceCommand::none(),
    }
}
```
