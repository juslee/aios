# App Kit

**Layer:** Application | **Crate:** `aios_app` | **Architecture:** *(needs creation)*

---

## 1. Overview

App Kit manages high-level application lifecycle -- launch, quit, suspend, resume, and graceful teardown. It wraps [IPC Kit](../kernel/ipc.md)'s message primitives into an event-driven dispatch loop, provides standardized lifecycle callbacks, and exposes a scripting interface so AIRS agents and other applications can control any running app through a uniform protocol.

Every native AIOS application starts with App Kit as its entry point. Where [IPC Kit](../kernel/ipc.md) provides the raw message channels and [Capability Kit](../kernel/capability.md) provides the security primitives, App Kit composes them into a coherent application model: a message loop that dispatches incoming IPC messages to typed handlers, lifecycle hooks that respond to system events (suspend, memory pressure, power state changes), and a mandatory `Scriptable` trait that makes every application introspectable and controllable by AIRS.

The `Scriptable` trait is App Kit's key differentiator from other OS application frameworks. Inspired by BeOS's `BHandler` scripting suites and the `hey` command-line tool, every AIOS application publishes a self-describing schema of properties and actions. AIRS agents can enumerate what any running application exposes, read and write properties, invoke actions, and subscribe to state changes -- all through a single, capability-gated protocol. This transforms applications from opaque binaries into programmable building blocks that AIRS can compose into multi-agent workflows without per-application API knowledge. See the [BeOS/Haiku design lessons discussion](../../knowledge/discussions/2026-03-23-jl-beos-haiku-redox-lessons.md) for the full rationale.

---

## 2. Core Traits

### 2.1 Application

The root object representing a running application instance. An `Application` owns the message loop, holds the process-level capability set, and coordinates lifecycle transitions.

```rust
/// Root application object. One per process.
pub trait Application: Scriptable + Send {
    fn app_id(&self) -> &AppId;
    fn state(&self) -> AppState;
    fn capabilities(&self) -> &CapabilitySet;
    fn delegate(&self) -> &dyn AppDelegate;

    /// Start the message loop. Blocks until quit. Called by the AIOS
    /// runtime after process creation and capability grant.
    fn run(&mut self) -> Result<(), AppError>;
    /// Request graceful shutdown via the message loop.
    fn quit(&mut self) -> Result<(), AppError>;
    /// Immediate termination (skips delegate). System-only.
    fn force_quit(&mut self) -> Result<(), AppError>;
    /// Register a handler for a specific message type.
    fn add_handler<H: MessageHandler + 'static>(
        &mut self, message_type: MessageType, handler: H,
    ) -> Result<(), AppError>;
    /// The IPC channel for system lifecycle messages.
    fn system_channel(&self) -> ChannelId;
}
```

### 2.2 AppDelegate

Lifecycle callbacks invoked by the message loop at each state transition. Every application provides a delegate to respond to system events.

```rust
/// Lifecycle callbacks for application state transitions.
pub trait AppDelegate: Send {
    /// Called after the message loop starts and capabilities are granted.
    fn launched(&mut self, ctx: &LaunchContext) -> Result<(), AppError>;
    /// Return Ok(true) to permit quit, Ok(false) to defer (unsaved changes).
    fn will_quit(&mut self) -> Result<bool, AppError>;
    /// Called after will_quit returns true, before process teardown.
    fn quit_cleanup(&mut self) -> Result<(), AppError> { Ok(()) }
    /// Called on background transition, system sleep, or memory pressure.
    fn suspend(&mut self, reason: SuspendReason) -> Result<(), AppError>;
    /// Called when the app returns to foreground or system resumes.
    fn resume(&mut self, ctx: &ResumeContext) -> Result<(), AppError>;
    /// Release caches under memory pressure. Default: no-op.
    fn memory_pressure(&mut self, level: MemoryPressureLevel) -> Result<(), AppError> { Ok(()) }
    /// A capability was revoked -- stop using it immediately.
    fn capability_revoked(&mut self, handle: CapabilityHandle) -> Result<(), AppError> { Ok(()) }
}

pub enum SuspendReason {
    BackgroundTransition,  // User switched to another app
    SystemSleep,           // Device entering sleep state
    MemoryPressure,        // System needs memory back
    DeviceLock,            // User locked the device
}

pub struct ResumeContext {
    pub suspended_duration_ms: u64,
    pub system_changes: SystemChangeFlags,
}
```

