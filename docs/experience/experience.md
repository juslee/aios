# AIOS Experience Layer

## What Makes the GUI Unique

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [compositor.md](../platform/compositor.md) — Compositor and display, [airs.md](../intelligence/airs.md) — AI Runtime Service, [spaces.md](../storage/spaces.md) — Space Storage

-----

## 1. Core Insight

Every desktop operating system presents the same model: a desktop with icons, a taskbar with running applications, a file manager with directory trees, a notification center with a list of alerts. This model was designed for a world where a computer runs isolated programs that don't know about each other and the user is the only intelligence that connects them.

AIOS doesn't have that world. AIOS has spaces instead of files, agents instead of applications, Flow instead of clipboard, context instead of explicit modes, and an AI runtime that understands what the user is doing. The GUI must reflect this fundamentally different model.

**The AIOS experience is not a reskinned Linux desktop.** There is no desktop with scattered icons. There is no Start menu with a list of installed programs. There is no file manager with a tree of directories. There is no notification center that dumps every alert into a chronological list. Every one of these is replaced by something designed for how AIOS actually works.

-----

## 2. The Five Surfaces

The AIOS experience is built on five primary surfaces. Everything the user sees is one of these:

```
┌─────────────────────────────────────────────────────────────┐
│                                                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │                    1. WORKSPACE                        │  │
│  │           The contextual home view                     │  │
│  │     What you see when you're between activities        │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │                 2. ACTIVITY WINDOWS                     │  │
│  │    Browser, terminal, media, games, agent UIs          │  │
│  │     The actual things you're doing right now            │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │               3. CONVERSATION BAR                      │  │
│  │       Natural language interface to everything         │  │
│  │    One gesture away, never forced, always available    │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │                 4. FLOW TRAY                            │  │
│  │       Visual pipeline of data in transit               │  │
│  │     What's moving between spaces and agents            │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │               5. STATUS STRIP                          │  │
│  │     System health, attention digest, context           │  │
│  │       Minimal, non-intrusive, always truthful          │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

-----

## 3. The Workspace

### 3.1 Not a Desktop

Traditional desktops show you the same thing regardless of what you're doing. Icons on a wallpaper. A taskbar. A clock. It's a static stage that never changes.

The Workspace is **contextual**. It adapts to what the Context Engine infers about your current activity. It's what you see when you return to home — between activities, at the start of a session, after closing your last window.

### 3.2 Work Context

When the Context Engine detects work engagement:

```
┌─────────────────────────────────────────────────────────────┐
│  WORKSPACE — Work                                     14:30 │
│                                                              │
│  ┌─ Active Tasks ─────────────────────────────────────────┐ │
│  │                                                         │ │
│  │  ● Review PR #427          ◐ ████░░░░░░ 40%            │ │
│  │    3 files reviewed, 2 remaining                       │ │
│  │    [Continue in Browser] [Open Terminal]                │ │
│  │                                                         │ │
│  │  ○ Write architecture doc                               │ │
│  │    Last edited 2 hours ago                              │ │
│  │    [Resume]                                             │ │
│  │                                                         │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                              │
│  ┌─ Recent Spaces ─────────────┐ ┌─ Attention ───────────┐ │
│  │                              │ │                        │ │
│  │  📁 project-alpha/          │ │  3 items since 13:00   │ │
│  │     Modified 5 min ago      │ │                        │ │
│  │     by code-assistant       │ │  ▸ Alex: meeting moved │ │
│  │                              │ │    to 16:00 (Slack)   │ │
│  │  📁 research/papers/        │ │  ▸ CI passed on main   │ │
│  │     3 new objects today     │ │  ▸ 2 emails (digest)   │ │
│  │                              │ │                        │ │
│  │  📁 notes/daily/            │ │  [See digest]          │ │
│  │     Today's note exists     │ │                        │ │
│  └──────────────────────────────┘ └────────────────────────┘ │
│                                                              │
│  ┌─ Quick Actions ────────────────────────────────────────┐ │
│  │  [New Terminal]  [Open Browser]  [Search Spaces]       │ │
│  └─────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

