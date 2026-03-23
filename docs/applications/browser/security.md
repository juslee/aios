# AIOS Browser Security Architecture

Part of: [browser.md](../browser.md) — Browser Kit Architecture
**Related:** [sdk.md](./sdk.md) — Browser Kit SDK, [origin-mapping.md](./origin-mapping.md) — Origin-to-Capability Mapping

-----

## 11. Security Architecture

### 11.1 Threat Model

Web content is **untrusted input** by definition. The browser must defend against threats that traditional browsers handle in userspace but AIOS handles at the OS level:

**Renderer exploits.** A bug in the HTML parser, CSS layout engine, or JavaScript runtime allows arbitrary code execution within the Tab Agent. In a traditional browser, this compromises the renderer process and the attacker attempts sandbox escape. In AIOS, the Tab Agent's capability set is the sandbox — a compromised renderer can only exercise capabilities already granted. There is no sandbox to "escape" because there is no ambient authority to reach.

**Cross-origin data theft.** Malicious content in one tab attempts to read data belonging to another origin. Traditional browsers enforce same-origin policy in the browser process. AIOS enforces it through hardware-isolated address spaces — Tab Agents for different origins share no memory, no capabilities, and no IPC channels unless explicitly bridged through the Browser Shell.

**Spectre and side-channel attacks.** Shared microarchitectural state (caches, branch predictors, TLBs) can leak data across isolation boundaries. AIOS mitigates this through separate agent address spaces (separate TTBR0, separate ASID), Cross-Origin Read Blocking at the network bridge, and timer coarsening in the JS runtime.

**Extension compromise.** Browser extensions have historically been a vector for data exfiltration. AIOS extensions run as separate agents with explicit, minimal capability sets — an ad blocker cannot read passwords, a password manager cannot intercept arbitrary network traffic.

**Drive-by downloads.** Web content initiates file downloads to compromise the host. In AIOS, downloads flow through the Browser Shell into a user-visible Flow channel. The Tab Agent has no capability to write to the filesystem directly — it can only emit content into Flow, where the user decides what to accept. Downloaded files are quarantined in an ephemeral space with restricted capabilities until the user explicitly accepts them.

**Cryptojacking.** Malicious scripts consume CPU/GPU resources for cryptocurrency mining. AIOS's per-agent resource accounting makes this immediately visible — the Behavioral Monitor detects sustained high CPU usage and can throttle or terminate the Tab Agent. The user sees "news-site.com: 95% CPU for 10 minutes" in the agent inspector.

**Phishing.** Deceptive sites impersonate legitimate services. AIOS has richer context for detection than any browser alone — the OS sees the user's bookmark space, history space, certificate ages, and cross-agent URL correlation.

### 11.2 Chrome Sandbox Comparison

Chrome's security architecture represents the state of the art for traditional browsers:

```text
Chrome Architecture                     AIOS Architecture
──────────────────                     ──────────────────
Site Isolation (one process/origin)  →  Agent-per-origin (one agent/origin)
seccomp-bpf syscall filter           →  Capability set (no syscall surface)
IPC via Mojo                         →  Kernel IPC channels
Renderer process sandbox             →  Agent capability restriction
Browser process (privileged)         →  Browser Shell agent (limited caps)
GPU process (shared)                 →  Compute Kit access via capabilities
Network process (shared)             →  OS Network Services (per-agent caps)
```

**Where AIOS improves on Chrome:**

Chrome's site isolation uses one OS process per origin. Each process carries ~10-30MB of overhead (address space, kernel structures, shared libraries). With 50 tabs, Chrome routinely consumes 1-2GB just in process overhead.

AIOS Tab Agents are lightweight agents — isolated address spaces with minimal kernel metadata. The microkernel's IPC is designed for high-throughput message passing between agents. Agent creation is an O(1) kernel operation, not a fork+exec.

