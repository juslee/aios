#!/usr/bin/env bash
# scripts/agent/brief.sh - deterministic session briefing for `/justin:brief` (no LLM).
#
# Prints, as Markdown: git state (branch, dirty files, worktrees, unpushed
# commits), open PRs with check status and a merge-ready verdict, main CI, the
# newest soak of a main commit and the newest other soak, the .remember handoff,
# knowledge notes changed since the last session, open needs-human issues, the
# next unchecked step of the current phase doc, and a one-line docs-check summary.
#
# merge-ready is "yes" only for a non-draft PR with at least one check, every
# check passed, GitHub mergeable with merge state CLEAN, no changes requested,
# no unresolved review threads, and no open needs-human issue naming it (#N in
# the issue title gates the PR; #N in the body also blocks it). When the gates
# or the review threads cannot be read, the PR is not merge-ready. The Docs
# check never fails on drift (.github/workflows/docs.yml), so a failing Docs
# check is a checker error and counts like any other failing check; drift is
# reported in the Docs drift section, from a local docs-check run.
#
# Every section degrades to a one-line notice when git, gh, jq, python3 or the
# network is unavailable. Text from GitHub (titles, branch names) is printed as
# data with control characters replaced; it is never executed. Side effects:
# `git fetch --prune origin` (skip with --no-fetch) and a timestamp marker in
# the git common dir ($GIT_COMMON_DIR/aios-agent/last-brief).
#
# Usage: scripts/agent/brief.sh [--no-fetch]
# Works with macOS bash 3.2 and GNU/Linux.

set -u

FETCH=1
for arg in "$@"; do
    case "$arg" in
        --no-fetch) FETCH=0 ;;
        -h | --help)
            sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
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

# Open needs-human issues feed both the PR merge gates and the Needs human section.
ISSUES_OK=0
if [ "$GH_OK" = 1 ]; then
    if gh issue list --state open --label needs-human --limit 1000 --json number,title,body,updatedAt \
        >"$TMP/issues.json" 2>"$TMP/issues.err"; then
        ISSUES_OK=1
    fi
fi
[ "$ISSUES_OK" = 1 ] || echo '[]' >"$TMP/issues.json"

# Unresolved review threads per open PR (number -> count), newest 50 PRs.
THREADS_OK=0
if [ "$GH_OK" = 1 ]; then
    # shellcheck disable=SC2016 # $owner and $name are GraphQL variables, not shell expansions
    if gh api graphql -F owner='{owner}' -F name='{repo}' -f query='
        query($owner: String!, $name: String!) {
          repository(owner: $owner, name: $name) {
            pullRequests(states: OPEN, first: 50, orderBy: {field: CREATED_AT, direction: DESC}) {
              nodes { number reviewThreads(first: 100) { nodes { isResolved } } }
            }
          }
        }' --jq '[.data.repository.pullRequests.nodes[]
                  | {key: (.number | tostring), value: ([.reviewThreads.nodes[] | select(.isResolved | not)] | length)}]
                 | from_entries' >"$TMP/threads.json" 2>"$TMP/threads.err"; then
        THREADS_OK=1
    fi
fi
[ "$THREADS_OK" = 1 ] || echo '{}' >"$TMP/threads.json"

# Pull requests ---------------------------------------------------------------