**What's different from a traditional desktop:**
- **Tasks, not applications.** You see your active tasks and their progress, not a list of running programs. The OS tracks what you're working on, not just what's open.
- **Spaces, not files.** Recent Spaces shows activity — what changed, what agents did, how many new objects — not just file names and modification dates.
- **Attention, not notifications.** The Attention section shows an AI-triaged digest, not a chronological dump. Three items, not thirty.
- **Context-aware actions.** Quick Actions show what's relevant right now (terminal, browser for code review), not every possible application.

### 3.3 Leisure Context

When the Context Engine detects leisure:

```
┌─────────────────────────────────────────────────────────────┐
│  WORKSPACE — Evening                                  20:15 │
│                                                              │
│  ┌─ Continue ─────────────────────────────────────────────┐ │
│  │                                                         │ │
│  │  🎵 Album: Dark Side of the Moon — Pink Floyd          │ │
│  │     Paused at track 4 · 23:15 remaining                │ │
│  │     [Resume]                                            │ │
│  │                                                         │ │
│  │  🌐 Reading: "Attention Is All You Need" — arxiv.org   │ │
│  │     Tab open from 18:30 · scrolled to page 4           │ │
│  │     [Continue Reading]                                  │ │
│  │                                                         │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                              │
│  ┌─ Media ──────────────────┐ ┌─ Browse ──────────────────┐│
│  │                           │ │                            ││
│  │  Recent albums            │ │  Bookmarks                 ││
│  │  Recent playlists         │ │  Reading list              ││
│  │  Continue podcast         │ │  Saved articles            ││
│  │                           │ │                            ││
│  └───────────────────────────┘ └────────────────────────────┘│
│                                                              │
│  Notifications suppressed until 07:00                        │
│  (except calls from Favorites)                               │
└─────────────────────────────────────────────────────────────┘
```

**What's different:**
- The layout is sparser. Less information density. More breathing room.
- Tasks are hidden. You're not working.
- Continue section shows media and reading state — picked up from where you left off across agents.
- Notifications are suppressed. The OS tells you this explicitly so you feel safe.

### 3.4 How the Workspace Knows

The Workspace isn't hardcoded. It queries:
1. **Task Manager:** Active tasks, their state and progress
2. **Context Engine:** Work/leisure/gaming/focus context
3. **Space Storage:** Recently accessed spaces, modification activity
4. **Attention Manager:** Triaged notification digest
5. **Agent Runtime:** Running agents, their current state
6. **Media subsystem:** Now playing, queue, pause state

The Workspace is a **live view over system state**, not a static page. It updates in real-time as state changes.

-----

## 4. The Conversation Bar

### 4.1 Design Philosophy

The Conversation Bar is the single most important GUI element in AIOS. It's not a search bar (like Spotlight or Krunner) and it's not a chatbot widget. It's a **natural language command interface** that can do anything the OS can do.

**One gesture to invoke.** A swipe from the edge, a keyboard shortcut, or a tap on the status strip. It slides in as a panel, not a fullscreen takeover.

**Never forced.** The system never opens the Conversation Bar uninvited. Even during onboarding, it's presented as "this is here if you want it" not "you must use this."

**Conversational, not form-based.** You don't fill in fields. You describe what you want.

### 4.2 What It Can Do

