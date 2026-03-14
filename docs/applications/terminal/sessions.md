# AIOS Terminal Sessions, PTY & Shell Integration

Part of: [terminal.md](../terminal.md) — Terminal Emulator Architecture
**Related:** [emulation.md](./emulation.md) — VT parser consumes PTY output, [input.md](./input.md) — Input translator produces PTY input, [multiplexer.md](./multiplexer.md) — Multi-session management, [integration.md](./integration.md) — Capability gate and audit

-----

## 5. Sessions, PTY & Shell Integration

The terminal session model replaces Unix's device-file-based pseudo-terminal with an IPC-channel-based design. A "PTY" in AIOS is a pair of IPC channels connecting the terminal agent to a shell process, with the kernel providing no TTY-specific device driver. All terminal semantics (line discipline, echo, signal generation) are handled in userspace by the VT emulation engine and shell.

### 5.1 PTY Abstraction: IPC-Based

#### 5.1.1 Traditional Unix PTY vs AIOS PTY

```text
Unix PTY:
  Terminal emulator ←→ /dev/ptmx (master) ←→ kernel TTY layer ←→ /dev/pts/N (slave) ←→ Shell
                                                    ↑
                                              Line discipline
                                              (echo, signals,
                                               canonical mode)

AIOS PTY:
  Terminal agent ←→ IPC Channel A ←→ Shell agent
                    IPC Channel B ←→ (reverse direction)
                    Shared Memory  ←→ (bulk transfers)
                         ↑
                   No kernel TTY layer.
                   VT engine handles echo, modes.
                   Notification channel handles signals.
```

The key differences:

| Aspect | Unix PTY | AIOS PTY |
|---|---|---|
| Transport | Device file (`/dev/pts/N`) | IPC channel pair |
| Line discipline | Kernel (`termios`) | VT emulation engine (userspace) |
| Echo | Kernel echoes input to output | Terminal renders input locally |
| Signal generation | Kernel generates SIGINT on Ctrl+C | Terminal sends notification |
| Window size query | `ioctl(TIOCGWINSZ)` | IPC query message |
| Capability control | Unix permissions (owner/group) | Capability tokens |
| Multiplexing | File descriptor passing | Channel delegation |

#### 5.1.2 PTY Channel Structure

```rust
/// A pseudo-terminal session connecting a terminal agent to a shell.
pub struct PtySession {
    /// Session identifier (unique within the terminal agent).
    pub id: SessionId,

    /// Channel from terminal → shell (keyboard input, resize events).
    pub input_channel: ChannelId,

    /// Channel from shell → terminal (program output).
    pub output_channel: ChannelId,

    /// Notification channel for signals (Ctrl+C, Ctrl+Z, window resize).
    pub signal_notify: NotificationId,

    /// Shared memory region for bulk data transfer.
    pub bulk_buffer: Option<SharedMemoryId>,

    /// Terminal dimensions (columns × rows).
    pub cols: u16,
    pub rows: u16,

    /// Shell process ID.
    pub shell_pid: ProcessId,

    /// Session state.
    pub state: PtySessionState,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PtySessionState {
    /// Session is active — shell is running.
    Active,
    /// Session is detached — shell is alive but no terminal is connected.
    Detached,
    /// Shell has exited — session is awaiting cleanup.
    Exited { exit_code: i32 },
    /// Session is suspended (power management).
    Suspended,
}
```

#### 5.1.3 Channel Message Types

The PTY channels carry typed messages. Each variant is serialized as a 1-byte tag followed by its fields, fitting within the 256-byte `RawMessage.data` field:

