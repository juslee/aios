# AIOS Terminal Emulator Architecture

## Compositor-Native Terminal Agent

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [subsystem-framework.md](../platform/subsystem-framework.md) — Universal hardware abstraction (capability gate, sessions, data channels, audit, power, POSIX bridge), [compositor.md](../platform/compositor.md) — Surface lifecycle, semantic hints, input routing, [posix.md](../platform/posix.md) — PTY device translation, FD lifecycle, process semantics, [agents.md](./agents.md) — Agent manifest, capability inheritance, process lifecycle, [ipc.md](../kernel/ipc.md) — Channel mechanics, shared memory, direct switch, [browser.md](./browser.md) — Companion application architecture

**Note:** The terminal emulator implements the subsystem framework. Its capability gate, session model, audit logging, power management, and POSIX bridge follow the universal patterns defined in the framework document. This document covers the terminal-specific design decisions and architecture.

-----

## Document Map

This document was split for navigability. Each sub-document preserves the original section numbers for cross-reference stability.

| Document | Sections | Content |
|---|---|---|
| **This file** | §1, §2, §9–§12 | Core insight, architecture overview, design principles, implementation order, future directions, AI-native terminal |
| [emulation.md](./terminal/emulation.md) | §3 | VT emulation engine: state machine, escape sequence parser, terminal modes, character sets, cell grid, color model, sequence reference |
| [rendering.md](./terminal/rendering.md) | §4 | Text rendering pipeline: font engine, glyph atlas, GPU rendering, cell-to-pixel mapping, damage tracking, scrollback, compositor integration |
| [sessions.md](./terminal/sessions.md) | §5 | Sessions, PTY, and shell: IPC-based PTY abstraction, session lifecycle, shell spawning, job control, signal translation, POSIX bridge |
| [input.md](./terminal/input.md) | §6 | Input handling: keyboard event flow, VT escape translation, mouse reporting, selection, clipboard, secure input, IME support |
| [multiplexer.md](./terminal/multiplexer.md) | §7 | Session multiplexer and remote: detachable PTYs, pane splitting, SSH forwarding, session persistence, reconnection |
| [integration.md](./terminal/integration.md) | §8 | Platform integration: subsystem framework, capability gate, spaces, Flow, agent manifest, accessibility, audit |

-----

## 1. Core Insight

Every terminal emulator today is a standalone application that reimplements the same stack: a VT100 escape sequence parser, a text rendering engine, a pseudo-terminal driver interface, clipboard integration, font management, and scrollback storage. Alacritty, WezTerm, Ghostty, kitty — each one builds these from scratch, because the operating system provides nothing useful for terminal rendering. The OS gives you a PTY device file and raw byte streams. Everything else — understanding escape sequences, rendering glyphs, managing sessions, handling input — is the application's problem.

AIOS doesn't have this problem. The compositor already renders text surfaces with GPU acceleration. The IPC subsystem already provides bidirectional channels with zero-copy shared memory. The capability system already enforces process isolation. The POSIX bridge already translates file descriptors to IPC channels. The agent framework already manages process lifecycle and capability inheritance. The terminal emulator doesn't need to rebuild any of that.

**The AIOS terminal emulator is not a standalone application. It is a compositor-native agent that connects a VT emulation engine to existing OS services.** The VT parser translates escape sequences into cell grid updates. The rendering pipeline maps cells to glyphs on a compositor surface. The PTY is an IPC channel pair, not a device file. Shell processes are child agents with inherited capabilities. Scrollback history lives in a space. Session persistence survives compositor restarts.

This decomposition means the terminal emulator is remarkably small. The emulation engine (§3) and input translation (§6) contain the terminal-specific logic. Everything else — rendering, process management, storage, security, accessibility — delegates to OS subsystems that already exist for every other application.

### 1.1 Responsibility Decomposition

What a traditional terminal emulator does, and where each responsibility lives in AIOS:

```text
Traditional Terminal                  AIOS Decomposition
──────────────────                    ──────────────────
PTY allocation (/dev/ptmx)        →  IPC channel pair (kernel)
PTY master/slave byte streams     →  Bidirectional IPC channels with shared memory
VT100 escape sequence parsing     ←  STAYS in terminal (domain-specific)
Cell grid management              ←  STAYS in terminal (domain-specific)
Font loading and shaping          →  OS font service (shared with all text-rendering agents)
Glyph rasterization               →  OS glyph atlas (compositor-managed, GPU-accelerated)
Text rendering to pixels          →  Compositor surface (shared buffer + damage reporting)
Clipboard copy/paste              →  Flow subsystem (clipboard channel)
Scrollback storage                →  Space object (searchable, syncable, persistent)
Session management                →  Subsystem framework (session lifecycle, capability gate)
Process spawning (fork/exec)      →  Agent framework (ProcessCreate capability)
Signal delivery (Ctrl+C, Ctrl+Z)  →  Notification subsystem (atomic signal + mask wake)
Window management (resize, move)  →  Compositor (surface lifecycle, configure events)
Input handling                    ←  STAYS in terminal (VT escape translation)
Configuration (colors, fonts)     →  Space object (terminal profile)
Multiplexing (tmux-like)          ←  STAYS in terminal (session broker)
SSH/remote sessions               →  Networking subsystem + terminal session forwarding
──────────────────────────────────────────────────────────
```

The terminal retains only the parts that require terminal-specific knowledge: VT emulation, cell grid management, input translation, and session multiplexing. Everything else delegates to an OS service.

-----

## 2. Architecture: Terminal as a Compositor-Native Agent

The terminal emulator is a set of cooperating components, most of which are existing OS services:

```text
┌─────────────────────────────────────────────────────────────────┐
│                    Terminal Agent (System Trust)                  │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │ VT Emulation │  │   Session    │  │   Input Translator   │  │
│  │   Engine     │  │  Multiplexer │  │                      │  │
│  │              │  │              │  │  Keycode → VT escape  │  │
│  │  Parser ──┐  │  │  Broker ──┐  │  │  Mouse → SGR report  │  │
│  │  Modes    │  │  │  Sessions │  │  │  IME → UTF-8         │  │
│  │  Grid  ◄──┘  │  │  Panes   │  │  │                      │  │
│  └──────┬───────┘  └────┬─────┘  └──────────┬───────────────┘  │
│         │               │                     │                  │
│         ▼               ▼                     │                  │
│  ┌──────────────────────────────┐             │                  │
│  │     Rendering Pipeline       │◄────────────┘                  │
│  │  Grid → Glyphs → Surface    │                                 │
│  └──────────────┬───────────────┘                                │
│                 │                                                 │
└─────────────────┼─────────────────────────────────────────────────┘
                  │
    ══════════════╪══════════ OS Service Boundary ═══════════════
                  │
         ┌────────┴────────┐
         │                 │
    ┌────▼────┐    ┌───────▼───────┐    ┌───────────────┐
    │Compositor│    │  IPC Channels │    │ Agent Runtime  │
    │ Surface  │    │  (PTY pairs)  │    │ (shell spawn)  │
    │ + Input  │    │  + Shared Mem │    │ + cap inherit  │
    └─────────┘    └───────┬───────┘    └───────┬───────┘
                           │                     │
                    ┌──────▼──────┐       ┌──────▼──────┐
                    │ Shell Agent │       │ Command     │
                    │ (FreeBSD sh)│       │ Agents      │
                    │ (bash, zsh) │       │ (ls, grep)  │
                    └─────────────┘       └─────────────┘
```

### 2.1 Data Flow

Terminal I/O follows two paths:

**Output path** (shell → screen):

```text
Shell writes bytes → IPC channel → VT parser → Cell grid update
  → Dirty cells identified → Glyph lookup → Surface buffer write
  → Damage region reported → Compositor composites frame
```

**Input path** (keyboard → shell):

```text
Compositor routes InputEvent → Terminal agent receives KeyboardEvent
  → Input translator converts to VT escape sequence (or raw UTF-8)
  → Bytes written to PTY IPC channel → Shell reads input
```

