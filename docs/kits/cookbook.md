# AIOS Kit Cookbook

End-to-end examples showing how multiple Kits compose into real applications. Each recipe is a complete, self-contained walkthrough — from agent manifest to running code.

All examples use aspirational `aios_*` crate APIs. They represent the target SDK design, not current implementation.

---

## Recipe 1: Notes App

**Kits used:** App Kit + Interface Kit + Storage Kit + Search Kit + Flow Kit

A native notes app with full-text search, reactive updates, and clipboard integration. This recipe demonstrates the Elm Architecture with Space-backed state and reactive queries — when another agent modifies a note in the same Space, your UI updates automatically.

### Agent Manifest

```toml
[agent]
name = "com.aios.notes"
version = "0.1.0"
display_name = "Notes"

[capabilities.required]
storage_read = { spaces = ["user/notes/"] }
storage_write = { spaces = ["user/notes/"] }
search_index = { spaces = ["user/notes/"] }

[capabilities.optional]
flow_clipboard = true
flow_share = true

[ui]
requires_compositor = true
min_surface_size = { width = 320, height = 480 }

[content_types]
handles = ["text/plain", "text/markdown"]
produces = ["text/plain", "text/markdown"]

[scriptable]
actions = ["create_note", "search", "export"]
properties = ["note_count", "current_note"]
```

### Data Model

```rust
use aios_storage::{Space, Object, ObjectId, Query, ReactiveQuery};
use aios_search::{SearchIndex, SearchQuery, SearchResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Note {
    title: String,
    body: String,
    tags: Vec<String>,
    created_at: aios_storage::Timestamp,
    modified_at: aios_storage::Timestamp,
}

/// The Elm Architecture model — all app state lives here.
struct NotesModel {
    notes: Vec<(ObjectId, Note)>,
    selected: Option<ObjectId>,
    search_query: String,
    search_results: Vec<SearchResult>,
    editing: bool,
}
```

### Messages & Update

```rust
use aios_interface::{Command, InterfaceCommand};

#[derive(Clone, Debug)]
enum Msg {
    // User actions
    SelectNote(ObjectId),
    CreateNote,
    DeleteNote(ObjectId),
    UpdateTitle(String),
    UpdateBody(String),
    SearchChanged(String),

    // System events (from reactive queries)
    NotesChanged(Vec<(ObjectId, Note)>),
    SearchResults(Vec<SearchResult>),

    // Flow events
    PasteFromClipboard(String),
}

fn update(model: &mut NotesModel, msg: Msg) -> Vec<Command<Msg>> {
    match msg {
        Msg::CreateNote => {
            let note = Note {
                title: "Untitled".into(),
                body: String::new(),
                tags: vec![],
                created_at: aios_storage::now(),
                modified_at: aios_storage::now(),
            };
            // Command writes to Space — reactive query will fire NotesChanged
            vec![Command::perform(
                aios_storage::object_create("user/notes/", &note),
                |id| Msg::SelectNote(id),
            )]
        }

        Msg::SearchChanged(query) => {
            model.search_query = query.clone();
            vec![Command::perform(
                aios_search::search(SearchQuery::fulltext(&query).space("user/notes/")),
                Msg::SearchResults,
            )]
        }

        Msg::NotesChanged(notes) => {
            model.notes = notes;
            vec![]
        }

        Msg::SelectNote(id) => {
            model.selected = Some(id);
            model.editing = true;
            vec![]
        }

        Msg::UpdateBody(body) => {
            if let Some(id) = model.selected {
                vec![Command::perform(
                    aios_storage::object_update(id, |note: &mut Note| {
                        note.body = body;
                        note.modified_at = aios_storage::now();
                    }),
                    |_| Msg::SelectNote(id),
                )]
            } else {
                vec![]
            }
        }

        _ => vec![], // Other handlers omitted for brevity
    }
}
```

### View

