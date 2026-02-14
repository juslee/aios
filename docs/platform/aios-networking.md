# AIOS Networking: Network Translation Module

## Design Document — Deep Technical Architecture

**Parent document:** [aios-architecture.md](../project/aios-architecture.md)
**Related:** [aios-development-plan.md](../project/aios-development-plan.md) — Phase 7 (basic networking), Phase 16 (full NTM), [aios-subsystem-framework.md](./aios-subsystem-framework.md) — Universal hardware abstraction

**Note:** The networking subsystem implements the subsystem framework. Its capability gate, session model, audit logging, power management, and POSIX bridge follow the universal patterns defined in the framework document. This document covers the network-specific design decisions and architecture.

-----

## 1. Core Insight

In every existing OS, networking is plumbing that applications must manage. Applications open sockets, handle DNS, negotiate TLS, manage connections, implement retry logic, handle offline states, manage caching. Every application reimplements these same patterns badly.

AIOS inverts this. Applications never see the network. There are only **space operations** — some of which happen to involve remote spaces — and the OS handles everything else.

```
What applications see:

    space::read("openai/v1/models")         ← looks like reading a local object
    space::write("collab/doc/123", edit)     ← looks like writing a local object
    space::subscribe("feed/news", on_change) ← looks like subscribing to local changes
    Flow::transfer(remote_obj, local_space)  ← looks like Flow between spaces

What the OS does underneath:

    DNS resolution → TLS handshake → HTTP/2 connection pool →
    request construction → response parsing → cache management →
    retry on failure → circuit breaking → bandwidth scheduling →
    capability enforcement → provenance tracking
```

The application doesn't know or care that `openai/v1/models` is on a server in San Francisco. It's an object in a space. The OS makes it available.

-----

## 2. Full Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Agent / Application                    │
│                                                          │
│   space::remote("openai/v1")?.read("models")            │
│   space::remote("collab/doc/123")?.subscribe(callback)  │
│   Flow::transfer(remote_object, local_space)             │
└──────────────────────┬──────────────────────────────────┘
                       │ Space Operations (kernel syscalls)
                       ▼
┌─────────────────────────────────────────────────────────┐
│              NETWORK TRANSLATION MODULE                   │
│                  (kernel service)                         │
│                                                          │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │   Space      │  │  Connection  │  │   Shadow      │  │
│  │   Resolver   │  │  Manager     │  │   Engine      │  │
│  │             │  │              │  │               │  │
│  │  semantic   │  │  pool/reuse  │  │  local copies │  │
│  │  name → URI │  │  TLS session │  │  of remote    │  │
│  │  + protocol │  │  multiplexing│  │  spaces for   │  │
│  │  + endpoint │  │  keepalive   │  │  offline use  │  │
│  └─────────────┘  └──────────────┘  └───────────────┘  │
│                                                          │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │  Resilience  │  │  Bandwidth   │  │  Capability   │  │
│  │  Engine     │  │  Scheduler   │  │  Gate         │  │
│  │             │  │              │  │               │  │
│  │  retry      │  │  fair share  │  │  verify cap   │  │
│  │  backoff    │  │  priority    │  │  before ANY   │  │
│  │  circuit    │  │  multi-path  │  │  network op   │  │
│  │  breaker    │  │  QoS         │  │  audit trail  │  │
│  └─────────────┘  └──────────────┘  └───────────────┘  │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │              Protocol Translators                 │   │
│  │                                                    │   │
│  │  space.read()     → HTTP GET / AIOS-proto READ   │   │
│  │  space.write()    → HTTP POST/PUT / AIOS-proto   │   │
│  │  space.list()     → HTTP GET (collection)         │   │
│  │  space.delete()   → HTTP DELETE                   │   │
│  │  space.subscribe()→ WebSocket / SSE / AIOS-proto  │   │
│  │  Flow.transfer()  → HTTP chunked / QUIC streams   │   │
│  │  space.query()    → GraphQL / SQL / AIOS-proto    │   │
│  └──────────────────────────────────────────────────┘   │
└──────────────────────┬──────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────┐
│                   Protocol Engines                       │
│                                                          │
│  ┌──────────┐ ┌───────────┐ ┌──────────┐ ┌──────────┐ │
│  │ HTTP/2   │ │ HTTP/3    │ │ AIOS     │ │ MQTT     │ │
│  │ h2 crate │ │ QUIC      │ │ Peer     │ │ (IoT)   │ │
│  │          │ │ quinn     │ │ Protocol │ │          │ │
│  └──────────┘ └───────────┘ └──────────┘ └──────────┘ │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │  Raw Socket Engine (for POSIX compat layer)       │   │
│  │  BSD tools see normal sockets, translated here    │   │
│  └──────────────────────────────────────────────────┘   │
└──────────────────────┬──────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────┐
│                    Transport Layer                        │
│                                                          │
│  ┌───────────┐  ┌──────────┐  ┌───────────────────┐    │
│  │ TLS 1.3   │  │ QUIC     │  │ Plain TCP/UDP     │    │
│  │ (rustls)  │  │ (quinn)  │  │ (POSIX compat)    │    │
│  └───────────┘  └──────────┘  └───────────────────┘    │
└──────────────────────┬──────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────┐
│                    Network Stack                         │
│                    (smoltcp)                              │
│                                                          │
│  TCP │ UDP │ ICMP │ IPv4 │ IPv6 │ ARP │ NDP │ DHCP    │
└──────────────────────┬──────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────┐
│                  Interface Drivers                        │
│                                                          │
│  VirtIO-Net │ Ethernet │ WiFi │ Bluetooth │ Cellular   │
└─────────────────────────────────────────────────────────┘
```

-----

## 3. The Six Components

### 3.1 Space Resolver — Semantic Addressing, Not IP Addressing

Traditional DNS maps names to IP addresses. The Space Resolver maps semantic identifiers to everything the OS needs to reach a remote space.

**Traditional approach:**

```
"api.openai.com" → 104.18.7.192
(application still needs to know: port 443, HTTPS, path /v1/models,
 auth header, content type, etc.)
