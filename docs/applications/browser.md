# AIOS Browser Architecture

## Decomposed Web Content Runtime

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [subsystem-framework.md](../platform/subsystem-framework.md), [networking.md](../platform/networking.md)

-----

## 1. Core Insight

Every browser today is a miniature operating system running inside your actual operating system. Chrome has its own process model, its own sandboxing, its own networking stack, its own storage layer, its own security policy, its own GPU abstraction. Chrome's source code is larger than most operating systems.

Browsers became mini-OSes because the actual OS underneath provided nothing useful for web security. The OS gives files and sockets. The browser needs origin isolation, content security policies, sandboxed execution. So the browser rebuilt everything from scratch, on top of an OS that actively gets in its way.

AIOS doesn't have this problem. AIOS already has capabilities, isolation, audited networking, spaces, and Flow. The browser doesn't need to rebuild all of that. It needs to use what the OS provides and focus on the one thing only a browser can do: **execute web content.**

-----

## 2. Responsibility Decomposition

What a traditional browser does, and where each responsibility lives in AIOS:

```
Traditional Browser                    AIOS Decomposition
──────────────────                    ──────────────────
Network stack (HTTP, TLS, DNS)     →  OS Network Services (mandatory for browser)
Connection pooling, caching        →  OS HTTP Service
Cookie storage                     →  Web Storage Space (OS-managed)
localStorage, IndexedDB            →  Web Storage Space (per-origin sub-spaces)
Process isolation per site         →  OS capability isolation (per-tab agent)
Sandboxed renderer                 →  OS agent sandbox (capabilities)
Same-origin policy                 →  OS capabilities mapped from origins
Content Security Policy            →  OS capability restrictions
GPU access (WebGL, WebGPU)         →  OS GPU capability (compositor-mediated)
Media playback                     →  OS media service (audio subsystem)
Permissions (camera, mic, location)→  OS capability prompts (subsystem framework)
Download management                →  Flow into user space
──────────────────────────────────────────────────────────
HTML/CSS parsing + layout          ←  STAYS in browser (web-specific)
JavaScript execution               ←  STAYS in browser (web-specific)
WASM execution                     ←  STAYS in browser (web-specific)
DOM manipulation                   ←  STAYS in browser (web-specific)
Web API surface                    ←  STAYS in browser (thin shims to OS)
```

The browser shrinks from a mini-OS to a **web content runtime** — a rendering engine and a language runtime, with thin shims that bridge Web APIs to OS services.

-----

## 3. Architecture: The Browser as a Constellation of Agents

Instead of one monolithic browser process, the AIOS browser is a set of cooperating agents:

```
┌─────────────────────────────────────────────────────────┐
│                    Browser Shell Agent                    │
│  Tab management, URL bar, bookmarks, history, settings   │
│  Built with portable UI toolkit (iced)                   │
│  Capabilities: compositor, space(bookmarks),             │
│                space(history), space(web-storage)         │
└──────────┬──────────────────────────────┬───────────────┘
           │                              │
    ┌──────▼──────┐                ┌──────▼──────┐
    │  Tab Agent   │                │  Tab Agent   │
    │  (site A)    │                │  (site B)    │
    │              │                │              │
    │ ┌──────────┐ │                │ ┌──────────┐ │
    │ │ Renderer │ │                │ │ Renderer │ │
    │ │ HTML/CSS │ │                │ │ HTML/CSS │ │
    │ │ layout   │ │                │ │ layout   │ │
    │ └──────────┘ │                │ └──────────┘ │
    │ ┌──────────┐ │                │ ┌──────────┐ │
    │ │ JS/WASM  │ │                │ │ JS/WASM  │ │
    │ │ Runtime  │ │                │ │ Runtime  │ │
    │ └──────────┘ │                │ └──────────┘ │
    │              │                │              │
    │ Capabilities:│                │ Capabilities:│
    │  net(site-a) │                │  net(site-b) │
    │  gpu(limited)│                │  gpu(limited)│
    │  storage(a)  │                │  storage(b)  │
    └──────────────┘                └──────────────┘
```

Each tab is a literal AIOS agent with its own capability set, its own memory isolation, its own entry in the audit log. The OS provides the isolation that Chrome spends millions of engineering hours reimplementing.

-----

## 4. Tab Agent Capabilities Derived From Origin

When you navigate to `https://weather.com`, the Browser Shell Agent spawns a Tab Agent with capabilities derived from the URL:

```rust
// Browser Shell spawns a tab agent for weather.com
let tab_caps = CapabilitySet {
    // Network: can ONLY reach weather.com and its CDN
    network: vec![
        NetworkCap::service("weather.com", HTTPS, GET | POST),
        NetworkCap::service("cdn.weather.com", HTTPS, GET),
        // Third-party scripts get sub-capabilities
        NetworkCap::service("ads.doubleclick.net", HTTPS, GET)
            .with_flag(UserCanBlock),  // user can revoke this one
    ],

    // Storage: isolated to this origin
    storage: SpaceCap::subspace("web-storage", "weather.com"),

    // GPU: limited WebGL/WebGPU access
    gpu: GpuCap::webgl(max_memory: Megabytes(256)),

    // No camera, no mic, no location — until user grants
    camera: None,
    microphone: None,
    geolocation: None,

    // Clipboard: read requires user gesture, write always allowed
    clipboard: ClipboardCap::write_only(),

    // Cannot access other tabs, other spaces, or system services
    // Cannot spawn child agents
    // Cannot access Flow channels (except through browser shell)
};

let tab = agent::spawn("web-tab", tab_caps)?;
```

**This is same-origin policy enforced by the kernel.** In Chrome, same-origin policy is enforced by the browser's own logic and has been bypassed by countless vulnerabilities (Spectre, Meltdown, renderer exploits). In AIOS, the Tab Agent for `weather.com` physically cannot read memory belonging to the Tab Agent for `bank.com`. It's not a browser policy — it's a hardware-enforced capability boundary.

-----

## 5. How JavaScript Sees the Network

JavaScript in a tab calls `fetch()`. The call flows through the subsystem framework:

```
Traditional browser:
  JS: fetch("https://api.weather.com/forecast")
    → Browser network stack resolves DNS
    → Browser opens TCP connection
    → Browser does TLS handshake
    → Browser sends HTTP request
    → Browser receives response
    → Browser checks CORS headers
    → Browser returns response to JS
  The browser does everything. The OS knows nothing.

AIOS browser:
  JS: fetch("https://api.weather.com/forecast")
    ↓
  Web API shim: translate to service channel request
    ↓
  Tab Agent: channel.request(GET, "api.weather.com", "/forecast")
    ↓
  OS Capability Gate: Tab agent has net(weather.com)? YES → allow
    ↓
  OS TLS Service: reuse pooled connection to api.weather.com
    ↓
  OS HTTP Service: send GET, receive response, cache if cacheable
    ↓
  OS Audit: log connection in network-audit space
    ↓
  Response flows back through the same path to JS
```

JavaScript doesn't know the difference. `fetch()` returns a `Response` object. The code works identically. But underneath:

- The OS enforced the capability (no CORS bypass possible, even with a renderer exploit)
- The OS managed TLS (no certificate spoofing possible)
- The OS logged the connection (auditable)
- The OS pooled the connection (efficient)
- The OS applied QoS based on intent

### 5.1 Cross-Origin Requests (CORS)

CORS maps naturally to capabilities:

```
Page: https://weather.com
JS calls: fetch("https://api.maps.google.com/tiles")
```

The Tab Agent's capabilities don't include `api.maps.google.com`. Two resolution paths:

**Static capability from page metadata.** When the page loads, the Browser Shell parses its `Content-Security-Policy` and `<link>` tags to identify expected third-party origins. These become sub-capabilities on the Tab Agent:

```rust
// Derived from CSP: connect-src 'self' api.maps.google.com
network: vec![
    NetworkCap::service("weather.com", ...),
    NetworkCap::service("api.maps.google.com", HTTPS, GET)
        .derived_from("weather.com/CSP")
        .restricted(read_only: true),
]
```

**Dynamic capability grant.** If the request isn't pre-declared, the Tab Agent asks the Browser Shell Agent for a cross-origin capability. The Browser Shell checks the CORS preflight response:

```
Tab Agent → Browser Shell: "I need to reach api.maps.google.com"
Browser Shell → OS: preflight OPTIONS request to api.maps.google.com
Server responds: Access-Control-Allow-Origin: https://weather.com
Browser Shell: CORS allows it → grant temporary NetworkCap to Tab Agent
Tab Agent: proceeds with the actual request
```

The capability grant is logged, auditable, and revocable. The user can see: "weather.com accessed api.maps.google.com 47 times today" and can revoke that cross-origin permission.

-----

## 6. Web Storage as Spaces

Traditional browsers have a mess of storage APIs: cookies, localStorage, sessionStorage, IndexedDB, Cache API, each with its own size limits, eviction policies, and security model.