### 2.3 MessageLoop

The event dispatch loop that receives IPC messages and routes them to registered handlers. App Kit's `MessageLoop` wraps [IPC Kit](../kernel/ipc.md)'s `Channel` and `IpcSelect` into a higher-level dispatch model.

```rust
/// Application-level message with typed routing.
pub struct AppMessage {
    pub message_type: MessageType,   // dispatch routing key
    pub source: ChannelId,           // who sent this
    pub payload: MessagePayload,     // deserialized from IPC RawMessage
    pub expects_reply: bool,
}

/// System messages 0..1024; application messages start at 1024.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MessageType(pub u32);

impl MessageType {
    pub const QUIT_REQUESTED: Self = Self(1);
    pub const SUSPEND: Self = Self(2);
    pub const RESUME: Self = Self(3);
    pub const MEMORY_PRESSURE: Self = Self(4);
    pub const CAPABILITY_REVOKED: Self = Self(5);
    pub const SCRIPT_REQUEST: Self = Self(6);
    pub const CONTENT_OPENED: Self = Self(7);
    pub const APP_BASE: Self = Self(1024); // first app-defined type
}

/// Handler for a specific message type.
pub trait MessageHandler: Send {
    fn handle(&mut self, msg: &AppMessage) -> Result<Option<MessagePayload>, AppError>;
}

/// Drives the application event loop. Uses IPC Kit's `ipc_select` internally.
pub trait MessageLoop {
    /// Enter the dispatch loop. Blocks until quit.
    fn run_loop(&mut self) -> Result<(), AppError>;
    /// Dispatch a single message (useful for testing and scripting).
    fn dispatch_one(&mut self, msg: AppMessage) -> Result<Option<MessagePayload>, AppError>;
    /// Post a message to the app's own queue (deferred processing).
    fn post_message(&self, msg: AppMessage) -> Result<(), AppError>;
    /// Register/unregister an IPC channel to listen on.
    fn watch_channel(&mut self, channel: ChannelId) -> Result<(), AppError>;
    fn unwatch_channel(&mut self, channel: ChannelId) -> Result<(), AppError>;
    /// Set a periodic timer that posts a message at the given interval.
    fn set_timer(&mut self, interval: Duration, msg_type: MessageType) -> Result<TimerHandle, AppError>;
    fn cancel_timer(&mut self, handle: TimerHandle) -> Result<(), AppError>;
}
```

### 2.4 Scriptable

The trait that makes every AIOS application introspectable and controllable by AIRS agents, other applications, and command-line tools. Inspired by BeOS's `BHandler` scripting suites with the `hey` protocol. See [BeOS Lesson 1: Scriptable Agent Protocol](../../knowledge/discussions/2026-03-23-jl-beos-haiku-redox-lessons.md).

The `Scriptable` trait is **mandatory** for all native AIOS applications. A default derive macro provides basic lifecycle properties automatically; applications extend with domain-specific suites.

