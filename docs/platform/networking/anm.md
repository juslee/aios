---
tags: [platform, networking, anm]
type: architecture
---

# AIOS Networking — AI Network Model (ANM) Specification

**Part of:** [networking.md](../networking.md) — Network Translation Module
**Related:** [security.md](./security.md) — Network security, [protocols.md](./protocols.md) — Protocol engines, [components.md](./components.md) — NTM components, [../../security/decentralisation.md](../../security/decentralisation.md) — Decentralisation

-----

## A1. Model Overview

The AI Network Model (ANM) is AIOS's replacement for the OSI 7-layer model. Where OSI was designed for connecting mainframes over telephone wires, ANM is designed for AI agents operating on content-addressed, capability-secured, identity-rooted data.

ANM has **5 layers plus a Bridge Module**. It is not a 7-layer model trimmed down — it is a fundamentally different decomposition that reflects how AI-native systems actually communicate.

### Why OSI Fails for AI-Native Systems

OSI assumes applications manage their own network lifecycle: open sockets, negotiate TLS, construct HTTP requests, parse responses, handle retries. Every application reimplements these patterns. OSI's addressing model (IP addresses, port numbers) exposes infrastructure details that applications should never see. Its security model (TLS as an optional bolt-on at layer 6) means encryption is a choice rather than a structural guarantee.

ANM eliminates these problems. Applications see only space operations. The network is an implementation detail the OS manages entirely.

### Layer Diagram

```text
┌─────────────────────────────────────────────────┐
│  L5  Space Layer         (data operations)      │
├─────────────────────────────────────────────────┤
│  L4  Capability Layer    (authorization)        │
├─────────────────────────────────────────────────┤
│  L3  Identity Layer      (authenticated crypto) │
├─────────────────────────────────────────────────┤
│  L2  Mesh Layer          (routing + transport)  │
├─────────────────────────────────────────────────┤
│  L1  Link Layer          (physical framing)     │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│  Bridge Module  (ANM ↔ TCP/IP translation)      │
│  Sits beside the stack, not inside it.          │
└─────────────────────────────────────────────────┘
```

The Bridge Module is explicitly **not a layer**. It is a translation unit that maps ANM operations to legacy TCP/IP infrastructure and back. It exists because the internet runs TCP/IP today, not because the model requires it.

-----

## A2. Layer Specification

### A2.1 Layer 5 — Space Layer

**Replaces:** OSI Application (L7), Presentation (L6), Session (L5)

The Space Layer is where agents operate. All network communication begins as a space operation — read, write, subscribe, query — on a content-addressed object within a named space. The agent does not know whether the space is local or remote.

#### Data Unit: SpaceOperation

```rust
/// The top-level data unit. An agent's intent expressed as a space operation.
pub struct SpaceOperation {
    /// Content-addressed space identifier (SHA-256 of space descriptor).
    pub space_id: SpaceHash,
    /// Target object within the space. None for space-level operations.
    pub object_id: Option<ContentHash>,
    /// The operation to perform.
    pub operation: OpType,
    /// Serialized payload (object content, query parameters, etc.).
    pub payload: Vec<u8>,
}

pub enum OpType {
    Read,
    Write,
    Delete,
    Subscribe,
    Query,
    Sync,
}
```

**Addressing model:** Content-addressed. Spaces are identified by `SpaceHash`, objects by `ContentHash`. No IP addresses, no hostnames, no port numbers.

**Failure semantics:** Operations return `SpaceError`. The Space Layer retries transparently for transient failures (network timeout, temporary unavailability). Permanent failures (space not found, object deleted) propagate to the caller. The agent never sees network-level errors.

**Relationship to L4:** Every `SpaceOperation` must pass through the Capability Layer before the system acts on it. A `SpaceOperation` without a matching capability is rejected before any network activity occurs.

-----

### A2.2 Layer 4 — Capability Layer

**Replaces:** No direct OSI equivalent (new layer)

