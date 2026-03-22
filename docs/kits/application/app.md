# App Kit

**Layer:** Application | **Architecture:** needs creation

## Purpose

App Kit manages high-level application lifecycle — launch, quit, suspend, resume, and graceful teardown. It wraps IPC Kit's message primitives into an event-driven dispatch loop and exposes scripting interfaces so agents and automation can drive applications programmatically.

## Key APIs

| Trait / API | Description |
|---|---|
| `Application` | Root object representing a running application instance |
| `AppDelegate` | Trait for lifecycle callbacks: `launched`, `will_quit`, `suspend`, `resume` |
| `MessageLoop` | Drives the application's event dispatch loop, routing messages to handlers |
| `ScriptingInterface` | Exposes application actions to AIRS agents and automation scripts |

## Orchestrates

- **IPC Kit** — message channels and reply semantics underpin `MessageLoop`
- **Capability Kit** — capabilities gate what an application may do at launch
- **Memory Kit** — application heap and address space setup on launch

## Implementation Phase

Phase 13+
