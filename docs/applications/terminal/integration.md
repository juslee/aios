# AIOS Terminal Platform Integration

Part of: [terminal.md](../terminal.md) — Terminal Emulator Architecture
**Related:** [sessions.md](./sessions.md) — Session lifecycle, [multiplexer.md](./multiplexer.md) — Session management, [emulation.md](./emulation.md) — VT engine state

-----

## 8. Platform Integration

The terminal emulator integrates with AIOS's platform services through the universal subsystem framework (see [subsystem-framework.md](../../platform/subsystem-framework.md)). This section documents how each framework component is instantiated for the terminal.

### 8.1 Subsystem Framework Conformance

The terminal implements all five subsystem framework layers, plus the two cross-cutting concerns (capability gate and audit) that span them:

```text
┌─────────────────────────────────────────────────────┐
│ Agent API Layer                                      │
│   TerminalCreate, TerminalAttach, TerminalControl    │
│   (capability-gated, typed IPC messages)             │
├─────────────────────────────────────────────────────┤
│ POSIX Translation                                    │
│   /dev/tty, /dev/pts/*, /dev/ptmx, /dev/console     │
│   ioctl: TIOCGWINSZ, TCGETS, TIOCSPGRP, etc.        │
├─────────────────────────────────────────────────────┤
│ Terminal Service                                     │
│   Session registry (active + detached sessions)      │
│   VT emulation engine instances                      │
│   Multiplexer (tabs, panes, detach/reattach)         │
├─────────────────────────────────────────────────────┤
│ PTY Abstraction                                      │
│   IPC channel pairs (input/output per session)       │
│   Notification channels (signals)                    │
│   Shared memory regions (bulk transfer)              │
├─────────────────────────────────────────────────────┤
│ Rendering Driver                                     │
│   Compositor surface (shared buffer + damage)        │
│   Glyph atlas integration                            │
│   GPU-accelerated text rendering                     │
└─────────────────────────────────────────────────────┘
   🔒 Capability Gate          📋 Audit Space
   (all operations gated)      (all sessions logged)
```

### 8.2 Capability Gate

The terminal defines capability types for its operations:

#### 8.2.1 Terminal Capabilities

| Capability | Required For | Trust Level |
|---|---|---|
| `TerminalCreate` | Creating new terminal windows/sessions | System (auto-granted to system agents) |
| `TerminalAttach` | Reattaching to existing sessions | System |
| `TerminalControl` | Programmatic session management (create/destroy sessions via API) | Agent (requires explicit grant) |
| `ProcessCreate` | Spawning shell and command processes | System (delegatable to shells) |
| `CompositorSurface` | Creating the terminal's display surface | System |
| `ClipboardWrite` | Writing to clipboard (copy from terminal) | System |
| `ClipboardRead` | Reading from clipboard (paste into terminal) | User-prompted |
| `SpaceRead("terminal/*")` | Reading terminal profile, scrollback history | System |
| `SpaceWrite("terminal/*")` | Writing terminal profile, scrollback history | System |
| `NetworkConnect` | SSH/remote terminal connections | User-prompted (per destination) |

#### 8.2.2 Capability Inheritance for Shells

When the terminal spawns a shell, capabilities are attenuated:

```text
Terminal capabilities:
  TerminalCreate       → NOT inherited (shell can't create terminals)
  ProcessCreate        → Inherited (shell can run commands)
  CompositorSurface    → NOT inherited (shell doesn't render)
  SpaceRead("user/*")  → Inherited, possibly narrowed per-tab
  SpaceWrite("user/*") → Inherited, possibly narrowed per-tab
  ClipboardWrite       → Inherited (commands can write clipboard via OSC 52)
  ClipboardRead        → NOT inherited (commands can't read clipboard)
  NetworkConnect       → Inherited if terminal has it

Shell inherits:
  ProcessCreate (delegatable: true, max_children: 64)
  SpaceRead    (attenuated to tab's scope)
  SpaceWrite   (attenuated to tab's scope)
  IpcCreate    (for shell pipes)
  ClipboardWrite (for OSC 52)
```

#### 8.2.3 Enforcement Points

The capability gate enforces at these critical points:

```text
Terminal operations:
  Create session    → check TerminalCreate
  Attach session    → check TerminalAttach + session ownership
  Destroy session   → check TerminalControl or session ownership
  Split pane        → check TerminalCreate (creates new session)
  SSH connect       → check NetworkConnect for destination host

Shell operations:
  Run command       → check ProcessCreate
  Read file         → check SpaceRead for file's space
  Write file        → check SpaceWrite for file's space
  Create pipe       → check IpcCreate
  Write clipboard   → check ClipboardWrite

Programmatic access (other agents):
  Create session    → check TerminalControl
  Send input        → check TerminalControl + session ownership
  Read output       → check TerminalControl + session ownership
```

