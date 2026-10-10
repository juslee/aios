#!/bin/sh
# scripts/agent/qemu-lock.sh - the host QEMU lock (rule 11) until R4b's lease.
#
# One QEMU workload runs on this host at a time. The lock is a directory,
# <git-common-dir>/aios-agent/qemu.lock, shared by every worktree. mkdir is
# atomic, so two callers cannot both take it. An owner file inside records who
# holds it, including the wrapper's own pid. Every QEMU start on this host goes
# through `run` below. The PreToolUse guard (.claude/hooks/git-push-guard.py,
# rule 1) denies any other QEMU-starting command, and every one while the lock
# directory exists. Among agents it lets only the verifier call `run`; team
# leads never boot; the main thread of an owner or solo session may call `run`
# itself (rule 11).
#
# Usage:
#   qemu-lock.sh status
#       Print "free", or the owner lines plus a `state=` line: live (the
#       wrapper runs), starting (the owner file is not written yet and the lock
#       is under 2 minutes old) or dead (the wrapper is gone: a SIGKILL or a
#       closed session skipped its traps). An `overdue=1` line follows when the
#       eta has passed. Exit 0.
#   qemu-lock.sh stale
#       Exit 0, printing the owner lines, when the holder is dead and no
#       qemu-system-aarch64 process runs; exit 1 otherwise. That is all it
#       checks: a SIGKILLed wrapper (state=dead) orphans CMD, which can still
#       run without a QEMU. Look for leftover harness processes (ps, pgrep -f
#       for `aios soak`, the soak script, CMD itself) before clear-stale.
#   qemu-lock.sh clear-stale
#       When `stale` holds, append the owner lines to
#       <git-common-dir>/aios-agent/qemu-lock.log, remove the lock and print
#       "cleared"; exit 0. Otherwise print why not and exit 1. The lock moves
#       to a private name first and is checked again there, so two concurrent
#       calls, or a `run` that takes the lock meanwhile, never lose a live
#       holder's lock. Any lead may run it. A live holder past its eta is never cleared here: report
#       LOCK-STALE to the holder's lead (rule 11).
#   qemu-lock.sh run --team T --mode boot|quiet --label L --eta-min N
#                    [--max-load X] [--settle-min M] -- CMD...
#       T is team-build, team-fix or solo. Take the lock, run CMD, and release
#       the lock when CMD ends or this script is interrupted or terminated (a
#       TERM to pid= in the owner file also stops CMD and everything it
#       started). Stopping goes in two passes over the processes CMD started:
#       first every one but QEMU, parents first, so a harness that supervises
#       its own QEMU (`aios soak`, `just soak`: QEMU runs in its own process
#       group and the harness stops it on TERM, exit 143) ends the boot
#       itself instead of recording a QEMU killed under it as a boot; then,
#       once none of those runs any more or after a grace period (default
#       15 s, above the harness's 10 s SIGTERM-to-SIGKILL grace), whatever is
#       left, QEMU included. A bare QEMU (`just run*`) loses its parents at
#       once, so it is stopped without the wait. A process that ignores TERM
#       is then sent SIGKILL. The lock is released only when nothing of the
#       tree survives; otherwise the lock stays (status shows state=dead, with
#       a `survivors=` line), the survivors go to stderr and the exit status
#       is 78.
#       CMD runs with stdin from /dev/null.
#       Before CMD runs, with the lock held:
#         - exit 76, lock released, when a qemu-system-aarch64 process already
#           runs (a boot that bypassed the lock);
#         - exit 77 ("deferred: load"), lock released, when the 1-minute load
#           average is above X (default 30 in boot mode, 3.0 in quiet mode).
#           Quiet mode first waits up to M minutes (default 5) for the load to
#           settle, re-checking every 30 seconds.
#       Exit 75 without running anything when the lock is held. Otherwise exit
#       with CMD's status. One run covers one whole window: an A/B or quiet
#       session passes a loop that alternates the arms as CMD.
#
# Test hooks: AIOS_QEMU_LOCK_LOADAVG replaces the measured load average,
# AIOS_QEMU_LOCK_SETTLE_STEP the 30-second settle interval (seconds), and
# AIOS_QEMU_LOCK_GRACE the 15-second stop grace (seconds), and
# AIOS_QEMU_LOCK_KILL the command that sends SIGKILL (default kill).
#
# R4b replaces this script with a flock lease inside `aios soak` and the
# `just run*` recipes, and changes guard rule 1 to read the lease.