Both paths use zero-copy shared memory where possible. The PTY channels use inline messages for small payloads (< 256 bytes) and shared memory regions for bulk transfers (scrollback dumps, large pastes).

### 2.2 Component Summary

| Component | Location | Purpose |
|---|---|---|
| VT Emulation Engine | Terminal agent | Parse escape sequences, maintain cell grid state |
| Rendering Pipeline | Terminal agent → Compositor | Map cells to glyphs, write to surface buffer |
| Input Translator | Terminal agent | Convert keycodes to VT escape sequences |
| Session Multiplexer | Terminal agent | Manage multiple PTY sessions, pane layout |
| PTY Channel Pair | Kernel IPC | Bidirectional byte stream between terminal and shell |
| Shell Agent | Agent Runtime | FreeBSD sh, bash, zsh — child process with inherited caps |
| Compositor Surface | Compositor | GPU-composited text surface with damage tracking |
| Scrollback Space | Space Storage | Persistent, searchable terminal history |
| Terminal Profile | Space Storage | Colors, fonts, keybindings, shell preference |

-----

## 9. Design Principles

### 9.1 Terminal is Infrastructure, Not an Application

The terminal emulator is a system agent (TrustLevel::System), not a user-installed application. It ships with the OS, is always available, and provides the foundational text interaction surface. Other agents (shells, CLI tools, development environments) build on top of it.

### 9.2 IPC-First, Not Device-First

Traditional Unix terminals are built around device files (`/dev/tty`, `/dev/pts/*`). AIOS terminals are built around IPC channels. The POSIX bridge translates device file semantics to IPC operations for compatibility, but the native interface is always IPC. This means:

- No `ioctl()` needed for terminal attributes — the terminal negotiates directly with the shell via typed IPC messages
- No `termios` struct manipulation — terminal modes are managed by the VT emulation engine
- Window resize is a compositor configure event, not a `SIGWINCH` signal

### 9.3 Capability-Bounded Shell Processes

Every shell spawned by the terminal agent inherits an attenuated subset of the terminal's capabilities. A shell cannot exceed its parent terminal's access. The user can further restrict per-tab capabilities (e.g., a "sandboxed shell" tab with only `/tmp` access).

### 9.4 Sessions Survive Compositor Restarts

Terminal sessions are decoupled from the compositor surface. If the compositor restarts (crash, display server switch, monitor hotplug), the terminal agent reconnects its sessions to new surfaces without losing shell state. The session multiplexer maintains PTY connections independently of display state.

### 9.5 Scrollback is a Space Object

Terminal scrollback history is stored as a space object, not an in-memory ring buffer. This means scrollback is:

- **Persistent** — survives terminal restart
- **Searchable** — AIRS can search terminal history semantically
- **Syncable** — can sync across devices via Space Mesh Protocol
- **Quotaed** — respects space storage quotas, not unbounded memory growth

### 9.6 Security by Default

- Secure input mode suppresses logging and screenshots during password entry
- Audit trail records session creation, process attachment, and capability grants
- No terminal escape sequence can escalate capabilities or access resources outside the shell's capability set
- Terminal title and window hints are sanitized to prevent escape sequence injection

-----

## 10. Implementation Order

The terminal emulator is implemented across multiple development phases, building complexity incrementally.