```rust
use aios_capability::CapabilityHandle;
use aios_app::{
    AppError, CapabilityKind, PropertyInfo, PropertyValue,
    ScriptVerb, Specifier, Suite, ValueType,
};

/// Mandatory for all native AIOS applications. The base derive macro provides
/// "aios:app:lifecycle" (Name, State, Version, Capabilities) automatically.
pub trait Scriptable {
    /// Return all suites this object publishes.
    fn suites(&self) -> &[Suite];

    /// Execute a scripting request. The runtime checks capabilities at each
    /// specifier traversal step before calling this method.
    fn script(
        &mut self,
        verb: ScriptVerb,
        property: &str,
        specifier: &[Specifier],
        value: Option<PropertyValue>,
    ) -> Result<PropertyValue, AppError>;

    /// Resolve a child scriptable object for hierarchical traversal.
    /// e.g., `GET Title of Window 0` resolves Window 0, then reads Title.
    fn resolve_specifier(
        &self, property: &str, specifier: &Specifier,
    ) -> Result<&dyn Scriptable, AppError> {
        Err(AppError::PropertyNotFound)
    }
}
```

### 2.5 AgentManifest

Every AIOS application ships with a TOML-format manifest embedded in the application's Space (see [BeOS Lesson 3: Package-as-Filesystem](../../knowledge/discussions/2026-03-23-jl-beos-haiku-redox-lessons.md)). The manifest declares the application's identity, capabilities, content types, and scripting suites.

```toml
# agent.manifest.toml -- embedded in the agent's Space at /manifest.toml
[agent]
id = "com.example.text-editor"
name = "Text Editor"
version = "1.2.0"
developer = "did:peer:z6Mkq..."
min_aios_version = "0.1.0"

[capabilities.required]
storage_read = { spaces = ["user/home"] }
storage_write = { spaces = ["user/home"] }
ipc_channels = 4

[capabilities.optional]
network_access = { reason = "Cloud sync and collaborative editing" }
clipboard = { reason = "Copy/paste between applications" }

[content_types]
preferred = ["text/plain", "text/markdown", "text/x-rust"]
supported = ["text/html", "text/csv", "application/json"]
supertype = "text/*"

[scripting]
suites = ["com.example:editor:document", "com.example:editor:selection"]

[ui]
needs_window = true
min_window_size = { width = 400, height = 300 }
supports_multiwindow = true
accessibility_level = "full"

[lifecycle]
supports_suspend = true
persist_state_on_suspend = true
background_allowed = true
max_background_duration_sec = 300
```

The Rust representation mirrors the TOML structure. See [Agent Identity](../../experience/identity/agents.md) for the full `SignedManifest` type including developer signatures and supply chain verification.

```rust
/// Parsed agent manifest. Verified and signed at install time.
pub struct AgentManifest {
    pub agent_id: AppId,
    pub name: String,
    pub version: Version,
    pub developer_did: String,
    pub min_aios_version: Version,
    pub required_capabilities: Vec<DeclaredCapability>,
    pub optional_capabilities: Vec<DeclaredCapability>,
    pub content_types: ContentTypeDeclaration,
    pub scripting_suites: Vec<String>,
    pub ui_requirements: UiRequirements,
    pub lifecycle: LifecycleConfig,
}
```

### 2.6 ContentTypeHandler

Responds to content-type-based launch requests. When the system resolves a Space object to a handler agent via the content type registry, the handler receives the object reference. See [BeOS Lesson 4](../../knowledge/discussions/2026-03-23-jl-beos-haiku-redox-lessons.md).

```rust
pub trait ContentTypeHandler {
    /// A Space object was opened with this agent as the handler.
    fn open_content(&mut self, object: ObjectRef, content_type: &ContentType) -> Result<(), AppError>;
    /// Can this agent accept the given content type via Flow Kit drag-and-drop?
    fn can_accept_content(&self, content_type: &ContentType) -> Result<bool, AppError>;
    /// Accept dropped content from Flow Kit.
    fn accept_content(&mut self, entry: FlowEntry, content_type: &ContentType) -> Result<(), AppError>;
}
```

---

## 3. Usage Patterns

### 3.1 Minimal: Headless Agent

A headless agent with no UI -- just a message loop processing IPC messages and exposing a scriptable interface. Suitable for background services, data processors, and automation agents.

