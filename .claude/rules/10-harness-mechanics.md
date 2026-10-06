# Harness Mechanics

Claude Code behavior that breaks edits and tool calls, whatever user config dir or account the session runs under.

## Read-State Resets

After a context compaction ("This session is being continued from a previous conversation…") and after `EnterWorktree`, the harness treats **every file as unread**: your memory of its contents does not count. Read each file at its current absolute path before the first Edit or Write. `MEMORY.md` is the same trap: its contents appear in the system prompt, but you must still Read the file before editing it.

## Stale-Read Errors

On "File has not been read yet" or "File modified since read", never re-send the same Edit. Read the file fresh, then Edit once.

Commands that rewrite files invalidate read-state for every file they touch. Treat those files as unread afterward:

- `just fmt` or `cargo fmt`: any `.rs` file
- `AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test docs_check_parity`: the goldens under `tools/tests/golden/`
- `just docs-check --update-baseline`: `scripts/docs/baseline.json`
- `git merge`, `git rebase`, `git checkout`, `git switch`, `git stash pop`: every file they change

## Deferred Tools

Grep, Glob and many MCP tools are not loaded at session start; a habit call fails with "No such tool available". Load them with ToolSearch before first use, or use `rg` via Bash. End zero-match-is-fine probe chains with `|| true` so a clean "no matches" (exit 1) does not read as failure.

## Typed-Tool Arguments

Arguments come from the tool's schema, not memory. Never re-send an MCP call that was just rejected for an unknown field or a wrong wrapper key: fetch the schema (ToolSearch `select:<name>`) and fix the shape.
