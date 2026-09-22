#!/usr/bin/env bash
# scripts/agent/checkpoint.sh - deterministic work-in-progress checkpoint for `/justin:pause`.
#
# Works on the current checkout (a linked worktree is fine):
#  1. Commits only on a claude/* branch, never on main or a detached HEAD, and
#     not while a merge, cherry-pick, revert, rebase or bisect is in progress.
#  2. Stages everything that is not gitignored, except .remember/ and
#     __pycache__/ (excluded by pathspec, so this holds before .gitignore
#     lists them). Before any
#     commit it scans the staged changes and the local commits not yet on origin:
#     a path new to the repo that looks like a secret (.env, *.pem, *.key,
#     id_rsa*, *credential*, *secret*, ...) or an added line that looks like a
#     private key or access token stops the checkpoint. On a stop the index is
#     restored exactly and nothing is committed or pushed.
#  3. Commits "wip: checkpoint <branch>" when anything is staged.
#  4. Pushes the branch under its own name when it has commits that are not on
#     origin, also when the tree was already clean. Never forced, never with
#     --no-verify. Wip commits stay local while the branch has an open PR that
#     is ready for review: each push to it starts CI and the Claude review.
#  5. Prints one checkpoint line and a resume hint.
#
# Usage: scripts/agent/checkpoint.sh [--handoff saved|not-saved]
# Exit status: 0 nothing is left only in this checkout; 1 uncommitted files or
# commits not on origin remain (the output says why); 3 stopped on a possible
# secret; 2 usage error. Works with macOS bash 3.2 and GNU/Linux.

set -u

HANDOFF="not recorded"
while [ $# -gt 0 ]; do
    case "$1" in
        --handoff)
            case "${2:-}" in
                saved) HANDOFF="saved" ;;
                not-saved) HANDOFF="not saved" ;;
                *)
                    echo "checkpoint.sh: --handoff takes 'saved' or 'not-saved'" >&2
                    exit 2
                    ;;
            esac
            shift
            ;;
        -h | --help)
            sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "checkpoint.sh: unknown argument: $1" >&2
            exit 2
            ;;
    esac
    shift
done

if ! ROOT=$(git rev-parse --show-toplevel 2>/dev/null); then
    echo "checkpoint.sh: not inside a git repository" >&2
    exit 2
fi
cd "$ROOT" || exit 2

TMP=$(mktemp -d "${TMPDIR:-/tmp}/aios-checkpoint.XXXXXX") || exit 2
trap 'rm -rf "$TMP"' EXIT

# Case-insensitive, matched against repo-relative paths that are new to the repo.
SECRET_PATH_RE='(^|/)(\.env(\.[^/]*)?|\.netrc|\.npmrc|\.pypirc|id_(rsa|dsa|ecdsa|ed25519)[^/]*|[^/]*\.(pem|key|p12|pfx|jks|keystore)|[^/]*(credential|secret)[^/]*)$'
# Matched against added lines: PEM private keys, GitHub, Anthropic, AWS and Slack tokens.
SECRET_TEXT_RE='-----BEGIN ([A-Z0-9]+ )*PRIVATE KEY-----|gh[pousr]_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{22,}|sk-ant-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|xox[abposr]-[A-Za-z0-9-]{10,}'
# Never committed, even where .gitignore does not list them yet.
EXCLUDES=(':(exclude).remember' ':(exclude,glob)**/__pycache__/**')

in_progress() { # prints the operation that blocks a commit, if any
    if git rev-parse -q --verify MERGE_HEAD >/dev/null 2>&1; then
        echo "merge in progress"
    elif git rev-parse -q --verify CHERRY_PICK_HEAD >/dev/null 2>&1; then
        echo "cherry-pick in progress"
    elif git rev-parse -q --verify REVERT_HEAD >/dev/null 2>&1; then
        echo "revert in progress"
    elif [ -e "$(git rev-parse --git-path rebase-merge)" ] || [ -e "$(git rev-parse --git-path rebase-apply)" ]; then
        echo "rebase in progress"
    elif [ -e "$(git rev-parse --git-path BISECT_LOG)" ]; then
        echo "bisect in progress"
    elif [ -n "$(git ls-files -u 2>/dev/null)" ]; then
        echo "unmerged paths"
    fi
}

