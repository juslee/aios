# Knowledge Hive Rules

`docs/knowledge/` is the project knowledge base: plain Markdown, searched with Grep (lessons and decisions by keyword). Locate architecture docs through `docs/project/doc-map.md`.

## Writing Knowledge

After significant sessions, write insights to `docs/knowledge/`:

- `decisions/` — Architecture Decision Records (why we chose X over Y)
- `research/` — Research notes on explored topics
- `lessons/` — Hard-won lessons (bugs, gotchas, platform quirks)
- `discussions/` — Semi-permanent design explorations (graduate to arch docs when settled)
- `plans/` — Working implementation plans (ephemeral — delete after distilling lessons/decisions)

## Conventions

- Naming: `YYYY-MM-DD-initials-short-description.md`
- Required frontmatter: author, date, tags, status (draft/in-progress/final; `discussions/` may also use active, then graduated — see `docs/knowledge/README.md`)
- Tags: kernel, memory, ipc, sched, storage, platform, security, intelligence, boot, mmu, smp, drivers, compositor, gpu, audio, usb, networking, input, wireless, camera, media, tooling

## .claude/CLAUDE.md Self-Maintenance

Team-lead updates `.claude/CLAUDE.md` and the relevant rule files after every milestone:

1. Review what changed (new files, crates, constants, conventions)
2. Update: Workspace Layout, Key Technical Facts, Architecture Doc Map in `.claude/CLAUDE.md`; and the corresponding rule files in `.claude/rules/` (code conventions, quality gates, etc.)
3. Commit as part of the milestone commit (same commit)

`.claude/CLAUDE.md` and `.claude/rules/` sit under Claude Code's protected `.claude/` path: these edits go through a permission prompt (or the auto-mode classifier) instead of being auto-approved, so an unattended run where nobody can answer a prompt is refused them (docs policy in `docs/project/agent-loop.md`).
