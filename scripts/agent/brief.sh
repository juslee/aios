#!/usr/bin/env bash
# scripts/agent/brief.sh - deterministic session briefing for `/start` (no LLM).
#
# Prints, as Markdown: git state (branch, dirty files, worktrees, unpushed
# commits), open PRs with check status and mergeability, main CI, the latest
# soak summary, the .remember handoff, knowledge notes changed since the last
# session, open needs-human issues, the next unchecked step of the current
# phase doc, and a one-line docs-check summary.
#
# Every section degrades to a one-line notice when git, gh, jq, python3 or the
# network is unavailable. Read-only apart from `git fetch` and a timestamp
# marker in the git common dir ($GIT_COMMON_DIR/aios-agent/last-brief).
#
# Usage: scripts/agent/brief.sh [--no-fetch]
# Works with macOS bash 3.2 and GNU/Linux.

set -u

FETCH=1
for arg in "$@"; do
    case "$arg" in
        --no-fetch) FETCH=0 ;;
        -h | --help)
            sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "brief.sh: unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

if ! ROOT=$(git rev-parse --show-toplevel 2>/dev/null); then
    echo "brief.sh: not inside a git repository" >&2
    exit 2
fi
cd "$ROOT" || exit 2
COMMON=$(cd "$(git rev-parse --git-common-dir)" && pwd)
WORKTREES=$(git worktree list --porcelain | awk '/^worktree /{print substr($0, 10)}')
MAIN_WT=$(printf '%s\n' "$WORKTREES" | head -n 1)
MARKER_DIR="$COMMON/aios-agent"
MARKER="$MARKER_DIR/last-brief"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/aios-brief.XXXXXX") || exit 2
trap 'rm -rf "$TMP"' EXIT

# Portable helpers ------------------------------------------------------------

mtime() { stat -c %Y "$1" 2>/dev/null || stat -f %m "$1" 2>/dev/null || echo 0; }

fmt_epoch() { # $1 = epoch seconds, $2 = date format
    date -r "$1" "$2" 2>/dev/null || date -d "@$1" "$2" 2>/dev/null || echo "$1"
}

section() { printf '\n## %s\n\n' "$1"; }