set -u

# The lock belongs to the repository this script lives in, not the caller's cwd.
common=$(git -C "$(dirname -- "$0")" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || {
    echo "qemu-lock: not in a git repository" >&2
    exit 2
}
lock="$common/aios-agent/qemu.lock"
log="$common/aios-agent/qemu-lock.log"

# QEMU processes, matched on argv0 so that a shell whose command line merely
# mentions the name (this script's own, or the caller's) does not count.
qemu_pids() {
    pgrep -f '^([^ ]*/)?qemu-system-aarch64( |$)' 2>/dev/null
}

# The owner lines of the lock directory $1 (default: the lock).
owner_lines() {
    if [ -f "${1:-$lock}/owner" ]; then
        cat "${1:-$lock}/owner"
    else
        echo "held (no owner file yet)"
    fi
}

# live, starting or dead. The pid is the wrapper's own ($$ when it wrote the
# owner file); a reused pid that is not this script's `run` counts as dead.
self=$(basename -- "$0")
holder_state() {
    pid=$(sed -n 's/^pid=//p' "${1:-$lock}/owner" 2>/dev/null)
    if [ -z "$pid" ]; then
        if [ -n "$(find "${1:-$lock}" -prune -mmin +2 2>/dev/null)" ]; then
            echo dead
        else
            echo starting
        fi
        return
    fi
    case "$(ps -o command= -p "$pid" 2>/dev/null)" in
    *"$self"*" run "*) echo live ;;
    *) echo dead ;;
    esac
}

load1() {
    if [ -n "${AIOS_QEMU_LOCK_LOADAVG:-}" ]; then
        echo "$AIOS_QEMU_LOCK_LOADAVG"
    elif v=$(LC_ALL=C sysctl -n vm.loadavg 2>/dev/null); then
        echo "$v" | tr -d '{}' | LC_ALL=C awk '{print $1}'
    else
        LC_ALL=C cut -d' ' -f1 /proc/loadavg 2>/dev/null
    fi
}

# Exit status 0 when $1 > $2 (decimal numbers).
above() {
    LC_ALL=C awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 > b + 0) }'
}

is_stale() {
    [ -d "$lock" ] || return 1
    [ "$(holder_state)" = dead ] || return 1
    [ -z "$(qemu_pids)" ]
}

cmd=${1:-}
[ $# -gt 0 ] && shift

case "$cmd" in
status)
    if [ -d "$lock" ]; then
        owner_lines
        echo "state=$(holder_state)"
        eta=$(sed -n 's/^eta=//p' "$lock/owner" 2>/dev/null)
        case "$eta" in '' | *[!0-9]*) ;; *) [ "$(date +%s)" -gt "$eta" ] && echo "overdue=1" ;; esac
    else
        echo free
    fi
    exit 0
    ;;
stale)
    is_stale || exit 1
    owner_lines
    exit 0
    ;;
clear-stale)
    if ! is_stale; then
        if [ ! -d "$lock" ]; then
            echo "free: nothing to clear"
        elif [ -n "$(qemu_pids)" ]; then
            echo "not cleared: a qemu-system-aarch64 process runs"
        else
            echo "not cleared: holder is $(holder_state)"
        fi
        exit 1
    fi
    # Not atomic with is_stale: move the lock to a private name (atomic), then
    # check that what we hold is still the stale holder before removing it.
    stale_pid=$(sed -n 's/^pid=//p' "$lock/owner" 2>/dev/null)
    moved="$lock.clearing.$$"
    if ! mv "$lock" "$moved" 2>/dev/null; then
        echo "not cleared: the lock changed"
        exit 1
    fi
    moved_pid=$(sed -n 's/^pid=//p' "$moved/owner" 2>/dev/null)
    if [ "$moved_pid" != "$stale_pid" ] || [ "$(holder_state "$moved")" != dead ] || [ -n "$(qemu_pids)" ]; then
        if [ ! -e "$lock" ] && mv "$moved" "$lock" 2>/dev/null; then
            echo "not cleared: the lock changed hands"
        else
            echo "not cleared: the lock changed hands and a new one exists; left at $moved"
        fi
        exit 1
    fi
    {
        echo "cleared_at=$(date +%s) by_pid=$$"
        owner_lines "$moved"
        echo "--"
    } >>"$log"
    rm -rf "$moved"
    echo cleared
    exit 0
    ;;