```rust
use aios_app::*;

struct DataProcessor { records_processed: u64 }

impl AppDelegate for DataProcessor {
    fn launched(&mut self, _ctx: &LaunchContext) -> Result<(), AppError> {
        log::info!("DataProcessor launched, ready for work");
        Ok(())
    }
    fn will_quit(&mut self) -> Result<bool, AppError> { Ok(true) }
    fn suspend(&mut self, _: SuspendReason) -> Result<(), AppError> { Ok(()) }
    fn resume(&mut self, _: &ResumeContext) -> Result<(), AppError> { Ok(()) }
}

// Scriptable is mandatory. Even headless agents expose the base lifecycle
// suite plus domain-specific properties.
impl Scriptable for DataProcessor {
    fn suites(&self) -> &[Suite] { &[LIFECYCLE_SUITE, PROCESSOR_SUITE] }

    fn script(
        &mut self, verb: ScriptVerb, property: &str,
        _specifier: &[Specifier], _value: Option<PropertyValue>,
    ) -> Result<PropertyValue, AppError> {
        match (verb, property) {
            (ScriptVerb::Get, "RecordsProcessed") =>
                Ok(PropertyValue::U64(self.records_processed)),
            (ScriptVerb::Execute, "Reset") => {
                self.records_processed = 0;
                Ok(PropertyValue::Void)
            }
            _ => Err(AppError::VerbNotSupported),
        }
    }
}

fn main() -> Result<(), AppError> {
    AiosApp::new(DataProcessor { records_processed: 0 }).run()
}
```

### 3.2 Realistic: GUI App with Lifecycle and Scripting

A text editor with windows, state persistence on suspend, and a scriptable interface.

```rust
use aios_app::*;
use aios_interface::{Window, View};
use aios_storage::{SpaceRef, ObjectRef};

struct TextEditor {
    windows: Vec<EditorWindow>,
    state_space: SpaceRef,
    unsaved_changes: bool,
}

impl AppDelegate for TextEditor {
    fn launched(&mut self, ctx: &LaunchContext) -> Result<(), AppError> {
        if ctx.lifecycle.persist_state_on_suspend {
            self.restore_session(&ctx.state_space)?;
        }
        if let Some(object) = ctx.open_object {
            self.open_document(object)?;
        }
        Ok(())
    }

    fn will_quit(&mut self) -> Result<bool, AppError> {
        if self.unsaved_changes {
            self.show_save_dialog()?;
            return Ok(false); // Defer quit until user decides.
        }
        Ok(true)
    }

    fn suspend(&mut self, _reason: SuspendReason) -> Result<(), AppError> {
        self.persist_session()?;
        for window in &mut self.windows { window.release_surface(); }
        Ok(())
    }

    fn resume(&mut self, ctx: &ResumeContext) -> Result<(), AppError> {
        for window in &mut self.windows { window.acquire_surface()?; }
        if ctx.system_changes.contains(SystemChangeFlags::STORAGE) {
            self.check_external_modifications()?;
        }
        Ok(())
    }
}

impl Scriptable for TextEditor {
    fn suites(&self) -> &[Suite] {
        &[LIFECYCLE_SUITE, DOCUMENT_SUITE, SELECTION_SUITE]
    }

    fn script(
        &mut self, verb: ScriptVerb, property: &str,
        _specifier: &[Specifier], _value: Option<PropertyValue>,
    ) -> Result<PropertyValue, AppError> {
        match (verb, property) {
            (ScriptVerb::Get, "Document") => { /* return doc info */ }
            (ScriptVerb::Set, "Selection") => { /* update selection */ }
            (ScriptVerb::Execute, "Save") => { /* save the document */ }
            (ScriptVerb::Count, "Document") =>
                Ok(PropertyValue::U64(self.windows.len() as u64)),
            _ => Err(AppError::VerbNotSupported),
        }
    }

    fn resolve_specifier(
        &self, property: &str, specifier: &Specifier,
    ) -> Result<&dyn Scriptable, AppError> {
        match (property, specifier) {
            ("Window", Specifier::Index(i)) => self.windows.get(*i)
                .map(|w| w as &dyn Scriptable)
                .ok_or(AppError::IndexOutOfRange),
            _ => Err(AppError::PropertyNotFound),
        }
    }
}
```