Chrome's seccomp-bpf filter blocks dangerous syscalls but still exposes a ~60-syscall surface. Historical sandbox escapes have exploited allowed syscalls in unexpected combinations (`prctl`, `clone`, `futex` interactions). AIOS agents have no syscall surface at all in the traditional sense — they hold capabilities, and each capability is a kernel-mediated IPC endpoint. The attack surface is the set of granted capabilities, which is minimal and auditable. There is no ambient authority to exploit.

Chrome's browser process is fully privileged and is the single point of failure for the entire browser. If compromised, everything is lost. AIOS's Browser Shell agent holds only the capabilities it needs — tab management, bookmarks space, history space, web-storage space. It cannot access the kernel, the filesystem, or other agents' data.

### 11.3 Capsicum-Style Capability Mode

AIOS renderers operate under a restriction model inspired by FreeBSD's Capsicum capability mode. When a Tab Agent is spawned for web content, it enters **restricted capability mode** — a one-way transition after which the agent cannot acquire new capabilities beyond what was pre-granted.

```rust
/// Enforced by the kernel on every Tab Agent hosting web content.
/// Once entered, the agent's capability set is frozen — no new
/// capabilities can be granted without user interaction through
/// the Browser Shell.
pub trait WebContentProcess {
    /// Enter restricted capability mode. This is a one-way transition.
    /// After this call:
    /// - capability_request() returns PermissionDenied for any new cap
    /// - Only capabilities in the agent's CapabilityTable function
    /// - Agent cannot open new file descriptors, network connections,
    ///   or IPC channels not already held
    /// - Agent cannot spawn child agents
    fn enter_capability_mode(&mut self) -> Result<(), SecurityError>;

    /// Check whether this agent is in restricted mode.
    fn is_restricted(&self) -> bool;

    /// The only path to new capabilities is through the Browser Shell,
    /// which mediates user permission prompts. The Shell grants a
    /// capability into the agent's table; the agent did not request it.
    fn receive_granted_capability(&mut self, cap: CapabilityToken)
        -> Result<CapabilityHandle, SecurityError>;
}
```

**How this differs from Chrome's sandbox:**

Chrome's seccomp-bpf filter is a deny-list — it blocks specific syscalls but allows everything else. New syscalls added to the kernel are allowed by default until the filter is updated. AIOS capability mode is an allow-list — only pre-granted capabilities function. Everything else is denied by construction. New kernel features are inaccessible to restricted agents unless explicitly granted.

Like Capsicum's `cap_enter()`, capability mode is irrevocable within the agent's lifetime. A compromised renderer cannot undo it. The kernel enforces the restriction, not the agent.

### 11.4 Spectre and Side-Channel Mitigations

AIOS agents already provide stronger isolation than Chrome's site-isolated processes because each agent runs in a separate address space (separate TTBR0 with unique ASID). This eliminates direct memory access across origins. But microarchitectural side channels require additional defenses:

**Cross-Origin Read Blocking (CORB).** The `NetworkBridge` inspects every cross-origin HTTP response before delivering it to a Tab Agent. Responses with MIME types that should never be cross-origin readable (`text/html`, `application/json`, `text/xml`) are stripped if the request context suggests a side-channel probe (e.g., loaded as an image or script):

```rust
/// Applied at the NetworkBridge layer, before response data
/// reaches the Tab Agent's address space.
pub fn corb_filter(
    request: &HttpRequest,
    response: &HttpResponse,
    tab_origin: &Origin,
) -> CorbDecision {
    let response_origin = Origin::from_url(response.url());

    if response_origin == *tab_origin {
        return CorbDecision::Allow; // same-origin, no filtering
    }

    let content_type = response.content_type();
    let request_destination = request.destination();

    // Block cross-origin HTML/JSON/XML loaded in non-document contexts
    // (script, image, style, etc.) — these are Spectre gadget vectors
    if content_type.is_opaque_to_cross_origin()
        && !request_destination.is_document()
    {
        return CorbDecision::Block {
            reason: "CORB: cross-origin opaque MIME type in non-document context",
        };
    }

    // Sniff confirmation: if Content-Type says text/plain but body
    // starts with HTML/JSON signatures, block anyway
    if content_type.is_ambiguous() && response.body_sniff().is_opaque() {
        return CorbDecision::Block {
            reason: "CORB: content sniffing detected opaque body",
        };
    }

    CorbDecision::Allow
}
```