```rust
/// Messages from terminal → shell (input channel).
/// Wire format: [tag: u8 | payload...], total ≤ 256 bytes.
#[repr(u8)]
pub enum PtyInputMessage {
    /// Keyboard input bytes (UTF-8 or VT escape sequences).
    /// Wire: [0x01 | len: u16 | bytes: [u8; len]] — max 248 payload bytes.
    Data { bytes: [u8; 248], len: u16 } = 0x01,
    /// Terminal window has been resized.
    /// Wire: [0x02 | cols: u16 | rows: u16] — 5 bytes total.
    Resize { cols: u16, rows: u16 } = 0x02,
    /// Terminal is requesting shell status.
    /// Wire: [0x03] — 1 byte total.
    StatusQuery = 0x03,
}

/// Messages from shell → terminal (output channel).
/// Wire format: [tag: u8 | payload...], total ≤ 256 bytes.
#[repr(u8)]
pub enum PtyOutputMessage {
    /// Program output bytes.
    /// Wire: [0x01 | len: u16 | bytes: [u8; len]] — max 248 payload bytes.
    Data { bytes: [u8; 248], len: u16 } = 0x01,
    /// Shell has changed working directory (for OSC 7).
    /// Wire: [0x02 | len: u16 | path: [u8; len]] — max 240 path bytes.
    WorkingDirectory { path: [u8; 240], len: u16 } = 0x02,
    /// Shell has set the terminal title (for OSC 0/2).
    /// Wire: [0x03 | len: u16 | title: [u8; len]] — max 240 title bytes.
    Title { title: [u8; 240], len: u16 } = 0x03,
    /// Shell has exited.
    /// Wire: [0x04 | code: i32] — 5 bytes total.
    Exit { code: i32 } = 0x04,
}
```

The largest variant (`Data` with 248-byte payload) uses 1 (tag) + 2 (len) + 248 (bytes) = 251 bytes, well within the 256-byte `RawMessage.data` limit. For bulk transfers (large file pastes, scrollback dumps), the shared memory region is used instead.

-----

### 5.2 Session Lifecycle

#### 5.2.1 Session Creation

```text
1. Terminal agent receives "new session" request (user action or API call)
2. Terminal creates IPC channel pair (input + output) via ChannelCreate syscall
3. Terminal creates notification channel for signals
4. Terminal optionally creates shared memory region for bulk transfer
5. Terminal spawns shell process via ProcessCreate syscall:
   - Shell binary: user's preferred shell (from terminal profile)
   - Capabilities: attenuated from terminal's capability set
   - Environment: TERM=xterm-256color, COLORTERM=truecolor, SHELL=<path>
   - Stdin/stdout/stderr: connected to PTY channel pair
6. Terminal creates VT emulation engine instance for this session
7. Terminal starts reading from output channel (shell → terminal)
8. Session state → Active
```

#### 5.2.2 Session Destruction

When a session ends (shell exits, user closes tab, or timeout):

```text
1. Shell process exits (or is killed)
2. Terminal receives Exit message on output channel
3. Terminal displays "[Process exited with code N]" in the session's grid
4. User acknowledges (any key) or auto-close timer expires
5. Terminal destroys:
   - IPC channels (input + output)
   - Notification channel
   - Shared memory region (if any)
   - VT emulation engine instance
6. Terminal updates multiplexer (remove tab/pane)
7. Audit log: session_destroyed event
```

#### 5.2.3 Detach and Reattach

Sessions can be detached from the terminal surface without killing the shell:

```text
Detach:
1. Terminal stops reading from output channel
2. Session state → Detached
3. Shell continues running (output buffers in channel ring)
4. Multiplexer removes session from visible panes
5. Audit log: session_detached event

Reattach:
1. Terminal (possibly different terminal instance) acquires session handle
2. Terminal drains buffered output from channel
3. Terminal creates new VT engine, replays buffered output
4. Session state → Active
5. Terminal resumes normal I/O loop
6. Audit log: session_reattached event
```

Detached sessions survive compositor restarts, terminal agent restarts, and even device sleep/wake cycles. The kernel maintains the IPC channels and shell process independently of any terminal agent.

-----

### 5.3 Shell Spawning and Capability Inheritance

When the terminal spawns a shell, it creates a child agent with carefully attenuated capabilities:

```rust
/// Capabilities granted to a shell spawned by the terminal.
pub fn shell_capabilities(terminal_caps: &CapabilitySet) -> CapabilitySet {
    CapabilitySet {
        // File system access: inherited from terminal, possibly narrowed
        read_space: terminal_caps.read_space.attenuate_to("user/home/"),
        write_space: terminal_caps.write_space.attenuate_to("user/home/"),

        // Process creation: shell can spawn commands
        process_create: Some(ProcessCreateCap {
            delegatable: true,
            max_children: 64,
        }),

        // Network: inherited from terminal (if terminal has network access)
        network: terminal_caps.network.clone(),

        // IPC: shell can create channels for pipes
        ipc_create: Some(IpcCreateCap { max_channels: 32 }),

        // No direct compositor access (shell doesn't render)
        compositor: None,

        // No direct audio/GPU access
        audio: None,
        gpu: None,
    }
}
```

**Monotonic capability restriction:** The shell can never gain capabilities the terminal doesn't have. If the terminal was spawned without network access (sandboxed terminal tab), the shell and all its children are also network-isolated.

**Per-tab capability profiles:** The user can create terminal tabs with different capability levels:

| Profile | Read | Write | Network | Process |
|---|---|---|---|---|
| Full (default) | `user/*` | `user/*` | Yes | Unlimited |
| Project-scoped | `user/project/` | `user/project/` | Yes | 32 |
| Sandboxed | `ephemeral/` | `ephemeral/` | No | 8 |
| Read-only | `user/*` | None | No | 4 |

-----

### 5.4 Process Group Semantics and Job Control

Traditional Unix shells use process groups and the controlling terminal for job control (`Ctrl+Z` to suspend, `fg`/`bg` to resume, `jobs` to list). AIOS translates these semantics to its native process and notification model.

#### 5.4.1 Process Groups as Agent Groups

```text
Unix process group:
  Shell (PGID=100) → child (PGID=101) → grandchild (PGID=101)
  Foreground PGID set on TTY via tcsetpgrp()

AIOS agent group:
  Shell agent → command agent → sub-process agent
  Foreground group tracked by terminal session:
    session.foreground_group: ProcessId  // the "active" process group leader
```

The terminal tracks which process group is "foreground" for each session. Only the foreground group receives keyboard input. Background groups continue running but don't receive input.

#### 5.4.2 Job Control Operations

| Shell Operation | Unix Mechanism | AIOS Mechanism |
|---|---|---|
| `Ctrl+C` (interrupt) | Kernel sends SIGINT to foreground PGID | Terminal sends InterruptNotification to foreground agent group |
| `Ctrl+Z` (suspend) | Kernel sends SIGTSTP to foreground PGID | Terminal sends SuspendNotification; agent runtime suspends the group |
| `fg` (resume foreground) | Shell calls tcsetpgrp() + SIGCONT | Shell sends ResumeNotification + updates session.foreground_group |
| `bg` (resume background) | Shell sends SIGCONT without tcsetpgrp() | Shell sends ResumeNotification (group stays background) |
| `jobs` (list jobs) | Shell tracks child PIDs and states | Shell queries agent runtime for child process states |
| `wait` (wait for exit) | Shell calls waitpid() | Shell calls process_wait syscall |

-----

### 5.5 Signal Translation

Unix signals don't exist in AIOS. The terminal translates signal-like operations to AIOS notification events:

```rust
/// Signal-equivalent notifications sent by the terminal.
pub enum TerminalSignal {
    /// Ctrl+C: interrupt the foreground process group.
    Interrupt,
    /// Ctrl+Z: suspend the foreground process group.
    Suspend,
    /// Ctrl+\: quit with core dump (abort the foreground process group).
    Quit,
    /// Ctrl+D: end of input (EOF on stdin channel).
    EndOfInput,
    /// Window resize: new terminal dimensions available.
    WindowResize { cols: u16, rows: u16 },
    /// Terminal hangup: terminal session is closing.
    Hangup,
}
```

The terminal intercepts specific key combinations before they reach the VT input translator:

```text
Key combination → Terminal action:
  Ctrl+C  → Send Interrupt notification to foreground group
  Ctrl+Z  → Send Suspend notification to foreground group
  Ctrl+\  → Send Quit notification to foreground group
  Ctrl+D  → Send EndOfInput (close input channel direction)
  Ctrl+S  → Pause output (XOFF — stop reading from output channel)
  Ctrl+Q  → Resume output (XON — resume reading from output channel)
```