### 3.3 Advanced: Multi-Window App with Content Type Registration

An image viewer that registers as a handler for multiple image content types, supports multi-window, and integrates with [Flow Kit](../intelligence/flow.md) for drag-and-drop.

```rust
use aios_app::*;
use aios_storage::ObjectRef;

struct ImageViewer {
    windows: Vec<ViewerWindow>,
}

impl ContentTypeHandler for ImageViewer {
    fn open_content(
        &mut self,
        object: ObjectRef,
        content_type: &ContentType,
    ) -> Result<(), AppError> {
        // Open a new window for the image.
        let window = ViewerWindow::new(object, content_type)?;
        self.windows.push(window);
        Ok(())
    }

    fn can_accept_content(&self, content_type: &ContentType) -> Result<bool, AppError> {
        // Accept any image/* content via Flow Kit drag-and-drop.
        Ok(content_type.matches_supertype("image/*"))
    }

    fn accept_content(
        &mut self,
        entry: FlowEntry,
        content_type: &ContentType,
    ) -> Result<(), AppError> {
        // Convert the Flow entry to a Space object and open it.
        let object = entry.into_object()?;
        self.open_content(object, content_type)
    }
}
```

The corresponding manifest declares the content types:

```toml
[content_types]
preferred = ["image/png", "image/jpeg", "image/webp"]
supported = ["image/gif", "image/svg+xml", "image/bmp", "image/tiff"]
supertype = "image/*"
```

### 3.4 Common Mistakes

**Blocking the message loop.** Long-running work must be dispatched to background threads via [IPC Kit](../kernel/ipc.md) channels. The message loop thread processes lifecycle events and scripting requests -- blocking it freezes the entire application and causes the system to report the app as unresponsive.

```rust
// WRONG: blocks the message loop for 30 seconds
fn handle(&mut self, msg: &AppMessage) -> Result<Option<MessagePayload>, AppError> {
    let result = self.process_gigabyte_file(); // freezes the app
    Ok(Some(result.into()))
}

// RIGHT: dispatch to background, reply via IPC
fn handle(&mut self, msg: &AppMessage) -> Result<Option<MessagePayload>, AppError> {
    self.background_thread.post(WorkRequest {
        data: msg.payload.clone(),
        reply_to: msg.source,
    });
    Ok(None) // Background thread sends reply asynchronously.
}
```

**Ignoring `capability_revoked`.** When the system revokes a capability (e.g., the user withdrew storage access in Settings), the app must stop using that resource immediately. Continuing to use a revoked capability triggers an `AppError::CapabilityDenied` on the next access and may result in the app being terminated by the Behavioral Monitor.

**Not implementing Scriptable beyond the default.** The derive macro provides basic lifecycle properties, but apps that do not expose domain-specific properties are invisible to AIRS for composition. An image editor that only exposes `Name` and `State` cannot be scripted into a workflow like "resize all images in this Space to 800px wide."

**Holding locks across `suspend`.** The `suspend` callback must release shared resources (GPU surfaces, IPC channels with timeout-sensitive peers, held mutexes on shared memory regions) before returning. The system may freeze the process after `suspend` returns.

---

## 4. Integration Examples

### 4.1 App Kit + IPC Kit (Message Channels)

App Kit's `MessageLoop::run_loop()` calls `ipc_select` on the system channel plus application-registered channels, deserializes `RawMessage` payloads into typed `AppMessage` values, and dispatches to registered handlers.

```rust
fn setup_worker_channel(&mut self, app: &mut dyn Application) -> Result<(), AppError> {
    let (my_end, worker_end) = aios_ipc::channel_create()?;
    app.watch_channel(my_end)?;
    aios_ipc::spawn_thread(move || { worker_loop(worker_end); });
    app.add_handler(MessageType::APP_BASE, WorkerResultHandler::new(my_end))
}
```