**Timer coarsening.** High-resolution timers are the primary tool for Spectre-style timing attacks. The JS runtime's `performance.now()` is coarsened to 100us resolution (matching Chrome's post-Spectre behavior). `SharedArrayBuffer` is only available when the page opts into cross-origin isolation via `Cross-Origin-Opener-Policy` and `Cross-Origin-Embedder-Policy` headers — this is enforced at the `CapabilityMapper` level, not in JavaScript.

**Process-per-origin.** Unlike Chrome, which made site isolation optional and then gradually enabled it, AIOS has agent-per-origin from day one. There is no "same-process cross-origin" mode. Every origin gets its own agent with its own address space. This is the strongest Spectre mitigation available — separate page tables mean separate cache footprints.

**Spectre mitigation summary:**

```text
Attack Vector              Chrome Mitigation                 AIOS Mitigation
────────────               ────────────────                 ───────────────
Spectre v1 (bounds check)  Site isolation + CORB             Agent isolation + CORB at NetworkBridge
Spectre v2 (branch pred.)  Retpolines + site isolation       Separate TTBR0/ASID per agent
Timer-based side channels  100us performance.now()           100us coarsening + SAB gating
SharedArrayBuffer abuse    COOP/COEP requirement             Capability-gated SAB access
Cache probing              Site isolation (partial)           Separate address spaces (complete)
```

### 11.5 Extension Security

Browser extensions run as separate AIOS agents, each with an explicit capability set derived from the extension's declared permissions. The principle: **an extension gets only what it declares, and the user approves each capability individually.**

```text
Extension               Capabilities Granted              Capabilities Denied
─────────               ────────────────────              ───────────────────
Ad blocker              NetworkAccess(read-only, all)      StorageAccess (any origin)
                        BrowserShell(block-request)        ClipboardAccess
                                                          HistoryAccess

Password manager        StorageAccess(passwords/)          NetworkAccess (all origins)
                        NetworkAccess(own-server-only)     BrowserShell(modify-page)
                        ClipboardAccess(write, 30s TTL)    HistoryAccess
                        FormFill(password-fields-only)

Privacy dashboard       HistoryAccess(read-only)           NetworkAccess (any)
                        StorageAccess(web-storage, ro)     ClipboardAccess
                        AuditAccess(network-log, ro)       FormFill
```

Extensions cannot escalate their capabilities at runtime. An ad blocker that requests `StorageAccess` after installation triggers a user prompt — the request is visible, auditable, and revocable.

Extensions interact with Tab Agents through the Browser Shell, never directly. An extension cannot inject code into a Tab Agent's address space. Instead, it communicates via IPC: the extension sends a message to the Browser Shell, which forwards it to the appropriate Tab Agent through a capability-checked channel. This prevents the class of extension vulnerabilities where a compromised extension injects scripts into banking pages.

Extension capability sets use Trust Level 3 (Third-party) TTLs — capabilities expire after 90 days and must be re-approved. Extensions installed from untrusted sources can be further restricted to Trust Level 4 (Web content) with 24-hour capability TTLs.

### 11.6 CSP Integration

Content Security Policy headers map directly to capability restrictions on the Tab Agent. When the Browser Shell loads a page and parses its CSP header, it translates each directive into capability constraints before spawning (or reconfiguring) the Tab Agent:

```text
CSP Directive                    Capability Effect
─────────────                    ─────────────────
script-src 'self'              → No ScriptExecution cap for third-party origins;
                                 inline scripts blocked at the engine level
connect-src 'self' api.x.com   → NetworkCap restricted to page origin + api.x.com;
                                 all other network requests denied by kernel
img-src * data:                → ImageLoad cap unrestricted (wildcard)
frame-ancestors 'none'         → Tab Agent cannot be embedded as iframe
                                 (Browser Shell enforces at navigation time)
upgrade-insecure-requests      → NetworkBridge rewrites HTTP → HTTPS;
                                 plain HTTP NetworkCap not granted
```

