# Browser Kit

**Layer:** Application | **Crate:** `aios_browser` | **Architecture:** [`docs/applications/browser.md`](../../applications/browser.md)

## 1. Overview

Browser Kit provides the SDK surface through which web browsers integrate with AIOS subsystems.
Rather than building a custom browser engine, AIOS exposes GPU rendering, networking, media
decode, input dispatch, and storage as platform hooks that existing engines (Chromium, WebKit
via Servo, Gecko) consume directly. The result is that any browser running on AIOS automatically
inherits capability-gated security, per-tab agent isolation, and AI-native content assistance
without engine-level modifications.

The central design insight is that each browser tab maps to a separate AIOS agent. A tab's
agent receives its own capability set, address space, and resource budget. This means that
one tab cannot access another tab's network connections, storage, or sensor grants unless
the user explicitly delegates that capability. The browser chrome (address bar, bookmarks,
tab strip) runs as a privileged orchestrator agent that spawns and manages per-tab agents,
mediating capability delegation on the user's behalf.

Browser Kit is intentionally thin. It defines bridge traits that translate between browser
engine abstractions (content processes, compositor layers, network channels) and the
corresponding AIOS Kit APIs. A browser vendor ports to AIOS by implementing these bridge
traits rather than rewriting their rendering pipeline. WebGPU surfaces map to Compute Kit,
fetch requests map to Network Kit, `<video>` elements map to Media Kit, and so on.

## 2. Core Traits

```rust
use aios_capability::{Capability, CapabilityHandle};
use aios_compute::Surface;
use aios_network::{Connection, TlsConfig};
use aios_media::MediaSession;
use aios_input::InputEvent;
use aios_storage::Space;

/// Bridge between a browser engine and AIOS subsystems.
///
/// Implemented once per engine (e.g., `ChromiumEngine`, `ServoEngine`).
/// The browser shell calls these methods; the Kit routes them to the
/// appropriate platform services.
pub trait BrowserEngine {
    /// Initialize the engine, returning a handle for tab creation.
    fn init(&mut self, config: EngineConfig) -> Result<EngineHandle, BrowserError>;

    /// Shut down the engine, tearing down all tabs and releasing resources.
    fn shutdown(&mut self) -> Result<(), BrowserError>;

    /// Query engine capabilities (supported web standards, codec list).
    fn capabilities(&self) -> EngineCapabilities;
}

/// A single browser tab backed by its own AIOS agent.
///
/// Each tab receives an isolated capability set. Navigation, script
/// execution, and resource loading are all gated by the tab's grants.
pub trait Tab {
    /// Navigate to a URL. Returns once the navigation commit occurs.
    /// Requires `NetworkAccess` capability for the target origin.
    fn navigate(&mut self, url: &Url) -> Result<NavigationId, BrowserError>;

    /// Stop the current navigation or page load.
    fn stop(&mut self);

    /// Reload the current page, optionally bypassing cache.
    fn reload(&mut self, bypass_cache: bool) -> Result<(), BrowserError>;

    /// Execute JavaScript in the tab's isolated world.
    /// Requires `ScriptExecution` capability.
    fn execute_script(&self, script: &str) -> Result<ScriptResult, BrowserError>;

    /// Retrieve the tab's current security state (TLS info, mixed content).
    fn security_state(&self) -> SecurityState;

    /// Return the capability set currently granted to this tab.
    fn granted_capabilities(&self) -> &[CapabilityHandle];

    /// Request an additional capability (e.g., camera access for an origin).
    /// This triggers a user-facing permission prompt via Security Kit.
    fn request_capability(&mut self, cap: Capability) -> Result<CapabilityHandle, BrowserError>;
}

/// Compositor surface contract for browser content rendering.
///
/// Maps browser compositor layers to AIOS Compute Kit surfaces.
/// Supports both software-rasterized and GPU-accelerated content.
pub trait WebView {
    /// Attach the web view to a compositor surface for display.
    fn attach_surface(&mut self, surface: Surface) -> Result<(), BrowserError>;

    /// Resize the rendering viewport.
    fn resize(&mut self, width: u32, height: u32);

    /// Deliver an input event to the web view's hit-test pipeline.
    fn deliver_input(&mut self, event: InputEvent) -> InputResult;

    /// Request a frame, triggering the browser's rendering pipeline.
    fn request_frame(&mut self) -> Result<FrameToken, BrowserError>;

    /// Enable or disable hardware-accelerated compositing.
    fn set_gpu_compositing(&mut self, enabled: bool);
}

/// Content filtering applied to page loads and subresource requests.
///
/// Runs before network requests leave the tab agent, enforcing
/// origin-level restrictions and content policy.
pub trait ContentFilter {
    /// Evaluate whether a request should proceed, be blocked, or redirected.
    fn evaluate(&self, request: &ResourceRequest) -> FilterDecision;

    /// Register a filter rule (e.g., ad blocking, tracker prevention).
    fn add_rule(&mut self, rule: FilterRule) -> Result<RuleId, BrowserError>;

    /// Remove a previously registered rule.
    fn remove_rule(&mut self, id: RuleId) -> Result<(), BrowserError>;

    /// List all active filter rules with match statistics.
    fn list_rules(&self) -> &[FilterRuleEntry];
}

/// Connects the browser's network stack to AIOS Network Kit.
///
/// Each origin's connections are isolated to the tab agent's
/// capability set — cross-origin leaks are structurally impossible.
pub trait NetworkBridge {
    /// Open a connection to a remote host, inheriting the tab's TLS config.
    fn connect(&mut self, host: &str, port: u16, tls: TlsConfig)
        -> Result<Connection, BrowserError>;

    /// Create a WebSocket connection with capability-gated upgrade.
    fn websocket(&mut self, url: &Url) -> Result<WebSocketHandle, BrowserError>;

    /// Perform a DNS lookup through the system resolver.
    fn resolve(&self, hostname: &str) -> Result<Vec<IpAddr>, BrowserError>;
}

/// Routes HTML5 media elements through AIOS Media Kit.
pub trait MediaBridge {
    /// Create a media session for a `<video>` or `<audio>` element.
    fn create_session(&mut self, params: MediaParams) -> Result<MediaSession, BrowserError>;

    /// Query available codecs and DRM support.
    fn supported_codecs(&self) -> Vec<CodecInfo>;
}
```