```text
Phase 7:   Terminal emulator (basic)
           ├── VT100 escape sequence parser (core CSI/SGR subset)
           ├── Cell grid with 16-color support
           ├── Compositor surface rendering (CPU-rasterized, basic font)
           ├── Keyboard input → PTY IPC channel
           ├── Single session (one shell per terminal window)
           ├── TerminalCreate capability type
           └── Test: interactive shell session in QEMU

Phase 8:   Terminal emulator (enhanced)
           ├── Full xterm-256color emulation
           ├── GPU-accelerated glyph rendering (glyph atlas)
           ├── Mouse reporting (SGR mode)
           ├── Scrollback buffer (space-backed)
           ├── Selection and clipboard (Flow integration)
           └── Terminal profile (colors, font, shell preference)

Phase 10:  Session multiplexer
           ├── Multi-tab sessions (multiple PTYs per terminal window)
           ├── Pane splitting (horizontal/vertical within a surface)
           ├── Session detach/reattach
           ├── Session persistence across compositor restarts
           └── Audit logging (session lifecycle events)

Phase 15:  POSIX compatibility
           ├── /dev/tty and /dev/pts/* mapping via POSIX bridge
           ├── termios translation to VT mode state
           ├── SIGWINCH delivery on resize
           ├── Job control signal translation (SIGINT, SIGTSTP, SIGCONT)
           └── Process group semantics

Phase 18:  Remote terminals
           ├── SSH PTY forwarding (networking subsystem integration)
           ├── Remote session reconnection (Mosh-style)
           ├── Session migration between devices
           └── Encrypted session state transfer

Phase 22:  AI-native features
           ├── AIRS context-aware command suggestions
           ├── Semantic scrollback search
           ├── Anomaly detection for suspicious commands
           ├── Intelligent output parsing and error highlighting
           └── Voice-to-command input
```

-----

## 11. Future Directions

### 11.1 Spatial Terminal

Extend the terminal beyond a 2D character grid. A spatial terminal renders command output as structured blocks that can be folded, reordered, and linked. Each command invocation becomes a discrete region with its own scrollback, exit status indicator, and execution time. Related commands group visually, forming a navigable execution history rather than an undifferentiated stream of text.

### 11.2 Terminal-as-a-Service

Expose terminal sessions as a capability-gated service that other agents can use programmatically. A CI agent could spawn a terminal session, execute a build script, capture structured output, and release the session — all without user interaction. This extends the terminal from a user-facing tool to a system automation primitive.

### 11.3 Semantic Paste

When pasting text into a terminal, the terminal can collaborate with AIRS to understand the content type and apply appropriate transformations. Pasting a file path from a different space resolves to the correct local path. Pasting a code snippet detects the language and suggests the right interpreter. Pasting a URL into a shell invokes the appropriate download tool with safety checks.

### 11.4 Collaborative Terminal

Multiple users connected to the same terminal session with different capability levels. One user types commands (write access), another observes (read-only). Capability tokens control who can type, who can see output, and who can detach the session. This replaces ad-hoc screen-sharing with a capability-gated collaboration model.

### 11.5 Terminal Widgets

Embed interactive widgets within the terminal output stream. A progress bar renders as a native compositor element overlaid on the terminal surface, not as a series of ASCII redraws. A chart renders as a GPU-accelerated graphic. A file picker opens as a compositor popover. The terminal becomes a hybrid text/graphical interface while remaining compatible with pure-text output for remote sessions.

### 11.6 Provenance-Aware History

Every command in the terminal history carries provenance metadata: which agent suggested it, which task it was part of, which space objects it accessed, and what its exit status was. Users can search history not just by text content but by context: "show me all commands I ran while debugging the auth issue last Tuesday."

-----

## 12. AI-Native Terminal

AIOS's AI Runtime System (AIRS) integrates with the terminal at multiple levels, transforming it from a passive byte-stream renderer into an intelligent interaction surface.

### 12.1 AIRS-Dependent Features

These features require semantic understanding from the AI Runtime and are unavailable when AIRS is offline:

**Context-Aware Command Suggestions.** AIRS observes the user's current task context (open files, recent git operations, active project) and suggests relevant shell commands. If the user is reviewing a pull request, AIRS might suggest `git diff main...feature-branch` or `cargo test --lib`. Suggestions appear as ghost text in the terminal input line, accepted with Tab.

**Semantic Scrollback Search.** Instead of text-matching `grep` over scrollback, AIRS enables queries like "find where the build failed" or "show the last database migration output." AIRS understands command boundaries, exit codes, and error patterns, returning semantically relevant scrollback regions.

