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
    LocationResolved(String),
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
                    |city| Msg::LocationResolved(city),
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

        Msg::LocationResolved(city) => {
            model.location = Some(city);
            vec![Command::message(Msg::Refresh)]
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

## Recipe 3: Media Player

**Kits used:** App Kit + Media Kit + Audio Kit + Interface Kit + Storage Kit + Flow Kit

A full-featured media player demonstrating pipeline graph construction, A/V synchronization,
session management with Now Playing metadata, and Flow integration for sharing. Shows how
Media Kit, Audio Kit, and Interface Kit compose for multimedia playback.

### Agent Manifest

```toml
[agent]
name = "com.aios.mediaplayer"
version = "0.1.0"
display_name = "Media Player"

[capabilities.required]
storage_read = { spaces = ["user/media/"] }
audio_playback = { roles = ["media"] }

[capabilities.optional]
media_drm = true
storage_write = { spaces = ["user/media/"] }
flow_share = true
network_http = { domains = ["*"] }    # Streaming sources

[ui]
requires_compositor = true
min_surface_size = { width = 480, height = 320 }

[content_types]
handles = [
    "audio/mpeg", "audio/flac", "audio/ogg",
    "video/mp4", "video/webm", "video/mkv",
]
produces = ["application/x-now-playing"]

[scriptable]
actions = ["play", "pause", "stop", "next", "previous", "seek"]
properties = ["state", "position", "duration", "track_title", "volume"]
```

### Data Model

```rust
use aios_media::{
    PlaybackPipeline, MediaSession, MediaCodec, ContainerEngine,
    PlaybackState, MediaTrack, StreamingEngine,
};
use aios_audio::{AudioSession, AudioRole};
use aios_storage::{Space, Object, ObjectId, Query, ReactiveQuery};
use aios_flow::{FlowChannel, FlowEntry};
use std::time::Duration;

struct PlayerModel {
    // Library
    library: Vec<(ObjectId, MediaTrack)>,
    playlist: Vec<ObjectId>,
    current_index: usize,

    // Playback state
    state: PlaybackState,
    position: Duration,
    duration: Duration,
    volume: f32,

    // Sessions
    audio_session: Option<AudioSession>,
    media_session: Option<MediaSession>,

    // UI
    show_playlist: bool,
    visualizer_enabled: bool,
}

#[derive(Clone, Debug)]
enum Msg {
    // Transport controls
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    Seek(Duration),
    SetVolume(f32),

    // Library
    LibraryChanged(Vec<(ObjectId, MediaTrack)>),
    OpenFile(ObjectId),
    AddToPlaylist(ObjectId),

    // Pipeline events
    PipelineReady(PlaybackPipeline),
    PositionTick(Duration),
    TrackEnded,
    PipelineError(aios_media::MediaError),

    // Audio session events
    AudioSessionAcquired(AudioSession),
    AudioInterrupted,
    AudioResumed,

    // Flow
    ShareNowPlaying,
}
```

### Pipeline Construction

```rust
fn build_pipeline(track: &MediaTrack) -> Command<Msg> {
    Command::perform(
        async move {
            // 1. Open container and probe streams
            let container = ContainerEngine::open(track.uri()).await?;
            let audio_stream = container.best_audio_stream()?;
            let video_stream = container.best_video_stream(); // Optional

            // 2. Select codecs (hardware preferred, software fallback)
            let audio_codec = MediaCodec::select_for(&audio_stream).await?;
            let video_codec = match &video_stream {
                Some(stream) => Some(MediaCodec::select_for(stream).await?),
                None => None,
            };

            // 3. Build pipeline graph
            let mut pipeline = PlaybackPipeline::builder()
                .source(container)
                .audio_decoder(audio_codec)
                .audio_sink(AudioSession::current()?)
                .build();

            if let (Some(stream), Some(codec)) = (&video_stream, video_codec) {
                pipeline.add_video_decoder(codec)?;
            }

            Ok(pipeline)
        },
        |result| match result {
            Ok(pipeline) => Msg::PipelineReady(pipeline),
            Err(e) => Msg::PipelineError(e),
        },
    )
}
```

### Audio Session Management

```rust
fn update(model: &mut PlayerModel, msg: Msg) -> Vec<Command<Msg>> {
    match msg {
        Msg::Play => {
            // Acquire an audio session before starting playback
            if model.audio_session.is_none() {
                return vec![Command::perform(
                    AudioSession::acquire(AudioRole::Media),
                    |result| match result {
                        Ok(session) => Msg::AudioSessionAcquired(session),
                        Err(_) => Msg::PipelineError(aios_media::MediaError::AudioUnavailable),
                    },
                )];
            }
            model.state = PlaybackState::Playing;
            // Register Now Playing metadata with the system media session
            if let Some(ref mut session) = model.media_session {
                session.set_playback_state(PlaybackState::Playing);
            }
            vec![]
        }

        Msg::AudioSessionAcquired(session) => {
            model.audio_session = Some(session);
            // Now that we have audio, start playback
            vec![Command::message(Msg::Play)]
        }

        Msg::AudioInterrupted => {
            // Another agent (e.g., phone call) took audio focus
            model.state = PlaybackState::Paused;
            vec![]
        }

        Msg::AudioResumed => {
            // Audio focus returned — resume if we were playing
            if model.state == PlaybackState::Paused {
                model.state = PlaybackState::Playing;
            }
            vec![]
        }

        Msg::TrackEnded => {
            if model.current_index + 1 < model.playlist.len() {
                vec![Command::message(Msg::Next)]
            } else {
                model.state = PlaybackState::Stopped;
                model.position = Duration::ZERO;
                vec![]
            }
        }

        Msg::ShareNowPlaying => {
            if let Some(track) = model.current_track() {
                vec![Command::perform(
                    aios_flow::publish(
                        FlowChannel::share(),
                        FlowEntry::new(track.as_now_playing()),
                    ),
                    |_| Msg::Play, // No-op on completion
                )]
            } else {
                vec![]
            }
        }

        Msg::Seek(position) => {
            model.position = position;
            vec![]
        }

        Msg::SetVolume(vol) => {
            model.volume = vol.clamp(0.0, 1.0);
            if let Some(ref session) = model.audio_session {
                session.set_volume(model.volume);
            }
            vec![]
        }

        _ => vec![],
    }
}
```

### Playback View

```rust
use aios_interface::prelude::*;

fn view(model: &PlayerModel) -> impl View<Msg> {
    let track_info = match model.current_track() {
        Some(track) => Column::new()
            .push(Text::new(&track.title).size(20).style(Style::Heading))
            .push(Text::new(&track.artist).size(14).style(Style::Secondary))
            .push(Text::new(&track.album).size(12).style(Style::Tertiary)),
        None => Column::new()
            .push(Text::new("No track selected").style(Style::Placeholder)),
    };

    let progress = ProgressBar::new(0.0..=model.duration.as_secs_f64())
        .value(model.position.as_secs_f64())
        .on_drag(|pos| Msg::Seek(Duration::from_secs_f64(pos)));

    let transport = Row::new()
        .push(Button::icon(Icon::Previous).on_press(Msg::Previous))
        .push(match model.state {
            PlaybackState::Playing => Button::icon(Icon::Pause).on_press(Msg::Pause),
            _ => Button::icon(Icon::Play).on_press(Msg::Play),
        })
        .push(Button::icon(Icon::Next).on_press(Msg::Next))
        .push(
            Slider::new(0.0..=1.0, model.volume, Msg::SetVolume)
                .width(Length::Fixed(100.0)),
        )
        .push(Button::icon(Icon::Share).on_press(Msg::ShareNowPlaying))
        .spacing(12)
        .align(Alignment::Center);

    Column::new()
        .push(track_info)
        .push(progress)
        .push(transport)
        .spacing(16)
        .padding(20)
}
```

### Subscriptions

```rust
fn subscriptions(model: &PlayerModel) -> Vec<Subscription<Msg>> {
    let mut subs = vec![
        // Watch media library for new files
        aios_storage::watch(
            Query::space("user/media/")
                .content_type("audio/*")
                .sort_by("title", Ascending),
        )
        .map(Msg::LibraryChanged),
    ];

    // Position tick only while playing
    if model.state == PlaybackState::Playing {
        subs.push(
            aios_app::interval(Duration::from_millis(250))
                .map(|_| Msg::PositionTick(Duration::from_millis(250))),
        );
    }

    // Audio session interruption events
    if model.audio_session.is_some() {
        subs.push(
            aios_audio::watch_session_events()
                .map(|event| match event {
                    aios_audio::SessionEvent::Interrupted => Msg::AudioInterrupted,
                    aios_audio::SessionEvent::Resumed => Msg::AudioResumed,
                    _ => Msg::Play, // Ignore other events
                }),
        );
    }

    subs
}
```

---

## Recipe 4: Chat Client

**Kits used:** App Kit + Conversation Kit + AIRS Kit + Network Kit + Notification Kit + Interface Kit + Storage Kit

An AI-powered chat client demonstrating Conversation Kit's session lifecycle, streaming token
delivery, tool orchestration, and Notification Kit integration. Shows how to build a responsive
chat interface with real-time AI responses.

### Agent Manifest

```toml
[agent]
name = "com.aios.chat"
version = "0.1.0"
display_name = "Chat"

[capabilities.required]
conversation_session = true
storage_read = { spaces = ["user/chat/"] }
storage_write = { spaces = ["user/chat/"] }

[capabilities.optional]
network_http = { domains = ["*"] }     # For tool execution (web search)
notification_post = { channels = ["messages"] }
search_index = { spaces = ["user/chat/"] }
flow_clipboard = true

[ui]
requires_compositor = true
min_surface_size = { width = 400, height = 600 }

[content_types]
handles = ["text/plain", "text/markdown", "application/x-chat-session"]
produces = ["text/plain", "text/markdown"]

[scriptable]
actions = ["new_session", "send_message", "search_history"]
properties = ["session_count", "current_session", "model_name"]
```

### Data Model

```rust
use aios_conversation::{
    ConversationSession, SessionId, MessageRole, StreamingResponse,
    ToolInvocation, ContextWindow, SessionConfig,
};
use aios_airs::{InferenceEngine, ModelId, InferenceSession};
use aios_notification::{NotificationBuilder, ChannelId, Urgency};
use aios_storage::{Space, Object, ObjectId, Query};

struct ChatModel {
    // Sessions
    sessions: Vec<ChatSession>,
    active_session: Option<SessionId>,

    // Current conversation
    messages: Vec<ChatMessage>,
    input_text: String,
    streaming: Option<StreamingState>,

    // Available models
    models: Vec<ModelInfo>,
    selected_model: ModelId,

    // UI state
    sidebar_visible: bool,
}

struct ChatSession {
    id: SessionId,
    title: String,
    last_message_at: aios_storage::Timestamp,
    message_count: usize,
}

struct ChatMessage {
    role: MessageRole,
    content: String,
    tool_calls: Vec<ToolInvocation>,
    timestamp: aios_storage::Timestamp,
}

struct StreamingState {
    partial_content: String,
    tokens_received: usize,
    tool_in_progress: Option<String>,
}

#[derive(Clone, Debug)]
enum Msg {
    // Session management
    NewSession,
    SwitchSession(SessionId),
    SessionCreated(SessionId),
    SessionsLoaded(Vec<ChatSession>),
    DeleteSession(SessionId),

    // Messaging
    InputChanged(String),
    SendMessage,
    MessageSent(SessionId),

    // Streaming response
    StreamToken(String),
    StreamToolCall(ToolInvocation),
    StreamComplete(String),
    StreamError(aios_conversation::ConversationError),

    // Model selection
    ModelsAvailable(Vec<ModelInfo>),
    SelectModel(ModelId),

    // History
    HistoryLoaded(Vec<ChatMessage>),
    SearchHistory(String),
    CopyMessage(usize),
}
```

### Session Lifecycle

```rust
fn update(model: &mut ChatModel, msg: Msg) -> Vec<Command<Msg>> {
    match msg {
        Msg::NewSession => {
            vec![Command::perform(
                async {
                    let session = ConversationSession::create(
                        SessionConfig::builder()
                            .title("New Chat")
                            .persist_to("user/chat/")
                            .context_window(ContextWindow::default())
                            .build(),
                    ).await?;
                    Ok(session.id())
                },
                |result: Result<SessionId, _>| match result {
                    Ok(id) => Msg::SessionCreated(id),
                    Err(e) => Msg::StreamError(e),
                },
            )]
        }

        Msg::SendMessage => {
            if model.input_text.trim().is_empty() {
                return vec![];
            }

            let text = model.input_text.clone();
            model.input_text.clear();

            // Add user message to local state immediately
            model.messages.push(ChatMessage {
                role: MessageRole::User,
                content: text.clone(),
                tool_calls: vec![],
                timestamp: aios_storage::now(),
            });

            // Begin streaming state
            model.streaming = Some(StreamingState {
                partial_content: String::new(),
                tokens_received: 0,
                tool_in_progress: None,
            });

            let session_id = model.active_session.unwrap();
            let model_id = model.selected_model.clone();

            vec![Command::perform(
                async move {
                    let session = ConversationSession::resume(session_id).await?;
                    session.send_and_stream(&text, &model_id).await
                },
                |result| match result {
                    Ok(_) => Msg::MessageSent(session_id),
                    Err(e) => Msg::StreamError(e),
                },
            )]
        }

        Msg::StreamToken(token) => {
            if let Some(ref mut streaming) = model.streaming {
                streaming.partial_content.push_str(&token);
                streaming.tokens_received += 1;
            }
            vec![]
        }

        Msg::StreamToolCall(invocation) => {
            if let Some(ref mut streaming) = model.streaming {
                streaming.tool_in_progress = Some(invocation.tool_name.clone());
            }
            vec![]
        }

        Msg::StreamComplete(full_response) => {
            // Finalize the assistant message
            let tool_calls = model.streaming.as_ref()
                .map(|s| s.tool_in_progress.clone())
                .flatten()
                .into_iter()
                .collect();

            model.messages.push(ChatMessage {
                role: MessageRole::Assistant,
                content: full_response,
                tool_calls: vec![],
                timestamp: aios_storage::now(),
            });
            model.streaming = None;

            // Send notification if app is in background
            vec![Command::perform(
                async {
                    NotificationBuilder::new(ChannelId::new("messages"))
                        .title("New response")
                        .body("Your AI assistant has replied")
                        .urgency(Urgency::Default)
                        .post()
                        .await
                },
                |_| Msg::SendMessage, // No-op
            )]
        }

        Msg::SelectModel(model_id) => {
            model.selected_model = model_id;
            vec![]
        }

        Msg::CopyMessage(index) => {
            if let Some(message) = model.messages.get(index) {
                vec![Command::perform(
                    aios_flow::copy_to_clipboard(
                        FlowEntry::text(&message.content),
                    ),
                    |_| Msg::InputChanged(String::new()), // No-op
                )]
            } else {
                vec![]
            }
        }

        _ => vec![],
    }
}
```

### Streaming Chat View

```rust
use aios_interface::prelude::*;

fn view(model: &ChatModel) -> impl View<Msg> {
    let sidebar = Column::new()
        .push(
            Button::new("New Chat")
                .on_press(Msg::NewSession)
                .style(Style::Primary)
                .width(Length::Fill),
        )
        .push(
            Scrollable::new(Column::from_iter(
                model.sessions.iter().map(|session| {
                    SessionListItem::new(&session.title, &session.last_message_at)
                        .selected(model.active_session == Some(session.id))
                        .on_press(Msg::SwitchSession(session.id))
                }),
            )),
        )
        .width(Length::Fixed(240.0));

    let messages = Scrollable::new(
        Column::from_iter(
            model.messages.iter().enumerate().map(|(i, msg)| {
                MessageBubble::new(&msg.content)
                    .role(msg.role)
                    .tool_calls(&msg.tool_calls)
                    .on_copy(Msg::CopyMessage(i))
            })
        )
        // Show streaming partial response
        .push_maybe(model.streaming.as_ref().map(|s| {
            MessageBubble::new(&s.partial_content)
                .role(MessageRole::Assistant)
                .streaming(true)
                .tool_in_progress(s.tool_in_progress.as_deref())
        })),
    )
    .anchor_bottom();

    let input_bar = Row::new()
        .push(
            TextInput::new("Type a message...", &model.input_text)
                .on_input(Msg::InputChanged)
                .on_submit(Msg::SendMessage)
                .width(Length::Fill),
        )
        .push(
            Button::icon(Icon::Send)
                .on_press(Msg::SendMessage)
                .enabled(!model.input_text.is_empty() && model.streaming.is_none()),
        )
        .spacing(8)
        .padding(12);

    let chat_area = Column::new()
        .push(messages.height(Length::Fill))
        .push(Rule::horizontal(1))
        .push(input_bar);

    Row::new()
        .push(sidebar)
        .push(Rule::vertical(1))
        .push(chat_area.width(Length::Fill))
}
```

### Subscriptions

```rust
fn subscriptions(model: &ChatModel) -> Vec<Subscription<Msg>> {
    let mut subs = vec![
        // Watch session list for updates from other devices
        aios_storage::watch(
            Query::space("user/chat/")
                .content_type("application/x-chat-session")
                .sort_by("last_message_at", Descending),
        )
        .map(Msg::SessionsLoaded),
    ];

    // Subscribe to streaming tokens when a response is in progress
    if model.streaming.is_some() {
        if let Some(session_id) = model.active_session {
            subs.push(
                aios_conversation::watch_stream(session_id)
                    .map(|event| match event {
                        aios_conversation::StreamEvent::Token(t) => Msg::StreamToken(t),
                        aios_conversation::StreamEvent::ToolCall(tc) => Msg::StreamToolCall(tc),
                        aios_conversation::StreamEvent::Complete(text) => Msg::StreamComplete(text),
                        aios_conversation::StreamEvent::Error(e) => Msg::StreamError(e),
                    }),
            );
        }
    }

    subs
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