rel_path() { # path relative to the main checkout when inside it
    case "$1" in
        "$MAIN_WT") echo "(main checkout)" ;;
        "$MAIN_WT"/*) echo "${1#"$MAIN_WT"/}" ;;
        *) echo "$1" ;;
    esac
}

unpushed_desc() { # $1 = worktree path
    local b n m
    if ! b=$(git -C "$1" symbolic-ref --quiet --short HEAD 2>/dev/null); then
        echo "detached HEAD"
        return
    fi
    if git -C "$1" rev-parse -q --verify "refs/remotes/origin/$b" >/dev/null; then
        n=$(git -C "$1" rev-list --count "origin/$b..HEAD" 2>/dev/null || echo "?")
        m=$(git -C "$1" rev-list --count "HEAD..origin/$b" 2>/dev/null || echo "?")
        echo "$n unpushed, $m behind origin/$b"
    else
        n=$(git -C "$1" rev-list --count HEAD --not --remotes 2>/dev/null || echo "?")
        echo "not on origin; $n commit(s) on no remote"
    fi
}

# Start docs-check early; it is the slowest local step.
if [ -f scripts/docs/check.py ] && command -v python3 >/dev/null 2>&1; then
    (
        python3 scripts/docs/check.py --json >"$TMP/docs.json" 2>"$TMP/docs.err"
        echo $? >"$TMP/docs.rc"
    ) &
    DOCS_PID=$!
else
    DOCS_PID=""
fi

printf '# AIOS brief - %s\n' "$(date '+%Y-%m-%d %H:%M %Z')"

# Git -------------------------------------------------------------------------

section "Git"
if [ "$FETCH" = 1 ]; then
    if ! GIT_TERMINAL_PROMPT=0 git fetch --quiet --prune origin 2>"$TMP/fetch.err"; then
        echo "- fetch failed ($(head -n 1 "$TMP/fetch.err")); remote refs may be stale"
    fi
fi
branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || echo "(detached)")
dirty=$(git status --porcelain 2>/dev/null | wc -l | tr -d ' ')
echo "- checkout: $(rel_path "$ROOT") on \`$branch\` @ $(git log -1 --format='%h %s' 2>/dev/null)"
echo "- uncommitted: $dirty file(s); $(unpushed_desc "$ROOT")"
if git rev-parse -q --verify refs/remotes/origin/main >/dev/null; then
    echo "- origin/main @ $(git log -1 --format='%h %s (%cr)' origin/main)"
    if git rev-parse -q --verify refs/heads/main >/dev/null; then
        behind=$(git rev-list --count main..origin/main 2>/dev/null || echo "?")
        [ "$behind" != "0" ] && echo "- local main is $behind commit(s) behind origin/main"
    fi
fi
echo "- worktrees:"
while IFS= read -r wt; do
    [ -n "$wt" ] || continue
    if [ ! -d "$wt" ]; then
        echo "  - $(rel_path "$wt"): missing (run \`git worktree prune\`)"
        continue
    fi
    wb=$(git -C "$wt" symbolic-ref --quiet --short HEAD 2>/dev/null || echo "(detached)")
    wd=$(git -C "$wt" status --porcelain 2>/dev/null | wc -l | tr -d ' ')
    echo "  - $(rel_path "$wt"): \`$wb\`, $wd uncommitted, $(unpushed_desc "$wt")"
done <<EOF
$WORKTREES
EOF

# GitHub probe ----------------------------------------------------------------

GH_OK=0
GH_WHY=""
if ! command -v gh >/dev/null 2>&1; then
    GH_WHY="gh is not installed"
elif ! command -v jq >/dev/null 2>&1; then
    GH_WHY="jq is not installed"
elif ! remaining=$(gh api rate_limit --jq '.resources.core.remaining' 2>"$TMP/gh.err"); then
    GH_WHY=$(head -n 1 "$TMP/gh.err")
elif [ "${remaining:-0}" = "0" ]; then
    GH_WHY="API rate limit exhausted"
else
    GH_OK=1
fi
gh_unavailable() { echo "- GitHub unavailable: ${GH_WHY:-unknown error}"; }

# Pull requests ---------------------------------------------------------------

section "Open pull requests"
if [ "$GH_OK" = 1 ]; then
    if gh pr list --state open --limit 30 \
        --json number,title,headRefName,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,author \
        >"$TMP/prs.json" 2>"$TMP/prs.err"; then
        jq -r '
          if length == 0 then "- none" else .[] |
            ([.statusCheckRollup[]? | {name: (.name // .context // "?"),
               s: ((.conclusion // .state // .status // "") | ascii_upcase)}]) as $c
            | ($c | map(select(.s == "SUCCESS" or .s == "NEUTRAL" or .s == "SKIPPED")) | length) as $pass
            | ($c | map(select(.s == "FAILURE" or .s == "ERROR" or .s == "CANCELLED" or .s == "TIMED_OUT"
                               or .s == "ACTION_REQUIRED" or .s == "STARTUP_FAILURE"))) as $failed
            | (($c | length) - $pass - ($failed | length)) as $pend
            | "- #\(.number) \(.title) [`\(.headRefName)`, \(.author.login // "?")]\(if .isDraft then " (draft)" else "" end)\n"
              + "  checks: \($pass) pass, \($failed | length) fail, \($pend) pending"
              + (if ($failed | length) > 0 then " (failing: \($failed | map(.name) | unique | join(", ")))" else "" end)
              + "; mergeable: \(.mergeable) / \(.mergeStateStatus); review: \(.reviewDecision // "" | if . == "" then "none" else . end)"
          end' "$TMP/prs.json"
    else
        echo "- GitHub unavailable: $(head -n 1 "$TMP/prs.err")"
    fi
else
    gh_unavailable
fi

# Main CI ---------------------------------------------------------------------

section "Main CI"
if [ "$GH_OK" = 1 ]; then
    tip=$(git rev-parse -q --verify refs/remotes/origin/main 2>/dev/null || echo "")
    if gh run list --branch main --limit 40 \
        --json workflowName,status,conclusion,headSha,createdAt,event \
        >"$TMP/runs.json" 2>"$TMP/runs.err"; then
        jq -r --arg tip "$tip" '
          map(select(.event == "push" or .event == "schedule" or .event == "workflow_dispatch"))
          | if length == 0 then "- no runs on main" else
            group_by(.workflowName) | map(max_by(.createdAt)) | sort_by(.workflowName) | .[]
            | "- \(.workflowName): \(if .status == "completed" then .conclusion else .status end)"
              + " @ \(.headSha[0:7]) (\(.event), \(.createdAt[0:16] | sub("T"; " ")) UTC)"
              + (if $tip != "" and .headSha != $tip then " - not the origin/main tip" else "" end)
          end' "$TMP/runs.json"
    else
        echo "- GitHub unavailable: $(head -n 1 "$TMP/runs.err")"
    fi
else
    gh_unavailable
fi

# Soak ------------------------------------------------------------------------

section "Latest soak"
latest=""
latest_m=0
while IFS= read -r wt; do
    [ -n "$wt" ] || continue
    for f in "$wt"/target/soak/*/summary.tsv "$wt"/target/soak/*/summary.md; do
        [ -f "$f" ] || continue
        m=$(mtime "$f")
        if [ "$m" -gt "$latest_m" ]; then
            latest_m=$m
            latest=$f
        fi
    done
done <<EOF
$WORKTREES
EOF
if [ -z "$latest" ]; then
    echo "- no soak results (target/soak/*/summary.* in any worktree); run \`just soak\` once the harness is merged"
else
    dir=$(dirname "$latest")
    echo "- $(rel_path "$dir") ($(fmt_epoch "$latest_m" '+%Y-%m-%d %H:%M'))"
    if [ -f "$dir/summary.md" ]; then
        # shellcheck disable=SC2016 # the backticks are literal Markdown, not expansions
        commit=$(grep -m 1 '^| Commit |' "$dir/summary.md" | sed -n 's/^| Commit | `\([^`]*\)`.*/\1/p')
        boots=$(grep -m 1 '^| Boots |' "$dir/summary.md" | sed 's/^| Boots | \(.*\) |$/\1/')
        [ -n "$commit$boots" ] && echo "- commit ${commit:-?}; ${boots:-?}"
    fi
    if [ -f "$dir/summary.tsv" ]; then
        awk -F'\t' 'NR > 1 { n++; c[$3]++; mode = $2 }
            END {
                if (n == 0) { print "- summary.tsv has no rows"; exit }
                printf "- %s mode: CLEAN %d/%d, PCZERO %d, PANIC %d, EXCEPTION %d, WEDGE %d\n",
                    mode, c["CLEAN"], n, c["PCZERO"], c["PANIC"], c["EXCEPTION"], c["WEDGE"]
            }' "$dir/summary.tsv"
    else
        grep -m 1 '^CLEAN rate:' "$dir/summary.md" | sed 's/^/- /'
    fi
fi

# Handoff ---------------------------------------------------------------------

section "Handoff (.remember)"
REMEMBER_DIR="$MAIN_WT/.remember"
if [ -f "$REMEMBER_DIR/remember.md" ]; then
    echo "remember.md ($(fmt_epoch "$(mtime "$REMEMBER_DIR/remember.md")" '+%Y-%m-%d %H:%M')):"
    echo
    head -n 60 "$REMEMBER_DIR/remember.md"
else
    echo "- no handoff at $REMEMBER_DIR/remember.md"
fi
if [ -s "$REMEMBER_DIR/now.md" ]; then
    echo
    echo "now.md (last entries):"
    echo
    tail -n 12 "$REMEMBER_DIR/now.md"
fi

# Knowledge changed since last session -----------------------------------------

section "Knowledge notes changed since last session"
if [ -f "$REMEMBER_DIR/remember.md" ]; then
    since=$(mtime "$REMEMBER_DIR/remember.md")
    since_label="last handoff"
elif [ -f "$MARKER" ]; then
    since=$(mtime "$MARKER")
    since_label="last brief"
else
    since=$(($(date +%s) - 7 * 86400))
    since_label="7 days ago (no handoff or marker yet)"
fi
since_iso=$(fmt_epoch "$since" '+%Y-%m-%dT%H:%M:%S%z')
echo "- since $since_label: $(fmt_epoch "$since" '+%Y-%m-%d %H:%M')"
{
    git log --all --since="$since_iso" --name-only --format= -- docs/knowledge 2>/dev/null
    git status --porcelain -- docs/knowledge 2>/dev/null | awk '{print $NF " (uncommitted)"}'
} | grep -v '^$' | grep -v '/_template\.md' | sort -u >"$TMP/knowledge.txt"
if [ -s "$TMP/knowledge.txt" ]; then
    head -n 25 "$TMP/knowledge.txt" | sed 's/^/- /'
    total=$(wc -l <"$TMP/knowledge.txt" | tr -d ' ')
    [ "$total" -gt 25 ] && echo "- ... and $((total - 25)) more"
else
    echo "- none"
fi

# Issues ----------------------------------------------------------------------

section "Needs human"
if [ "$GH_OK" = 1 ]; then
    if gh issue list --state open --limit 100 --json number,title,labels,updatedAt \
        >"$TMP/issues.json" 2>"$TMP/issues.err"; then
        jq -r '
          def has($l): any(.labels[]?; .name == $l);
          (map(select(has("needs-human")))) as $h
          | (if ($h | length) == 0 then "- none" else
              ($h | sort_by(.number) | .[] | "- #\(.number) \(.title) (updated \(.updatedAt[0:10]))") end),
            "- queue: \(map(select(has("agent-ready"))) | length) agent-ready, \(map(select(has("agent-working"))) | length) agent-working"
        ' "$TMP/issues.json"
    else
        echo "- GitHub unavailable: $(head -n 1 "$TMP/issues.err")"
    fi
else
    gh_unavailable
fi

# Next phase step ---------------------------------------------------------------

section "Next phase-doc step"
phase_doc=""
for f in docs/phases/[0-9]*.md; do
    [ -f "$f" ] || continue
    status=$(grep -m 1 '^\*\*Status:\*\*' "$f" | sed 's/^\*\*Status:\*\* *//')
    case "$status" in
        "In Progress"*)
            phase_doc=$f
            break
            ;;
        Complete*) ;;
        *) [ -z "$phase_doc" ] && phase_doc=$f ;;
    esac
