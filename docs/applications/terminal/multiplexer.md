# AIOS Terminal Session Multiplexer & Remote

Part of: [terminal.md](../terminal.md) — Terminal Emulator Architecture
**Related:** [sessions.md](./sessions.md) — PTY session lifecycle, [input.md](./input.md) — Input routing to active pane, [integration.md](./integration.md) — Capability gate for multiplexer operations

-----

## 7. Session Multiplexer & Remote

The session multiplexer manages multiple PTY sessions within a single terminal agent, providing tab management, pane splitting, session detach/reattach, and remote terminal forwarding. Unlike tmux or screen which run as separate server processes, the AIOS multiplexer is built into the terminal agent and uses native IPC channels — no Unix socket protocol, no escape sequence hijacking.

### 7.1 Multiplexer Architecture

#### 7.1.1 Component Overview

```text
Terminal Agent
├── Session Broker
│   ├── Session registry (all PTY sessions, active + detached)
│   ├── Session creation / destruction
│   ├── Detach / reattach protocol
│   └── Session persistence (space-backed)
│
├── Layout Manager
│   ├── Tab bar state (which tabs, active tab)
│   ├── Pane tree (split layout within a tab)
│   ├── Focus tracking (which pane receives input)
│   └── Resize propagation (surface resize → pane resize → PTY resize)
│
└── Input Router
    ├── Multiplexer hotkeys (Ctrl+B prefix, or configurable)
    ├── Tab switching (Ctrl+B + n)
    ├── Pane navigation (Ctrl+B + arrow)
    └── Passthrough (everything else → active pane's PTY)
```

#### 7.1.2 Session Broker

The session broker is the multiplexer's core: it owns all PTY sessions and mediates access.

```rust
/// The session broker manages all PTY sessions for this terminal agent.
pub struct SessionBroker {
    /// All sessions (active, detached, and exited).
    sessions: Vec<PtySession>,
    /// Maximum concurrent sessions.
    max_sessions: usize,
    /// Next session ID.
    next_id: SessionId,
    /// Space reference for session persistence.
    persistence_space: SpaceObjectId,
}

impl SessionBroker {
    /// Create a new PTY session with the given shell and capabilities.
    pub fn create_session(
        &mut self,
        shell: &str,
        env: &[(String, String)],
        caps: CapabilitySet,
    ) -> Result<SessionId, SessionError>;

    /// Destroy a session (kill shell, release channels).
    pub fn destroy_session(&mut self, id: SessionId) -> Result<(), SessionError>;

    /// Detach a session (keep shell alive, release terminal binding).
    pub fn detach_session(&mut self, id: SessionId) -> Result<(), SessionError>;

    /// Reattach to a detached session.
    pub fn reattach_session(&mut self, id: SessionId) -> Result<(), SessionError>;

    /// List all sessions with their states.
    pub fn list_sessions(&self) -> Vec<SessionInfo>;

    /// Serialize all session metadata to space storage.
    pub fn persist(&self) -> Result<(), StorageError>;
}
```

### 7.2 Window/Pane Splitting

Within a single tab, the terminal can split the visible area into multiple panes, each connected to a different PTY session.

#### 7.2.1 Pane Tree

Panes are organized as a binary tree of splits:

```text
Tab layout:
┌──────────────────────────────────────────┐
│                                           │
│  Pane 1 (session A)  │  Pane 2 (sess B)  │
│                      │                    │
│  $ cargo build       │  $ git log         │
│  Compiling...        │  commit abc123     │
│                      │  commit def456     │
│                      │                    │
│──────────────────────┤                    │
│                      │                    │
│  Pane 3 (session C)  │                    │
│                      │                    │
│  $ htop              │                    │
│                      │                    │
└──────────────────────────────────────────┘

Pane tree:
        VSplit(50%)
       /           \
  HSplit(60%)    Pane 2
  /         \
Pane 1    Pane 3
```

```rust
/// A node in the pane layout tree.
pub enum PaneNode {
    /// A leaf pane containing a PTY session.
    Pane {
        session_id: SessionId,
        /// This pane's dimensions (computed from parent split).
        cols: u16,
        rows: u16,
    },
    /// A horizontal split (top/bottom).
    HSplit {
        ratio: f32,  // 0.0-1.0, fraction for top pane
        top: Box<PaneNode>,
        bottom: Box<PaneNode>,
    },
    /// A vertical split (left/right).
    VSplit {
        ratio: f32,  // 0.0-1.0, fraction for left pane
        left: Box<PaneNode>,
        right: Box<PaneNode>,
    },
}
```

#### 7.2.2 Split Operations

