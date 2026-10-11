---
author: jl + claude
date: 2026-10-06
tags: [tooling]
status: final
---

# Lesson: Test shell snippets in zsh, one call at a time, without pushing

## What happened

Reviews of the `/merge-and-cleanup` skill and of other `bash` fences in `.claude/skills/*/SKILL.md` found snippets that only worked in bash, or only when a variable set in an earlier snippet was still there. Scratch-repository test harnesses were also refused by the push guard.

## Why it happened

- The Bash tool runs **zsh** (`/bin/zsh`, 5.9 here), not bash, even for a fence marked `bash`.
- Each Bash call starts a fresh shell: variables, functions and `cd` do not persist between calls.
- `.claude/hooks/git-push-guard.py` parses the command the way a shell does, including the script files a command runs. It refuses a `git push` in a helper script, even to a scratch bare repository, and a push to a branch named `main` even in a scratch repository.

## What we learned

The nomatch, pipeline, `${X:?}` and hex-arithmetic facts were re-run on 2026-10-10 with zsh 5.9 and bash; the rest come from the 2026-10-06 reviews.

- **nomatch.** An unmatched glob such as `dir/*/` aborts that command in zsh with `no matches found`, where bash passes the literal through.
- **Pipelines.** The last element of a pipeline runs in the current shell in zsh, so `exit 1` inside `... | while read ...; done` exits the whole block; in bash it exits only the subshell. (Both fail closed for an "ends with X" pattern.) `read -r -d ''` and `${x##*/}` work in both.
- **Guards.** `: "${X:?}"` at the top level of a block aborts it in both shells. Inside `$(...)` it only kills the subshell: the block continues with an empty value, so a guard must be a top-level `: "${X:?}"`. A skill that sets `FOO=...` in one snippet and uses `"$FOO"` in a later call gets an empty string; `[ -d "$FOO/x" ]` then silently skips, and `git -C ""` is a no-op that runs in the current directory.
- **Hex arithmetic.** zsh truncates a 16-digit hex literal in arithmetic (`number truncated after 15 digits`), which silently corrupts a 64-bit kernel address. Use literal hex in the command.
- **`grep -c` counts** (found 2026-10-10 in `scripts/agent/brief.sh`, Routing log). On an existing file with no match, `grep -c PATTERN file` prints `0` and exits 1, so `$(grep -c PATTERN file || echo 0)` gives `0` twice (`0\n0`) and breaks any line or arithmetic that uses it. On a missing file it prints nothing on stdout and exits 2. Capture the count without a fallback and supply the default when you expand it: `n=$(grep -c PATTERN file 2>/dev/null)`, then `${n:-0}`. That gives one `0` in both cases, in zsh and in bash. (Where `grep` and `rg` come from in the Bash tool: rule 10, Search Commands.)
- **Other platform facts** recorded in review (2026-10-06): `git worktree remove` without `--force` deletes ignored files silently; `git status --porcelain --ignored=matching` lists ignored files inside untracked directories and C-quotes paths holding spaces, tabs or quotes; git accepts `'` in branch names; Apple's `diff -rq` exits 0 when a directory is unreadable, so it is no proof that a copy is complete; `git worktree remove` on a tree with an unreadable directory deletes `.git` and the admin directory, then fails and leaves the files orphaned; macOS `cp -R src/ dest` copies the contents and merges on a rerun, where GNU `cp` nests `dest/src`; a `--force-with-lease=refs/heads/B:<sha>` takes a full ref name, and a bare `--delete B` fails with "matches more than one" when a tag has the same name.

## How to avoid next time

- Run every fenced block under both `bash` and `zsh`, each in its own call. Substitute placeholders (`<number>`, `<branch-name>`) with `sed` first and extract fences with `awk`; shellcheck is at `/opt/homebrew/bin/shellcheck`.
- Write a skill so that each block recomputes what it needs, and put the guard in a top-level `: "${VAR:?}"`.
- Build scratch repositories without `git push`. Seed a bare origin with `git init --bare`, then `git -C origin.git fetch <repo> 'refs/heads/*:refs/heads/*'`; create `refs/pull/N/head` with `update-ref`; then `remote add origin` and `fetch`. Never put a `git push` in a helper script, and write helper scripts with the Write tool rather than a heredoc in the command that runs them (the guard reads what the command is about to run, and it reads a heredoc rewrite too). Rule 03 forbids rephrasing a refused command to get past the guard; a push-free set-up avoids the refusal instead.
- Stub `gh` with a script on `PATH` that answers `--jq` queries from environment variables (`headRefOid` from the bare origin).
- A model of "origin" that needs a merge can be a plain non-bare repository, with a fake `gh` that merges by `git -C origin merge --squash`, and the clone only fetching.
- To test a check-then-act race in a script deterministically, shim an external command the script runs between the check and the act, and do the racing action in the shim's first call. The recipe, worked for `scripts/agent/qemu-lock.sh clear-stale`, is in [the qemu-lock testing lesson](2026-10-10-jl-qemu-lock-testing.md).