The Capability Layer enforces authorization. It wraps a `SpaceOperation` in a signed capability token that proves the caller has permission to perform the operation. This layer is **mandatory and non-bypassable** — it is enforced in the kernel, not in userspace.

#### Data Unit: AuthorizedOp

```rust
/// A space operation authorized by a capability token.
pub struct AuthorizedOp {
    /// The operation being authorized.
    pub operation: SpaceOperation,
    /// Signed capability token proving authorization.
    pub capability: SignedCapabilityToken,
    /// Unique identifier for audit trail linkage.
    pub audit_id: AuditId,
}
```

**Addressing model:** Capability-addressed. Reachability is determined by capability possession, not by knowing an IP address or hostname. If you hold a valid capability for a space, you can reach it. If you do not, the space does not exist from your perspective.

**Failure semantics:** Capability verification either succeeds or fails. There is no degraded mode. A revoked, expired, or invalid capability results in immediate rejection. This layer **never degrades** — partial authorization is not a concept.

**Relationship to L3:** An `AuthorizedOp` is handed to the Identity Layer for encryption and authentication before transmission.

-----

### A2.3 Layer 3 — Identity Layer

**Replaces:** OSI Transport (L4) security aspects, plus TLS from Session (L5)

The Identity Layer handles cryptographic identity and encryption. Every frame is encrypted with the sender's identity key. There is no concept of an unencrypted frame — encryption is structural, not optional.

#### Data Unit: IdentityFrame

```rust
/// An authorized operation encrypted under the sender's identity.
pub struct IdentityFrame {
    /// The authorized operation (encrypted within noise_ciphertext).
    pub authorized_op: AuthorizedOp,
    /// Device identity of the sender.
    pub source: DeviceId,
    /// Noise Protocol Framework ciphertext (encrypts authorized_op).
    pub noise_ciphertext: Vec<u8>,
}
```

**Addressing model:** Identity-addressed. Peers are identified by `DeviceId` (derived from their public key), not by IP address. A `DeviceId` is a stable, location-independent identifier that follows the device across networks.

**Failure semantics:** Cryptographic failures (invalid signature, decryption failure, unknown identity) are fatal — the frame is dropped and an audit event is logged. Identity verification does not degrade to plaintext.

**Relationship to L2:** An `IdentityFrame` is wrapped in a `MeshPacket` for routing and transport.

-----

### A2.4 Layer 2 — Mesh Layer

**Replaces:** OSI Transport (L4) + Network (L3)

The Mesh Layer handles routing and reliable delivery. It collapses OSI's transport and network layers into a single layer because in a peer-to-peer mesh, routing and transport are inseparable concerns. The Mesh Layer selects the best available transport and handles multi-hop relay when direct connectivity is unavailable.

#### Data Unit: MeshPacket

```rust
/// A routable packet in the mesh network.
pub struct MeshPacket {
    /// The identity frame being transported.
    pub identity_frame: IdentityFrame,
    /// Routing metadata (next hop, TTL, relay path).
    pub routing: RoutingInfo,
    /// Transport mode selection.
    pub transport_mode: TransportMode,
}

pub enum TransportMode {
    /// Direct peer-to-peer (LAN, Bluetooth, WiFi Direct).
    Direct,
    /// Relayed through infrastructure (TURN-like).
    Relayed,
    /// Bridge to TCP/IP (for legacy internet services).
    Bridged,
}
```

**Addressing model:** Topology-aware. The Mesh Layer resolves `DeviceId` to a reachable path, which may involve direct connection, relay through trusted intermediaries, or bridge to TCP/IP. The addressing is dynamic — the same `DeviceId` may be reachable via different paths at different times.

**Failure semantics:** Transport failures trigger automatic failover. If a direct path fails, the Mesh Layer attempts relay. If relay fails, it queues for later delivery (if the operation supports eventual consistency). Connection loss is reported to L3 only after all transport options are exhausted.

