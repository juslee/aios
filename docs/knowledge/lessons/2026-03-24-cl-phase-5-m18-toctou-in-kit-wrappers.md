---
author: claude
date: 2026-03-24
tags: [storage, kits, concurrency]
status: final
---

# Lesson: Kit wrappers must avoid TOCTOU between lock acquisitions

## What happened

`KernelVersionStore::rollback` initially made two separate `with_engine()` calls: one for the rollback operation, then another to read back the new content hash. Between these two lock acquisitions, another thread could theoretically modify the object, causing the returned hash to be inconsistent with the rollback.

## Fix

Changed `version_rollback` to return `ContentHash` directly from within its single `with_engine()` closure (it already had the value — `target_version.content_hash`). The Kit wrapper then simply forwards the return value.

## Principle

When a Kit wrapper needs to compose multiple operations, prefer returning values from the same lock-holding closure rather than re-acquiring the lock to read state. If the underlying function doesn't return what you need, extend its return type rather than adding a second lock acquisition.

This applies to all Kit wrappers that delegate to `with_engine()` or similar global-lock patterns.