### 4.2 App Kit + Capability Kit (Capability Request at Launch)

At launch, the system grants the intersection of manifest-requested capabilities and user trust policy. Required capabilities are always present (launch fails otherwise); optional capabilities may be absent.

```rust
impl AppDelegate for SecureNotes {
    fn launched(&mut self, ctx: &LaunchContext) -> Result<(), AppError> {
        if ctx.capabilities.has(Capability::NetworkAccess) {
            self.enable_cloud_sync();
        } else {
            self.show_offline_banner();
        }
        let storage = ctx.capabilities.get(Capability::StorageWrite)
            .expect("required capability always granted");
        self.notes_space = storage.open_space("user/notes")?;
        Ok(())
    }
}
```

### 4.3 App Kit + Interface Kit (GUI App with Delegate Callbacks)

[Interface Kit](interface.md) provides the UI toolkit. App Kit manages the lifecycle; Interface Kit manages the visual hierarchy. The delegate's `launched()` callback creates windows and builds the view hierarchy; `suspend()` and `resume()` coordinate surface lifecycle with the compositor.

```rust
impl AppDelegate for Calculator {
    fn launched(&mut self, _ctx: &LaunchContext) -> Result<(), AppError> {
        let config = WindowConfig { title: "Calculator", size: (320, 480), resizable: false };
        self.window = Some(Window::create(config)?);
        let root = self.window.as_mut().unwrap().root_view();
        root.add_child(self.build_display())?;
        root.add_child(self.build_keypad())?;
        self.window.as_mut().unwrap().show()
    }
}
```

### 4.4 App Kit + Storage Kit (Package-as-Space, State Persistence)

Agent packages are Spaces, not opaque archives. The agent's code, manifest, and assets are browsable via [Storage Kit](../platform/storage.md) queries. Mutable state is stored in a separate data Space.

```rust
impl TextEditor {
    fn persist_session(&self) -> Result<(), AppError> {
        let data = self.state_space.open("session")?;
        let docs: Vec<String> = self.windows.iter()
            .map(|w| w.document_path().to_string()).collect();
        data.write_object("open_documents", &docs)?;
        for w in &self.windows {
            data.write_object(&format!("state/{}", w.document_id()), &w.editor_state())?;
        }
        Ok(())
    }

    fn restore_session(&mut self, state_space: &SpaceRef) -> Result<(), AppError> {
        let data = state_space.open("session")?;
        if let Ok(docs) = data.read_object::<Vec<String>>("open_documents") {
            for path in docs {
                if let Ok(obj) = data.resolve(&path) { self.open_document(obj)?; }
            }
        }
        Ok(())
    }
}
```

The package Space is read-only (code, assets, manifest). The data Space is read-write (user data, settings, session state). On uninstall, the data Space can be preserved or deleted per user choice.

---

## 5. Capability Requirements

Every App Kit operation that interacts with system resources is gated by [Capability Kit](../kernel/capability.md). The following table documents which capabilities are required for each significant method.

| Method / Operation | Capability Required | Default Grant | Notes |
| --- | --- | --- | --- |
| `Application::run()` | `AppLaunch` | Granted at process creation | System always grants this |
| `Application::quit()` | None | -- | App can always quit itself |
| `Application::force_quit()` | `ProcessControl` | System-only | Only the Service Manager calls this |
| `MessageLoop::watch_channel()` | `ChannelAccess` | Per-channel | Must hold the channel handle |
| `Scriptable::script(Get, ...)` | Per-property | Varies | `PropertyInfo.capability` checked per traversal step |
| `Scriptable::script(Set, ...)` | Per-property | Varies | Write requires at least the read capability |
| `Scriptable::script(Execute, ...)` | Per-property | Varies | Action-specific capability |
| `Scriptable::script(Subscribe, ...)` | `ScriptSubscription` | Not granted by default | Must be requested in manifest |
| `ContentTypeHandler::open_content()` | `StorageRead` | Required capability | Declared in manifest |
| `ContentTypeHandler::accept_content()` | `FlowAccess` | Granted with Flow Kit | [Flow Kit](../intelligence/flow.md) provides the entry |
| `AppDelegate::launched()` | All required capabilities | From manifest | Launch fails if not grantable |
| State persistence (Space writes) | `StorageWrite` | Required capability | To agent's data Space only |
| Timer creation | None | -- | Timers are internal to the message loop |
| Background execution | `BackgroundExecution` | Optional capability | Without this, app is suspended when backgrounded |

