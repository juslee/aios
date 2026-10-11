---
author: jl + claude
date: 2026-10-10
tags: [tooling]
status: final
---

# Lesson: Test qemu-lock.sh in a scratch repository with a fake QEMU, never a boot

## What happened

`scripts/agent/qemu-lock.sh` is rule 11's host QEMU lock, added in PR #234. That PR changed no kernel code, so it was tested without a single QEMU boot: a real boot takes minutes and holds the very lock under test. The cases ran on 2026-10-06 (v3 draft), 2026-10-09 (v4) and 2026-10-10 (review fixes) as throwaway smoke scripts that were never committed, so the repository has no test file for the script. Issue #243 asks for a test of each deferred qemu-lock edge case, written in "the existing shell test harness with a temp lock path", before the fix. That harness is the recipe below, which replaces the throwaway scripts.

## Why it happened

- The lock path is fixed to the script's own repository, so a test of the worktree's copy would take the host's real lock. A test needs a copy in a scratch repository and a fake QEMU, and nothing in the tree set that up.
- The cases lived only in the PR's working plan, which was deleted when the PR was distilled.
- Since PR #234, guard rule 1 and the project settings make every in-session `run` prompt and deny a fake QEMU start, so the cases cannot run unattended inside Claude Code.

## What we learned

- **The lock path cannot be configured.** The script takes `<git-common-dir>/aios-agent/qemu.lock` of the repository it lives in (`git -C "$(dirname -- "$0")"`), not of the caller's cwd, and has no override. A worktree shares the main checkout's common directory, so running the worktree's own copy in a test takes the real host lock: that blocks every team's boots, and guard rule 1 denies every QEMU start while the lock exists. Copy the script into a scratch repository to get a temporary lock path.
- **The fake QEMU must be a symlink.** On macOS a copied system binary is killed by code signing, so `cp /bin/sleep` does not work. `ln -s /bin/sleep fake/qemu-system-aarch64` does, and the wrapper's `pgrep -f '^([^ ]*/)?qemu-system-aarch64( |$)'` counts it as a QEMU process.
- **Every exit status is reachable in seconds.** The wrapper finds QEMU by argv0, reads the 1-minute load average and stops a tree with `kill`; four environment hooks replace the load, the settle step, the stop grace and the SIGKILL command. That reaches 75 (lock held), 76 (a QEMU already running without the lock), 77 (deferred under load), 78 (survivors keep the lock), 2 (usage error), 143 (TERM) and `clear-stale`'s 1.
- **Guard rule 1 sees these commands.** The recorded runs predate rule 1 on `main`. The rule matches `qemu-system-aarch64` by basename, so starting `fake/qemu-system-aarch64` from a Claude Code Bash call is denied for every caller, including from a script or heredoc the command runs (the guard reads those too). A copied wrapper called `run --team ...` gets the wrapper's caller rules: only the verifier or the main thread of an owner or solo session may call it, never while the lock of the session's repository is held; its body is read and the command after its `--` is analysed like one typed on its own, so a test's wrapped command gets every rule, and an unquoted expansion in an option value (`--label $X`) asks. Every call of a copy such as `sh q.sh` asks (checked 2026-10-11 against the guard at f294d1d), and the settings allow only `bash scripts/agent/qemu-lock.sh status` and `clear-stale`, so every `run` prompts. Run the fake-QEMU cases from the owner's own terminal, outside Claude Code. Do not rephrase a refused command to get past the guard (rule 03).

## How to avoid next time

Set-up (POSIX sh, no push):

```sh
S=$(mktemp -d) && cd "$S" && git init -q -b main . && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m s
cp "<repo>/scripts/agent/qemu-lock.sh" q.sh
mkdir fake && ln -s /bin/sleep fake/qemu-system-aarch64   # start one with: fake/qemu-system-aarch64 20 &
L=.git/aios-agent/qemu.lock
```

Test hooks, set as environment variables:

- `AIOS_QEMU_LOCK_LOADAVG` fixes the load average: 1 passes, 40 defers boot mode (limit 30), 5 defers quiet mode (limit 3.0).
- `AIOS_QEMU_LOCK_SETTLE_STEP` and `AIOS_QEMU_LOCK_GRACE` shorten the 30 s settle step and the 15 s stop grace (2 is enough).
- `AIOS_QEMU_LOCK_KILL=true` stands in for a SIGKILL that fails.

The script must stay POSIX `sh`: `shellcheck -s sh` and `dash -n` exit 0, and the cases run under both `sh` and `dash`. Pass `--mode boot --label <x> --eta-min 1` unless the table says otherwise. In the table, `LOADAVG=` and `GRACE=` stand for `AIOS_QEMU_LOCK_LOADAVG=` and `AIOS_QEMU_LOCK_GRACE=`. The wrapper writes its refusals (held, foreign QEMU, deferred, survivors, usage) to stderr. To stop a holder, run `kill -TERM "$(sed -n 's/^pid=//p' "$L/owner")"`.