## 3. Usage Patterns

**Minimal -- launch a tab and navigate:**

```rust
use aios_browser::{BrowserKit, EngineConfig, TabConfig};

let engine = BrowserKit::default_engine(EngineConfig::default())?;
let tab = engine.create_tab(TabConfig {
    initial_url: "https://example.com".parse()?,
    ..Default::default()
})?;

// Tab is now loading. The tab agent has NetworkAccess for example.com.
let state = tab.security_state();
assert!(state.is_secure());
```

**Realistic -- multi-tab browser with content filtering:**

```rust
use aios_browser::{BrowserKit, EngineConfig, TabConfig, ContentFilter};

let mut engine = BrowserKit::default_engine(EngineConfig {
    gpu_compositing: true,
    max_tabs: 32,
    ..Default::default()
})?;

// Install a tracker-blocking filter
let mut filter = engine.content_filter();
filter.add_rule(FilterRule::block_domain("tracker.example.net"))?;
filter.add_rule(FilterRule::block_pattern("*/ads/*"))?;

// Open tabs — each is an isolated agent
let tab_a = engine.create_tab(TabConfig::for_url("https://news.example.com")?)?;
let tab_b = engine.create_tab(TabConfig::for_url("https://mail.example.com")?)?;

// tab_b cannot read tab_a's cookies, network connections, or storage.
// This is enforced at the capability level, not just by cookie partitioning.

// Grant camera access to a video-call origin
tab_b.request_capability(Capability::CameraAccess {
    origin: "https://meet.example.com".into(),
})?;
```

**Advanced -- embedding a web view in a native AIOS application:**

```rust
use aios_browser::{WebView, WebViewConfig};
use aios_compute::Surface;
use aios_interface::View;

struct HelpPanel {
    web_view: Box<dyn WebView>,
}

impl HelpPanel {
    fn new(surface: Surface) -> Result<Self, BrowserError> {
        let mut web_view = BrowserKit::create_web_view(WebViewConfig {
            sandboxed: true,
            javascript_enabled: true,
            network_restricted_to: vec!["https://help.myapp.example".into()],
        })?;
        web_view.attach_surface(surface)?;
        web_view.navigate("https://help.myapp.example/docs")?;
        Ok(Self { web_view })
    }
}
```

> **Common Mistakes**
>
> - **Sharing capabilities across tabs.** Each tab is a separate agent. Do not pass
>   `CapabilityHandle` values between tab agents -- they will fail validation. Use
>   `Tab::request_capability()` per tab.
> - **Assuming direct GPU buffer access.** Browser content renders through the Compute Kit
>   surface abstraction. Do not attempt raw GPU memory mapping from the browser engine.
> - **Ignoring `FilterDecision::Redirect`.** Content filters can redirect requests, not just
>   block them. Always handle all three variants of `FilterDecision`.

## 4. Integration Examples

**Browser + Network Kit + Security Kit -- secure browsing with credential isolation:**

```rust
use aios_browser::Tab;
use aios_network::CredentialVault;
use aios_security::PermissionPrompt;

// Each tab's network connections are automatically routed through
// Network Kit's per-agent isolation. Credentials are fetched from
// the vault only for origins that match the tab's grants.

let tab = engine.create_tab(TabConfig::for_url("https://bank.example.com")?)?;

// When the page requests authentication, the browser delegates to
// Identity Kit via the NetworkBridge. The user sees a Security Kit
// permission prompt, not a raw HTTP basic auth dialog.
//
// Identity Kit resolves the credential; Network Kit injects it into
// the TLS session; the tab agent never sees the raw password.
```

**Browser + Media Kit + Audio Kit -- video playback:**