```
┌─────────────────────────────────────────────────────────────┐
│  CONVERSATION BAR                                      ─ ×  │
│                                                              │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ Find my notes about the IPC design from last week   │    │
│  └─────────────────────────────────────────────────────┘    │
│                                                              │
│  Found 3 objects in research/aios-notes:                    │
│                                                              │
│  📄 IPC Performance Analysis                                │
│     Created Tuesday · 1,200 words · tags: ipc, benchmarks   │
│     "Analysis of L4 IPC patterns and how they apply to..."  │
│     [Open] [Send via Flow] [Show provenance]                │
│                                                              │
│  📄 Syscall Design Draft                                    │
│     Created Monday · 800 words · tags: syscalls, kernel      │
│     "Minimal syscall set: IpcCall, IpcSend, IpcRecv..."     │
│     [Open] [Send via Flow] [Show provenance]                │
│                                                              │
│  📄 Microkernel Comparison                                  │
│     Created Monday · 2,100 words · tags: seL4, L4, QNX     │
│     "Comparing IPC approaches: seL4 vs QNX vs..."           │
│     [Open] [Send via Flow] [Show provenance]                │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**Space queries:**
- "Find my notes about transformer architectures"
- "Show me everything I saved from arxiv last month"
- "What did the research agent add to my project space today?"

**Task management:**
- "Help me review this PR" → creates task, opens browser + terminal, loads diff
- "I'm done with the architecture doc" → marks task complete, creates snapshot
- "What am I working on?" → shows active tasks with progress

**System control:**
- "I'm heads down for 2 hours" → Context Engine override, notifications suppressed
- "Turn off the music" → audio subsystem control
- "What's using my network right now?" → NTM audit query

**Agent interaction:**
- "Ask the code assistant to explain this function" → spawns/resumes agent with context
- "Send this to Alex's shared space" → Flow operation with identity + capability check

**Quick actions with structured output:**
- "Create a new space for the conference paper" → space created, opened
- "Schedule backup at midnight" → agent task scheduled
- "What permissions does the weather agent have?" → capability listing

### 4.3 Structured Visual Output

The Conversation Bar doesn't just return text. It returns **structured visual results** that the user can interact with:

- Space query results → clickable object cards with summaries
- Task creation → live task card that updates with progress
- Agent status → capability list with revoke buttons
- System queries → tables, charts, timelines
- Errors → clear explanation with suggested fix action

This is the "natural language in, structured visual output" principle from the design principles.

-----

## 5. The Flow Tray

### 5.1 What Flow Looks Like

Flow replaces the clipboard. But unlike the clipboard (which is invisible), Flow is **visible**. The Flow Tray shows data in transit between agents and spaces:

```
┌─ FLOW TRAY ──────────────────────────────────────────────┐
│                                                           │
│  ↓ research-agent → research/papers/               now   │
│    3 PDF objects (transformer survey, attention...)       │
│    [View] [Cancel]                                        │
│                                                           │
│  ✓ Browser tab (arxiv) → research/papers/     2 min ago  │
│    Highlighted text + source URL                          │
│    [View in space]                                        │
│                                                           │
│  ◐ backup-agent → backup/daily/              in progress  │
│    47 of 312 objects synced                               │
│    [Pause] [Details]                                      │
│                                                           │
└───────────────────────────────────────────────────────────┘
```

**What's different from a clipboard:**
- **Visible.** You can see what's moving and where it's going.
- **Typed.** Flow knows it's carrying PDFs, not just bytes. It can show previews.
- **Contextual.** Flow transforms data based on destination. Dropping rich text into a terminal strips formatting. Dropping an image into a research space adds provenance metadata.
- **Historical.** The tray shows recent transfers, not just the current one. You can grab something you flowed 10 minutes ago.
- **Bidirectional.** Flow isn't just "copy from A, paste into B." It's a pipeline. An agent can push data to your Flow, and you can route it to any destination.

### 5.2 Flow Interactions

**Drag and drop:** Drag from any surface, drop into any surface. The compositor routes through Flow. The data is capability-checked at both ends.

**Flow gestures:** Select text in the browser, flick toward the edge → appears in Flow Tray. Tap an item in Flow Tray, flick into a window → data arrives with context.

**Agent-initiated Flow:** An agent pushes results to your Flow Tray. You see a notification in the tray: "Research agent found 3 papers." You choose where they go.

**Cross-device Flow:** On device A, put something in Flow. On device B (synced via Space Mesh), it appears in your Flow Tray. Seamless cross-device data transfer without cloud intermediaries.

-----

## 6. The Status Strip

### 6.1 Minimal, Truthful, Non-Intrusive

The Status Strip replaces the traditional taskbar/system tray. It lives at the bottom (or top, user-configurable) and contains only essential information:

```
┌─────────────────────────────────────────────────────────────┐
│ ● Work · 3 tasks │ 🔊 · 📶 · 🔋 78% │ Tue 14:30 │ ▲ 2  │
└─────────────────────────────────────────────────────────────┘
  ↑                  ↑                    ↑             ↑
  Context + tasks    System status        Time          Attention
                                                        badge
