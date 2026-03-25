---
author: jl + claude
date: 2026-03-25
tags: [networking, research, anm]
status: final
---

# Research References: AI Network Model (ANM)

Projects and papers that informed the ANM design, and what AIOS takes from each.

## Projects

| Project | What It Is | What ANM Takes From It | What ANM Does Differently |
|---|---|---|---|
| Noise Protocol Framework | Cryptographic handshake framework by Trevor Perrin (co-creator of Signal) | IK pattern for 0-RTT peer auth, ChaCha20-Poly1305 AEAD, X25519 DH | ANM uses Noise as the ONLY encryption (not optional like in some Noise deployments) |
| IPFS | Content-addressed peer-to-peer file system | Content-addressing (CID), Merkle DAG, peer-to-peer content delivery | ANM integrates content-addressing at the OS level (not an overlay); adds capability-gating |
| NDN/CCN (Named Data Networking) | Research network architecture: request data by name, not location | Name-based routing, in-network caching, data-centric security | ANM uses content hashes (not human-readable names); adds capability tokens as routing credentials |
| Yggdrasil | Encrypted IPv6 overlay mesh network | Crypto-routing (public key = address), spanning tree routing | ANM is native OS networking (not overlay); doesn't use IPv6 overlay |
| CJDNS | Encrypted mesh networking with IPv6 addressing | Public key as network address, encrypted by default | ANM doesn't overlay on IPv6; uses raw Ethernet for LAN |
| libp2p | Modular networking stack for P2P applications | Multi-transport, peer identity, protocol negotiation | ANM is OS-level (not a library); single protocol (not negotiated) |
| WireGuard | Modern VPN protocol using Noise | Noise IK handshake, stateless design, minimal attack surface | ANM's mesh is not a VPN (no IP tunneling in native mode); WireGuard lives in Bridge Module |
| Signal Protocol | End-to-end encrypted messaging | Double Ratchet for forward secrecy, X3DH key agreement | ANM uses simpler Noise IK (not Double Ratchet) -- sufficient for device-to-device (not multi-device group chat) |
| seL4 / EROS | Capability-based operating system kernels | Capability tokens as unforgeable access credentials | ANM extends capabilities to NETWORKING (not just local resources) -- capabilities as routing credentials |
| Plan 9 (9P) | Research OS: everything is a file server | Network transparency, uniform resource naming | ANM uses spaces instead of files; content-addressed instead of path-addressed |
| Fuchsia (FIDL) | Google's capability-based OS | Structured IPC, capability-based security, network as service | ANM makes the network NATIVE (not a service); identity = address (Fuchsia still uses IP) |
| Tor | Anonymous communication overlay | Onion routing, traffic padding, relay architecture | Future research for ANM Bridge anonymization; not in initial implementation |

## Papers

- **Trevor Perrin, "The Noise Protocol Framework" (2018)** -- Foundation for L2/L3 crypto. Defines the IK handshake pattern used for 0-RTT mutual authentication between known peers. The framework's composability (mixing patterns, PSK modes) gives ANM room to evolve without replacing the core protocol.

- **Van Jacobson et al., "Networking Named Content" (NDN project, 2009)** -- Content-centric networking. Demonstrated that requesting data by name (rather than by host location) fundamentally changes network architecture: enables in-network caching, multipath delivery, and natural multicast. ANM adapts this insight using content hashes instead of human-readable names.

- **Dennis & Van Horn, "Programming Semantics for Multiprogrammed Computations" (1966)** -- Capability-based security origin. Introduced the concept of capabilities as unforgeable tokens granting access rights. ANM extends this from local process resources to network routing -- a capability token is required not just to access a resource but to reach it at all.

- **Jason Donenfeld, "WireGuard: Next Generation Kernel Network Tunnel" (NDSS 2017)** -- Validates Noise + ChaCha20 for kernel networking. Demonstrates that the Noise IK + ChaCha20-Poly1305 + X25519 crypto stack is production-ready for kernel-level networking with minimal attack surface. ANM reuses this exact stack, gaining confidence from WireGuard's deployment scale.

- **Mark Miller, "Robust Composition: Towards a Unified Approach to Access Control and Concurrency Control" (2006)** -- Object-capability model. Shows that capabilities compose naturally and can unify access control with concurrency control. ANM applies this to networking: capability tokens compose (attenuation, delegation) and naturally gate concurrent network access.