```rust
use aios_interface::prelude::*;

fn view(model: &NotesModel) -> impl View<Msg> {
    let sidebar = Column::new()
        .push(
            TextInput::new("Search notes...", &model.search_query)
                .on_input(Msg::SearchChanged)
                .style(Style::SearchField),
        )
        .push(
            Button::new("New Note")
                .on_press(Msg::CreateNote)
                .style(Style::Primary),
        )
        .push(
            Scrollable::new(
                Column::from_iter(model.notes.iter().map(|(id, note)| {
                    NoteListItem::new(&note.title, &note.modified_at)
                        .selected(model.selected == Some(*id))
                        .on_press(Msg::SelectNote(*id))
                })),
            ),
        )
        .width(Length::Fixed(260.0));

    let editor = match model.selected {
        Some(id) => {
            let note = model.notes.iter().find(|(nid, _)| *nid == id);
            match note {
                Some((_, note)) => Column::new()
                    .push(
                        TextInput::new("Title", &note.title)
                            .on_input(Msg::UpdateTitle)
                            .style(Style::Title),
                    )
                    .push(
                        TextEditor::new(&note.body)
                            .on_edit(Msg::UpdateBody)
                            .height(Length::Fill),
                    )
                    .into_view(),
                None => Text::new("Note not found").into_view(),
            }
        }
        None => Text::new("Select or create a note").centered().into_view(),
    };

    Row::new()
        .push(sidebar)
        .push(Rule::vertical(1))
        .push(editor.width(Length::Fill))
}
```

### Reactive Queries (BeOS-Inspired)

The key differentiator from traditional desktop apps: when *any* agent modifies the `user/notes/` Space, the notes list updates automatically.

```rust
fn subscriptions(model: &NotesModel) -> Vec<Subscription<Msg>> {
    vec![
        // Reactive query: re-fires whenever objects in user/notes/ change
        aios_storage::watch(
            Query::space("user/notes/").sort_by("modified_at", Descending),
        )
        .map(Msg::NotesChanged),

        // Flow clipboard: receive paste events
        aios_flow::clipboard_watch()
            .filter(|entry| entry.content_type().is_text())
            .map(|entry| Msg::PasteFromClipboard(entry.as_text().unwrap())),
    ]
}
```

### Scriptable Interface

AIRS agents can drive the Notes app programmatically:

```rust
use aios_app::Scriptable;

impl Scriptable for NotesApp {
    fn enumerate_properties(&self) -> Vec<Property> {
        vec![
            Property::new("note_count", PropertyType::Integer),
            Property::new("current_note", PropertyType::ObjectId),
        ]
    }

    fn get_property(&self, name: &str) -> Option<Value> {
        match name {
            "note_count" => Some(Value::Integer(self.model.notes.len() as i64)),
            "current_note" => self.model.selected.map(Value::ObjectId),
            _ => None,
        }
    }

    fn invoke_action(&mut self, name: &str, args: &[Value]) -> Result<Value, ScriptError> {
        match name {
            "create_note" => {
                let cmds = update(&mut self.model, Msg::CreateNote);
                self.execute_commands(cmds);
                Ok(Value::Null)
            }
            "search" => {
                let query = args.first().and_then(|v| v.as_str()).unwrap_or("");
                let cmds = update(&mut self.model, Msg::SearchChanged(query.into()));
                self.execute_commands(cmds);
                Ok(Value::Integer(self.model.search_results.len() as i64))
            }
            _ => Err(ScriptError::UnknownAction(name.into())),
        }
    }
}
```

---

## Recipe 2: Weather Agent

**Kits used:** App Kit + Network Kit + Interface Kit + Context Kit + Capability Kit

A context-aware weather agent that adapts its display based on the user's current activity context. Demonstrates capability-gated network access and context subscriptions.

### Agent Manifest

```toml
[agent]
name = "com.aios.weather"
version = "0.1.0"
display_name = "Weather"

[capabilities.required]
network_http = { domains = ["api.weather.gov", "api.openweathermap.org"] }

[capabilities.optional]
location_coarse = true   # City-level, not GPS
context_read = true      # Read current activity context

[ui]
requires_compositor = true
min_surface_size = { width = 300, height = 200 }

[scriptable]
actions = ["refresh", "set_location"]
properties = ["temperature", "condition", "location"]
```

### Context-Aware Model

```rust
use aios_context::{ContextSnapshot, ActivityState};
use aios_capability::{CapabilityHandle, CapabilityStatus};
use aios_network::{HttpClient, HttpRequest};

struct WeatherModel {
    location: Option<String>,
    current: Option<WeatherData>,
    forecast: Vec<ForecastDay>,
    loading: bool,
    error: Option<String>,

    // Context awareness
    activity: ActivityState,
    display_mode: DisplayMode,

    // Capability status
    network_granted: bool,
    location_granted: bool,
}

#[derive(Clone, Debug)]
enum DisplayMode {
    Full,           // Default: full forecast with charts
    Compact,        // During meetings/focus: just temp + icon
    Glanceable,     // Lock screen: single line
}

#[derive(Clone, Debug)]
enum Msg {
    Refresh,
    WeatherLoaded(Result<WeatherData, WeatherError>),
    ForecastLoaded(Result<Vec<ForecastDay>, WeatherError>),

    // Context changes
    ContextChanged(ContextSnapshot),

    // Capability responses
    CapabilityResult(CapabilityStatus),
}
```