```

**AIOS Space Resolution:**

```
"openai/v1/models" → SpaceEndpoint {
    protocol: HTTPS,
    host: "api.openai.com",
    port: 443,
    path: "/v1/models",
    auth: CredentialRef("openai-api-key"),  // from credential space
    content_type: "application/json",
    cache_ttl: 300s,
    rate_limit: 60/min,
    fallback: None,
}
```

**Resolution chain (consulted in order):**

```
1. Local cache (recently resolved, still valid)
2. Space Registry (local database of known remote spaces)
3. Well-known providers (openai/, github/, google/ have built-in mappings)
4. AIOS Discovery Protocol (mDNS-like, finds nearby AIOS peers)
5. DNS (fallback for raw hostnames, used by POSIX compat layer)
```

**The Space Registry** is the critical piece. It's a local database that maps semantic space identifiers to connection details. Registries are:

- Pre-populated for common services (like `/etc/hosts` but for the AI era — OpenAI, Anthropic, HuggingFace, GitHub, etc.)
- User-extensible — add your own company's APIs as spaces
- Agent-contributed — when you install an agent, it can register the remote spaces it needs
- Shareable — export your registry, share with team

**Agent manifest declares remote spaces:**

```toml
[agent]
name = "research-assistant"

[spaces.remote]
"openai/v1" = { purpose = "LLM inference", operations = ["read"] }
"arxiv/papers" = { purpose = "paper search", operations = ["read", "query"] }
"user/notes" = { purpose = "save findings", operations = ["read", "write"] }
```

At install time, the user approves these space capabilities. The agent never knows an IP address. It never opens a socket. It just reads from and writes to spaces.

-----

### 3.2 Connection Manager — Invisible, Intelligent Connections

Applications never manage connections. The Connection Manager does.

**Connection pooling.** Multiple reads from `openai/v1` reuse the same HTTP/2 connection. The agent doesn't know or care.

**Protocol negotiation.** The OS picks the best protocol. Two AIOS devices nearby? Use the native AIOS peer protocol (faster, richer semantics). Talking to a web API? HTTP/2. Need real-time updates? WebSocket or HTTP/3 server push. The agent doesn't choose — the OS does.

**TLS session management.** TLS handshakes are expensive. The OS caches TLS sessions, resumes them across connections, and handles certificate verification. No agent ever sees a certificate, handles a TLS error, or decides whether to trust a server. The OS decides based on the system certificate store and the space's trust policy.

**Multiplexing.** HTTP/2 and QUIC support multiplexing — many requests over one connection. The OS exploits this transparently. Ten agents reading different objects from `github/api` share one connection.

```
Agent A: space::read("github/api/repos")  ─┐
Agent B: space::read("github/api/users")  ─┼─→ Single HTTP/2 connection
Agent C: space::read("github/api/issues") ─┘    to api.github.com:443
```

-----

### 3.3 Shadow Engine — Networking Disappears When Offline

The Shadow Engine maintains local shadows of remote spaces. A shadow is a local copy of remote space objects, kept in sync when online and served locally when offline.

**Shadow policy per space:**

```
"openai/v1"     → no shadow (live API, caching pointless for generation)
"arxiv/papers"  → shadow pinned papers (user's saved papers available offline)
"weather/local" → shadow with 1hr TTL (recent forecast available offline)
"collab/doc/X"  → full shadow + conflict resolution (offline editing)
"email/inbox"   → shadow last 7 days (readable offline)
```

**State transitions:**

```
Online state:
    Agent reads "collab/doc/123"
    → OS fetches from remote, stores shadow, returns to agent
    → Shadow marked: version=47, synced_at=now

    Agent writes "collab/doc/123"
    → OS writes to remote, updates shadow, confirms to agent

Transition to offline:
    → OS detects connectivity loss
    → No notification to agents (they don't care)

Offline state:
    Agent reads "collab/doc/123"
    → OS serves from shadow (version=47)
    → Agent doesn't know it's reading a shadow

    Agent writes "collab/doc/123"
    → OS writes to shadow, marks as pending_sync
    → Agent gets success (write accepted)

Transition to online:
    → OS detects connectivity restored
    → Shadow sync begins automatically
    → Pending writes are pushed to remote
    → Conflicts resolved by space-specific CRDT policy
    → Agent notified only if conflict affected their data
```

**Applications never know whether they're online or offline.** There's no `navigator.onLine` check. No "offline mode" the user enables. The OS handles it seamlessly.

This is fundamentally impossible in traditional networking because applications own their connections. If the socket dies, the application knows. In AIOS, the application never had a socket. It had a space. The space is always there.

-----

### 3.4 Resilience Engine — Failures Are the OS's Problem

Every network operation goes through the Resilience Engine.

**Retry policies (per space, configurable):**

```
"openai/v1"     → retry 3x, exponential backoff 1s/2s/4s, then fail
"collab/doc/X"  → retry indefinitely, backoff capped at 30s
"payment/api"   → retry 2x, no backoff (time-sensitive), then fail
```

**Circuit breaker:**

```
If "openai/v1" fails 5 times in 60 seconds:
    → circuit OPEN (stop trying, fail fast)
    → after 30s, try one probe request
    → if probe succeeds, circuit CLOSED (resume)
    → if probe fails, stay OPEN another 30s

Agents see: SpaceError::Unavailable { retry_after: Duration }
Not: ConnectionRefused, TimeoutError, SSLHandshakeFailure,
     DNSResolutionFailed, HTTP503, TCP_RESET...

One error type. The OS absorbed all the complexity.
```

**Error simplification — six errors instead of hundreds:**

```
Traditional errors          → AIOS space errors

DNS_RESOLUTION_FAILED    ─┐
CONNECTION_REFUSED        │
CONNECTION_TIMEOUT        ├─→ SpaceError::Unreachable
SSL_HANDSHAKE_FAILURE     │
NETWORK_UNREACHABLE       ─┘

HTTP_429_RATE_LIMITED     ─┐
HTTP_503_UNAVAILABLE      ├─→ SpaceError::Unavailable { retry_after }
CONNECTION_RESET          ─┘

HTTP_401_UNAUTHORIZED     ─┐
HTTP_403_FORBIDDEN        ├─→ SpaceError::PermissionDenied
CAPABILITY_REVOKED        ─┘

HTTP_404_NOT_FOUND        ──→ SpaceError::NotFound
HTTP_409_CONFLICT         ──→ SpaceError::Conflict { local, remote }
REQUEST_BODY_TOO_LARGE    ──→ SpaceError::TooLarge { max }
```

Six error types instead of hundreds. Agents handle six cases, not six hundred.

-----

### 3.5 Capability Gate — Security by Design, Not by Firewall

The most important component and the most radical departure from traditional networking.

**Traditional security model:** Applications have unrestricted network access. A firewall (if one exists) blocks by port/IP. Any application can connect to any server, exfiltrate any data, phone home to any tracking endpoint.

**AIOS model:** No agent has ANY network access by default. Each network operation requires a specific capability. The kernel enforces this before the packet ever reaches the network stack.

```
Capability: net:read:openai/v1/models
    Grants: Read objects from the "openai/v1/models" space
    Denies: Everything else

    Can:    GET https://api.openai.com/v1/models
    Cannot: GET https://api.openai.com/v1/completions  (different space)
    Cannot: POST https://api.openai.com/v1/models       (write, not read)
    Cannot: GET https://evil.com/exfiltrate              (different space)
    Cannot: TCP connect to 192.168.1.1:22                (no raw socket cap)
```

**What this means in practice:** A research agent that reads papers from arxiv CANNOT send your data to its developer's server, mine cryptocurrency, participate in a botnet, port-scan your network, or connect to any server not declared in its manifest. The capability is granular to the operation level — not "network access" (too coarse), not "access to openai.com" (still too coarse), but `net:read:openai/v1/models` meaning read from exactly that space.

**Credential isolation:**

```
Traditional:
    Agent reads API key from environment variable
    Agent attaches it to HTTP request
    Agent could log it, send it elsewhere, store it

AIOS:
    Credential stored in system credential space
    Agent has capability: cred:use:openai-api-key
    Agent calls: space::read("openai/v1/models")
    OS attaches credential to outgoing request
    Agent NEVER SEES the credential
    Agent cannot extract, copy, or exfiltrate API keys
```

The agent uses the credential without possessing it. Like a hotel room key that opens one door — you can use it, but you can't copy it, and it stops working when you check out.

-----

### 3.6 Bandwidth Scheduler — Fair, Priority-Aware, Multi-Path

The OS controls all network operations, so it can schedule them intelligently.

**Priority levels:**

```
Critical:  OS updates, security patches
High:      Active user interaction (web browsing, chat)
Normal:    Background agent work, sync
Low:       Prefetch, shadow updates, analytics
```

**Multi-path routing:**

```
WiFi (fast, high bandwidth)     → large transfers, browsing
Ethernet (fastest, most stable) → preferred when available
Bluetooth (slow, short range)   → nearby device sync
Cellular (metered, medium)      → fallback only, honor data cap

The OS knows: user has 2GB/month cellular plan
→ shadow sync NEVER uses cellular
→ large downloads pause on cellular, resume on WiFi
→ user never gets surprise data charges
```

Agents don't choose their network path. They submit space operations. The OS routes them based on priority, available interfaces, cost, and bandwidth.

-----

## 4. AIOS Peer Protocol

When two AIOS machines talk to each other, they don't need HTTP. They speak a native protocol that carries the full richness of spaces.

```
AIOS Peer Protocol:
    Transport: QUIC (connection migration, multiplexing, 0-RTT)
    Auth: Mutual TLS with AIOS identity certificates
    Encoding: Structured (not text-based like HTTP)

    Operations:
        SPACE_READ    (key)            → object + metadata
        SPACE_WRITE   (key, value)     → ack + version
        SPACE_LIST    (prefix, filter) → object list
        SPACE_QUERY   (semantic query) → results
        SPACE_SUBSCRIBE (filter)       → event stream
        SPACE_SYNC    (since_version)  → delta updates
        FLOW_TRANSFER (source, dest)   → streaming transfer
        CAPABILITY_EXCHANGE            → mutual capability negotiation
```

**Capability exchange — unique to AIOS-to-AIOS communication:**

When two AIOS devices connect, they negotiate capabilities:

```
Machine A: "I have space 'photos/vacation'. I'm willing to grant you: read."
Machine B: "I accept. I have space 'music/shared'. I'm willing to grant you: read, write."
Machine A: "I accept read only."

→ Machine A can read Machine B's shared music
→ Machine B can read Machine A's vacation photos
→ Both are enforced by kernel capabilities
→ Either side can revoke at any time
```

This is AirDrop but generalized, persistent, capability-controlled, and working for any space — not just individual file transfers.

-----

## 5. Concrete Examples

### 5.1 Web Browsing

The browser (Servo-based) doesn't manage connections. It requests space objects:

```rust
// Browser engine (simplified):
// Traditional browser: manage socket pool, DNS cache, TLS sessions,
//   HTTP cache, cookie jar, CORS checks, redirect chains...

// AIOS browser:
fn load_page(url: &str) -> Document {
    // URL is mapped to a remote space by the resolver
    let page_space = space::remote(url)?;

    // Read the HTML — OS handles connection, TLS, cache, everything
    let html = page_space.read("/")?;

    // Parse HTML, find resources
    let resources = parse_html(&html).resources();

    // Fetch resources in parallel — OS multiplexes over shared connections
    let loaded = space::read_batch(resources)?;

    // Build document
    Document::build(html, loaded)
}
```

The browser is dramatically simpler because the OS handles connection pooling, TLS, caching (shadow engine), offline (cached pages), privacy (per-space cookie isolation), and security (CORS-like rules enforced at capability level).

### 5.2 Agent-to-Agent Communication

Two agents on the same machine communicate via IPC. Two agents on different machines? Same API:

```rust
// Agent A on Machine 1:
let shared = space::open("team/shared-research")?;
shared.write("finding-42", my_analysis)?;

// Agent B on Machine 2:
let shared = space::remote("team/shared-research")?;
shared.subscribe(|change| {
    if change.key == "finding-42" {
        process_finding(change.value);
    }
});

// Agent B's code is IDENTICAL whether Agent A is:
//   - on the same machine (IPC, nanoseconds)
//   - on the local network (AIOS peer protocol, milliseconds)
//   - across the internet (HTTPS, tens of milliseconds)
// The OS routes appropriately. Agents don't know or care.
```

This is the Plan 9 dream, realized. Location transparency — not as a leaky abstraction over sockets, but as a fundamental property of the space model.

### 5.3 POSIX Compatibility

BSD tools still work through the POSIX layer:

```
curl https://api.example.com/data
  ↓ POSIX layer
socket(AF_INET, SOCK_STREAM, 0)  → OS creates space channel
connect(fd, addr, len)           → space::remote("api.example.com")
write(fd, request, len)          → space.write(request_bytes)
read(fd, buffer, len)            → space.read() → response bytes
close(fd)                        → channel dropped
  ↓
Network Translation Module handles everything below
```

BSD tools never know they're not on a traditional OS. But they still benefit from OS-managed TLS, capabilities enforcement, connection pooling, and audit logging.

### 5.4 Automatic Credential Routing

```
# User configures once:
aios credential add openai-api-key "sk-..."
aios credential add github-token "ghp_..."

# In space registry:
"openai/v1" → auth: Bearer(cred:openai-api-key)
"github/api" → auth: Bearer(cred:github-token)

# Any agent with capability to read openai/v1:
space::read("openai/v1/models")
# OS automatically attaches: Authorization: Bearer sk-...
# Agent never sees "sk-..."

# Even curl through POSIX layer:
curl https://api.openai.com/v1/models
# OS recognizes the host, attaches credential automatically
# No more: curl -H "Authorization: Bearer $OPENAI_API_KEY"
```

Credentials flow from the credential space to the Network Translation Module. They never transit through application code. They can't be logged, leaked, or exfiltrated.

-----

## 6. What This Architecture Enables

**1. Network operations are auditable.** Every space read/write is logged with the requesting agent, capability used, target space, and timestamp. You can ask: "What network requests did this agent make?" and get a complete, kernel-verified answer. Not from an app's self-reporting — from the OS.

**2. Network behavior is sandboxed by default.** Installing a "weather agent" that secretly mines crypto is impossible. It declared `net:read:weather/api` — that's all it can do.

**3. Offline is not a special mode.** It's just how the system works. Applications are always working with spaces. Sometimes the OS syncs those spaces with remote endpoints. Sometimes it doesn't. The application's code doesn't change.

**4. Credentials are infrastructure.** No more `.env` files, no more API keys in source code, no more "I accidentally committed my secret to GitHub." Credentials live in the credential space, flow through the OS, and never touch application code.

**5. Protocol evolution is transparent.** When HTTP/4 arrives, the OS upgrades its protocol engine. Every agent immediately uses HTTP/4. No library updates, no dependency bumps, no breaking changes. The space API didn't change.

**6. Network is multi-path by default.** The OS uses WiFi, Ethernet, Bluetooth, and cellular simultaneously, routing each operation optimally. No application ever picks a network interface.

**7. The network is typed.** When you read from a space, you get structured objects — not byte streams you parse yourself. The OS knows the content type, handles serialization/deserialization, and validates the data. An agent reading `weather/local/forecast` gets a typed weather object, not a JSON string it has to parse and hope is valid.

-----

## 7. Implementation Order

Each sub-phase delivers usable functionality independently. Basic networking is part of Phase 7 (Input, Terminal & Basic Networking). The full Network Translation Module is Phase 16.

```
Phase 7a:  smoltcp + VirtIO-Net driver     → raw TCP/IP works
Phase 7b:  rustls + DNS/DHCP               → TLS and name resolution work
Phase 7c:  POSIX socket emulation           → BSD tools with networking (curl, ssh)
Phase 16a: Connection Manager + Protocol    → HTTP/2, WebSocket work
Phase 16b: Space Resolver + Capability Gate → space operations over network
Phase 16c: Shadow Engine                    → offline support
Phase 16d: Resilience + Bandwidth Scheduler → production-grade
Phase 16e: AIOS Peer Protocol               → AIOS-to-AIOS communication
```

After Phase 7c, a developer can `curl` from the AIOS shell. After Phase 16b, agents can reach remote spaces. After 16c, the system works offline. Each layer is testable independently.

-----

## 8. Key Technology Choices

|Component        |Choice             |License       |Rationale                            |
|-----------------|-------------------|--------------|-------------------------------------|
|TCP/IP stack     |smoltcp            |BSD-2-Clause  |Pure Rust, no_std, production-quality|
|TLS              |rustls             |Apache-2.0/MIT|Pure Rust, no OpenSSL dependency     |
|QUIC             |quinn              |Apache-2.0/MIT|Pure Rust, built on rustls           |
|HTTP/2           |h2                 |MIT           |Pure Rust, async                     |
|DNS              |trust-dns / hickory|Apache-2.0/MIT|Pure Rust, async                     |
|Certificate store|webpki-roots       |MPL-2.0       |Mozilla's CA bundle                  |

All pure Rust, all permissively licensed, all no_std compatible or portable.

-----

## 9. Data Model

```rust
/// Resolved endpoint for a remote space
pub struct SpaceEndpoint {
    protocol: Protocol,
    host: String,
    port: u16,
    path: String,
    auth: Option<CredentialRef>,
    content_type: ContentType,
    cache_policy: CachePolicy,
    rate_limit: Option<RateLimit>,
    fallback: Option<Box<SpaceEndpoint>>,
}

pub enum Protocol {
    Https,
    Wss,         // WebSocket Secure
    AiosPeer,    // Native AIOS-to-AIOS
    Mqtt,        // IoT
    RawTcp,      // POSIX compat fallback
}

/// Shadow of a remote space object
pub struct Shadow {
    space_id: RemoteSpaceId,
    key: String,
    local_content: Content,
    remote_version: u64,
    synced_at: Timestamp,
    pending_writes: Vec<PendingWrite>,
    shadow_policy: ShadowPolicy,
}

pub enum ShadowPolicy {
    None,                                // Never shadow (live API)
    Pinned,                              // Shadow explicitly pinned objects
    TtlBased { ttl: Duration },          // Shadow with time-to-live
    Full { conflict: SyncConflictPolicy },// Full shadow + offline writes
}

pub enum SyncConflictPolicy {
    LastWriteWins,
    CrdtMerge,
    ManualResolve,
}

/// Simplified error model (6 errors, not 600)
pub enum SpaceError {
    Unreachable,
    Unavailable { retry_after: Option<Duration> },
    PermissionDenied,
    NotFound,
    Conflict { local: Version, remote: Version },
    TooLarge { max_bytes: u64 },
}

/// Network capability (kernel-enforced)
pub enum NetCapability {
    ReadSpace(RemoteSpaceId),
    WriteSpace(RemoteSpaceId),
    SubscribeSpace(RemoteSpaceId),
    QuerySpace(RemoteSpaceId),
    UseCredential(CredentialId),
    RawSocket(HostPort),  // Only for POSIX compat, heavily restricted
}

/// Circuit breaker state
pub enum CircuitState {
    Closed,                          // Normal operation
    Open { until: Timestamp },       // Failing, fast-reject
    HalfOpen,                        // Probing after open period
}
```

-----

## 10. Design Principles

1. **Applications see spaces, not sockets.** The network is an implementation detail of remote spaces.
1. **The OS owns all connections.** No application opens sockets, negotiates TLS, or manages connection pools.
1. **Offline is the default assumption.** Every remote space operation must have a defined offline behavior (shadow, fail, queue).
1. **Credentials are infrastructure.** They flow through the OS, never through application code. Applications use credentials without possessing them.
1. **Six errors, not six hundred.** The OS absorbs network complexity and presents a simple, consistent error model.
1. **Network access requires capability.** No default network access. Every operation is audited.
1. **Protocol choice is the OS's decision.** The OS picks the best protocol for each operation based on endpoint type, available interfaces, and conditions.
1. **Location is transparent.** `space::read()` works identically whether the data is local, on the LAN, or across the internet.

-----

## 11. Layered Service Architecture

The networking subsystem follows the "mandatory kernel gate + optional userspace services" pattern from the subsystem framework.

### 11.1 What's Mandatory (Kernel)

The **capability gate** is the only part in the kernel. Every network connection passes through it. Non-negotiable, non-bypassable:

```rust
// Kernel level — a few hundred lines
fn network_connect(agent: AgentId, destination: &ServiceTarget) -> Result<RawChannel> {
    let caps = capability_store.get(agent);
    if !caps.allows_network(destination) {
        audit_log(agent, destination, "DENIED");
        return Err(PermissionDenied);
    }
    audit_log(agent, destination, "ALLOWED");
    Ok(create_raw_channel(destination))
}
```

The gate enforces WHO can talk to WHAT. It doesn't understand HTTP or manage TLS. It checks capabilities and logs everything.

### 11.2 What's Optional (Userspace Services)

Everything above the gate is a userspace service that agents can use or bypass:

**OS TLS Service (strongly recommended):** Provides connection pooling, session resumption, certificate pinning, unified trust store. Agents using it get a `tls:os-managed` capability label (higher trust). Agents can opt out and do their own TLS — they get `tls:self-managed` (visible to user, lower trust).

**OS HTTP Service (optional):** Provides connection pooling, response caching, compression, retry with backoff, rate limit management. Convenience, not requirement.

**OS DNS Service (strongly recommended):** Provides encrypted DNS (DoH/DoT), caching in DNS space, audit. Agents can bypass with raw UDP capability — flagged as `dns:self-managed`.

### 11.3 Trust Labels

The layered approach creates visible trust signals:

```
Agent A: net(api.weather.gov), tls(os-managed), http(os-managed), dns(os-managed)
  → "Fully auditable. Maximum trust."

Agent B: net(custom-server.io), tls(os-managed), http(self-managed), dns(os-managed)
  → "Custom protocol over OS-verified TLS."

Agent C: net(*.onion), tls(self-managed), dns(self-managed)
  → "Manages own encryption and DNS. OS verifies destination only."
```

The user sees meaningful information, not IP addresses and port numbers.

### 11.4 Browser Exception

The web browser is the one agent where OS-managed TLS and HTTP are **mandatory, not optional**. The browser runs arbitrary, untrusted code (JavaScript) from any website. The browser agent cannot opt out of OS network management because its execution environment is fundamentally untrusted.