**Capability attenuation for scripting.** Hierarchical paths like `GET Password of Account "admin" of Agent "identity"` check capabilities at each specifier step: (1) `ChannelAccess` to the agent, (2) `PropertyAccess(Account)` to enumerate, (3) `PropertyAccess(Account.Password)` to read. Each step attenuates -- derived capabilities are always a subset of the parent. See [Capability Kit](../kernel/capability.md).

---

## 6. Error Handling & Degradation

### 6.1 AppError Enum

```rust
#[derive(Debug)]
pub enum AppError {
    // Lifecycle
    LaunchFailed(String),
    QuitRefused,
    SuspendFailed(String),
    ResumeFailed(String),

    // Capabilities
    CapabilityDenied(CapabilityKind),
    CapabilityRevoked(CapabilityHandle),

    // Scripting
    PropertyNotFound,
    VerbNotSupported,
    SpecifierNotSupported,
    IndexOutOfRange,
    TypeMismatch { expected: ValueType, got: ValueType },
    SubscriptionLimitReached,

    // Message loop
    IpcError(aios_ipc::IpcError),
    HandlerPanicked(String),
    TimerLimitReached,

    // Content types
    ContentTypeNotSupported(ContentType),
    FlowConversionFailed(String),

    // Storage
    StatePersistFailed(String),
    StateRestoreFailed(String),
}
```

### 6.2 Degradation Behavior

App Kit degrades gracefully when capabilities are denied or resources are constrained:

- **Optional capabilities denied at launch.** The delegate checks `LaunchContext.capabilities` and disables features that depend on missing capabilities. The app continues with reduced functionality.
- **Capability revoked at runtime.** The system delivers `CAPABILITY_REVOKED` through the message loop. The app must stop using the revoked resource. Continued access returns `CapabilityDenied`; repeated violations trigger termination by the [Behavioral Monitor](../../intelligence/behavioral-monitor.md).
- **Suspend failure.** The system force-suspends the process. On resume, the app should handle inconsistent state gracefully.
- **Critical memory pressure.** The system calls `memory_pressure(Critical)`. Unresponsive apps are terminated (lowest-priority first).
- **IPC errors.** Transient errors are retried internally. Fatal errors cause `run_loop()` to return `AppError::IpcError`, initiating shutdown.
- **Scripting with denied capability.** The runtime returns `CapabilityDenied` to the caller before `script()` is invoked. The app never sees the request.

---

## 7. Platform & AI Availability

### 7.1 Always Available (No AIRS Dependency)

The following App Kit features function identically whether or not AIRS is running. These are pure application-framework primitives with no intelligence dependency:

| Feature | Availability | Notes |
| --- | --- | --- |
| `Application` lifecycle (launch, quit, suspend, resume) | Always | Core framework -- no AI needed |
| `AppDelegate` callbacks | Always | Pure event-driven callbacks |
| `MessageLoop` dispatch | Always | Wraps IPC Kit, which is a kernel primitive |
| `Scriptable` trait (all verbs) | Always | Protocol-level introspection -- any agent or CLI tool can script any app, regardless of AIRS availability |
| `AgentManifest` parsing and verification | Always | Static TOML parsing + signature verification |
| `ContentTypeHandler` dispatch | Always | Content type registry is in the Service Manager, not AIRS |
| Capability checking and attenuation | Always | Kernel-level enforcement via [Capability Kit](../kernel/capability.md) |
| State persistence to Spaces | Always | Direct [Storage Kit](../platform/storage.md) operations |
| Timer management | Always | Backed by the kernel timer subsystem |