### 8.3 Space Integration

The terminal uses space storage for persistent configuration and history:

#### 8.3.1 Terminal Space Layout

```text
terminal/                           ← Terminal system space
  profiles/                         ← Terminal profiles
    default.toml                    ← Default profile (colors, font, shell)
    minimal.toml                    ← Minimal profile (no decorations)
    presentation.toml               ← Large font, high contrast
  sessions/                         ← Persistent session state
    session-001.state               ← Serialized session (see §5.8)
    session-002.state
  history/                          ← Command history
    global.history                  ← Cross-session command history
    session-001.scrollback          ← Session scrollback (space-tier)
    session-002.scrollback
  keybindings.toml                  ← Custom key bindings
```

#### 8.3.2 Terminal Profile

```toml
# terminal/profiles/default.toml

[font]
family = "JetBrains Mono"
size = 13.0
ligatures = true
fallbacks = ["Noto Sans Mono", "Symbols Nerd Font"]

[colors]
foreground = "#d4d4d4"
background = "#1e1e1e"
cursor = "#aeafad"
selection_bg = "#264f78"
selection_fg = "#ffffff"

[colors.normal]
black   = "#1e1e1e"
red     = "#f44747"
green   = "#6a9955"
yellow  = "#dcdcaa"
blue    = "#569cd6"
magenta = "#c586c0"
cyan    = "#4ec9b0"
white   = "#d4d4d4"

[colors.bright]
black   = "#808080"
red     = "#f44747"
green   = "#6a9955"
yellow  = "#dcdcaa"
blue    = "#9cdcfe"
magenta = "#c586c0"
cyan    = "#4ec9b0"
white   = "#ffffff"

[cursor]
shape = "block"     # block, underline, bar
blink = true
blink_interval_ms = 530

[scrollback]
memory_lines = 10000
persistent = true
total_limit = 100000

[shell]
program = "/bin/sh"
args = ["-l"]
env = { COLORTERM = "truecolor" }

[padding]
left = 4
right = 4
top = 4
bottom = 4

[window]
opacity = 1.0
decorations = true
tab_bar = true

[multiplexer]
prefix_key = "Ctrl+B"
enable_mouse = true
```

#### 8.3.3 Profile Switching

Users can switch terminal profiles at runtime:

```text
Terminal menu → Settings → Profile: [Default ▾]
  → Minimal (no tab bar, no padding, transparent)
  → Presentation (36pt font, high contrast)
  → Custom...

Profile switch:
1. Terminal reads new profile from space
2. Updates font (triggers glyph cache rebuild)
3. Updates color palette (triggers full surface redraw)
4. Updates cursor shape and blink settings
5. Does NOT restart shell sessions
```

### 8.4 Flow Integration

The Flow subsystem (see [flow.md](../../storage/flow.md)) provides clipboard, drag-and-drop, and data transfer between agents. The terminal integrates with Flow for:

#### 8.4.1 Clipboard

- **Copy from terminal:** Selected text → Flow clipboard channel → available to all agents
- **Paste into terminal:** Flow clipboard channel → terminal → PTY input (with bracketed paste if enabled)
- **OSC 52:** Programs can read/write clipboard via escape sequences (capability-gated)

#### 8.4.2 Drag and Drop

```text
Drag file from file manager → drop on terminal surface:
1. Flow delivers file reference to terminal
2. Terminal converts to shell-safe path:
   /spaces/user/home/documents/file.txt → ~/documents/file.txt
3. Terminal pastes path into PTY input (as if typed)
4. Shell receives: ~/documents/file.txt

Drag text selection from terminal → drop on another agent:
1. Terminal extracts selected text
2. Flow delivers text content to target agent
3. Target agent receives text (e.g., editor opens at that location)
```

#### 8.4.3 Terminal Output Capture

Commands can pipe output to the Flow system for structured use by other agents:

```text
$ cat report.json | flow --to editor
  → Terminal intercepts flow command
  → Terminal sends output content to Flow channel
  → Editor agent receives JSON content
  → Editor opens with report.json content
```

### 8.5 Agent Manifest

The terminal agent's manifest declares its identity, capabilities, and runtime requirements:

```rust
/// Terminal agent manifest.
pub const TERMINAL_MANIFEST: AgentManifest = AgentManifest {
    name: "aios.terminal",
    version: "1.0.0",
    developer: "AIOS",
    trust_level: TrustLevel::System,
    runtime: RuntimeType::Native,

    requested_capabilities: &[
        Capability::TerminalCreate,
        Capability::TerminalAttach,
        Capability::ProcessCreate { delegatable: true },
        Capability::CompositorSurface,
        Capability::ClipboardWrite,
        Capability::ClipboardRead,  // user-prompted
        Capability::SpaceRead("terminal/*"),
        Capability::SpaceWrite("terminal/*"),
        Capability::SpaceRead("user/*"),
        Capability::SpaceWrite("user/*"),
    ],

    resource_limits: KernelResourceLimits {
        max_memory_mb: 256,
        max_threads: 16,
        max_channels: 128,
        max_children: 64,
    },

    lifecycle: Lifecycle::Persistent {
        start_on_boot: false,
        start_on_demand: true,  // launched when user opens terminal
        restart_on_crash: true,
        idle_timeout: None,     // never auto-stop
    },
};
```

### 8.6 Accessibility

The terminal provides accessibility features through the experience layer (see [accessibility.md](../../experience/accessibility.md)):

#### 8.6.1 Screen Reader Integration

```text
Terminal output → Accessibility bridge:
1. New text printed to grid → announce via screen reader
2. Cursor movement → announce current line/word/character
3. Mode changes → announce (e.g., "insert mode", "alternate screen")
4. Bell character → play audio alert + announce "bell"
5. Prompt detection → announce "ready for input"

Challenges:
- Terminal output is a continuous byte stream, not structured content
- Screen readers need semantic boundaries (command output vs. prompt)
- Solution: shell integration markers (OSC 133) provide semantic hints:
  - Prompt start/end markers
  - Command start marker
  - Command output start marker
  → Screen reader can announce "command: cargo build" then "output: ..."
```

#### 8.6.2 Visual Accessibility

| Feature | Implementation |
|---|---|
| Font scaling | Profile setting: size 8-72pt, independent of other apps |
| High contrast | Profile: high-contrast color scheme with WCAG AA+ ratios |
| Cursor visibility | Configurable: block/underline/bar, blink on/off, custom color |
| Minimum contrast | Auto-adjust fg/bg if contrast ratio < 4.5:1 (WCAG AA) |
| Color blindness | Profile: color schemes optimized for protanopia/deuteranopia/tritanopia |
| Reduced motion | Disable cursor blink, smooth scrolling, animations |
| Focus indicators | Bold border on focused pane, high-contrast tab bar |

#### 8.6.3 Motor Accessibility

| Feature | Implementation |
|---|---|
| Sticky keys | Compositor-level support, terminal receives composed keys |
| Key repeat rate | Compositor-configurable per-user |
| Mouse keys | Compositor-level numpad → mouse emulation |
| Voice input | AIRS voice-to-command (§12.1), compositor voice input |
| Switch access | Compositor scanning mode, terminal receives synthesized keys |
| Custom hotkeys | Fully configurable multiplexer prefix and terminal hotkeys |

### 8.7 Audit Logging

Every significant terminal operation is logged to the audit ring and the terminal's audit space:

#### 8.7.1 Audited Events

| Event | Data Logged |
|---|---|
| Session created | Session ID, shell binary, capabilities granted, creation time |
| Session destroyed | Session ID, exit code, duration, destruction reason |
| Session detached | Session ID, detach time, shell still running |
| Session reattached | Session ID, reattach time, from which terminal |
| Shell spawned | Process ID, shell binary, capabilities, environment |
| Command executed | (opt-in only) Command text, working directory, timestamp |
| Capability granted | Token ID, capability type, grantee, expiry |
| Capability revoked | Token ID, capability type, revocation reason |
| Remote connection | Destination host, port, authentication method, timestamp |
| Profile changed | Old profile, new profile, changed fields |

#### 8.7.2 Privacy Controls

Command execution logging is **opt-in only**. By default, the terminal does not log the text of commands entered by the user. Users can enable command logging for:

- All sessions (global preference)
- Specific sessions (per-tab setting)
- Specific capability scopes (e.g., log commands in sandboxed tabs)

When command logging is enabled, the audit trail includes command text, working directory, timestamp, and exit code. This enables the provenance-aware history feature (§11.6).

#### 8.7.3 Audit Space Layout

```text
system/audit/terminal/              ← Terminal audit space
  sessions/
    2026-03-15/                     ← Daily partitioning
      session-001-created.log
      session-001-destroyed.log
  commands/                          ← Only if opt-in
    2026-03-15/
      session-001-commands.log
  security/
    capability-grants.log
    capability-revocations.log
    remote-connections.log
```

Audit data is retained per the system audit retention policy (configurable, default 90 days). Audit space objects are append-only and tamper-evident (content-addressed via SHA-256 hashes).