run) ;;
*)
    echo "usage: qemu-lock.sh status | stale | clear-stale | run --team T --mode boot|quiet --label L --eta-min N [--max-load X] [--settle-min M] -- CMD..." >&2
    exit 2
    ;;
esac

optval() {
    echo "qemu-lock: $1 needs a value" >&2
    exit 2
}
team='' mode='' label='' eta_min='' max_load='' settle_min=5
while [ $# -gt 0 ]; do
    case "$1" in
    --team) [ $# -ge 2 ] || optval "$1"; team=$2; shift 2 ;;
    --mode) [ $# -ge 2 ] || optval "$1"; mode=$2; shift 2 ;;
    --label) [ $# -ge 2 ] || optval "$1"; label=$2; shift 2 ;;
    --eta-min) [ $# -ge 2 ] || optval "$1"; eta_min=$2; shift 2 ;;
    --max-load) [ $# -ge 2 ] || optval "$1"; max_load=$2; shift 2 ;;
    --settle-min) [ $# -ge 2 ] || optval "$1"; settle_min=$2; shift 2 ;;
    --) shift; break ;;
    *) echo "qemu-lock: unknown option $1" >&2; exit 2 ;;
    esac
done
case "$team" in team-build | team-fix | solo) ;; *) echo "qemu-lock: --team must be team-build, team-fix or solo" >&2; exit 2 ;; esac
case "$mode" in boot | quiet) ;; *) echo "qemu-lock: --mode must be boot or quiet" >&2; exit 2 ;; esac
case "$eta_min" in '' | *[!0-9]*) echo "qemu-lock: --eta-min takes whole minutes" >&2; exit 2 ;; esac
case "$settle_min" in '' | *[!0-9]*) echo "qemu-lock: --settle-min takes whole minutes" >&2; exit 2 ;; esac
if [ -z "$max_load" ]; then
    if [ "$mode" = quiet ]; then max_load=3.0; else max_load=30; fi