CSP enforcement in traditional browsers is a browser-process policy that can be bypassed by renderer exploits. In AIOS, CSP restrictions become kernel-enforced capabilities. Even a fully compromised renderer cannot violate CSP because the capability gate sits in the kernel, outside the agent's address space.

The `CapabilityMapper` re-evaluates CSP on every navigation. When a Tab Agent navigates from a page with strict CSP to one with a permissive policy, capabilities are widened. When navigating to a stricter policy, capabilities are attenuated (never revoked and re-granted — attenuation preserves the audit trail). This matches the behavior specified in the capability token lifecycle (see [capabilities.md](../../security/model/capabilities.md) -- 3.1 Attenuation).

-----

## 12. Unique Capabilities

What this browser architecture enables that no existing browser can achieve:

### 12.1 True Site Isolation Without Performance Cost

Chrome's site isolation runs each origin in a separate OS process. Each process carries 10-30MB of overhead (virtual address space metadata, kernel structures, loaded shared libraries, V8 isolate). With 50 tabs across 30 origins, Chrome consumes 300-900MB in process overhead alone.

AIOS Tab Agents are lightweight agents, not heavyweight processes. An agent is an entry in the kernel's agent table (a few KB of metadata), an isolated address space (one TTBR0 page table root), and a capability set (a small array of tokens). The microkernel's IPC is designed for high-throughput, low-latency message passing — compositor frame submission, network responses, and JS bridge calls all flow through kernel IPC channels optimized for zero-copy where possible.

The result: better isolation (hardware-enforced address space separation, capability-only authority) at lower cost (no process overhead, no shared library duplication, no IPC serialization for simple messages).

```text
Metric                  Chrome (50 tabs, 30 origins)     AIOS (50 tabs, 30 origins)
──────                  ────────────────────────────     ─────────────────────────
Process overhead        300-900 MB                       ~3 MB (agent metadata)
IPC mechanism           Mojo (serialized, cross-process) Kernel channels (zero-copy)
Isolation boundary      seccomp-bpf (deny-list)          Capabilities (allow-list)
New tab creation        fork+exec (~50ms)                agent_spawn (~0.1ms)
Memory sharing          Shared libraries per process     Shared kernel text/rodata
```

### 12.2 Per-Site Resource Accounting

Because each tab is a separate AIOS agent, the OS naturally tracks its resource usage through standard kernel accounting:

```text
weather.com:
  Memory:   180 MB (120 MB heap, 45 MB GPU buffers, 15 MB code)
  CPU:      3.2% (1.8% JS execution, 0.9% layout, 0.5% rendering)
  Network:  2.4 MB transferred this session
  Storage:  12 MB in web-storage space
  Requests: 47 to weather.com, 23 to ad networks, 8 to CDNs
  Duration: 34 minutes active, 2 hours backgrounded
```

This is not a browser developer tool — it is standard OS resource accounting applied to web content. Any user can see it through the system's agent inspector. The Behavioral Monitor tracks anomalies: "This tab is using 3x more CPU than its 7-day average" triggers the same alert infrastructure that monitors native agents.

Per-site accounting enables informed decisions: the user sees that a news site consumes 400MB because of ad scripts, not because of content. This visibility is impossible when the browser is a single opaque process.

### 12.3 Capability-Level Ad Blocking

Instead of content blockers that pattern-match URLs in JavaScript (fragile, detectable, bypassable), AIOS denies network capabilities for known tracking domains:

```rust
// During Tab Agent capability derivation, the CapabilityMapper
// consults the user's tracking protection preferences.
fn derive_network_caps(origin: &Origin, prefs: &UserPrefs) -> Vec<NetworkCap> {
    let mut caps = vec![NetworkCap::service(origin.host(), HTTPS, ALL_METHODS)];

    for third_party in page_metadata.third_party_origins() {
        if prefs.tracking_protection.is_blocked(&third_party) {
            // No capability granted. Period.
            // The Tab Agent cannot reach this domain at all.
            continue;
        }
        caps.push(NetworkCap::service(
            third_party.host(), HTTPS, GET,
        ).with_flag(UserCanRevoke));
    }

    caps
}
```

