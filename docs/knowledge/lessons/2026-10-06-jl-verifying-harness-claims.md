---
author: jl + claude
date: 2026-10-06
tags: [tooling, security]
status: final
---

# Lesson: Verify harness claims against the installed tool, not against memory

## What happened

Audits of harness changes (`.claude/settings.json`, plugins and marketplaces, the `/merge-and-cleanup` skill, CI review) kept meeting claims that read well and could not be checked from the repository: a plugin name, a marketplace source field, how `gh pr merge --delete-branch` treats worktrees, what the CI review action restores from the base branch. In several cases the memory-based answer was wrong or only half right (for example, which files a worktree removal loses: git deletes ignored files silently and refuses on untracked or modified ones, so "untracked files are lost" is wrong).

## Why it happened

The facts live outside the repository: in the Claude Code config directory, in the installed binary, in marketplace clones, in `gh`'s source at the installed tag, and in other repositories' source. A reviewer who reads only the diff has nothing to compare a claim with.

## What we learned

**Where each kind of claim is checked.** All paths below were re-checked on 2026-10-10 (Claude Code 2.1.292, gh 2.102.0).

- *Config directory.* A session's config directory is `$CLAUDE_CONFIG_DIR`, set per checkout by the untracked `.envrc`; without it, `~/.claude`. Sessions can run under more than one config directory, so check the one the current session uses. Marketplace clones are at `$CLAUDE_CONFIG_DIR/plugins/marketplaces/<name>/.claude-plugin/marketplace.json` (per-plugin `source`, and inline `lspServers` for LSP plugins such as `rust-analyzer-lsp`). Installed plugin versions are in `$CLAUDE_CONFIG_DIR/plugins/installed_plugins.json` and their files under `$CLAUDE_CONFIG_DIR/plugins/cache/<marketplace>/<plugin>/<version>/` (`skills/`, `commands/`, `hooks/hooks.json`).
- *Settings schema questions* (is `ref` valid on a github marketplace source, which hook events exist) can be answered from the installed binary. Dump it with `strings -n 8 ~/.local/share/claude/versions/<version>` into a scratch file and search it with Python's `re`: ugrep in this shell stops with "exceeds complexity limits" on long `.{N}` patterns.
- *Push-guard impact of a new plugin.* List its hook events. Only a `PreToolUse` hook could pre-empt `.claude/hooks/git-push-guard.py`.
- *CI review.* `anthropics/claude-code-action` restores `.claude`, `.mcp.json`, `CLAUDE.md` and similar paths from the PR's base branch before it runs (`restoreConfigFromBase` in `src/github/operations/restore-config.ts`, read at its default branch with `gh api -H 'Accept: application/vnd.github.raw' repos/anthropics/claude-code-action/contents/<path>`). A PR's own settings and instruction changes therefore take effect in the CI review only after merge. The required checks of the `main` ruleset (Check, Build (release), Test (host), Security, Miri, Tools (host)) do not include the Claude review. To prove that a `rust-toolchain.toml` component exists for CI, read `https://static.rust-lang.org/dist/<date>/channel-rust-nightly.toml` (`pkg.<name>-preview.target.x86_64-unknown-linux-gnu.available`).
- *LSP.* The repo's `.claude/.lsp.json` was removed on 2026-10-05: the `rust-analyzer-lsp` plugin supplies the server and `rust-toolchain.toml` lists the `rust-analyzer` component. A reappearing `.lsp.json` is a duplicate.
- *`gh` behaviour.* Read gh's own source at the installed tag (`gh --version`), not memory:

  ```bash
  gh api -H 'Accept: application/vnd.github.raw' 'repos/cli/cli/contents/pkg/cmd/pr/merge/merge.go?ref=v2.102.0'
  ```

  `git/client.go` gives the exact git argument vectors and `git/objects.go` has `WorktreeForBranch`. The behaviour itself is documented where it is used, in `.claude/skills/merge-and-cleanup/SKILL.md`; the method is what this lesson keeps. At v2.102.0, `deleteLocalBranch` has these cases: head in the current linked worktree, warn and skip; head in the current main worktree, `git checkout <base>` (a failure returns an error, so gh exits non-zero and skips the remote delete), `git pull --ff-only` (a failure only warns), then `git branch -D`, unless `<base>` is checked out in another worktree, when gh warns, skips and exits 0; head in the main worktree while gh runs elsewhere, warn and skip; head in another linked worktree, `git worktree remove -- <path>` without `--force` (a failure warns and skips the local delete; the remote branch is still deleted afterwards), then `git branch -D`. A prunable (hand-deleted) head worktree is pruned first. `WorktreeForBranch` returns the first match. Worktree handling arrived in v2.99.0 (cli/cli#14007).

**Greps and anchors in harness docs.**

- Plain `rg` skips hidden directories, so a stale-reference sweep misses `.claude/` and `.github/` unless it runs with `--hidden` (and excludes `.claude/worktrees/**` and `.git/**`).
- Commit-pinned citations in ADRs (`docs/knowledge/decisions/`) and checked-off tasks in completed phase docs are history, not stale paths; only forward-looking rows ("Docs to update" tables for unimplemented steps) are worth flagging. The crash-fix ADR pins every `path:line` at e98e1ad and the capability-lifetime ADR at 746fe1f. A row that names `.claude/CLAUDE.md` but keeps old line numbers needs an `at <commit>` qualifier.

## How to avoid next time

- When a PR touches `.claude/settings.json` (`enabledPlugins`, `extraKnownMarketplaces`) or a skill that describes another tool, find the claim's source in the list above before accepting it.
- Say "ignored" for files a worktree removal deletes without asking, and "untracked or modified" for what makes git refuse.
- Related: `2026-10-06-jl-gates-in-worktrees.md` for the gates themselves.