These mappings are configurable via the terminal profile. Shells can disable them (e.g., `stty -isig` equivalent) by sending a mode change message to the terminal.

-----

### 5.6 Multi-Tab / Multi-Window Sessions

The terminal supports multiple concurrent sessions, presented as tabs within a single terminal window or as separate windows:

```text
Terminal Window (compositor surface)
┌─────────────────────────────────────────────────────┐
│ [Tab 1: ~/project] [Tab 2: ~/docs] [Tab 3: ssh]    │ ← tab bar
├─────────────────────────────────────────────────────┤
│                                                      │
│  $ cargo build                                       │
│  Compiling aios v0.1.0                              │
│  ...                                                 │
│                                                      │
│  user@aios:~/project$                               │
│                                                      │
└─────────────────────────────────────────────────────┘
```

Each tab is an independent `PtySession` with its own:

- VT emulation engine instance
- IPC channel pair
- Shell process
- Scrollback buffer
- Capability set (can differ per tab)

Tab management is the responsibility of the session multiplexer (§7). The terminal's rendering pipeline renders only the active tab's grid to the surface.

-----

### 5.7 POSIX Bridge

The POSIX compatibility layer (see [posix.md](../../platform/posix.md) §5-§7) translates Unix terminal APIs to AIOS native operations:

#### 5.7.1 Device Node Mapping

```text
/dev/tty     → The controlling terminal's PTY input/output channels
/dev/pts/N   → PTY session N's channels (via FD table translation)
/dev/ptmx    → Terminal agent's session creation API (open → allocate new PTY)
/dev/console → System console (kernel log output channel)
```

#### 5.7.2 ioctl Translation

| ioctl | AIOS Translation |
|---|---|
| `TIOCGWINSZ` | Query session dimensions from PtySession |
| `TIOCSWINSZ` | Resize session (triggers grid resize + damage) |
| `TCGETS` (get termios) | Query terminal mode state from VT engine |
| `TCSETS` (set termios) | Send mode change message to VT engine |
| `TIOCSCTTY` | Set controlling terminal (associate FD with session) |
| `TIOCNOTTY` | Release controlling terminal |
| `TIOCGPGRP` | Query foreground process group |
| `TIOCSPGRP` | Set foreground process group |
| `FIONREAD` | Query bytes available in output channel |

#### 5.7.3 termios Mapping

The `termios` structure maps to VT engine mode state:

```text
termios.c_iflag:
  ICRNL  → VT engine: translate CR to NL on input
  IGNCR  → VT engine: ignore CR on input
  INLCR  → VT engine: translate NL to CR on input
  IXON   → Terminal: enable Ctrl+S/Ctrl+Q flow control
  IXOFF  → Terminal: send XOFF when input buffer full

termios.c_oflag:
  OPOST  → VT engine: enable output processing
  ONLCR  → VT engine: translate NL to CR+NL on output

termios.c_lflag:
  ECHO   → VT engine: echo input characters to output
  ICANON → VT engine: canonical mode (line editing)
  ISIG   → Terminal: enable Ctrl+C/Ctrl+Z signal generation
  IEXTEN → VT engine: enable extended input processing

termios.c_cc[]:
  VINTR  → Terminal: interrupt key (default Ctrl+C)
  VQUIT  → Terminal: quit key (default Ctrl+\)
  VSUSP  → Terminal: suspend key (default Ctrl+Z)
  VEOF   → Terminal: EOF key (default Ctrl+D)
```

-----

### 5.8 Session Persistence and Detach/Reattach

Terminal sessions persist independently of the terminal agent's lifecycle. This enables tmux-like functionality as a native OS feature:

#### 5.8.1 Persistence Model