| Operation | Hotkey (default) | Effect |
|---|---|---|
| Split vertical | `Ctrl+B %` | Split active pane left/right |
| Split horizontal | `Ctrl+B "` | Split active pane top/bottom |
| Close pane | `Ctrl+B x` | Close active pane (kill or detach session) |
| Next pane | `Ctrl+B o` | Focus next pane (round-robin) |
| Previous pane | `Ctrl+B ;` | Focus previous pane |
| Pane up | `Ctrl+B ↑` | Focus pane above |
| Pane down | `Ctrl+B ↓` | Focus pane below |
| Pane left | `Ctrl+B ←` | Focus pane to the left |
| Pane right | `Ctrl+B →` | Focus pane to the right |
| Resize pane | `Ctrl+B Alt+↑/↓/←/→` | Grow/shrink active pane |
| Swap panes | `Ctrl+B {` / `Ctrl+B }` | Swap active pane with prev/next |
| Zoom pane | `Ctrl+B z` | Toggle pane zoom (full tab / split view) |
| Break pane | `Ctrl+B !` | Move pane to its own tab |

#### 7.2.3 Pane Rendering

Each pane renders its own VT engine's grid to a sub-region of the terminal surface:

```text
Surface rendering:
1. Compute pane pixel bounds from tree layout and surface dimensions
2. For each pane:
   a. Set clip rect to pane bounds
   b. Render pane's grid (using §4 rendering pipeline)
   c. Draw pane border (1px line between panes)
3. Draw focus indicator (highlighted border on active pane)
4. Report damage (union of all dirty pane regions)
```

Pane borders consume 1 pixel between adjacent panes. The cell grid dimensions for each pane account for border pixels:

```text
pane_cols = (pane_pixel_width - border_pixels) / cell_width
pane_rows = (pane_pixel_height - border_pixels) / cell_height
```

### 7.3 Session Detach/Reattach Protocol

#### 7.3.1 Detach Flow

```text
User: Ctrl+B d  (detach)
  ↓
1. Session broker marks all active sessions as Detached
2. For each session:
   a. Stop reading from output channel
   b. Shell continues running (output buffers in channel ring buffer)
   c. Persist session metadata to space
3. Terminal agent can now:
   a. Close the window (surface destroyed, agent exits)
   b. Attach to different sessions
   c. Create new sessions
4. Audit log: all sessions detached
```

#### 7.3.2 Reattach Flow

```text
User opens terminal, types: attach  (or terminal UI shows detached sessions)
  ↓
1. Session broker lists detached sessions from space storage
2. User selects session(s) to reattach
3. For each selected session:
   a. Verify shell process is still alive (IPC channel still valid)
   b. Drain buffered output from channel ring buffer
   c. Create VT engine, process buffered output to reconstruct grid
   d. Mark session as Active
4. Layout manager restores saved pane layout (from space storage)
5. Rendering pipeline draws all panes to surface
6. Audit log: sessions reattached
```

#### 7.3.3 Cross-Terminal Reattach

A session detached from Terminal A can be reattached from Terminal B (different window, different device after sync):

```text
Terminal A: creates session S1, detaches
Terminal B: lists detached sessions, sees S1, reattaches

Requirements:
- Terminal B must have sufficient capabilities to own session S1
- Session S1's channel endpoints must be transferable
- If Terminal B is on a different device: session state must be synced via Space Mesh Protocol
```

### 7.4 SSH PTY Forwarding

Remote terminal sessions are created by the multiplexer in cooperation with the networking subsystem:

#### 7.4.1 Remote Session Creation

```text
1. User requests: ssh user@remote.host
2. Terminal multiplexer creates a local PtySession (session_type = Remote)
3. Terminal delegates connection to SSH agent:
   a. SSH agent authenticates with remote host
   b. SSH agent establishes encrypted channel
   c. SSH agent allocates remote PTY
4. Local PTY channels bridge to remote PTY:
   - Local input channel → SSH encrypt → remote input
   - Remote output → SSH decrypt → local output channel
5. Terminal renders remote output identically to local output
6. Multiplexer tracks session as remote (for status display)
```

#### 7.4.2 SSH Integration Architecture

```text
Terminal Agent ←→ Local PTY Channels ←→ SSH Bridge Agent ←→ Network
                                                              ↕
                                                        Remote Host
                                                              ↕
                                                        Remote Shell
```

The SSH bridge agent handles:

- Key exchange and authentication (via credential vault in space storage)
- Channel encryption/decryption
- PTY size forwarding (local resize → remote SIGWINCH equivalent)
- Port forwarding (if requested)
- Agent forwarding (if requested)

The terminal itself does not implement SSH. It treats the SSH session as another PTY session with a different transport.

### 7.5 Remote Session Reconnection and State Recovery

For unreliable networks, the multiplexer supports persistent remote sessions:

#### 7.5.1 Reconnection Protocol

```text
Connection lost:
1. SSH bridge agent detects connection failure
2. Multiplexer marks remote session as Reconnecting (not Detached)
3. Terminal displays "Connection lost, reconnecting..." overlay on session pane
4. SSH bridge agent attempts reconnection:
   a. Try same IP/port (network glitch recovery)
   b. Try DNS re-resolution (IP change)
   c. Exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s (max)
5. On reconnection:
   a. Re-authenticate (session token if available, otherwise full auth)
   b. Request full screen state from remote (Mosh-style sync)
   c. Update local VT engine with remote state
   d. Resume normal I/O
6. On timeout (configurable, default 5 minutes):
   a. Mark session as Disconnected
   b. User can manually retry or close
```