In AIOS, all of these map to a single concept: **a sub-space within the web-storage space, scoped to the origin.**

```
web-storage/                          ← System space for all web data
  weather.com/                        ← Origin sub-space
    cookies/                          ← Cookie objects
      session_id: {value, expiry, httponly, secure, sameSite}
    local/                            ← localStorage key-value pairs
    indexed-db/                       ← IndexedDB databases
      forecast-cache/                 ← Individual database
        hourly/                       ← Object stores
    cache-api/                        ← Service worker caches
      v1/                             ← Named cache
        /forecast → {response, headers, timestamp}
    session/                          ← sessionStorage (ephemeral, cleared on tab close)
  bank.com/                           ← Completely isolated from weather.com
    ...
```

### 6.1 Why This Matters

**Unified quota management.** Instead of each API having its own size limit, the origin has a total space quota. The user sees: "weather.com is using 12MB of storage" — not "2MB in localStorage, 8MB in IndexedDB, 2MB in Cache API."

**Searchable.** Because it's a space, AIRS can search it: "What cookies do I have from tracking domains?" This is impossible in traditional browsers without browser-specific extensions.

**Backup and sync.** Web storage is a space. Spaces sync across devices through the Space Mesh Protocol. Browser state (bookmarks, saved passwords, site data) syncs transparently — no Google Account or Firefox Sync required.

**User control.** "Delete all data from weather.com" is deleting a sub-space. Clean, atomic, complete. No wondering whether you got everything.

**Privacy.** The user can inspect exactly what each site has stored, because it's just objects in a space. No hidden state.

-----

## 7. Service Workers as Persistent Tab Agents

In traditional browsers, a service worker is a JavaScript script that runs in the background, intercepts network requests, and can serve responses from cache.

In AIOS, a service worker is a **persistent Tab Agent** with constrained capabilities:

```rust
let sw_caps = CapabilitySet {
    // Same network caps as the origin
    network: origin_network_caps.clone(),

    // Access to cache-api sub-space
    storage: SpaceCap::subspace("web-storage", "weather.com/cache-api"),

    // Can intercept fetch requests from tabs of same origin
    intercept: InterceptCap::fetch_requests("weather.com"),

    // Background execution (survives tab close)
    lifecycle: Lifecycle::Persistent {
        wake_on: [PushEvent, FetchEvent, SyncEvent],
        idle_timeout: Minutes(5),
    },

    // NO GPU, NO compositor, NO user interaction
    gpu: None,
    compositor: None,
};
```

The service worker runs JavaScript (same runtime as tabs) but with different capabilities. It can intercept fetch requests from tabs of the same origin and serve cached responses. The OS manages its lifecycle — waking it on events, suspending it when idle.

This is cleaner than the traditional model because the service worker's persistence and background execution are explicit OS capabilities, not browser hacks.

-----

## 8. JavaScript/WASM Runtime

The runtime is the one part that doesn't decompose. JavaScript and WebAssembly must execute within the Tab Agent. But the execution environment is fundamentally different because of what surrounds it.

### 8.1 Engine Choice