```text
Layer 1: Shell process (kernel-managed)
  → Survives: terminal close, compositor restart, display disconnect
  → Dies: explicit exit, OOM kill, system shutdown

Layer 2: IPC channels (kernel-managed)
  → Survive: terminal close, compositor restart
  → Destroyed: when both endpoints close, or session explicitly destroyed

Layer 3: Session metadata (space-stored)
  → Survives: everything including system restart (if space is persistent)
  → Contains: session ID, shell binary, environment, working directory,
              capability set, creation time, last attach time
```

When a terminal agent exits (window closed), detached sessions remain alive at Layer 1 and 2. A new terminal agent can list available detached sessions and reattach:

```rust
/// List all detached PTY sessions available to this user.
pub fn list_detached_sessions() -> Vec<DetachedSessionInfo> {
    // Query service manager for sessions owned by current user
    // that have no attached terminal agent.
}

/// Reattach to a detached session.
pub fn reattach_session(session_id: SessionId) -> Result<PtySession, SessionError> {
    // 1. Acquire channel endpoints from session broker
    // 2. Drain buffered output
    // 3. Create VT engine, replay buffered output
    // 4. Resume I/O loop
}
```

#### 5.8.2 Session Serialization

For sessions that must survive system restart (persistent sessions), the terminal serializes session state to a space object:

```rust
/// Serialized session state for persistence across restarts.
pub struct SerializedSession {
    pub id: SessionId,
    pub shell_binary: String,
    pub environment: Vec<(String, String)>,
    pub working_directory: String,
    pub capabilities: CapabilitySet,
    pub grid_state: SerializedGrid,  // last known grid content
    pub scrollback_ref: SpaceObjectId, // reference to scrollback space object
    pub created_at: Timestamp,
    pub last_attached: Timestamp,
}
```

On system startup, the terminal agent reads serialized sessions and offers to reconnect. The shell process must also support restart (e.g., by re-reading shell history and restoring working directory), which is a shell-level feature, not a terminal-level feature.

-----

### 5.9 Remote Terminal (SSH PTY Forwarding)

Remote terminal sessions integrate with the networking subsystem to provide SSH-like functionality:

#### 5.9.1 Architecture

```text
Local terminal agent ←→ [IPC channels] ←→ Network agent (SSH client)
                                                ↕ encrypted
                                          Remote AIOS device
                                                ↕
                                   Remote terminal agent ←→ Remote shell
```

The local terminal agent treats the remote connection as another PTY session. The network agent handles SSH protocol negotiation, key exchange, and encrypted transport. From the terminal's perspective, a remote session is identical to a local session — just with higher latency on the IPC channels.

#### 5.9.2 Connection Persistence (Mosh-Style)

For unreliable network connections (mobile, WiFi roaming), the terminal supports Mosh-style connection persistence:

1. **Local prediction:** The terminal renders input locally before the remote acknowledges it, providing instant feedback regardless of latency
2. **State synchronization:** The remote sends full screen state periodically (not diffs), allowing the connection to recover from packet loss without retransmission
3. **Roaming:** The connection survives IP address changes — the session is identified by a token, not a TCP connection
4. **Reconnection:** If the connection drops, the terminal automatically reconnects and resynchronizes state without losing the remote shell session

These features are implemented in the networking subsystem, not the terminal. The terminal simply connects to a remote PTY session via a resilient network channel.

-----

### 5.10 Session Serialization and Restore Across Compositor Restarts

When the compositor restarts (crash recovery, GPU driver reset, display reconfiguration), terminal sessions must reconnect seamlessly:

```text
1. Compositor crash detected (terminal loses surface)
2. Terminal agent is NOT killed — it continues running
3. Shell processes continue running (PTY channels still valid)
4. Terminal agent:
   a. Saves current grid state for each session
   b. Waits for compositor to restart
   c. Creates new compositor surface
   d. Restores grid state from saved data
   e. Resumes rendering (full surface redraw)
5. User sees: brief flicker, then terminal reappears with same content

Total downtime: compositor restart time (target < 500ms)
No shell state lost. No commands interrupted. No scrollback lost.
```

This is possible because the terminal's data model (cell grid, VT engine state, PTY channels) is entirely independent of the compositor surface. The surface is just a rendering target — losing it loses nothing except the pixels on screen, which are regenerated from the grid.
