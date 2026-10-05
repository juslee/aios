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

## Update (2026-09-29, #190)

The rule held, but it was applied only to `ENOSPC`. `EPERM`, the code every capability check returns, had no override and mapped to `SharedMemoryError`, while `CapabilityDenied` came only from `EACCES`, which the kernel never returns; and the kernel kept its own copy of the table (`i64_to_kit_err`), which drifted from `From<IpcError>`. Now there is one table, `IpcKitError::from_code` (decode with `IpcError::try_from`, then `From<IpcError>`), and "least specific correct" needed generic variants (`WouldBlock`, `InvalidArgument`, `ResourceExhausted`, `Suspended`, `Unsupported`, `ObjectGone`), because no domain variant was correct for `EAGAIN`, `EINVAL`, `ENOSPC`, `ENOMEM`, `EACCES`, `ENOTSUP` or `EPIPE` on every path (`EPIPE` is a gone channel on the channel paths and a gone region on the shared memory paths, so its default names no id). Even then, `EPERM` and `ENOSPC` have no variant that is correct on every path, so their default is wrong for some paths and those wrappers must override it: `EPERM` also covers rights that are not capabilities (being a region's creator or mapper), a thread or target with no process and a `ProcessWait` wake that finds no exit code, and `ENOSPC` also covers a request above a fixed limit, where retrying cannot help. `SharedMemoryCreate` above 4 MiB and `MemoryMap` above 64 pages return `EINVAL` instead, so their `ENOSPC` means only a full table. A syscall on such a path with no Kit wrapper (`SharedMemoryShare`, `MemoryUnmap`, `ProcessWait`, `DebugPrint`, `AuditLog`) decodes to the wrong variant. `EPROTO` keeps `NoReply` by choice, so that `NoReply` survives a round trip, although its only path today, `ipc_reply` with no pending call, is an `InvalidArgument` that `reply` reports. The mapping and each wrapper's overrides are in `docs/kits/kernel/ipc.md` §6. When a kernel errno gains a new meaning, check the table row and every wrapper override, not just the wrapper at hand.
