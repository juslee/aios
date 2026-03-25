---
author: jl + claude
date: 2026-03-25
tags: [networking, implementation, phases, anm]
status: final
---

# ADR: Mesh-First Phase 9 Implementation Order

## Context

Phase 9 implements basic networking. The current plan orders milestones as: 8a = smoltcp TCP/IP stack, 8b = TLS/DNS, 8c = POSIX socket emulation. This TCP-first ordering means the mesh layer would be built on top of TCP/IP abstractions, and TCP/IP assumptions would be baked into the foundation. Given the ANM decision (ADR: ANM Over OSI), the implementation order should be reconsidered.

## Options Considered

### Option A: TCP-first (current plan)

- Pros: Internet access immediately in first milestone, familiar development flow (curl equivalent first), smoltcp is well-tested and documented
- Cons: Mesh becomes an afterthought layered on top of TCP/IP, TCP/IP assumptions get baked into driver and buffer management code, first milestone tests internet connectivity instead of peer-to-peer

### Option B: Mesh-first

- Pros: Mesh is the foundation from day one (all subsequent features built on mesh primitives), first milestone demonstrates the core AIOS networking philosophy, driver and buffer management designed for mesh from the start, forces early validation of Noise IK and raw Ethernet
- Cons: Internet access delayed until second milestone, cannot test against external servers in first milestone

## Decision

Mesh-first. Phase 9 milestones reordered: 8a = Mesh Layer (VirtIO-Net driver, raw Ethernet frames, Noise IK encryption, LAN peer discovery), 8b = Bridge Layer (smoltcp TCP/IP, rustls TLS, DHCP/DNS), 8c = Integration (POSIX socket emulation, QUIC WAN tunnel, space-level networking API).

## Consequences

- First milestone deliverable: two QEMU instances communicating over raw Ethernet with Noise IK encryption, zero IP involved
- Internet access (HTTP, DNS) delayed to second milestone
- All subsequent networking features (Shadow Engine, Resilience Engine, Space Sync) built on mesh primitives
- QEMU multi-instance test harness needed from day one (two VMs connected via virtual network)
- VirtIO-Net driver must support raw frame injection in first milestone
- Validates the ANM architecture before adding Bridge Layer complexity
