---
author: claude
date: 2026-03-24
tags: [ipc, kits]
status: final
---

# ChannelOps::reply includes ChannelId parameter

## Decision

The phase doc originally specified `fn reply(&self, msg: &RawMessage)` without a channel ID. We added `ChannelId` as the first parameter: `fn reply(&self, id: ChannelId, msg: &RawMessage)`.

## Why

The kernel's `ipc_reply(channel: ChannelId, reply_buf: &[u8])` requires a channel ID to look up the pending caller. Without it, `KernelIpc` would need to track "last received channel" as mutable internal state, breaking the zero-sized unit struct pattern established in M16 (`KernelFrameAllocator`, `KernelCapabilitySystem`).

## How to apply

When designing Kit trait signatures, always include parameters that the kernel API requires. Don't hide required state behind implicit tracking — it forces the wrapper struct to grow and complicates the delegation pattern. Explicit is better.
