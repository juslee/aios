---
author: jl + claude
date: 2026-10-06
tags: [ipc, security, tooling]
status: final
---

# Lesson: A change to IPC capability checks touches docs that docs-check cannot see

## What happened

Audits of changes to IPC capability enforcement (`kernel/src/ipc/*.rs`, `kernel/src/cap/mod.rs`) kept finding that several docs restate which syscall checks which capability, or pin function line numbers, and that the mechanical docs check does not see either kind of statement. The first audit (2026-09-23) found the project instructions' capability block omitting `ipc_cancel` and `channel_destroy`, and the agent sandbox doc's syscall enumeration omitting `IpcSelect`. Both have since been fixed.

## Why it happened

`aios docs-check` verifies links, section references, paths, recipe names, lock-order tables and counts that it knows about (`aios docs-check --list-checks`). It does not parse prose enumerations ("the kernel checks X on every call, send and recv") or `// file.rs:NN` anchors, so those drift silently.

## What we learned

Touchpoints to audit when capability enforcement changes, checked against the code at 368d99a:

- `.claude/CLAUDE.md`, Key Technical Facts, "Capability enforcement": `channel_create` checks `ChannelCreate`; `ipc_call`, `ipc_send`, `ipc_recv`, `ipc_cancel` and `channel_destroy` check `ChannelAccess`; `ipc_select` checks it for every channel entry; `ipc_reply` checks nothing. This matches `kernel/src/ipc/` and `kernel/src/cap/mod.rs`.
- `docs/applications/agents/communication.md`, §10.1, the "Capability enforcement at every hop" paragraph.
- `docs/applications/agents/sandbox.md`, §7.1, the `AgentSyscall` enum: per-variant `Requires:` lines, the section total ("18 syscalls") and the category counts ("IPC (5 syscalls)"). It lists `IpcSelect` now.
- `docs/project/ai-agent-context.md`, §6.2 and §6.3, "Key APIs" blocks, which pin `// file.rs:NN` anchors (for example `channel.rs:33` for `ipc_call`, `ipc/mod.rs:182` for `channel_create`, `cap/mod.rs:94` for `check_channel_access`). Adding doc comments above a function makes them stale; say which anchors a change broke and which were already wrong.
- `docs/kernel/deadlock-prevention.md`, §3.3, which quotes lock-order comments with line numbers (`select.rs:30-31` and others).
- The phase record `docs/phases/03-ipc-and-capability-system.md` is checked off history. Do not flag it.

Code facts behind these docs, verified at the same commit:

- `channel_create` checks `ChannelCreate` and grants its creator no `ChannelAccess`; the capability-lifetime ADR decides the fix.
- Notification operations are not capability-checked (`kernel/src/ipc/notify.rs` has no check).
- `check_channel_access` range-checks the channel id (EINVAL) before it takes `PROCESS_TABLE`.

## How to avoid next time

- When a PR changes who checks what, grep the docs above by hand for the syscall names, and for `file.rs:NN` anchors into the files it edited.
- Run `just docs-check` as usual for new drift, but do not read a clean result as evidence that these touchpoints are right.
- If the PR changes lock order, `aios docs-check` does check the production `Mutex` statics against `docs/kernel/deadlock-prevention.md` §3.3 and §3.4 and the lock-order block in `.claude/CLAUDE.md`; the quoted comments with line numbers are still by hand.
