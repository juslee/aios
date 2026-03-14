# AIOS Networking — Concrete Examples & Data Model

**Part of:** [networking.md](../networking.md) — Network Translation Module
**Related:** [components.md](./components.md) — NTM components, [security.md](./security.md) — Network security, [protocols.md](./protocols.md) — Protocol engines

-----

## 9. Concrete Examples & Integration

### 9.1 Web Browsing

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

-----

### 9.2 Agent-to-Agent Communication

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

-----

### 9.3 POSIX Compatibility

BSD tools still work through the POSIX layer:

```text
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

**POSIX socket mapping** translates between the traditional socket API and the space model:

```text
socket()   → allocate a space channel descriptor
bind()     → register local endpoint in space registry
listen()   → create subscription on incoming connections
accept()   → receive next incoming space connection
connect()  → space::remote() + resolve + establish connection
send()     → space.write() on the connected channel
recv()     → space.read() on the connected channel
select()   → IPC select on multiple space channels
close()    → drop channel, release resources
```

For the POSIX translation layer architecture, see [posix.md](../posix.md).

-----

### 9.4 Automatic Credential Routing

```text
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

**Credential routing process:**

1. Agent submits space operation (e.g., `space::read("openai/v1/models")`)
2. NTM resolves space to endpoint, finds `auth: Bearer(cred:openai-api-key)`
3. NTM requests credential from credential space with agent's `cred:use:openai-api-key` capability
4. Credential space verifies capability, returns credential handle (not the credential itself)
5. NTM injects credential into outgoing HTTP request
6. Response flows back to agent — credential was never in agent's address space

For credential vault architecture, see [security.md §6.4](./security.md).

-----

### 9.5 Data Model

The core types that define the networking subsystem's interfaces.

#### 9.5.1 Space Endpoint Resolution

```rust
/// Resolved endpoint for a remote space
pub struct SpaceEndpoint {
    /// Transport protocol to use
    protocol: Protocol,
    /// Remote hostname or IP
    host: String,
    /// Remote port
    port: u16,
    /// URL path prefix
    path: String,
    /// Credential reference for authentication
    auth: Option<CredentialRef>,
    /// Expected content type for responses
    content_type: ContentType,
    /// Client-side cache policy
    cache_policy: CachePolicy,
    /// Rate limit to respect (client-side throttle)
    rate_limit: Option<RateLimit>,
    /// Fallback endpoint if primary is unreachable
    fallback: Option<Box<SpaceEndpoint>>,
}

pub enum Protocol {
    Https,       // Standard web APIs
    Wss,         // WebSocket Secure (subscriptions, real-time)
    AiosPeer,    // Native AIOS-to-AIOS protocol over QUIC
    Mqtt,        // IoT device communication
    RawTcp,      // POSIX compat fallback (heavily restricted)
}

pub enum CachePolicy {
    NoCache,                         // Always fetch fresh
    TtlBased { ttl: Duration },      // Cache with expiry
    Immutable,                       // Never changes (e.g., content-addressed)
    Revalidate,                      // Cache but check freshness (ETag/If-Modified-Since)
}
```

#### 9.5.2 Shadow Types

```rust
/// Shadow of a remote space object
pub struct Shadow {
    /// The remote space this shadows
    space_id: RemoteSpaceId,
    /// Object key within the space
    key: String,
    /// Local copy of the content
    local_content: Content,
    /// Version number from last successful sync
    remote_version: u64,
    /// Timestamp of last successful sync
    synced_at: Timestamp,
    /// Writes made while offline, ordered
    pending_writes: Vec<PendingWrite>,
    /// How this shadow behaves
    shadow_policy: ShadowPolicy,
}

pub enum ShadowPolicy {
    None,                                 // Never shadow (live API)
    Pinned,                               // Shadow explicitly pinned objects
    TtlBased { ttl: Duration },           // Shadow with time-to-live
    Full { conflict: SyncConflictPolicy },// Full shadow + offline writes
}

pub enum SyncConflictPolicy {
    LastWriteWins,    // Timestamp-based resolution
    CrdtMerge,        // Automatic CRDT merge
    ManualResolve,    // Present conflict to user/agent
}
```

#### 9.5.3 Error Model

```rust
/// Simplified error model (6 errors, not 600)
pub enum SpaceError {
    /// Remote space cannot be reached at all
    Unreachable,
    /// Remote space temporarily unavailable
    Unavailable { retry_after: Option<Duration> },
    /// Agent lacks capability for this operation
    PermissionDenied,
    /// Requested object does not exist in the space
    NotFound,
    /// Local and remote versions have diverged
    Conflict { local: Version, remote: Version },
    /// Request payload exceeds space/network limits
    TooLarge { max_bytes: u64 },
}
```

#### 9.5.4 Network Capabilities

```rust
/// Network capability (kernel-enforced)
pub enum NetCapability {
    /// Read objects from a remote space
    ReadSpace(RemoteSpaceId),
    /// Write objects to a remote space
    WriteSpace(RemoteSpaceId),
    /// Subscribe to changes in a remote space
    SubscribeSpace(RemoteSpaceId),
    /// Execute queries against a remote space
    QuerySpace(RemoteSpaceId),
    /// Use a credential (without seeing it)
    UseCredential(CredentialId),
    /// Raw socket access (POSIX compat only, heavily restricted)
    RawSocket(HostPort),
}
```

#### 9.5.5 Circuit Breaker State

```rust
/// Circuit breaker state per remote space
pub enum CircuitState {
    /// Normal operation — requests flow through
    Closed,
    /// Service is failing — fast-reject all requests
    Open { until: Timestamp },
    /// Probing after cooldown — allow single test request
    HalfOpen,
}

/// Circuit breaker configuration
pub struct CircuitConfig {
    /// Number of failures to trigger open
    failure_threshold: u32,
    /// Time window for counting failures
    failure_window: Duration,
    /// How long to stay open before probing
    cooldown: Duration,
    /// Number of successful probes to close
    success_threshold: u32,
}
```

-----

## What This Architecture Enables

The NTM architecture produces seven emergent properties that are impossible with traditional application-level networking:

**1. Network operations are auditable.** Every space read/write is logged with the requesting agent, capability used, target space, and timestamp. You can ask: "What network requests did this agent make?" and get a complete, kernel-verified answer. Not from an app's self-reporting — from the OS.

**2. Network behavior is sandboxed by default.** Installing a "weather agent" that secretly mines crypto is impossible. It declared `net:read:weather/api` — that's all it can do.

**3. Offline is not a special mode.** It's just how the system works. Applications are always working with spaces. Sometimes the OS syncs those spaces with remote endpoints. Sometimes it doesn't. The application's code doesn't change.

**4. Credentials are infrastructure.** No more `.env` files, no more API keys in source code, no more "I accidentally committed my secret to GitHub." Credentials live in the credential space, flow through the OS, and never touch application code.

**5. Protocol evolution is transparent.** When HTTP/4 arrives, the OS upgrades its protocol engine. Every agent immediately uses HTTP/4. No library updates, no dependency bumps, no breaking changes. The space API didn't change.

**6. Network is multi-path by default.** The OS uses WiFi, Ethernet, Bluetooth, and cellular simultaneously, routing each operation optimally. No application ever picks a network interface.

**7. The network is typed.** When you read from a space, you get structured objects — not byte streams you parse yourself. The OS knows the content type, handles serialization/deserialization, and validates the data. An agent reading `weather/local/forecast` gets a typed weather object, not a JSON string it has to parse and hope is valid.