section "Open pull requests"
PRS_OK=0
if [ "$GH_OK" = 1 ]; then
    if gh pr list --state open --limit 30 \
        --json number,title,headRefName,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,author \
        >"$TMP/prs.json" 2>"$TMP/prs.err"; then
        PRS_OK=1
        jq -r --slurpfile iss "$TMP/issues.json" --argjson issues_ok "$ISSUES_OK" \
            --slurpfile thr "$TMP/threads.json" --argjson threads_ok "$THREADS_OK" '
          def clean: tostring | gsub("[[:cntrl:]]"; "?");
          def refs: [(. // "") | scan("#([0-9]+)") | .[0] | tonumber] | unique;
          def names($n): refs | any(.[]; . == $n);
          $iss[0] as $nh
          | if length == 0 then "- none" else .[] |
            .number as $n
            | (if $threads_ok == 1 then $thr[0][$n | tostring] else null end) as $open_threads
            | ($nh | map(select(.title | names($n))) | map("#\(.number)")) as $gated
            | ($nh | map(select((.title | names($n) | not) and (.body | names($n)))) | map("#\(.number)")) as $named
            | ([.statusCheckRollup[]? | {name: (.name // .context // "?"),
               s: ((.conclusion // .state // .status // "") | ascii_upcase)}]) as $c
            | ($c | map(select(.s == "SUCCESS" or .s == "NEUTRAL" or .s == "SKIPPED")) | length) as $pass
            | ($c | map(select(.s == "FAILURE" or .s == "ERROR" or .s == "CANCELLED" or .s == "TIMED_OUT"
                               or .s == "ACTION_REQUIRED" or .s == "STARTUP_FAILURE"))) as $failed
            | (($c | length) - $pass - ($failed | length)) as $pend
            | .mergeStateStatus as $state
            | ([ (if .isDraft then "draft" else empty end),
                 (if ($gated | length) > 0 then "gated by \($gated | join(", "))" else empty end),
                 (if ($named | length) > 0 then "named in needs-human \($named | join(", "))" else empty end),
                 (if $issues_ok == 0 then "needs-human gates unknown" else empty end),
                 (if ($c | length) == 0 then "no checks reported" else empty end),
                 (if ($failed | length) > 0 then "\($failed | length) failing" else empty end),
                 (if $pend > 0 then "\($pend) pending" else empty end),
                 (if .mergeable != "MERGEABLE" then "mergeable: \(.mergeable)" else empty end),
                 (if ($state == "CLEAN" or $state == "DRAFT") then empty else "merge state: \($state)" end),
                 (if .reviewDecision == "CHANGES_REQUESTED" then "changes requested" else empty end),
                 (if $open_threads == null then "review threads unknown"
                  elif $open_threads > 0 then "\($open_threads) unresolved review thread(s)" else empty end)
               ]) as $blockers
            | "- #\(.number) \(.title | clean) [`\(.headRefName | clean)`, \(.author.login // "?" | clean)]\(if .isDraft then " (draft)" else "" end)\n"
              + "  checks: \($pass) pass, \($failed | length) fail, \($pend) pending"
              + (if ($failed | length) > 0 then " (failing: \($failed | map(.name | clean) | unique | join(", ")))" else "" end)
              + "; mergeable: \(.mergeable) / \($state); review: \(.reviewDecision // "" | if . == "" then "none" else . end)"
              + "; unresolved threads: \($open_threads // "unknown")\n"
              + "  merge-ready: " + (if ($blockers | length) == 0 then "yes" else "no (\($blockers | join("; ")))" end)
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
#
# scripts/soak-qemu.sh writes summary.tsv row by row and summary.md (with the
# commit, "<sha>" or "<sha>-dirty") when the run finishes. A run counts as a
# main soak only when it finished and its commit is on origin/main with no
# uncommitted changes; everything else (branch commits, dirty trees, runs still
# in progress) is reported separately so it is never read as main's state.

MAIN_TIP=$(git rev-parse -q --verify refs/remotes/origin/main 2>/dev/null || echo "")

soak_commit() { # $1 = run dir
    # shellcheck disable=SC2016 # the backticks are literal Markdown, not expansions
    grep -m 1 '^| Commit |' "$1/summary.md" 2>/dev/null | sed -n 's/^| Commit | `\([^`]*\)`.*/\1/p'
}

soak_relation() { # $1 = recorded commit; prints its relation to origin/main, returns 0 for a clean main commit
    local raw=$1 c full rel
    c=${raw%-dirty}
    if [ -z "$raw" ]; then
        echo "commit not recorded"
        return 1
    fi
    if ! full=$(git rev-parse -q --verify "$c^{commit}" 2>/dev/null); then
        echo "commit not in this repository"
        return 1
    fi
    if [ -z "$MAIN_TIP" ]; then
        echo "origin/main unknown"
        return 1
    fi
    if [ "$full" = "$MAIN_TIP" ]; then
        rel="origin/main tip"
    elif git merge-base --is-ancestor "$full" "$MAIN_TIP" 2>/dev/null; then
        rel="on main, $(git rev-list --count "$full..$MAIN_TIP") commit(s) behind origin/main"
    else
        rel="not on main"
    fi
    if [ "$c" != "$raw" ]; then
        echo "$rel, tree had uncommitted changes"
        return 1
    fi
    echo "$rel"
    [ "$rel" != "not on main" ]
}

print_soak() { # $1 = label, $2 = run dir, $3 = worktree, $4 = mtime
    local d=$2 b commit rel boots
    b=$(git -C "$3" symbolic-ref --quiet --short HEAD 2>/dev/null || echo "(detached)")
    if [ -f "$d/summary.md" ]; then
        commit=$(soak_commit "$d")
        rel=$(soak_relation "$commit")
        boots=$(grep -m 1 '^| Boots |' "$d/summary.md" | sed 's/^| Boots | \(.*\) |$/\1/')
        echo "- $1: $(rel_path "$d") (worktree on \`$b\`, $(fmt_epoch "$4" '+%Y-%m-%d %H:%M'))"
        echo "  - commit ${commit:-?} ($rel); ${boots:-boots not recorded}"
    else
        echo "- $1: $(rel_path "$d") (worktree on \`$b\`, $(fmt_epoch "$4" '+%Y-%m-%d %H:%M'))"
        echo "  - partial: no summary.md yet (run in progress or aborted); commit not recorded"
    fi
    if [ -f "$d/summary.tsv" ]; then
        awk -F'\t' -v partial="$([ -f "$d/summary.md" ] || echo " so far")" '
            NR > 1 { n++; c[$3]++; mode = $2 }
            END {
                if (n == 0) { print "  - summary.tsv has no rows yet"; exit }
                printf "  - %s mode%s: CLEAN %d/%d, PCZERO %d, PANIC %d, EXCEPTION %d, WEDGE %d\n",
                    mode, partial, c["CLEAN"], n, c["PCZERO"], c["PANIC"], c["EXCEPTION"], c["WEDGE"]
            }' "$d/summary.tsv"
    elif [ -f "$d/summary.md" ]; then
        grep -m 1 '^CLEAN rate:' "$d/summary.md" | sed 's/^/  - /'
    fi
}

section "Soak"
main_dir="" main_wt="" main_m=0
other_dir="" other_wt="" other_m=0
while IFS= read -r wt; do
    [ -n "$wt" ] || continue
    for d in "$wt"/target/soak/*/; do
        d=${d%/}
        if [ -f "$d/summary.md" ]; then
            m=$(mtime "$d/summary.md")
        elif [ -f "$d/summary.tsv" ]; then
            m=$(mtime "$d/summary.tsv")
        else
            continue
        fi
        if [ -f "$d/summary.md" ] && soak_relation "$(soak_commit "$d")" >/dev/null; then
            if [ "$m" -gt "$main_m" ]; then
                main_m=$m main_dir=$d main_wt=$wt
            fi
        elif [ "$m" -gt "$other_m" ]; then
            other_m=$m other_dir=$d other_wt=$wt
        fi
    done
done <<EOF
$WORKTREES
EOF
if [ -z "$main_dir$other_dir" ]; then
    echo "- no soak results (target/soak/*/summary.* in any worktree); run \`just soak\` on main once the harness is merged"
else
    if [ -n "$main_dir" ]; then
        print_soak "main soak" "$main_dir" "$main_wt" "$main_m"
    else
        echo "- main soak: none (no finished run of a commit on origin/main without local changes)"
    fi
    if [ -n "$other_dir" ]; then
        print_soak "newest other soak (not main's state)" "$other_dir" "$other_wt" "$other_m"
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
    if [ "$ISSUES_OK" = 1 ]; then
        jq -r '
          def clean: tostring | gsub("[[:cntrl:]]"; "?");
          if length == 0 then "- none" else
            sort_by(.number) | .[] | "- #\(.number) \(.title | clean) (updated \(.updatedAt[0:10]))" end
        ' "$TMP/issues.json"
    else
        echo "- GitHub unavailable: $(head -n 1 "$TMP/issues.err")"
    fi
    ready=$(gh issue list --state open --label agent-ready --limit 1000 --json number --jq length 2>/dev/null || echo "?")
    working=$(gh issue list --state open --label agent-working --limit 1000 --json number --jq length 2>/dev/null || echo "?")
    echo "- queue: $ready agent-ready, $working agent-working"
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
    next=$(awk '
        /^## / { ms = substr($0, 4); step = "" }
        /^### Step / { step = substr($0, 5) }
        /^[[:space:]]*[-*] \[ \] / && $0 !~ /\[ \] ~~/ {
            task = $0; sub(/^[[:space:]]*[-*] \[ \] /, "", task)
            printf "- next: %s / %s\n- first open task: %s\n", ms, (step == "" ? "(no step heading)" : step), task
            found = 1; exit
        }
        END { if (!found) print "- every task box in this doc is checked" }
    ' "$phase_doc")
    printf '%s\n' "$next"
    ms_num=$(printf '%s\n' "$next" | sed -n 's/^- next: Milestone \([0-9][0-9]*\).*/\1/p')
    if [ -n "$ms_num" ]; then
        if [ "$PRS_OK" = 1 ]; then
            # A "Phase N MK:" (or any "MK") PR title means that milestone is already in flight.
            jq -r --arg m "$ms_num" '
              def clean: tostring | gsub("[[:cntrl:]]"; "?");
              [.[] | select(.title | test("(^|[^A-Za-z0-9])M" + $m + "([^0-9]|$)"))]
              | if length == 0 then "- open PR for M\($m): none"
                else .[] | "- open PR for M\($m): #\(.number) \(.title | clean) (its merge-ready line is under Open pull requests)" end
            ' "$TMP/prs.json"
        else
            echo "- open PR for M$ms_num: unknown (GitHub unavailable)"
        fi
    fi
fi

# Docs drift -----------------------------------------------------------------------

section "Docs drift"
if [ -n "$DOCS_PID" ]; then
    wait "$DOCS_PID" 2>/dev/null
    rc=$(cat "$TMP/docs.rc" 2>/dev/null || echo "?")
    if [ "$rc" != 0 ] && [ "$rc" != 1 ]; then
        echo "- docs-check failed (exit $rc, a checker error, not drift): $(head -n 1 "$TMP/docs.err" 2>/dev/null)"
    elif ! command -v jq >/dev/null 2>&1; then
        echo "- docs-check ran ($([ "$rc" = 0 ] && echo "exit 0: no new drift" || echo "exit 1: new drift")) but jq is not installed to summarise it; run \`just docs-check\`"
    elif ! jq -e . "$TMP/docs.json" >/dev/null 2>&1; then
        echo "- docs-check ran (exit $rc) but its JSON output is unreadable: $(head -n 1 "$TMP/docs.err" 2>/dev/null)"
    else
        jq -r '
          "- docs-check: \(.summary.new) new vs baseline, \(.summary.total) total (\(.summary.baselined) baselined, of which \(.summary.accepted // 0) accepted false positives; \(.summary.resolved) resolved)"
          + (if .summary.new > 0 then "; new in: " + ([.checks | to_entries[] | select((.value.new // 0) > 0) | "\(.key) \(.value.new)"] | join(", ")) else "" end)
          + (if (.summary.resolved + (.summary.reduced // 0)) > 0 then "; run `just docs-check --update-baseline` to prune resolved or reduced entries" else "" end)
        ' "$TMP/docs.json"
    fi
else
    echo "- docs-check unavailable (needs python3 and scripts/docs/check.py on this checkout)"
fi

mkdir -p "$MARKER_DIR" 2>/dev/null && touch "$MARKER" 2>/dev/null
exit 0
