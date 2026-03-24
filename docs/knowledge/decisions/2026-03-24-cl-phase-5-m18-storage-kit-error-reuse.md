---
author: claude
date: 2026-03-24
tags: [storage]
status: final
---

# Decision: Reuse StorageError directly instead of creating StorageKitError

## Context

Memory Kit and IPC Kit both introduced new Kit-level error types (`MemoryError`, `IpcKitError`) because their syscall-level counterparts were flat enums with no context fields. Storage Kit faced the same choice.

## Decision

Reuse `StorageError` directly — no new `StorageKitError`.

## Rationale

`StorageError` already has 19 domain-specific variants covering all failure modes (BlockNotFound, ChecksumFailed, QuotaExceeded, etc.). All variants are `Copy`-compatible with no fields. Creating a parallel `StorageKitError` would add no semantic value — unlike `IpcError` (which was a flat POSIX-style errno enum needing richer context), `StorageError` already *is* the domain-specific error type.

If field-rich errors become needed later (e.g., `QuotaExceeded { space_id, usage, quota }`), we can introduce `StorageKitError` at that point. For now, the existing enum is sufficient.

## Impact

- One fewer type to maintain
- Kit consumers import the same error type as kernel internals
- Pattern is not universal — Memory/IPC Kits still have their own error types. This is intentional: apply patterns where they add value, not uniformly.
