#!/usr/bin/env bash
# scripts/agent/checkpoint.sh - deterministic work-in-progress checkpoint for `/justin:pause`.
#
# Commits and pushes only in the current checkout (a linked worktree is fine):
#  1. Commits only on a claude/* branch, never on main or a detached HEAD, and
#     not while a merge, cherry-pick, revert, rebase or bisect is in progress.
#  2. Stages everything that is not gitignored, except .remember/ and
#     __pycache__/ (excluded by pathspec, so this holds before .gitignore
#     lists them). Before any commit it scans the staged changes and the
#     local commits not yet on origin:
#     a path new to the repo that looks like a secret (.env, *.pem, *.key,
#     id_rsa*, *credential*, *secret*, ...) or an added line that looks like a
#     private key or access token stops the checkpoint. On a stop the index is
#     restored exactly and nothing is committed or pushed. --allow <path>
#     (repeatable, exact repo-relative path as listed) lets a flagged path
#     through; pass it only for paths the user confirmed are safe to publish.
#  3. Commits "wip: checkpoint <branch>" when anything is staged.
#  4. Pushes the branch under its own name when it has commits that are not on
#     origin, also when the tree was already clean. Never forced, never with
#     --no-verify. Wip commits stay local while the branch has an open PR that
#     is ready for review (each push to it starts CI and the Claude review),
#     and also when gh cannot tell whether such a PR exists.
#  5. Lists every other worktree that has uncommitted files or commits not on
#     origin. It never commits there: a background agent may be working in it.
#  6. Prints one checkpoint line, then "Safe to /clear" only when no worktree
#     holds work that is not on origin.
#
# Usage: scripts/agent/checkpoint.sh [--handoff saved|not-saved] [--allow <path>]...
# Exit status: 0 no worktree holds work that is not on origin; 1 uncommitted
# files or commits not on origin remain here or in another worktree (the output
# says where and why); 3 stopped on a possible secret; 2 usage error. Works
# with macOS bash 3.2 and GNU/Linux.

set -u

HANDOFF="not recorded"
ALLOW="" # newline-separated repo-relative paths (no arrays: bash 3.2 and set -u)
while [ $# -gt 0 ]; do
    case "$1" in
        --allow)
            case "${2:-}" in
                "" | -*)
                    echo "checkpoint.sh: --allow takes a repo-relative path" >&2
                    exit 2
                    ;;
            esac
            ALLOW="$ALLOW${2#./}
"
            shift
            ;;
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
            sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "checkpoint.sh: unknown argument: $1" >&2
            exit 2
            ;;
    esac
    shift
done

SELF="$(cd "$(dirname "$0")" && pwd -P)/$(basename "$0")" # before cd: $0 may be relative
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

secret_scan() { # prints "path<TAB>reason" per suspicious path; empty output means clean
    {
        git -c core.quotePath=false diff --cached --name-only --no-renames --diff-filter=A
        git -c core.quotePath=false log --format= --name-only --no-renames --diff-filter=A HEAD --not --remotes=origin 2>/dev/null
    } | grep -v '^$' | grep -E -i -e "$SECRET_PATH_RE" | awk '{ print $0 "\tfile name" }'
    {
        git -c core.quotePath=false diff --cached --no-color --no-ext-diff --no-textconv -U0
        git -c core.quotePath=false log -p --format= --no-color --no-ext-diff --no-textconv -U0 HEAD --not --remotes=origin 2>/dev/null
    } | added_lines | grep -E -e "$SECRET_TEXT_RE" | cut -f1 | awk '{ print $0 "\tkey or token in an added line" }'
}

split_allowed() { # stdin: secret_scan lines; $1 = "allowed" or "blocked" selects which to print
    ALLOW_LIST="$ALLOW" awk -F'\t' -v want="$1" '
        BEGIN { n = split(ENVIRON["ALLOW_LIST"], a, "\n"); for (i = 1; i <= n; i++) if (a[i] != "") ok[a[i]] = 1 }
        (want == "allowed") == ($1 in ok)'
}

describe_paths() { # stdin: "path<TAB>reason" lines; prints "  - path (reason)"
    awk -F'\t' '{ print "  - " $1 " (" $2 ")" }'
}

other_worktrees() { # prints "path<TAB>branch<TAB>uncommitted<TAB>not on origin" per other worktree holding work
    local here wt real b d u
    here=$(pwd -P)
    git worktree list --porcelain | awk '/^worktree /{print substr($0, 10)}' | while IFS= read -r wt; do
        [ -n "$wt" ] && [ -d "$wt" ] || continue
        real=$(cd "$wt" 2>/dev/null && pwd -P) || continue
        [ "$real" = "$here" ] && continue
        b=$(git -C "$wt" symbolic-ref --quiet --short HEAD 2>/dev/null || echo "detached HEAD")
        # --no-optional-locks: never take another worktree's index.lock from under an agent working there.
        d=$(git --no-optional-locks -C "$wt" status --porcelain --untracked-files=all -- . "${EXCLUDES[@]}" 2>/dev/null | wc -l | tr -d ' ')
        u=$(git -C "$wt" rev-list --count HEAD --not --remotes=origin 2>/dev/null || echo 0)
        if [ "$d" != 0 ] || [ "$u" != 0 ]; then
            printf '%s\t%s\t%s\t%s\n' "$wt" "$b" "$d" "$u"
        fi
    done
}

