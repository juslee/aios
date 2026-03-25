---
author: jl + claude
date: 2026-03-25
tags: [networking, performance, anm]
status: final
---

# ADR: Raw Ethernet Direct Link for LAN Communication

## Context

Two AIOS devices on the same LAN segment (same switch or WiFi AP) need to communicate. The standard path would be IP (TCP handshake + TLS handshake + HTTP framing), which adds 2-3 RTTs of overhead before any application data flows. On a local network where latency matters (clipboard sync, drag-and-drop, screen sharing), this overhead is significant.

## Options Considered

### Option A: IP even for LAN

- Pros: Works with all network hardware without exception, uses standard stack (smoltcp), single code path for LAN and WAN
- Cons: TCP handshake (1 RTT) + TLS handshake (1 RTT) + HTTP framing overhead, 2-3 RTTs minimum before data flows, IP/DNS resolution adds latency, unnecessary protocol overhead for same-segment peers

### Option B: Custom EtherType (0x4149) raw Ethernet frames

- Pros: 0 RTT after initial Noise IK peer authentication (first data in first frame), no IP stack overhead, space operations (read/write/sync) can be encoded directly in Ethernet payload, ~100 microsecond latency on gigabit LAN
- Cons: Some corporate managed switches may filter unknown EtherTypes, does not work across routers (L2 only), requires Ethernet-level driver access

## Decision

Raw Ethernet with EtherType 0x4149 ("AI" in ASCII) for Direct Link mode between same-segment peers. Bridge tunnel (IP/QUIC) serves as automatic fallback when raw Ethernet is blocked or peers are on different subnets. Discovery uses link-local multicast with the same EtherType.

## Consequences

- Zero-RTT LAN communication after initial peer authentication
- Must handle EtherType filtering gracefully: detect blocked EtherType within 500ms and fall back to Bridge tunnel automatically
- Discovery protocol uses link-local multicast (Ethernet broadcast with EtherType 0x4149) for peer announcement
- Two transport paths must be maintained: Direct Link (raw Ethernet) and Bridge tunnel (IP/QUIC)
- Direct Link is the preferred path; Bridge tunnel is fallback, never the other way around
- VirtIO-Net driver must support raw frame injection (not just IP packets)
