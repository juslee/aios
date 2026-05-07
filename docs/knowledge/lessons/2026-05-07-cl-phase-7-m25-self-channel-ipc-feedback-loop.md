---
author: claude
date: 2026-05-07
tags: [ipc, compositor, debugging, lock-ordering]
status: final
---

# Self-channel IPC feedback loops on placeholder bring-up

## What I hit

During M25 the compositor introduced its first IPC delivery path —
input events and `FocusChanged` notifications routed via
`ipc_send(surface.channel, event_bytes)`. Per-client channels are an
M26 feature, so M25 stores the well-known compositor service
channel as every surface's `Surface.channel` placeholder.

Result: every event the compositor sent to a "client" was queued
into its own `ipc_recv` ring. On the next loop iteration the
compositor would dequeue its own outgoing event, hand it to
`process_request`, the size check (`bytes.len() < size_of::<CompositorRequest>()`)
would fail because `CompositorEvent` is smaller than `CompositorRequest`,
and the warn-log would fire. Constant noise on every keypress.

## Fix

Add a predicate `is_self_channel(ch: ChannelId) -> bool` next to
the `COMPOSITOR_CHANNEL` static, and short-circuit
`send_event_bytes` when it returns true:

```rust
pub fn send_event_bytes(channel: ChannelId, event: &CompositorEvent) {
    if super::service::is_self_channel(channel) {
        return;
    }
    // ... ipc_send ...
}
```

All IPC-delivery callers (`deliver_to_surface`, `notify_focus_change`,
`apply_close_window`, the resize-Configure path) route through
`send_event_bytes`, so the fix is in one place.

## When this generalizes

Any kernel service that:

1. Owns one well-known IPC channel for incoming requests.
2. Stores the same channel as a placeholder for outgoing events
   to per-client subscribers (because per-client channels haven't
   been wired yet).
3. Drains its own request queue in a loop.

… will spam itself the same way unless it suppresses self-sends.
The right long-term fix is per-client channels (each client owns one
endpoint, the service owns the other; events flow asymmetrically).
The short-term fix is the predicate.

## Gotcha during the audit

The bug was invisible until the audit-loop's security/bug review
inspected the IPC dispatch path with the full system in mind. The
build was clean and tests passed because the host-side tests for
`route_event` and `FocusManager` don't exercise actual `ipc_send`
calls. A subagent walking the live data flow caught it.

## Detection

`grep -rn "ipc_send\|ipc_call" kernel/src/` and check for any send
where the target channel could be the same channel the caller's
service owns. The compositor was the first service in AIOS where
this pattern appeared; it'll show up again whenever per-client
channels are deferred to a later milestone.

Per-client channels for the compositor land in M26 alongside the
test app (Step 28); the predicate becomes a no-op once
`Surface.channel` is no longer ever the service channel.