**Relationship to L1:** `MeshPacket` is serialized and framed for the physical link layer.

-----

### A2.5 Layer 1 — Link Layer

**Replaces:** OSI Data Link (L2) + Physical (L1)

The Link Layer handles physical framing and transmission. This is the only layer that ANM shares substantially with OSI — Ethernet frames, WiFi frames, and BLE packets are what they are. ANM does not reinvent physical networking.

#### Data Unit: LinkFrame

```rust
/// Physical link framing. Varies by medium.
pub enum LinkFrame {
    Ethernet(EthernetFrame),
    WiFi(WiFiFrame),
    Ble(BlePacket),
    UsbNet(UsbNetFrame),
}
```

**Addressing model:** Hardware-addressed (MAC addresses, BLE device addresses). These addresses are used only for single-hop delivery and are invisible to all layers above.

**Failure semantics:** Link-level errors (CRC failure, collision, signal loss) are handled by the medium's own error correction and retransmission. Persistent link failure is reported to L2 for path failover.

**Relationship to L2:** The Link Layer delivers raw bytes to the Mesh Layer. It has no knowledge of the content it carries.

-----

### A2.6 Bridge Module

**Not a layer.** The Bridge Module translates between ANM and TCP/IP for communication with legacy internet services. It sits beside the stack, providing translation services when an ANM space operation resolves to a TCP/IP endpoint.

#### Data Units: BridgeRequest / BridgeResponse

```rust
/// Translation request: ANM space operation → TCP/IP.
pub struct BridgeRequest {
    /// The authorized operation to translate.
    pub authorized_op: AuthorizedOp,
    /// Resolved TCP/IP endpoint.
    pub endpoint: TcpIpEndpoint,
    /// Protocol mapping (HTTP/2, QUIC, WebSocket, etc.).
    pub protocol: BridgeProtocol,
}

/// Translation response: TCP/IP → ANM space result.
pub struct BridgeResponse {
    /// Original audit ID for correlation.
    pub audit_id: AuditId,
    /// Translated result.
    pub result: Result<SpaceResult, SpaceError>,
    /// Provenance metadata from the TCP/IP interaction.
    pub provenance: BridgeProvenance,
}
```

The Bridge Module handles DNS resolution, TLS negotiation, HTTP request construction, response parsing, connection pooling, retry logic, and caching. All of this complexity is invisible to the agent — it sees only the `SpaceOperation` result.

The Bridge Module is expected to shrink over time as more services adopt native ANM protocols.

-----

## A3. Encapsulation

Data flows downward through the stack via encapsulation and upward via decapsulation.

```text
Outbound (encapsulation):

  Agent issues:        space::read("remote/data")
        │
        ▼
  L5  SpaceOperation   { space_id, object_id, Read, payload }
        │
        ▼
  L4  AuthorizedOp     { SpaceOperation, capability_token, audit_id }
        │
        ▼
  L3  IdentityFrame    { AuthorizedOp(encrypted), source_device, ciphertext }
        │
        ▼
  L2  MeshPacket       { IdentityFrame, routing_info, transport_mode }
        │
        ▼
  L1  LinkFrame        { MeshPacket(serialized), MAC header, CRC }
        │
        ▼
       Wire


Inbound (decapsulation):

       Wire
        │
        ▼
  L1  LinkFrame        → extract payload, verify CRC
        │
        ▼
  L2  MeshPacket       → verify routing, check TTL
        │
        ▼
  L3  IdentityFrame    → decrypt, verify identity, authenticate
        │
        ▼
  L4  AuthorizedOp     → verify capability, audit log
        │
        ▼
  L5  SpaceOperation   → execute operation, return result to agent
```

If the destination is a TCP/IP service, L2 routes through the Bridge Module instead of emitting a `LinkFrame`:

```text
  L2  MeshPacket { transport_mode: Bridged }
        │
        ▼
  Bridge Module → DNS → TLS → HTTP/2 → TCP → IP → Wire
```

-----