| Case | How | Expected |
| --- | --- | --- |
| Free | `sh q.sh status` | `free` |
| Held | background `run --team team-fix ... -- sleep 3`, then a second `run` | `held by:`, the owner lines and `state=live`; `rc=75` |
| Load | `LOADAVG=40`, boot mode; `LOADAVG=5`, `--mode quiet --settle-min 0` | `deferred: load 40 is above 30 (boot mode)` or `deferred: load 5 is above 3.0 (quiet mode)`, `rc=77`, CMD not run |
| Normal | `LOADAVG=1 ... --team solo -- echo ran` | `ran`, `rc=0` |
| QEMU without the lock | fake QEMU running, then `run` | `already running without the lock:` and its process line, `rc=76`, CMD not run |
| Stale lock | `mkdir -p $L && printf 'team=team-fix\npid=999999\n' >$L/owner` | `state=dead`; `clear-stale` prints `cleared`, exits 0 and appends `cleared_at=... by_pid=...`, the owner lines and `--` to `.git/aios-agent/qemu-lock.log`; then `free` |
| TERM to the holder | `kill -TERM` the owner file's `pid=` during `run -- sh -c 'sleep 31'` | `free`; `pgrep -f 'sleep 31'` finds nothing |
| Command ignores TERM | CMD is `sh ign.sh` (`trap "" TERM`, then `sleep 37`), `GRACE=2`, then TERM to the holder | `rc=143`, `free`, nothing left |
| Survivor cannot be killed | same, plus `AIOS_QEMU_LOCK_KILL=true` | `rc=78`; `status` shows `state=dead` and `survivors=`; stderr says `survived SIGKILL; the lock stays held`; afterwards kill the `survivors=` pids and remove `$L` |
| `clear-stale` race | see "The `clear-stale` race" below | `not cleared: the lock changed hands`, exit 1, the live owner file intact, no `qemu.lock.clearing.*` left |
| Two concurrent `clear-stale` | two background calls on one stale lock | one `cleared` and one `not cleared: the lock changed` (or `free: nothing to clear` when the first call ends before the second starts), exits `0 1`, one log entry, `free` |
| Missing option values | `run --team`, then `--label`, then `--max-load` with no value | `<option> needs a value`, exit 2, `free` |
| Bad team | `run --team ship ... -- true` | `--team must be team-build, team-fix or solo`, exit 2 |
| Harness stops first | CMD is a script that starts its own fake QEMU, traps TERM and records whether that QEMU still runs when TERM arrives (as `aios soak` does) | `rc=143`, QEMU still running when the harness got TERM, `free`, no QEMU left. The wrapper must TERM the harness while QEMU still runs, so that `aios soak` ends its own boot; the old leaf-first stop failed this case. |

Give each case its own sleep duration, so that `pgrep -f` finds only that case's leftovers. Kill only pids you started: other sessions boot QEMU on the same host, and rule 11 forbids pattern kills.

### The `clear-stale` race

A check-then-act race becomes deterministic when a shim stands in for an external command the script runs between its check and its action. Put the shim first on `PATH`, and make it do the racing action on its first call only, using a marker file. After that it must `exec` the real binary by absolute path, because a bare name would find the shim again.

In `clear-stale`, the `pgrep` of the stale check's QEMU test is the last external command before the lock is read and moved aside. The shim swaps in the lock of a live holder there, and the check after the move finds that holder live. `$HP` is the pid of a live `run` holding `sleep 15`, started from a second copy in a second scratch repository. The heredoc is unquoted, so `$S`, `$L` and `$HP` are filled in when the shim is written, and `\$@` is escaped so that it expands when the shim runs:

```sh
mkdir -p shim && cat >shim/pgrep <<EOF
#!/bin/sh
if [ ! -f $S/swapped ]; then
    : >$S/swapped
    rm -rf $S/$L && mkdir $S/$L
    printf 'team=solo\npid=$HP\n' >$S/$L/owner
    exit 1
fi
exec /usr/bin/pgrep "\$@"
EOF
chmod 755 shim/pgrep
mkdir -p $L && printf 'team=team-fix\npid=999999\n' >$L/owner
PATH="$S/shim:$PATH" sh q.sh clear-stale; echo "rc=$?"
```

Expected: `not cleared: the lock changed hands`, `rc=1`, the owner file still holds `team=solo` and the holder's pid, and no `clearing` directory is left. Write the script that contains this with the Write tool, not as a heredoc in the command that runs it ([the shell-snippets lesson](2026-10-06-jl-testing-harness-shell-snippets.md)).

#243's fix commits these cases as one script under `scripts/agent/`, adds a case for each deferred edge case, sees it fail, then fixes the wrapper.