This is enforced below JavaScript. When `fetch("https://tracker.ads.com/pixel.gif")` executes, the capability gate in the kernel returns `NetworkError`. The JS runtime genuinely cannot distinguish between "blocked by OS" and "network unreachable." No anti-adblock script can detect this because there is no observable difference from a genuine network failure — no timing difference, no error code difference, no DOM state difference.

### 12.4 Cross-Agent Web Integration

Native AIOS agents interact with web content through Flow channels, creating workflows impossible in traditional browsers:

```text
Research agent (native AIOS) discovers 5 relevant papers:
  → Research agent emits URLs into Flow channel
  → Browser Shell receives Flow entries
  → Browser Shell spawns 5 Tab Agents (one per URL)
  → Tab Agents load papers through OS network services
  → User sees 5 paper tabs appear with provenance: "Opened by Research Agent"

User highlights a passage in a browser tab:
  → Browser Shell captures selection + origin metadata
  → Selection flows into research agent's Flow channel
  → Research agent stores in research space with provenance:
    "Highlighted by user from arxiv.org/abs/2026.12345,
     tab session started via research agent recommendation"
  → AIRS indexes the highlight for future retrieval
```

Web content and native agents cooperate through Flow. The browser is not a silo — it is integrated into the AIOS task and knowledge workflow. A coding agent can open documentation tabs. A travel agent can fill booking forms. A writing agent can research sources. All through auditable, capability-checked Flow channels.

### 12.5 PWA with Spaces

Progressive web apps on AIOS can use Spaces as their backend. Instead of cloud sync through a remote server, the PWA stores data in a local Space that syncs across devices through the Space Mesh Protocol:

```javascript
// In a PWA running in a Tab Agent — AIOS-specific Web API extension
const notes = await aios.space('notes');

// This looks like IndexedDB but it's a Space
await notes.put({ id: 'note-1', title: 'Meeting notes', body: '...' });

// This note is now:
// 1. Stored in the local Space (persistent, encrypted at rest)
// 2. Syncing to other AIOS devices via Space Mesh Protocol
// 3. Searchable through AIRS (semantic + full-text)
// 4. Versioned (Merkle DAG in the Version Store)
// 5. Backed up with Space backup
// No server required. No cloud account required.
```

For privacy-focused applications, this is transformative: the user's data never leaves their devices, but it is still synchronized, searchable, and versioned. A notes PWA, a todo PWA, or a personal finance PWA can offer full functionality without any backend server — the OS provides everything the cloud would.

### 12.6 Transparent Phishing Protection

The OS sees the full picture that no browser alone can assemble:

```text
Tab Agent requests connection to: paypa1.com (with "1" not "l")

OS-level analysis:
  → Domain similarity: paypa1.com is visually similar to paypal.com
     (found in user's bookmark space, used 47 times)
  → Certificate age: paypa1.com cert issued 2 days ago (suspicious)
  → Cross-agent correlation: a spam agent received a message
     containing this URL 3 hours ago
  → Network history: user has never visited this domain before

Decision: HIGH CONFIDENCE PHISHING
  → Alert user: "This site looks like PayPal but isn't. Proceed?"
  → Log to audit space with full analysis trail
  → If user proceeds: restrict Tab Agent capabilities further
     (no FormFill, no ClipboardAccess, no StorageAccess)
```

Traditional browsers use Safe Browsing databases — static lists of known-bad URLs that require phoning home to a centralized service on every navigation. AIOS has richer, personalized context: the user's actual browsing history, their bookmarks, cross-agent message correlation, certificate freshness analysis, and domain similarity scoring. This context is processed locally (no data sent to Google or any external service), preserving privacy while providing better protection.

When AIRS is available, phishing detection improves further: the model can analyze page content semantics (e.g., "this page asks for PayPal credentials but is not PayPal"), detect credential harvesting forms, and correlate suspicious activity across agents. Without AIRS, the system falls back to rule-based detection (domain similarity, certificate age, URL pattern matching) — still more capable than traditional Safe Browsing alone.