added_lines() { # reads a patch on stdin; prints "path<TAB>added line" for each added line
    awk '
        /^diff --git / { hdr = 1; next }
        hdr && /^\+\+\+ / { f = substr($0, 5); sub(/^b\//, "", f); next }
        /^@@/ { hdr = 0; next }
        !hdr && /^\+/ { print f "\t" substr($0, 2) }
    '
}

secret_scan() { # prints one line per suspicious path; empty output means clean
    {
        git -c core.quotePath=false diff --cached --name-only --no-renames --diff-filter=A
        git -c core.quotePath=false log --format= --name-only --no-renames --diff-filter=A HEAD --not --remotes=origin 2>/dev/null
    } | grep -v '^$' | grep -E -i -e "$SECRET_PATH_RE" | sed 's/$/ (file name)/'
    {
        git -c core.quotePath=false diff --cached --no-color --no-ext-diff --no-textconv -U0
        git -c core.quotePath=false log -p --format= --no-color --no-ext-diff --no-textconv -U0 HEAD --not --remotes=origin 2>/dev/null
    } | added_lines | grep -E -e "$SECRET_TEXT_RE" | cut -f1 | sed 's/$/ (key or token in an added line)/'
}

count_dirty() { git status --porcelain --untracked-files=all -- . "${EXCLUDES[@]}" 2>/dev/null | wc -l | tr -d ' '; }
count_unpushed() { git rev-list --count HEAD --not --remotes=origin 2>/dev/null || echo 0; }

branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
dirty=$(count_dirty)
wip=""
pushed=""
stopped=0
secrets=""

case "$branch" in
    "") block="detached HEAD" ;;
    claude/?*) block=$(in_progress) ;;
    main) block="on main" ;;
    *) block="$branch is not a claude/* branch" ;;
esac

if [ -n "$block" ]; then
    wip=$([ "$dirty" = 0 ] && echo "clean" || echo "not committed ($block)")
    pushed="no ($block)"
else
    if ! pre_index=$(git write-tree 2>/dev/null); then
        wip="not committed (cannot snapshot the index)"
        pushed="no (index error)"
        stopped=1
    else
        git add -A -- . "${EXCLUDES[@]}"
        secrets=$(secret_scan | sort -u)
        if [ -n "$secrets" ]; then
            git read-tree "$pre_index"
            wip="not committed (possible secret)"
            pushed="no (possible secret)"
            stopped=3
        elif git diff --cached --quiet; then
            wip="clean"
        else
            files=$(git diff --cached --name-only | wc -l | tr -d ' ')
            if git commit -q -m "wip: checkpoint $branch" \
                -m "Paused with /justin:pause; not a finished step." \
                -m "Co-Authored-By: Claude <noreply@anthropic.com>" 2>"$TMP/commit.err"; then
                wip="committed $files file(s)"
            else
                git read-tree "$pre_index"
                wip="not committed (git commit failed: $(head -n 1 "$TMP/commit.err"))"
                stopped=1
            fi
        fi
    fi
    if [ -z "$pushed" ]; then
        if [ "$(count_unpushed)" = 0 ]; then
            pushed="no (nothing to push)"
        else
            hold=""
            if git log --format=%s HEAD --not --remotes=origin | grep -q '^wip:' && command -v gh >/dev/null 2>&1; then
                pr=$(gh pr view "$branch" --json number,state,isDraft \
                    --jq 'select(.state == "OPEN" and (.isDraft | not)) | .number' 2>/dev/null || true)
                [ -n "$pr" ] && hold="#$pr"
            fi
            if [ -n "$hold" ]; then
                pushed="no (open PR $hold is ready for review; wip kept local, mark it draft to push wip commits)"
            elif GIT_TERMINAL_PROMPT=0 git push -q -u origin "refs/heads/$branch:refs/heads/$branch" 2>"$TMP/push.err"; then
                pushed="yes"
            else
                why=$(grep -m 1 -E '\[(remote )?rejected\]|^(error|fatal|remote):' "$TMP/push.err" || grep -m 1 -v '^$' "$TMP/push.err")
                why=${why#"${why%%[![:space:]!]*}"} # drop git's leading " ! " marker
                why=$(printf '%s' "$why" | tr -s ' ')
                pushed="no (rejected: $why)"
            fi
        fi
    fi
fi

dirty=$(count_dirty)
unpushed=$(count_unpushed)
echo "checkpoint: ${branch:-(detached)} @ $(git rev-parse --short HEAD 2>/dev/null || echo none) | wip: $wip | pushed: $pushed | handoff: $HANDOFF"
if [ "$stopped" = 3 ]; then
    echo "Stopped before committing: these look like secrets (the repository is public):"
    printf '%s\n' "$secrets" | sed 's/^/  - /'
    echo "Nothing was committed or pushed and the index is as it was. Remove or gitignore them, or confirm they are safe, then run /justin:pause again."
    exit 3
fi
if [ "$dirty" = 0 ] && [ "$unpushed" = 0 ]; then
    echo "Safe to /clear — run /justin:start after"
    exit 0
fi
echo "Not fully saved: $dirty uncommitted file(s) and $unpushed commit(s) not on origin remain only in $ROOT. /clear keeps them on disk; run /justin:start after."
exit 1