**SpiderMonkey through Servo.** Servo is a Rust-based browser engine that embeds SpiderMonkey (Mozilla's JS engine). Rather than embedding a JS engine from scratch, AIOS uses Servo's rendering engine and JS runtime as the core of the Tab Agent, with networking and storage layers replaced by AIOS service bridges.

```
Servo Components Used                AIOS Replacement
──────────────────────────           ────────────────────────────────────────
SpiderMonkey (JS engine)          →  KEEP (no alternative)
style (CSS engine)                →  KEEP
layout (box/flex/grid)            →  KEEP
WebRender (GPU rendering)         →  ADAPT to AIOS compositor
                                     (WebRender already uses wgpu)
net (network stack)               →  REPLACE with OS service channels
storage (cookies, localStorage)   →  REPLACE with web-storage space
fetch (HTTP client)               →  REPLACE with OS HTTP service bridge
```

Servo is the right choice because it's modular by design. The layout engine, CSS engine, and JS runtime are separable from the networking and storage layers.

### 8.2 The Web API Bridge

JavaScript calls Web APIs (fetch, localStorage, WebGL, etc.). These need to reach OS services. The bridge is a set of thin shims:

```rust
// Inside Tab Agent — bridges JS Web APIs to OS services

// fetch() → OS service channel
fn web_fetch(request: JsRequest) -> JsFuture<JsResponse> {
    let url = request.url();
    let method = request.method();

    // OS handles: capability check, TLS, HTTP, caching, retry
    let channel = self.network_channel.request(method, url, request.body())?;

    // Convert OS response to Web API Response object
    JsResponse::from_os_response(channel.response().await?)
}

// localStorage.setItem() → space write
fn local_storage_set(key: &str, value: &str) {
    let path = format!("{}/local/{}", self.origin, key);
    self.storage_space.write_object(path, value)?;
}

// navigator.geolocation.getCurrentPosition() → capability request
fn geolocation_get(callback: JsCallback) {
    match self.capabilities.geolocation {
        Some(cap) => {
            let position = os::geolocation::get(cap)?;
            callback.invoke(position);
        }
        None => {
            // Request capability from user through Browser Shell
            let granted = self.shell.request_permission(
                Permission::Geolocation,
                self.origin,
            ).await?;
            if granted {
                self.capabilities.geolocation = Some(granted);
                // retry
            } else {
                callback.error(PermissionDenied);
            }
        }
    }
}

// WebGL → OS GPU capability (display subsystem)
fn create_webgl_context(canvas: JsCanvas) -> JsWebGLContext {
    let gpu_surface = self.compositor.create_surface(
        canvas.width(), canvas.height(),
        self.capabilities.gpu,
    )?;
    JsWebGLContext::from_surface(gpu_surface)
}
```

Each Web API call crosses the agent boundary into an OS service. The JS runtime doesn't know this — it calls the same APIs it always has. But every call is mediated by capabilities.

### 8.3 WASM Execution

WebAssembly is simpler than JavaScript for this model because WASM has no built-in I/O. A WASM module is pure computation — it can only interact with the outside world through imported functions:

```
WASM module
  imports: {
    env.fetch(url, method) → Web API bridge → OS service channel
    env.storage_get(key)   → Web API bridge → origin sub-space
    env.random()           → OS entropy capability
    env.time()             → OS clock capability
  }
```

Every import is a capability-checked OS call. WASM can't do anything its Tab Agent's capabilities don't permit. This is MORE secure than traditional WASM execution because capability enforcement is OS-level, not browser-level.

-----

## 9. Unique Capabilities

What this browser architecture enables that no other browser can do:

### 9.1 True Site Isolation Without Performance Cost

Chrome's site isolation runs each origin in a separate OS process. This costs memory (process overhead) and IPC latency (cross-process communication for every frame composition).

AIOS Tab Agents are lightweight agents, not heavyweight processes. The microkernel's IPC is designed for high-throughput, low-latency message passing. Capability isolation is hardware-enforced without the overhead of full process isolation. Better isolation with lower cost.

### 9.2 User-Visible Per-Site Resource Accounting

Because each tab is an agent, the OS naturally tracks its resource usage:

"weather.com is using 180MB of memory, 3% CPU, and has transferred 2.4MB of network data in this session. It has 12MB of stored data. It has made 47 requests to its own domain and 23 requests to ad networks."

This isn't a browser developer tool — it's standard OS resource accounting applied to web content. Any user can see it in the audit space.

### 9.3 Ad/Tracker Blocking at the Capability Level

Instead of content blockers that pattern-match URLs (fragile, bypassable), AIOS denies network capabilities for known tracking domains:

```rust
// User preference: block known trackers
if tracking_database.contains(requested_domain) {
    // Don't grant the capability at all
    // JS fetch() to this domain returns NetworkError
    // No packets leave the machine
    // The site's JS can't detect whether blocking is happening
    //   vs a genuine network failure
}
```

This is enforced below JavaScript — the JS runtime genuinely cannot distinguish between "blocked by OS" and "network error." No anti-adblock script can detect it because from the runtime's perspective, the network is simply unreachable for that domain.

### 9.4 Cross-Agent Web Integration

Native AIOS agents can interact with web content through Flow:

```
User has a research agent (native AIOS) and a browser tab open to arxiv.org.

Research agent: "I found 5 relevant papers. Sending to your browser."
  → Flow channel from research agent to Browser Shell
  → Browser Shell opens 5 tabs, each a Tab Agent
  → Tab Agents load arxiv URLs through OS network services
  → User sees 5 paper tabs appear

User reads a paper in the browser tab, highlights a section:
  → Browser Shell captures selection
  → Flow channel to research agent
  → Research agent stores it in the research space with provenance:
    "Highlighted by user from arxiv.org/abs/2026.12345,
     tab session started via research agent recommendation"
```

Web content and native agents cooperate through Flow. The browser isn't a silo — it's integrated into the AIOS task and knowledge workflow.

### 9.5 Spaces as Web App Backend

Progressive web apps on AIOS can use spaces as their backend. Instead of cloud sync through a remote server, the PWA stores data in a space that syncs across devices through the Space Mesh Protocol:

```javascript
// In a PWA running in Tab Agent — AIOS-specific Web API extension
const notes = await aios.space('notes');

// This looks like IndexedDB but it's a space
await notes.put({ id: 'note-1', title: 'Meeting notes', body: '...' });

// This note is now:
// 1. Stored in the local space (persistent)
// 2. Syncing to other AIOS devices via mesh protocol
// 3. Searchable through AIRS
// 4. Backed up with space backup
// 5. Versioned (space WAL)
// No server required. No cloud account required.
```

This is a new Web API that only AIOS provides. PWAs gain OS-level capabilities — sync, search, versioning, backup — without a backend server. For privacy-focused applications, this is transformative: your data never leaves your devices, but it's still synchronized and searchable.

### 9.6 Transparent Phishing Protection

The OS sees the full picture that the browser alone can't:

```
Tab Agent requests connection to: paypa1.com (with a "1" not an "l")
OS Network Service:
  → Domain similarity check against known services in user's history
  → paypa1.com is visually similar to paypal.com (in user's bookmark space)
  → TLS certificate for paypa1.com issued 2 days ago (suspicious)
  → Alert user: "This site looks like PayPal but isn't. Proceed?"
```

Traditional browsers do some of this with Safe Browsing databases. AIOS has richer context — it knows what sites the user actually uses (from the history space and bookmark space), can compare certificate ages, and can correlate across all agents (did any agent recently receive a message containing this suspicious URL?).

-----

## 10. Subsystem Framework Integration

The browser uses the subsystem framework for all hardware access:

|Web API                  |Subsystem|Capability Required           |
|-------------------------|---------|------------------------------|
|`fetch()`                |Network  |NetworkCap for target origin  |
|`getUserMedia()` (camera)|Camera   |CameraCapability (prompted)   |
|`getUserMedia()` (mic)   |Audio    |AudioCapability (prompted)    |
|`navigator.geolocation`  |GPS      |GpsCapability (prompted)      |
|`AudioContext`           |Audio    |AudioCapability (playback)    |
|`WebGL` / `WebGPU`       |Display  |DisplayCapability (limited)       |
|`Bluetooth`              |Bluetooth|BluetoothCapability (prompted)|
|`USB` (WebUSB)           |USB      |UsbCapability (prompted)      |
|`Gamepad`                |Input    |InputCapability (gamepad)     |
|`Notifications`          |Attention|AttentionCap (prompted)       |

Every hardware access from web content goes through the same subsystem framework that native agents use. The same capability gate, the same audit logging, the same conflict resolution. The browser doesn't need its own permission system — the OS has one.

-----

## 11. Implementation Plan

Integrates with the existing phase plan at Phase 21 (Web Browser, 5 weeks total). Sub-phases overlap — 21A/21B run concurrently, as do 21C/21D:

### Phase 21A: Servo Integration (2 weeks)

- Build Servo's layout engine, CSS engine, SpiderMonkey for aarch64-aios target
- Strip Servo's networking and storage layers
- Create Tab Agent scaffold that hosts Servo's rendering

### Phase 21B: Web API Bridge (2 weeks, concurrent with 21A)

- `fetch()` → OS HTTP service via network subsystem
- `localStorage` / `IndexedDB` → web-storage space
- `WebGL` / `WebGPU` → OS GPU capability through display subsystem
- Permissions API → OS capability requests via subsystem framework
- Service Worker → persistent Tab Agent with constrained capabilities

### Phase 21C: Browser Shell (1 week)

- Tab management UI (portable toolkit / iced)
- URL bar, navigation, bookmarks (stored in bookmarks space)
- History (stored in history space)
- Origin capability derivation from URL
- Cross-origin capability grants (CORS → capability)

### Phase 21D: Integration (1 week, concurrent with 21C)

- Tab Agent spawning and lifecycle management
- Multi-tab compositor integration
- Flow integration between browser and native agents
- Ad/tracker blocking at capability level
- Resource accounting per Tab Agent

### Phase 21E: AIOS Web API Extensions (1 week)

- `aios.space()` API for PWA space access
- `aios.flow()` API for PWA ↔ native agent communication
- Feature detection for graceful degradation on non-AIOS browsers