### 12.7 Web API Subsystem Table

Each Web API maps to an AIOS subsystem, a Kit trait, and a required capability:

```text
Web API                   Subsystem        Kit                 Required Capability
───────                   ─────────        ───                 ───────────────────
fetch()                   Network          Net Kit             NetworkCap (origin-scoped)
XMLHttpRequest            Network          Net Kit             NetworkCap (origin-scoped)
WebSocket                 Network          Net Kit             NetworkCap (origin-scoped)
getUserMedia() (camera)   Camera           Media Kit           CameraCapability (prompted)
getUserMedia() (mic)      Audio            Media Kit           AudioCapability (prompted)
navigator.geolocation     Location         Sensor Kit          LocationCapability (prompted)
AudioContext              Audio            Media Kit           AudioCapability (playback)
WebGL / WebGPU            GPU              Compute Kit         GpuCapability (limited)
Bluetooth                 Wireless         Wireless Kit        BluetoothCapability (prompted)
WebUSB                    USB              Device Kit          UsbCapability (prompted)
Gamepad API               Input            Input Kit           InputCapability (gamepad)
Notifications             Attention        Experience Kit      AttentionCapability (prompted)
localStorage              Storage          Storage Kit         StorageCap (origin sub-space)
IndexedDB                 Storage          Storage Kit         StorageCap (origin sub-space)
Cache API                 Storage          Storage Kit         StorageCap (origin sub-space)
Clipboard                 Compositor       Interface Kit       ClipboardCap (gesture-gated)
Fullscreen API            Compositor       Interface Kit       FullscreenCap (gesture-gated)
Screen Wake Lock          Power            Power Kit           WakeLockCap (prompted)
Web Share                 Flow             Flow Kit            ShareCap (gesture-gated)
Payment Request           Credentials      Identity Kit        PaymentCap (prompted)
Credential Management     Credentials      Identity Kit        CredentialCap (prompted)
```

Every hardware access from web content goes through the same subsystem framework that native agents use. The same capability gate, the same audit logging, the same conflict resolution. The browser does not need its own permission system — the OS has one, and it is kernel-enforced.

**Capability prompting model:** Web APIs marked "(prompted)" in the table above trigger the OS permission prompt, not a browser-specific dialog. The user sees the same prompt regardless of whether a native agent or a web tab requests camera access. The grant is stored as a capability token with Trust Level 4 (Web content) TTL — 24 hours — after which the site must re-request access. This is stricter than Chrome's "remember this choice" model, which grants indefinite access.

**No-capability fallback:** When a Web API's required capability is absent, the bridge returns the appropriate Web API error (e.g., `NotAllowedError` for permissions, `NetworkError` for blocked domains). The JS runtime sees standard Web API behavior. No AIOS-specific error types leak into the web platform surface.

-----

## Cross-Reference Index

| Topic | Document | Relevant Sections |
| ----- | -------- | ----------------- |
| Capability token lifecycle | [capabilities.md](../../security/model/capabilities.md) | 3.1 Token lifecycle, 3.5 Temporal caps |
| Defense layers | [layers.md](../../security/model/layers.md) | 2 Eight security layers |
| Agent isolation | [agents.md](../agents.md) | Agent model, address spaces |
| Behavioral monitor | [behavioral-monitor.md](../../intelligence/behavioral-monitor.md) | Anomaly detection, baselines |
| Network capability gate | [networking/security.md](../../platform/networking/security.md) | 6.1 Capability gate |
| Flow channels | [flow.md](../../storage/flow.md) | Cross-agent communication |
| Space Mesh sync | [sync.md](../../storage/spaces/sync.md) | 8.1-8.4 Merkle exchange |
| Privacy architecture | [privacy.md](../../security/privacy.md) | Agent privacy model |
| Adversarial defense | [adversarial-defense.md](../../security/adversarial-defense.md) | Control/data separation |
| Subsystem framework | [subsystem-framework.md](../../platform/subsystem-framework.md) | Capability gate, audit |