**Output Understanding.** AIRS parses terminal output to identify errors, warnings, stack traces, and actionable items. A failed compilation highlights the error file and line, offering to open it in the editor. A test failure summary links to the specific test. This is not regex pattern matching — AIRS understands output structure across tools and languages.

**Anomaly Detection.** AIRS monitors command patterns for anomalies: unexpected `sudo` usage, commands targeting unfamiliar network hosts, bulk file deletions, or commands that don't match the user's typical workflow. Anomalies trigger a confirmation prompt, not a block — the user retains control but gains awareness.

**Natural Language Commands.** The user types a natural language description ("find all Python files modified this week larger than 1MB") and AIRS translates it to the correct shell command (`find . -name "*.py" -mtime -7 -size +1M`), showing the translation for approval before execution.

### 12.2 Kernel-Internal ML Features

These features use purely statistical models (frozen decision trees, frequency tables) that run without AIRS dependency:

**Command Frequency Prediction.** A per-user frequency table tracks command usage patterns. The terminal pre-warms shell completion caches for commonly used commands and pre-allocates resources for expected workloads. This is a simple counter, not a neural model.

**Idle Detection.** A statistical model detects when the terminal is idle (no input for N seconds, no output from running process) and signals the power manager to reduce display refresh rate for the terminal surface. The model uses exponentially weighted moving averages, not neural inference.

**Buffer Size Adaptation.** The PTY shared memory buffer size adapts based on measured throughput. High-throughput commands (compilation output, large file `cat`) trigger buffer growth. Interactive commands (shell prompt, text editing) shrink buffers to reduce latency. This is a simple heuristic, not ML.

### 12.3 Application-Level AI Features

These features run in the terminal agent itself, without kernel or AIRS involvement:

**Syntax Highlighting.** The terminal identifies command output type (JSON, XML, log format, diff, stack trace) and applies syntax highlighting to the output cells. This uses pattern matching and lightweight parsers, not AI models.

**URL Detection.** Clickable URL detection in terminal output with hover preview. URLs are identified via regex, verified via capability check (does the terminal have network access to this domain?), and rendered as interactive links.

**Smart Selection.** Double-click selects a word; triple-click selects a line. But the terminal also understands structural boundaries: double-clicking a file path selects the entire path. Double-clicking a quoted string selects the full quoted content. This uses a rule-based boundary detection engine.

-----

## Cross-Reference Index

| Section | Sub-Document | External References |
|---|---|---|
| §1 Core Insight | This file | [architecture.md](../project/architecture.md) |
| §2 Architecture | This file | [agents.md](./agents.md) §2, [ipc.md](../kernel/ipc.md) §3-§4 |
| §3 VT Emulation | [emulation.md](./terminal/emulation.md) | — |
| §4 Rendering | [rendering.md](./terminal/rendering.md) | [compositor/protocol.md](../platform/compositor/protocol.md) §3-§4, [compositor/rendering.md](../platform/compositor/rendering.md) §5 |
| §5 Sessions & PTY | [sessions.md](./terminal/sessions.md) | [ipc.md](../kernel/ipc.md) §3, [posix.md](../platform/posix.md) §5-§7, [agents.md](./agents.md) §2 |
| §6 Input | [input.md](./terminal/input.md) | [compositor/input.md](../platform/compositor/input.md) §7.1-§7.6 |
| §7 Multiplexer | [multiplexer.md](./terminal/multiplexer.md) | [networking.md](../platform/networking.md) §5 |
| §8 Integration | [integration.md](./terminal/integration.md) | [subsystem-framework.md](../platform/subsystem-framework.md) §2-§8, [capabilities.md](../security/model/capabilities.md) §3 |
| §9 Design Principles | This file | [subsystem-framework.md](../platform/subsystem-framework.md) §1 |
| §10 Impl Order | This file | [development-plan.md](../project/development-plan.md) §8 |
| §11 Future Directions | This file | — |
| §12 AI-Native | This file | [airs.md](../intelligence/airs.md), [context-engine.md](../intelligence/context-engine.md) |