fi
case "$max_load" in '' | *[!0-9.]* | *.*.*) echo "qemu-lock: --max-load takes a number" >&2; exit 2 ;; esac
[ -n "$label" ] || { echo "qemu-lock: --label is required" >&2; exit 2; }
[ $# -gt 0 ] || { echo "qemu-lock: no command after --" >&2; exit 2; }

mkdir -p "$common/aios-agent" || exit 2
if ! mkdir "$lock" 2>/dev/null; then
    echo "qemu-lock: held by:" >&2
    owner_lines >&2
    echo "state=$(holder_state)" >&2
    exit 75
fi

child=''
keep_lock=
# shellcheck disable=SC2329 # called from the traps below
release() {
    [ -n "$keep_lock" ] || rm -rf "$lock"
}
# The process $1 and everything it started, parents first, one pid per line.
# shellcheck disable=SC2329 # called from the traps below
tree_of() {
    echo "$1"
    for c in $(pgrep -P "$1" 2>/dev/null); do tree_of "$c"; done
}
# Exit status 0 when the pid $1 is one of the pids in $2 (one per line).
# shellcheck disable=SC2329 # called from the traps below
in_list() {
    printf '%s\n' "$2" | grep -qx "$1"
}
# Exit status 0 when process $1 runs (a zombie, which kill -0 still finds,
# does not count).
# shellcheck disable=SC2329 # called from the traps below
running() {
    case $(ps -o stat= -p "$1" 2>/dev/null) in
    '' | Z*) return 1 ;;
    esac
}
# Wait up to $2 seconds while any pid of the list $1 runs.
# shellcheck disable=SC2329 # called from stop_tree
wait_gone() {
    n=0
    while [ "$n" -lt "$2" ]; do
        any=
        for p in $1; do
            if running "$p"; then any=1; break; fi
        done
        [ -n "$any" ] || return 0
        sleep 1
        n=$((n + 1))
    done
}
# Stop CMD and everything it started (see the usage text): TERM every process
# of the tree but QEMU, parents first; wait while any of them still runs, up
# to the grace; then TERM what is left of the tree, re-read in case a
# process started more meanwhile, QEMU included; wait briefly; SIGKILL what
# still runs; wait briefly. Returns 1, naming the survivors on stderr and in
# the owner file, when a process of the tree still runs.
# shellcheck disable=SC2329 # called from the traps below
stop_tree() {
    tree=$(tree_of "$1")
    qemus=$(qemu_pids)
    harness=
    for p in $tree; do
        in_list "$p" "$qemus" || harness="$harness $p"
    done
    for p in $harness; do kill -TERM "$p" 2>/dev/null; done
    wait_gone "$harness" "${AIOS_QEMU_LOCK_GRACE:-15}"
    # The pids seen so far plus whatever the tree has started since.
    all=$tree
    for p in $tree; do all="$all $(tree_of "$p")"; done
    for p in $all; do running "$p" && kill -TERM "$p" 2>/dev/null; done
    wait_gone "$all" 3
    for p in $all; do running "$p" && ${AIOS_QEMU_LOCK_KILL:-kill} -KILL "$p" 2>/dev/null; done
    wait_gone "$all" 3
    left=
    for p in $all; do
        case " $left " in *" $p "*) continue ;; esac
        running "$p" && left="$left $p"
    done
    [ -z "$left" ] && return 0
    {
        echo "qemu-lock: the command's tree survived SIGKILL; the lock stays held:"
        for p in $left; do ps -o pid=,command= -p "$p"; done
    } >&2
    echo "survivors=${left# }" >>"$lock/owner" 2>/dev/null
    return 1
}
# shellcheck disable=SC2329 # called from the traps below
on_signal() {
    sig=$1
    if [ -n "$child" ] && ! stop_tree "$child"; then
        keep_lock=1
        exit 78
    fi
    release
    exit "$sig"
}
trap release EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

now=$(date +%s)
# Fail closed: a lock without an owner file reads as dead after two minutes and
# clear-stale would remove it under a live run. The EXIT trap releases the lock.
write_owner() {
    {
        echo "team=$team"
        echo "worktree=$(git rev-parse --show-toplevel)"
        echo "branch=$(git branch --show-current)"
        echo "sha=$(git rev-parse HEAD)"
        echo "mode=$mode"
        echo "label=$label"
        echo "started=$now"
        echo "eta=$((now + eta_min * 60))"
        echo "pid=$$"
    } >"$lock/owner.tmp" || return 1
    mv "$lock/owner.tmp" "$lock/owner"
}
if ! write_owner; then
    echo "qemu-lock: cannot write the owner file in $lock" >&2
    exit 2
fi

# The lock is ours now, so any QEMU still running was started without it.
# Refuse rather than measure two workloads at once; the trap releases the lock.
foreign=$(qemu_pids)
if [ -n "$foreign" ]; then
    echo "qemu-lock: qemu-system-aarch64 already running without the lock:" >&2
    for p in $foreign; do ps -o pid=,command= -p "$p" >&2; done
    exit 76
fi

# Load gate: a boot under heavy host load reports a false WEDGE or timeout, so
# defer instead. Quiet mode waits for the load to settle first.
step=${AIOS_QEMU_LOCK_SETTLE_STEP:-30}
waited=0
load=$(load1)
if [ "$mode" = quiet ]; then
    while above "${load:-0}" "$max_load" && [ "$waited" -lt $((settle_min * 60)) ]; do
        sleep "$step"
        waited=$((waited + step))
        load=$(load1)
    done
fi
if [ -z "$load" ]; then
    echo "qemu-lock: cannot read the load average" >&2
    exit 77
fi
if above "$load" "$max_load"; then
    echo "qemu-lock: deferred: load $load is above $max_load ($mode mode)" >&2
    exit 77
fi

# A stop request is a TERM to this script (pid= in the owner file). The trap
# stops CMD and every process it started, found by parent pid (stop_tree), so
# QEMU stops without touching any other session's processes.
"$@" </dev/null &
child=$!
wait "$child"
status=$?
child=''
exit "$status"