## A4. Design Principles

### Principle 1: Identity IS the Address

Peers are identified by cryptographic identity (`DeviceId`), not by IP address. A device's identity is stable across network changes — moving from WiFi to cellular does not change who you are. Location is a routing concern resolved at L2, not an addressing concern visible to applications.

### Principle 2: Authorization IS Reachability

If you do not hold a valid capability for a space, that space is unreachable. There is no equivalent of port scanning or unauthorized connection attempts. The capability check at L4 occurs before any network activity, so unauthorized operations never generate traffic.

### Principle 3: Content IS the Name

Objects are identified by content hash, spaces by space hash. Names are derived from content, not assigned by infrastructure. This enables verification without trust — you can confirm you received what you asked for by checking the hash.

### Principle 4: Encryption IS the Layer

There is no unencrypted mode. The Identity Layer (L3) encrypts every frame as a structural property of the protocol, not as an optional feature. Removing encryption would require removing the layer itself, which would break the stack.

### Principle 5: Servers ARE Peers

ANM does not distinguish between "client" and "server." Every participant is a peer with an identity, capabilities, and spaces. A web API endpoint is a peer that happens to be reachable via the Bridge Module. This symmetry simplifies the model and enables true peer-to-peer communication.

### Principle 6: Zero Trust IS Structural

Trust is not a policy decision layered on top — it is built into the stack. Every operation is authorized (L4), encrypted (L3), and audited. There is no "trusted network" mode that bypasses these checks.

### Principle 7: Decentralized by Default, Centralized by Choice

ANM assumes no central authority for naming, routing, or identity. DNS, certificate authorities, and centralized servers are accessed through the Bridge Module as legacy translation, not as architectural requirements. Organizations can choose to run centralized infrastructure, but the model does not require it.

-----

## A5. ANM vs OSI Comparison

| OSI Layer | OSI Name | ANM Equivalent | What Changed |
|---|---|---|---|
| L7 | Application | L5 Space | Replaced protocol-specific APIs (HTTP, FTP, SMTP) with unified space operations |
| L6 | Presentation | L5 Space | Collapsed into Space Layer — serialization is a space concern, not a separate layer |
| L5 | Session | L5 Space | Collapsed into Space Layer — session lifecycle managed by NTM, not applications |
| L4 | Transport | L2 Mesh | Collapsed with Network — transport and routing are inseparable in a mesh |
| L3 | Network | L2 Mesh | IP addressing replaced by identity-based routing |
| L2 | Data Link | L1 Link | Preserved — physical framing is universal |
| L1 | Physical | L1 Link | Preserved — physics does not change |
| (none) | (none) | L4 Capability | **New** — authorization as a mandatory layer, no OSI equivalent |
| (none) | (none) | L3 Identity | **New** — cryptographic identity and encryption as structural layer |
| (none) | (none) | Bridge Module | **New** — translation to legacy TCP/IP, not part of the layer stack |

**Key structural differences:**

- OSI has 7 layers; ANM has 5 + Bridge. The reduction comes from collapsing redundant layers (L5-L7 into Space, L3-L4 into Mesh), not from omitting functionality.
- OSI treats security as optional (TLS is bolted onto L5/L6). ANM makes security structural (L3 Identity, L4 Capability).
- OSI addresses by location (IP). ANM addresses by identity (DeviceId) and content (ContentHash).

-----

## A6. Failure Mode Table

