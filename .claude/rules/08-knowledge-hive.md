# Knowledge Hive Rules

The `docs/` directory is an Obsidian vault with MCP integration.

## Writing Knowledge

After significant sessions, write insights to `docs/knowledge/`:

- `decisions/` — Architecture Decision Records (why we chose X over Y)
- `research/` — Research notes on explored topics
- `lessons/` — Hard-won lessons (bugs, gotchas, platform quirks)
- `discussions/` — Semi-permanent design explorations (graduate to arch docs when settled)
- `plans/` — Working implementation plans (ephemeral — delete after distilling lessons/decisions)

## Conventions

- Naming: `YYYY-MM-DD-initials-short-description.md`
- Required frontmatter: author, date, tags, status (draft/in-progress/final)
- Tags: kernel, memory, ipc, sched, storage, platform, security, intelligence, boot, mmu, smp, drivers, compositor, gpu, audio, usb, networking, input, wireless, camera, media

## CLAUDE.md Self-Maintenance

Team-lead updates CLAUDE.md after every milestone:

1. Review what changed (new files, crates, constants, conventions)
2. Update: Workspace Layout, Key Technical Facts, Architecture Doc Map, Code Conventions, Quality Gates
3. Commit as part of the milestone commit (same commit)
