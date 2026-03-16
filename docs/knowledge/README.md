---
tags: [knowledge]
type: guide
---

# Knowledge Hive

Shared knowledge base for the AIOS project. Every developer's Claude Code instance can read, search, and contribute here via the Obsidian MCP server (configured in `.mcp.json`).

## Structure

| Folder | Purpose | Lifecycle |
| --- | --- | --- |
| `decisions/` | Architecture Decision Records — why we chose X over Y | Permanent |
| `research/` | Deep-dive research notes on explored topics | Permanent |
| `lessons/` | Hard-won lessons — bugs, gotchas, platform quirks | Permanent |
| `discussions/` | Architecture brainstorming, design explorations, Claude Code session notes | Semi-permanent — keep until graduated or irrelevant |
| `plans/` | Working docs for active phase/milestone implementation | Ephemeral — deleted after distilling |

## Conventions

### Naming

```
YYYY-MM-DD-initials-short-description.md
```

- Date: when the note was created
- Initials: author's initials (e.g., `jl` for Justin Lee)
- Description: kebab-case summary

### Frontmatter (required)

Every note must have YAML frontmatter:

```yaml
---
author: <name>
date: YYYY-MM-DD
tags: [<tag1>, <tag2>]
status: draft | in-progress | final
---
```

### Tags vocabulary

Use these tags for consistent search across the hive:

`kernel`, `memory`, `ipc`, `sched`, `storage`, `platform`, `security`, `intelligence`, `boot`, `mmu`, `smp`, `drivers`, `compositor`, `gpu`, `audio`, `usb`, `networking`, `input`, `wireless`, `camera`, `media`

### Write-once preference

Prefer creating new notes over editing existing ones. This minimizes merge conflicts in the multi-developer setup.

### Architecture docs are for finalized design only

Architecture docs (`docs/kernel/`, `docs/platform/`, `docs/intelligence/`, etc.) describe the **target design** — the settled vision. Do not use them for in-progress discussion, brainstorming, or planning.

Use `docs/knowledge/discussions/` for brainstorming and design exploration, and `docs/knowledge/plans/` for implementation planning. Graduate content to architecture docs only when the design is settled.

## Discussions (discussions/)

For architecture deep dives, design brainstorming, and session notes that aren't tied to a specific phase implementation:

1. **Create** a discussion doc: `discussions/YYYY-MM-DD-initials-topic.md`
   - Capture ideas, open questions, trade-offs explored
   - Set `status: draft` or `active`
2. **Revisit** across sessions — add new insights as they come up
3. **Graduate** when ready:
   - Settled designs → architecture docs (`docs/kernel/`, `docs/platform/`, etc.)
   - Key decisions → `decisions/` (permanent)
   - Set `status: graduated` and note where content landed

Unlike `plans/`, discussion docs are **not deleted** — they serve as a trail of how thinking evolved.

## Working Document Pattern (plans/)

When implementing a phase or milestone:

1. **Create** a plan doc: `plans/phase-N-MK-description.md`
   - Track approach, decisions, issues encountered
   - Set `status: in-progress`
2. **Update** as you work — it's a living scratchpad
3. **At completion**, distill:
   - Hard-won insights → `lessons/` (permanent)
   - Key decisions → `decisions/` (permanent)
4. **Delete** the plan doc — the permanent notes survive

## Searching

Via Claude Code (Obsidian MCP tools):

- `search_notes("query")` — full-text search across all docs
- `read_note("path")` — read any note
- `manage_tags` — browse by domain tags

Via Obsidian desktop app (optional):

- Graph view, backlinks, tag search, quick switcher
- Open `docs/` as vault (File → Open folder as vault)
