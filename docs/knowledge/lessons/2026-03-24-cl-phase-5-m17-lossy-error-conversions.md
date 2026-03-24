---
author: claude
date: 2026-03-24
tags: [ipc, kits]
status: final
---

# Lossy error conversions require context-sensitive overrides

## Lesson

The syscall-level `IpcError` uses `Enospc` for multiple semantically distinct failures: message too large, channel table full, ring buffer full, shmem table full, max shared mappings. A single `From<IpcError>` conversion cannot distinguish these — it must pick one mapping (we chose `SharedMemoryError { reason: "out of space" }`).

The fix: each Kit trait method wrapper (`send`, `call`, `channel_create`) applies context-sensitive overrides *before* falling through to the generic `i64_to_kit_err` helper. For example, `call` checks `request.len > MAX_MESSAGE_SIZE` to distinguish MessageTooLarge from ChannelFull when Enospc is returned.

## How to apply

For any future Kit where the kernel reuses the same error code for different failure modes, design the Kit wrapper to disambiguate at the call site rather than in the generic conversion. The `From<IpcError>` should map to the least-specific correct variant, and callers should override when they have context.