The `Scriptable` trait deserves emphasis: it is deliberately AIRS-independent. A command-line tool (analogous to BeOS's `hey`) can enumerate, query, and control any running application through the scripting protocol. Automation scripts, accessibility tools, and testing harnesses all use the same protocol. AIRS is simply the most sophisticated consumer of `Scriptable`, not the only one.

### 7.2 AIRS-Enhanced (When Intelligence is Available)

When AIRS is running, App Kit gains several intelligent behaviors that enhance the developer and user experience without changing the API surface:

| Enhancement | AIRS Service | Behavior |
| --- | --- | --- |
| Smart capability suggestions | Agent Capability Intelligence | At install time, AIRS analyzes the manifest and suggests whether optional capabilities should be granted based on the user's trust level and usage patterns |
| Predictive preloading | Context Engine + Runtime Advisor | AIRS predicts which app the user will launch next (based on time-of-day, activity context) and pre-warms its process, pre-loads its Space data, and pre-allocates its capabilities |
| Intelligent handler selection | Preference Service | When multiple agents can handle a content type, AIRS learns user preferences ("Justin always opens .rs files with the code editor") and adjusts the preferred handler without explicit configuration |
| Workflow composition | Tool Manager + Scriptable | AIRS discovers capabilities of running apps via `Describe` verb, composes multi-app workflows ("resize images in this Space and convert to WebP"), and executes them via `Script` requests through the scripting protocol |
| Anomaly detection | Behavioral Monitor | AIRS monitors app behavior patterns (IPC volume, memory usage, scripting requests) and flags anomalies that might indicate compromised agents |
| Suspend optimization | Context Engine | AIRS predicts when an app will be needed again and adjusts the aggressiveness of state persistence vs. fast-resume tradeoffs |

### 7.3 Graceful AIRS Degradation

When AIRS is unavailable, all enhanced features degrade to deterministic fallbacks: capability suggestions use the manifest's declared reason strings, handler selection uses the static preferred/supported/supertype chain, anomaly detection falls back to the Behavioral Monitor's Welford/z-score baseline, and predictive preloading is simply disabled. Workflow composition via the scripting protocol remains available to users and scripts -- only AIRS-driven auto-composition is lost.

No App Kit API changes behavior based on AIRS availability. The API contract is identical; only the quality of system-level decisions around the app changes.

---

## Related Documents

- [IPC Kit](../kernel/ipc.md) -- message channels and reply semantics that App Kit wraps
- [Capability Kit](../kernel/capability.md) -- capability model gating all App Kit operations
- [Memory Kit](../kernel/memory.md) -- application heap and address space
- [Interface Kit](interface.md) -- UI toolkit that builds on App Kit lifecycle
- [Flow Kit](../intelligence/flow.md) -- content transfer between applications
- [Storage Kit](../platform/storage.md) -- Space-based storage for agent packages and state
- [Identity Kit](identity.md) -- agent manifest signing and developer identity
- [Agent Identity](../../experience/identity/agents.md) -- manifest verification and supply chain
- [Agents Architecture](../../applications/agents.md) -- agent lifecycle and process model
- [ADR: App Kit](../../knowledge/decisions/2026-03-22-jl-app-kit.md) -- decision record for App Kit creation
- [BeOS/Haiku Lessons](../../knowledge/discussions/2026-03-23-jl-beos-haiku-redox-lessons.md) -- design lessons informing Scriptable, Package-as-Space, and Content Type Registry

## Implementation Phase

Phase 13+ (agent lifecycle, scripting protocol, message loop). Full content type registry integration Phase 14+.