#### 7.5.2 Local Echo During Latency

For high-latency connections, the terminal can enable local echo prediction:

```text
1. User types a character
2. Terminal immediately renders the character in the cell grid (gray/dimmed)
3. Terminal sends the character to remote via SSH
4. Remote shell processes and echoes back
5. Terminal receives echo, replaces predicted character with confirmed character
6. If prediction was wrong (e.g., tab completion changed the text):
   Terminal redraws from the remote's authoritative state
```

This provides instant visual feedback even on connections with hundreds of milliseconds of latency. The prediction is purely visual — the remote shell always has authoritative state.

### 7.6 Multiplexer IPC Protocol

The multiplexer uses a control channel separate from the PTY data channels to manage sessions:

#### 7.6.1 Control Messages

```rust
/// Messages on the multiplexer control channel.
pub enum MuxControlMessage {
    /// Create a new session.
    CreateSession {
        shell: [u8; 64],
        env_count: u16,
    },
    /// Destroy a session.
    DestroySession { session_id: SessionId },
    /// Detach a session.
    DetachSession { session_id: SessionId },
    /// Reattach a session.
    ReattachSession { session_id: SessionId },
    /// List all sessions.
    ListSessions,
    /// Split a pane.
    SplitPane {
        direction: SplitDirection,
        session_id: SessionId,
    },
    /// Close a pane.
    ClosePane { pane_id: PaneId },
    /// Resize a pane.
    ResizePane {
        pane_id: PaneId,
        delta_cols: i16,
        delta_rows: i16,
    },
    /// Focus a pane.
    FocusPane { pane_id: PaneId },
    /// Response: session created successfully.
    SessionCreated { session_id: SessionId },
    /// Response: session status list.
    SessionList { sessions: [SessionInfo; 16] },
    /// Response: operation failed.
    Error { code: u16 },
}

pub enum SplitDirection {
    Horizontal,
    Vertical,
}
```

The control channel is used by:

- The terminal's own hotkey handler (user presses Ctrl+B commands)
- External agents that want to manage terminal sessions programmatically (e.g., a CI agent creating a build terminal)
- The terminal profile system (restoring layout from saved state)

#### 7.6.2 Programmatic Session Management

Other agents can interact with the terminal multiplexer via the control channel, enabling automation:

```text
CI Agent wants to run a build:
1. CI Agent → Terminal: CreateSession { shell: "bash", ... }
2. Terminal → CI Agent: SessionCreated { session_id: 5 }
3. CI Agent → Session 5 input channel: "cargo build 2>&1\n"
4. CI Agent reads from Session 5 output channel: [build output]
5. CI Agent → Terminal: DestroySession { session_id: 5 }
```

This is the "terminal-as-a-service" pattern described in §11.2 (Future Directions). The capability gate (§8.2) controls which agents can create and manage sessions.

-----

### 7.7 Error Recovery

The multiplexer isolates failures so that one pane's crash never affects other panes or sessions.

#### 7.7.1 Pane Crash Isolation

When a shell in one pane crashes:

```text
1. Session broker receives shell exit notification
2. Affected session marked as "exited" (not destroyed)
3. Layout manager preserves pane slot with "[exited: code N]" indicator
4. Other panes continue running — no state change, no interruption
5. User can:
   a. Press Enter to start a new shell in the same pane
   b. Close the pane (layout reflows remaining panes)
   c. Leave it as-is (scrollback remains accessible)
```

#### 7.7.2 Layout Persistence

The pane layout (split directions, sizes, tab order) is persisted to the terminal's space object:

- Layout state saved on every structural change (split, close, resize, tab reorder)
- Survives terminal agent restart — broker reads saved layout on startup
- Each tab's layout tree is serialized independently

#### 7.7.3 Session Recovery on Agent Restart

When the terminal agent restarts (crash, update):

```text
1. Broker reads persisted session metadata from space
2. For each saved session:
   a. Check if shell process is still running (POSIX process groups
      survive agent restart if kernel manages process lifecycle)
   b. If running: reattach to existing PTY channels
   c. If exited: mark session as "[exited]", preserve scrollback from space
3. Restore layout tree from persisted state
4. Request new compositor surface
5. Full redraw of all visible panes
```

Sessions whose shells are still running reconnect seamlessly. Sessions whose shells exited during the restart show the exit indicator with preserved scrollback.

#### 7.7.4 Resource Cleanup

The session broker performs periodic cleanup of dead resources:

| Resource | Cleanup Trigger | Action |
|---|---|---|
| Exited sessions | User closes pane, or 24-hour timeout | Release channels, shared memory, notification objects |
| Detached sessions | Configurable timeout (default: 7 days) | Persist scrollback to space, release memory-tier resources |
| Orphaned channels | Session broker startup scan | Destroy channels with no associated session |
| Stale space objects | Session broker startup scan | Delete session metadata for sessions older than retention period |