| Layer | Failure | Response | Degrades Gracefully? |
|---|---|---|---|
| L5 Space | Object not found | `SpaceError::NotFound` returned to agent | Yes — partial results returned for queries |
| L5 Space | Space unavailable | Transparent retry with backoff, then error | Yes — cached data served if available |
| L4 Capability | Invalid capability | Immediate rejection, audit logged | **No** — never degrades. No partial authorization. |
| L4 Capability | Expired capability | Immediate rejection, audit logged | **No** — time-based enforcement is absolute. |
| L3 Identity | Decryption failure | Frame dropped, audit event logged | No — cannot fall back to plaintext |
| L3 Identity | Unknown identity | Frame dropped, optionally trigger discovery | No — unauthenticated frames are discarded |
| L2 Mesh | Path failure | Failover to relay or alternate path | Yes — tries Direct, then Relayed, then queues |
| L2 Mesh | All paths exhausted | Operation queued for eventual delivery or error | Yes — offline-capable operations succeed later |
| L1 Link | CRC / signal loss | Medium-specific retransmission | Yes — handled by hardware/firmware |
| L1 Link | Link down | Reported to L2 for path failover | Yes — L2 selects alternative link |
| Bridge | DNS failure | Retry, fallback to cached resolution | Yes — cached entries used during outage |
| Bridge | TLS failure | Connection refused, audit logged | No — cannot fall back to plaintext |
| Bridge | HTTP error (4xx/5xx) | Translated to `SpaceError` variant | Yes — retryable errors retried automatically |

The critical invariant: **L4 (Capability) and L3 (Identity) never degrade.** Security failures are always hard failures. Availability failures at L5, L2, and L1 degrade gracefully through caching, retry, and failover.

-----

## A7. Technology Stack

| Layer | Crate | License | Purpose |
|---|---|---|---|
| L5 Space | (kernel-internal) | BSD-2-Clause | Space operations, object store, query dispatch |
| L4 Capability | (kernel-internal) | BSD-2-Clause | Capability verification, audit logging |
| L3 Identity | `snow` | Apache-2.0 | Noise Protocol Framework for encrypted channels |
| L3 Identity | `ed25519-dalek` | BSD-3-Clause | Ed25519 signatures for device identity |
| L3 Identity | `x25519-dalek` | BSD-3-Clause | X25519 key agreement for session keys |
| L2 Mesh | (kernel-internal) | BSD-2-Clause | Mesh routing, transport mode selection |
| L1 Link | `smoltcp` | BSD-0-Clause | TCP/IP stack (used by Bridge Module and Link Layer) |
| Bridge | `rustls` | Apache-2.0 / MIT | TLS for Bridge Module TCP/IP connections |
| Bridge | `h2` | MIT | HTTP/2 framing for Bridge Module |
| Bridge | `quinn` | Apache-2.0 / MIT | QUIC transport for Bridge Module |

All crates are `no_std` compatible or have `no_std` feature flags. No GPL dependencies.

-----

## A8. Cross-References

### Architecture Decision Records

- [ADR: Capability-routed networking](../../knowledge/decisions/2026-03-25-jl-capability-routed-networking.md) — Why L4 is mandatory and non-bypassable
- [ADR: ANM over OSI](../../knowledge/decisions/2026-03-25-jl-anm-over-osi.md) — Why ANM replaces OSI for AI-native systems

### Related Architecture Documents

| Document | Relevance |
|---|---|
| [networking.md](../networking.md) | Parent hub — NTM architecture overview |
| [components.md](./components.md) | NTM components that implement L5 (Space Resolver) and Bridge (Connection Manager, Shadow Engine) |
| [protocols.md](./protocols.md) | Protocol engines used by the Bridge Module (HTTP/2, QUIC, TLS) |
| [security.md](./security.md) | L4 Capability Gate kernel implementation, per-agent isolation |
| [stack.md](./stack.md) | smoltcp integration used by L1 and Bridge Module |
| [../../security/decentralisation.md](../../security/decentralisation.md) | Decentralisation principles that ANM implements structurally |
| [../../security/model.md](../../security/model.md) | Security model — capability system that L4 enforces |
| [../../experience/identity.md](../../experience/identity.md) | Identity architecture that L3 relies on for DeviceId and key management |

### Discussion Document

The design exploration that led to this specification is recorded in [../../knowledge/discussions/2026-03-25-jl-ai-network-model.md](../../knowledge/discussions/2026-03-25-jl-ai-network-model.md).
