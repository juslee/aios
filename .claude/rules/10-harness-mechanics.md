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

Grep, Glob and many MCP tools are not loaded at session start; a habit call fails with "No such tool available". Load them with ToolSearch before first use, or search via Bash (see Search Commands). End zero-match-is-fine probe chains with `|| true` so a clean "no matches" (exit 1) does not read as failure.

## Search Commands

There is no `rg` binary on `PATH`. In the Bash tool, `rg` and `grep` are Claude Code shell functions defined by the session's shell snapshot, and some sessions have no `rg`. On 2026-10-10, two implementer runs had none and ran their acceptance `rg -n` lines as `grep -nE`. `bash -c`, `sh -c`, scripts, hooks and `just` recipes never get either function.

- If `command -v rg` prints nothing, run the same pattern with `grep -nE` (`grep -rnE` for a tree). Without `-E`, `grep` uses basic regex, so `a|b` matches only the literal text `a|b`.
- Never write `rg` into a script, hook, recipe or acceptance command. Use `grep -E` there.
- The Bash-tool `grep` is Claude Code's ugrep with `--ignore-files --hidden -I --exclude-dir=.git`. A recursive search silently skips gitignored paths (`target/`, `*.img`, `.claude/worktrees/`, `.remember/`) and binary files. A gitignored file named on the command line is still searched, but a binary file is not. When a search must see ignored or binary files, use `command grep` (macOS grep).

## Typed-Tool Arguments

Arguments come from the tool's schema, not memory. Never re-send an MCP call that was just rejected for an unknown field or a wrong wrapper key: fetch the schema (ToolSearch `select:<name>`) and fix the shape.

## Protected-Path Reads

`.claude/settings.json` has ask rules for `sed`, `awk` and `cp` on `.claude/settings*` and `.claude/hooks/*`, and for `awk` on `.git/hooks*`. They protect the guard, the shim and the settings from Bash writes, which they cannot tell apart from reads (`sed -n 5p` vs `sed -i`, or a `w` command), so a read-only `sed -n` on those paths prompts the owner and is refused in a background or `-p` run. Read those files with the Read tool (`offset`/`limit` for a line range) and search them with Grep or `grep -n`; never `sed`, `awk` or `cp` them, even to read. Agents briefing other agents pass this on: a prompt that says "read the guard" means the Read tool.