count_dirty() { git status --porcelain --untracked-files=all -- . "${EXCLUDES[@]}" 2>/dev/null | wc -l | tr -d ' '; }
count_unpushed() { git rev-list --count HEAD --not --remotes=origin 2>/dev/null || echo 0; }

branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
dirty=$(count_dirty)
wip=""
pushed=""
stopped=0
secrets=""
allowed=""

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
        secret_scan | sort -u >"$TMP/scan.txt"
        secrets=$(split_allowed blocked <"$TMP/scan.txt")
        allowed=$(split_allowed allowed <"$TMP/scan.txt")
        if [ -n "$secrets" ]; then
            git read-tree "$pre_index"
            wip="not committed (possible secret)"
            pushed="no (possible secret)"
            stopped=3
        elif git diff --cached --quiet; then
            wip="clean"
        else
            files=$(git diff --cached --name-only | wc -l | tr -d ' ')
            override=""
            if [ -n "$allowed" ]; then
                override="Secret scan overridden with --allow (confirmed safe by the user): $(printf '%s\n' "$allowed" | cut -f1 | sort -u | paste -sd ',' - | sed 's/,/, /g')"
            fi
            if git commit -q -m "wip: checkpoint $branch" \
                -m "Paused with /justin:pause; not a finished step." \
                ${override:+-m "$override"} \
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
            if git log --format=%s HEAD --not --remotes=origin | grep -q '^wip:'; then
                # Fail closed: without a definite "no ready PR" answer, wip commits stay local.
                if ! command -v gh >/dev/null 2>&1; then
                    hold="PR state unknown: gh is not installed"
                elif ! prs=$(GH_PROMPT_DISABLED=1 gh pr list --head "$branch" --state open --json number,isDraft \
                    --jq '.[] | select(.isDraft | not) | "#\(.number)"' 2>"$TMP/gh.err"); then
                    hold="PR state unknown: $(head -n 1 "$TMP/gh.err")"
                elif [ -n "$prs" ]; then
                    hold="open PR $(printf '%s\n' "$prs" | paste -sd ',' - | sed 's/,/, /g') is ready for review; mark it draft to push wip commits"
                fi
            fi
            if [ -n "$hold" ]; then
                pushed="no ($hold; wip kept local)"
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
if [ -n "$allowed" ]; then
    echo "Allowed by --allow (confirmed safe by the user):"
    printf '%s\n' "$allowed" | describe_paths
fi
unused=$(printf '%s' "$ALLOW" | while IFS= read -r a; do
    [ -n "$a" ] && ! printf '%s\n' "$allowed" | cut -f1 | grep -Fqx -e "$a" && echo "$a"
done)
[ -n "$unused" ] && [ -n "$ALLOW" ] && echo "--allow matched no flagged path: $(printf '%s\n' "$unused" | paste -sd ',' - | sed 's/,/, /g')"
if [ "$stopped" = 3 ]; then
    echo "Stopped before committing: these look like secrets (the repository is public):"
    printf '%s\n' "$secrets" | describe_paths
    echo "Nothing was committed or pushed and the index is as it was. Remove or gitignore them. If the user confirms a listed path is safe to publish, rerun with --allow <path> for each confirmed path, exactly as listed (/justin:pause asks first)."
    exit 3
fi

MAIN_WT=$(git worktree list --porcelain | awk '/^worktree /{print substr($0, 10); exit}')
other_worktrees >"$TMP/others.tsv"
others=$(wc -l <"$TMP/others.tsv" | tr -d ' ')
if [ "$others" != 0 ]; then
    echo "Other worktrees hold work that is not on origin (left untouched: an agent may be working there):"
    while IFS="$(printf '\t')" read -r wt b d u; do
        case "$wt" in
            "$MAIN_WT") label="(main checkout)" ;;
            "$MAIN_WT"/*) label=${wt#"$MAIN_WT"/} ;;
            *) label=$wt ;;
        esac
        echo "  - $label on $b: $d uncommitted file(s), $u commit(s) not on origin"
        case "$b" in
            claude/?*) echo "    checkpoint it: (cd '$wt' && bash '$SELF')" ;;
            *) echo "    not a claude/* branch: commit or push it by hand" ;;
        esac
    done <"$TMP/others.tsv"
fi
if [ "$dirty" = 0 ] && [ "$unpushed" = 0 ] && [ "$others" = 0 ]; then
    echo "Safe to /clear — run /justin:start after"
    exit 0
fi
if [ "$dirty" != 0 ] || [ "$unpushed" != 0 ]; then
    echo "Not fully saved: $dirty uncommitted file(s) and $unpushed commit(s) not on origin remain only in $ROOT."
fi
[ "$others" != 0 ] && echo "Not fully saved: $others other worktree(s) listed above hold work that is not on origin."
echo "/clear keeps all of it on disk; run /justin:start after."
exit 1