```

**Left: Context indicator.** Shows the current inferred context (Work, Leisure, Focus, Gaming) and active task count. Tapping opens the Workspace.

**Center: System status.** Audio, network, battery. Only shows active subsystems. No Wi-Fi icon if you're on Ethernet. No battery if you're plugged in. Shows what matters, hides what doesn't.

**Right: Time + Attention badge.** The attention badge shows how many items are waiting in the digest. Tapping opens the Attention panel — not a notification center, but an AI-summarized digest.

### 6.2 No System Tray Clutter

Traditional system trays accumulate icons from every background application. AIOS has no system tray. Background agents are managed through the Workspace (running agents section) or the Conversation Bar ("what agents are running?"). The Status Strip stays clean.

-----

## 7. The Attention Panel

### 7.1 Not a Notification Center

Traditional notification centers are reverse-chronological dumps. Every notification has equal visual weight. Thirty Slack messages look identical to one urgent message from your boss.

The Attention Panel is **AI-triaged**:

```
┌─ ATTENTION ──────────────────────────────────────────────┐
│                                                           │
│  ── Since your last check (2 hours ago) ──               │
│                                                           │
│  URGENT                                                   │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  Alex: "Server is down, need your help ASAP"        │ │
│  │  Slack · 15 min ago                                  │ │
│  │  [Reply] [Open Slack tab]                            │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                           │
│  SUMMARY                                                  │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  5 Slack messages in #engineering (none urgent)      │ │
│  │  2 emails: 1 newsletter, 1 meeting confirmation     │ │
│  │  CI: 3 builds passed, 0 failed                      │ │
│  │  backup-agent: daily backup completed (312 objects)  │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                           │
│  DEFERRED                                                 │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  System update available (non-urgent)                │ │
│  │  Weather: rain expected tomorrow                     │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                           │
│  [Mark all seen] [Settings]                               │
│                                                           │
└───────────────────────────────────────────────────────────┘
```

**What's different:**
- **Urgency tiers.** Urgent items are visually prominent. Everything else is condensed.
- **AI summarization.** Five Slack messages become one line: "5 Slack messages in #engineering (none urgent)." You decide if you want to read them, not the OS.
- **Since your last check.** Time-scoped, not infinite scrollback.
- **Actionable.** Every item has actions. Not just "dismiss" — actual responses.

-----

## 8. Space Navigator

### 8.1 Not a File Manager

AIOS has no file manager. It has a **Space Navigator** — a visual tool for exploring spaces by meaning, relationships, and activity rather than by directory path:

```
┌─ SPACE NAVIGATOR ────────────────────────────────────────┐
│                                                           │
│  Search: [architecture design decisions        ] 🔍      │
│                                                           │
│  ┌─ Results (semantic) ──────────────────────────────┐   │
│  │                                                    │   │
│  │  📄 IPC Design Rationale          research/aios/  │   │
│  │     "Why synchronous IPC is the right choice..."  │   │
│  │     Similarity: 0.94 · Created Mon · 2,400 words  │   │
│  │                                                    │   │
│  │  📄 Microkernel Architecture Doc   docs/           │   │
│  │     "Core architecture decisions for AIOS..."     │   │
│  │     Similarity: 0.91 · Created last week          │   │
│  │                                                    │   │
│  │  📄 L4 Comparison Notes            research/refs/  │   │
│  │     "seL4 capability model vs AIOS capability..." │   │
│  │     Similarity: 0.87 · Created 2 weeks ago        │   │
│  │                                                    │   │
│  └────────────────────────────────────────────────────┘   │
│                                                           │
│  ┌─ Relationships ────────────────────────────────────┐  │
│  │                                                     │  │
│  │  IPC Design Rationale                               │  │
│  │    ├── DerivedFrom: Microkernel Architecture Doc    │  │
│  │    ├── References: L4 Comparison Notes              │  │
│  │    ├── InputTo: Task "Design syscall interface"     │  │
│  │    └── RelatedTo: 4 other objects (show all)        │  │
│  │                                                     │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                           │
│  ┌─ Activity ──────────────────────────────────────────┐ │
│  │  This space: 12 objects modified today               │ │
│  │  Last agent: code-assistant (3 hours ago)            │ │
│  │  Space size: 847 objects, 124 MB                     │ │
│  └──────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────┘
```

**What's different from a file manager:**
- **Search first.** The primary interaction is search, not navigation. Type what you're looking for, get semantic results.
- **Relationships visible.** Every object shows its connections. You see the knowledge graph, not a flat list.
- **Activity, not just metadata.** You see what agents touched this space, when, and how much changed.
- **No paths to remember.** Objects have names, tags, and relationships — not `/home/user/Documents/Projects/AIOS/design/ipc/v3-final-FINAL.md`.

### 8.2 Path Navigation Still Works

For users who want it, the Space Navigator has a path bar. `user/research/aios/` works. The POSIX bridge means `ls /spaces/research/` works in the terminal too. Paths are a compatibility feature, not the primary interaction.

-----

## 9. Agent Presence

### 9.1 Agents Are Visible

In traditional OSes, background processes are invisible unless you open Task Manager. In AIOS, agents are first-class citizens of the experience. The user can always see which agents are active:

**In the Workspace:** Running agents section shows what's active and what they're doing.

**In the Status Strip:** Agent activity indicator (subtle glow or badge) when agents are actively working.

**In the Conversation Bar:** "What agents are running?" → full list with capabilities and current activity.

**In the Inspector:** Deep visibility — every action, every capability used, every space accessed.

### 9.2 Agent Cards

Each agent has a visual card that shows:
```
┌─ code-assistant ─────────────────────────────────────────┐
│                                                           │
│  Status: Active · Running for 2h 15m                     │
│  Current task: "Review PR #427"                          │
│                                                           │
│  Capabilities:                                            │
│    ✓ Read: project-alpha/                                │
│    ✓ Write: project-alpha/reviews/                       │
│    ✓ Network: github.com                                 │
│    ✗ Microphone, Camera, GPS (not requested)             │
│                                                           │
│  Resource usage:                                          │
│    Memory: 48 MB · CPU: 2% · Network: 1.2 MB today      │
│                                                           │
│  [Pause] [Stop] [Revoke capability...] [View audit log]  │
│                                                           │
└───────────────────────────────────────────────────────────┘
```

**Why this matters:** Users can see exactly what agents are doing and control them. No invisible background processes. No silent data exfiltration. Transparency is the default.

-----

## 10. Provenance Everywhere

### 10.1 Visible Origin

Every object in AIOS has a provenance chain. The GUI makes this visible:

**On hover/long-press:** A small provenance badge shows where this object came from.
```
Created by: research-agent · from arxiv.org/abs/2026.12345
Modified by: you · 2 hours ago
Derived from: "Attention Is All You Need" summary
```

**In the Space Navigator:** The Relationships panel shows the full provenance chain — what this was derived from, what was derived from it, which tasks used it as input.

**In the Inspector:** Full Merkle chain with cryptographic signatures. Every version, every edit, every agent that touched it.

### 10.2 Why Users Care

- "Where did this come from?" → Instantly answered for any object.
- "Did I write this or did the AI?" → Provenance records `AiGenerated { model, prompt_hash }`.
- "Who shared this with me?" → Provenance records identity and path.
- "Can I trust this data?" → Provenance shows the full chain from original source.

-----

## 11. Context Transitions

### 11.1 Visual Context Shifts

When the Context Engine detects a context change, the UI transitions smoothly:

**Work → Leisure:**
- Workspace layout shifts to sparser, media-focused
- Color temperature warms slightly (configurable)
- Notification threshold relaxes (shown subtly in Status Strip)
- Transition is animated, not jarring — 300ms fade

**Any → Focus:**
- Non-essential UI elements dim
- Status Strip minimizes to a thin line
- Notification badge disappears (Attention Manager in digest-only mode)
- Only the focused activity window remains prominent

**Any → Gaming:**
- Fullscreen handoff to game
- Status Strip hides completely
- Notifications suppressed except Interrupt-level
- Compositor enters low-latency mode (direct scanout)

### 11.2 Override Is Always Available

The user can always override the inferred context:
- Via Conversation Bar: "I'm working now" / "I'm done for the day"
- Via Status Strip: tap the context indicator to cycle or set manually
- Via gesture: configurable gesture to toggle focus mode

The system acknowledges the override visually and respects it until the user changes it or enough time passes that re-inference is appropriate (configurable, default 4 hours).

-----

## 12. Design Language

### 12.1 Visual Principles

1. **Density adapts to context.** Work mode shows more information. Leisure mode shows less. Focus mode shows almost nothing except the activity.
2. **Motion is meaningful.** Animations convey state changes (context transitions, Flow transfers, agent activity). Never gratuitous.
3. **Text is the primary medium.** AIOS is a text-forward OS. Summaries, descriptions, search results — text is how information is presented. Icons are secondary.
4. **Color conveys state, not decoration.** Urgency = warm. Normal = neutral. Suppressed = cool. Context changes shift the palette subtly.
5. **No chrome for chrome's sake.** Every pixel earns its place. No decorative borders, shadows, or gradients unless they convey information (depth, focus, grouping).

### 12.2 Built With Iced

The entire experience layer is built with the iced toolkit (Rust, Elm-inspired). This means:
- Cross-platform: the same UI code runs on AIOS, Linux, macOS, and Web
- Developers building agents use the same toolkit, so agent UIs feel native
- GPU-accelerated rendering via wgpu
- Accessibility tree generated from the iced widget hierarchy

-----

## 13. What Users Never See

The most important part of the AIOS experience is what's **absent**:

- **No installation wizards.** Agents declare capabilities and run. No "Next → Next → Install → Reboot."
- **No update interruptions.** A/B updates happen in the background. The user sees "Update ready, switch at next reboot" — never "Updating, please wait."
- **No driver prompts.** Plug in hardware. Subsystem framework recognizes it. It works. The user sees "New device: USB Headset" in the status strip.
- **No permission fatigue.** Capabilities are approved once at agent install. No "Allow camera access?" every time a known agent needs the camera.
- **No file-type associations.** Objects have content types. The OS knows what opens them. No "Which application do you want to use to open this file?"
- **No settings archaeology.** The Conversation Bar handles configuration: "Make the text bigger" → done. No hunting through Settings → Display → Font Size → Advanced.
- **No manual organization.** AIRS indexes everything. Tags are generated. Relationships are inferred. The user doesn't need to carefully file things into the right folder.

-----

## 14. Implementation Order

```
Phase 6a:  Compositor + window management        → surfaces composited
Phase 6b:  Status Strip (basic)                  → system status visible
Phase 6c:  Workspace (static, no context)        → home view with spaces + tasks
Phase 6d:  Input routing + focus management      → keyboard/mouse works
Phase 12a: Conversation Bar (search only)        → semantic space search
Phase 12b: Conversation Bar (full)               → task creation, system control, agent interaction
Phase 12c: Space Navigator                       → visual space exploration
Phase 15a: Flow Tray                             → visible data transfer
Phase 15b: Attention Panel                       → AI-triaged notifications
Phase 15c: Context transitions                   → visual context shifts
Phase 15d: Workspace (contextual)                → context-adaptive home view
Phase 16:  Agent Cards + presence                → agent visibility
Phase 29:  Iced integration                      → full toolkit with cross-platform
Phase 33:  Accessibility throughout              → screen reader, keyboard nav
```