### Capability Negotiation

```rust
fn init() -> (WeatherModel, Vec<Command<Msg>>) {
    let model = WeatherModel {
        location: None,
        current: None,
        forecast: vec![],
        loading: false,
        error: None,
        activity: ActivityState::default(),
        display_mode: DisplayMode::Full,
        network_granted: false,
        location_granted: false,
    };

    // Request capabilities at launch — user sees a single prompt
    let cmds = vec![
        Command::perform(
            aios_capability::request_capabilities(&[
                "network_http",
                "location_coarse",
                "context_read",
            ]),
            Msg::CapabilityResult,
        ),
    ];

    (model, cmds)
}

fn update(model: &mut WeatherModel, msg: Msg) -> Vec<Command<Msg>> {
    match msg {
        Msg::CapabilityResult(status) => {
            model.network_granted = status.is_granted("network_http");
            model.location_granted = status.is_granted("location_coarse");

            if !model.network_granted {
                model.error = Some("Weather requires network access".into());
                return vec![];
            }

            // Graceful degradation: without location, use manual entry
            if model.location_granted {
                vec![Command::perform(
                    aios_context::get_location_city(),
                    |city| Msg::Refresh,
                )]
            } else {
                // Show location picker instead
                model.location = Some("San Francisco".into()); // Default
                vec![Command::message(Msg::Refresh)]
            }
        }

        Msg::ContextChanged(snapshot) => {
            model.activity = snapshot.activity;
            model.display_mode = match model.activity {
                ActivityState::Meeting | ActivityState::Focused => DisplayMode::Compact,
                ActivityState::LockScreen => DisplayMode::Glanceable,
                _ => DisplayMode::Full,
            };
            vec![]
        }

        Msg::Refresh => {
            model.loading = true;
            let location = model.location.clone().unwrap_or_default();
            vec![
                Command::perform(
                    fetch_weather(&location),
                    Msg::WeatherLoaded,
                ),
                Command::perform(
                    fetch_forecast(&location),
                    Msg::ForecastLoaded,
                ),
            ]
        }

        _ => vec![],
    }
}
```

### Context Subscriptions

```rust
fn subscriptions(_model: &WeatherModel) -> Vec<Subscription<Msg>> {
    vec![
        // Adapt display when user's activity context changes
        aios_context::watch_activity().map(|snapshot| Msg::ContextChanged(snapshot)),

        // Auto-refresh every 30 minutes
        aios_app::interval(Duration::from_secs(30 * 60)).map(|_| Msg::Refresh),
    ]
}
```

### Adaptive View

```rust
fn view(model: &WeatherModel) -> impl View<Msg> {
    match model.display_mode {
        DisplayMode::Full => full_weather_view(model),
        DisplayMode::Compact => compact_weather_view(model),
        DisplayMode::Glanceable => glanceable_view(model),
    }
}

fn compact_weather_view(model: &WeatherModel) -> impl View<Msg> {
    match &model.current {
        Some(weather) => Row::new()
            .push(WeatherIcon::new(&weather.condition).size(32))
            .push(Text::new(format!("{}°", weather.temp_f)).size(24))
            .spacing(8)
            .align(Alignment::Center)
            .into_view(),
        None => Text::new("--°").size(24).into_view(),
    }
}
```

---

## Related Documents

- [Kit Architecture](README.md) — Hub document with dependency graph and design principles
- [App Kit](application/app.md) — Application lifecycle and Scriptable trait
- [Interface Kit](application/interface.md) — UI toolkit and Elm Architecture
- [Storage Kit](platform/storage.md) — Spaces, objects, and reactive queries
- [Search Kit](intelligence/search.md) — Full-text and semantic search
- [Flow Kit](intelligence/flow.md) — Clipboard and content transfer
- [Context Kit](intelligence/context.md) — Activity context and adaptation
- [Network Kit](platform/network.md) — Capability-gated HTTP
- [Capability Kit](kernel/capability.md) — Capability request and negotiation
