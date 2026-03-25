---
author: jl + claude
date: 2026-03-25
tags: [networking, security, crypto, anm]
status: final
---

# ADR: Noise IK Instead of TLS for Mesh Encryption

## Context

The ANM Mesh Layer needs peer-to-peer encryption between AIOS devices. Two candidates: TLS 1.3 (via rustls, pure Rust) or the Noise Protocol Framework (specifically the IK handshake pattern). The choice affects connection latency, code size, certificate management, and alignment with AIOS's identity model.

## Options Considered

### Option A: TLS 1.3 (via rustls)

- Pros: Widely understood and audited, rustls exists as pure Rust with no_std support, proven at internet scale, extensive tooling for debugging
- Cons: Designed for client-server (asymmetric roles), requires Certificate Authorities or complex self-signed certificate management, ~300KB code footprint, minimum 1 RTT handshake, identity and encryption keys are separate concepts

### Option B: Noise IK (x25519-dalek + chacha20poly1305)

- Pros: Designed for peer-to-peer (symmetric roles), uses raw public keys (no CAs needed), 0-RTT for known peers via the IK pattern (initiator sends static key in first message), ~5KB code footprint, identity key IS the encryption key (Ed25519 signing key converts to X25519 via birational map)
- Cons: Less widely understood than TLS, no existing AIOS implementation, fewer debugging tools available

## Decision

Noise IK for all Mesh Layer communication. TLS 1.3 remains in the Bridge Layer only, for legacy HTTPS interoperability. The IK pattern is chosen specifically because AIOS devices already know each other's public keys after pairing, enabling 0-RTT encrypted communication.

## Consequences

- Smaller attack surface: ~5KB vs ~300KB of cryptographic code in the hot path
- Faster connections: 0-RTT for known peers eliminates handshake latency on LAN
- No CA infrastructure needed: keys are self-sovereign, matching AIOS identity model
- TLS stays for Bridge Layer (HTTPS to external servers), creating a clear boundary between trust models
- Must implement Noise IK using x25519-dalek and chacha20poly1305 crates (both no_std, MIT-licensed)
- Ed25519-to-X25519 key conversion must be implemented for identity/encryption key unification
