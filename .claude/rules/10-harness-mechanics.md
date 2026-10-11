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

There is no `rg` binary on `PATH`. In the Bash tool, `rg` and `grep` are Claude Code shell functions defined by the session's shell snapshot, and some sessions have no `rg`. `bash -c`, `sh -c`, scripts, hooks and `just` recipes never get either function.

- If `command -v rg` prints nothing, run the same pattern with `grep -nE` (`grep -rnE` for a tree). Without `-E`, `grep` uses basic regex, so `a|b` matches only the literal text `a|b`.
- Never write `rg` into a script, hook, recipe or acceptance command. Use `grep -E` there.
- The Bash-tool `grep` is Claude Code's ugrep with `--ignore-files --hidden -I --exclude-dir=.git`. A recursive search silently skips gitignored paths (`target/`, `*.img`, `.claude/worktrees/`, `.remember/`) and binary files. A gitignored file named on the command line is still searched, but a binary file is not. When a search must see ignored or binary files, use `command grep` (macOS grep).

## Typed-Tool Arguments

Arguments come from the tool's schema, not memory. Never re-send an MCP call that was just rejected for an unknown field or a wrong wrapper key: fetch the schema (ToolSearch `select:<name>`) and fix the shape.

## Protected-Path Reads

`.claude/settings*` and `.claude/hooks/*` hold the guard, the shim and the settings, so Bash writes to them must reach the owner. For `sed`, the guard (`.claude/hooks/git-push-guard.py`) tells reads from writes: a read-only `sed -n 'A,Bp' file` on those paths runs without a prompt (options `-n`, `-E`, `-r`, `-s`, `-u`, `-z`, `-e` only; a literal script of `p`, `P`, `=`, `q`, `Q`, `l`, `n`, `N`, `d`, `D`, `g`, `G`, `h`, `H`, `x` and braces with line, `$`, `first~step` or `/regex/` addresses), and any other `sed` there asks (`-i`, `-I`, `--in-place`, `-f`, a `w`, `r`, `e` or `s` command, a script from a variable, output redirected into those paths). This holds once the owner has removed the settings' `sed` asks on those paths; until then every `sed` there still prompts. `.claude/settings.json` keeps its ask rules for `awk` and `cp` on those paths, and for `awk` on `.git/hooks*`, so those prompt the owner even to read and are refused in a background or `-p` run: never `awk` or `cp` them. The Read tool (`offset`/`limit` for a line range) and Grep or `grep -n` remain the safest way to read and search them. Agents briefing other agents pass this on: a prompt that says "read the guard" means the Read tool.