done
if [ -z "$phase_doc" ]; then
    echo "- no phase doc in progress (all docs/phases/*.md are Complete)"
else
    status=$(grep -m 1 '^\*\*Status:\*\*' "$phase_doc" | sed 's/^\*\*Status:\*\* *//')
    echo "- $phase_doc: $status"
    awk '
        /^## / { ms = substr($0, 4); step = "" }
        /^### Step / { step = substr($0, 5) }
        /^[[:space:]]*[-*] \[ \] / && $0 !~ /\[ \] ~~/ {
            task = $0; sub(/^[[:space:]]*[-*] \[ \] /, "", task)
            printf "- next: %s / %s\n- first open task: %s\n", ms, (step == "" ? "(no step heading)" : step), task
            found = 1; exit
        }
        END { if (!found) print "- every task box in this doc is checked" }
    ' "$phase_doc"
fi

# Docs drift -----------------------------------------------------------------------

section "Docs drift"
if [ -n "$DOCS_PID" ]; then
    wait "$DOCS_PID" 2>/dev/null
    rc=$(cat "$TMP/docs.rc" 2>/dev/null || echo "?")
    if { [ "$rc" = 0 ] || [ "$rc" = 1 ]; } && command -v jq >/dev/null 2>&1 && jq -e . "$TMP/docs.json" >/dev/null 2>&1; then
        jq -r '
          "- docs-check: \(.summary.new) new vs baseline, \(.summary.total) total (\(.summary.baselined) baselined, \(.summary.resolved) resolved)"
          + (if .summary.new > 0 then "; new in: " + ([.checks | to_entries[] | select((.value.new // 0) > 0) | "\(.key) \(.value.new)"] | join(", ")) else "" end)
          + (if .summary.resolved > 0 then "; run `just docs-check --update-baseline` to prune resolved entries" else "" end)
        ' "$TMP/docs.json"
    else
        echo "- docs-check failed (exit $rc): $(head -n 1 "$TMP/docs.err" 2>/dev/null)"
    fi
else
    echo "- docs-check unavailable (needs python3 and scripts/docs/check.py on this checkout)"
fi

mkdir -p "$MARKER_DIR" 2>/dev/null && touch "$MARKER" 2>/dev/null
exit 0