```rust
use aios_browser::MediaBridge;
use aios_media::MediaSession;

// HTML5 <video> elements are automatically routed through Media Kit.
// The browser engine calls MediaBridge::create_session(), which:
// 1. Selects the best codec (hardware-accelerated if available)
// 2. Creates a Media Kit session with the tab's capability set
// 3. Routes decoded frames to the WebView's compositor surface
// 4. Routes audio to Audio Kit with the tab's audio session

let codecs = engine.media_bridge().supported_codecs();
// Returns: [H264, VP9, AV1, AAC, Opus, ...]
// Hardware codecs are preferred when Compute Kit reports accelerator support.
```

**Browser + Storage Kit -- origin-partitioned persistence:**

```rust
use aios_storage::Space;

// Each origin gets a sub-Space within the tab agent's storage allocation.
// localStorage, IndexedDB, and Cache API all map to Space objects.
// This means browser storage inherits:
// - Versioning (every write is a Merkle DAG node)
// - Encryption at rest (per the Space's SecurityZone)
// - Quota enforcement (via Storage Kit budget system)

// The browser engine never touches raw disk I/O -- it goes through
// Storage Kit's object store, which provides crash consistency via WAL.
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `BrowserEngine::init` | `ProcessCreate` | Spawns the engine orchestrator agent |
| `Tab::navigate` | `NetworkAccess(origin)` | Per-origin; auto-requested on first navigation |
| `Tab::execute_script` | `ScriptExecution` | Granted by default for tab agents |
| `Tab::request_capability` | `CapabilityDelegate` | Triggers user permission prompt |
| `WebView::attach_surface` | `SurfaceCreate` | Via Compute Kit |
| `WebView::deliver_input` | `InputReceive` | Routed by compositor focus |
| `NetworkBridge::connect` | `NetworkAccess(host)` | Per-host, per-tab |
| `NetworkBridge::websocket` | `NetworkAccess(host)` + `WebSocketUpgrade` | Elevated for persistent connections |
| `MediaBridge::create_session` | `MediaDecode` | Codec access; DRM requires `DrmAccess` |
| `ContentFilter::add_rule` | `ContentFilterManage` | Chrome agent only, not per-tab |

## 6. Error Handling & Degradation

```rust
/// Errors returned by Browser Kit operations.
#[derive(Debug)]
pub enum BrowserError {
    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// Navigation failed (DNS, TLS, HTTP error).
    NavigationFailed { url: Url, reason: NavigationFailure },

    /// The tab agent crashed or was killed by the behavioral monitor.
    TabCrashed(TabId),

    /// GPU compositing is unavailable; software fallback engaged.
    GpuUnavailable,

    /// Content filter rejected the request.
    Blocked(FilterDecision),

    /// The media codec or DRM module is not available on this device.
    CodecUnavailable(CodecId),

    /// Resource limit exceeded (too many tabs, connections, etc.).
    ResourceExhausted(ResourceKind),

    /// Engine-internal error with diagnostic context.
    Internal(String),
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| GPU compositing unavailable | Automatic fallback to software rasterization via Compute Kit |
| Hardware codec missing | Software decode via Media Kit's fallback path |
| Network capability denied | Page load blocked; user sees a capability-request interstitial |
| Tab agent crash | Chrome agent respawns the tab with a "page crashed" message |
| DRM module absent | Non-DRM content plays normally; DRM content shows a clear error |
| Storage quota exhausted | Write operations fail; existing data remains accessible |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Smart content filtering | ML-based tracker detection, adaptive ad blocking | Static rule-based filtering only |
| Page summarization | One-sentence summaries in tab strip via Conversation Kit | No summaries; standard page titles |
| Phishing detection | Real-time page analysis against behavioral patterns | Static blocklist matching |
| Accessibility auto-describe | AI-generated alt text for images missing descriptions | Images show filename or blank alt |
| Translation | On-device neural translation of page content | No translation; content shown as-is |
| Smart form fill | Context-aware field detection and credential suggestion | Basic autofill via stored credentials |

**Platform availability:**

| Platform | GPU Compositing | Hardware Codecs | DRM | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | VirtIO-GPU (limited) | Software only | No | Development and testing only |
| Raspberry Pi 4 | VC4/V3D | H.264 (V4L2) | No | VideoCore IV acceleration |
| Raspberry Pi 5 | VideoCore VII | H.264/HEVC | No | Improved GPU, dual display |
| Apple Silicon | AGX (via driver) | H.264/HEVC/VP9 | Possible | Neural Engine for AI features |

**Implementation phase:** Phase 30+. Browser Kit depends on mature Compute Kit (Phase 6+),
Network Kit (Phase 7+), Media Kit (Phase 10+), and Input Kit (Phase 8+) foundations.

---

*See also: [Compute Kit](../kernel/compute.md) | [Network Kit](../platform/network.md) | [Media Kit](../platform/media.md) | [Input Kit](../platform/input.md) | [Security Kit](security.md) | [Storage Kit](../platform/storage.md)*
